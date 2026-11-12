//! Dynamic Island for Windows: a small always-on overlay at the top of the screen.
//! Native rewrite of the WPF app. Draws only when something changes and gets its
//! system state from events instead of polling.

#![windows_subsystem = "windows"]
#![allow(non_snake_case)]

mod event;
mod island;
mod net;
mod reminder;
mod render;
mod settings;
mod shell;
mod state;
mod sys;
mod window;

use windows::core::w;
use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
use windows::Win32::System::Ole::OleInitialize;
use windows::Win32::System::Threading::CreateMutexW;

fn main() {
    unsafe {
        // Single instance. The handle stays open until the process ends.
        let mutex = CreateMutexW(None, true, w!("Local\\WindowsDynamicIslandAppMutex_v1"));
        if mutex.is_err() || GetLastError() == ERROR_ALREADY_EXISTS {
            return;
        }
        // OLE (not just COM) for drag and drop.
        let _ = OleInitialize(None);
        window::run();
    }
}
