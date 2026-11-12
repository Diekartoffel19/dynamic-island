//! Camera and microphone in use, from the Windows privacy consent store.
//!
//! Windows writes `LastUsedTimeStart` / `LastUsedTimeStop` per app under
//! HKCU\...\CapabilityAccessManager\ConsentStore\{webcam,microphone}. An app
//! with a start time and a stop time of 0 uses the device right now.
//! One thread per device sleeps in RegNotifyChangeKeyValue: no polling.

use windows::core::{w, PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, ERROR_SUCCESS};
use windows::Win32::System::Registry::{
    RegCloseKey, RegEnumKeyExW, RegGetValueW, RegNotifyChangeKeyValue, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER, KEY_NOTIFY, KEY_READ,
    REG_NOTIFY_CHANGE_LAST_SET, REG_NOTIFY_CHANGE_NAME, RRF_RT_REG_QWORD,
};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject, INFINITE};

use crate::event::{post, Msg};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Device {
    Camera,
    Microphone,
}

pub fn start() {
    for device in [Device::Camera, Device::Microphone] {
        let _ = std::thread::Builder::new().name("privacy".into()).stack_size(64 * 1024).spawn(move || watch(device));
    }
}

fn watch(device: Device) {
    let path = match device {
        Device::Camera => w!("Software\\Microsoft\\Windows\\CurrentVersion\\CapabilityAccessManager\\ConsentStore\\webcam"),
        Device::Microphone => w!("Software\\Microsoft\\Windows\\CurrentVersion\\CapabilityAccessManager\\ConsentStore\\microphone"),
    };
    unsafe {
        let mut key = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, path, None, KEY_READ | KEY_NOTIFY, &mut key) != ERROR_SUCCESS {
            return;
        }
        let Ok(event) = CreateEventW(None, false, false, PCWSTR::null()) else {
            let _ = RegCloseKey(key);
            return;
        };
        let mut last = false;
        loop {
            // Arm the notification first so no change between scan and wait is lost.
            if RegNotifyChangeKeyValue(key, true, REG_NOTIFY_CHANGE_LAST_SET | REG_NOTIFY_CHANGE_NAME, Some(event), true) != ERROR_SUCCESS {
                break;
            }
            let on = any_in_use(key, 0);
            if on != last {
                last = on;
                post(Msg::Privacy { device, on });
            }
            WaitForSingleObject(event, INFINITE);
        }
        let _ = CloseHandle(event);
        let _ = RegCloseKey(key);
    }
}

/// True if any app below `key` uses the device. `NonPackaged` holds desktop apps one level deeper.
unsafe fn any_in_use(key: HKEY, depth: u32) -> bool {
    let mut index = 0u32;
    loop {
        let mut name = [0u16; 512];
        let mut len = name.len() as u32;
        if RegEnumKeyExW(key, index, Some(PWSTR(name.as_mut_ptr())), &mut len, None, None, None, None) != ERROR_SUCCESS {
            return false;
        }
        index += 1;
        let sub = PCWSTR(name.as_ptr());
        let stop = read_qword(key, sub, w!("LastUsedTimeStop"));
        let start = read_qword(key, sub, w!("LastUsedTimeStart"));
        if start.unwrap_or(0) != 0 && stop == Some(0) {
            return true;
        }
        if depth == 0 && String::from_utf16_lossy(&name[..len as usize]) == "NonPackaged" {
            let mut child = HKEY::default();
            if RegOpenKeyExW(key, sub, None, KEY_READ, &mut child) == ERROR_SUCCESS {
                let found = any_in_use(child, 1);
                let _ = RegCloseKey(child);
                if found {
                    return true;
                }
            }
        }
    }
}

unsafe fn read_qword(key: HKEY, sub: PCWSTR, value: PCWSTR) -> Option<u64> {
    let mut data = 0u64;
    let mut size = 8u32;
    let status = RegGetValueW(key, sub, value, RRF_RT_REG_QWORD, None, Some(&mut data as *mut u64 as *mut _), Some(&mut size));
    (status == ERROR_SUCCESS).then_some(data)
}
