//! Shell icons for the file shelf, as premultiplied BGRA pixels.

use windows::core::PCWSTR;
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
};
use windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES;
use windows::Win32::UI::Shell::{SHGetFileInfoW, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON};
use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, DrawIconEx, GetSystemMetrics, DI_NORMAL, SM_CXICON};

use super::wide;

pub struct IconPixels {
    pub size: u32,
    pub pixels: Vec<u8>,
}

pub fn load(path: &str) -> Option<IconPixels> {
    let path = wide(path);
    unsafe {
        let mut info = SHFILEINFOW::default();
        let result = SHGetFileInfoW(
            PCWSTR(path.as_ptr()),
            FILE_FLAGS_AND_ATTRIBUTES(0),
            Some(&mut info),
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_ICON | SHGFI_LARGEICON,
        );
        if result == 0 || info.hIcon.is_invalid() {
            return None;
        }

        let size = GetSystemMetrics(SM_CXICON).max(16);
        let bitmap_info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size,
                biHeight: -size,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };

        let mut pixels = None;
        let dc = CreateCompatibleDC(None);
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        if let Ok(bitmap) = CreateDIBSection(Some(dc), &bitmap_info, DIB_RGB_COLORS, &mut bits, None, 0) {
            let old = SelectObject(dc, bitmap.into());
            if !bits.is_null() && DrawIconEx(dc, 0, 0, info.hIcon, size, size, 0, None, DI_NORMAL).is_ok() {
                let len = (size * size * 4) as usize;
                let mut data = std::slice::from_raw_parts(bits as *const u8, len).to_vec();
                // Old icons carry no alpha channel: everything that was drawn is opaque.
                if data.chunks_exact(4).all(|p| p[3] == 0) {
                    for p in data.chunks_exact_mut(4) {
                        if p[0] != 0 || p[1] != 0 || p[2] != 0 {
                            p[3] = 255;
                        }
                    }
                }
                pixels = Some(data);
            }
            SelectObject(dc, old);
            let _ = DeleteObject(bitmap.into());
        }
        let _ = DeleteDC(dc);
        let _ = DestroyIcon(info.hIcon);

        pixels.map(|pixels| IconPixels { size: size as u32, pixels })
    }
}
