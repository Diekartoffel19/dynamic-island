//! Clipboard text in and out. Changes arrive as WM_CLIPBOARDUPDATE.

use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE};

const CF_BITMAP: u32 = 2;
const CF_DIB: u32 = 8;
const CF_UNICODETEXT: u32 = 13;

pub enum Content {
    Text(String),
    Image,
}

fn open(hwnd: HWND) -> bool {
    // Another program may hold the clipboard for a moment.
    for _ in 0..5 {
        if unsafe { OpenClipboard(Some(hwnd)) }.is_ok() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(8));
    }
    false
}

pub fn read(hwnd: HWND) -> Option<Content> {
    unsafe {
        if IsClipboardFormatAvailable(CF_UNICODETEXT).is_ok() {
            if !open(hwnd) {
                return None;
            }
            let mut result = None;
            if let Ok(handle) = GetClipboardData(CF_UNICODETEXT) {
                let global = HGLOBAL(handle.0);
                let ptr = GlobalLock(global) as *const u16;
                if !ptr.is_null() {
                    // Only the start is shown, so only the start is read.
                    let max = (GlobalSize(global) / 2).min(256);
                    let slice = std::slice::from_raw_parts(ptr, max);
                    let len = slice.iter().position(|&c| c == 0).unwrap_or(max);
                    result = Some(Content::Text(String::from_utf16_lossy(&slice[..len])));
                    let _ = GlobalUnlock(global);
                }
            }
            let _ = CloseClipboard();
            return result;
        }
        if IsClipboardFormatAvailable(CF_BITMAP).is_ok() || IsClipboardFormatAvailable(CF_DIB).is_ok() {
            return Some(Content::Image);
        }
        None
    }
}

pub fn set_text(hwnd: HWND, text: &str) -> bool {
    unsafe {
        let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        let Ok(global) = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2) else { return false };
        let ptr = GlobalLock(global) as *mut u16;
        if ptr.is_null() {
            let _ = GlobalFree(Some(global));
            return false;
        }
        std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr, wide.len());
        let _ = GlobalUnlock(global);

        if !open(hwnd) {
            let _ = GlobalFree(Some(global));
            return false;
        }
        let _ = EmptyClipboard();
        let ok = SetClipboardData(CF_UNICODETEXT, Some(HANDLE(global.0))).is_ok();
        if !ok {
            let _ = GlobalFree(Some(global));
        }
        let _ = CloseClipboard();
        ok
    }
}
