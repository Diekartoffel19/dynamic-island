//! Events that flow from the system sources and servers to the window thread.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::OnceLock;

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

/// Posted to the main window when a `Msg` is waiting in the channel.
pub const WM_APP_MSG: u32 = WM_APP + 1;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EventType {
    Info,
    Success,
    Warning,
    Error,
    Audio,
    Power,
    Network,
    Clipboard,
    KeyLock,
    Timer,
    Bluetooth,
    Usb,
    Nearby,
}

#[derive(Clone, Debug)]
pub struct IslandEvent {
    pub title: String,
    pub message: String,
    pub kind: EventType,
    /// 0..100, shows a progress bar when set.
    pub progress: Option<i32>,
    pub badge: String,
    pub duration_ms: u32,
}

impl IslandEvent {
    pub fn new(title: &str, message: &str, kind: EventType, progress: Option<i32>, badge: &str, duration_ms: u32) -> Self {
        IslandEvent {
            title: title.to_string(),
            message: message.to_string(),
            kind,
            progress,
            badge: badge.to_string(),
            duration_ms,
        }
    }
}

pub enum Msg {
    Alert(IslandEvent),
    Volume { percent: i32, muted: bool },
    AudioDeviceChanged,
    NetworkChanged,
    BtAdded { id: String, name: String, connected: bool, initial: bool },
    BtUpdated { id: String, connected: Option<bool>, name: Option<String> },
    BtRemoved { id: String },
    FileReceived(PathBuf),
    TextReceived(String),
    FileDownloaded(String),
    /// Wi-Fi or Bluetooth radio state.
    Radio { kind: crate::sys::radio::Kind, status: crate::sys::radio::Status },
    /// Wi-Fi network name and signal bars, None when not on Wi-Fi.
    Wifi(Option<(String, u8)>),
    // ---- Nearby share ----
    PeerFound { id: String, name: String, addr: SocketAddr },
    PeerGone { id: String },
    DropOffer { offer: u32, from: String, names: Vec<String>, total: u64 },
    /// The offer was answered or timed out on the receiving thread.
    DropClosed { offer: u32 },
    DropProgress { from: String, percent: i32 },
    DropDone { from: String, paths: Vec<PathBuf>, complete: bool },
    SendProgress { to: String, percent: i32 },
    SendDone { to: String, outcome: crate::net::airdrop::SendOutcome },
    /// Camera or microphone started or stopped being used by some app.
    Privacy { device: crate::sys::privacy::Device, on: bool },
    /// Now playing changed (title, state, timeline).
    Media(crate::sys::media::MediaInfo),
    /// Cover of the current track; None while there is none (yet).
    MediaCover(Option<crate::sys::media::Cover>),
    Weather(Result<crate::sys::weather::Weather, crate::sys::weather::WeatherError>),
}

static MAIN_HWND: AtomicIsize = AtomicIsize::new(0);
static TX: OnceLock<Sender<Msg>> = OnceLock::new();

/// Call once on the window thread. Returns the receiving end.
pub fn init(hwnd: HWND) -> Receiver<Msg> {
    let (tx, rx) = std::sync::mpsc::channel();
    let _ = TX.set(tx);
    MAIN_HWND.store(hwnd.0 as isize, Ordering::SeqCst);
    rx
}

/// Sends a message to the window thread. Safe to call from any thread.
pub fn post(msg: Msg) {
    let Some(tx) = TX.get() else { return };
    if tx.send(msg).is_err() {
        return;
    }
    let hwnd = MAIN_HWND.load(Ordering::SeqCst);
    if hwnd != 0 {
        unsafe {
            let _ = PostMessageW(Some(HWND(hwnd as *mut _)), WM_APP_MSG, WPARAM(0), LPARAM(0));
        }
    }
}
