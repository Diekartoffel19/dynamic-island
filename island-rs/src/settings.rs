//! Settings as a window of its own: movable, in the taskbar and Alt+Tab, takes
//! the keyboard (for the city field and for recording the hide shortcut).
//!
//! Drawn with the same Direct2D code as the island (`scene::draw_settings`).
//! The window thread renders it inside `App::frame`, right after the island.

use std::sync::atomic::{AtomicBool, Ordering};

use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{ScreenToClient, ValidateRect};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForSystem;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyNameTextW, GetKeyState, MapVirtualKeyW, ReleaseCapture, SetCapture, TrackMouseEvent, MAPVK_VK_TO_VSC, TME_LEAVE, TRACKMOUSEEVENT,
    VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::island::{Hit, Id};
use crate::render::Gfx;
use crate::window::app_opt;

const WM_MOUSELEAVE: u32 = 0x02A3;

/// Window size in DIP.
pub const SW_W: f32 = 480.0;
pub const SW_H: f32 = 500.0;
/// Height of the drag area at the top.
pub const TITLE_H: f32 = 48.0;
/// Tabs in the settings window.
pub const SETTINGS_TAB_COUNT: usize = 4;

// RegisterHotKey modifier bits.
pub const MOD_ALT: u32 = 0x1;
pub const MOD_CONTROL: u32 = 0x2;
pub const MOD_SHIFT: u32 = 0x4;
pub const MOD_WIN: u32 = 0x8;
pub const MOD_NOREPEAT: u32 = 0x4000;
/// Id of the hide/show hotkey on the island window.
pub const HOTKEY_ID: i32 = 1;

pub struct SettingsWindow {
    pub hwnd: HWND,
    pub gfx: Option<Gfx>,
    pub hits: Vec<Hit>,
    pub hot: Id,
    pub pressed: Id,
    pub scale: f32,
    tracking: bool,
}

static CLASS_DONE: AtomicBool = AtomicBool::new(false);

/// Creates the window (hidden). The caller stores it and shows it after the first frame.
pub fn create() -> Option<SettingsWindow> {
    unsafe {
        let instance = GetModuleHandleW(None).ok()?;
        let class_name = w!("DynamicIslandSettings");
        if !CLASS_DONE.swap(true, Ordering::Relaxed) {
            let icon = crate::shell::tray::create_icon();
            let class = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(wnd_proc),
                hInstance: instance.into(),
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                hIcon: icon,
                hIconSm: icon,
                lpszClassName: class_name,
                ..Default::default()
            };
            RegisterClassExW(&class);
        }
        let scale = GetDpiForSystem() as f32 / 96.0;
        let (w, h) = ((SW_W * scale).round() as i32, (SW_H * scale).round() as i32);
        // Centre on the work area of the primary screen.
        let mut work = RECT::default();
        let _ = SystemParametersInfoW(SPI_GETWORKAREA, 0, Some(&mut work as *mut _ as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));
        let x = work.left + ((work.right - work.left) - w) / 2;
        let y = work.top + ((work.bottom - work.top) - h).max(0) / 2;
        let hwnd = CreateWindowExW(
            WS_EX_NOREDIRECTIONBITMAP | WS_EX_APPWINDOW,
            class_name,
            w!("Dynamic Island – Einstellungen"),
            WS_POPUP | WS_SYSMENU | WS_MINIMIZEBOX,
            x,
            y,
            w,
            h,
            None,
            None,
            Some(instance.into()),
            None,
        )
        .ok()?;
        Some(SettingsWindow { hwnd, gfx: None, hits: Vec::new(), hot: Id::None, pressed: Id::None, scale, tracking: false })
    }
}

impl SettingsWindow {
    pub fn size_px(&self) -> (u32, u32) {
        ((SW_W * self.scale).round() as u32, (SW_H * self.scale).round() as u32)
    }

    pub fn hit_test(&self, x: f32, y: f32) -> Id {
        self.hits.iter().rev().find(|h| h.rect.contains(x, y)).map(|h| h.id).unwrap_or(Id::None)
    }
}

fn dip(lparam: LPARAM, scale: f32) -> (f32, f32) {
    let x = (lparam.0 & 0xFFFF) as i16 as f32;
    let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as f32;
    (x / scale, y / scale)
}

fn key_down(vk: u16) -> bool {
    unsafe { (GetKeyState(vk as i32) as u16 & 0x8000) != 0 }
}

/// Modifier bits held right now.
pub fn held_modifiers() -> u32 {
    let mut mods = 0;
    if key_down(VK_CONTROL.0) {
        mods |= MOD_CONTROL;
    }
    if key_down(VK_MENU.0) {
        mods |= MOD_ALT;
    }
    if key_down(VK_SHIFT.0) {
        mods |= MOD_SHIFT;
    }
    if key_down(VK_LWIN.0) || key_down(VK_RWIN.0) {
        mods |= MOD_WIN;
    }
    mods
}

/// Shift, Ctrl, Alt, Win (left/right variants included).
pub fn is_modifier(vk: u32) -> bool {
    matches!(vk, 0x10 | 0x11 | 0x12 | 0x5B | 0x5C | 0xA0..=0xA5)
}

fn key_name(vk: u32) -> String {
    match vk {
        0x30..=0x39 | 0x41..=0x5A => char::from_u32(vk).map(|c| c.to_string()).unwrap_or_default(),
        0x70..=0x87 => format!("F{}", vk - 0x6F),
        0x20 => "Leertaste".to_string(),
        _ => unsafe {
            let scan = MapVirtualKeyW(vk, MAPVK_VK_TO_VSC);
            // Navigation keys and the numpad divide are "extended".
            let extended = matches!(vk, 0x21..=0x2E | 0x6F | 0x90) as i32;
            let mut buf = [0u16; 32];
            let len = GetKeyNameTextW(((scan as i32) << 16) | (extended << 24), &mut buf);
            if len > 0 {
                String::from_utf16_lossy(&buf[..len as usize])
            } else {
                format!("Taste {vk}")
            }
        },
    }
}

/// "Strg + Alt + I".
pub fn hotkey_text(mods: u32, vk: u32) -> String {
    let mut parts: Vec<String> = Vec::new();
    if mods & MOD_WIN != 0 {
        parts.push("Win".into());
    }
    if mods & MOD_CONTROL != 0 {
        parts.push("Strg".into());
    }
    if mods & MOD_ALT != 0 {
        parts.push("Alt".into());
    }
    if mods & MOD_SHIFT != 0 {
        parts.push("Umschalt".into());
    }
    parts.push(key_name(vk));
    parts.join(" + ")
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let Some(app) = app_opt() else {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    };
    // Messages during CreateWindowExW arrive before the window is stored.
    let Some(scale) = app.settings.as_ref().filter(|s| s.hwnd == hwnd).map(|s| s.scale) else {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    };

    match msg {
        WM_NCHITTEST => {
            let mut p = POINT { x: (lparam.0 & 0xFFFF) as i16 as i32, y: ((lparam.0 >> 16) & 0xFFFF) as i16 as i32 };
            let _ = ScreenToClient(hwnd, &mut p);
            let (x, y) = (p.x as f32 / scale, p.y as f32 / scale);
            let over_button = app.settings.as_ref().map(|s| s.hit_test(x, y) != Id::None).unwrap_or(false);
            return LRESULT(if y < TITLE_H && !over_button { HTCAPTION } else { HTCLIENT } as isize);
        }
        WM_MOUSEMOVE => {
            let (x, y) = dip(lparam, scale);
            if let Some(s) = app.settings.as_mut() {
                if !s.tracking {
                    let mut track = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                    let _ = TrackMouseEvent(&mut track);
                    s.tracking = true;
                }
            }
            app.settings_mouse_move(x, y);
        }
        WM_MOUSELEAVE => {
            if let Some(s) = app.settings.as_mut() {
                s.tracking = false;
            }
            app.settings_set_hot(Id::None);
        }
        WM_LBUTTONDOWN => {
            let (x, y) = dip(lparam, scale);
            SetCapture(hwnd);
            app.settings_mouse_down(x, y);
        }
        WM_LBUTTONUP => {
            let (x, y) = dip(lparam, scale);
            let _ = ReleaseCapture();
            app.settings_mouse_up(x, y);
        }
        WM_SETCURSOR => {
            if (lparam.0 & 0xFFFF) as u32 == HTCLIENT {
                let hot = app.settings.as_ref().map(|s| s.hot).unwrap_or(Id::None);
                let cursor = if hot == Id::CityInput {
                    IDC_IBEAM
                } else if hot != Id::None {
                    IDC_HAND
                } else {
                    IDC_ARROW
                };
                SetCursor(LoadCursorW(None, cursor).ok());
                return LRESULT(1);
            }
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            if app.settings_key_down(wparam.0 as u32) {
                return LRESULT(0);
            }
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        WM_SYSCHAR if app.hotkey_recording => {}
        WM_CHAR => app.settings_char(wparam.0 as u16),
        WM_ACTIVATE => {
            if (wparam.0 & 0xFFFF) as u32 == WA_INACTIVE {
                app.settings_deactivated();
            }
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        WM_PAINT => {
            let _ = ValidateRect(Some(hwnd), None);
            app.invalidate();
        }
        WM_CLOSE => {
            let _ = DestroyWindow(hwnd);
        }
        WM_DESTROY => app.settings_destroyed(),
        _ => return DefWindowProcW(hwnd, msg, wparam, lparam),
    }
    LRESULT(0)
}
