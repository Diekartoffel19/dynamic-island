//! Colour picker: a low-level mouse hook catches the click, a small BitBlt feeds the loupe.

use std::sync::atomic::{AtomicIsize, Ordering};

use windows::Win32::Foundation::{LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER,
    BI_RGB, DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ, SRCCOPY,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_ESCAPE};
use windows::Win32::UI::WindowsAndMessaging::{CallNextHookEx, GetCursorPos, SetWindowsHookExW, UnhookWindowsHookEx, HHOOK, WH_MOUSE_LL};

use crate::window::app_opt;

/// The loupe shows LOUPE x LOUPE screen pixels around the cursor.
pub const LOUPE: usize = 11;

static HOOK: AtomicIsize = AtomicIsize::new(0);

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        if let Some(app) = app_opt() {
            if app.on_pick_mouse(wparam.0 as u32) {
                return LRESULT(1);
            }
        }
    }
    CallNextHookEx(None, code, wparam, lparam)
}

pub fn install_hook() -> bool {
    remove_hook();
    match unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(hook_proc), None, 0) } {
        Ok(hook) => {
            HOOK.store(hook.0 as isize, Ordering::SeqCst);
            true
        }
        Err(_) => false,
    }
}

pub fn remove_hook() {
    let hook = HOOK.swap(0, Ordering::SeqCst);
    if hook != 0 {
        unsafe {
            let _ = UnhookWindowsHookEx(HHOOK(hook as *mut _));
        }
    }
}

pub fn escape_down() -> bool {
    unsafe { GetAsyncKeyState(VK_ESCAPE.0 as i32) as u16 & 0x8000 != 0 }
}

/// Reusable screen grab of the pixels around the cursor.
pub struct Loupe {
    dc: HDC,
    bitmap: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u32,
}

impl Loupe {
    pub fn new() -> Option<Loupe> {
        unsafe {
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: LOUPE as i32,
                    biHeight: -(LOUPE as i32),
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let dc = CreateCompatibleDC(None);
            let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
            let Ok(bitmap) = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0) else {
                let _ = DeleteDC(dc);
                return None;
            };
            if bits.is_null() {
                let _ = DeleteObject(bitmap.into());
                let _ = DeleteDC(dc);
                return None;
            }
            let old = SelectObject(dc, bitmap.into());
            Some(Loupe { dc, bitmap, old, bits: bits as *mut u32 })
        }
    }

    /// Copies the pixels around the cursor into `out` (0x00RRGGBB) and returns the centre pixel.
    pub fn sample(&mut self, out: &mut [u32; LOUPE * LOUPE]) -> Option<(u8, u8, u8)> {
        unsafe {
            let mut p = POINT::default();
            GetCursorPos(&mut p).ok()?;
            let half = (LOUPE / 2) as i32;
            let screen = GetDC(None);
            let ok = BitBlt(self.dc, 0, 0, LOUPE as i32, LOUPE as i32, Some(screen), p.x - half, p.y - half, SRCCOPY).is_ok();
            ReleaseDC(None, screen);
            if !ok {
                return None;
            }
            let pixels = std::slice::from_raw_parts(self.bits, LOUPE * LOUPE);
            for (dst, src) in out.iter_mut().zip(pixels) {
                *dst = src & 0x00FF_FFFF;
            }
            let c = out[(LOUPE / 2) * LOUPE + LOUPE / 2];
            Some(((c >> 16) as u8, (c >> 8) as u8, c as u8))
        }
    }
}

impl Drop for Loupe {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.old);
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.dc);
        }
    }
}
