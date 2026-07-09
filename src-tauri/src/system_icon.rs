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
    use std::{
        env,
        ffi::{c_void, OsStr},
        fs,
        io::Cursor,
        os::windows::ffi::OsStrExt,
        path::Path,
        path::PathBuf,
        ptr::null_mut,
    };

    use base64::{engine::general_purpose::STANDARD, Engine as _};
    use windows_sys::core::{GUID, HRESULT, PCWSTR, PWSTR};
    use windows_sys::Win32::{
        Foundation::{HWND, RPC_E_CHANGED_MODE, S_FALSE, S_OK},
        Graphics::Gdi::{
            CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC,
            SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
        },
        Storage::FileSystem::{FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL, WIN32_FIND_DATAW},
        System::Com::{
            CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
            COINIT_APARTMENTTHREADED, STGM_READ,
        },
        UI::{
            Shell::{
                ExtractIconExW, SHGetFileInfoW, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON,
                SHGFI_USEFILEATTRIBUTES,
            },
            WindowsAndMessaging::{DestroyIcon, DrawIconEx, DI_NORMAL, HICON},
        },
    };

    const ICON_SIZE: i32 = 48;
    const CLSID_SHELL_LINK: GUID = GUID::from_u128(0x00021401_0000_0000_c000_000000000046);
    const IID_ISHELL_LINK_W: GUID = GUID::from_u128(0x000214f9_0000_0000_c000_000000000046);
    const IID_IPERSIST_FILE: GUID = GUID::from_u128(0x0000010b_0000_0000_c000_000000000046);
    const SLGP_UNCPRIORITY: u32 = 2;

    struct IconSource {
        path: PathBuf,
        icon_index: Option<i32>,
    }

    pub fn icon_data_url(path: &Path) -> Option<String> {
        let is_shortcut = is_windows_shortcut(path);
        let hicon = shortcut_icon_sources(path)
            .into_iter()
            .find_map(|source| icon_source_icon(&source))
            .or_else(|| {
                if is_shortcut {
                    return None;
                }
                let icon_path =
                    internet_shortcut_icon_path(path).unwrap_or_else(|| path.to_path_buf());
                shell_icon(&icon_path)
            })?;
        let png = unsafe { hicon_to_png(hicon, ICON_SIZE, ICON_SIZE) };
        unsafe {
            DestroyIcon(hicon);
        }
        png.map(|bytes| format!("data:image/png;base64,{}", STANDARD.encode(bytes)))
    }

    fn is_windows_shortcut(path: &Path) -> bool {
        path.extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("lnk"))
    }

    fn icon_source_icon(source: &IconSource) -> Option<HICON> {
        source
            .icon_index
            .and_then(|index| extracted_icon(&source.path, index))
            .or_else(|| shell_icon(&source.path))
    }

    fn shortcut_icon_sources(path: &Path) -> Vec<IconSource> {
        if !path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("lnk"))
        {
            return Vec::new();
        }

        unsafe {
            let init_result = CoInitializeEx(null_mut(), COINIT_APARTMENTTHREADED as u32);
            let initialized = init_result == S_OK || init_result == S_FALSE;
            let sources = if initialized || init_result == RPC_E_CHANGED_MODE {
                read_shortcut_icon_sources(path).unwrap_or_default()
            } else {
                Vec::new()
            };
            if initialized {
                CoUninitialize();
            }
            sources
        }
    }

    unsafe fn read_shortcut_icon_sources(path: &Path) -> Option<Vec<IconSource>> {
        let mut link_ptr = null_mut();
        let create_result = unsafe {
            CoCreateInstance(
                &CLSID_SHELL_LINK,
                null_mut(),
                CLSCTX_INPROC_SERVER,
                &IID_ISHELL_LINK_W,
                &mut link_ptr,
            )
        };
        if create_result < 0 || link_ptr.is_null() {
            return None;
        }

        let link = link_ptr.cast::<IShellLinkW>();
        let Some(persist) = query_persist_file(link) else {
            unsafe {
                release_interface(link.cast());
            }
            return None;
        };
        let link_path = wide_path(path);
        let load_result =
            unsafe { ((*(*persist).vtbl).load)(persist.cast(), link_path.as_ptr(), STGM_READ) };
        if load_result < 0 {
            unsafe {
                release_interface(persist.cast());
                release_interface(link.cast());
            }
            return None;
        }

        let mut sources = Vec::new();
        if let Some(icon_source) = shortcut_icon_location(link) {
            push_icon_source(&mut sources, icon_source);
        }
        if let Some(target_path) = shortcut_target_path(link) {
            push_icon_source(
                &mut sources,
                IconSource {
                    path: target_path,
                    icon_index: None,
                },
            );
        }

        unsafe {
            release_interface(persist.cast());
            release_interface(link.cast());
        }
        Some(sources)
    }

    unsafe fn query_persist_file(link: *mut IShellLinkW) -> Option<*mut IPersistFile> {
        let mut persist_ptr = null_mut();
        let result = unsafe {
            ((*(*link).vtbl).query_interface)(link.cast(), &IID_IPERSIST_FILE, &mut persist_ptr)
        };
        if result < 0 || persist_ptr.is_null() {
            None
        } else {
            Some(persist_ptr.cast())
        }
    }

    unsafe fn release_interface(interface: *mut c_void) {
        if interface.is_null() {
            return;
        }
        unsafe {
            let vtbl = *(interface.cast::<*const IUnknownVtbl>());
            ((*vtbl).release)(interface);
        }
    }

    fn shortcut_icon_location(link: *mut IShellLinkW) -> Option<IconSource> {
        let mut icon_index = 0i32;
        let mut buffer = vec![0u16; 32768];
        let result = unsafe {
            ((*(*link).vtbl).get_icon_location)(
                link.cast(),
                buffer.as_mut_ptr(),
                buffer.len() as i32,
                &mut icon_index,
            )
        };
        if result < 0 {
            return None;
        }
        let text = string_from_wide_buffer(&buffer)?;
        Some(IconSource {
            path: expand_environment_path(text.trim().trim_matches('"')),
            icon_index: Some(icon_index),
        })
    }

    fn shortcut_target_path(link: *mut IShellLinkW) -> Option<PathBuf> {
        let mut buffer = vec![0u16; 32768];
        let result = unsafe {
            ((*(*link).vtbl).get_path)(
                link.cast(),
                buffer.as_mut_ptr(),
                buffer.len() as i32,
                null_mut(),
                SLGP_UNCPRIORITY,
            )
        };
        if result < 0 {
            return None;
        }
        let text = string_from_wide_buffer(&buffer)?;
        Some(PathBuf::from(text.trim().trim_matches('"')))
    }

    fn push_icon_source(sources: &mut Vec<IconSource>, source: IconSource) {
        if source.path.as_os_str().is_empty() || !source.path.exists() {
            return;
        }
        if sources.iter().any(|existing| {
            existing.path == source.path && existing.icon_index == source.icon_index
        }) {
            return;
        }
        sources.push(source);
    }

    fn string_from_wide_buffer(buffer: &[u16]) -> Option<String> {
        let end = buffer
            .iter()
            .position(|value| *value == 0)
            .unwrap_or(buffer.len());
        if end == 0 {
            return None;
        }
        Some(String::from_utf16_lossy(&buffer[..end]))
    }

    fn internet_shortcut_icon_path(path: &Path) -> Option<PathBuf> {
        if !path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("url"))
        {
            return None;
        }

        let content = fs::read_to_string(path).ok()?;
        for line in content.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            if !key.trim().eq_ignore_ascii_case("IconFile") {
                continue;
            }
            let icon_path = expand_environment_path(value.trim().trim_matches('"'));
            if icon_path.exists() {
                return Some(icon_path);
            }
        }

        if content.lines().any(|line| {
            line.trim_start()
                .to_ascii_lowercase()
                .starts_with("url=steam://")
        }) {
            let steam = steam_executable_path();
            if steam.exists() {
                return Some(steam);
            }
        }

        None
    }

    fn expand_environment_path(value: &str) -> PathBuf {
        if !value.starts_with('%') {
            return PathBuf::from(value);
        }
        let Some(end) = value[1..].find('%') else {
            return PathBuf::from(value);
        };
        let name = &value[1..=end];
        let Ok(prefix) = env::var(name) else {
            return PathBuf::from(value);
        };
        PathBuf::from(format!("{prefix}{}", &value[end + 2..]))
    }

    fn steam_executable_path() -> PathBuf {
        env::var_os("ProgramFiles(x86)")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Program Files (x86)"))
            .join("Steam")
            .join("steam.exe")
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

    fn extracted_icon(path: &Path, icon_index: i32) -> Option<HICON> {
        let wide = wide_path(path);
        let mut icon = null_mut();
        let count = unsafe { ExtractIconExW(wide.as_ptr(), icon_index, &mut icon, null_mut(), 1) };
        if count == 0 || icon.is_null() {
            None
        } else {
            Some(icon)
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
            has_visible_icon_pixels(&rgba).then_some(rgba)
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

    fn has_visible_icon_pixels(rgba: &[u8]) -> bool {
        let mut visible_pixels = 0usize;
        let mut varied_pixels = 0usize;
        let mut first_visible_color: Option<[u8; 3]> = None;

        for pixel in rgba.chunks_exact(4) {
            let alpha = pixel[3];
            if alpha < 12 {
                continue;
            }

            visible_pixels += 1;
            let color = [pixel[0], pixel[1], pixel[2]];
            match first_visible_color {
                Some(first) => {
                    let delta = (i16::from(first[0]) - i16::from(color[0])).abs()
                        + (i16::from(first[1]) - i16::from(color[1])).abs()
                        + (i16::from(first[2]) - i16::from(color[2])).abs();
                    if delta > 18 {
                        varied_pixels += 1;
                    }
                }
                None => first_visible_color = Some(color),
            }
        }

        visible_pixels >= 12 && (visible_pixels >= 48 || varied_pixels >= 4)
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

    #[repr(C)]
    struct IUnknownVtbl {
        query_interface: unsafe extern "system" fn(
            this: *mut c_void,
            riid: *const GUID,
            ppv: *mut *mut c_void,
        ) -> HRESULT,
        add_ref: unsafe extern "system" fn(this: *mut c_void) -> u32,
        release: unsafe extern "system" fn(this: *mut c_void) -> u32,
    }

    #[repr(C)]
    struct IPersistFile {
        vtbl: *const IPersistFileVtbl,
    }

    #[repr(C)]
    struct IPersistFileVtbl {
        query_interface: unsafe extern "system" fn(
            this: *mut c_void,
            riid: *const GUID,
            ppv: *mut *mut c_void,
        ) -> HRESULT,
        add_ref: unsafe extern "system" fn(this: *mut c_void) -> u32,
        release: unsafe extern "system" fn(this: *mut c_void) -> u32,
        get_class_id: unsafe extern "system" fn(this: *mut c_void, class_id: *mut GUID) -> HRESULT,
        is_dirty: unsafe extern "system" fn(this: *mut c_void) -> HRESULT,
        load: unsafe extern "system" fn(this: *mut c_void, file_name: PCWSTR, mode: u32) -> HRESULT,
        save: unsafe extern "system" fn(
            this: *mut c_void,
            file_name: PCWSTR,
            remember: i32,
        ) -> HRESULT,
        save_completed: unsafe extern "system" fn(this: *mut c_void, file_name: PCWSTR) -> HRESULT,
        get_cur_file:
            unsafe extern "system" fn(this: *mut c_void, file_name: *mut PWSTR) -> HRESULT,
    }

    #[repr(C)]
    struct IShellLinkW {
        vtbl: *const IShellLinkWVtbl,
    }

    #[repr(C)]
    struct IShellLinkWVtbl {
        query_interface: unsafe extern "system" fn(
            this: *mut c_void,
            riid: *const GUID,
            ppv: *mut *mut c_void,
        ) -> HRESULT,
        add_ref: unsafe extern "system" fn(this: *mut c_void) -> u32,
        release: unsafe extern "system" fn(this: *mut c_void) -> u32,
        get_path: unsafe extern "system" fn(
            this: *mut c_void,
            file: PWSTR,
            cch: i32,
            find_data: *mut WIN32_FIND_DATAW,
            flags: u32,
        ) -> HRESULT,
        get_id_list:
            unsafe extern "system" fn(this: *mut c_void, pidl: *mut *mut c_void) -> HRESULT,
        set_id_list: unsafe extern "system" fn(this: *mut c_void, pidl: *const c_void) -> HRESULT,
        get_description:
            unsafe extern "system" fn(this: *mut c_void, name: PWSTR, cch: i32) -> HRESULT,
        set_description: unsafe extern "system" fn(this: *mut c_void, name: PCWSTR) -> HRESULT,
        get_working_directory:
            unsafe extern "system" fn(this: *mut c_void, dir: PWSTR, cch: i32) -> HRESULT,
        set_working_directory: unsafe extern "system" fn(this: *mut c_void, dir: PCWSTR) -> HRESULT,
        get_arguments:
            unsafe extern "system" fn(this: *mut c_void, args: PWSTR, cch: i32) -> HRESULT,
        set_arguments: unsafe extern "system" fn(this: *mut c_void, args: PCWSTR) -> HRESULT,
        get_hotkey: unsafe extern "system" fn(this: *mut c_void, hotkey: *mut u16) -> HRESULT,
        set_hotkey: unsafe extern "system" fn(this: *mut c_void, hotkey: u16) -> HRESULT,
        get_show_cmd: unsafe extern "system" fn(this: *mut c_void, show_cmd: *mut i32) -> HRESULT,
        set_show_cmd: unsafe extern "system" fn(this: *mut c_void, show_cmd: i32) -> HRESULT,
        get_icon_location: unsafe extern "system" fn(
            this: *mut c_void,
            icon_path: PWSTR,
            cch: i32,
            icon_index: *mut i32,
        ) -> HRESULT,
        set_icon_location: unsafe extern "system" fn(
            this: *mut c_void,
            icon_path: PCWSTR,
            icon_index: i32,
        ) -> HRESULT,
        set_relative_path:
            unsafe extern "system" fn(this: *mut c_void, path: PCWSTR, reserved: u32) -> HRESULT,
        resolve: unsafe extern "system" fn(this: *mut c_void, hwnd: HWND, flags: u32) -> HRESULT,
        set_path: unsafe extern "system" fn(this: *mut c_void, file: PCWSTR) -> HRESULT,
    }
}
