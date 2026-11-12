//! OLE drag and drop for the file shelf.
//!
//! Dropping in needs a hand-written IDropTarget. Dragging out uses the shell's
//! own data object and drop source, so the target sees a real file drag.

use std::cell::Cell;

use windows::core::{implement, Ref, Result, PCWSTR};
use windows::Win32::Foundation::{HWND, POINTL, S_OK};
use windows::Win32::System::Com::{IDataObject, DVASPECT_CONTENT, FORMATETC, TYMED_HGLOBAL};
use windows::Win32::System::Ole::{
    IDropTarget, IDropTarget_Impl, RegisterDragDrop, ReleaseStgMedium, RevokeDragDrop, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_LINK,
    DROPEFFECT_NONE,
};
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::UI::Shell::{DragQueryFileW, IShellItem, SHCreateItemFromParsingName, SHDoDragDrop, BHID_DataObject, HDROP};

use super::wide;
use crate::window::app;

const CF_HDROP: u16 = 15;

fn hdrop_format() -> FORMATETC {
    FORMATETC { cfFormat: CF_HDROP, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0, lindex: -1, tymed: TYMED_HGLOBAL.0 as u32 }
}

fn has_files(data: Option<&IDataObject>) -> bool {
    match data {
        Some(data) => unsafe { data.QueryGetData(&hdrop_format()) == S_OK },
        None => false,
    }
}

fn read_files(data: &IDataObject) -> Vec<String> {
    let mut files = Vec::new();
    unsafe {
        let Ok(mut medium) = data.GetData(&hdrop_format()) else { return files };
        let hdrop = HDROP(medium.u.hGlobal.0);
        let count = DragQueryFileW(hdrop, u32::MAX, None);
        for i in 0..count {
            let len = DragQueryFileW(hdrop, i, None) as usize;
            let mut buffer = vec![0u16; len + 1];
            let copied = DragQueryFileW(hdrop, i, Some(&mut buffer)) as usize;
            files.push(String::from_utf16_lossy(&buffer[..copied.min(len)]));
        }
        ReleaseStgMedium(&mut medium);
    }
    files
}

#[implement(IDropTarget)]
struct DropTarget {
    accepts: Cell<bool>,
}

impl IDropTarget_Impl for DropTarget_Impl {
    fn DragEnter(&self, data: Ref<IDataObject>, _keys: MODIFIERKEYS_FLAGS, _pt: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        let accepts = has_files(data.as_ref()) && app().on_drag_enter();
        self.accepts.set(accepts);
        unsafe { *effect = if accepts { DROPEFFECT_COPY } else { DROPEFFECT_NONE } };
        Ok(())
    }

    fn DragOver(&self, _keys: MODIFIERKEYS_FLAGS, _pt: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        unsafe { *effect = if self.accepts.get() { DROPEFFECT_COPY } else { DROPEFFECT_NONE } };
        Ok(())
    }

    fn DragLeave(&self) -> Result<()> {
        app().on_drag_leave();
        Ok(())
    }

    fn Drop(&self, data: Ref<IDataObject>, _keys: MODIFIERKEYS_FLAGS, _pt: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        let files = match data.as_ref() {
            Some(data) if self.accepts.get() => read_files(data),
            _ => Vec::new(),
        };
        app().on_drop(files);
        unsafe { *effect = DROPEFFECT_COPY };
        Ok(())
    }
}

pub fn register(hwnd: HWND) {
    let target: IDropTarget = DropTarget { accepts: Cell::new(false) }.into();
    unsafe {
        let _ = RegisterDragDrop(hwnd, &target);
    }
}

pub fn revoke(hwnd: HWND) {
    unsafe {
        let _ = RevokeDragDrop(hwnd);
    }
}

/// Drags the file out to Explorer, a browser, a chat window. Blocks until the drop ends.
pub fn drag_out(hwnd: HWND, path: &str) {
    let path = wide(path);
    unsafe {
        let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(PCWSTR(path.as_ptr()), None) else { return };
        let Ok(data) = item.BindToHandler::<_, IDataObject>(None, &BHID_DataObject) else { return };
        let _ = SHDoDragDrop(Some(hwnd), &data, None, DROPEFFECT_COPY | DROPEFFECT_LINK);
    }
}
