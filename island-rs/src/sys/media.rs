//! Now playing: title, artist, cover, position and transport controls of the
//! current media session (Spotify, browsers, Media Player, ...), through the
//! WinRT GlobalSystemMediaTransportControls API.
//!
//! One worker thread owns the session manager. Session events only wake it up;
//! it reads the state, decodes the cover once per track and posts a `Msg` to
//! the window thread when something visible changed. Commands from the UI go
//! through the same thread, so nothing ever blocks the window thread.

use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::OnceLock;
use std::sync::Mutex;
use std::time::Instant;

use windows::core::{IInspectable, Interface};
use windows::Foundation::TypedEventHandler;
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as Session, GlobalSystemMediaTransportControlsSessionManager as Manager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
};
use windows::Storage::Streams::{Buffer, DataReader, IRandomAccessStreamReference, InputStreamOptions};
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICImagingFactory, WICBitmapDitherTypeNone, WICBitmapInterpolationModeFant,
    WICBitmapPaletteTypeCustom, WICDecodeMetadataCacheOnDemand, WICRect,
};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};

use crate::event::{post, Msg};

/// Edge length of the decoded cover in pixels (drawn at up to 120 DIP on 150 % screens).
pub const COVER_PX: u32 = 192;
/// Covers larger than this are not read.
const COVER_MAX_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MediaInfo {
    /// A media session exists (something can be controlled).
    pub active: bool,
    pub title: String,
    pub artist: String,
    pub album: String,
    /// Friendly name of the playing app ("Spotify", "Chrome").
    pub app: String,
    pub playing: bool,
    /// Position when the info was read, in milliseconds.
    pub position_ms: i64,
    /// 0 when the app reports no timeline (many browser players).
    pub duration_ms: i64,
    pub can_seek: bool,
    pub can_prev: bool,
    pub can_next: bool,
}

/// Square cover, premultiplied BGRA, plus its vibrant glow colour.
pub struct Cover {
    pub size: u32,
    pub pixels: Vec<u8>,
    pub glow: (u8, u8, u8),
}

#[derive(Clone, Copy, Debug)]
pub enum Command {
    PlayPause,
    Next,
    Previous,
    /// Jump to this position in milliseconds.
    Seek(i64),
}

enum Req {
    Refresh,
    SessionChanged,
    Do(Command),
}

static TX: OnceLock<Mutex<Sender<Req>>> = OnceLock::new();

/// Starts the worker. Later calls do nothing.
pub fn start() {
    let (tx, rx) = channel();
    if TX.set(Mutex::new(tx.clone())).is_err() {
        return;
    }
    let _ = std::thread::Builder::new().name("IslandMedia".into()).stack_size(512 * 1024).spawn(move || unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        run(tx, rx);
    });
}

/// Sends a transport command. Returns false when no session can take it (caller may use media keys).
pub fn command(cmd: Command) -> bool {
    let Some(tx) = TX.get() else { return false };
    tx.lock().map(|tx| tx.send(Req::Do(cmd)).is_ok()).unwrap_or(false)
}

struct Worker {
    tx: Sender<Req>,
    manager: Manager,
    session: Option<Session>,
    tokens: [i64; 3],
    last: MediaInfo,
    /// When `last` was posted: lets the worker predict the position and skip timeline noise.
    last_at: Instant,
    cover_key: String,
    cover_loaded: bool,
}

fn run(tx: Sender<Req>, rx: Receiver<Req>) {
    let Ok(manager) = Manager::RequestAsync().and_then(|op| op.join()) else { return };
    {
        let tx = tx.clone();
        let _ = manager.CurrentSessionChanged(&TypedEventHandler::new(move |_, _| {
            let _ = tx.send(Req::SessionChanged);
            Ok(())
        }));
    }
    {
        let tx = tx.clone();
        let _ = manager.SessionsChanged(&TypedEventHandler::new(move |_, _| {
            let _ = tx.send(Req::SessionChanged);
            Ok(())
        }));
    }
    let mut w = Worker { tx, manager, session: None, tokens: [0; 3], last: MediaInfo::default(), last_at: Instant::now(), cover_key: String::new(), cover_loaded: false };
    w.bind_session();
    w.refresh(true);

    while let Ok(first) = rx.recv() {
        // Events come in bursts: handle the burst once.
        let mut rebind = false;
        let mut refresh = false;
        let mut pending = vec![first];
        while let Ok(more) = rx.try_recv() {
            pending.push(more);
        }
        for req in pending {
            match req {
                Req::Refresh => refresh = true,
                Req::SessionChanged => rebind = true,
                Req::Do(cmd) => {
                    w.execute(cmd);
                    refresh = true;
                }
            }
        }
        if rebind {
            w.bind_session();
        }
        if rebind || refresh {
            w.refresh(rebind);
        }
    }
}

impl Worker {
    fn bind_session(&mut self) {
        let current = self.manager.GetCurrentSession().ok();
        let same = match (&current, &self.session) {
            (Some(a), Some(b)) => a.as_raw() == b.as_raw(),
            (None, None) => true,
            _ => false,
        };
        if same {
            return;
        }
        if let Some(old) = self.session.take() {
            let _ = old.RemoveMediaPropertiesChanged(self.tokens[0]);
            let _ = old.RemovePlaybackInfoChanged(self.tokens[1]);
            let _ = old.RemoveTimelinePropertiesChanged(self.tokens[2]);
        }
        if let Some(session) = &current {
            let handler = || {
                let tx = self.tx.clone();
                move |_: windows::core::Ref<'_, Session>, _: windows::core::Ref<'_, IInspectable>| {
                    let _ = tx.send(Req::Refresh);
                    Ok(())
                }
            };
            let h0 = handler();
            let h1 = handler();
            let h2 = handler();
            self.tokens[0] = session
                .MediaPropertiesChanged(&TypedEventHandler::new(move |s, _| h0(s, windows::core::Ref::default())))
                .unwrap_or(0);
            self.tokens[1] = session
                .PlaybackInfoChanged(&TypedEventHandler::new(move |s, _| h1(s, windows::core::Ref::default())))
                .unwrap_or(0);
            self.tokens[2] = session
                .TimelinePropertiesChanged(&TypedEventHandler::new(move |s, _| h2(s, windows::core::Ref::default())))
                .unwrap_or(0);
        }
        self.session = current;
        self.cover_loaded = false;
        self.cover_key.clear();
    }

    fn execute(&mut self, cmd: Command) {
        let Some(s) = &self.session else {
            fallback_key(cmd);
            return;
        };
        let ok = match cmd {
            Command::PlayPause => s.TryTogglePlayPauseAsync().and_then(|op| op.join()),
            Command::Next => s.TrySkipNextAsync().and_then(|op| op.join()),
            Command::Previous => s.TrySkipPreviousAsync().and_then(|op| op.join()),
            // TimeSpan ticks are 100 ns.
            Command::Seek(ms) => s.TryChangePlaybackPositionAsync(ms.max(0) * 10_000).and_then(|op| op.join()),
        };
        if !ok.unwrap_or(false) && !matches!(cmd, Command::Seek(_)) {
            fallback_key(cmd);
        }
    }

    fn refresh(&mut self, force: bool) {
        let mut info = MediaInfo::default();
        let mut thumbnail: Option<IRandomAccessStreamReference> = None;
        if let Some(s) = &self.session {
            info.active = true;
            info.app = s.SourceAppUserModelId().map(|id| friendly_app(&id.to_string_lossy())).unwrap_or_default();
            if let Ok(props) = s.TryGetMediaPropertiesAsync().and_then(|op| op.join()) {
                info.title = props.Title().map(|t| t.to_string_lossy()).unwrap_or_default();
                info.artist = props.Artist().map(|t| t.to_string_lossy()).unwrap_or_default();
                if info.artist.is_empty() {
                    info.artist = props.AlbumArtist().map(|t| t.to_string_lossy()).unwrap_or_default();
                }
                info.album = props.AlbumTitle().map(|t| t.to_string_lossy()).unwrap_or_default();
                thumbnail = props.Thumbnail().ok();
            }
            if let Ok(playback) = s.GetPlaybackInfo() {
                info.playing = playback.PlaybackStatus().map(|st| st == Status::Playing).unwrap_or(false);
                if let Ok(c) = playback.Controls() {
                    info.can_seek = c.IsPlaybackPositionEnabled().unwrap_or(false);
                    info.can_prev = c.IsPreviousEnabled().unwrap_or(false);
                    info.can_next = c.IsNextEnabled().unwrap_or(false);
                }
            }
            if let Ok(t) = s.GetTimelineProperties() {
                let start = t.StartTime().map(|d| d.Duration).unwrap_or(0);
                let end = t.EndTime().map(|d| d.Duration).unwrap_or(0);
                let mut pos = t.Position().map(|d| d.Duration).unwrap_or(0) - start;
                // Position is as of LastUpdatedTime: move it forward to now while playing.
                if info.playing {
                    if let Ok(updated) = t.LastUpdatedTime() {
                        let now = filetime_now();
                        if updated.UniversalTime > 0 && now > updated.UniversalTime {
                            pos += now - updated.UniversalTime;
                        }
                    }
                }
                info.duration_ms = ((end - start) / 10_000).max(0);
                info.position_ms = (pos / 10_000).clamp(0, info.duration_ms.max(0));
                if info.duration_ms == 0 {
                    info.can_seek = false;
                }
            }
        }

        // Cover: decoded once per track, retried while the app has not delivered it yet.
        let key = format!("{}\u{1}{}\u{1}{}", info.title, info.artist, info.album);
        if key != self.cover_key {
            self.cover_key = key;
            self.cover_loaded = false;
            post(Msg::MediaCover(None));
        }
        if !self.cover_loaded && info.active {
            if let Some(cover) = thumbnail.and_then(|t| load_cover(&t)) {
                self.cover_loaded = true;
                post(Msg::MediaCover(Some(cover)));
            }
        }

        if force || self.changed(&info) {
            self.last = info.clone();
            self.last_at = Instant::now();
            post(Msg::Media(info));
        }
    }

    /// Visible change: anything but a position that moved as predicted.
    fn changed(&self, info: &MediaInfo) -> bool {
        let a = &self.last;
        if a.active != info.active
            || a.title != info.title
            || a.artist != info.artist
            || a.album != info.album
            || a.app != info.app
            || a.playing != info.playing
            || a.duration_ms != info.duration_ms
            || a.can_seek != info.can_seek
            || a.can_prev != info.can_prev
            || a.can_next != info.can_next
        {
            return true;
        }
        let predicted = a.position_ms + if a.playing { self.last_at.elapsed().as_millis() as i64 } else { 0 };
        (predicted - info.position_ms).abs() > 1500
    }
}

/// Media keys when no session answers.
fn fallback_key(cmd: Command) {
    use windows::Win32::UI::Input::KeyboardAndMouse::{keybd_event, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, VK_MEDIA_NEXT_TRACK, VK_MEDIA_PLAY_PAUSE, VK_MEDIA_PREV_TRACK};
    let vk = match cmd {
        Command::PlayPause => VK_MEDIA_PLAY_PAUSE,
        Command::Next => VK_MEDIA_NEXT_TRACK,
        Command::Previous => VK_MEDIA_PREV_TRACK,
        Command::Seek(_) => return,
    };
    unsafe {
        keybd_event(vk.0 as u8, 0, KEYEVENTF_EXTENDEDKEY, 0);
        keybd_event(vk.0 as u8, 0, KEYEVENTF_EXTENDEDKEY | KEYEVENTF_KEYUP, 0);
    }
}

/// Current time as a WinRT DateTime (100 ns since 1601, UTC).
fn filetime_now() -> i64 {
    let ft = unsafe { windows::Win32::System::SystemInformation::GetSystemTimeAsFileTime() };
    ((ft.dwHighDateTime as i64) << 32) | ft.dwLowDateTime as i64
}

/// "Spotify.exe" / "SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify" / "MSEdge" -> readable name.
fn friendly_app(id: &str) -> String {
    let lower = id.to_lowercase();
    const KNOWN: [(&str, &str); 10] = [
        ("spotify", "Spotify"),
        ("msedge", "Edge"),
        ("chrome", "Chrome"),
        ("firefox", "Firefox"),
        ("zunemusic", "Medienwiedergabe"),
        ("zunevideo", "Filme & TV"),
        ("vlc", "VLC"),
        ("brave", "Brave"),
        ("opera", "Opera"),
        ("applemusic", "Apple Music"),
    ];
    if let Some((_, name)) = KNOWN.iter().find(|(k, _)| lower.contains(k)) {
        return name.to_string();
    }
    let last = id.rsplit(['!', '\\', '/']).next().unwrap_or(id);
    let last = last.strip_suffix(".exe").or_else(|| last.strip_suffix(".EXE")).unwrap_or(last);
    last.to_string()
}

fn read_stream(reference: &IRandomAccessStreamReference) -> Option<Vec<u8>> {
    let stream = reference.OpenReadAsync().ok()?.join().ok()?;
    let size = stream.Size().ok()?;
    if size == 0 || size > COVER_MAX_BYTES {
        return None;
    }
    let buffer = Buffer::Create(size as u32).ok()?;
    let filled = stream.ReadAsync(&buffer, size as u32, InputStreamOptions::None).ok()?.join().ok()?;
    let len = filled.Length().ok()? as usize;
    let reader = DataReader::FromBuffer(&filled).ok()?;
    let mut bytes = vec![0u8; len];
    reader.ReadBytes(&mut bytes).ok()?;
    Some(bytes)
}

/// Decodes the thumbnail and cuts a centred square of COVER_PX.
fn load_cover(reference: &IRandomAccessStreamReference) -> Option<Cover> {
    let bytes = read_stream(reference)?;
    unsafe {
        let factory: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;
        let stream = factory.CreateStream().ok()?;
        stream.InitializeFromMemory(&bytes).ok()?;
        let decoder = factory.CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand).ok()?;
        let frame = decoder.GetFrame(0).ok()?;
        let (mut w, mut h) = (0u32, 0u32);
        frame.GetSize(&mut w, &mut h).ok()?;
        if w == 0 || h == 0 {
            return None;
        }
        // Short side becomes COVER_PX; the long side is cropped around the centre.
        let scale = COVER_PX as f64 / w.min(h) as f64;
        let (sw, sh) = (((w as f64 * scale).round() as u32).max(COVER_PX), ((h as f64 * scale).round() as u32).max(COVER_PX));
        let scaler = factory.CreateBitmapScaler().ok()?;
        scaler.Initialize(&frame, sw, sh, WICBitmapInterpolationModeFant).ok()?;
        let converter = factory.CreateFormatConverter().ok()?;
        converter
            .Initialize(&scaler, &GUID_WICPixelFormat32bppPBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom)
            .ok()?;
        let rect = WICRect { X: ((sw - COVER_PX) / 2) as i32, Y: ((sh - COVER_PX) / 2) as i32, Width: COVER_PX as i32, Height: COVER_PX as i32 };
        let mut pixels = vec![0u8; (COVER_PX * COVER_PX * 4) as usize];
        converter.CopyPixels(&rect, COVER_PX * 4, &mut pixels).ok()?;

        let glow = cover_glow_color(&pixels);
        Some(Cover { size: COVER_PX, pixels, glow })
    }
}

/// Extracts a vibrant dominant glow colour from premultiplied BGRA pixels.
fn cover_glow_color(pixels: &[u8]) -> (u8, u8, u8) {
    const BINS: usize = 12;
    let (mut sr, mut sg, mut sb, mut n) = (0u64, 0u64, 0u64, 0u64);
    let mut bin_w = [0.0f32; BINS];
    let mut bin_r = [0.0f32; BINS];
    let mut bin_g = [0.0f32; BINS];
    let mut bin_b = [0.0f32; BINS];

    for px in pixels.chunks_exact(4).step_by(7) {
        let a = px[3];
        let (b, g, r) = (px[0], px[1], px[2]);
        sb += b as u64;
        sg += g as u64;
        sr += r as u64;
        n += 1;
        if a < 128 {
            continue;
        }
        let af = a as f32;
        let rf = (r as f32 / af).min(1.0);
        let gf = (g as f32 / af).min(1.0);
        let bf = (b as f32 / af).min(1.0);
        let max = rf.max(gf).max(bf);
        let min = rf.min(gf).min(bf);
        let delta = max - min;
        let sat = if max > 0.001 { delta / max } else { 0.0 };
        if sat <= 0.16 || max <= 0.12 || (max > 0.94 && sat < 0.14) {
            continue;
        }
        let hue = if (max - rf).abs() < 1e-5 {
            ((gf - bf) / delta).rem_euclid(6.0)
        } else if (max - gf).abs() < 1e-5 {
            (bf - rf) / delta + 2.0
        } else {
            (rf - gf) / delta + 4.0
        };
        let bin = ((hue / 6.0 * BINS as f32).floor() as usize).min(BINS - 1);
        let w = sat * sat * (0.35 + 0.65 * max);
        bin_w[bin] += w;
        bin_r[bin] += rf * w;
        bin_g[bin] += gf * w;
        bin_b[bin] += bf * w;
    }

    let n = n.max(1);
    let avg = ((sr / n) as u8, (sg / n) as u8, (sb / n) as u8);

    let mut best = 0usize;
    let mut best_score = 0.0f32;
    for i in 0..BINS {
        let prev = (i + BINS - 1) % BINS;
        let next = (i + 1) % BINS;
        let score = bin_w[i] + 0.5 * bin_w[prev] + 0.5 * bin_w[next];
        if score > best_score {
            best_score = score;
            best = i;
        }
    }

    let (mut gr, mut gg, mut gb) = if best_score > 1.5 {
        let prev = (best + BINS - 1) % BINS;
        let next = (best + 1) % BINS;
        (
            (bin_r[best] + 0.5 * bin_r[prev] + 0.5 * bin_r[next]) / best_score,
            (bin_g[best] + 0.5 * bin_g[prev] + 0.5 * bin_g[next]) / best_score,
            (bin_b[best] + 0.5 * bin_b[prev] + 0.5 * bin_b[next]) / best_score,
        )
    } else {
        (avg.0 as f32 / 255.0, avg.1 as f32 / 255.0, avg.2 as f32 / 255.0)
    };

    // Lift dark covers so the background glow stays visible.
    let max_c = gr.max(gg).max(gb);
    if max_c < 0.08 {
        return (170, 180, 200);
    }
    if max_c < 0.72 {
        let scale = 0.72 / max_c;
        gr = (gr * scale).min(1.0);
        gg = (gg * scale).min(1.0);
        gb = (gb * scale).min(1.0);
    }

    ((gr * 255.0).round() as u8, (gg * 255.0).round() as u8, (gb * 255.0).round() as u8)
}

/// Position as shown: the posted one plus the time since it arrived, while playing.
pub fn position_now(info: &MediaInfo, received: Instant) -> i64 {
    let mut pos = info.position_ms;
    if info.playing {
        pos += received.elapsed().as_millis() as i64;
    }
    if info.duration_ms > 0 {
        pos.min(info.duration_ms)
    } else {
        pos
    }
}

/// "3:07" / "1:02:33".
pub fn format_ms(ms: i64) -> String {
    let s = (ms / 1000).max(0);
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::cover_glow_color;

    fn solid_bgra(r: u8, g: u8, b: u8, count: usize) -> Vec<u8> {
        let mut v = Vec::with_capacity(count * 4);
        for _ in 0..count {
            v.extend_from_slice(&[b, g, r, 255]);
        }
        v
    }

    #[test]
    fn dark_cover_with_accent_picks_vibrant_accent() {
        let mut px = solid_bgra(10, 10, 12, 800);
        px.extend_from_slice(&solid_bgra(220, 30, 50, 200));
        let (r, g, b) = cover_glow_color(&px);
        assert!(r > 180 && g < 60 && b < 80, "got ({r}, {g}, {b})");
    }

    #[test]
    fn deep_color_is_lifted_for_visibility() {
        let px = solid_bgra(15, 25, 65, 500);
        let (r, g, b) = cover_glow_color(&px);
        assert!(b >= 175 && b > r && b > g, "got ({r}, {g}, {b})");
    }

    #[test]
    fn pitch_black_cover_falls_back_to_neutral_glow() {
        let px = solid_bgra(0, 0, 0, 500);
        let (r, g, b) = cover_glow_color(&px);
        assert_eq!((r, g, b), (170, 180, 200));
    }
}
