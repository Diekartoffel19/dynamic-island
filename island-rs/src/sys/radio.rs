//! Wi-Fi and Bluetooth radios: state, change events and switching (WinRT Radio API).
//!
//! Started the first time the controls tab opens, so the radio stack is not
//! loaded while nobody looks at it. After that, changes arrive as events.

use std::sync::{Mutex, OnceLock};

use windows::core::IInspectable;
use windows::Devices::Radios::{Radio, RadioAccessStatus, RadioKind, RadioState};
use windows::Foundation::TypedEventHandler;
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

use crate::event::{post, EventType, IslandEvent, Msg};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Wifi,
    Bluetooth,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    /// Not known yet (radios not queried).
    Unknown,
    /// This PC has no such radio.
    Missing,
    Off,
    On,
    /// Switched off by hardware or policy; cannot be turned on from here.
    Disabled,
}

/// The first radio of each kind, kept alive so its StateChanged handler stays registered.
static RADIOS: OnceLock<Mutex<Vec<(Kind, Radio)>>> = OnceLock::new();

fn kind_of(radio: &Radio) -> Option<Kind> {
    match radio.Kind().ok()? {
        RadioKind::WiFi => Some(Kind::Wifi),
        RadioKind::Bluetooth => Some(Kind::Bluetooth),
        _ => None,
    }
}

fn status_of(radio: &Radio) -> Status {
    match radio.State() {
        Ok(RadioState::On) => Status::On,
        Ok(RadioState::Off) => Status::Off,
        Ok(RadioState::Disabled) => Status::Disabled,
        _ => Status::Unknown,
    }
}

/// Runs `f` on a short-lived worker thread inside the multithreaded apartment.
/// WinRT async calls are awaited there, never on the window thread.
fn worker(f: impl FnOnce() + Send + 'static) {
    let _ = std::thread::Builder::new().name("IslandRadio".into()).stack_size(256 * 1024).spawn(move || unsafe {
        let ok = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
        f();
        if ok {
            CoUninitialize();
        }
    });
}

/// Queries the radios once and subscribes to their changes. Later calls do nothing.
pub fn start() {
    if RADIOS.set(Mutex::new(Vec::new())).is_err() {
        return;
    }
    worker(|| {
        let mut found: Vec<(Kind, Radio)> = Vec::new();
        if let Ok(list) = Radio::GetRadiosAsync().and_then(|op| op.join()) {
            let count = list.Size().unwrap_or(0);
            for i in 0..count {
                let Ok(radio) = list.GetAt(i) else { continue };
                let Some(kind) = kind_of(&radio) else { continue };
                if found.iter().any(|(k, _)| *k == kind) {
                    continue;
                }
                let _ = radio.StateChanged(&TypedEventHandler::<Radio, IInspectable>::new(move |sender, _| {
                    if let Some(radio) = sender.as_ref() {
                        post(Msg::Radio { kind, status: status_of(radio) });
                    }
                    Ok(())
                }));
                post(Msg::Radio { kind, status: status_of(&radio) });
                found.push((kind, radio));
            }
        }
        for kind in [Kind::Wifi, Kind::Bluetooth] {
            if !found.iter().any(|(k, _)| *k == kind) {
                post(Msg::Radio { kind, status: Status::Missing });
            }
        }
        if let Some(store) = RADIOS.get() {
            *store.lock().unwrap() = found;
        }
    });
}

/// Switches a radio on or off. The new state arrives through StateChanged.
pub fn set(kind: Kind, on: bool) {
    worker(move || {
        let denied = |text: &str| {
            post(Msg::Alert(IslandEvent::new("Funk-Steuerung", text, EventType::Warning, None, "", 3600)));
        };
        let access = Radio::RequestAccessAsync().and_then(|op| op.join());
        if access != Ok(RadioAccessStatus::Allowed) {
            denied("Zugriff verweigert · Einstellungen > Datenschutz > Funkempfänger");
            return;
        }
        let radio = RADIOS.get().and_then(|store| store.lock().unwrap().iter().find(|(k, _)| *k == kind).map(|(_, r)| r.clone()));
        let Some(radio) = radio else {
            denied("Kein passender Funkempfänger gefunden");
            return;
        };
        let state = if on { RadioState::On } else { RadioState::Off };
        match radio.SetStateAsync(state).and_then(|op| op.join()) {
            Ok(RadioAccessStatus::Allowed) => {}
            Ok(RadioAccessStatus::DeniedBySystem) => denied("Vom System gesperrt (Flugmodus oder Hardware-Schalter?)"),
            _ => denied("Umschalten fehlgeschlagen"),
        }
    });
}
