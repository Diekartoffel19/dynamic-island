//! Shell integration: tray icon, drag and drop, file icons, colour loupe, tooltips.

pub mod dragdrop;
pub mod icons;
pub mod picker;
pub mod tooltip;
pub mod tray;

use windows::core::PCWSTR;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Opens a file or folder with its default program.
pub fn open(path: &str) -> bool {
    let path = wide(path);
    unsafe { ShellExecuteW(None, PCWSTR::null(), PCWSTR(path.as_ptr()), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL).0 as usize > 32 }
}

/// Shows the file selected in an Explorer window.
pub fn reveal(path: &str) {
    let exe = wide("explorer.exe");
    let args = wide(&format!("/select,\"{path}\""));
    unsafe {
        ShellExecuteW(None, PCWSTR::null(), PCWSTR(exe.as_ptr()), PCWSTR(args.as_ptr()), PCWSTR::null(), SW_SHOWNORMAL);
    }
}
