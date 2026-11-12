//! Drive arrival and removal from WM_DEVICECHANGE.

use windows::core::PCWSTR;
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::Storage::FileSystem::GetVolumeInformationW;

use crate::event::{EventType, IslandEvent};

const DBT_DEVICEARRIVAL: usize = 0x8000;
const DBT_DEVICEREMOVECOMPLETE: usize = 0x8004;
const DBT_DEVTYP_VOLUME: u32 = 2;

fn volume_label(drive: &str) -> Option<String> {
    let root: Vec<u16> = format!("{drive}\\").encode_utf16().chain(std::iter::once(0)).collect();
    let mut name = [0u16; 261];
    unsafe { GetVolumeInformationW(PCWSTR(root.as_ptr()), Some(&mut name), None, None, None, None).ok()? };
    let len = name.iter().position(|&c| c == 0).unwrap_or(0);
    let label = String::from_utf16_lossy(&name[..len]);
    if label.trim().is_empty() {
        None
    } else {
        Some(label)
    }
}

pub fn events(wparam: WPARAM, lparam: LPARAM) -> Vec<IslandEvent> {
    let mut out = Vec::new();
    let arrived = wparam.0 == DBT_DEVICEARRIVAL;
    if !(arrived || wparam.0 == DBT_DEVICEREMOVECOMPLETE) || lparam.0 == 0 {
        return out;
    }

    // DEV_BROADCAST_VOLUME: device type at offset 4, drive bit mask at offset 12.
    let (device_type, unit_mask) = unsafe {
        let base = lparam.0 as *const u8;
        ((base.add(4) as *const u32).read_unaligned(), (base.add(12) as *const u32).read_unaligned())
    };
    if device_type != DBT_DEVTYP_VOLUME {
        return out;
    }

    for i in 0..26u32 {
        if unit_mask & (1 << i) == 0 {
            continue;
        }
        let drive = format!("{}:", (b'A' + i as u8) as char);
        let label = if arrived { volume_label(&drive) } else { None };
        let title = label.unwrap_or_else(|| format!("Laufwerk {drive}"));
        let message = if arrived { format!("Laufwerk {drive} angeschlossen") } else { format!("Laufwerk {drive} entfernt") };
        out.push(IslandEvent::new(
            &title,
            &message,
            if arrived { EventType::Usb } else { EventType::Warning },
            None,
            if arrived { &drive } else { "OFF" },
            2800,
        ));
    }
    out
}
