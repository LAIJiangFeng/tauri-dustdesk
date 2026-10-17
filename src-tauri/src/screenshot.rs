//! Region screenshot: per-monitor GDI capture, window snapping rects, overlay sessions,
//! pinned images, OCR and the `dustshot` image protocol.
//!
//! All coordinates handled here are physical pixels in virtual-desktop space.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Condvar, Mutex, MutexGuard, OnceLock,
    },
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use tauri::{
    http::{header, Method, Request as HttpRequest, Response as HttpResponse, StatusCode},
    ipc::{InvokeBody, Request as InvokeRequest},
    utils::config::Color,
    AppHandle, Emitter, EventTarget, Manager, PhysicalPosition, PhysicalSize, Position, Size,
    UriSchemeResponder, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
};
use tauri_plugin_dialog::DialogExt;

use crate::{
    clipboard_bridge, configured_settings, to_message, with_lazy_window_operation,
    with_webview_window_creation,
};

pub const URI_SCHEME: &str = "dustshot";
const OVERLAY_LABEL_PREFIX: &str = "screenshot-overlay-";
const PIN_LABEL_PREFIX: &str = "screenshot-pin-";
const START_EVENT: &str = "dustdesk://screenshot-start";
const END_EVENT: &str = "dustdesk://screenshot-end";
const RECT_HEADER: &str = "x-dustshot-rect";
const HIDEABLE_WINDOW_LABELS: [&str; 3] = ["main", "clipboard", "search"];
const HIDE_WINDOW_SETTLE_DELAY: Duration = Duration::from_millis(180);
const OFFSCREEN_POSITION: f64 = -32_000.0;
/// How long the start log waits for the overlays to report their first painted capture.
const OVERLAY_READY_TIMEOUT: Duration = Duration::from_millis(1_500);
/// Time given to overlay pages to present a cleared frame before they are hidden.
const OVERLAY_CLEAR_DELAY: Duration = Duration::from_millis(60);
/// Overlays are created this long after startup, when the app is idle.
const OVERLAY_PREWARM_DELAY: Duration = Duration::from_secs(3);
const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// True from the moment a capture starts until its session has fully ended, so repeated
/// shortcut presses never start a second capture.
static SESSION_BUSY: AtomicBool = AtomicBool::new(false);
static SESSION: OnceLock<Mutex<Option<CaptureSession>>> = OnceLock::new();
static RESTORE_WINDOW_LABELS: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
static NEXT_SESSION_ID: AtomicU64 = AtomicU64::new(1);
static PINS: OnceLock<Mutex<BTreeMap<u64, PinImage>>> = OnceLock::new();
static NEXT_PIN_ID: AtomicU64 = AtomicU64::new(1);
static OVERLAY_READY: OnceLock<(Mutex<OverlayReadyState>, Condvar)> = OnceLock::new();
/// Time spent building the current session's first `dustshot` image response, for the start log.
static LAST_SERVE_MICROS: AtomicU64 = AtomicU64::new(0);

/// Stage timings of one capture start, printed as a single `[screenshot]` line.
struct StartTimer {
    triggered: Instant,
    last: Instant,
    stages: Vec<(&'static str, Duration)>,
}

impl StartTimer {
    fn new(triggered: Instant) -> Self {
        Self {
            triggered,
            last: triggered,
            stages: Vec::new(),
        }
    }

    fn mark(&mut self, stage: &'static str) {
        let now = Instant::now();
        self.stages.push((stage, now - self.last));
        self.last = now;
    }

    fn report(&self, session_id: &str) {
        let stages = self
            .stages
            .iter()
            .map(|(stage, elapsed)| format!("{stage} {}ms", elapsed.as_millis()))
            .collect::<Vec<_>>()
            .join(", ");
        eprintln!(
            "[screenshot] session {session_id}: {stages}; image response {}ms; total {}ms",
            LAST_SERVE_MICROS.load(Ordering::SeqCst) / 1_000,
            self.triggered.elapsed().as_millis()
        );
    }
}

/// Overlays of the current session that still have to report their first painted frame.
#[derive(Default)]
struct OverlayReadyState {
    session_id: Option<String>,
    pending: BTreeSet<usize>,
}

struct CaptureSession {
    id: String,
    monitors: Vec<MonitorCapture>,
}

struct MonitorCapture {
    info: MonitorInfo,
    bmp: Arc<Vec<u8>>,
    windows: Vec<WindowRect>,
}

struct PinImage {
    png: Arc<Vec<u8>>,
    width: u32,
    height: u32,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct MonitorInfo {
    index: usize,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    scale_factor: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct WindowRect {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    title: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScreenshotSession {
    session_id: String,
    monitor: MonitorInfo,
    image_url: String,
    windows: Vec<WindowRect>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScreenshotPin {
    pin_id: String,
    image_url: String,
    width: u32,
    height: u32,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct OcrResult {
    text: String,
    lines: Vec<OcrLine>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OcrLine {
    text: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

#[derive(Debug, Clone, Copy, Serialize)]
struct StartPayload<'a> {
    session_id: &'a str,
    monitor_index: usize,
}

/// Frontend may pass the pin id either as the string returned by `screenshot_pin` or as a number
/// parsed from the window URL.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum PinIdArg {
    Number(u64),
    Text(String),
}

impl PinIdArg {
    fn value(&self) -> Result<u64, String> {
        match self {
            Self::Number(value) => Ok(*value),
            Self::Text(value) => value
                .trim()
                .parse()
                .map_err(|_| format!("钉图编号无效：{value}")),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct PhysicalRect {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

fn session_slot() -> MutexGuard<'static, Option<CaptureSession>> {
    SESSION
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn overlay_ready() -> &'static (Mutex<OverlayReadyState>, Condvar) {
    OVERLAY_READY.get_or_init(|| (Mutex::new(OverlayReadyState::default()), Condvar::new()))
}

fn overlay_ready_state() -> MutexGuard<'static, OverlayReadyState> {
    overlay_ready()
        .0
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn begin_overlay_ready(session_id: &str, indices: impl IntoIterator<Item = usize>) {
    let mut state = overlay_ready_state();
    state.session_id = Some(session_id.to_owned());
    state.pending = indices.into_iter().collect();
}

fn clear_overlay_ready() {
    let mut state = overlay_ready_state();
    state.session_id = None;
    state.pending.clear();
    overlay_ready().1.notify_all();
}

/// Blocks until every overlay of `session_id` reported ready, the session ended, or the timeout
/// elapsed. Returns whether every overlay reported ready.
fn wait_overlays_ready(session_id: &str, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    let mut state = overlay_ready_state();
    while state.session_id.as_deref() == Some(session_id) && !state.pending.is_empty() {
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            break;
        };
        state = overlay_ready()
            .1
            .wait_timeout(state, remaining)
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .0;
    }
    state.pending.is_empty()
}

fn is_current_session(session_id: &str) -> bool {
    session_slot()
        .as_ref()
        .is_some_and(|session| session.id == session_id)
}

fn restore_labels_slot() -> MutexGuard<'static, Vec<String>> {
    RESTORE_WINDOW_LABELS
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn pins_slot() -> MutexGuard<'static, BTreeMap<u64, PinImage>> {
    PINS.get_or_init(|| Mutex::new(BTreeMap::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

async fn run_blocking<T: Send + 'static>(
    operation: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(operation)
        .await
        .map_err(to_message)?
}

fn overlay_label(index: usize) -> String {
    format!("{OVERLAY_LABEL_PREFIX}{index}")
}

fn pin_label(pin_id: u64) -> String {
    format!("{PIN_LABEL_PREFIX}{pin_id}")
}

fn protocol_url(path: &str) -> String {
    #[cfg(windows)]
    {
        format!("http://{URI_SCHEME}.localhost/{path}")
    }
    #[cfg(not(windows))]
    {
        format!("{URI_SCHEME}://localhost/{path}")
    }
}

fn overlay_windows(app: &AppHandle) -> Vec<WebviewWindow> {
    app.webview_windows()
        .into_iter()
        .filter(|(label, _)| label.starts_with(OVERLAY_LABEL_PREFIX))
        .map(|(_, window)| window)
        .collect()
}

fn place_window_physical(window: &WebviewWindow, rect: PhysicalRect) -> Result<(), String> {
    let position = Position::Physical(PhysicalPosition::new(rect.x, rect.y));
    let size = Size::Physical(PhysicalSize::new(rect.width.max(1), rect.height.max(1)));
    // Moving across monitors with different scale factors triggers a DPI change that rescales
    // the window, so the size is applied after the move and the move is repeated afterwards.
    window.set_position(position).map_err(to_message)?;
    window.set_size(size).map_err(to_message)?;
    window.set_position(position).map_err(to_message)?;
    Ok(())
}

fn ensure_window_physical(window: &WebviewWindow, rect: PhysicalRect) {
    let position_ok = window
        .outer_position()
        .is_ok_and(|position| position.x == rect.x && position.y == rect.y);
    let size_ok = window
        .inner_size()
        .is_ok_and(|size| size.width == rect.width && size.height == rect.height);
    if !position_ok || !size_ok {
        let _ = place_window_physical(window, rect);
    }
}

// ---------------------------------------------------------------------------------------------
// Session lifecycle
// ---------------------------------------------------------------------------------------------

/// Shortcut / tray entry point: never blocks the caller.
pub fn start_screenshot_async(app: AppHandle, delay: Duration) {
    let triggered = Instant::now();
    tauri::async_runtime::spawn_blocking(move || {
        if !delay.is_zero() {
            std::thread::sleep(delay);
        }
        if let Err(error) = start_session_blocking(&app, triggered) {
            eprintln!("failed to start screenshot: {error}");
        }
    });
}

#[tauri::command]
pub async fn start_screenshot(app: AppHandle) -> Result<(), String> {
    let triggered = Instant::now();
    run_blocking(move || start_session_blocking(&app, triggered)).await
}

fn start_session_blocking(app: &AppHandle, triggered: Instant) -> Result<(), String> {
    if SESSION_BUSY.swap(true, Ordering::SeqCst) {
        return Ok(());
    }

    let result = start_session_inner(app, StartTimer::new(triggered));
    if result.is_err() {
        finish_session(app);
    }
    result
}

fn start_session_inner(app: &AppHandle, mut timer: StartTimer) -> Result<(), String> {
    timer.mark("dispatch");
    let settings = configured_settings();
    let mut needs_settle = false;
    with_lazy_window_operation(|| {
        // Overlays are hidden when a session ends; hide again defensively so a capture can
        // never contain the previous overlay.
        for window in overlay_windows(app) {
            if window.is_visible().unwrap_or(false) {
                let _ = window.hide();
                needs_settle = true;
            }
        }
        if settings.screenshot_hide_window {
            let mut restore = restore_labels_slot();
            for label in HIDEABLE_WINDOW_LABELS {
                let Some(window) = app.get_webview_window(label) else {
                    continue;
                };
                if window.is_visible().unwrap_or(false) && window.hide().is_ok() {
                    restore.push(label.to_owned());
                    needs_settle = true;
                }
            }
        }
        Ok(())
    })?;
    if needs_settle {
        // Give DWM time to repaint the area under the hidden windows.
        std::thread::sleep(HIDE_WINDOW_SETTLE_DELAY);
    }
    timer.mark("hide");

    let monitors = app.available_monitors().map_err(to_message)?;
    if monitors.is_empty() {
        return Err("没有检测到可截图的显示器".to_owned());
    }
    let infos = monitors
        .iter()
        .enumerate()
        .map(|(index, monitor)| MonitorInfo {
            index,
            x: monitor.position().x,
            y: monitor.position().y,
            width: monitor.size().width,
            height: monitor.size().height,
            scale_factor: monitor.scale_factor(),
        })
        .collect::<Vec<_>>();

    let mut bitmaps = Vec::with_capacity(infos.len());
    for info in &infos {
        bitmaps.push(platform::capture_bmp(
            info.x,
            info.y,
            info.width,
            info.height,
        )?);
    }
    timer.mark("capture");

    let excluded = overlay_windows(app)
        .iter()
        .filter_map(window_handle_value)
        .collect::<Vec<_>>();
    let raw_windows = platform::enumerate_windows(&excluded);
    timer.mark("windows");

    let session_id = NEXT_SESSION_ID.fetch_add(1, Ordering::SeqCst).to_string();
    let captures = infos
        .iter()
        .zip(bitmaps)
        .map(|(info, bmp)| MonitorCapture {
            info: *info,
            bmp: Arc::new(bmp),
            windows: windows_for_monitor(&raw_windows, info),
        })
        .collect::<Vec<_>>();
    // Registered before the session becomes readable, so an overlay page that picks the session
    // up on mount can never report ready before it is waited for.
    begin_overlay_ready(&session_id, infos.iter().map(|info| info.index));
    LAST_SERVE_MICROS.store(0, Ordering::SeqCst);
    *session_slot() = Some(CaptureSession {
        id: session_id.clone(),
        monitors: captures,
    });

    with_lazy_window_operation(|| show_overlays(app, &session_id, &infos))?;
    timer.mark("show");
    // The overlays are already on screen; ready only marks the first painted capture for the log.
    // Waiting happens outside the window lock so cancelling or other windows never block on it.
    let ready = wait_overlays_ready(&session_id, OVERLAY_READY_TIMEOUT);
    if is_current_session(&session_id) {
        timer.mark(if ready { "painted" } else { "paint-timeout" });
        timer.report(&session_id);
    }
    Ok(())
}

/// Shows every overlay directly on its monitor and tells it which session to load.
///
/// Overlays are transparent and draw nothing until their page has decoded and painted the
/// capture (base image and dim layer in the same frame), so showing them right away never exposes
/// a dimmed live desktop. Showing them in place instead of staging them off-screen and moving them
/// after their first frame saves a round of window moves, and Esc works from the first moment.
fn show_overlays(app: &AppHandle, session_id: &str, infos: &[MonitorInfo]) -> Result<(), String> {
    let mut shown = Vec::with_capacity(infos.len());
    for info in infos {
        let label = overlay_label(info.index);
        let window = match app.get_webview_window(&label) {
            Some(window) => window,
            None => create_overlay_window(app, &label, info.index)?,
        };
        let rect = monitor_rect(info);
        place_window_physical(&window, rect)?;
        window.show().map_err(to_message)?;
        let _ = window.set_always_on_top(true);
        ensure_window_physical(&window, rect);
        let _ = app.emit_to(
            EventTarget::webview_window(label.as_str()),
            START_EVENT,
            StartPayload {
                session_id,
                monitor_index: info.index,
            },
        );
        shown.push(window);
    }

    // Overlays left over from a monitor that has since been disconnected stay hidden.
    for window in overlay_windows(app) {
        let stale = window
            .label()
            .strip_prefix(OVERLAY_LABEL_PREFIX)
            .and_then(|index| index.parse::<usize>().ok())
            .is_none_or(|index| index >= infos.len());
        if stale {
            let _ = window.hide();
        }
    }

    // Focused right away so Esc works even before the capture is painted.
    let cursor = platform::cursor_position();
    let focus_index = infos
        .iter()
        .position(|info| {
            cursor.is_some_and(|(x, y)| {
                x >= info.x
                    && y >= info.y
                    && x < info.x + info.width as i32
                    && y < info.y + info.height as i32
            })
        })
        .unwrap_or(0);
    if let Some(window) = shown.get(focus_index) {
        let _ = window.set_focus();
    }
    Ok(())
}

fn monitor_rect(info: &MonitorInfo) -> PhysicalRect {
    PhysicalRect {
        x: info.x,
        y: info.y,
        width: info.width,
        height: info.height,
    }
}

/// Creates an overlay window hidden; callers place and show it.
fn create_overlay_window(
    app: &AppHandle,
    label: &str,
    index: usize,
) -> Result<WebviewWindow, String> {
    with_webview_window_creation(|| {
        WebviewWindowBuilder::new(
            app,
            label,
            WebviewUrl::App(format!("index.html#/screenshot-overlay?monitor={index}").into()),
        )
        .title("DustDesk Screenshot")
        .inner_size(800.0, 600.0)
        .position(OFFSCREEN_POSITION, OFFSCREEN_POSITION)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .decorations(false)
        .shadow(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .focused(false)
        // Transparent instead of an opaque placeholder: until the page painted the capture the
        // live desktop shows through unchanged rather than black.
        .transparent(true)
        .background_color(Color(0, 0, 0, 0))
        .visible(false)
        .build()
        .map_err(to_message)
    })
}

/// Creates the overlay windows of all current monitors ahead of the first capture, so the first
/// shortcut press does not wait for a WebView to be created and its page to load.
pub fn prewarm_overlays_async(app: AppHandle) {
    tauri::async_runtime::spawn_blocking(move || {
        std::thread::sleep(OVERLAY_PREWARM_DELAY);
        let monitor_count = app.available_monitors().map(|monitors| monitors.len());
        for index in 0..monitor_count.unwrap_or(0) {
            if SESSION_BUSY.load(Ordering::SeqCst) {
                break;
            }
            let label = overlay_label(index);
            let result = with_lazy_window_operation(|| {
                if app.get_webview_window(&label).is_none() {
                    create_overlay_window(&app, &label, index)?;
                }
                Ok(())
            });
            if let Err(error) = result {
                eprintln!("[screenshot] failed to prewarm {label}: {error}");
                break;
            }
        }
    });
}

/// Ends the current session: overlays are told to clear and hidden, capture memory is released
/// and windows hidden for the capture are restored. Safe to call when no session is active.
fn finish_session(app: &AppHandle) {
    clear_overlay_ready();
    let overlays = overlay_windows(app);
    for window in &overlays {
        let _ = app.emit_to(EventTarget::webview_window(window.label()), END_EVENT, ());
    }
    let released = session_slot().take();
    drop(released);
    // A hidden WebView can present its last frame again when it is shown, so the pages get a few
    // frames to present their cleared, fully transparent state before they are hidden; otherwise
    // the next capture could briefly show this one.
    if overlays
        .iter()
        .any(|window| window.is_visible().unwrap_or(false))
    {
        std::thread::sleep(OVERLAY_CLEAR_DELAY);
    }
    let _ = with_lazy_window_operation(|| {
        for window in &overlays {
            let _ = window.hide();
        }
        Ok(())
    });

    let labels = std::mem::take(&mut *restore_labels_slot());
    if !labels.is_empty() {
        let _ = with_lazy_window_operation(|| {
            for label in labels {
                if let Some(window) = app.get_webview_window(&label) {
                    let _ = window.show();
                }
            }
            Ok(())
        });
    }

    SESSION_BUSY.store(false, Ordering::SeqCst);
}

fn windows_for_monitor(raw_windows: &[platform::RawWindow], info: &MonitorInfo) -> Vec<WindowRect> {
    let monitor_right = info.x.saturating_add(info.width as i32);
    let monitor_bottom = info.y.saturating_add(info.height as i32);
    raw_windows
        .iter()
        .filter_map(|window| {
            let left = window.left.max(info.x);
            let top = window.top.max(info.y);
            let right = window.right.min(monitor_right);
            let bottom = window.bottom.min(monitor_bottom);
            (right > left && bottom > top).then(|| WindowRect {
                x: left - info.x,
                y: top - info.y,
                width: (right - left) as u32,
                height: (bottom - top) as u32,
                title: window.title.clone(),
            })
        })
        .collect()
}

fn window_handle_value(window: &WebviewWindow) -> Option<isize> {
    #[cfg(windows)]
    {
        window.hwnd().ok().map(|hwnd| hwnd.0 as isize)
    }
    #[cfg(not(windows))]
    {
        let _ = window;
        None
    }
}

#[tauri::command]
pub async fn get_screenshot_session(
    monitor_index: usize,
) -> Result<Option<ScreenshotSession>, String> {
    let session = session_slot();
    let Some(session) = session.as_ref() else {
        return Ok(None);
    };
    Ok(session
        .monitors
        .iter()
        .find(|capture| capture.info.index == monitor_index)
        .map(|capture| ScreenshotSession {
            session_id: session.id.clone(),
            monitor: capture.info,
            image_url: protocol_url(&format!(
                "session/{}/{}.bmp",
                session.id, capture.info.index
            )),
            windows: capture.windows.clone(),
        }))
}

/// Called by an overlay page once the session's capture is decoded and its first frame painted.
/// Reports for a session that already ended or was replaced are ignored.
#[tauri::command]
pub async fn screenshot_overlay_ready(
    session_id: String,
    monitor_index: usize,
) -> Result<(), String> {
    let mut state = overlay_ready_state();
    if state.session_id.as_deref() == Some(session_id.as_str())
        && state.pending.remove(&monitor_index)
    {
        overlay_ready().1.notify_all();
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Finishing actions
// ---------------------------------------------------------------------------------------------

fn raw_png_body(request: &InvokeRequest<'_>) -> Result<Vec<u8>, String> {
    match request.body() {
        InvokeBody::Raw(bytes) if bytes.starts_with(&PNG_SIGNATURE) => Ok(bytes.clone()),
        InvokeBody::Raw(_) => Err("截图数据不是有效的 PNG 图片".to_owned()),
        InvokeBody::Json(_) => Err("缺少截图图片数据".to_owned()),
    }
}

fn png_dimensions(png: &[u8]) -> Result<(u32, u32), String> {
    // The IHDR chunk always comes first: signature(8) + length(4) + type(4) + width + height.
    let read = |offset: usize| {
        png.get(offset..offset + 4)
            .map(|bytes| u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    };
    match (png.get(12..16), read(16), read(20)) {
        (Some(b"IHDR"), Some(width), Some(height)) if width > 0 && height > 0 => {
            Ok((width, height))
        }
        _ => Err("截图数据不是有效的 PNG 图片".to_owned()),
    }
}

fn parse_rect_header(request: &InvokeRequest<'_>) -> Result<PhysicalRect, String> {
    let value = request
        .headers()
        .get(RECT_HEADER)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| "缺少钉图选区位置".to_owned())?;
    let parts = value
        .split(',')
        .map(|part| part.trim().parse::<f64>().map(|value| value.round()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| format!("钉图选区位置无效：{value}"))?;
    match parts.as_slice() {
        [x, y, width, height] if *width >= 1.0 && *height >= 1.0 => Ok(PhysicalRect {
            x: *x as i32,
            y: *y as i32,
            width: *width as u32,
            height: *height as u32,
        }),
        _ => Err(format!("钉图选区位置无效：{value}")),
    }
}

fn default_screenshot_file_name() -> String {
    format!(
        "DustDesk截图_{}.png",
        chrono::Local::now().format("%Y%m%d_%H%M%S")
    )
}

fn set_windows_topmost(windows: &[WebviewWindow], topmost: bool) {
    for window in windows {
        let _ = window.set_always_on_top(topmost);
    }
}

/// Shows the native save dialog owned by `parent`. Topmost DustDesk windows are lowered while
/// the dialog is open so it is guaranteed to appear above them.
fn pick_save_path(
    app: &AppHandle,
    parent: &WebviewWindow,
    lowered: &[WebviewWindow],
) -> Option<PathBuf> {
    set_windows_topmost(lowered, false);
    let mut dialog = app
        .dialog()
        .file()
        .set_parent(parent)
        .set_title("保存截图")
        .add_filter("PNG 图片", &["png"])
        .set_file_name(default_screenshot_file_name());
    if let Ok(directory) = app.path().picture_dir() {
        dialog = dialog.set_directory(directory);
    }
    let picked = dialog.blocking_save_file();
    set_windows_topmost(lowered, true);
    let _ = parent.set_focus();

    let mut path = picked?.into_path().ok()?;
    let has_png_extension = path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("png"));
    if !has_png_extension {
        let mut file_name = path.file_name()?.to_os_string();
        file_name.push(".png");
        path.set_file_name(file_name);
    }
    Some(path)
}

fn write_png_file(path: &PathBuf, png: &[u8]) -> Result<(), String> {
    std::fs::write(path, png).map_err(|error| format!("保存截图失败：{error}"))
}

#[tauri::command]
pub async fn screenshot_copy(app: AppHandle, request: InvokeRequest<'_>) -> Result<(), String> {
    let png = raw_png_body(&request)?;
    run_blocking(move || {
        clipboard_bridge::copy_png_to_clipboard_with_history(&png)?;
        finish_session(&app);
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn screenshot_save(
    app: AppHandle,
    window: WebviewWindow,
    request: InvokeRequest<'_>,
) -> Result<Option<String>, String> {
    let png = raw_png_body(&request)?;
    run_blocking(move || {
        let overlays = overlay_windows(&app)
            .into_iter()
            .filter(|overlay| overlay.is_visible().unwrap_or(false))
            .collect::<Vec<_>>();
        let Some(path) = pick_save_path(&app, &window, &overlays) else {
            // Cancelled: the session stays open so the user can keep editing.
            return Ok(None);
        };
        write_png_file(&path, &png)?;
        finish_session(&app);
        Ok(Some(path.display().to_string()))
    })
    .await
}

#[tauri::command]
pub async fn screenshot_pin(app: AppHandle, request: InvokeRequest<'_>) -> Result<String, String> {
    let png = raw_png_body(&request)?;
    let rect = parse_rect_header(&request)?;
    let (width, height) = png_dimensions(&png)?;
    run_blocking(move || {
        let pin_id = NEXT_PIN_ID.fetch_add(1, Ordering::SeqCst);
        pins_slot().insert(
            pin_id,
            PinImage {
                png: Arc::new(png),
                width,
                height,
            },
        );
        let window = match with_lazy_window_operation(|| create_pin_window(&app, pin_id, rect)) {
            Ok(window) => window,
            Err(error) => {
                pins_slot().remove(&pin_id);
                return Err(error);
            }
        };
        finish_session(&app);
        let _ = window.set_focus();
        Ok(pin_id.to_string())
    })
    .await
}

fn create_pin_window(
    app: &AppHandle,
    pin_id: u64,
    rect: PhysicalRect,
) -> Result<WebviewWindow, String> {
    let label = pin_label(pin_id);
    let window = with_webview_window_creation(|| {
        WebviewWindowBuilder::new(
            app,
            &label,
            WebviewUrl::App(format!("index.html#/screenshot-pin?id={pin_id}").into()),
        )
        .title("DustDesk 钉图")
        .inner_size(320.0, 240.0)
        .position(OFFSCREEN_POSITION, OFFSCREEN_POSITION)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .decorations(false)
        .shadow(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .focused(false)
        .transparent(false)
        .visible(true)
        .build()
        .map_err(to_message)
    })?;
    if let Err(error) = place_window_physical(&window, rect) {
        let _ = window.destroy();
        return Err(error);
    }
    let _ = window.set_always_on_top(true);
    ensure_window_physical(&window, rect);
    Ok(window)
}

#[tauri::command]
pub async fn screenshot_ocr(request: InvokeRequest<'_>) -> Result<OcrResult, String> {
    let png = raw_png_body(&request)?;
    run_blocking(move || platform::recognize_text(&png)).await
}

#[tauri::command]
pub async fn screenshot_cancel(app: AppHandle) -> Result<(), String> {
    run_blocking(move || {
        finish_session(&app);
        Ok(())
    })
    .await
}

// ---------------------------------------------------------------------------------------------
// Pins
// ---------------------------------------------------------------------------------------------

fn pin_png(pin_id: u64) -> Result<Arc<Vec<u8>>, String> {
    pins_slot()
        .get(&pin_id)
        .map(|pin| pin.png.clone())
        .ok_or_else(|| "钉图不存在或已关闭".to_owned())
}

#[tauri::command]
pub async fn get_screenshot_pin(pin_id: PinIdArg) -> Result<ScreenshotPin, String> {
    let pin_id = pin_id.value()?;
    let pins = pins_slot();
    let pin = pins
        .get(&pin_id)
        .ok_or_else(|| "钉图不存在或已关闭".to_owned())?;
    Ok(ScreenshotPin {
        pin_id: pin_id.to_string(),
        image_url: protocol_url(&format!("pin/{pin_id}.png")),
        width: pin.width,
        height: pin.height,
    })
}

#[tauri::command]
pub async fn screenshot_pin_copy(pin_id: PinIdArg) -> Result<(), String> {
    let png = pin_png(pin_id.value()?)?;
    run_blocking(move || clipboard_bridge::copy_png_to_clipboard_with_history(&png)).await
}

#[tauri::command]
pub async fn screenshot_pin_save(
    app: AppHandle,
    pin_id: PinIdArg,
) -> Result<Option<String>, String> {
    let pin_id = pin_id.value()?;
    let png = pin_png(pin_id)?;
    run_blocking(move || {
        let window = app
            .get_webview_window(&pin_label(pin_id))
            .ok_or_else(|| "钉图窗口不存在".to_owned())?;
        let Some(path) = pick_save_path(&app, &window, std::slice::from_ref(&window)) else {
            return Ok(None);
        };
        write_png_file(&path, &png)?;
        Ok(Some(path.display().to_string()))
    })
    .await
}

#[tauri::command]
pub async fn screenshot_pin_close(app: AppHandle, pin_id: PinIdArg) -> Result<(), String> {
    let pin_id = pin_id.value()?;
    run_blocking(move || {
        let result = with_lazy_window_operation(|| {
            if let Some(window) = app.get_webview_window(&pin_label(pin_id)) {
                window.destroy().map_err(to_message)?;
            }
            Ok(())
        });
        pins_slot().remove(&pin_id);
        result
    })
    .await
}

/// Releases pin memory when a pin window goes away by any route (e.g. Alt+F4).
pub fn handle_window_event(window: &tauri::Window, event: &tauri::WindowEvent) {
    if !matches!(event, tauri::WindowEvent::Destroyed) {
        return;
    }
    if let Some(pin_id) = window
        .label()
        .strip_prefix(PIN_LABEL_PREFIX)
        .and_then(|id| id.parse::<u64>().ok())
    {
        pins_slot().remove(&pin_id);
    }
}

// ---------------------------------------------------------------------------------------------
// dustshot:// protocol
// ---------------------------------------------------------------------------------------------

pub fn handle_protocol_request(request: HttpRequest<Vec<u8>>, responder: UriSchemeResponder) {
    let is_preflight = request.method() == Method::OPTIONS;
    let path = request.uri().path().to_owned();
    tauri::async_runtime::spawn_blocking(move || {
        let started = Instant::now();
        let response = if is_preflight {
            protocol_response(StatusCode::NO_CONTENT, "text/plain", Vec::new())
        } else {
            match resolve_protocol_image(&path) {
                Some((content_type, bytes)) => {
                    let response =
                        protocol_response(StatusCode::OK, content_type, bytes.as_ref().clone());
                    if path.starts_with("/session/") {
                        let micros = started.elapsed().as_micros() as u64;
                        let _ = LAST_SERVE_MICROS.compare_exchange(
                            0,
                            micros.max(1),
                            Ordering::SeqCst,
                            Ordering::SeqCst,
                        );
                    }
                    response
                }
                None => protocol_response(StatusCode::NOT_FOUND, "text/plain", Vec::new()),
            }
        };
        responder.respond(response);
    });
}

fn resolve_protocol_image(path: &str) -> Option<(&'static str, Arc<Vec<u8>>)> {
    let segments = path.trim_matches('/').split('/').collect::<Vec<_>>();
    match segments.as_slice() {
        ["session", session_id, file] => {
            let index = file.strip_suffix(".bmp")?.parse::<usize>().ok()?;
            let session = session_slot();
            let session = session
                .as_ref()
                .filter(|session| session.id == *session_id)?;
            let capture = session
                .monitors
                .iter()
                .find(|capture| capture.info.index == index)?;
            Some(("image/bmp", capture.bmp.clone()))
        }
        ["pin", file] => {
            let pin_id = file.strip_suffix(".png")?.parse::<u64>().ok()?;
            Some(("image/png", pins_slot().get(&pin_id)?.png.clone()))
        }
        _ => None,
    }
}

fn protocol_response(
    status: StatusCode,
    content_type: &str,
    body: Vec<u8>,
) -> HttpResponse<Vec<u8>> {
    HttpResponse::builder()
        .status(status)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .header(header::ACCESS_CONTROL_ALLOW_METHODS, "GET, OPTIONS")
        .header(header::CACHE_CONTROL, "no-store")
        .body(body)
        .unwrap_or_else(|_| HttpResponse::new(Vec::new()))
}

// ---------------------------------------------------------------------------------------------
// OCR text post-processing
// ---------------------------------------------------------------------------------------------

fn is_cjk(ch: char) -> bool {
    matches!(
        ch as u32,
        0x3000..=0x303F
            | 0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xF900..=0xFAFF
            | 0xFF00..=0xFFEF
            | 0x20000..=0x2A6DF
    )
}

/// Joins OCR words with spaces, except between two CJK characters when the recognizer is
/// Chinese (Windows OCR separates every Han character with a space).
fn join_ocr_words<'a>(words: impl IntoIterator<Item = &'a str>, chinese: bool) -> String {
    let mut text = String::new();
    for word in words {
        let word = word.trim();
        if word.is_empty() {
            continue;
        }
        if let (Some(previous), Some(next)) = (text.chars().last(), word.chars().next()) {
            if !(chinese && is_cjk(previous) && is_cjk(next)) {
                text.push(' ');
            }
        }
        text.push_str(word);
    }
    text
}

// ---------------------------------------------------------------------------------------------
// Platform layer
// ---------------------------------------------------------------------------------------------

#[cfg(windows)]
mod platform {
    use std::{ffi::c_void, mem::size_of, ptr::null_mut};

    use windows_sys::{
        core::BOOL,
        Win32::{
            Foundation::{HWND, LPARAM, POINT, RECT},
            Graphics::{
                Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS},
                Gdi::{
                    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject,
                    GetDC, GetDIBits, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER,
                    BI_RGB, CAPTUREBLT, DIB_RGB_COLORS, SRCCOPY,
                },
            },
            UI::WindowsAndMessaging::{
                EnumWindows, GetCursorPos, GetLayeredWindowAttributes, GetWindow, GetWindowLongW,
                GetWindowRect, GetWindowTextW, IsIconic, IsWindowVisible, GWL_EXSTYLE, GW_CHILD,
                GW_HWNDNEXT, LWA_ALPHA, WS_EX_LAYERED, WS_EX_TRANSPARENT,
            },
        },
    };

    use super::OcrResult;

    const BMP_FILE_HEADER_SIZE: usize = 14;
    const BMP_INFO_HEADER_SIZE: usize = 40;
    const BMP_HEADER_SIZE: usize = BMP_FILE_HEADER_SIZE + BMP_INFO_HEADER_SIZE;
    const MAX_WINDOW_RECTS: usize = 4096;
    const MAX_CHILD_DEPTH: usize = 6;
    const MIN_CHILD_EDGE: i32 = 8;

    pub struct RawWindow {
        pub left: i32,
        pub top: i32,
        pub right: i32,
        pub bottom: i32,
        pub title: String,
    }

    /// Captures one monitor area into an uncompressed top-down 24bpp BMP file.
    ///
    /// 24bpp lets GDI write the final pixel rows directly: a 32bpp capture has an undefined alpha
    /// channel that would need a per-pixel pass to make opaque, which is slow in debug builds.
    pub fn capture_bmp(x: i32, y: i32, width: u32, height: u32) -> Result<Vec<u8>, String> {
        if width == 0 || height == 0 {
            return Err("显示器尺寸无效".to_owned());
        }
        // BMP rows are padded to a multiple of 4 bytes.
        let pixel_bytes = (width as usize)
            .checked_mul(3)
            .map(|row| (row + 3) & !3)
            .and_then(|stride| stride.checked_mul(height as usize))
            .filter(|bytes| *bytes <= u32::MAX as usize - BMP_HEADER_SIZE)
            .ok_or_else(|| "显示器尺寸过大".to_owned())?;
        let mut bmp = vec![0u8; BMP_HEADER_SIZE + pixel_bytes];

        unsafe {
            let screen = GetDC(null_mut());
            if screen.is_null() {
                return Err("无法获取屏幕设备上下文".to_owned());
            }
            let memory = CreateCompatibleDC(screen);
            let bitmap = CreateCompatibleBitmap(screen, width as i32, height as i32);
            if memory.is_null() || bitmap.is_null() {
                if !bitmap.is_null() {
                    DeleteObject(bitmap);
                }
                if !memory.is_null() {
                    DeleteDC(memory);
                }
                ReleaseDC(null_mut(), screen);
                return Err("无法创建截图缓冲区".to_owned());
            }

            let previous = SelectObject(memory, bitmap);
            let copied = BitBlt(
                memory,
                0,
                0,
                width as i32,
                height as i32,
                screen,
                x,
                y,
                SRCCOPY | CAPTUREBLT,
            );
            SelectObject(memory, previous);

            let mut info: BITMAPINFO = std::mem::zeroed();
            info.bmiHeader = BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width as i32,
                biHeight: -(height as i32),
                biPlanes: 1,
                biBitCount: 24,
                biCompression: BI_RGB,
                ..std::mem::zeroed()
            };
            let lines = if copied != 0 {
                GetDIBits(
                    memory,
                    bitmap,
                    0,
                    height,
                    bmp[BMP_HEADER_SIZE..].as_mut_ptr().cast::<c_void>(),
                    &mut info,
                    DIB_RGB_COLORS,
                )
            } else {
                0
            };

            DeleteObject(bitmap);
            DeleteDC(memory);
            ReleaseDC(null_mut(), screen);

            if copied == 0 || lines == 0 {
                return Err("截取屏幕失败".to_owned());
            }
        }

        let file_size = bmp.len() as u32;
        bmp[0..2].copy_from_slice(b"BM");
        bmp[2..6].copy_from_slice(&file_size.to_le_bytes());
        bmp[10..14].copy_from_slice(&(BMP_HEADER_SIZE as u32).to_le_bytes());
        let info = &mut bmp[BMP_FILE_HEADER_SIZE..BMP_HEADER_SIZE];
        info[0..4].copy_from_slice(&(BMP_INFO_HEADER_SIZE as u32).to_le_bytes());
        info[4..8].copy_from_slice(&(width as i32).to_le_bytes());
        info[8..12].copy_from_slice(&(-(height as i32)).to_le_bytes());
        info[12..14].copy_from_slice(&1u16.to_le_bytes());
        info[14..16].copy_from_slice(&24u16.to_le_bytes());
        info[20..24].copy_from_slice(&(pixel_bytes as u32).to_le_bytes());
        Ok(bmp)
    }

    pub fn cursor_position() -> Option<(i32, i32)> {
        let mut point = POINT { x: 0, y: 0 };
        (unsafe { GetCursorPos(&mut point) } != 0).then_some((point.x, point.y))
    }

    /// Visible top-level windows from top to bottom of the Z order. Each window's visible
    /// descendants are emitted before it, deepest first, so the first hit is the most specific.
    pub fn enumerate_windows(excluded: &[isize]) -> Vec<RawWindow> {
        let mut top_level: Vec<HWND> = Vec::new();
        unsafe {
            EnumWindows(
                Some(collect_top_level),
                &mut top_level as *mut Vec<HWND> as LPARAM,
            );
        }

        let mut output = Vec::new();
        for hwnd in top_level {
            if output.len() >= MAX_WINDOW_RECTS {
                break;
            }
            if excluded.contains(&(hwnd as isize)) || !is_capturable_top_level(hwnd) {
                continue;
            }
            let Some(bounds) = frame_bounds(hwnd) else {
                continue;
            };
            collect_children(hwnd, bounds, 0, &mut output);
            output.push(raw_window(bounds, window_title(hwnd)));
        }
        output
    }

    unsafe extern "system" fn collect_top_level(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let list = unsafe { &mut *(lparam as *mut Vec<HWND>) };
        list.push(hwnd);
        1
    }

    fn is_capturable_top_level(hwnd: HWND) -> bool {
        unsafe {
            if IsWindowVisible(hwnd) == 0 || IsIconic(hwnd) != 0 || is_cloaked(hwnd) {
                return false;
            }
            let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
            if ex_style & WS_EX_LAYERED != 0 {
                // Click-through or fully transparent layered windows are invisible overlays.
                if ex_style & WS_EX_TRANSPARENT != 0 {
                    return false;
                }
                let mut alpha = 255u8;
                let mut flags = 0u32;
                if GetLayeredWindowAttributes(hwnd, null_mut(), &mut alpha, &mut flags) != 0
                    && flags & LWA_ALPHA != 0
                    && alpha == 0
                {
                    return false;
                }
            }
        }
        true
    }

    fn is_cloaked(hwnd: HWND) -> bool {
        let mut cloaked = 0u32;
        let result = unsafe {
            DwmGetWindowAttribute(
                hwnd,
                DWMWA_CLOAKED as _,
                (&mut cloaked as *mut u32).cast::<c_void>(),
                size_of::<u32>() as u32,
            )
        };
        result == 0 && cloaked != 0
    }

    fn frame_bounds(hwnd: HWND) -> Option<RECT> {
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        let result = unsafe {
            DwmGetWindowAttribute(
                hwnd,
                DWMWA_EXTENDED_FRAME_BOUNDS as _,
                (&mut rect as *mut RECT).cast::<c_void>(),
                size_of::<RECT>() as u32,
            )
        };
        let rect = if result == 0 {
            Some(rect)
        } else {
            window_rect(hwnd)
        }?;
        (rect.right > rect.left && rect.bottom > rect.top).then_some(rect)
    }

    fn window_rect(hwnd: HWND) -> Option<RECT> {
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        (unsafe { GetWindowRect(hwnd, &mut rect) } != 0).then_some(rect)
    }

    fn collect_children(parent: HWND, clip: RECT, depth: usize, output: &mut Vec<RawWindow>) {
        if depth >= MAX_CHILD_DEPTH {
            return;
        }
        let mut child = unsafe { GetWindow(parent, GW_CHILD) };
        while !child.is_null() && output.len() < MAX_WINDOW_RECTS {
            if unsafe { IsWindowVisible(child) } != 0 {
                if let Some(rect) = window_rect(child).and_then(|rect| intersect(rect, clip)) {
                    if rect.right - rect.left >= MIN_CHILD_EDGE
                        && rect.bottom - rect.top >= MIN_CHILD_EDGE
                    {
                        collect_children(child, rect, depth + 1, output);
                        // Child titles are skipped: reading them may message other processes.
                        output.push(raw_window(rect, String::new()));
                    }
                }
            }
            child = unsafe { GetWindow(child, GW_HWNDNEXT) };
        }
    }

    fn intersect(a: RECT, b: RECT) -> Option<RECT> {
        let rect = RECT {
            left: a.left.max(b.left),
            top: a.top.max(b.top),
            right: a.right.min(b.right),
            bottom: a.bottom.min(b.bottom),
        };
        (rect.right > rect.left && rect.bottom > rect.top).then_some(rect)
    }

    fn raw_window(rect: RECT, title: String) -> RawWindow {
        RawWindow {
            left: rect.left,
            top: rect.top,
            right: rect.right,
            bottom: rect.bottom,
            title,
        }
    }

    fn window_title(hwnd: HWND) -> String {
        let mut buffer = [0u16; 256];
        let len = unsafe { GetWindowTextW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32) };
        String::from_utf16_lossy(&buffer[..len.max(0) as usize])
    }

    pub fn recognize_text(png: &[u8]) -> Result<OcrResult, String> {
        ocr::recognize(png).map_err(|error| match error {
            ocr::OcrError::Message(message) => message,
            ocr::OcrError::Windows(error) => format!("文字识别失败：{}", error.message()),
        })
    }

    mod ocr {
        use windows::{
            core::HSTRING,
            Globalization::Language,
            Graphics::Imaging::{BitmapAlphaMode, BitmapDecoder, BitmapPixelFormat},
            Media::Ocr::OcrEngine,
            Storage::Streams::{DataWriter, InMemoryRandomAccessStream},
        };

        use crate::screenshot::{join_ocr_words, OcrLine, OcrResult};

        const PREFERRED_LANGUAGES: [&str; 2] = ["zh-Hans-CN", "zh-Hans"];

        pub enum OcrError {
            Message(String),
            Windows(windows::core::Error),
        }

        impl From<windows::core::Error> for OcrError {
            fn from(error: windows::core::Error) -> Self {
                Self::Windows(error)
            }
        }

        fn create_engine() -> Result<OcrEngine, OcrError> {
            for tag in PREFERRED_LANGUAGES {
                let Ok(language) = Language::CreateLanguage(&HSTRING::from(tag)) else {
                    continue;
                };
                if !OcrEngine::IsLanguageSupported(&language).unwrap_or(false) {
                    continue;
                }
                if let Ok(engine) = OcrEngine::TryCreateFromLanguage(&language) {
                    return Ok(engine);
                }
            }
            OcrEngine::TryCreateFromUserProfileLanguages()
                .map_err(|_| OcrError::Message("系统未安装可用的 OCR 语言包".to_owned()))
        }

        pub fn recognize(png: &[u8]) -> Result<OcrResult, OcrError> {
            let engine = create_engine()?;

            let stream = InMemoryRandomAccessStream::new()?;
            let writer = DataWriter::CreateDataWriter(&stream)?;
            writer.WriteBytes(png)?;
            writer.StoreAsync()?.get()?;
            writer.FlushAsync()?.get()?;
            writer.DetachStream()?;
            stream.Seek(0)?;

            let decoder = BitmapDecoder::CreateAsync(&stream)?.get()?;
            let max_dimension = OcrEngine::MaxImageDimension()?;
            if decoder.PixelWidth()? > max_dimension || decoder.PixelHeight()? > max_dimension {
                return Err(OcrError::Message(format!(
                    "图片尺寸超过系统 OCR 上限（{max_dimension} 像素）"
                )));
            }
            let bitmap = decoder
                .GetSoftwareBitmapConvertedAsync(
                    BitmapPixelFormat::Bgra8,
                    BitmapAlphaMode::Premultiplied,
                )?
                .get()?;

            let chinese = engine
                .RecognizerLanguage()
                .and_then(|language| language.LanguageTag())
                .map(|tag| tag.to_string().to_ascii_lowercase().starts_with("zh"))
                .unwrap_or(false);
            let result = engine.RecognizeAsync(&bitmap)?.get()?;

            let mut lines = Vec::new();
            for line in result.Lines()? {
                let mut words = Vec::new();
                let mut bounds: Option<(f64, f64, f64, f64)> = None;
                for word in line.Words()? {
                    words.push(word.Text()?.to_string());
                    let rect = word.BoundingRect()?;
                    let (left, top) = (rect.X as f64, rect.Y as f64);
                    let (right, bottom) = (left + rect.Width as f64, top + rect.Height as f64);
                    bounds = Some(match bounds {
                        Some((l, t, r, b)) => {
                            (l.min(left), t.min(top), r.max(right), b.max(bottom))
                        }
                        None => (left, top, right, bottom),
                    });
                }
                let text = if words.is_empty() {
                    line.Text()?.to_string()
                } else {
                    join_ocr_words(words.iter().map(String::as_str), chinese)
                };
                if text.trim().is_empty() {
                    continue;
                }
                let (left, top, right, bottom) = bounds.unwrap_or_default();
                lines.push(OcrLine {
                    text,
                    x: left,
                    y: top,
                    width: right - left,
                    height: bottom - top,
                });
            }

            let text = lines
                .iter()
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            Ok(OcrResult { text, lines })
        }
    }
}

#[cfg(not(windows))]
mod platform {
    use super::OcrResult;

    pub struct RawWindow {
        pub left: i32,
        pub top: i32,
        pub right: i32,
        pub bottom: i32,
        pub title: String,
    }

    pub fn capture_bmp(_x: i32, _y: i32, _width: u32, _height: u32) -> Result<Vec<u8>, String> {
        Err("当前平台暂不支持截图".to_owned())
    }

    pub fn cursor_position() -> Option<(i32, i32)> {
        None
    }

    pub fn enumerate_windows(_excluded: &[isize]) -> Vec<RawWindow> {
        Vec::new()
    }

    pub fn recognize_text(_png: &[u8]) -> Result<OcrResult, String> {
        Err("当前平台暂不支持文字识别".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chinese_ocr_words_drop_spaces_between_han_characters() {
        assert_eq!(join_ocr_words(["你", "好", "world"], true), "你好 world");
        assert_eq!(join_ocr_words(["hello", "world"], true), "hello world");
        assert_eq!(join_ocr_words(["你", "好"], false), "你 好");
    }

    #[test]
    fn protocol_paths_for_missing_session_return_none() {
        assert!(resolve_protocol_image("/session/999/0.bmp").is_none());
        assert!(resolve_protocol_image("/pin/999.png").is_none());
        assert!(resolve_protocol_image("/unknown").is_none());
    }

    #[test]
    fn png_dimensions_reads_ihdr() {
        let mut png = PNG_SIGNATURE.to_vec();
        png.extend_from_slice(&13u32.to_be_bytes());
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&640u32.to_be_bytes());
        png.extend_from_slice(&480u32.to_be_bytes());
        assert_eq!(png_dimensions(&png), Ok((640, 480)));
        assert!(png_dimensions(&PNG_SIGNATURE).is_err());
    }
}
