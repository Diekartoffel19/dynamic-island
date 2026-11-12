//! Island state and behaviour: shape spring, the three views, toasts, tabs,
//! focus timer, colour picker and file shelf.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Direct2D::ID2D1Bitmap1;
use windows::Win32::Graphics::Gdi::{CreateRectRgn, ScreenToClient, SetWindowRgn};
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    keybd_event, GetAsyncKeyState, GetCapture, ReleaseCapture, SetCapture, TrackMouseEvent, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP,
    TME_LEAVE, TRACKMOUSEEVENT, VK_LBUTTON, VK_MEDIA_NEXT_TRACK, VK_MEDIA_PLAY_PAUSE, VK_MEDIA_PREV_TRACK, VK_MENU,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DestroyWindow, GetCursorPos, GetSystemMetrics, GetWindowLongPtrW, KillTimer, PostMessageW, SendMessageW, SetLayeredWindowAttributes,
    SetTimer, SetWindowLongPtrW, SetWindowPos, GWL_EXSTYLE, HTCAPTION, HWND_TOPMOST, LWA_ALPHA, SM_CXSCREEN, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_NCLBUTTONDOWN, WM_RBUTTONDOWN, WM_RBUTTONUP,
    WS_EX_LAYERED, WS_EX_TRANSPARENT,
};

use crate::event::{EventType, IslandEvent, Msg};
use crate::net::airdrop::{self, SendOutcome};
use crate::net::qr::{self, Qr};
use crate::net::share_server::ShareServer;
use crate::render::anim::{Anims, Ease, Spring};
use crate::render::scene::{self, CX, WIN_H, WIN_W};
use crate::render::{Gfx, Rect, ACCENTS};
use crate::shell::icons::IconPixels;
use crate::shell::picker::{self, Loupe, LOUPE};
use crate::shell::tooltip::Tooltip;
use crate::shell::{dragdrop, tray};
use crate::sys::audio::Audio;
use crate::sys::bluetooth;
use crate::sys::telemetry::CpuSampler;
use crate::sys::radio::{self, Status as RadioStatus};
use crate::sys::media::{self, Command as MediaCommand, Cover, MediaInfo};
use crate::sys::weather::{self, Weather, WeatherError};
use crate::sys::{clipboard, keys, network, power, telemetry};
use crate::settings::{self, SettingsWindow};
use crate::{state, window};


// ---- Shape per state: width, height, corner radius ----
const COMPACT: (f64, f64, f64) = (190.0, 34.0, 17.0);
const TOAST: (f64, f64, f64) = (344.0, 58.0, 29.0);
const EXPANDED: (f64, f64, f64) = (420.0, 198.0, 38.0);
/// The start tab is taller: clock, notifications, quick actions, footer.
const EXPANDED_HOME: (f64, f64, f64) = (420.0, 236.0, 38.0);
/// One-line level display for volume changes.
const TOAST_HUD: (f64, f64, f64) = (300.0, 40.0, 20.0);
/// The media tab: big cover, title, transport controls, timeline.
const EXPANDED_MEDIA: (f64, f64, f64) = (420.0, 228.0, 38.0);
/// The reminder tab: input field plus three reminders.
const EXPANDED_REMINDERS: (f64, f64, f64) = (420.0, 236.0, 38.0);
/// Peak level above which the output counts as playing.
const PLAYING_PEAK: f32 = 0.002;
/// Hover delay before the island opens, per setting.
const OPEN_DELAYS: [u32; 3] = [90, 260, 550];
/// Toast duration factor, per setting.
const TOAST_LENGTHS: [f32; 3] = [0.65, 1.0, 1.7];
/// Notifications kept for the start tab.
const MAX_NOTICES: usize = 20;

// ---- Timers ----
pub const T_CLOCK: usize = 1;
pub const T_TOAST: usize = 2;
pub const T_HOVER: usize = 3;
pub const T_LEAVE: usize = 4;
pub const T_FOCUS: usize = 5;
pub const T_PICK: usize = 6;
pub const T_TELEMETRY: usize = 7;
pub const T_EQ: usize = 8;
pub const T_KEYS: usize = 9;
pub const T_NET: usize = 10;
pub const T_MODAL: usize = 11;
pub const T_TRIM: usize = 12;
pub const T_PROBE: usize = 13;
pub const T_REMINDER: usize = 14;
pub const T_CARET: usize = 15;
pub const T_MEDIA: usize = 16;
pub const T_WEATHER: usize = 17;

pub const TAB_MEDIA: usize = 1;
pub const TAB_COLOR: usize = 2;
pub const TAB_FILES: usize = 3;
pub const TAB_REMINDERS: usize = 5;
pub const TAB_CONTROLS: usize = 6;
pub const TAB_COUNT: usize = 7;
/// Longest city name that can be typed.
const CITY_MAX: usize = 40;
/// Width of one tab in the tab bar.
pub const TAB_W: f32 = 29.0;
/// Longest reminder text that can be typed.
const INPUT_MAX: usize = 80;
/// A peer that has not answered a probe for this long leaves the picker.
const PEER_TIMEOUT: Duration = Duration::from_secs(9);
const SHARE_DEFAULT_HINT: &str = "Ablage laden, Dateien und Text an den PC senden";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum VisualState {
    Compact = 0,
    Toast = 1,
    Expanded = 2,
}

/// Everything that can be hovered or clicked.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Id {
    None,
    Tab(u8),
    Pin,
    Prev,
    Play,
    Next,
    Mute,
    Slider,
    Hex,
    Pipette,
    Rgb,
    Hsl,
    History(u8),
    Phone,
    FilesClear,
    Chip(u32),
    ChipLink(u32),
    ChipRemove(u32),
    ShareCopy,
    ShareFolder,
    ShareClose,
    Preset(u8),
    FocusToggle,
    FocusReset,
    /// 0 = Wi-Fi, 1 = Bluetooth, 2 = nearby visibility.
    Toggle(u8),
    Nearby,
    ChipSend(u32),
    Peer(u8),
    NearbyClose,
    OfferAccept,
    OfferDecline,
    /// A notification card on the start tab; a click dismisses it.
    Notice(u32),
    /// Quick action pill on the start tab.
    Quick(u8),
    /// Accent preset swatch in the settings.
    Accent(u8),
    /// On/off switch in the settings (see `SWITCHES`).
    Switch(u8),
    /// Segmented control `.0`, option `.1`.
    Seg(u8, u8),
    ClearNotices,
    ResetSettings,
    ReminderInput,
    ReminderAdd,
    ReminderRemove(u32),
    /// Gear in the island header: opens the settings window.
    OpenSettings,
    SettingsClose,
    SettingsMin,
    SettingsTab(u8),
    /// Shortcut field in the settings: click, then press the new combination.
    HotkeyRecord,
    /// City field of the weather in the settings.
    CityInput,
    MediaPrev,
    MediaPlay,
    MediaNext,
    /// Timeline of the media tab: a click jumps there.
    MediaSeek,
}

/// Switches of the settings window: label, description.
pub const SWITCHES: [(&str, &str); 8] = [
    ("Hover öffnet", "Sonst per Klick öffnen"),
    ("Lautstärke-Pille", "Bei Lautstärke-Änderung"),
    ("Kopier-Meldung", "Toast beim Kopieren"),
    ("Ungelesen-Zahl", "Neue Meldungen im Pill"),
    ("Musik-Balken", "Nur wenn Audio läuft"),
    ("Wetter im Pill", "Temperatur neben der Uhr"),
    ("Per Tastenkürzel ausblenden", "Island verstecken und wieder zeigen"),
    ("Innen-Leuchten", "Farbschein nur innerhalb der Island"),
];
/// Switch index of the hide shortcut.
pub const SWITCH_HOTKEY: u8 = 6;
/// Switch index of the inner glow.
pub const SWITCH_GLOW: u8 = 7;
/// Default shortcut: Ctrl + Alt + I.
const HOTKEY_DEFAULT: (u32, u32) = (settings::MOD_CONTROL | settings::MOD_ALT, 0x49);

/// Animated values.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    ViewOpacity(u8),
    ViewScale(u8),
    ViewBlur(u8),
    TabOpacity(u8),
    TabShift(u8),
    TabHighlight,
    SettingsTabOpacity(u8),
    SettingsTabShift(u8),
    SettingsTabHighlight,
    Eq(u8),
    /// 0 = CPU, 1 = RAM, 2 = battery, 3 = toast progress.
    Bar(u8),
    Hover(Id),
    Press(Id),
    PopScale(u32),
    PopOpacity(u32),
    ChipActions(u32),
    SharePanel,
    /// New toast content slides in while the toast is already open.
    ToastSwap,
    /// Knob position of a settings switch, 0 = off, 1 = on.
    SwitchPos(u8),
    /// Highlight position of a segmented control, in option units.
    SegPos(u8),
    /// Music bars in the compact pill, 0 = hidden, 1 = shown.
    MusicVis,
    /// Album cover background glow opacity and RGB channels (0..=1).
    GlowVis,
    GlowR,
    GlowG,
    GlowB,
}

pub struct Reminder {
    pub id: u32,
    pub text: String,
    /// Unix seconds.
    pub due: u64,
}

pub fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Local wall-clock text for a Unix time: "18:30", or "morgen 07:00" / "Fr 07:00" further out.
pub fn due_text(due: u64) -> String {
    let now = unsafe { GetLocalTime() };
    let now_of_day = now.wHour as i64 * 3600 + now.wMinute as i64 * 60 + now.wSecond as i64;
    let at = now_of_day + due as i64 - unix_now() as i64;
    let day = at.div_euclid(86400);
    let of_day = at.rem_euclid(86400);
    let clock = format!("{:02}:{:02}", of_day / 3600, (of_day % 3600) / 60);
    match day {
        0 => clock,
        1 => format!("morgen {clock}"),
        _ => {
            const DAYS: [&str; 7] = ["So", "Mo", "Di", "Mi", "Do", "Fr", "Sa"];
            format!("{} {clock}", DAYS[((now.wDayOfWeek as i64 + day) % 7) as usize])
        }
    }
}

pub struct Hit {
    pub id: Id,
    pub rect: Rect,
}

pub struct ShelfItem {
    pub id: u32,
    pub path: String,
    pub name: String,
    pub icon: Option<IconPixels>,
    pub is_dir: bool,
    pub bitmap: Option<ID2D1Bitmap1>,
}

pub struct Toast {
    pub title: String,
    pub message: String,
    pub badge: String,
    pub kind: EventType,
    pub has_progress: bool,
    /// Incoming nearby offer: the toast shows accept / decline buttons.
    pub offer: Option<u32>,
}

/// A past alert, shown on the start tab.
pub struct Notice {
    pub id: u32,
    pub title: String,
    pub message: String,
    pub kind: EventType,
    /// "12:30" when it arrived.
    pub time: String,
}

pub struct Peer {
    pub id: String,
    pub name: String,
    addr: SocketAddr,
    seen: Instant,
}

#[derive(Default)]
pub struct Telemetry {
    pub cpu: i32,
    pub ram: i32,
    pub ram_used_gb: f64,
    pub ram_total_gb: f64,
    pub battery: i32,
    pub charging: bool,
    pub has_battery: bool,
    pub volume: i32,
    pub muted: bool,
}

struct BtDevice {
    name: String,
    connected: bool,
}

pub type Rgb = (u8, u8, u8);

pub struct App {
    pub hwnd: HWND,
    pub scale: f32,
    pub gfx: Option<Gfx>,
    pub anims: Anims<Key>,
    rx: Receiver<Msg>,
    dirty: bool,

    // Shape spring
    sw: Spring,
    sh: Spring,
    sr: Spring,
    spring_active: bool,
    spring_last: f64,
    region: (i32, i32, i32, i32),

    // State
    pub state: VisualState,
    view_visible: [bool; 3],
    pub pinned: bool,
    pub click_through: bool,
    hold_open: bool,
    drag_over: bool,
    mouse_over: bool,
    tracking_leave: bool,

    // Widgets
    pub hits: Vec<Hit>,
    pub hot: Id,
    pressed: Id,
    slider_drag: bool,
    chip_drag: Option<(u32, f32, f32)>,
    tooltip: Option<Tooltip>,

    // Content
    pub tab: usize,
    pub clock_text: String,
    pub date_text: String,
    pub date_long: String,
    pub toast: Toast,
    /// The current toast is the compact volume display.
    pub toast_hud: bool,
    pub notices: Vec<Notice>,
    next_notice_id: u32,
    /// Notifications that arrived since the start tab was last seen.
    pub unread: u32,

    // Settings, saved in settings.txt
    pub opt_hover: bool,
    pub opt_hud: bool,
    pub opt_clip: bool,
    pub opt_bell: bool,
    pub opt_eq: bool,
    /// Index into OPEN_DELAYS.
    pub open_delay: u8,
    /// Index into TOAST_LENGTHS.
    pub toast_len: u8,
    pub opt_weather_pill: bool,
    pub opt_hotkey: bool,
    pub opt_glow: bool,
    /// 0 = Bei Musik, 1 = Nur Medien, 2 = Immer.
    pub glow_mode: u8,
    /// 0 = Dezent, 1 = Normal, 2 = Kräftig.
    pub glow_strength: u8,
    pub settings_tab: usize,
    /// RegisterHotKey modifiers and virtual key of the hide shortcut.
    pub hotkey_mods: u32,
    pub hotkey_vk: u32,
    /// The settings window waits for a new key combination.
    pub hotkey_recording: bool,
    /// Problem with the shortcut ("bereits belegt"), shown in the settings.
    pub hotkey_problem: Option<String>,
    /// The island is hidden (shortcut or tray).
    pub hidden: bool,
    pub settings: Option<SettingsWindow>,

    // Weather
    pub weather: Option<Weather>,
    pub weather_error: Option<WeatherError>,
    pub weather_loading: bool,
    pub city: String,
    pub city_input: String,
    pub city_focus: bool,

    // Now playing
    pub media: MediaInfo,
    pub media_at: Instant,
    pub cover: Option<Cover>,
    pub cover_bitmap: Option<ID2D1Bitmap1>,
    media_tick_on: bool,

    // Reminders
    pub reminders: Vec<Reminder>,
    next_reminder_id: u32,
    pub input: String,
    pub input_focus: bool,
    pub caret_on: bool,
    /// Window that had the focus before the reminder field took it.
    prev_foreground: isize,

    // Compact status
    pub cam_on: bool,
    pub mic_on: bool,
    /// Something is playing: the music bars show.
    pub music: bool,
    silent_ms: u32,
    pub tel: Telemetry,
    cpu: CpuSampler,
    telemetry_on: bool,
    eq_on: bool,
    rng: u32,

    // System state for change detection
    audio: Option<Audio>,
    last_volume: i32,
    last_muted: bool,
    last_caps: bool,
    last_num: bool,
    last_line: u8,
    battery_warned: bool,
    last_online: bool,
    bt_devices: HashMap<String, BtDevice>,
    bt_recent: HashMap<u64, (bool, Instant)>,
    _bt_watchers: Option<bluetooth::Watchers>,

    // Focus timer
    pub focus_total: i32,
    pub focus_remaining: i32,
    pub focus_running: bool,

    // Colour picker
    pub color: Rgb,
    pub shown_color: Rgb,
    color_before_pick: Rgb,
    pub history: Vec<Rgb>,
    pub color_hint: String,
    pub picking: bool,
    pick_result: u8, // 0 = running, 1 = picked, 2 = cancelled
    loupe: Option<Loupe>,
    pub loupe_pixels: [u32; LOUPE * LOUPE],

    // File shelf
    pub shelf: Vec<ShelfItem>,
    next_item_id: u32,
    pub files_scroll: f32,
    pub files_info: String,
    self_drag: bool,
    share: Arc<ShareServer>,
    pub share_open: bool,
    share_url: Option<String>,
    pub share_hint: String,
    pub qr: Option<Qr>,
    ignore_clipboard_until: Instant,

    // Controls tab
    pub radio_wifi: RadioStatus,
    pub radio_bt: RadioStatus,
    pub wifi: Option<(String, u8)>,
    pub net_name: String,
    pub local_ip: String,

    // Nearby share
    pub nearby_visible: bool,
    pub nearby_open: bool,
    pub nearby_files: Vec<String>,
    pub nearby_bytes: u64,
    pub nearby_hint: String,
    pub peers: Vec<Peer>,
    /// Peer name and percent of the transfer in progress.
    pub sending: Option<(String, i32)>,
    pending_offer: Option<u32>,
    deferred_alert: Option<IslandEvent>,

    /// Hand the working set back once the island has settled in compact.
    ws_trim_due: bool,
}

/// "12,4 MB" style size.
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{:.1} {}", value, UNITS[unit]).replace('.', ",")
    }
}

fn truncate_preview(text: &str) -> String {
    let clean = text.replace(['\r', '\n'], " ").trim().to_string();
    if clean.chars().count() > 38 {
        clean.chars().take(38).collect::<String>() + "..."
    } else {
        clean
    }
}

pub fn format_seconds(total: i32) -> String {
    format!("{:02}:{:02}", total / 60, total % 60)
}

pub fn to_hex(c: Rgb) -> String {
    format!("#{:02X}{:02X}{:02X}", c.0, c.1, c.2)
}

pub fn to_rgb(c: Rgb) -> String {
    format!("{}, {}, {}", c.0, c.1, c.2)
}

pub fn to_hsl(c: Rgb) -> String {
    let (r, g, b) = (c.0 as f64 / 255.0, c.1 as f64 / 255.0, c.2 as f64 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    let (mut h, mut s) = (0.0, 0.0);
    let d = max - min;
    if d > 0.0 {
        s = d / (1.0 - (2.0 * l - 1.0).abs());
        h = if max == r {
            ((g - b) / d) % 6.0
        } else if max == g {
            (b - r) / d + 2.0
        } else {
            (r - g) / d + 4.0
        };
        h *= 60.0;
        if h < 0.0 {
            h += 360.0;
        }
    }
    format!("{}°, {}%, {}%", (h.round() as i32) % 360, (s * 100.0).round() as i32, (l * 100.0).round() as i32)
}

impl App {
    pub fn new(hwnd: HWND, scale: f32, rx: Receiver<Msg>) -> App {
        let accent: Rgb = (90, 162, 255);
        App {
            hwnd,
            scale,
            gfx: None,
            anims: Anims::new(),
            rx,
            dirty: true,
            sw: Spring::new(COMPACT.0),
            sh: Spring::new(COMPACT.1),
            sr: Spring::new(COMPACT.2),
            spring_active: false,
            spring_last: 0.0,
            region: (0, 0, 0, 0),
            state: VisualState::Compact,
            view_visible: [true, false, false],
            pinned: false,
            click_through: false,
            hold_open: false,
            drag_over: false,
            mouse_over: false,
            tracking_leave: false,
            hits: Vec::new(),
            hot: Id::None,
            pressed: Id::None,
            slider_drag: false,
            chip_drag: None,
            tooltip: None,
            tab: 0,
            clock_text: String::new(),
            date_text: String::new(),
            date_long: String::new(),
            toast_hud: false,
            notices: Vec::new(),
            next_notice_id: 1,
            unread: 0,
            opt_hover: true,
            opt_hud: true,
            opt_clip: true,
            opt_bell: true,
            opt_eq: true,
            open_delay: 0,
            toast_len: 1,
            opt_weather_pill: true,
            opt_hotkey: false,
            opt_glow: true,
            glow_mode: 0,
            glow_strength: 1,
            settings_tab: 0,
            hotkey_mods: HOTKEY_DEFAULT.0,
            hotkey_vk: HOTKEY_DEFAULT.1,
            hotkey_recording: false,
            hotkey_problem: None,
            hidden: false,
            settings: None,
            weather: None,
            weather_error: None,
            weather_loading: false,
            city: String::new(),
            city_input: String::new(),
            city_focus: false,
            media: MediaInfo::default(),
            media_at: Instant::now(),
            cover: None,
            cover_bitmap: None,
            media_tick_on: false,
            reminders: Vec::new(),
            next_reminder_id: 1,
            input: String::new(),
            input_focus: false,
            caret_on: false,
            prev_foreground: 0,
            cam_on: false,
            mic_on: false,
            music: false,
            silent_ms: 0,
            toast: Toast { title: String::new(), message: String::new(), badge: String::new(), kind: EventType::Info, has_progress: false, offer: None },
            tel: Telemetry { cpu: 8, ram: 40, battery: 100, volume: 50, ..Default::default() },
            cpu: CpuSampler::new(),
            telemetry_on: false,
            eq_on: false,
            rng: 0x2545_F491,
            audio: None,
            last_volume: -1,
            last_muted: false,
            last_caps: false,
            last_num: false,
            last_line: 255,
            battery_warned: false,
            last_online: true,
            bt_devices: HashMap::new(),
            bt_recent: HashMap::new(),
            _bt_watchers: None,
            focus_total: 25 * 60,
            focus_remaining: 25 * 60,
            focus_running: false,
            color: accent,
            shown_color: accent,
            color_before_pick: accent,
            history: Vec::new(),
            color_hint: "Wert anklicken zum Kopieren".to_string(),
            picking: false,
            pick_result: 0,
            loupe: None,
            loupe_pixels: [0; LOUPE * LOUPE],
            shelf: Vec::new(),
            next_item_id: 1,
            files_scroll: 0.0,
            files_info: String::new(),
            self_drag: false,
            share: ShareServer::new(19846),
            share_open: false,
            share_url: None,
            share_hint: SHARE_DEFAULT_HINT.to_string(),
            qr: None,
            ignore_clipboard_until: Instant::now(),
            radio_wifi: RadioStatus::Unknown,
            radio_bt: RadioStatus::Unknown,
            wifi: None,
            net_name: String::new(),
            local_ip: String::new(),
            nearby_visible: false,
            nearby_open: false,
            nearby_files: Vec::new(),
            nearby_bytes: 0,
            nearby_hint: String::new(),
            peers: Vec::new(),
            sending: None,
            pending_offer: None,
            deferred_alert: None,
            ws_trim_due: false,
        }
    }

    /// Hooks up every event source. The window exists and is shown.
    pub fn start(&mut self) {
        self.load_settings();
        self.tooltip = Tooltip::new(self.hwnd);
        self.update_clock();
        self.update_files_ui();

        // Baseline, so nothing toasts at start.
        self.audio = Audio::new();
        if let Some((volume, muted)) = self.audio.as_ref().and_then(|a| a.get()) {
            self.last_volume = volume;
            self.last_muted = muted;
            self.tel.volume = volume;
            self.tel.muted = muted;
        }
        self.last_caps = keys::caps_lock();
        self.last_num = keys::num_lock();
        let power_state = power::read();
        self.last_line = power_state.line;
        self.apply_power(power_state);
        let (online, net_name) = network::status();
        self.last_online = online;
        self.net_name = if online { net_name } else { "Offline".to_string() };
        self.sample_telemetry();

        keys::register(self.hwnd);
        power::register(self.hwnd);
        network::register();
        self._bt_watchers = Some(bluetooth::start());
        crate::net::notify_server::start(19845);
        // Visible to nearby PCs unless switched off before.
        self.nearby_visible = state::load_setting("nearby").as_deref() != Some("0");
        if self.nearby_visible {
            airdrop::set_visible(true);
        }
        dragdrop::register(self.hwnd);
        tray::add(self.hwnd);
        unsafe {
            let _ = windows::Win32::System::DataExchange::AddClipboardFormatListener(self.hwnd);
        }

        crate::sys::privacy::start();
        media::start();
        self.city = state::load_setting("weather_city").unwrap_or_default();
        self.city_input = self.city.clone();
        self.refresh_weather();
        self.apply_hotkey();
        for (due, text) in state::load_reminders() {
            let id = self.next_reminder_id;
            self.next_reminder_id += 1;
            self.reminders.push(Reminder { id, text, due });
        }
        self.reminders.sort_by_key(|r| r.due);
        self.schedule_reminders();

        for path in state::load_shelf() {
            self.add_to_shelf(&path, false);
        }
        self.update_files_ui();
        self.update_timers();

        self.enqueue_alert(IslandEvent::new("Dynamic Island", "Bereit • Maus darüber zum Öffnen", EventType::Success, None, "", 2600));
    }

    pub fn cleanup(&mut self) {
        self.end_picking(false);
        unsafe {
            let _ = windows::Win32::UI::Input::KeyboardAndMouse::UnregisterHotKey(Some(self.hwnd), settings::HOTKEY_ID);
        }
        if let Some(s) = self.settings.take() {
            unsafe {
                let _ = DestroyWindow(s.hwnd);
            }
        }
        tray::remove(self.hwnd);
        dragdrop::revoke(self.hwnd);
        unsafe {
            let _ = windows::Win32::System::DataExchange::RemoveClipboardFormatListener(self.hwnd);
        }
        self.audio = None;
        self._bt_watchers = None;
        if let Some(offer) = self.pending_offer.take() {
            airdrop::answer(offer, false);
        }
        airdrop::shutdown();
    }

    // =====================================================================
    //  Frames
    // =====================================================================

    pub fn invalidate(&mut self) {
        self.dirty = true;
    }

    /// True while a frame is due: something changed or something still moves.
    pub fn wants_frame(&self) -> bool {
        self.dirty || self.spring_active || self.anims.active()
    }

    pub fn frame(&mut self) {
        // The compact pill grows and shrinks with what it shows.
        if self.state == VisualState::Compact {
            let target = self.state_shape(VisualState::Compact);
            if (target.0 - self.sw.target).abs() > 0.5 {
                self.set_shape_target(target);
            }
        }
        self.anims.tick();
        self.step_spring();
        self.dirty = false;
        self.update_region();

        if self.gfx.is_none() {
            let (w, h) = ((WIN_W * self.scale).round() as u32, (WIN_H * self.scale).round() as u32);
            self.gfx = Gfx::new(self.hwnd, w, h, self.scale).ok();
            for item in &mut self.shelf {
                item.bitmap = None;
            }
        }
        let Some(mut gfx) = self.gfx.take() else { return };
        scene::draw(&mut gfx, self);
        if gfx.end() {
            self.gfx = Some(gfx);
        } else {
            // Device lost: build a new one on the next frame.
            self.dirty = true;
        }
        self.draw_settings_window();

        if !self.wants_frame() {
            self.set_timer(T_TRIM, 5000);
        }
    }

    /// Drops the device, e.g. after a DPI change.
    pub fn reset_graphics(&mut self, scale: f32) {
        self.scale = scale;
        self.gfx = None;
        self.region = (0, 0, 0, 0);
        self.dirty = true;
    }

    fn set_timer(&self, id: usize, ms: u32) {
        unsafe {
            SetTimer(Some(self.hwnd), id, ms, None);
        }
    }

    fn kill_timer(&self, id: usize) {
        unsafe {
            let _ = KillTimer(Some(self.hwnd), id);
        }
    }

    pub fn on_timer(&mut self, id: usize) {
        match id {
            T_CLOCK => self.update_clock(),
            T_TOAST => {
                self.kill_timer(T_TOAST);
                if self.pending_offer.is_some() {
                    // Nobody answered: decline.
                    self.answer_offer(false);
                    return;
                }
                if self.state == VisualState::Toast {
                    self.transition(VisualState::Compact);
                }
            }
            T_HOVER => {
                // Small intent delay so brushing past the island does not open it.
                self.kill_timer(T_HOVER);
                if self.cursor_over_island() && !self.click_through && self.pending_offer.is_none() {
                    self.kill_timer(T_TOAST);
                    self.transition(VisualState::Expanded);
                }
            }
            T_LEAVE => self.on_leave_tick(),
            T_FOCUS => self.on_focus_tick(),
            T_PICK => self.on_pick_tick(),
            T_TELEMETRY => {
                self.sample_telemetry();
                if self.state == VisualState::Expanded {
                    self.dirty = true;
                }
            }
            T_EQ => self.on_eq_tick(),
            T_KEYS => {
                self.kill_timer(T_KEYS);
                self.check_lock_keys();
            }
            T_NET => {
                self.kill_timer(T_NET);
                self.check_network();
            }
            T_MODAL => {
                if self.wants_frame() {
                    self.frame();
                }
            }
            T_PROBE => self.probe_peers(),
            T_REMINDER => self.on_reminder_tick(),
            T_CARET => {
                self.caret_on = !self.caret_on;
                self.dirty = true;
            }
            T_MEDIA => self.dirty = true,
            T_WEATHER => self.refresh_weather(),
            T_TRIM => {
                self.kill_timer(T_TRIM);
                if !self.wants_frame() {
                    if self.state == VisualState::Compact {
                        // Icon bitmaps come back from their pixels on the next expand.
                        for item in &mut self.shelf {
                            item.bitmap = None;
                        }
                        self.cover_bitmap = None;
                    }
                    let redraw = self.gfx.as_mut().map(|g| g.trim()).unwrap_or(false);
                    if redraw {
                        self.frame();
                        self.kill_timer(T_TRIM);
                    }
                    if self.state == VisualState::Compact && std::mem::take(&mut self.ws_trim_due) {
                        trim_process_memory();
                    }
                }
            }
            _ => {}
        }
    }

    /// Telemetry and the bar animation only run while their output is on screen.
    fn update_timers(&mut self) {
        let want_telemetry = self.state == VisualState::Expanded;
        // The output level is only checked while the compact pill can show the bars.
        let want_eq = self.state == VisualState::Compact && !self.focus_running && self.opt_eq && self.audio.is_some();

        if want_telemetry != self.telemetry_on {
            self.telemetry_on = want_telemetry;
            if want_telemetry {
                self.set_timer(T_TELEMETRY, 1000);
            } else {
                self.kill_timer(T_TELEMETRY);
            }
        }
        // Timeline of the media tab: ticks only while it is on screen and playing.
        let want_media = self.state == VisualState::Expanded && self.tab == TAB_MEDIA && self.media.playing && self.media.duration_ms > 0;
        if want_media != self.media_tick_on {
            self.media_tick_on = want_media;
            if want_media {
                self.set_timer(T_MEDIA, 500);
            } else {
                self.kill_timer(T_MEDIA);
            }
        }
        if want_eq != self.eq_on {
            self.eq_on = want_eq;
            if want_eq {
                self.silent_ms = 0;
                self.set_timer(T_EQ, if self.music { 150 } else { 700 });
            } else {
                self.kill_timer(T_EQ);
                if self.music && (self.focus_running || !self.opt_eq) {
                    self.music = false;
                    self.anims.remove(Key::MusicVis);
                }
            }
        }
    }

    // =====================================================================
    //  Shape spring
    // =====================================================================

    /// Width, height and corner radius as drawn.
    pub fn shape(&self) -> (f32, f32, f32) {
        // Even widths keep the centred content from jittering by half pixels.
        let w = ((self.sw.x / 2.0).round() * 2.0).max(2.0);
        let h = self.sh.x.round().max(2.0);
        let r = self.sr.x.min(h / 2.0).max(0.0);
        (w as f32, h as f32, r as f32)
    }

    pub fn island_rect(&self) -> Rect {
        let (w, h, _) = self.shape();
        Rect::new(CX - w / 2.0, 0.0, w, h)
    }

    fn expanded_shape(&self) -> (f64, f64, f64) {
        match self.tab {
            0 => EXPANDED_HOME,
            TAB_REMINDERS => EXPANDED_REMINDERS,
            TAB_MEDIA => EXPANDED_MEDIA,
            _ => EXPANDED,
        }
    }

    /// Height the expanded content is laid out for.
    pub fn expanded_height(&self) -> f32 {
        self.expanded_shape().1 as f32
    }

    fn state_shape(&self, state: VisualState) -> (f64, f64, f64) {
        match state {
            VisualState::Toast if self.toast_hud => TOAST_HUD,
            VisualState::Toast => TOAST,
            VisualState::Expanded => self.expanded_shape(),
            VisualState::Compact => (self.compact_width(), COMPACT.1, COMPACT.2),
        }
    }

    /// Width the compact pill needs for its status items (estimated, matches scene::compact).
    pub fn compact_width(&self) -> f64 {
        let mut items: Vec<f64> = Vec::new();
        let leds = self.cam_on as u8 + self.mic_on as u8;
        if leds > 0 {
            items.push(leds as f64 * 10.0 - 4.0);
        }
        if self.focus_running {
            items.push(40.0);
        } else if self.music {
            items.push(19.5);
        }
        if !self.reminders.is_empty() {
            // "18:30" or "morgen 07:00"
            items.push(16.0 + if due_text(self.reminders[0].due).len() > 5 { 82.0 } else { 36.0 });
        }
        let unread = self.unread_shown();
        if unread > 0 && self.opt_bell {
            items.push(17.0 + 8.0 * unread.to_string().len() as f64);
        }
        // The clock sits in the middle: the pill is symmetric, so the wider side decides.
        let right = if items.is_empty() { 0.0 } else { items.iter().sum::<f64>() + 10.0 * (items.len() as f64 - 1.0) };
        let left = self.compact_weather().map(|(_, t)| 18.0 + 8.0 * t.chars().count() as f64).unwrap_or(0.0);
        let side = right.max(left);
        if side <= 0.0 {
            return COMPACT.0;
        }
        let needed = 2.0 * (22.0 + 14.0 + side + 16.0);
        needed.max(COMPACT.0).ceil()
    }

    /// Weather shown left of the clock: glyph and "14°".
    pub fn compact_weather(&self) -> Option<(&'static str, String)> {
        if !self.opt_weather_pill {
            return None;
        }
        self.weather.as_ref().map(|w| (weather::glyph(&w.icon), format!("{}°", w.temp)))
    }

    /// Unread count, never more than the notifications that still exist.
    pub fn unread_shown(&self) -> u32 {
        self.unread.min(self.notices.len() as u32)
    }

    fn set_shape_target(&mut self, target: (f64, f64, f64)) {
        self.sw.target = target.0;
        self.sh.target = target.1;
        self.sr.target = target.2;
        if !self.spring_active {
            self.spring_active = true;
            self.spring_last = self.anims.time();
        }
    }

    fn step_spring(&mut self) {
        if !self.spring_active {
            return;
        }
        let now = self.anims.time();
        let dt = (now - self.spring_last).min(0.1);
        self.spring_last = now;
        if dt <= 0.0 {
            return;
        }

        let steps = ((dt / 0.004).ceil() as i32).max(1);
        let h = dt / steps as f64;
        for _ in 0..steps {
            self.sw.step(h);
            self.sh.step(h);
            self.sr.step(h);
        }

        if self.sw.settled() && self.sh.settled() && self.sr.settled() {
            self.sw.snap();
            self.sh.snap();
            self.sr.snap();
            self.spring_active = false;
        }
    }

    /// The window only takes mouse input on the island itself.
    fn update_region(&mut self) {
        let (w, h, _) = self.shape();
        let s = self.scale;
        let m = scene::REGION_MARGIN;
        let region = (
            ((CX - w / 2.0 - 8.0 - m) * s).floor() as i32,
            0,
            ((CX + w / 2.0 + 8.0 + m) * s).ceil() as i32,
            ((h + m) * s).ceil() as i32,
        );
        if region != self.region {
            self.region = region;
            unsafe {
                let rgn = CreateRectRgn(region.0, region.1, region.2, region.3);
                SetWindowRgn(self.hwnd, Some(rgn), false);
            }
        }
    }

    // =====================================================================
    //  State transitions
    // =====================================================================

    pub fn transition(&mut self, target: VisualState) {
        if target != VisualState::Expanded && self.picking {
            self.end_picking(false);
        }
        if target != VisualState::Expanded && self.share_open {
            self.close_share_panel();
        }
        if target != VisualState::Expanded && self.nearby_open {
            self.close_nearby();
        }

        if target != VisualState::Expanded {
            self.blur_input(false);
        }
        let changed = target != self.state;
        self.state = target;
        self.set_shape_target(self.state_shape(target));
        self.dirty = true;
        if !changed {
            return;
        }

        for v in 0..3 {
            self.animate_view(v, v == target as usize);
        }
        if target == VisualState::Expanded {
            self.sample_telemetry();
            self.animate_cards();
            if self.tab == 0 {
                self.mark_read();
            }
            if self.tab == TAB_CONTROLS {
                self.refresh_controls();
            }
        }
        if target == VisualState::Compact {
            self.ws_trim_due = true;
        }
        self.update_timers();
    }

    // Content melts out (fade + shrink + blur) and the next one resolves in.
    fn animate_view(&mut self, v: usize, visible: bool) {
        if self.view_visible[v] == visible {
            return;
        }
        self.view_visible[v] = visible;
        let k = v as u8;
        let default_opacity = if v == 0 { 1.0 } else { 0.0 };

        if visible {
            self.anims.to_delayed(Key::ViewOpacity(k), default_opacity, 1.0, 60, 300, Ease::CubicOut);
            self.anims.run(Key::ViewScale(k), 0.9, 1.0, 60, 300, Ease::CubicOut);
            self.anims.run(Key::ViewBlur(k), 12.0, 0.0, 60, 300, Ease::CubicOut);
        } else {
            self.anims.to(Key::ViewOpacity(k), default_opacity, 0.0, 140, Ease::Linear);
            self.anims.to(Key::ViewScale(k), 1.0, 0.92, 140, Ease::CubicOut);
            self.anims.run(Key::ViewBlur(k), 0.0, 10.0, 0, 140, Ease::Linear);
        }
    }

    fn schedule_collapse(&mut self, ms: u32) {
        self.set_timer(T_LEAVE, ms);
    }

    fn cursor_over_island(&self) -> bool {
        unsafe {
            let mut p = POINT::default();
            if GetCursorPos(&mut p).is_err() {
                return false;
            }
            let _ = ScreenToClient(self.hwnd, &mut p);
            self.island_rect().contains(p.x as f32 / self.scale, p.y as f32 / self.scale)
        }
    }

    fn mouse_captured(&self) -> bool {
        unsafe { GetCapture() == self.hwnd }
    }

    fn on_leave_tick(&mut self) {
        self.kill_timer(T_LEAVE);
        if self.state != VisualState::Expanded {
            return;
        }
        if self.pinned || self.hold_open || self.drag_over || self.share_open || self.nearby_open {
            return;
        }
        if self.mouse_captured() {
            // A slider drag is still in progress: look again shortly.
            self.schedule_collapse(260);
            return;
        }
        if self.cursor_over_island() {
            return;
        }
        self.transition(VisualState::Compact);
    }

    fn on_enter(&mut self) {
        // An open offer keeps its toast (and its buttons) under the cursor.
        if self.click_through || self.pending_offer.is_some() {
            return;
        }
        self.kill_timer(T_LEAVE);
        if self.state != VisualState::Expanded && self.opt_hover {
            self.set_timer(T_HOVER, OPEN_DELAYS[self.open_delay as usize]);
        }
    }

    fn on_leave(&mut self) {
        self.kill_timer(T_HOVER);
        self.schedule_collapse(260);
    }

    pub fn toggle_expanded_from_tray(&mut self) {
        if self.state == VisualState::Expanded {
            self.pinned = false;
            self.transition(VisualState::Compact);
        } else {
            self.pinned = true;
            self.transition(VisualState::Expanded);
        }
    }

    pub fn center_on_screen(&mut self) {
        unsafe {
            let screen = GetSystemMetrics(SM_CXSCREEN);
            let width = (WIN_W * self.scale).round() as i32;
            let _ = SetWindowPos(self.hwnd, None, (screen - width) / 2, 0, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
        }
    }

    pub fn ensure_topmost(&self) {
        if self.hidden {
            return;
        }
        unsafe {
            let _ = SetWindowPos(self.hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
        }
    }

    pub fn set_click_through(&mut self, enabled: bool) {
        self.click_through = enabled;
        unsafe {
            let mut ex = GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE);
            let flags = (WS_EX_TRANSPARENT.0 | WS_EX_LAYERED.0) as isize;
            if enabled {
                ex |= flags;
            } else {
                ex &= !flags;
            }
            SetWindowLongPtrW(self.hwnd, GWL_EXSTYLE, ex);
            if enabled {
                let _ = SetLayeredWindowAttributes(self.hwnd, windows::Win32::Foundation::COLORREF(0), 255, LWA_ALPHA);
            }
        }
        self.ensure_topmost();
        self.set_hot(Id::None);
        self.dirty = true;
    }

    // =====================================================================
    //  Mouse
    // =====================================================================

    fn hit_test(&self, x: f32, y: f32) -> Id {
        self.hits.iter().rev().find(|h| h.rect.contains(x, y)).map(|h| h.id).unwrap_or(Id::None)
    }

    fn chip_of(id: Id) -> Option<u32> {
        match id {
            Id::Chip(i) | Id::ChipLink(i) | Id::ChipRemove(i) => Some(i),
            _ => None,
        }
    }

    fn set_hot(&mut self, id: Id) {
        if id == self.hot {
            return;
        }
        let old = self.hot;
        self.hot = id;
        if old != Id::None {
            self.anims.to(Key::Hover(old), 0.0, 0.0, 220, Ease::Linear);
        }
        if id != Id::None {
            self.anims.to(Key::Hover(id), 0.0, 1.0, 120, Ease::Linear);
        }

        // The small buttons on a file chip show while the chip is hovered.
        let (old_chip, new_chip) = (Self::chip_of(old), Self::chip_of(id));
        if old_chip != new_chip {
            if let Some(c) = old_chip {
                self.anims.to(Key::ChipActions(c), 0.0, 0.0, 200, Ease::Linear);
            }
            if let Some(c) = new_chip {
                self.anims.to(Key::ChipActions(c), 0.0, 1.0, 120, Ease::Linear);
            }
        }

        self.update_tooltip();
        self.dirty = true;
    }

    fn update_tooltip(&mut self) {
        let text = self.tip_text(self.hot);
        let rect = self.hits.iter().rev().find(|h| h.id == self.hot).map(|h| h.rect);
        let s = self.scale;
        let Some(tooltip) = self.tooltip.as_mut() else { return };
        match (text, rect) {
            (Some(text), Some(r)) => {
                let px = RECT {
                    left: (r.x * s) as i32,
                    top: (r.y * s) as i32,
                    right: (r.right() * s) as i32,
                    bottom: (r.bottom() * s) as i32,
                };
                tooltip.set(Some((px, &text)));
            }
            _ => tooltip.set(None),
        }
    }

    fn tip_text(&self, id: Id) -> Option<String> {
        let text = match id {
            Id::Tab(0) => "Übersicht",
            Id::Tab(1) => "Medien",
            Id::Tab(2) => "Farbwähler",
            Id::Tab(3) => "Datei-Ablage",
            Id::Tab(4) => "Fokus-Timer",
            Id::Tab(6) => "WLAN, Bluetooth, Nearby",
            Id::OpenSettings => "Einstellungen",
            Id::MediaPrev => "Vorheriger Titel",
            Id::MediaPlay => "Wiedergabe / Pause",
            Id::MediaNext => "Nächster Titel",
            Id::MediaSeek => "Klicken zum Springen",
            Id::Toggle(0) => "WLAN ein / aus",
            Id::Toggle(1) => "Bluetooth ein / aus",
            Id::Toggle(2) => "Für PCs in der Nähe sichtbar (Dateien empfangen)",
            Id::Nearby => "Ablage an einen PC in der Nähe senden",
            Id::ChipSend(_) => "An einen PC in der Nähe senden",
            Id::OfferAccept => "Annehmen",
            Id::OfferDecline => "Ablehnen",
            Id::Peer(i) => return self.peers.get(i as usize).map(|p| format!("An {} senden", p.name)),
            Id::Notice(_) => "Klicken zum Entfernen",
            Id::Quick(0) => "Windows-Rechner öffnen",
            Id::Quick(1) => "Farbe vom Bildschirm aufnehmen",
            Id::Quick(2) => "Zwischenablage-Verlauf (Win+V)",
            Id::Quick(3) => "Neue Erinnerung eintippen",
            Id::Tab(5) => "Erinnerungen",
            Id::ReminderInput => "z. B. „20 min Wäsche“, „um 18:30 Essen“, „morgen 7 Uhr Müll“",
            Id::ReminderRemove(_) => "Erinnerung löschen",
            Id::Accent(i) => return ACCENTS.get(i as usize).map(|a| a.0.to_string()),
            Id::ClearNotices => "Alle gespeicherten Benachrichtigungen entfernen",
            Id::ResetSettings => "Alle Einstellungen auf Standard",
            Id::Pin => "Offen halten",
            Id::Prev => "Vorheriger Titel",
            Id::Play => "Wiedergabe / Pause",
            Id::Next => "Nächster Titel",
            Id::Mute => "Stumm",
            Id::Hex => "HEX kopieren",
            Id::Rgb => "RGB kopieren",
            Id::Hsl => "HSL kopieren",
            Id::Phone => "QR-Code: Dateien mit dem Handy austauschen",
            Id::ShareFolder => "Empfangene Dateien öffnen",
            Id::FocusReset => "Zurücksetzen",
            Id::ChipLink(_) => "Download-Link fürs lokale Netzwerk kopieren",
            Id::ChipRemove(_) => "Aus Ablage entfernen",
            Id::History(i) => return self.history.get(i as usize).map(|c| to_hex(*c)),
            Id::Chip(i) => {
                return self
                    .shelf
                    .iter()
                    .find(|item| item.id == i)
                    .map(|item| format!("{}\nDoppelklick öffnet · Rechtsklick zeigt im Explorer", item.path));
            }
            _ => return None,
        };
        Some(text.to_string())
    }

    /// True if the hovered widget takes clicks (hand cursor).
    pub fn hot_is_clickable(&self) -> bool {
        self.hot != Id::None
    }

    pub fn on_mouse_move(&mut self, x: f32, y: f32) {
        if !self.tracking_leave {
            let mut track = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: self.hwnd, dwHoverTime: 0 };
            unsafe {
                let _ = TrackMouseEvent(&mut track);
            }
            self.tracking_leave = true;
        }

        let over = !self.click_through && self.island_rect().contains(x, y);
        if over != self.mouse_over {
            self.mouse_over = over;
            if over {
                self.on_enter();
            } else {
                self.on_leave();
            }
        }

        if self.slider_drag {
            self.slider_to(x);
            return;
        }

        if let Some((id, sx, sy)) = self.chip_drag {
            let left_down = unsafe { GetAsyncKeyState(VK_LBUTTON.0 as i32) as u16 & 0x8000 != 0 };
            if !left_down {
                self.chip_drag = None;
            } else if (x - sx).abs() >= 4.0 || (y - sy).abs() >= 4.0 {
                self.drag_chip_out(id);
                return;
            }
        }

        let id = self.hit_test(x, y);
        self.set_hot(id);
    }

    pub fn on_mouse_leave(&mut self) {
        self.tracking_leave = false;
        if self.mouse_over {
            self.mouse_over = false;
            self.on_leave();
        }
        self.set_hot(Id::None);
    }

    pub fn on_lbutton_down(&mut self, x: f32, y: f32, double: bool) {
        // Alt + left-drag moves the island.
        let alt = unsafe { GetAsyncKeyState(VK_MENU.0 as i32) as u16 & 0x8000 != 0 };
        if alt && self.island_rect().contains(x, y) {
            unsafe {
                let _ = ReleaseCapture();
                SendMessageW(self.hwnd, WM_NCLBUTTONDOWN, Some(WPARAM(HTCAPTION as usize)), Some(LPARAM(0)));
            }
            return;
        }

        let id = self.hit_test(x, y);
        // A click anywhere else ends typing.
        if self.input_focus && id != Id::ReminderInput && id != Id::ReminderAdd {
            self.blur_input(false);
        }
        // Without hover-open (or before the delay ran out) a click opens the island.
        if self.state != VisualState::Expanded && id == Id::None && self.island_rect().contains(x, y) && !self.click_through && self.pending_offer.is_none() {
            self.kill_timer(T_HOVER);
            self.kill_timer(T_TOAST);
            self.transition(VisualState::Expanded);
            return;
        }
        if double {
            if let Id::Chip(item) = id {
                self.chip_drag = None;
                if let Some(path) = self.shelf.iter().find(|i| i.id == item).map(|i| i.path.clone()) {
                    if !crate::shell::open(&path) {
                        self.files_info = "Öffnen fehlgeschlagen".to_string();
                        self.dirty = true;
                    }
                }
                return;
            }
        }

        self.pressed = id;
        unsafe {
            SetCapture(self.hwnd);
        }
        match id {
            Id::None => {}
            Id::Slider => {
                self.slider_drag = true;
                self.slider_to(x);
            }
            Id::Chip(item) => self.chip_drag = Some((item, x, y)),
            Id::MediaSeek => self.seek_media(x),
            _ => self.anims.to(Key::Press(id), 1.0, 0.9, 80, Ease::Linear),
        }
        self.dirty = true;
    }

    pub fn on_lbutton_up(&mut self, x: f32, y: f32) {
        // Take the pressed widget first: ReleaseCapture sends WM_CAPTURECHANGED
        // right away, and on_capture_lost would clear it.
        let id = std::mem::replace(&mut self.pressed, Id::None);
        if self.mouse_captured() {
            unsafe {
                let _ = ReleaseCapture();
            }
        }
        self.slider_drag = false;
        self.chip_drag = None;
        if id == Id::None {
            return;
        }
        self.anims.to(Key::Press(id), 1.0, 1.0, 280, Ease::BackOut(0.6));
        self.dirty = true;
        if self.hit_test(x, y) == id {
            self.click(id);
        }
    }

    /// The capture went away without a button-up (e.g. a modal loop took over).
    pub fn on_capture_lost(&mut self) {
        self.slider_drag = false;
        if self.pressed != Id::None {
            let id = std::mem::replace(&mut self.pressed, Id::None);
            self.anims.to(Key::Press(id), 1.0, 1.0, 280, Ease::BackOut(0.6));
            self.dirty = true;
        }
    }

    pub fn on_rbutton_up(&mut self, x: f32, y: f32) {
        if let Id::Chip(item) = self.hit_test(x, y) {
            if let Some(found) = self.shelf.iter().find(|i| i.id == item) {
                crate::shell::reveal(&found.path);
            }
        }
    }

    pub fn on_wheel(&mut self, delta: i32) {
        if self.state == VisualState::Expanded && self.tab == TAB_FILES && !self.share_open && !self.nearby_open {
            self.files_scroll -= delta as f32 * 0.6;
            self.clamp_scroll();
            self.dirty = true;
        }
    }

    fn clamp_scroll(&mut self) {
        let max = (self.shelf.len() as f32 * scene::CHIP_PITCH - scene::CONTENT_W).max(0.0);
        self.files_scroll = self.files_scroll.clamp(0.0, max);
    }

    fn click(&mut self, id: Id) {
        match id {
            Id::Tab(i) => self.select_tab(i as usize),
            Id::Pin => {
                self.pinned = !self.pinned;
                if !self.pinned && !self.cursor_over_island() {
                    self.schedule_collapse(260);
                }
            }
            Id::Prev | Id::MediaPrev => self.media_command(MediaCommand::Previous, VK_MEDIA_PREV_TRACK.0 as u8),
            Id::Play | Id::MediaPlay => self.media_command(MediaCommand::PlayPause, VK_MEDIA_PLAY_PAUSE.0 as u8),
            Id::Next | Id::MediaNext => self.media_command(MediaCommand::Next, VK_MEDIA_NEXT_TRACK.0 as u8),
            Id::OpenSettings => self.open_settings(),
            Id::Mute => {
                if let Some(audio) = &self.audio {
                    audio.toggle_mute();
                }
            }
            Id::Hex => self.copy_color_text(to_hex(self.color), "HEX kopiert"),
            Id::Rgb => self.copy_color_text(format!("rgb({})", to_rgb(self.color)), "RGB kopiert"),
            Id::Hsl => self.copy_color_text(format!("hsl({})", to_hsl(self.color).replace('°', "")), "HSL kopiert"),
            Id::Pipette => {
                if self.picking {
                    self.end_picking(false);
                } else {
                    self.start_picking();
                }
            }
            Id::History(i) => {
                if let Some(&c) = self.history.get(i as usize) {
                    self.color = c;
                    self.shown_color = c;
                    self.copy_color_text(to_hex(c), "HEX kopiert");
                }
            }
            Id::Phone => self.open_share_panel(),
            Id::FilesClear => self.clear_shelf(),
            Id::ChipLink(item) => self.share_link(item),
            Id::ChipRemove(item) => self.remove_from_shelf(item),
            Id::ShareCopy => {
                if let Some(url) = self.share_url.clone() {
                    self.ignore_clipboard_until = Instant::now() + Duration::from_secs(1);
                    self.share_hint = if clipboard::set_text(self.hwnd, &url) { format!("Link kopiert · {url}") } else { "Kopieren fehlgeschlagen".to_string() };
                }
            }
            Id::ShareFolder => {
                let folder = self.share.receive_folder().to_path_buf();
                let _ = std::fs::create_dir_all(&folder);
                if !crate::shell::open(&folder.to_string_lossy()) {
                    self.share_hint = "Ordner konnte nicht geöffnet werden".to_string();
                }
            }
            Id::ShareClose => {
                self.close_share_panel();
                if self.state == VisualState::Expanded && !self.cursor_over_island() {
                    self.schedule_collapse(260);
                }
            }
            Id::Preset(minutes) => self.reset_focus(minutes as i32 * 60),
            Id::FocusToggle => {
                self.focus_running = !self.focus_running;
                if self.focus_running {
                    self.set_timer(T_FOCUS, 1000);
                } else {
                    self.kill_timer(T_FOCUS);
                }
                self.update_timers();
            }
            Id::FocusReset => self.reset_focus(self.focus_total),
            Id::Toggle(0) => self.toggle_radio(radio::Kind::Wifi, self.radio_wifi),
            Id::Toggle(1) => self.toggle_radio(radio::Kind::Bluetooth, self.radio_bt),
            Id::Toggle(_) => self.set_nearby_visible(!self.nearby_visible),
            Id::Nearby => {
                let files: Vec<String> = self.shelf.iter().map(|i| i.path.clone()).collect();
                self.open_nearby(files);
            }
            Id::ChipSend(item) => {
                let files: Vec<String> = self.shelf.iter().filter(|i| i.id == item).map(|i| i.path.clone()).collect();
                self.open_nearby(files);
            }
            Id::Peer(i) => self.send_to_peer(i as usize),
            Id::NearbyClose => {
                self.close_nearby();
                if self.state == VisualState::Expanded && !self.cursor_over_island() {
                    self.schedule_collapse(260);
                }
            }
            Id::OfferAccept => self.answer_offer(true),
            Id::OfferDecline => self.answer_offer(false),
            Id::Notice(notice) => self.dismiss_notice(notice),
            Id::Quick(0) => {
                if !crate::shell::open("calc.exe") {
                    self.enqueue_alert(IslandEvent::new("Rechner", "Konnte nicht gestartet werden", EventType::Error, None, "", 2200));
                }
            }
            Id::Quick(1) => {
                self.select_tab(TAB_COLOR);
                self.start_picking();
            }
            Id::Quick(2) => open_clipboard_history(),
            Id::Quick(_) => {
                self.select_tab(TAB_REMINDERS);
                self.focus_input();
            }
            Id::ReminderInput => self.focus_input(),
            Id::ReminderAdd => self.add_reminder_from_input(),
            Id::ReminderRemove(id) => self.remove_reminder(id),
            Id::Accent(i) => {
                crate::render::set_accent(i);
                state::save_setting("accent", &i.to_string());
                self.update_media_glow();
            }
            Id::Switch(i) => {
                let on = !self.switch_value(i);
                self.set_switch(i, on, true);
            }
            Id::Seg(control, option) => self.set_segment(control, option, true),
            Id::ClearNotices => {
                self.hot = Id::None;
                for n in std::mem::take(&mut self.notices) {
                    self.forget_notice_anims(n.id);
                }
                self.unread = 0;
            }
            Id::ResetSettings => self.reset_settings(),
            Id::SettingsTab(i) => self.select_settings_tab(i as usize),
            // Settings window widgets are handled in settings_click.
            Id::SettingsClose | Id::SettingsMin | Id::HotkeyRecord | Id::CityInput | Id::MediaSeek => {}
            Id::Slider | Id::Chip(_) | Id::None => {}
        }
        self.dirty = true;
    }

    // =====================================================================
    //  Tabs
    // =====================================================================

    fn select_tab(&mut self, index: usize) {
        if index == self.tab {
            return;
        }
        if self.picking {
            self.end_picking(false);
        }
        if self.share_open {
            self.close_share_panel();
        }
        if self.nearby_open {
            self.close_nearby();
        }
        if index == TAB_CONTROLS {
            self.refresh_controls();
        }

        if index != TAB_REMINDERS {
            self.blur_input(false);
        }
        let direction = if index > self.tab { 1.0 } else { -1.0 };
        let old = self.tab;
        self.tab = index;
        self.update_timers();
        if self.state == VisualState::Expanded {
            // The start tab is taller: the island grows or shrinks with the spring.
            self.set_shape_target(self.expanded_shape());
            if index == 0 {
                self.mark_read();
            }
        }

        self.anims.to(Key::TabHighlight, old as f32 * TAB_W, index as f32 * TAB_W, 320, Ease::BackOut(0.35));

        for i in 0..TAB_COUNT {
            let k = i as u8;
            let default_opacity = if i == old { 1.0 } else { 0.0 };
            if i == index {
                self.anims.to_delayed(Key::TabOpacity(k), default_opacity, 1.0, 50, 280, Ease::CubicOut);
                self.anims.run(Key::TabShift(k), direction * 18.0, 0.0, 0, 280, Ease::CubicOut);
            } else if self.anims.get(Key::TabOpacity(k), default_opacity) > 0.0 || i == old {
                self.anims.to(Key::TabOpacity(k), default_opacity, 0.0, 120, Ease::Linear);
                self.anims.to(Key::TabShift(k), 0.0, -direction * 18.0, 120, Ease::CubicOut);
            }
        }
        self.dirty = true;
    }

    // =====================================================================
    //  Clock, compact pill, telemetry
    // =====================================================================

    fn update_clock(&mut self) {
        let now = unsafe { GetLocalTime() };
        self.clock_text = format!("{:02}:{:02}", now.wHour, now.wMinute);
        self.date_text = window::format_date(&now);
        self.date_long = window::format_date_long(&now);
        self.dirty = true;
        // Next update at the minute boundary.
        let ms = 60_000 - (now.wSecond as u32 * 1000 + now.wMilliseconds as u32) + 20;
        self.set_timer(T_CLOCK, ms);
    }

    fn random(&mut self) -> f32 {
        // xorshift32
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        (self.rng >> 8) as f32 / (1u32 << 24) as f32
    }

    /// Music bars: shown and moving only while the default output really plays something.
    fn on_eq_tick(&mut self) {
        if self.state != VisualState::Compact || self.focus_running {
            return;
        }
        let peak = self.audio.as_ref().map(|a| a.peak()).unwrap_or(0.0);
        let playing = peak > PLAYING_PEAK && !self.tel.muted;
        if playing {
            self.silent_ms = 0;
            if !self.music {
                self.music = true;
                self.anims.to(Key::MusicVis, 0.0, 1.0, 220, Ease::CubicOut);
                self.set_timer(T_EQ, 150);
            }
            let level = (peak * 3.0).sqrt().clamp(0.35, 1.0);
            for i in 0..4u8 {
                let target = 0.2 + self.random() * 0.8 * level;
                self.anims.to(Key::Eq(i), scene::EQ_REST[i as usize], target, 150, Ease::SineInOut);
            }
        } else if self.music {
            // Short pauses between tracks keep the bars; a longer silence hides them.
            self.silent_ms += 150;
            for i in 0..4u8 {
                self.anims.to(Key::Eq(i), scene::EQ_REST[i as usize], 0.18, 220, Ease::SineInOut);
            }
            if self.silent_ms >= 1800 {
                self.music = false;
                self.anims.to(Key::MusicVis, 1.0, 0.0, 260, Ease::CubicOut);
                self.set_timer(T_EQ, 700);
            }
        }
    }

    fn sample_telemetry(&mut self) {
        if let Some(cpu) = self.cpu.sample() {
            self.tel.cpu = cpu;
        }
        if let Some(mem) = telemetry::memory() {
            self.tel.ram = mem.percent;
            self.tel.ram_used_gb = mem.used_gb;
            self.tel.ram_total_gb = mem.total_gb;
        }
    }

    fn animate_cards(&mut self) {
        self.dirty = true;
    }

    // =====================================================================
    //  System events
    // =====================================================================

    /// Drains the channel that the worker threads and callbacks post to.
    pub fn on_messages(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Alert(ev) => self.enqueue_alert(ev),
                Msg::Volume { percent, muted } => self.on_volume(percent, muted),
                Msg::AudioDeviceChanged => {
                    if let Some(audio) = self.audio.as_mut() {
                        audio.rebind();
                        if let Some((volume, muted)) = audio.get() {
                            self.last_volume = volume;
                            self.last_muted = muted;
                            self.tel.volume = volume;
                            self.tel.muted = muted;
                            self.dirty = true;
                        }
                    }
                }
                Msg::NetworkChanged => self.set_timer(T_NET, 800),
                Msg::BtAdded { id, name, connected, initial } => {
                    if !initial && connected {
                        self.bluetooth_alert(&id, &name, true);
                    }
                    self.bt_devices.insert(id, BtDevice { name, connected });
                }
                Msg::BtUpdated { id, connected, name } => {
                    let mut alert = None;
                    if let Some(device) = self.bt_devices.get_mut(&id) {
                        if let Some(name) = name {
                            device.name = name;
                        }
                        if let Some(connected) = connected {
                            if connected != device.connected {
                                device.connected = connected;
                                alert = Some((device.name.clone(), connected));
                            }
                        }
                    }
                    if let Some((name, connected)) = alert {
                        self.bluetooth_alert(&id, &name, connected);
                    }
                }
                Msg::BtRemoved { id } => {
                    self.bt_devices.remove(&id);
                }
                Msg::FileReceived(path) => self.on_file_received(&path.to_string_lossy()),
                Msg::TextReceived(text) => self.on_text_received(&text),
                Msg::FileDownloaded(name) => {
                    self.share_hint = format!("Gesendet · {name}");
                    self.enqueue_alert(IslandEvent::new("Datei gesendet", &name, EventType::Success, Some(100), "", 2600));
                }
                Msg::Radio { kind, status } => {
                    match kind {
                        radio::Kind::Wifi => self.radio_wifi = status,
                        radio::Kind::Bluetooth => self.radio_bt = status,
                    }
                    self.dirty = true;
                }
                Msg::Wifi(info) => {
                    self.wifi = info;
                    self.dirty = true;
                }
                Msg::PeerFound { id, name, addr } => {
                    let now = Instant::now();
                    if let Some(peer) = self.peers.iter_mut().find(|p| p.id == id) {
                        peer.name = name;
                        peer.addr = addr;
                        peer.seen = now;
                    } else if self.peers.len() < scene::MAX_PEERS {
                        let k = self.peers.len() as u32;
                        self.peers.push(Peer { id, name, addr, seen: now });
                        // Peers pop in like shelf chips.
                        self.anims.run(Key::PopScale(PEER_POP + k), 0.6, 1.0, 0, 360, Ease::BackOut(0.5));
                    }
                    self.dirty = true;
                }
                Msg::PeerGone { id } => {
                    self.peers.retain(|p| p.id != id);
                    self.dirty = true;
                }
                Msg::DropOffer { offer, from, names, total } => self.on_offer(offer, &from, &names, total),
                Msg::DropClosed { offer } => {
                    if self.pending_offer == Some(offer) {
                        self.pending_offer = None;
                        self.toast.offer = None;
                        self.kill_timer(T_TOAST);
                        self.enqueue_alert(IslandEvent::new("Anfrage abgelaufen", "Keine Dateien empfangen", EventType::Warning, None, "", 2200));
                    }
                }
                Msg::DropProgress { from, percent } => {
                    self.enqueue_alert(IslandEvent::new(&format!("Empfange von {from}"), &format!("{percent} %"), EventType::Nearby, Some(percent), "", 8000));
                }
                Msg::DropDone { from, paths, complete } => self.on_drop_done(&from, paths, complete),
                Msg::SendProgress { to, percent } => {
                    self.sending = Some((to.clone(), percent));
                    self.nearby_hint = format!("Sende an {to} · {percent} %");
                    self.enqueue_alert(IslandEvent::new(&format!("Sende an {to}"), &format!("{percent} %"), EventType::Nearby, Some(percent), "", 8000));
                }
                Msg::Privacy { device, on } => {
                    match device {
                        crate::sys::privacy::Device::Camera => self.cam_on = on,
                        crate::sys::privacy::Device::Microphone => self.mic_on = on,
                    }
                    self.dirty = true;
                }
                Msg::Media(info) => {
                    let track_changed = info.title != self.media.title || info.active != self.media.active;
                    let play_changed = info.playing != self.media.playing || info.active != self.media.active;
                    self.media = info;
                    self.media_at = Instant::now();
                    if track_changed || play_changed || self.state == VisualState::Expanded {
                        self.dirty = true;
                    }
                    if play_changed {
                        self.update_media_glow();
                    }
                    self.update_timers();
                }
                Msg::MediaCover(cover) => {
                    let had_cover = self.cover.is_some();
                    self.cover = cover;
                    self.cover_bitmap = None;
                    if self.cover.is_some() || had_cover {
                        self.update_media_glow();
                    }
                    self.dirty = true;
                }
                Msg::Weather(result) => {
                    self.weather_loading = false;
                    match result {
                        Ok(w) => {
                            self.weather = Some(w);
                            self.weather_error = None;
                        }
                        Err(e) => {
                            // A failed refresh keeps the last good values on screen.
                            if matches!(e, WeatherError::NoCity | WeatherError::CityNotFound | WeatherError::NoKey | WeatherError::BadKey) {
                                self.weather = None;
                            }
                            self.weather_error = Some(e);
                        }
                    }
                    self.dirty = true;
                }
                Msg::SendDone { to, outcome } => {
                    self.sending = None;
                    let ev = match outcome {
                        SendOutcome::Sent => IslandEvent::new("Gesendet", &format!("Dateien an {to} übertragen"), EventType::Success, Some(100), "", 2800),
                        SendOutcome::Declined => IslandEvent::new("Abgelehnt", &format!("{to} hat abgelehnt"), EventType::Warning, None, "", 2800),
                        SendOutcome::Failed => IslandEvent::new("Senden fehlgeschlagen", &format!("Keine Verbindung zu {to}"), EventType::Error, None, "", 3200),
                    };
                    self.nearby_hint = ev.message.clone();
                    self.enqueue_alert(ev);
                }
            }
        }
    }

    fn on_volume(&mut self, volume: i32, muted: bool) {
        if !self.slider_drag {
            self.tel.volume = volume;
        }
        self.tel.muted = muted;
        self.dirty = true;

        if muted != self.last_muted {
            self.last_muted = muted;
            self.last_volume = volume;
            let ev = if muted {
                IslandEvent::new("Audio Stummgeschaltet", "Master-Lautstärke ist stumm", EventType::Audio, Some(0), "MUTE", 2000)
            } else {
                IslandEvent::new("Audio Aktiv", &format!("Lautstärke auf {volume}%"), EventType::Audio, Some(volume), &format!("{volume}%"), 2000)
            };
            self.enqueue_alert(ev);
        } else if self.last_volume >= 0 && (volume - self.last_volume).abs() >= 1 {
            self.last_volume = volume;
            self.enqueue_alert(IslandEvent::new("Lautstärke", &format!("Master-Pegel {volume}%"), EventType::Audio, Some(volume), &format!("{volume}%"), 1800));
        } else if self.last_volume < 0 {
            self.last_volume = volume;
        }
    }

    fn slider_to(&mut self, x: f32) {
        let r = scene::SLIDER;
        let value = (((x - r.x - 7.0) / (r.w - 14.0)) * 100.0).round().clamp(0.0, 100.0) as i32;
        if value != self.tel.volume {
            self.tel.volume = value;
            if let Some(audio) = &self.audio {
                audio.set_volume(value);
            }
        }
        self.dirty = true;
    }

    /// A lock key was pressed somewhere; the toggle state settles a moment later.
    pub fn on_lock_key(&mut self) {
        self.set_timer(T_KEYS, 40);
    }

    fn check_lock_keys(&mut self) {
        let caps = keys::caps_lock();
        if caps != self.last_caps {
            self.last_caps = caps;
            self.enqueue_alert(IslandEvent::new(
                "Feststelltaste (CapsLock)",
                if caps { "GROSSSCHREIBUNG AKTIV" } else { "Normaler Modus" },
                EventType::KeyLock,
                None,
                if caps { "ON" } else { "OFF" },
                1700,
            ));
        }
        let num = keys::num_lock();
        if num != self.last_num {
            self.last_num = num;
            self.enqueue_alert(IslandEvent::new(
                "NumLock",
                if num { "Ziffernblock aktiviert" } else { "Ziffernblock deaktiviert" },
                EventType::KeyLock,
                None,
                if num { "NUM" } else { "OFF" },
                1600,
            ));
        }
    }

    fn apply_power(&mut self, p: power::PowerState) {
        self.tel.has_battery = p.has_battery;
        self.tel.battery = p.percent.clamp(0, 100);
        self.tel.charging = p.line == 1;
    }

    pub fn on_power_change(&mut self) {
        let p = power::read();
        self.apply_power(p);

        if p.line != self.last_line {
            self.last_line = p.line;
            let battery = self.tel.battery;
            self.enqueue_alert(IslandEvent::new(
                if p.line == 1 { "Netzteil Angeschlossen" } else { "Akkubetrieb" },
                &format!("Ladestand bei {battery}%"),
                EventType::Power,
                Some(battery),
                &format!("{battery}%"),
                2600,
            ));
        }

        if self.tel.has_battery && !self.tel.charging && self.tel.battery <= 20 && !self.battery_warned {
            self.battery_warned = true;
            let battery = self.tel.battery;
            self.enqueue_alert(IslandEvent::new("Akku Schwach", &format!("Nur noch {battery}% verbleibend"), EventType::Warning, Some(battery), "LOW", 3200));
        } else if self.tel.battery > 25 {
            self.battery_warned = false;
        }

        if self.state == VisualState::Expanded {
            self.animate_cards();
        }
    }

    fn check_network(&mut self) {
        if self.state == VisualState::Expanded && self.tab == TAB_CONTROLS {
            self.refresh_controls();
        }
        let (online, name) = network::status();
        self.net_name = if online { name.clone() } else { "Offline".to_string() };
        self.dirty = true;
        if online != self.last_online {
            self.last_online = online;
            if online && self.weather_error == Some(WeatherError::Offline) {
                self.refresh_weather();
            }
            self.enqueue_alert(IslandEvent::new(
                if online { "Netzwerk Verbunden" } else { "Netzwerk Getrennt" },
                &if online { format!("Verbindung aktiv ({name})") } else { "Keine aktive Verbindung".to_string() },
                if online { EventType::Network } else { EventType::Warning },
                None,
                if online { "NET" } else { "OFF" },
                2800,
            ));
        }
    }

    fn bluetooth_alert(&mut self, id: &str, name: &str, connected: bool) {
        let address = bluetooth::address_from_id(id);
        // A dual-mode device reports once as classic and once as LE.
        if let Some(address) = address {
            let now = Instant::now();
            if let Some((state, when)) = self.bt_recent.get(&address) {
                if *state == connected && now.duration_since(*when) < Duration::from_secs(3) {
                    return;
                }
            }
            self.bt_recent.insert(address, (connected, now));
        }

        let kind = bluetooth::describe(address);
        let title = if name.trim().is_empty() { kind } else { name.trim() };
        self.enqueue_alert(IslandEvent::new(
            title,
            &format!("{kind} {}", if connected { "verbunden" } else { "getrennt" }),
            if connected { EventType::Bluetooth } else { EventType::Warning },
            None,
            if connected { "BT" } else { "OFF" },
            2800,
        ));
    }

    pub fn on_clipboard_update(&mut self) {
        if !self.opt_clip {
            return;
        }
        // Text sent from a phone gets its own toast.
        if Instant::now() < self.ignore_clipboard_until {
            return;
        }
        let preview = match clipboard::read(self.hwnd) {
            Some(clipboard::Content::Text(text)) => truncate_preview(&text),
            Some(clipboard::Content::Image) => "[Bild in Zwischenablage]".to_string(),
            None => return,
        };
        self.enqueue_alert(IslandEvent::new(
            "In Zwischenablage kopiert",
            if preview.is_empty() { "Element gespeichert" } else { &preview },
            EventType::Clipboard,
            None,
            "COPY",
            2200,
        ));
    }

    pub fn on_tray_command(&mut self, cmd: u32) {
        match cmd {
            tray::CMD_TOGGLE => self.toggle_expanded_from_tray(),
            tray::CMD_CENTER => self.center_on_screen(),
            tray::CMD_SETTINGS => self.open_settings(),
            tray::CMD_HIDE => self.toggle_hidden(),
            tray::CMD_CLICK_THROUGH => self.set_click_through(!self.click_through),
            tray::CMD_DEMO_SUCCESS => self.enqueue_alert(IslandEvent::new("Build Erfolgreich", "DynamicIsland.exe (0 Fehler, 1.1s)", EventType::Success, Some(100), "OK", 2600)),
            tray::CMD_DEMO_WARNING => self.enqueue_alert(IslandEvent::new("System-Warnung", "Hintergrundprozess nutzt 89% CPU", EventType::Warning, Some(89), "WARN", 2600)),
            tray::CMD_DEMO_AUDIO => self.enqueue_alert(IslandEvent::new("Medienwiedergabe", "Synthwave FM – Midnight Drive", EventType::Audio, Some(72), "LIVE", 2600)),
            tray::CMD_DEMO_ERROR => self.enqueue_alert(IslandEvent::new("Verbindungsfehler", "Server Timeout auf Port 8080", EventType::Error, None, "ERR", 2600)),
            tray::CMD_EXIT => {
                self.cleanup();
                unsafe {
                    let _ = DestroyWindow(self.hwnd);
                }
            }
            _ => {}
        }
    }

    // =====================================================================
    //  Toasts
    // =====================================================================

    pub fn enqueue_alert(&mut self, ev: IslandEvent) {
        // An open offer keeps the toast; the newest other alert waits behind it.
        if self.pending_offer.is_some() {
            self.deferred_alert = Some(ev);
            return;
        }
        self.log_notice(&ev);

        let hud = self.opt_hud && ev.kind == EventType::Audio && ev.progress.is_some();
        // Different content while the toast is open: slide the new one in.
        if self.state == VisualState::Toast && (ev.title != self.toast.title || hud != self.toast_hud) {
            self.anims.run(Key::ToastSwap, 0.0, 1.0, 0, 240, Ease::CubicOut);
        }
        self.toast_hud = hud;
        self.toast = Toast { title: ev.title, message: ev.message, badge: ev.badge, kind: ev.kind, has_progress: ev.progress.is_some(), offer: None };
        if let Some(progress) = ev.progress {
            let fraction = (progress as f32 / 100.0).clamp(0.0, 1.0);
            self.anims.to(Key::Bar(3), 0.0, fraction, if hud { 220 } else { 600 }, Ease::CubicOut);
        }
        self.dirty = true;

        // While the user has the island open, do not pull it away from them.
        if self.state != VisualState::Expanded {
            self.transition(VisualState::Toast);
            let ms = (ev.duration_ms as f32 * TOAST_LENGTHS[self.toast_len as usize]) as u32;
            self.set_timer(T_TOAST, ms.max(1200));
        }
    }

    // =====================================================================
    //  Settings
    // =====================================================================

    const SWITCH_KEYS: [&'static str; 8] = ["hover_open", "volume_hud", "clipboard_toasts", "bell", "music_bars", "weather_pill", "hotkey", "glow"];

    pub fn switch_value(&self, i: u8) -> bool {
        match i {
            0 => self.opt_hover,
            1 => self.opt_hud,
            2 => self.opt_clip,
            3 => self.opt_bell,
            4 => self.opt_eq,
            5 => self.opt_weather_pill,
            6 => self.opt_hotkey,
            _ => self.opt_glow,
        }
    }

    fn set_switch(&mut self, i: u8, on: bool, animate: bool) {
        let from = if self.switch_value(i) { 1.0 } else { 0.0 };
        match i {
            0 => self.opt_hover = on,
            1 => self.opt_hud = on,
            2 => self.opt_clip = on,
            3 => self.opt_bell = on,
            4 => {
                self.opt_eq = on;
                self.update_timers();
            }
            5 => self.opt_weather_pill = on,
            6 => {
                self.opt_hotkey = on;
                self.apply_hotkey();
            }
            _ => {
                self.opt_glow = on;
                self.update_media_glow();
            }
        }
        let to = if on { 1.0 } else { 0.0 };
        if animate {
            state::save_setting(Self::SWITCH_KEYS[i as usize], if on { "1" } else { "0" });
            self.anims.run(Key::SwitchPos(i), from, to, 0, 220, Ease::CubicOut);
        } else {
            self.anims.run(Key::SwitchPos(i), to, to, 0, 0, Ease::Linear);
        }
        self.dirty = true;
    }

    pub fn segment_value(&self, control: u8) -> u8 {
        match control {
            0 => self.open_delay,
            1 => self.toast_len,
            2 => self.glow_mode,
            _ => self.glow_strength,
        }
    }

    fn set_segment(&mut self, control: u8, option: u8, animate: bool) {
        let option = option.min(2);
        let from = self.segment_value(control) as f32;
        match control {
            0 => self.open_delay = option,
            1 => self.toast_len = option,
            2 => {
                self.glow_mode = option;
                self.update_media_glow();
            }
            _ => {
                self.glow_strength = option;
                self.update_media_glow();
            }
        }
        if animate {
            let key = match control {
                0 => "open_delay",
                1 => "toast_length",
                2 => "glow_mode",
                _ => "glow_strength",
            };
            state::save_setting(key, &option.to_string());
            self.anims.run(Key::SegPos(control), from, option as f32, 0, 260, Ease::BackOut(0.3));
        } else {
            self.anims.run(Key::SegPos(control), option as f32, option as f32, 0, 0, Ease::Linear);
        }
        self.dirty = true;
    }

    fn select_settings_tab(&mut self, index: usize) {
        let index = index.min(settings::SETTINGS_TAB_COUNT - 1);
        if index == self.settings_tab {
            return;
        }
        self.commit_city();
        if self.hotkey_recording {
            self.hotkey_recording = false;
            self.apply_hotkey();
        }
        let direction = if index > self.settings_tab { 1.0 } else { -1.0 };
        let old = self.settings_tab;
        self.settings_tab = index;
        self.anims.to(Key::SettingsTabHighlight, old as f32, index as f32, 260, Ease::BackOut(0.3));
        for i in 0..settings::SETTINGS_TAB_COUNT {
            let k = i as u8;
            let default_opacity = if i == old { 1.0 } else { 0.0 };
            if i == index {
                self.anims.to_delayed(Key::SettingsTabOpacity(k), default_opacity, 1.0, 40, 240, Ease::CubicOut);
                self.anims.run(Key::SettingsTabShift(k), direction * 14.0, 0.0, 0, 240, Ease::CubicOut);
            } else if self.anims.get(Key::SettingsTabOpacity(k), default_opacity) > 0.0 || i == old {
                self.anims.to(Key::SettingsTabOpacity(k), default_opacity, 0.0, 110, Ease::Linear);
                self.anims.to(Key::SettingsTabShift(k), 0.0, -direction * 14.0, 110, Ease::CubicOut);
            }
        }
        self.dirty = true;
    }

    fn load_settings(&mut self) {
        let accent = state::load_setting("accent").and_then(|v| v.parse::<u8>().ok()).unwrap_or(0);
        crate::render::set_accent(accent);
        for (i, key) in Self::SWITCH_KEYS.iter().enumerate() {
            if let Some(v) = state::load_setting(key) {
                self.set_switch(i as u8, v == "1", false);
            }
        }
        if let (Some(m), Some(k)) = (
            state::load_setting("hotkey_mods").and_then(|v| v.parse::<u32>().ok()),
            state::load_setting("hotkey_vk").and_then(|v| v.parse::<u32>().ok()),
        ) {
            if k != 0 {
                self.hotkey_mods = m;
                self.hotkey_vk = k;
            }
        }
        let open = state::load_setting("open_delay").and_then(|v| v.parse::<u8>().ok()).unwrap_or(0);
        let length = state::load_setting("toast_length").and_then(|v| v.parse::<u8>().ok()).unwrap_or(1);
        let glow_mode = state::load_setting("glow_mode").and_then(|v| v.parse::<u8>().ok()).unwrap_or(0);
        let glow_strength = state::load_setting("glow_strength").and_then(|v| v.parse::<u8>().ok()).unwrap_or(1);
        self.set_segment(0, open, false);
        self.set_segment(1, length, false);
        self.set_segment(2, glow_mode, false);
        self.set_segment(3, glow_strength, false);
        self.update_media_glow();
    }

    // =====================================================================
    //  Reminders: typed text field, parsed with crate::reminder
    // =====================================================================

    /// True while the reminder field may take the keyboard (WM_MOUSEACTIVATE).
    pub fn wants_keyboard(&self) -> bool {
        self.input_focus || self.hot == Id::ReminderInput
    }

    pub fn focus_input(&mut self) {
        if self.input_focus {
            return;
        }
        unsafe {
            use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, SetForegroundWindow};
            let foreground = GetForegroundWindow();
            if foreground != self.hwnd {
                self.prev_foreground = foreground.0 as isize;
            }
            let _ = SetForegroundWindow(self.hwnd);
            let _ = windows::Win32::UI::Input::KeyboardAndMouse::SetFocus(Some(self.hwnd));
        }
        self.input_focus = true;
        self.hold_open = true;
        self.caret_on = true;
        self.set_timer(T_CARET, 530);
        self.dirty = true;
    }

    /// Ends typing. `restore` hands the focus back to the window that had it before.
    pub fn blur_input(&mut self, restore: bool) {
        if !self.input_focus {
            return;
        }
        // Cleared first: giving the focus away sends WM_KILLFOCUS, which lands here again.
        self.input_focus = false;
        self.hold_open = false;
        self.caret_on = false;
        self.kill_timer(T_CARET);
        let previous = std::mem::take(&mut self.prev_foreground);
        if restore && previous != 0 {
            unsafe {
                let _ = windows::Win32::UI::WindowsAndMessaging::SetForegroundWindow(HWND(previous as *mut _));
            }
        }
        if self.state == VisualState::Expanded && !self.cursor_over_island() {
            self.schedule_collapse(400);
        }
        self.dirty = true;
    }

    pub fn on_char(&mut self, c: u16) {
        if !self.input_focus {
            return;
        }
        match c {
            8 => {
                self.input.pop();
            }
            // Ctrl+Backspace: last word.
            127 => {
                let trimmed = self.input.trim_end().len();
                self.input.truncate(trimmed);
                while self.input.chars().last().is_some_and(|c| !c.is_whitespace()) {
                    self.input.pop();
                }
            }
            13 => {
                self.add_reminder_from_input();
                return;
            }
            27 => {
                self.blur_input(true);
                return;
            }
            // Ctrl+V
            22 => {
                if let Some(clipboard::Content::Text(text)) = clipboard::read(self.hwnd) {
                    for ch in text.chars().map(|c| if c.is_whitespace() { ' ' } else { c }).filter(|c| !c.is_control()) {
                        if self.input.chars().count() >= INPUT_MAX {
                            break;
                        }
                        self.input.push(ch);
                    }
                }
            }
            c if c < 32 || (0xD800..=0xDFFF).contains(&c) => return,
            c => {
                if let Some(ch) = char::from_u32(c as u32) {
                    if self.input.chars().count() < INPUT_MAX {
                        self.input.push(ch);
                    }
                }
            }
        }
        // The caret stays solid while typing.
        self.caret_on = true;
        self.set_timer(T_CARET, 530);
        self.dirty = true;
    }

    /// Live preview of the typed text: (text, "18:30 · in 20 Min") or what is missing.
    pub fn input_preview(&self) -> Option<Result<(String, String), &'static str>> {
        let input = self.input.trim();
        if input.is_empty() {
            return None;
        }
        Some(crate::reminder::parse(input, local_seconds()).map(|p| {
            let due = unix_now() + p.seconds;
            (p.text, format!("{} · {}", due_text(due), crate::reminder::format_left(p.seconds as i64)))
        }))
    }

    fn add_reminder_from_input(&mut self) {
        let input = self.input.trim().to_string();
        if input.is_empty() {
            self.blur_input(true);
            return;
        }
        // An unclear entry stays in the field; the preview line says what is missing.
        let Ok(parsed) = crate::reminder::parse(&input, local_seconds()) else { return };
        let id = self.next_reminder_id;
        self.next_reminder_id += 1;
        self.reminders.push(Reminder { id, text: parsed.text, due: unix_now() + parsed.seconds });
        self.reminders.sort_by_key(|r| r.due);
        self.save_reminders();
        self.schedule_reminders();
        self.anims.run(Key::PopScale(REM_POP + id), 0.9, 1.0, 0, 340, Ease::BackOut(0.5));
        self.anims.run(Key::PopOpacity(REM_POP + id), 0.0, 1.0, 0, 220, Ease::Linear);
        self.input.clear();
        self.blur_input(true);
        self.dirty = true;
    }

    fn forget_reminder_anims(&mut self, id: u32) {
        for key in [Key::PopScale(REM_POP + id), Key::PopOpacity(REM_POP + id), Key::Hover(Id::ReminderRemove(id)), Key::Press(Id::ReminderRemove(id))] {
            self.anims.remove(key);
        }
    }

    fn remove_reminder(&mut self, id: u32) {
        self.reminders.retain(|r| r.id != id);
        self.forget_reminder_anims(id);
        self.hot = Id::None;
        self.update_tooltip();
        self.save_reminders();
        self.schedule_reminders();
        self.dirty = true;
    }

    fn save_reminders(&self) {
        let items: Vec<(u64, String)> = self.reminders.iter().map(|r| (r.due, r.text.clone())).collect();
        state::save_reminders(&items);
    }

    /// One timer for the next due reminder; at most 30 s so sleep and clock changes are caught.
    fn schedule_reminders(&mut self) {
        self.kill_timer(T_REMINDER);
        if let Some(first) = self.reminders.first() {
            let ms = first.due.saturating_sub(unix_now()).saturating_mul(1000).clamp(100, 30_000);
            self.set_timer(T_REMINDER, ms as u32);
        }
    }

    fn on_reminder_tick(&mut self) {
        self.kill_timer(T_REMINDER);
        let now = unix_now();
        let (due, rest): (Vec<Reminder>, Vec<Reminder>) = std::mem::take(&mut self.reminders).into_iter().partition(|r| r.due <= now);
        self.reminders = rest;
        if !due.is_empty() {
            self.save_reminders();
            unsafe {
                let _ = windows::Win32::System::Diagnostics::Debug::MessageBeep(windows::Win32::UI::WindowsAndMessaging::MB_ICONASTERISK);
            }
            for r in due {
                self.forget_reminder_anims(r.id);
                let message = if now - r.due > 120 { format!("Erinnerung · verpasst ({})", due_text(r.due)) } else { "Erinnerung".to_string() };
                self.enqueue_alert(IslandEvent::new(&r.text, &message, EventType::Timer, None, "", 9000));
            }
        }
        self.schedule_reminders();
        self.dirty = true;
    }


    fn reset_settings(&mut self) {
        crate::render::set_accent(0);
        state::save_setting("accent", "0");
        for (i, on) in [true, true, true, true, true, true, false, true].into_iter().enumerate() {
            if self.switch_value(i as u8) != on {
                self.set_switch(i as u8, on, true);
            }
        }
        self.set_segment(0, 0, true);
        self.set_segment(1, 1, true);
        self.set_segment(2, 0, true);
        self.set_segment(3, 1, true);
        self.update_media_glow();
    }

    /// Keeps alerts worth reading later. Level changes and progress steps are left out.
    fn log_notice(&mut self, ev: &IslandEvent) {
        let transient = matches!(ev.kind, EventType::Audio | EventType::KeyLock)
            || (ev.kind == EventType::Nearby && ev.progress.is_some_and(|p| p < 100))
            || ev.title == "Dynamic Island";
        if transient {
            return;
        }
        let time = self.clock_text.clone();
        if let Some(first) = self.notices.first_mut() {
            if first.title == ev.title && first.message == ev.message {
                first.time = time;
                self.dirty = true;
                return;
            }
        }
        let id = self.next_notice_id;
        self.next_notice_id += 1;
        self.notices.insert(0, Notice { id, title: ev.title.clone(), message: ev.message.clone(), kind: ev.kind, time });
        while self.notices.len() > MAX_NOTICES {
            if let Some(old) = self.notices.pop() {
                self.forget_notice_anims(old.id);
            }
        }
        self.anims.run(Key::PopScale(NOTE_POP + id), 0.92, 1.0, 0, 340, Ease::BackOut(0.5));
        self.anims.run(Key::PopOpacity(NOTE_POP + id), 0.0, 1.0, 0, 220, Ease::Linear);
        if !(self.state == VisualState::Expanded && self.tab == 0) {
            self.unread = (self.unread + 1).min(99);
        }
        self.dirty = true;
    }

    fn forget_notice_anims(&mut self, id: u32) {
        for key in [Key::PopScale(NOTE_POP + id), Key::PopOpacity(NOTE_POP + id), Key::Hover(Id::Notice(id)), Key::Press(Id::Notice(id))] {
            self.anims.remove(key);
        }
    }

    fn dismiss_notice(&mut self, id: u32) {
        self.notices.retain(|n| n.id != id);
        self.unread = self.unread.min(self.notices.len() as u32);
        self.forget_notice_anims(id);
        // The card under the cursor is gone; the next mouse move finds the new one.
        self.hot = Id::None;
        self.update_tooltip();
        self.dirty = true;
    }

    fn mark_read(&mut self) {
        if self.unread != 0 {
            self.unread = 0;
            self.dirty = true;
        }
    }

    // =====================================================================
    //  Focus timer
    // =====================================================================

    fn reset_focus(&mut self, total_seconds: i32) {
        self.focus_running = false;
        self.kill_timer(T_FOCUS);
        self.focus_total = total_seconds;
        self.focus_remaining = total_seconds;
        self.update_timers();
        self.dirty = true;
    }

    fn on_focus_tick(&mut self) {
        if self.focus_remaining > 1 {
            self.focus_remaining -= 1;
            self.dirty = true;
            return;
        }
        let minutes = self.focus_total / 60;
        self.reset_focus(self.focus_total);
        self.enqueue_alert(IslandEvent::new("Fokus beendet", &format!("{minutes} Minuten geschafft"), EventType::Timer, Some(100), "", 4000));
    }

    // =====================================================================
    //  Colour picker
    // =====================================================================

    fn start_picking(&mut self) {
        if self.picking {
            return;
        }
        if self.loupe.is_none() {
            self.loupe = Loupe::new();
        }
        if self.loupe.is_none() || !picker::install_hook() {
            self.color_hint = "Pipette konnte nicht gestartet werden".to_string();
            return;
        }

        self.picking = true;
        self.pick_result = 0;
        self.hold_open = true;
        self.color_before_pick = self.color;
        self.color_hint = "Klick wählt · Rechtsklick oder Esc bricht ab".to_string();
        self.set_timer(T_PICK, 30);
        self.dirty = true;
    }

    /// Called from the low-level mouse hook. True swallows the mouse event.
    pub fn on_pick_mouse(&mut self, msg: u32) -> bool {
        if !self.picking {
            return false;
        }
        if msg == WM_LBUTTONDOWN && self.pick_result == 0 {
            // Let clicks on the island through (e.g. the cancel button).
            if !self.cursor_over_island() {
                if let Some(c) = self.sample_screen() {
                    self.color = c;
                }
                self.pick_result = 1;
                return true;
            }
        } else if msg == WM_RBUTTONDOWN && self.pick_result == 0 {
            self.pick_result = 2;
            return true;
        } else if (msg == WM_LBUTTONUP || msg == WM_RBUTTONUP) && self.pick_result != 0 {
            let commit = self.pick_result == 1;
            unsafe {
                let _ = PostMessageW(Some(self.hwnd), window::WM_APP_PICK_END, WPARAM(commit as usize), LPARAM(0));
            }
            return true;
        }
        false
    }

    fn sample_screen(&mut self) -> Option<Rgb> {
        let loupe = self.loupe.as_mut()?;
        loupe.sample(&mut self.loupe_pixels)
    }

    fn on_pick_tick(&mut self) {
        if !self.picking || self.pick_result != 0 {
            return;
        }
        if picker::escape_down() {
            self.end_picking(false);
            return;
        }
        if let Some(live) = self.sample_screen() {
            self.shown_color = live;
            self.dirty = true;
        }
    }

    pub fn end_picking(&mut self, commit: bool) {
        if !self.picking {
            return;
        }
        self.picking = false;
        self.kill_timer(T_PICK);
        picker::remove_hook();
        self.loupe = None;
        self.hold_open = false;
        self.dirty = true;

        if commit {
            self.shown_color = self.color;
            self.add_color_to_history(self.color);
            self.copy_color_text(to_hex(self.color), "HEX kopiert");
            // Keep the result on screen for a moment before collapsing.
            if self.state == VisualState::Expanded {
                self.schedule_collapse(3000);
            }
        } else {
            self.color = self.color_before_pick;
            self.shown_color = self.color;
            self.color_hint = "Wert anklicken zum Kopieren".to_string();
            if self.state == VisualState::Expanded && !self.cursor_over_island() {
                self.schedule_collapse(600);
            }
        }
    }

    fn add_color_to_history(&mut self, c: Rgb) {
        self.history.retain(|h| *h != c);
        self.history.insert(0, c);
        self.history.truncate(14);
        // The newest dot pops in.
        self.anims.run(Key::PopScale(0), 0.6, 1.0, 0, 360, Ease::BackOut(0.5));
        self.anims.run(Key::PopOpacity(0), 0.0, 1.0, 0, 220, Ease::Linear);
    }

    fn copy_color_text(&mut self, text: String, hint: &str) {
        self.ignore_clipboard_until = Instant::now();
        self.color_hint = if clipboard::set_text(self.hwnd, &text) { format!("{hint} · {text}") } else { "Kopieren fehlgeschlagen".to_string() };
        self.dirty = true;
    }

    // =====================================================================
    //  File shelf
    // =====================================================================

    /// A drag with files entered the island. True accepts it.
    pub fn on_drag_enter(&mut self) -> bool {
        if self.click_through {
            return false;
        }
        self.drag_over = true;
        self.kill_timer(T_LEAVE);
        self.kill_timer(T_HOVER);
        self.kill_timer(T_TOAST);
        self.select_tab(TAB_FILES);
        self.transition(VisualState::Expanded);
        true
    }

    pub fn on_drag_leave(&mut self) {
        self.drag_over = false;
        self.schedule_collapse(320);
    }

    pub fn on_drop(&mut self, paths: Vec<String>) {
        self.drag_over = false;
        if self.self_drag {
            return;
        }
        for path in &paths {
            self.add_to_shelf(path, true);
        }
        self.update_files_ui();
        self.save_shelf();
    }

    fn add_to_shelf(&mut self, path: &str, animate: bool) {
        let p = std::path::Path::new(path);
        if !p.exists() {
            return;
        }
        if self.shelf.iter().any(|item| item.path.eq_ignore_ascii_case(path) || item.path.to_lowercase() == path.to_lowercase()) {
            return;
        }

        let trimmed = path.trim_end_matches(['\\', '/']);
        let name = trimmed.rsplit(['\\', '/']).next().filter(|n| !n.is_empty()).unwrap_or(path).to_string();
        if p.is_file() {
            self.share.publish(path);
        }

        let id = self.next_item_id;
        self.next_item_id += 1;
        self.shelf.push(ShelfItem { id, path: path.to_string(), name, icon: crate::shell::icons::load(path), is_dir: p.is_dir(), bitmap: None });
        if animate {
            // Ids start at 1; key 0 belongs to the colour history.
            self.anims.run(Key::PopScale(id), 0.6, 1.0, 0, 360, Ease::BackOut(0.5));
            self.anims.run(Key::PopOpacity(id), 0.0, 1.0, 0, 220, Ease::Linear);
        }
        self.files_scroll = f32::MAX;
        self.clamp_scroll();
        self.dirty = true;
    }

    fn forget_item(&mut self, item: &ShelfItem) {
        self.share.unshare(&item.path);
        self.anims.remove(Key::PopScale(item.id));
        self.anims.remove(Key::PopOpacity(item.id));
        self.anims.remove(Key::ChipActions(item.id));
    }

    fn remove_from_shelf(&mut self, id: u32) {
        if let Some(index) = self.shelf.iter().position(|i| i.id == id) {
            let item = self.shelf.remove(index);
            self.forget_item(&item);
        }
        self.clamp_scroll();
        self.update_files_ui();
        self.save_shelf();
    }

    fn clear_shelf(&mut self) {
        let items = std::mem::take(&mut self.shelf);
        for item in &items {
            self.forget_item(item);
        }
        self.files_scroll = 0.0;
        self.update_files_ui();
        self.save_shelf();
    }

    fn save_shelf(&self) {
        let paths: Vec<String> = self.shelf.iter().map(|i| i.path.clone()).collect();
        state::save_shelf(&paths);
    }

    fn update_files_ui(&mut self) {
        let count = self.shelf.len();
        self.files_info = if count == 0 {
            "Ablage ist leer".to_string()
        } else {
            format!("{count} {} · herausziehen oder Link teilen", if count == 1 { "Element" } else { "Elemente" })
        };
        self.dirty = true;
    }

    // Drag the file back out to Explorer, a browser, a chat window …
    fn drag_chip_out(&mut self, id: u32) {
        self.chip_drag = None;
        self.pressed = Id::None;
        let Some(path) = self.shelf.iter().find(|i| i.id == id).map(|i| i.path.clone()) else { return };

        self.hold_open = true;
        self.self_drag = true;
        unsafe {
            let _ = ReleaseCapture();
        }
        // The drag runs its own message loop: a timer keeps the frames coming.
        self.set_timer(T_MODAL, 16);
        dragdrop::drag_out(self.hwnd, &path);
        self.kill_timer(T_MODAL);

        self.self_drag = false;
        self.hold_open = false;
        self.drag_over = false;
        self.schedule_collapse(500);
    }

    fn share_link(&mut self, id: u32) {
        let Some(path) = self.shelf.iter().find(|i| i.id == id).map(|i| i.path.clone()) else { return };
        if std::path::Path::new(&path).is_dir() {
            self.files_info = "Ordner lassen sich nicht verlinken".to_string();
            return;
        }
        self.files_info = match self.share.share(&path) {
            Ok(url) => {
                self.ignore_clipboard_until = Instant::now() + Duration::from_secs(1);
                if clipboard::set_text(self.hwnd, &url) {
                    format!("Link kopiert · {url}")
                } else {
                    url
                }
            }
            Err(_) => "Freigabe fehlgeschlagen (Port 19846 belegt?)".to_string(),
        };
    }

    // =====================================================================
    //  Phone share (QR code + hub page)
    // =====================================================================

    fn open_share_panel(&mut self) {
        let url = match self.share.open_hub() {
            Ok(url) => url,
            Err(_) => {
                self.files_info = "Freigabe fehlgeschlagen (Port 19846 belegt?)".to_string();
                return;
            }
        };

        let Some(code) = qr::encode(&url) else {
            self.files_info = if clipboard::set_text(self.hwnd, &url) { format!("Link kopiert · {url}") } else { url };
            return;
        };

        self.qr = Some(code);
        self.share_url = Some(url);
        self.share_open = true;
        self.share_hint = SHARE_DEFAULT_HINT.to_string();
        self.anims.run(Key::SharePanel, 0.0, 1.0, 0, 180, Ease::Linear);
    }

    fn close_share_panel(&mut self) {
        self.share_open = false;
        self.qr = None;
        self.dirty = true;
    }

    fn on_file_received(&mut self, path: &str) {
        self.add_to_shelf(path, true);
        self.update_files_ui();
        self.save_shelf();

        let name = path.rsplit(['\\', '/']).next().unwrap_or(path).to_string();
        self.files_info = format!("Empfangen · {name}");
        self.share_hint = format!("Empfangen · {name}");
        self.enqueue_alert(IslandEvent::new("Datei empfangen", &name, EventType::Success, Some(100), "", 3200));
    }

    fn on_text_received(&mut self, text: &str) {
        self.ignore_clipboard_until = Instant::now() + Duration::from_secs(1);
        let copied = clipboard::set_text(self.hwnd, text);
        let preview = truncate_preview(text);
        self.share_hint = if copied { "Text empfangen · in Zwischenablage" } else { "Text empfangen · Kopieren fehlgeschlagen" }.to_string();
        self.enqueue_alert(IslandEvent::new("Text vom Handy", &preview, EventType::Clipboard, None, if copied { "COPY" } else { "" }, 3000));
    }

    // =====================================================================
    //  Controls tab: Wi-Fi, Bluetooth, nearby visibility
    // =====================================================================

    /// Fresh radio and network state while the controls tab is on screen.
    fn refresh_controls(&mut self) {
        radio::start();
        let (online, name) = network::status();
        self.net_name = if online { name } else { "Offline".to_string() };
        self.local_ip = if online { network::local_address() } else { String::new() };
        network::query_wifi();
        self.dirty = true;
    }

    fn toggle_radio(&mut self, kind: radio::Kind, status: RadioStatus) {
        match status {
            RadioStatus::On => radio::set(kind, false),
            RadioStatus::Off => radio::set(kind, true),
            RadioStatus::Disabled => {
                self.enqueue_alert(IslandEvent::new("Funk-Steuerung", "Gesperrt (Flugmodus oder Hardware-Schalter?)", EventType::Warning, None, "", 2600))
            }
            RadioStatus::Missing => self.enqueue_alert(IslandEvent::new("Funk-Steuerung", "Kein passender Adapter vorhanden", EventType::Warning, None, "", 2600)),
            RadioStatus::Unknown => radio::start(),
        }
    }

    /// Connected Bluetooth devices, each name once (dual-mode devices report twice).
    pub fn bt_connected_names(&self) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        for device in self.bt_devices.values() {
            let name = device.name.trim();
            if device.connected && !name.is_empty() && !names.iter().any(|n| n == name) {
                names.push(name.to_string());
            }
        }
        names.sort();
        names
    }

    fn set_nearby_visible(&mut self, visible: bool) {
        self.nearby_visible = visible;
        airdrop::set_visible(visible);
        state::save_setting("nearby", if visible { "1" } else { "0" });
        self.dirty = true;
    }

    // =====================================================================
    //  Nearby share: sending
    // =====================================================================

    fn open_nearby(&mut self, files: Vec<String>) {
        let files: Vec<String> = files.into_iter().filter(|p| std::path::Path::new(p).is_file()).collect();
        if files.is_empty() {
            self.files_info = "Nur Dateien lassen sich senden, keine Ordner".to_string();
            return;
        }
        if !airdrop::probe() {
            self.files_info = "Nearby nicht verfügbar (Port 19847 belegt?)".to_string();
            return;
        }
        self.nearby_bytes = files.iter().filter_map(|p| std::fs::metadata(p).ok()).map(|m| m.len()).sum();
        self.nearby_files = files;
        self.nearby_open = true;
        self.nearby_hint = String::new();
        // Probes repeat only while the picker is open.
        self.set_timer(T_PROBE, 3000);
        self.anims.run(Key::SharePanel, 0.0, 1.0, 0, 180, Ease::Linear);
        self.dirty = true;
    }

    fn close_nearby(&mut self) {
        self.nearby_open = false;
        self.kill_timer(T_PROBE);
        self.dirty = true;
    }

    fn probe_peers(&mut self) {
        if !self.nearby_open {
            self.kill_timer(T_PROBE);
            return;
        }
        let before = self.peers.len();
        self.peers.retain(|p| p.seen.elapsed() < PEER_TIMEOUT);
        if self.peers.len() != before {
            self.dirty = true;
        }
        airdrop::probe();
    }

    fn send_to_peer(&mut self, index: usize) {
        if self.sending.is_some() {
            self.nearby_hint = "Es läuft bereits eine Übertragung".to_string();
            return;
        }
        let Some(peer) = self.peers.get(index) else { return };
        let (name, addr) = (peer.name.clone(), peer.addr);
        self.sending = Some((name.clone(), 0));
        self.nearby_hint = format!("Warte auf Annahme durch {name} …");
        airdrop::send(addr, name, self.nearby_files.clone());
        self.dirty = true;
    }

    // =====================================================================
    //  Nearby share: receiving
    // =====================================================================

    fn on_offer(&mut self, offer: u32, from: &str, names: &[String], total: u64) {
        if self.pending_offer.is_some() {
            // One question at a time.
            airdrop::answer(offer, false);
            return;
        }
        let what = if names.len() == 1 { format!("„{}“", names[0]) } else { format!("{} Dateien", names.len()) };
        let ev = IslandEvent::new(from, &format!("möchte {what} senden · {}", format_size(total)), EventType::Nearby, None, "", 0);
        self.enqueue_alert(ev);
        self.toast.offer = Some(offer);
        self.pending_offer = Some(offer);
        // The question wins over the open island.
        if self.state != VisualState::Toast {
            self.transition(VisualState::Toast);
        }
        self.set_timer(T_TOAST, airdrop::DECIDE_SECONDS as u32 * 1000 - 1500);
        self.dirty = true;
    }

    fn answer_offer(&mut self, accept: bool) {
        let Some(offer) = self.pending_offer.take() else { return };
        self.toast.offer = None;
        airdrop::answer(offer, accept);
        self.kill_timer(T_TOAST);
        let deferred = self.deferred_alert.take();
        if accept {
            let from = self.toast.title.clone();
            self.enqueue_alert(IslandEvent::new(&format!("Empfange von {from}"), "0 %", EventType::Nearby, Some(0), "", 8000));
        } else if let Some(ev) = deferred {
            self.enqueue_alert(ev);
        } else {
            self.set_timer(T_TOAST, 400);
        }
        self.dirty = true;
    }

    fn on_drop_done(&mut self, from: &str, paths: Vec<std::path::PathBuf>, complete: bool) {
        for path in &paths {
            self.add_to_shelf(&path.to_string_lossy(), true);
        }
        self.update_files_ui();
        self.save_shelf();
        let count = paths.len();
        let ev = if complete {
            let what = if count == 1 {
                paths[0].file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
            } else {
                format!("{count} Dateien")
            };
            self.files_info = format!("Empfangen von {from} · {what}");
            IslandEvent::new(&format!("Von {from} empfangen"), &what, EventType::Success, Some(100), "", 3200)
        } else {
            IslandEvent::new("Empfang abgebrochen", &format!("Verbindung zu {from} verloren · {count} gespeichert"), EventType::Error, None, "", 3200)
        };
        self.enqueue_alert(ev);
    }

    // =====================================================================
    //  Now playing
    // =====================================================================

    /// Through the media session; media keys when none is there.
    fn media_command(&mut self, cmd: MediaCommand, vk: u8) {
        if !media::command(cmd) {
            send_media_key(vk);
        }
    }

    /// Click on the media timeline: jump there.
    fn seek_media(&mut self, x: f32) {
        if !self.media.can_seek || self.media.duration_ms <= 0 {
            return;
        }
        let bar = scene::MEDIA_BAR;
        let fraction = ((x - bar.x) / bar.w).clamp(0.0, 1.0);
        let ms = (self.media.duration_ms as f64 * fraction as f64) as i64;
        media::command(MediaCommand::Seek(ms));
        // Shown right away; the session confirms a moment later.
        self.media.position_ms = ms;
        self.media_at = Instant::now();
        self.dirty = true;
    }

    pub fn media_position(&self) -> i64 {
        media::position_now(&self.media, self.media_at)
    }

    pub fn glow_strength_factor(&self) -> f32 {
        match self.glow_strength {
            0 => 0.55,
            2 => 1.45,
            _ => 1.0,
        }
    }

    /// Current inner background glow colour and opacity, if visible.
    pub fn glow_color(&self) -> Option<crate::render::Color> {
        let a = self.anims.get(Key::GlowVis, 0.0);
        if a <= 0.004 {
            return None;
        }
        let factor = self.glow_strength_factor();
        Some(crate::render::Color {
            r: self.anims.get(Key::GlowR, 0.0).clamp(0.0, 1.0),
            g: self.anims.get(Key::GlowG, 0.0).clamp(0.0, 1.0),
            b: self.anims.get(Key::GlowB, 0.0).clamp(0.0, 1.0),
            a: (a * factor).clamp(0.0, 1.45),
        })
    }

    fn update_media_glow(&mut self) {
        if !self.opt_glow {
            if self.anims.get(Key::GlowVis, 0.0) > 0.004 {
                self.anims.to(Key::GlowVis, 0.0, 0.0, 320, Ease::CubicOut);
                self.dirty = true;
            }
            return;
        }

        let want_glow = match self.glow_mode {
            2 => true, // Immer
            1 => self.media.active, // Nur Medien
            _ => self.media.active && self.media.playing, // Bei Musik
        };

        if want_glow {
            let (tr, tg, tb) = if self.media.active && (self.media.playing || self.glow_mode == 1) {
                if let Some(c) = &self.cover {
                    (c.glow.0 as f32 / 255.0, c.glow.1 as f32 / 255.0, c.glow.2 as f32 / 255.0)
                } else {
                    let acc = crate::render::accent();
                    (acc.r, acc.g, acc.b)
                }
            } else {
                let acc = crate::render::accent();
                (acc.r, acc.g, acc.b)
            };
            let cur_vis = self.anims.get(Key::GlowVis, 0.0);
            for (key, target) in [(Key::GlowR, tr), (Key::GlowG, tg), (Key::GlowB, tb)] {
                if cur_vis <= 0.01 {
                    self.anims.run(key, target, target, 0, 0, Ease::Linear);
                } else {
                    self.anims.to(key, target, target, 420, Ease::CubicOut);
                }
            }
            self.anims.to(Key::GlowVis, 0.0, 1.0, 380, Ease::CubicOut);
            self.dirty = true;
        } else if self.anims.get(Key::GlowVis, 0.0) > 0.004 {
            self.anims.to(Key::GlowVis, 0.0, 0.0, 420, Ease::CubicOut);
            self.dirty = true;
        }
    }

    // =====================================================================
    //  Weather
    // =====================================================================

    fn refresh_weather(&mut self) {
        self.weather_loading = true;
        weather::fetch(self.city.clone());
        self.set_timer(T_WEATHER, weather::REFRESH_MS);
        self.dirty = true;
    }

    fn commit_city(&mut self) {
        if !self.city_focus {
            return;
        }
        self.city_focus = false;
        if !self.input_focus {
            self.kill_timer(T_CARET);
        }
        let city = self.city_input.trim().to_string();
        self.city_input = city.clone();
        if city != self.city {
            self.city = city;
            state::save_setting("weather_city", &self.city);
            self.weather = None;
            self.weather_error = None;
            self.refresh_weather();
        }
        self.dirty = true;
    }

    // =====================================================================
    //  Hide shortcut
    // =====================================================================

    /// (Re-)registers the global hide shortcut on the island window.
    pub fn apply_hotkey(&mut self) {
        use windows::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS};
        unsafe {
            let _ = UnregisterHotKey(Some(self.hwnd), settings::HOTKEY_ID);
        }
        self.hotkey_problem = None;
        if self.opt_hotkey && !self.hotkey_recording {
            let ok = unsafe { RegisterHotKey(Some(self.hwnd), settings::HOTKEY_ID, HOT_KEY_MODIFIERS(self.hotkey_mods | settings::MOD_NOREPEAT), self.hotkey_vk) }.is_ok();
            if !ok {
                self.hotkey_problem = Some("Kürzel ist schon von einem anderen Programm belegt".to_string());
            }
        }
        // Without a shortcut the island must not stay hidden.
        if !self.opt_hotkey && self.hidden {
            self.toggle_hidden();
        }
        self.dirty = true;
    }

    pub fn toggle_hidden(&mut self) {
        use windows::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE, SW_SHOWNOACTIVATE};
        if self.hidden {
            self.hidden = false;
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
            }
            self.ensure_topmost();
        } else {
            self.pinned = false;
            self.blur_input(false);
            self.kill_timer(T_TOAST);
            self.kill_timer(T_HOVER);
            self.transition(VisualState::Compact);
            self.set_hot(Id::None);
            self.hidden = true;
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
        }
        self.dirty = true;
    }

    // =====================================================================
    //  Settings window
    // =====================================================================

    pub fn open_settings(&mut self) {
        use windows::Win32::UI::WindowsAndMessaging::{IsIconic, SetForegroundWindow, ShowWindow, SW_RESTORE, SW_SHOW};
        if let Some(s) = &self.settings {
            unsafe {
                if IsIconic(s.hwnd).as_bool() {
                    let _ = ShowWindow(s.hwnd, SW_RESTORE);
                }
                let _ = SetForegroundWindow(s.hwnd);
            }
            return;
        }
        let Some(s) = settings::create() else { return };
        let hwnd = s.hwnd;
        self.settings = Some(s);
        self.city_input = self.city.clone();
        // First frame before the window shows, so it never flashes empty.
        self.dirty = true;
        self.frame();
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);
        }
    }

    fn draw_settings_window(&mut self) {
        let Some(mut s) = self.settings.take() else { return };
        if s.gfx.is_none() {
            let (w, h) = s.size_px();
            s.gfx = Gfx::new(s.hwnd, w, h, s.scale).ok();
            if let Some(g) = s.gfx.as_mut() {
                g.sync = 0;
            }
        }
        if let Some(mut g) = s.gfx.take() {
            // The scene records hits into App::hits: lend it the window's list.
            std::mem::swap(&mut self.hits, &mut s.hits);
            scene::draw_settings(&mut g, self);
            std::mem::swap(&mut self.hits, &mut s.hits);
            if g.end() {
                s.gfx = Some(g);
            } else {
                self.dirty = true;
            }
        }
        self.settings = Some(s);
    }

    pub fn settings_set_hot(&mut self, id: Id) {
        let Some(s) = self.settings.as_mut() else { return };
        if s.hot == id {
            return;
        }
        let old = std::mem::replace(&mut s.hot, id);
        if old != Id::None {
            self.anims.to(Key::Hover(old), 0.0, 0.0, 220, Ease::Linear);
        }
        if id != Id::None {
            self.anims.to(Key::Hover(id), 0.0, 1.0, 120, Ease::Linear);
        }
        self.dirty = true;
    }

    pub fn settings_mouse_move(&mut self, x: f32, y: f32) {
        let Some(s) = self.settings.as_ref() else { return };
        let id = s.hit_test(x, y);
        self.settings_set_hot(id);
    }

    pub fn settings_mouse_down(&mut self, x: f32, y: f32) {
        let Some(s) = self.settings.as_mut() else { return };
        let id = s.hit_test(x, y);
        s.pressed = id;
        if id != Id::CityInput {
            self.commit_city();
        }
        if id != Id::HotkeyRecord && self.hotkey_recording {
            self.hotkey_recording = false;
            self.apply_hotkey();
        }
        if id != Id::None {
            self.anims.to(Key::Press(id), 1.0, 0.94, 80, Ease::Linear);
        }
        self.dirty = true;
    }

    pub fn settings_mouse_up(&mut self, x: f32, y: f32) {
        let Some(s) = self.settings.as_mut() else { return };
        let id = std::mem::replace(&mut s.pressed, Id::None);
        let over = s.hit_test(x, y);
        if id == Id::None {
            return;
        }
        self.anims.to(Key::Press(id), 1.0, 1.0, 280, Ease::BackOut(0.6));
        self.dirty = true;
        if over == id {
            self.settings_click(id);
        }
    }

    fn settings_click(&mut self, id: Id) {
        use windows::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_MINIMIZE, WM_CLOSE};
        let Some(hwnd) = self.settings.as_ref().map(|s| s.hwnd) else { return };
        match id {
            Id::SettingsClose => unsafe {
                let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
            },
            Id::SettingsMin => unsafe {
                let _ = ShowWindow(hwnd, SW_MINIMIZE);
            },
            Id::HotkeyRecord => {
                self.hotkey_recording = !self.hotkey_recording;
                self.apply_hotkey();
            }
            Id::CityInput => {
                if !self.city_focus {
                    self.city_focus = true;
                    self.caret_on = true;
                    self.set_timer(T_CARET, 530);
                }
            }
            other => self.click(other),
        }
        self.dirty = true;
    }

    /// Key in the settings window. True when it was used.
    pub fn settings_key_down(&mut self, vk: u32) -> bool {
        if self.hotkey_recording {
            if settings::is_modifier(vk) {
                return true;
            }
            let mods = settings::held_modifiers();
            if vk == 0x1B && mods == 0 {
                self.hotkey_recording = false;
                self.apply_hotkey();
                return true;
            }
            let function_key = (0x70..=0x87).contains(&vk);
            if mods == 0 && !function_key {
                self.hotkey_problem = Some("Mit Strg, Alt, Umschalt oder Win kombinieren".to_string());
                self.dirty = true;
                return true;
            }
            self.hotkey_mods = mods;
            self.hotkey_vk = vk;
            self.hotkey_recording = false;
            state::save_setting("hotkey_mods", &mods.to_string());
            state::save_setting("hotkey_vk", &vk.to_string());
            if self.opt_hotkey {
                self.apply_hotkey();
            } else {
                // A new shortcut is meant to be used: switch it on.
                self.set_switch(SWITCH_HOTKEY, true, true);
            }
            return true;
        }
        if self.city_focus {
            // Text arrives as WM_CHAR.
            return false;
        }
        if vk == 0x1B {
            if let Some(s) = &self.settings {
                unsafe {
                    let _ = PostMessageW(Some(s.hwnd), windows::Win32::UI::WindowsAndMessaging::WM_CLOSE, WPARAM(0), LPARAM(0));
                }
            }
            return true;
        }
        false
    }

    pub fn settings_char(&mut self, c: u16) {
        if !self.city_focus {
            return;
        }
        match c {
            8 => {
                self.city_input.pop();
            }
            13 => {
                self.commit_city();
                return;
            }
            27 => {
                self.city_input = self.city.clone();
                self.commit_city();
                return;
            }
            22 => {
                if let Some(clipboard::Content::Text(text)) = clipboard::read(self.hwnd) {
                    for ch in text.chars().filter(|c| !c.is_control()) {
                        if self.city_input.chars().count() >= CITY_MAX {
                            break;
                        }
                        self.city_input.push(ch);
                    }
                }
            }
            c if c < 32 || (0xD800..=0xDFFF).contains(&c) => return,
            c => {
                if let Some(ch) = char::from_u32(c as u32) {
                    if self.city_input.chars().count() < CITY_MAX {
                        self.city_input.push(ch);
                    }
                }
            }
        }
        self.caret_on = true;
        self.set_timer(T_CARET, 530);
        self.dirty = true;
    }

    pub fn settings_deactivated(&mut self) {
        if self.hotkey_recording {
            self.hotkey_recording = false;
            self.apply_hotkey();
        }
        self.commit_city();
    }

    pub fn settings_destroyed(&mut self) {
        self.settings_deactivated();
        self.settings = None;
        self.ws_trim_due = true;
        self.set_timer(T_TRIM, 2000);
        self.dirty = true;
    }

    /// The tray menu runs its own message loop: a timer keeps the frames coming.
    pub fn show_tray_menu(&mut self) {
        self.set_timer(T_MODAL, 16);
        let cmd = tray::show_menu(self.hwnd, self.click_through, self.hidden);
        self.kill_timer(T_MODAL);
        self.on_tray_command(cmd);
    }
}

/// Key offset for the pop-in animation of nearby peers; far above any shelf item id.
pub const PEER_POP: u32 = 1 << 30;
/// Key offset for the pop-in animation of notification cards.
pub const NOTE_POP: u32 = 1 << 29;
/// Key offset for the pop-in animation of reminder rows.
pub const REM_POP: u32 = 1 << 28;

/// Local time of day in seconds.
fn local_seconds() -> u32 {
    let now = unsafe { GetLocalTime() };
    now.wHour as u32 * 3600 + now.wMinute as u32 * 60 + now.wSecond as u32
}

/// Gives unused pages back to the system: free heap blocks, then the working set.
fn trim_process_memory() {
    unsafe {
        use windows::Win32::System::Memory::{GetProcessHeap, HeapCompact, HEAP_FLAGS};
        use windows::Win32::System::ProcessStatus::EmptyWorkingSet;
        use windows::Win32::System::Threading::GetCurrentProcess;
        if let Ok(heap) = GetProcessHeap() {
            HeapCompact(heap, HEAP_FLAGS(0));
        }
        let _ = EmptyWorkingSet(GetCurrentProcess());
    }
}

/// Windows clipboard history (Win+V) for the app in the foreground.
fn open_clipboard_history() {
    use windows::Win32::UI::Input::KeyboardAndMouse::VK_LWIN;
    unsafe {
        keybd_event(VK_LWIN.0 as u8, 0, KEYEVENTF_EXTENDEDKEY, 0);
        keybd_event(b'V', 0, Default::default(), 0);
        keybd_event(b'V', 0, KEYEVENTF_KEYUP, 0);
        keybd_event(VK_LWIN.0 as u8, 0, KEYEVENTF_EXTENDEDKEY | KEYEVENTF_KEYUP, 0);
    }
}

fn send_media_key(vk: u8) {
    unsafe {
        keybd_event(vk, 0, KEYEVENTF_EXTENDEDKEY, 0);
        keybd_event(vk, 0, KEYEVENTF_EXTENDEDKEY | KEYEVENTF_KEYUP, 0);
    }
}
