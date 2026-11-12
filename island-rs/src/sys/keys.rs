//! CapsLock and NumLock through a Raw Input keyboard sink.

use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CAPITAL, VK_NUMLOCK};
use windows::Win32::UI::Input::{
    GetRawInputData, RegisterRawInputDevices, HRAWINPUT, RAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER, RIDEV_INPUTSINK, RID_INPUT,
    RIM_TYPEKEYBOARD,
};

pub fn register(hwnd: HWND) {
    let device = RAWINPUTDEVICE { usUsagePage: 1, usUsage: 6, dwFlags: RIDEV_INPUTSINK, hwndTarget: hwnd };
    unsafe {
        let _ = RegisterRawInputDevices(&[device], std::mem::size_of::<RAWINPUTDEVICE>() as u32);
    }
}

/// True if the WM_INPUT message is a CapsLock or NumLock key event.
pub fn is_lock_key(lparam: LPARAM) -> bool {
    unsafe {
        let mut raw = RAWINPUT::default();
        let mut size = std::mem::size_of::<RAWINPUT>() as u32;
        let read = GetRawInputData(
            HRAWINPUT(lparam.0 as *mut _),
            RID_INPUT,
            Some(&mut raw as *mut _ as *mut _),
            &mut size,
            std::mem::size_of::<RAWINPUTHEADER>() as u32,
        );
        if read == u32::MAX || raw.header.dwType != RIM_TYPEKEYBOARD.0 {
            return false;
        }
        let vk = raw.data.keyboard.VKey;
        vk == VK_CAPITAL.0 || vk == VK_NUMLOCK.0
    }
}

pub fn caps_lock() -> bool {
    unsafe { GetKeyState(VK_CAPITAL.0 as i32) & 1 != 0 }
}

pub fn num_lock() -> bool {
    unsafe { GetKeyState(VK_NUMLOCK.0 as i32) & 1 != 0 }
}
