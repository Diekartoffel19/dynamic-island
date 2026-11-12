//! Tray icon and its popup menu.

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{CreateBitmap, CreateDIBSection, DeleteObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS};
use windows::Win32::UI::Shell::{Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIconIndirect, CreatePopupMenu, DestroyMenu, GetCursorPos, PostMessageW, SetForegroundWindow, TrackPopupMenu,
    HICON, ICONINFO, MF_CHECKED, MF_POPUP, MF_SEPARATOR, MF_STRING, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_APP, WM_NULL,
};

use super::wide;

/// Callback message of the tray icon. lParam holds the mouse message.
pub const WM_TRAY: u32 = WM_APP + 2;

pub const CMD_TOGGLE: u32 = 1;
pub const CMD_CENTER: u32 = 2;
pub const CMD_CLICK_THROUGH: u32 = 3;
pub const CMD_DEMO_SUCCESS: u32 = 4;
pub const CMD_DEMO_WARNING: u32 = 5;
pub const CMD_DEMO_AUDIO: u32 = 6;
pub const CMD_DEMO_ERROR: u32 = 7;
pub const CMD_EXIT: u32 = 8;
pub const CMD_SETTINGS: u32 = 9;
pub const CMD_HIDE: u32 = 10;

fn data(hwnd: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW { cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32, hWnd: hwnd, uID: 1, ..Default::default() }
}

pub fn add(hwnd: HWND) {
    let mut nid = data(hwnd);
    nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    nid.uCallbackMessage = WM_TRAY;
    nid.hIcon = create_icon();
    let tip = wide("Windows Dynamic Island");
    nid.szTip[..tip.len()].copy_from_slice(&tip);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_ADD, &nid);
    }
}

pub fn remove(hwnd: HWND) {
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &data(hwnd));
    }
}

/// Shows the menu at the cursor. Returns the chosen command, 0 for none.
pub fn show_menu(hwnd: HWND, click_through: bool, hidden: bool) -> u32 {
    unsafe {
        let (Ok(menu), Ok(demo)) = (CreatePopupMenu(), CreatePopupMenu()) else { return 0 };
        let item = |menu, flags, id: u32, text: &str| {
            let text = wide(text);
            let _ = AppendMenuW(menu, flags, id as usize, PCWSTR(text.as_ptr()));
        };

        item(menu, MF_STRING, CMD_TOGGLE, "Dynamic Island öffnen / erweitern");
        item(menu, MF_STRING, CMD_HIDE, if hidden { "Island einblenden" } else { "Island ausblenden" });
        item(menu, MF_STRING, CMD_CENTER, "Oben zentrieren");
        item(menu, if click_through { MF_STRING | MF_CHECKED } else { MF_STRING }, CMD_CLICK_THROUGH, "Click-Through Modus (Maus durchklicken)");
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());

        item(demo, MF_STRING, CMD_DEMO_SUCCESS, "Erfolg (Build abgeschlossen)");
        item(demo, MF_STRING, CMD_DEMO_WARNING, "Warnung (Hohe Auslastung)");
        item(demo, MF_STRING, CMD_DEMO_AUDIO, "Audio / Musik-Status");
        item(demo, MF_STRING, CMD_DEMO_ERROR, "Fehler-Alarm");
        let label = wide("Test-Statusmeldungen senden");
        let _ = AppendMenuW(menu, MF_POPUP, demo.0 as usize, PCWSTR(label.as_ptr()));

        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        item(menu, MF_STRING, CMD_SETTINGS, "Einstellungen …");
        item(menu, MF_STRING, CMD_EXIT, "Beenden");

        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        // Without this the menu does not close on a click elsewhere.
        let _ = SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON, pt.x, pt.y, None, hwnd, None).0 as u32;
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        cmd
    }
}

/// 32x32 icon: black pill with a blue outline and a purple dot.
pub fn create_icon() -> HICON {
    const N: usize = 32;
    const SS: usize = 4;
    let mut pixels = vec![0u8; N * N * 4];

    let inside = |x: f32, y: f32, cx: f32, cy: f32, rx: f32, ry: f32| -> bool {
        let (dx, dy) = ((x - cx) / rx, (y - cy) / ry);
        dx * dx + dy * dy <= 1.0
    };

    for py in 0..N {
        for px in 0..N {
            let (mut r, mut g, mut b, mut a) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
            for sy in 0..SS {
                for sx in 0..SS {
                    let x = px as f32 + (sx as f32 + 0.5) / SS as f32;
                    let y = py as f32 + (sy as f32 + 0.5) / SS as f32;
                    let colour = if inside(x, y, 11.0, 16.0, 3.0, 3.0) {
                        Some((139.0, 108.0, 255.0))
                    } else if inside(x, y, 16.0, 16.0, 12.75, 8.75) {
                        Some((0.0, 0.0, 0.0))
                    } else if inside(x, y, 16.0, 16.0, 15.25, 11.25) {
                        Some((90.0, 162.0, 255.0))
                    } else {
                        None
                    };
                    if let Some((cr, cg, cb)) = colour {
                        r += cr;
                        g += cg;
                        b += cb;
                        a += 255.0;
                    }
                }
            }
            // Premultiplied: the colour sum is already weighted by coverage.
            let n = (SS * SS) as f32;
            let i = (py * N + px) * 4;
            pixels[i] = (b / n) as u8;
            pixels[i + 1] = (g / n) as u8;
            pixels[i + 2] = (r / n) as u8;
            pixels[i + 3] = (a / n) as u8;
        }
    }

    unsafe {
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: N as i32,
                biHeight: -(N as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let Ok(colour) = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0) else { return HICON::default() };
        if !bits.is_null() {
            std::ptr::copy_nonoverlapping(pixels.as_ptr(), bits as *mut u8, pixels.len());
        }
        let mask = CreateBitmap(N as i32, N as i32, 1, 1, None);
        let icon_info = ICONINFO { fIcon: true.into(), xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: colour };
        let icon = CreateIconIndirect(&icon_info).unwrap_or_default();
        let _ = DeleteObject(colour.into());
        let _ = DeleteObject(mask.into());
        icon
    }
}
