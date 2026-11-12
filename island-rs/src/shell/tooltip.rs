//! One native tooltip that follows the hovered widget.

use windows::core::PWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows::Win32::UI::Controls::{
    InitCommonControlsEx, ICC_WIN95_CLASSES, INITCOMMONCONTROLSEX, TOOLTIPS_CLASSW, TTF_SUBCLASS, TTM_ADDTOOLW, TTM_DELTOOLW,
    TTM_SETMAXTIPWIDTH, TTS_ALWAYSTIP, TTS_NOPREFIX, TTTOOLINFOW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, SendMessageW, SetWindowPos, CW_USEDEFAULT, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, WINDOW_STYLE,
    WS_EX_TOPMOST, WS_POPUP,
};

use super::wide;

pub struct Tooltip {
    tip: HWND,
    owner: HWND,
    active: bool,
}

impl Tooltip {
    pub fn new(owner: HWND) -> Option<Tooltip> {
        unsafe {
            let icc = INITCOMMONCONTROLSEX { dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32, dwICC: ICC_WIN95_CLASSES };
            let _ = InitCommonControlsEx(&icc);
            let tip = CreateWindowExW(
                WS_EX_TOPMOST,
                TOOLTIPS_CLASSW,
                None,
                WS_POPUP | WINDOW_STYLE(TTS_ALWAYSTIP | TTS_NOPREFIX),
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                Some(owner),
                None,
                None,
                None,
            )
            .ok()?;
            let _ = SetWindowPos(tip, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            // A width limit makes the control honour line breaks.
            SendMessageW(tip, TTM_SETMAXTIPWIDTH, Some(WPARAM(0)), Some(LPARAM(480)));
            Some(Tooltip { tip, owner, active: false })
        }
    }

    fn info(&self) -> TTTOOLINFOW {
        TTTOOLINFOW { cbSize: std::mem::size_of::<TTTOOLINFOW>() as u32, uFlags: TTF_SUBCLASS, hwnd: self.owner, uId: 1, ..Default::default() }
    }

    /// Shows `text` while the mouse rests inside `rect` (client pixels). None removes the tip.
    pub fn set(&mut self, tool: Option<(RECT, &str)>) {
        unsafe {
            if self.active {
                let info = self.info();
                SendMessageW(self.tip, TTM_DELTOOLW, Some(WPARAM(0)), Some(LPARAM(&info as *const _ as isize)));
                self.active = false;
            }
            if let Some((rect, text)) = tool {
                let mut text = wide(text);
                let mut info = self.info();
                info.rect = rect;
                info.lpszText = PWSTR(text.as_mut_ptr());
                SendMessageW(self.tip, TTM_ADDTOOLW, Some(WPARAM(0)), Some(LPARAM(&info as *const _ as isize)));
                self.active = true;
            }
        }
    }
}
