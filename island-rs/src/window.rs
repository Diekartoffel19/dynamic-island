//! Overlay window, WndProc and the message loop.
//!
//! The loop sleeps in GetMessage while nothing moves. While a spring or tween
//! is active it drains the queue and renders once per vertical blank.

use std::sync::atomic::{AtomicPtr, AtomicU32, Ordering};

use windows::core::{w, BOOL};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, SYSTEMTIME, WPARAM};
use windows::Win32::Globalization::GetDateFormatEx;
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, MonitorFromWindow, ValidateRect, HDC, HMONITOR, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Accessibility::{SetWinEventHook, HWINEVENTHOOK};
use windows::Win32::UI::HiDpi::GetDpiForSystem;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::event::{self, WM_APP_MSG};
use crate::island::App;
use crate::render::scene::{WIN_H, WIN_W};
use crate::shell::tray::{self, WM_TRAY};
use crate::sys::{drives, keys};

/// Posted by the mouse hook when a colour pick ends. wParam: 1 = commit.
pub const WM_APP_PICK_END: u32 = WM_APP + 3;

const WM_MOUSELEAVE: u32 = 0x02A3;
const PBT_APMPOWERSTATUSCHANGE: usize = 0x000A;
const PBT_POWERSETTINGCHANGE: usize = 0x8013;
const EVENT_SYSTEM_FOREGROUND: u32 = 0x0003;
const WINEVENT_OUTOFCONTEXT: u32 = 0;

static APP: AtomicPtr<App> = AtomicPtr::new(std::ptr::null_mut());
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

#[derive(Clone, Copy, Debug)]
pub struct MonitorInfo {
    pub bounds: RECT,
    pub work: RECT,
    pub primary: bool,
}

unsafe extern "system" fn enum_monitor_cb(hmon: HMONITOR, _hdc: HDC, _rect: *mut RECT, data: LPARAM) -> BOOL {
    let list = &mut *(data.0 as *mut Vec<MonitorInfo>);
    let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    if GetMonitorInfoW(hmon, &mut info).as_bool() {
        list.push(MonitorInfo {
            bounds: info.rcMonitor,
            work: info.rcWork,
            primary: (info.dwFlags & 1) != 0,
        });
    }
    BOOL(1)
}

/// All active monitors, sorted left-to-right then top-to-bottom.
pub fn monitors() -> Vec<MonitorInfo> {
    let mut list: Vec<MonitorInfo> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(enum_monitor_cb), LPARAM(&mut list as *mut _ as isize));
    }
    list.sort_by_key(|m| (m.bounds.left, m.bounds.top));
    if list.is_empty() {
        let w = unsafe { GetSystemMetrics(SM_CXSCREEN) };
        let h = unsafe { GetSystemMetrics(SM_CYSCREEN) };
        let r = RECT { left: 0, top: 0, right: w, bottom: h };
        list.push(MonitorInfo { bounds: r, work: r, primary: true });
    }
    list
}

/// Index in `list` matching `hwnd`'s monitor.
pub fn monitor_index_for_hwnd(hwnd: HWND, list: &[MonitorInfo]) -> usize {
    unsafe {
        let hmon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if GetMonitorInfoW(hmon, &mut info).as_bool() {
            if let Some(i) = list.iter().position(|m| m.bounds.left == info.rcMonitor.left && m.bounds.top == info.rcMonitor.top) {
                return i;
            }
        }
    }
    list.iter().position(|m| m.primary).unwrap_or(0)
}

/// The one App. Only touch it on the window thread.
pub fn app() -> &'static mut App {
    app_opt().expect("app not created")
}

pub fn app_opt() -> Option<&'static mut App> {
    unsafe { APP.load(Ordering::Relaxed).as_mut() }
}

/// "Do, 12. Nov" style date, from the same Windows locale data .NET uses.
pub fn format_date(time: &SYSTEMTIME) -> String {
    let mut buffer = [0u16; 64];
    let len = unsafe { GetDateFormatEx(w!("de-DE"), Default::default(), Some(time), w!("ddd, d. MMM"), Some(&mut buffer), None) };
    if len <= 1 {
        return String::new();
    }
    String::from_utf16_lossy(&buffer[..len as usize - 1])
}

/// "Donnerstag, 12. November" style date for the start tab.
pub fn format_date_long(time: &SYSTEMTIME) -> String {
    let mut buffer = [0u16; 80];
    let len = unsafe { GetDateFormatEx(w!("de-DE"), Default::default(), Some(time), w!("dddd, d. MMMM"), Some(&mut buffer), None) };
    if len <= 1 {
        return String::new();
    }
    String::from_utf16_lossy(&buffer[..len as usize - 1])
}

fn dip(lparam: LPARAM, scale: f32) -> (f32, f32) {
    let x = (lparam.0 & 0xFFFF) as i16 as f32;
    let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as f32;
    (x / scale, y / scale)
}

pub fn run() {
    unsafe {
        let instance = GetModuleHandleW(None).unwrap_or_default();
        let class_name = w!("DynamicIslandWindow");
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_DBLCLKS,
            lpfnWndProc: Some(wnd_proc),
            hInstance: instance.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: class_name,
            ..Default::default()
        };
        if RegisterClassExW(&class) == 0 {
            return;
        }
        TASKBAR_CREATED.store(RegisterWindowMessageW(w!("TaskbarCreated")), Ordering::Relaxed);

        let scale = GetDpiForSystem() as f32 / 96.0;
        let width = (WIN_W * scale).round() as i32;
        let height = (WIN_H * scale).round() as i32;
        let x = (GetSystemMetrics(SM_CXSCREEN) - width) / 2;

        let Ok(hwnd) = CreateWindowExW(
            WS_EX_NOREDIRECTIONBITMAP | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            class_name,
            w!("Dynamic Island"),
            WS_POPUP,
            x,
            0,
            width,
            height,
            None,
            None,
            Some(instance.into()),
            None,
        ) else {
            return;
        };

        let rx = event::init(hwnd);
        let app = Box::leak(Box::new(App::new(hwnd, scale, rx)));
        APP.store(app, Ordering::Relaxed);
        app.apply_position();

        // First frame before the window shows, so it never flashes empty.
        app.frame();
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        app.start();

        // Other topmost windows may cover the island: move back up when the foreground changes.
        SetWinEventHook(EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND, None, Some(on_foreground_change), 0, 0, WINEVENT_OUTOFCONTEXT);

        let mut msg = MSG::default();
        'run: loop {
            if app.wants_frame() {
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    if msg.message == WM_QUIT {
                        break 'run;
                    }
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
                if app.wants_frame() {
                    app.frame();
                }
            } else {
                if !GetMessageW(&mut msg, None, 0, 0).as_bool() {
                    break;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}

unsafe extern "system" fn on_foreground_change(_hook: HWINEVENTHOOK, _event: u32, _hwnd: HWND, _object: i32, _child: i32, _thread: u32, _time: u32) {
    if let Some(app) = app_opt() {
        app.ensure_topmost();
    }
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let Some(app) = app_opt() else {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    };
    let scale = app.scale;

    match msg {
        WM_MOUSEMOVE => {
            let (x, y) = dip(lparam, scale);
            app.on_mouse_move(x, y);
        }
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
            let (x, y) = dip(lparam, scale);
            app.on_lbutton_down(x, y, msg == WM_LBUTTONDBLCLK);
        }
        WM_LBUTTONUP => {
            let (x, y) = dip(lparam, scale);
            app.on_lbutton_up(x, y);
        }
        WM_RBUTTONUP => {
            let (x, y) = dip(lparam, scale);
            app.on_rbutton_up(x, y);
        }
        WM_MOUSEWHEEL => {
            app.on_wheel(((wparam.0 >> 16) & 0xFFFF) as i16 as i32);
        }
        WM_MOUSELEAVE => app.on_mouse_leave(),
        WM_CAPTURECHANGED => app.on_capture_lost(),
        // The island never takes focus, except for typing into the reminder field.
        WM_MOUSEACTIVATE => return LRESULT(if app.wants_keyboard() { MA_ACTIVATE } else { MA_NOACTIVATE } as isize),
        WM_CHAR => app.on_char(wparam.0 as u16),
        WM_KILLFOCUS => app.blur_input(false),
        WM_SETCURSOR => {
            if (lparam.0 & 0xFFFF) as u32 == HTCLIENT {
                let cursor = if app.hot_is_clickable() { IDC_HAND } else { IDC_ARROW };
                SetCursor(LoadCursorW(None, cursor).ok());
                return LRESULT(1);
            }
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        WM_TIMER => app.on_timer(wparam.0),
        WM_HOTKEY => {
            if wparam.0 as i32 == crate::settings::HOTKEY_ID {
                app.toggle_hidden();
            }
        }
        WM_APP_MSG => app.on_messages(),
        WM_APP_PICK_END => app.end_picking(wparam.0 == 1),
        WM_TRAY => match (lparam.0 & 0xFFFF) as u32 {
            WM_RBUTTONUP | WM_CONTEXTMENU => app.show_tray_menu(),
            WM_LBUTTONDBLCLK => app.toggle_expanded_from_tray(),
            _ => {}
        },
        WM_INPUT => {
            if keys::is_lock_key(lparam) {
                app.on_lock_key();
            }
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        WM_POWERBROADCAST => {
            if wparam.0 == PBT_APMPOWERSTATUSCHANGE || wparam.0 == PBT_POWERSETTINGCHANGE {
                app.on_power_change();
            }
            return LRESULT(1);
        }
        WM_DEVICECHANGE => {
            for ev in drives::events(wparam, lparam) {
                app.enqueue_alert(ev);
            }
            return LRESULT(1);
        }
        WM_CLIPBOARDUPDATE => app.on_clipboard_update(),
        WM_ENTERSIZEMOVE => app.on_enter_move(),
        WM_MOVE => app.on_window_moving(),
        WM_EXITSIZEMOVE => app.on_window_moved(),
        WM_DPICHANGED | WM_DISPLAYCHANGE => {
            let scale = GetDpiForSystem() as f32 / 96.0;
            let _ = SetWindowPos(hwnd, None, 0, 0, (WIN_W * scale).round() as i32, (WIN_H * scale).round() as i32, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
            app.reset_graphics(scale);
            app.refresh_monitors();
            app.apply_position();
        }
        WM_PAINT => {
            let _ = ValidateRect(Some(hwnd), None);
            app.invalidate();
        }
        WM_DESTROY => {
            PostQuitMessage(0);
        }
        _ => {
            if msg != 0 && msg == TASKBAR_CREATED.load(Ordering::Relaxed) {
                // Explorer restarted: the tray icon is gone.
                tray::add(hwnd);
            }
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
    }
    LRESULT(0)
}
