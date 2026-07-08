use std::path::Path;

#[cfg(windows)]
pub fn icon_data_url(path: &Path) -> Option<String> {
    windows_icon::icon_data_url(path)
}

#[cfg(not(windows))]
pub fn icon_data_url(_path: &Path) -> Option<String> {
    None
}

#[cfg(windows)]
mod windows_icon {
    use std::{ffi::OsStr, io::Cursor, os::windows::ffi::OsStrExt, path::Path, ptr::null_mut};

    use base64::{engine::general_purpose::STANDARD, Engine as _};
    use windows_sys::Win32::{
        Graphics::Gdi::{
            CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC,
            SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
        },
        Storage::FileSystem::{FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL},
        UI::{
            Shell::{
                SHGetFileInfoW, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON, SHGFI_USEFILEATTRIBUTES,
            },
            WindowsAndMessaging::{DestroyIcon, DrawIconEx, DI_NORMAL, HICON},
        },
    };

    const ICON_SIZE: i32 = 48;

    pub fn icon_data_url(path: &Path) -> Option<String> {
        let hicon = shell_icon(path)?;
        let png = unsafe { hicon_to_png(hicon, ICON_SIZE, ICON_SIZE) };
        unsafe {
            DestroyIcon(hicon);
        }
        png.map(|bytes| format!("data:image/png;base64,{}", STANDARD.encode(bytes)))
    }

    fn shell_icon(path: &Path) -> Option<HICON> {
        let wide = wide_path(path);
        let mut info = SHFILEINFOW::default();
        let attributes = if path.is_dir() {
            FILE_ATTRIBUTE_DIRECTORY
        } else {
            FILE_ATTRIBUTE_NORMAL
        };
        let mut flags = SHGFI_ICON | SHGFI_LARGEICON;
        if !path.exists() {
            flags |= SHGFI_USEFILEATTRIBUTES;
        }
        let result = unsafe {
            SHGetFileInfoW(
                wide.as_ptr(),
                attributes,
                &mut info,
                std::mem::size_of::<SHFILEINFOW>() as u32,
                flags,
            )
        };

        if result == 0 || info.hIcon.is_null() {
            None
        } else {
            Some(info.hIcon)
        }
    }

    unsafe fn hicon_to_png(hicon: HICON, width: i32, height: i32) -> Option<Vec<u8>> {
        let screen_dc = unsafe { GetDC(null_mut()) };
        if screen_dc.is_null() {
            return None;
        }

        let memory_dc = unsafe { CreateCompatibleDC(screen_dc) };
        if memory_dc.is_null() {
            unsafe {
                ReleaseDC(null_mut(), screen_dc);
            }
            return None;
        }

        let bitmap_info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                ..BITMAPINFOHEADER::default()
            },
            ..BITMAPINFO::default()
        };
        let mut bits = null_mut();
        let bitmap = unsafe {
            CreateDIBSection(
                screen_dc,
                &bitmap_info,
                DIB_RGB_COLORS,
                &mut bits,
                null_mut(),
                0,
            )
        };

        unsafe {
            ReleaseDC(null_mut(), screen_dc);
        }

        if bitmap.is_null() || bits.is_null() {
            unsafe {
                DeleteDC(memory_dc);
            }
            return None;
        }

        let old_bitmap = unsafe { SelectObject(memory_dc, bitmap) };
        let drawn = unsafe {
            DrawIconEx(
                memory_dc,
                0,
                0,
                hicon,
                width,
                height,
                0,
                null_mut(),
                DI_NORMAL,
            ) != 0
        };

        let pixels_len = (width * height * 4) as usize;
        let rgba = if drawn {
            let pixels = unsafe { std::slice::from_raw_parts(bits.cast::<u8>(), pixels_len) };
            let mut rgba = Vec::with_capacity(pixels_len);
            for pixel in pixels.chunks_exact(4) {
                rgba.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
            }
            Some(rgba)
        } else {
            None
        };

        unsafe {
            SelectObject(memory_dc, old_bitmap);
            DeleteObject(bitmap);
            DeleteDC(memory_dc);
        }

        rgba.and_then(|bytes| encode_png(&bytes, width as u32, height as u32))
    }

    fn encode_png(rgba: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
        let mut output = Cursor::new(Vec::new());
        let mut encoder = png::Encoder::new(&mut output, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        {
            let mut writer = encoder.write_header().ok()?;
            writer.write_image_data(rgba).ok()?;
        }
        Some(output.into_inner())
    }

    fn wide_path(path: &Path) -> Vec<u16> {
        OsStr::new(path)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }
}
