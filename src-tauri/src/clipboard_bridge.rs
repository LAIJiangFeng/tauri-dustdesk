use std::{
    fs,
    io::Cursor,
    path::PathBuf,
    sync::{Mutex, OnceLock},
    thread,
    time::{Duration, Instant},
};

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use chrono::Local;

use crate::{
    models::{ClipboardData, ClipboardHistoryItem, ClipboardHistoryKind},
    store::AppStore,
};

const MAX_HISTORY_ITEMS: usize = 80;
const MAX_TEXT_CHARS: usize = 20_000;
const MAX_IMAGE_BASE64_CHARS: usize = 32_000_000;
const THUMBNAIL_MAX_EDGE: u32 = 420;

#[cfg(windows)]
static LAST_TARGET_HWND: OnceLock<Mutex<isize>> = OnceLock::new();
static CLIPBOARD_MONITOR_SUPPRESSED_UNTIL: OnceLock<Mutex<Option<Instant>>> = OnceLock::new();

pub fn spawn_text_history_monitor() {
    thread::spawn(|| {
        let mut last_seen = String::new();
        let mut last_seen_image = String::new();
        let mut last_sequence = 0u32;
        loop {
            if let Some(sequence) = system_clipboard_sequence_number() {
                if sequence == last_sequence {
                    thread::sleep(Duration::from_millis(700));
                    continue;
                }
                last_sequence = sequence;
            }

            if is_clipboard_monitor_suppressed() {
                thread::sleep(Duration::from_millis(180));
                continue;
            }

            if let Some(image_png_base64) = system_clipboard_image_png_base64() {
                let fingerprint = image_fingerprint(&image_png_base64);
                if !fingerprint.is_empty() && fingerprint != last_seen_image {
                    last_seen_image = fingerprint;
                    let _ = store_image_history(image_png_base64);
                }
            } else if let Some(text) = system_clipboard_text() {
                if should_store_text(&text) && text != last_seen {
                    last_seen = text.clone();
                    let _ = store_text_history(text);
                }
            }
            thread::sleep(Duration::from_millis(700));
        }
    });
}

pub fn sync_current_clipboard_once() {
    if let Some(text) = system_clipboard_text() {
        if should_store_text(&text) {
            let _ = store_text_history(text);
        }
    }
}

pub fn paste_history_item(id: &str, before_send: impl FnOnce()) -> Result<(), String> {
    let store = AppStore::open().map_err(to_message)?;
    let clipboard = store.load_clipboard();
    let item = clipboard
        .items
        .into_iter()
        .find(|item| item.id == id)
        .ok_or_else(|| "剪贴记录不存在".to_owned())?;

    match item.kind {
        ClipboardHistoryKind::Text => {
            if item.text.trim().is_empty() {
                return Err("文本剪贴记录为空".to_owned());
            }
            suppress_clipboard_monitor_for(Duration::from_secs(3));
            paste_text_to_last_target(&item.text, before_send)
        }
        ClipboardHistoryKind::Image => {
            let image_png_base64 = if !item.image_png_base64.trim().is_empty() {
                item.image_png_base64
            } else if !item.image_path.trim().is_empty() {
                let bytes = fs::read(&item.image_path).map_err(to_message)?;
                BASE64.encode(bytes)
            } else {
                String::new()
            };

            if image_png_base64.trim().is_empty() {
                return Err("图片剪贴记录缺少图片数据".to_owned());
            }
            suppress_clipboard_monitor_for(Duration::from_secs(4));
            paste_image_to_last_target(&image_png_base64, before_send)
        }
    }
}

pub fn image_base64(id: &str) -> Result<String, String> {
    let store = AppStore::open().map_err(to_message)?;
    let clipboard = store.load_clipboard();
    let item = clipboard
        .items
        .into_iter()
        .find(|item| item.id == id)
        .ok_or_else(|| "剪贴记录不存在".to_owned())?;

    if item.kind != ClipboardHistoryKind::Image {
        return Err("剪贴记录不是图片".to_owned());
    }

    if !item.image_png_base64.trim().is_empty() {
        return Ok(item.image_png_base64);
    }

    if item.image_path.trim().is_empty() {
        return Err("图片剪贴记录缺少图片数据".to_owned());
    }

    let bytes = fs::read(&item.image_path).map_err(to_message)?;
    Ok(BASE64.encode(bytes))
}

pub fn normalize_clipboard_image_storage(clipboard: &mut ClipboardData) -> Result<bool, String> {
    let mut changed = false;
    for item in &mut clipboard.items {
        if item.kind != ClipboardHistoryKind::Image {
            continue;
        }

        if !item.image_path.trim().is_empty() && !item.image_thumb_path.trim().is_empty() {
            if !item.image_png_base64.trim().is_empty() {
                item.image_png_base64.clear();
                changed = true;
            }
            continue;
        }

        if item.image_png_base64.trim().is_empty() {
            continue;
        }

        let image_id = if item.id.trim().is_empty() {
            new_id()
        } else {
            item.id.clone()
        };
        let stored = persist_image_files(&image_id, &item.image_png_base64)?;
        item.id = image_id;
        item.image_path = stored.image_path.display().to_string();
        item.image_thumb_path = stored.image_thumb_path.display().to_string();
        item.image_hash = image_fingerprint(&item.image_png_base64);
        item.image_png_base64.clear();
        changed = true;
    }
    Ok(changed)
}

pub fn remember_foreground_window() {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

        let hwnd = GetForegroundWindow() as isize;
        let cell = LAST_TARGET_HWND.get_or_init(|| Mutex::new(0));
        if let Ok(mut value) = cell.lock() {
            *value = hwnd;
        }
    }
}

fn suppress_clipboard_monitor_for(duration: Duration) {
    let until = Instant::now() + duration;
    if let Ok(mut value) = CLIPBOARD_MONITOR_SUPPRESSED_UNTIL
        .get_or_init(|| Mutex::new(None))
        .lock()
    {
        *value = Some(until);
    }
}

fn is_clipboard_monitor_suppressed() -> bool {
    let Ok(mut value) = CLIPBOARD_MONITOR_SUPPRESSED_UNTIL
        .get_or_init(|| Mutex::new(None))
        .lock()
    else {
        return false;
    };

    if value.as_ref().is_some_and(|until| Instant::now() < *until) {
        return true;
    }

    *value = None;
    false
}

fn store_text_history(text: String) -> Result<(), String> {
    let normalized = limit_text(text);
    let store = AppStore::open().map_err(to_message)?;
    let mut clipboard = store.load_clipboard();
    if clipboard
        .items
        .first()
        .is_some_and(|item| item.text == normalized)
    {
        return Ok(());
    }

    if let Some(index) = clipboard
        .items
        .iter()
        .position(|item| item.text == normalized)
    {
        let item = clipboard.items.remove(index);
        clipboard.items.insert(0, item);
        truncate_history(&mut clipboard);
        return store.save_clipboard(&clipboard).map_err(to_message);
    }

    clipboard.items.insert(
        0,
        ClipboardHistoryItem {
            id: new_id(),
            kind: ClipboardHistoryKind::Text,
            text: normalized,
            image_png_base64: String::new(),
            image_path: String::new(),
            image_thumb_path: String::new(),
            image_hash: String::new(),
            created_at: now_local(),
            is_locked: false,
            is_pinned: false,
        },
    );
    truncate_history(&mut clipboard);
    store.save_clipboard(&clipboard).map_err(to_message)
}

fn store_image_history(image_png_base64: String) -> Result<(), String> {
    let normalized = image_png_base64.trim().to_owned();
    if normalized.is_empty() {
        return Ok(());
    }
    if normalized.len() > MAX_IMAGE_BASE64_CHARS {
        return Err("图片剪贴内容过大，已跳过记录".to_owned());
    }

    let store = AppStore::open().map_err(to_message)?;
    let mut clipboard = store.load_clipboard();
    let image_hash = image_fingerprint(&normalized);
    if clipboard.items.first().is_some_and(|item| {
        item.kind == ClipboardHistoryKind::Image
            && (item.image_hash == image_hash || item.image_png_base64 == normalized)
    }) {
        return Ok(());
    }

    if let Some(index) = clipboard.items.iter().position(|item| {
        item.kind == ClipboardHistoryKind::Image
            && (item.image_hash == image_hash || item.image_png_base64 == normalized)
    }) {
        let item = clipboard.items.remove(index);
        clipboard.items.insert(0, item);
        truncate_history(&mut clipboard);
        return store.save_clipboard(&clipboard).map_err(to_message);
    }

    let id = new_id();
    let stored = persist_image_files(&id, &normalized)?;
    clipboard.items.insert(
        0,
        ClipboardHistoryItem {
            id,
            kind: ClipboardHistoryKind::Image,
            text: "[图片剪贴内容]".to_owned(),
            image_png_base64: String::new(),
            image_path: stored.image_path.display().to_string(),
            image_thumb_path: stored.image_thumb_path.display().to_string(),
            image_hash,
            created_at: now_local(),
            is_locked: false,
            is_pinned: false,
        },
    );
    truncate_history(&mut clipboard);
    store.save_clipboard(&clipboard).map_err(to_message)
}

fn truncate_history(clipboard: &mut ClipboardData) {
    if clipboard.items.len() > MAX_HISTORY_ITEMS {
        clipboard.items.truncate(MAX_HISTORY_ITEMS);
    }
}

fn should_store_text(text: &str) -> bool {
    !text.trim().is_empty()
}

fn limit_text(text: String) -> String {
    let mut output = String::new();
    for (index, ch) in text.chars().enumerate() {
        if index >= MAX_TEXT_CHARS {
            break;
        }
        output.push(ch);
    }
    output
}

fn image_fingerprint(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let prefix: String = trimmed.chars().take(96).collect();
    let suffix: String = trimmed
        .chars()
        .rev()
        .take(96)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    format!("{}:{prefix}:{suffix}", trimmed.len())
}

fn persist_image_files(id: &str, image_png_base64: &str) -> Result<StoredImageFiles, String> {
    let store = AppStore::open().map_err(to_message)?;
    let root = store.clipboard_images_root();
    fs::create_dir_all(&root).map_err(to_message)?;

    let safe_id = safe_file_stem(id);
    let image_path = root.join(format!("{safe_id}.png"));
    let image_thumb_path = root.join(format!("{safe_id}.thumb.png"));
    let image_bytes = BASE64
        .decode(image_png_base64.trim())
        .map_err(|error| format!("图片数据无效：{error}"))?;

    fs::write(&image_path, &image_bytes).map_err(to_message)?;
    let thumbnail_bytes =
        png_bytes_to_thumbnail(&image_bytes).unwrap_or_else(|_| image_bytes.clone());
    fs::write(&image_thumb_path, thumbnail_bytes).map_err(to_message)?;

    Ok(StoredImageFiles {
        image_path,
        image_thumb_path,
    })
}

fn png_bytes_to_thumbnail(image_bytes: &[u8]) -> Result<Vec<u8>, String> {
    let image = png_bytes_to_rgba(image_bytes)?;
    let thumbnail = resize_rgba_nearest(&image, THUMBNAIL_MAX_EDGE);
    rgba_to_png_bytes(thumbnail.width, thumbnail.height, &thumbnail.pixels)
}

fn png_bytes_to_rgba(image_bytes: &[u8]) -> Result<RgbaImageData, String> {
    let mut decoder = png::Decoder::new(Cursor::new(image_bytes));
    decoder.set_transformations(
        png::Transformations::normalize_to_color8() | png::Transformations::ALPHA,
    );
    let mut reader = decoder
        .read_info()
        .map_err(|error| format!("无法读取图片：{error}"))?;
    let mut buffer = vec![
        0;
        reader
            .output_buffer_size()
            .ok_or_else(|| "图片尺寸过大".to_owned())?
    ];
    let info = reader
        .next_frame(&mut buffer)
        .map_err(|error| format!("无法解码图片：{error}"))?;
    decoded_png_to_rgba(
        &buffer[..info.buffer_size()],
        info.width,
        info.height,
        info.color_type,
    )
}

fn decoded_png_to_rgba(
    bytes: &[u8],
    width: u32,
    height: u32,
    color_type: png::ColorType,
) -> Result<RgbaImageData, String> {
    let mut pixels = Vec::with_capacity(
        (width as usize)
            .saturating_mul(height as usize)
            .saturating_mul(4),
    );
    match color_type {
        png::ColorType::Rgba => pixels.extend_from_slice(bytes),
        png::ColorType::Rgb => {
            for chunk in bytes.chunks_exact(3) {
                pixels.extend_from_slice(&[chunk[0], chunk[1], chunk[2], 255]);
            }
        }
        png::ColorType::Grayscale => {
            for gray in bytes {
                pixels.extend_from_slice(&[*gray, *gray, *gray, 255]);
            }
        }
        png::ColorType::GrayscaleAlpha => {
            for chunk in bytes.chunks_exact(2) {
                pixels.extend_from_slice(&[chunk[0], chunk[0], chunk[0], chunk[1]]);
            }
        }
        png::ColorType::Indexed => return Err("不支持索引色图片".to_owned()),
    }

    Ok(RgbaImageData {
        width,
        height,
        pixels,
    })
}

fn resize_rgba_nearest(image: &RgbaImageData, max_edge: u32) -> RgbaImageData {
    let largest_edge = image.width.max(image.height);
    if largest_edge <= max_edge || max_edge == 0 {
        return image.clone();
    }

    let width =
        (((image.width as f64) * (max_edge as f64) / (largest_edge as f64)).round() as u32).max(1);
    let height =
        (((image.height as f64) * (max_edge as f64) / (largest_edge as f64)).round() as u32).max(1);
    let mut pixels = vec![0u8; (width as usize) * (height as usize) * 4];

    for y in 0..height {
        let source_y = ((y as u64) * (image.height as u64) / (height as u64)) as u32;
        for x in 0..width {
            let source_x = ((x as u64) * (image.width as u64) / (width as u64)) as u32;
            let source = ((source_y * image.width + source_x) as usize) * 4;
            let target = ((y * width + x) as usize) * 4;
            pixels[target..target + 4].copy_from_slice(&image.pixels[source..source + 4]);
        }
    }

    RgbaImageData {
        width,
        height,
        pixels,
    }
}

fn rgba_to_png_bytes(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    let mut png_bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png_bytes, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|error| format!("无法编码图片：{error}"))?;
        writer
            .write_image_data(rgba)
            .map_err(|error| format!("无法写入图片：{error}"))?;
    }
    Ok(png_bytes)
}

fn safe_file_stem(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

#[derive(Debug)]
struct StoredImageFiles {
    image_path: PathBuf,
    image_thumb_path: PathBuf,
}

#[derive(Clone)]
struct RgbaImageData {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

fn now_local() -> String {
    Local::now().format("%Y-%m-%dT%H:%M:%S%.3f").to_string()
}

fn new_id() -> String {
    let now = Local::now();
    format!("c{:x}{:x}", now.timestamp_millis(), std::process::id())
}

fn to_message(error: impl std::fmt::Display) -> String {
    error.to_string()
}

#[cfg(not(windows))]
fn system_clipboard_text() -> Option<String> {
    None
}

#[cfg(not(windows))]
fn system_clipboard_image_png_base64() -> Option<String> {
    None
}

#[cfg(not(windows))]
fn system_clipboard_sequence_number() -> Option<u32> {
    None
}

#[cfg(not(windows))]
fn paste_text_to_last_target(_text: &str, _before_send: impl FnOnce()) -> Result<(), String> {
    Err("当前平台暂不支持快速粘贴".to_owned())
}

#[cfg(not(windows))]
fn paste_image_to_last_target(
    _image_png_base64: &str,
    _before_send: impl FnOnce(),
) -> Result<(), String> {
    Err("当前平台暂不支持图片快速粘贴".to_owned())
}

#[cfg(windows)]
fn system_clipboard_text() -> Option<String> {
    windows_clipboard::get_text()
}

#[cfg(windows)]
fn system_clipboard_image_png_base64() -> Option<String> {
    windows_clipboard::get_image_png_base64()
}

#[cfg(windows)]
fn system_clipboard_sequence_number() -> Option<u32> {
    Some(windows_clipboard::clipboard_sequence_number())
}

#[cfg(windows)]
fn paste_text_to_last_target(text: &str, before_send: impl FnOnce()) -> Result<(), String> {
    windows_clipboard::paste_text(text, before_send)
}

#[cfg(windows)]
fn paste_image_to_last_target(
    image_png_base64: &str,
    before_send: impl FnOnce(),
) -> Result<(), String> {
    windows_clipboard::paste_image_png_base64(image_png_base64, before_send)
}

#[cfg(windows)]
mod windows_clipboard {
    use std::{
        io::Cursor,
        ptr::null_mut,
        thread,
        time::{Duration, Instant},
    };

    use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};

    use windows_sys::Win32::{
        Foundation::{GlobalFree, HWND},
        System::{
            DataExchange::{
                CloseClipboard, EmptyClipboard, GetClipboardData, GetClipboardSequenceNumber,
                IsClipboardFormatAvailable, OpenClipboard, SetClipboardData,
            },
            Memory::{GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE},
            Threading::{AttachThreadInput, GetCurrentThreadId},
        },
        UI::{
            Input::KeyboardAndMouse::{
                SendInput, SetActiveWindow, SetFocus, INPUT, INPUT_0, INPUT_KEYBOARD,
                KEYBDINPUT, KEYEVENTF_KEYUP, VK_CONTROL, VK_V,
            },
            WindowsAndMessaging::{
                AllowSetForegroundWindow, BringWindowToTop, GetForegroundWindow,
                GetWindowThreadProcessId, IsWindow, SetForegroundWindow, ShowWindow, ASFW_ANY,
                SW_RESTORE,
            },
        },
    };

    use super::LAST_TARGET_HWND;

    const BI_RGB: u32 = 0;
    const CF_DIB: u32 = 8;
    const CF_UNICODETEXT: u32 = 13;
    const FOCUS_WAIT_TIMEOUT: Duration = Duration::from_millis(900);
    const FOCUS_WAIT_STEP: Duration = Duration::from_millis(25);
    const TEXT_PASTE_READY_DELAY: Duration = Duration::from_millis(180);
    const IMAGE_PASTE_READY_DELAY: Duration = Duration::from_millis(360);

    pub fn clipboard_sequence_number() -> u32 {
        unsafe { GetClipboardSequenceNumber() }
    }

    pub fn get_text() -> Option<String> {
        unsafe {
            if IsClipboardFormatAvailable(CF_UNICODETEXT) == 0 || OpenClipboard(null_mut()) == 0 {
                return None;
            }

            let result = read_open_clipboard_text();
            CloseClipboard();
            result
        }
    }

    pub fn get_image_png_base64() -> Option<String> {
        unsafe {
            if IsClipboardFormatAvailable(CF_DIB) == 0 || OpenClipboard(null_mut()) == 0 {
                return None;
            }

            let result = read_open_clipboard_dib().and_then(|dib| dib_to_png_base64(&dib));
            CloseClipboard();
            result
        }
    }

    pub fn paste_text(text: &str, before_send: impl FnOnce()) -> Result<(), String> {
        set_text(text)?;
        before_send();
        if let Some(hwnd) = focus_last_target() {
            wait_for_foreground(hwnd, FOCUS_WAIT_TIMEOUT);
        }
        thread::sleep(TEXT_PASTE_READY_DELAY);
        send_ctrl_v()?;
        Ok(())
    }

    pub fn paste_image_png_base64(
        image_png_base64: &str,
        before_send: impl FnOnce(),
    ) -> Result<(), String> {
        let dib = png_base64_to_dib(image_png_base64)?;
        let ready_delay = image_ready_delay(dib.len());
        set_dib(&dib)?;
        before_send();
        if let Some(hwnd) = focus_last_target() {
            wait_for_foreground(hwnd, FOCUS_WAIT_TIMEOUT);
        }
        thread::sleep(ready_delay);
        send_ctrl_v()?;
        Ok(())
    }

    unsafe fn read_open_clipboard_text() -> Option<String> {
        let handle = unsafe { GetClipboardData(CF_UNICODETEXT) };
        if handle.is_null() {
            return None;
        }

        let ptr = unsafe { GlobalLock(handle) as *const u16 };
        if ptr.is_null() {
            return None;
        }

        let mut len = 0usize;
        while unsafe { *ptr.add(len) } != 0 && len < 1_000_000 {
            len += 1;
        }
        let slice = unsafe { std::slice::from_raw_parts(ptr, len) };
        let text = String::from_utf16_lossy(slice);
        unsafe {
            GlobalUnlock(handle);
        }
        Some(text)
    }

    unsafe fn read_open_clipboard_dib() -> Option<Vec<u8>> {
        let handle = unsafe { GetClipboardData(CF_DIB) };
        if handle.is_null() {
            return None;
        }

        let size = unsafe { GlobalSize(handle) };
        if size == 0 {
            return None;
        }

        let ptr = unsafe { GlobalLock(handle) as *const u8 };
        if ptr.is_null() {
            return None;
        }

        let bytes = unsafe { std::slice::from_raw_parts(ptr, size).to_vec() };
        unsafe {
            GlobalUnlock(handle);
        }
        Some(bytes)
    }

    fn set_text(text: &str) -> Result<(), String> {
        let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        let bytes_len = wide.len() * std::mem::size_of::<u16>();

        unsafe {
            let handle = GlobalAlloc(GMEM_MOVEABLE, bytes_len);
            if handle.is_null() {
                return Err("无法分配剪贴板内存".to_owned());
            }

            let ptr = GlobalLock(handle) as *mut u8;
            if ptr.is_null() {
                GlobalFree(handle);
                return Err("无法写入剪贴板内存".to_owned());
            }

            std::ptr::copy_nonoverlapping(wide.as_ptr() as *const u8, ptr, bytes_len);
            GlobalUnlock(handle);

            if OpenClipboard(null_mut()) == 0 {
                GlobalFree(handle);
                return Err("无法打开系统剪贴板".to_owned());
            }

            EmptyClipboard();
            let set_result = SetClipboardData(CF_UNICODETEXT, handle);
            CloseClipboard();

            if set_result.is_null() {
                GlobalFree(handle);
                return Err("无法设置系统剪贴板".to_owned());
            }
        }

        Ok(())
    }

    fn set_dib(dib: &[u8]) -> Result<(), String> {
        if dib.is_empty() {
            return Err("图片剪贴记录为空".to_owned());
        }

        unsafe {
            let handle = GlobalAlloc(GMEM_MOVEABLE, dib.len());
            if handle.is_null() {
                return Err("无法分配图片剪贴板内存".to_owned());
            }

            let ptr = GlobalLock(handle) as *mut u8;
            if ptr.is_null() {
                GlobalFree(handle);
                return Err("无法写入图片剪贴板内存".to_owned());
            }

            std::ptr::copy_nonoverlapping(dib.as_ptr(), ptr, dib.len());
            GlobalUnlock(handle);

            if OpenClipboard(null_mut()) == 0 {
                GlobalFree(handle);
                return Err("无法打开系统剪贴板".to_owned());
            }

            EmptyClipboard();
            let set_result = SetClipboardData(CF_DIB, handle);
            CloseClipboard();

            if set_result.is_null() {
                GlobalFree(handle);
                return Err("无法设置图片剪贴板".to_owned());
            }
        }

        Ok(())
    }

    fn dib_to_png_base64(dib: &[u8]) -> Option<String> {
        let rgba = dib_to_rgba(dib)?;
        rgba_to_png_base64(rgba.width, rgba.height, &rgba.pixels).ok()
    }

    fn dib_to_rgba(dib: &[u8]) -> Option<RgbaImage> {
        let header_size = read_u32(dib, 0)? as usize;
        if header_size < 40 || header_size > dib.len() {
            return None;
        }

        let width = read_i32(dib, 4)?;
        let height = read_i32(dib, 8)?;
        let planes = read_u16(dib, 12)?;
        let bit_count = read_u16(dib, 14)?;
        let compression = read_u32(dib, 16)?;
        if width <= 0
            || height == 0
            || planes != 1
            || compression != BI_RGB
            || !matches!(bit_count, 24 | 32)
        {
            return None;
        }

        let width = width as usize;
        let height_abs = height.checked_abs()? as usize;
        let bytes_per_pixel = (bit_count / 8) as usize;
        let row_stride = ((width * bit_count as usize + 31) / 32) * 4;
        let pixel_offset = header_size;
        let image_size = row_stride.checked_mul(height_abs)?;
        if pixel_offset.checked_add(image_size)? > dib.len() {
            return None;
        }

        let mut pixels = vec![0u8; width.checked_mul(height_abs)?.checked_mul(4)?];
        let bottom_up = height > 0;
        let mut has_nonzero_alpha = false;

        for y in 0..height_abs {
            let source_y = if bottom_up { height_abs - 1 - y } else { y };
            let row_offset = pixel_offset + source_y * row_stride;
            for x in 0..width {
                let source = row_offset + x * bytes_per_pixel;
                let target = (y * width + x) * 4;
                let b = dib[source];
                let g = dib[source + 1];
                let r = dib[source + 2];
                let a = if bit_count == 32 {
                    dib[source + 3]
                } else {
                    255
                };
                has_nonzero_alpha |= a != 0;
                pixels[target] = r;
                pixels[target + 1] = g;
                pixels[target + 2] = b;
                pixels[target + 3] = a;
            }
        }

        if bit_count == 32 && !has_nonzero_alpha {
            for pixel in pixels.chunks_exact_mut(4) {
                pixel[3] = 255;
            }
        }

        Some(RgbaImage {
            width: width as u32,
            height: height_abs as u32,
            pixels,
        })
    }

    fn rgba_to_png_base64(width: u32, height: u32, rgba: &[u8]) -> Result<String, String> {
        let mut png_bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut png_bytes, width, height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder
                .write_header()
                .map_err(|error| format!("无法编码图片：{error}"))?;
            writer
                .write_image_data(rgba)
                .map_err(|error| format!("无法写入图片：{error}"))?;
        }
        Ok(BASE64.encode(png_bytes))
    }

    fn png_base64_to_dib(image_png_base64: &str) -> Result<Vec<u8>, String> {
        let png_bytes = BASE64
            .decode(image_png_base64.trim())
            .map_err(|error| format!("图片数据无效：{error}"))?;
        let mut decoder = png::Decoder::new(Cursor::new(png_bytes));
        decoder.set_transformations(
            png::Transformations::normalize_to_color8() | png::Transformations::ALPHA,
        );
        let mut reader = decoder
            .read_info()
            .map_err(|error| format!("无法读取图片：{error}"))?;
        let mut buffer = vec![
            0;
            reader
                .output_buffer_size()
                .ok_or_else(|| "图片尺寸过大".to_owned())?
        ];
        let info = reader
            .next_frame(&mut buffer)
            .map_err(|error| format!("无法解码图片：{error}"))?;
        let bytes = &buffer[..info.buffer_size()];
        let rgba = decoded_png_to_rgba(bytes, info.width, info.height, info.color_type)?;
        rgba_to_dib(rgba.width, rgba.height, &rgba.pixels)
    }

    fn decoded_png_to_rgba(
        bytes: &[u8],
        width: u32,
        height: u32,
        color_type: png::ColorType,
    ) -> Result<RgbaImage, String> {
        let mut pixels = Vec::with_capacity(
            (width as usize)
                .saturating_mul(height as usize)
                .saturating_mul(4),
        );
        match color_type {
            png::ColorType::Rgba => pixels.extend_from_slice(bytes),
            png::ColorType::Rgb => {
                for chunk in bytes.chunks_exact(3) {
                    pixels.extend_from_slice(&[chunk[0], chunk[1], chunk[2], 255]);
                }
            }
            png::ColorType::Grayscale => {
                for gray in bytes {
                    pixels.extend_from_slice(&[*gray, *gray, *gray, 255]);
                }
            }
            png::ColorType::GrayscaleAlpha => {
                for chunk in bytes.chunks_exact(2) {
                    pixels.extend_from_slice(&[chunk[0], chunk[0], chunk[0], chunk[1]]);
                }
            }
            png::ColorType::Indexed => return Err("不支持索引色图片".to_owned()),
        }

        Ok(RgbaImage {
            width,
            height,
            pixels,
        })
    }

    fn rgba_to_dib(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
        if width == 0 || height == 0 {
            return Err("图片尺寸为空".to_owned());
        }

        let width_usize = width as usize;
        let height_usize = height as usize;
        let expected_len = width_usize
            .checked_mul(height_usize)
            .and_then(|value| value.checked_mul(4))
            .ok_or_else(|| "图片尺寸过大".to_owned())?;
        if rgba.len() < expected_len {
            return Err("图片像素数据不完整".to_owned());
        }

        let image_size = expected_len;
        let mut dib = vec![0u8; 40 + image_size];
        write_u32(&mut dib, 0, 40);
        write_i32(&mut dib, 4, width as i32);
        write_i32(&mut dib, 8, -(height as i32));
        write_u16(&mut dib, 12, 1);
        write_u16(&mut dib, 14, 32);
        write_u32(&mut dib, 16, BI_RGB);
        write_u32(&mut dib, 20, image_size as u32);

        let mut target = 40;
        for pixel in rgba[..expected_len].chunks_exact(4) {
            dib[target] = pixel[2];
            dib[target + 1] = pixel[1];
            dib[target + 2] = pixel[0];
            dib[target + 3] = pixel[3];
            target += 4;
        }

        Ok(dib)
    }

    fn image_ready_delay(dib_len: usize) -> Duration {
        if dib_len > 16 * 1024 * 1024 {
            Duration::from_millis(620)
        } else if dib_len > 6 * 1024 * 1024 {
            Duration::from_millis(480)
        } else {
            IMAGE_PASTE_READY_DELAY
        }
    }

    fn focus_last_target() -> Option<HWND> {
        let hwnd = LAST_TARGET_HWND
            .get_or_init(|| std::sync::Mutex::new(0))
            .lock()
            .map(|value| *value)
            .unwrap_or_default();

        if hwnd == 0 {
            return None;
        }

        unsafe {
            let hwnd = hwnd as HWND;
            if IsWindow(hwnd) == 0 {
                return None;
            }
            activate_window(hwnd);
            Some(hwnd)
        }
    }

    fn wait_for_foreground(hwnd: HWND, timeout: Duration) {
        let start = Instant::now();
        while start.elapsed() < timeout {
            if unsafe { GetForegroundWindow() } == hwnd {
                return;
            }

            unsafe {
                activate_window(hwnd);
            }
            thread::sleep(FOCUS_WAIT_STEP);
        }
    }

    unsafe fn activate_window(hwnd: HWND) {
        AllowSetForegroundWindow(ASFW_ANY);
        ShowWindow(hwnd, SW_RESTORE);

        let current_thread = GetCurrentThreadId();
        let foreground = GetForegroundWindow();
        let foreground_thread = if !foreground.is_null() {
            GetWindowThreadProcessId(foreground, null_mut())
        } else {
            0
        };
        let target_thread = GetWindowThreadProcessId(hwnd, null_mut());

        if foreground_thread != 0 && foreground_thread != current_thread {
            AttachThreadInput(current_thread, foreground_thread, 1);
        }
        if target_thread != 0 && target_thread != current_thread {
            AttachThreadInput(current_thread, target_thread, 1);
        }

        BringWindowToTop(hwnd);
        SetForegroundWindow(hwnd);
        SetActiveWindow(hwnd);
        SetFocus(hwnd);

        if target_thread != 0 && target_thread != current_thread {
            AttachThreadInput(current_thread, target_thread, 0);
        }
        if foreground_thread != 0 && foreground_thread != current_thread {
            AttachThreadInput(current_thread, foreground_thread, 0);
        }
    }

    fn send_ctrl_v() -> Result<(), String> {
        let inputs = [
            key_input(VK_CONTROL, 0),
            key_input(VK_V, 0),
            key_input(VK_V, KEYEVENTF_KEYUP),
            key_input(VK_CONTROL, KEYEVENTF_KEYUP),
        ];

        let sent = unsafe {
            SendInput(
                inputs.len() as u32,
                inputs.as_ptr(),
                std::mem::size_of::<INPUT>() as i32,
            )
        };

        if sent != inputs.len() as u32 {
            return Err("无法发送粘贴快捷键".to_owned());
        }

        Ok(())
    }

    fn key_input(key: u16, flags: u32) -> INPUT {
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: key,
                    wScan: 0,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }
    }

    struct RgbaImage {
        width: u32,
        height: u32,
        pixels: Vec<u8>,
    }

    fn read_u16(bytes: &[u8], offset: usize) -> Option<u16> {
        let slice = bytes.get(offset..offset + 2)?;
        Some(u16::from_le_bytes([slice[0], slice[1]]))
    }

    fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
        let slice = bytes.get(offset..offset + 4)?;
        Some(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
    }

    fn read_i32(bytes: &[u8], offset: usize) -> Option<i32> {
        let slice = bytes.get(offset..offset + 4)?;
        Some(i32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
    }

    fn write_u16(bytes: &mut [u8], offset: usize, value: u16) {
        bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }

    fn write_u32(bytes: &mut [u8], offset: usize, value: u32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn write_i32(bytes: &mut [u8], offset: usize, value: i32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
}
