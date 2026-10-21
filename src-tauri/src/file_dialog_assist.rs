//! File dialog search assistant: when another program shows a standard open/save dialog, a
//! DustDesk panel is docked below it to jump to organizer folders or pick organized files.
//!
//! Detection only uses out-of-context WinEvent hooks on a dedicated message-pump thread; nothing
//! is injected into other processes. Hook callbacks only post thread messages; window operations
//! run on blocking workers.

use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicIsize, AtomicU32, AtomicU64, Ordering},
        mpsc, Mutex, OnceLock,
    },
    thread,
    time::{Duration, UNIX_EPOCH},
};

use pinyin::ToPinyinMulti;
use serde::{Deserialize, Serialize};
use tauri::{Emitter, EventTarget, Manager, WebviewUrl, WebviewWindowBuilder};

use crate::{
    append_desktop_diagnostic, configured_settings, effective_search_paths, is_noise_search_dir,
    safe_windows_file_name, to_message, with_config_mutation, with_lazy_window_operation,
    with_webview_window_creation, AppSettings, AppStore, SEARCH_SCAN_LIMIT,
};

pub const FILE_DIALOG_ASSIST_CHANGED_EVENT: &str = "dustdesk://file-dialog-assist-changed";
const PANEL_LABEL: &str = "file-dialog-assist";
const PANEL_HEIGHT: f64 = 300.0;
const PANEL_GAP: f64 = 4.0;
const PANEL_PREWARM_DELAY: Duration = Duration::from_secs(4);
const RESULT_LIMIT: usize = 50;
const LIST_LIMIT: usize = 500;
const INTERNAL_NAME_PREFIX: &str = ".dustdesk-";

static APP_HANDLE: OnceLock<tauri::AppHandle> = OnceLock::new();
static HOOK_CONTROL_LOCK: Mutex<()> = Mutex::new(());
static HOOK_THREAD_ID: AtomicU32 = AtomicU32::new(0);
static HOOK_GENERATION: AtomicU64 = AtomicU64::new(0);
static TRACKED_HWND: AtomicIsize = AtomicIsize::new(0);
static LOCATION_PENDING: AtomicBool = AtomicBool::new(false);
static PANEL_HWND: AtomicIsize = AtomicIsize::new(0);
static ACTIVE_DIALOG: Mutex<Option<DialogHandles>> = Mutex::new(None);
static PANEL_REQUEST: Mutex<PanelRequest> = Mutex::new(PanelRequest::hidden());
static PANEL_WORKER_RUNNING: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PhysicalRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl PhysicalRect {
    fn width(&self) -> i32 {
        self.right - self.left
    }

    fn height(&self) -> i32 {
        self.bottom - self.top
    }
}

/// Docks the panel below the dialog; above when the work area has no room below; inside the
/// bottom of the work area when neither side fits. All values are physical pixels.
pub fn panel_placement(
    dialog: PhysicalRect,
    work_area: PhysicalRect,
    panel_height: i32,
    gap: i32,
) -> PhysicalRect {
    let width = dialog.width().clamp(1, work_area.width().max(1));
    let height = panel_height.clamp(1, work_area.height().max(1));
    let left = dialog.left.clamp(
        work_area.left,
        (work_area.right - width).max(work_area.left),
    );

    let below = dialog.bottom + gap;
    let above = dialog.top - gap - height;
    let top = if below + height <= work_area.bottom {
        below
    } else if above >= work_area.top {
        above
    } else {
        work_area.bottom - height
    };

    PhysicalRect {
        left,
        top,
        right: left + width,
        bottom: top + height,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DialogKind {
    Open,
    Save,
    Folder,
}

/// Infers the dialog type from the IDOK button caption, falling back to the title. Unknown
/// captions are treated as an open dialog.
pub fn classify_dialog_kind(button_text: &str, title: &str) -> DialogKind {
    fn normalized(text: &str) -> String {
        text.chars()
            .filter(|ch| *ch != '&')
            .collect::<String>()
            .to_lowercase()
    }

    fn kind_of(text: &str) -> Option<DialogKind> {
        let folder_words = ["选择文件夹", "選擇資料夾", "select folder", "choose folder"];
        let save_words = ["保存", "儲存", "另存", "save"];
        let open_words = ["打开", "開啟", "open", "上传", "upload"];
        if folder_words.iter().any(|word| text.contains(word)) {
            Some(DialogKind::Folder)
        } else if save_words.iter().any(|word| text.contains(word)) {
            Some(DialogKind::Save)
        } else if open_words.iter().any(|word| text.contains(word)) {
            Some(DialogKind::Open)
        } else {
            None
        }
    }

    kind_of(&normalized(button_text))
        .or_else(|| kind_of(&normalized(title)))
        .unwrap_or(DialogKind::Open)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DialogActionMode {
    /// Folders navigate; files are selected in open dialogs and only located otherwise.
    Auto,
    /// Navigate to the containing folder and fill in the file name without confirming.
    Locate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialogAction {
    Navigate(PathBuf),
    Select(PathBuf),
    Locate { folder: PathBuf, file_name: String },
}

/// Decides what clicking an entry does. Save and folder-picker dialogs only ever navigate and
/// keep their prefilled name, so an existing file can never be overwritten from the panel.
pub fn plan_dialog_action(
    kind: DialogKind,
    mode: DialogActionMode,
    path: &Path,
    is_dir: bool,
) -> Option<DialogAction> {
    if is_dir {
        return Some(DialogAction::Navigate(path.to_path_buf()));
    }
    let folder = path.parent()?.to_path_buf();
    match (kind, mode) {
        (DialogKind::Open, DialogActionMode::Auto) => {
            Some(DialogAction::Select(path.to_path_buf()))
        }
        (DialogKind::Open, DialogActionMode::Locate) => Some(DialogAction::Locate {
            folder,
            file_name: path.file_name()?.to_string_lossy().into_owned(),
        }),
        (DialogKind::Save | DialogKind::Folder, _) => Some(DialogAction::Navigate(folder)),
    }
}

/// Match quality, lower is better.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MatchTier {
    Exact = 0,
    Prefix = 1,
    Substring = 2,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameMatch {
    pub tier: MatchTier,
    /// Matched character indices (Unicode scalar positions) for highlighting.
    pub highlight: Vec<usize>,
}

/// Case-insensitive substring match on the name, plus pinyin initial matching such as `ndbg`
/// for 「年度报告」. `query_lower` must already be trimmed and lowercased.
pub fn match_name(name: &str, query_lower: &str) -> Option<NameMatch> {
    if query_lower.is_empty() {
        return None;
    }
    let text = text_match(name, query_lower);
    let initials = if query_lower.chars().all(|ch| ch.is_ascii_alphanumeric()) {
        initials_match(name, query_lower)
    } else {
        None
    };
    match (text, initials) {
        (Some(text), Some(initials)) if initials.tier < text.tier => Some(initials),
        (Some(text), _) => Some(text),
        (None, initials) => initials,
    }
}

fn text_match(name: &str, query_lower: &str) -> Option<NameMatch> {
    let chars = name.chars().collect::<Vec<_>>();
    let lower = chars
        .iter()
        .map(|ch| ch.to_lowercase().next().unwrap_or(*ch))
        .collect::<Vec<_>>();
    let query = query_lower.chars().collect::<Vec<_>>();
    if query.len() > lower.len() {
        return None;
    }
    let start =
        (0..=lower.len() - query.len()).find(|start| lower[*start..].starts_with(&query))?;
    let stem_len = stem_char_len(name);
    let tier = if start == 0 && (query.len() == lower.len() || query.len() == stem_len) {
        MatchTier::Exact
    } else if start == 0 {
        MatchTier::Prefix
    } else {
        MatchTier::Substring
    };
    Some(NameMatch {
        tier,
        highlight: (start..start + query.len()).collect(),
    })
}

fn stem_char_len(name: &str) -> usize {
    match name.rfind('.') {
        Some(index) if index > 0 => name[..index].chars().count(),
        _ => name.chars().count(),
    }
}

/// Per searchable character: its index in the name and the initial letters it can stand for.
/// Separators and symbols are skipped so 「年度 报告」 still matches `ndbg`.
fn name_initials(name: &str) -> Vec<(usize, Vec<char>)> {
    name.chars()
        .enumerate()
        .filter_map(|(index, ch)| {
            if ch.is_ascii_alphanumeric() {
                return Some((index, vec![ch.to_ascii_lowercase()]));
            }
            let multi = ch.to_pinyin_multi()?;
            let mut letters = Vec::new();
            for pinyin in multi {
                if let Some(letter) = pinyin.first_letter().chars().next() {
                    let letter = letter.to_ascii_lowercase();
                    if !letters.contains(&letter) {
                        letters.push(letter);
                    }
                }
            }
            (!letters.is_empty()).then_some((index, letters))
        })
        .collect()
}

fn initials_match(name: &str, query_lower: &str) -> Option<NameMatch> {
    let initials = name_initials(name);
    let query = query_lower.chars().collect::<Vec<_>>();
    if query.len() > initials.len() {
        return None;
    }
    let start = (0..=initials.len() - query.len()).find(|start| {
        query
            .iter()
            .enumerate()
            .all(|(offset, ch)| initials[start + offset].1.contains(ch))
    })?;
    let stem_len = stem_char_len(name);
    let stem_initials = initials
        .iter()
        .filter(|(index, _)| *index < stem_len)
        .count();
    let tier = if start == 0 && query.len() == stem_initials {
        MatchTier::Exact
    } else if start == 0 {
        MatchTier::Prefix
    } else {
        MatchTier::Substring
    };
    Some(NameMatch {
        tier,
        highlight: initials[start..start + query.len()]
            .iter()
            .map(|(index, _)| *index)
            .collect(),
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct FileDialogEntry {
    pub name: String,
    pub path: String,
    pub parent: String,
    pub is_dir: bool,
    pub extension: String,
    pub modified_ms: Option<i64>,
    pub highlight: Vec<usize>,
    #[serde(skip)]
    tier: Option<MatchTier>,
}

/// Exact before prefix before substring; newer modification first within a tier.
fn sort_search_results(entries: &mut [FileDialogEntry]) {
    entries.sort_by(|left, right| {
        left.tier
            .cmp(&right.tier)
            .then_with(|| right.modified_ms.cmp(&left.modified_ms))
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });
}

/// Folders first, then newest first.
fn sort_listing(entries: &mut [FileDialogEntry]) {
    entries.sort_by(|left, right| {
        right
            .is_dir
            .cmp(&left.is_dir)
            .then_with(|| right.modified_ms.cmp(&left.modified_ms))
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });
}

fn is_internal_entry_name(name: &str) -> bool {
    name.to_ascii_lowercase().starts_with(INTERNAL_NAME_PREFIX)
}

fn entry_from_path(path: &Path, is_dir: bool, name_match: Option<NameMatch>) -> FileDialogEntry {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    let modified_ms = std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .and_then(|duration| i64::try_from(duration.as_millis()).ok());
    let (tier, highlight) = match name_match {
        Some(name_match) => (Some(name_match.tier), name_match.highlight),
        None => (None, Vec::new()),
    };
    FileDialogEntry {
        name,
        path: path.display().to_string(),
        parent: path
            .parent()
            .map(|parent| parent.display().to_string())
            .unwrap_or_default(),
        is_dir,
        extension: if is_dir {
            "DIR".to_owned()
        } else {
            crate::path_extension(path)
        },
        modified_ms,
        highlight,
        tier,
    }
}

fn search_roots(root: Option<&str>) -> Result<Vec<PathBuf>, String> {
    let store = AppStore::open().map_err(to_message)?;
    let config = store.load_config();
    let roots = effective_search_paths(&store, &config.settings)
        .into_iter()
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    let Some(root) = root.map(str::trim).filter(|root| !root.is_empty()) else {
        return Ok(roots);
    };
    let root = PathBuf::from(root);
    if roots
        .iter()
        .any(|allowed| crate::is_path_within(&root, allowed))
    {
        Ok(vec![root])
    } else {
        Err("只能浏览搜索范围内的目录".to_owned())
    }
}

fn search_entries(roots: &[PathBuf], query_lower: &str) -> Vec<FileDialogEntry> {
    let mut scanned = 0usize;
    let mut matches = Vec::new();
    for root in roots {
        let mut queue = VecDeque::from([root.clone()]);
        while let Some(dir) = queue.pop_front() {
            if scanned >= SEARCH_SCAN_LIMIT {
                break;
            }
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                if scanned >= SEARCH_SCAN_LIMIT {
                    break;
                }
                scanned += 1;
                let name = entry.file_name().to_string_lossy().into_owned();
                if is_internal_entry_name(&name) {
                    continue;
                }
                let Ok(file_type) = entry.file_type() else {
                    continue;
                };
                let is_dir = file_type.is_dir();
                if is_dir && is_noise_search_dir(&name) {
                    continue;
                }
                let path = entry.path();
                if let Some(name_match) = match_name(&name, query_lower) {
                    matches.push(entry_from_path(&path, is_dir, Some(name_match)));
                }
                if is_dir {
                    queue.push_back(path);
                }
            }
        }
    }
    sort_search_results(&mut matches);
    matches.truncate(RESULT_LIMIT);
    matches
}

fn list_entries(dir: &Path) -> Result<Vec<FileDialogEntry>, String> {
    let entries = std::fs::read_dir(dir).map_err(to_message)?;
    let mut items = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if is_internal_entry_name(&name) {
                return None;
            }
            let is_dir = entry.file_type().ok()?.is_dir();
            Some(entry_from_path(&entry.path(), is_dir, None))
        })
        .take(LIST_LIMIT)
        .collect::<Vec<_>>();
    sort_listing(&mut items);
    Ok(items)
}

#[derive(Debug, Clone, Serialize)]
pub struct FileDialogShortcut {
    pub name: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileDialogInfo {
    pub id: String,
    pub kind: DialogKind,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileDialogAssistState {
    pub dialog: Option<FileDialogInfo>,
    pub organizer_root: String,
    pub shortcuts: Vec<FileDialogShortcut>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FileDialogActionOutcome {
    Done,
    /// The dialog belongs to an elevated process and UIPI blocked the messages.
    Elevated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DialogHandles {
    hwnd: isize,
    edit: isize,
    ok: isize,
    kind: DialogKind,
    pid: u32,
    tid: u32,
}

impl DialogHandles {
    fn info(&self) -> FileDialogInfo {
        FileDialogInfo {
            id: format!("{:x}", self.hwnd),
            kind: self.kind,
        }
    }
}

fn active_dialog() -> Option<DialogHandles> {
    ACTIVE_DIALOG.lock().ok().and_then(|dialog| *dialog)
}

fn set_active_dialog(dialog: Option<DialogHandles>) {
    if let Ok(mut active) = ACTIVE_DIALOG.lock() {
        *active = dialog;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PanelRequest {
    rect: Option<PhysicalRect>,
    dialog: Option<DialogHandles>,
}

impl PanelRequest {
    const fn hidden() -> Self {
        Self {
            rect: None,
            dialog: None,
        }
    }
}

/// Latest-wins panel placement: hook-thread handlers only record the desired state, a single
/// blocking worker applies it so rapid location changes coalesce.
fn request_panel(rect: Option<PhysicalRect>, dialog: Option<DialogHandles>) {
    if let Ok(mut request) = PANEL_REQUEST.lock() {
        *request = PanelRequest { rect, dialog };
    }
    if PANEL_WORKER_RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    let Some(app) = APP_HANDLE.get().cloned() else {
        PANEL_WORKER_RUNNING.store(false, Ordering::SeqCst);
        return;
    };
    tauri::async_runtime::spawn_blocking(move || run_panel_worker(&app));
}

fn current_panel_request() -> PanelRequest {
    PANEL_REQUEST
        .lock()
        .map(|request| *request)
        .unwrap_or(PanelRequest::hidden())
}

fn run_panel_worker(app: &tauri::AppHandle) {
    static APPLIED: Mutex<PanelRequest> = Mutex::new(PanelRequest::hidden());
    loop {
        let request = current_panel_request();
        let applied = APPLIED
            .lock()
            .map(|applied| *applied)
            .unwrap_or(PanelRequest::hidden());
        if request != applied {
            if let Err(error) = apply_panel_request(app, request, applied) {
                append_desktop_diagnostic("file_dialog_assist_panel_failed", &error);
            }
            if let Ok(mut slot) = APPLIED.lock() {
                *slot = request;
            }
            continue;
        }
        PANEL_WORKER_RUNNING.store(false, Ordering::SeqCst);
        // A request may have arrived between the comparison and releasing the flag.
        if current_panel_request() == request || PANEL_WORKER_RUNNING.swap(true, Ordering::SeqCst) {
            return;
        }
    }
}

fn apply_panel_request(
    app: &tauri::AppHandle,
    request: PanelRequest,
    applied: PanelRequest,
) -> Result<(), String> {
    let Some(rect) = request.rect else {
        platform::hide_panel(PANEL_HWND.load(Ordering::SeqCst));
        return Ok(());
    };
    let hwnd = ensure_panel(app)?;
    if request.dialog.map(|dialog| dialog.hwnd) != applied.dialog.map(|dialog| dialog.hwnd) {
        let info = request.dialog.map(|dialog| dialog.info());
        let _ = app.emit_to(
            EventTarget::webview_window(PANEL_LABEL),
            FILE_DIALOG_ASSIST_CHANGED_EVENT,
            info,
        );
    }
    platform::show_panel(hwnd, rect);
    Ok(())
}

fn ensure_panel(app: &tauri::AppHandle) -> Result<isize, String> {
    let existing = PANEL_HWND.load(Ordering::SeqCst);
    if existing != 0 && app.get_webview_window(PANEL_LABEL).is_some() {
        return Ok(existing);
    }
    with_lazy_window_operation(|| {
        let window = match app.get_webview_window(PANEL_LABEL) {
            Some(window) => window,
            None => with_webview_window_creation(|| {
                // Created visible off-screen with `focused(false)`, then hidden natively, so the
                // page is loaded ahead of time and later appearances never steal focus.
                WebviewWindowBuilder::new(
                    app,
                    PANEL_LABEL,
                    WebviewUrl::App("index.html#/file-dialog-assist".into()),
                )
                .title("DustDesk 文件对话框助手")
                .inner_size(640.0, PANEL_HEIGHT)
                .position(-32_000.0, -32_000.0)
                .resizable(false)
                .maximizable(false)
                .minimizable(false)
                .decorations(false)
                .always_on_top(true)
                .skip_taskbar(true)
                .focused(false)
                .visible(true)
                .build()
                .map_err(to_message)
            })?,
        };
        let hwnd = window.hwnd().map_err(to_message)?.0 as isize;
        platform::hide_panel(hwnd);
        PANEL_HWND.store(hwnd, Ordering::SeqCst);
        Ok(hwnd)
    })
}

fn prewarm_panel_async() {
    let Some(app) = APP_HANDLE.get().cloned() else {
        return;
    };
    tauri::async_runtime::spawn_blocking(move || {
        thread::sleep(PANEL_PREWARM_DELAY);
        if HOOK_THREAD_ID.load(Ordering::SeqCst) == 0 {
            return;
        }
        if let Err(error) = ensure_panel(&app) {
            append_desktop_diagnostic("file_dialog_assist_prewarm_failed", &error);
        }
    });
}

/// Called once from setup; starts the hook thread when the setting is enabled.
pub fn start(app: tauri::AppHandle) {
    let _ = APP_HANDLE.set(app);
    tauri::async_runtime::spawn_blocking(|| {
        if configured_settings().file_dialog_assist_enabled {
            start_hook_thread();
            prewarm_panel_async();
        }
    });
}

fn set_enabled(enabled: bool) {
    if enabled {
        start_hook_thread();
        prewarm_panel_async();
    } else {
        stop_hook_thread();
        request_panel(None, None);
    }
}

fn start_hook_thread() {
    let Ok(_guard) = HOOK_CONTROL_LOCK.lock() else {
        return;
    };
    if HOOK_THREAD_ID.load(Ordering::SeqCst) != 0 {
        return;
    }
    let generation = HOOK_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    let (ready_tx, ready_rx) = mpsc::sync_channel::<u32>(1);
    let spawned = thread::Builder::new()
        .name("dustdesk-file-dialog-assist".to_owned())
        .spawn(move || platform::run_hook_thread(generation, ready_tx));
    match spawned {
        Ok(_) => match ready_rx.recv_timeout(Duration::from_secs(2)) {
            Ok(thread_id) if thread_id != 0 => HOOK_THREAD_ID.store(thread_id, Ordering::SeqCst),
            _ => append_desktop_diagnostic(
                "file_dialog_assist_hook_failed",
                "hook thread did not report ready",
            ),
        },
        Err(error) => {
            append_desktop_diagnostic("file_dialog_assist_hook_failed", &error.to_string())
        }
    }
}

fn stop_hook_thread() {
    let Ok(_guard) = HOOK_CONTROL_LOCK.lock() else {
        return;
    };
    let thread_id = HOOK_THREAD_ID.swap(0, Ordering::SeqCst);
    // A newer generation makes the exiting thread skip clearing state owned by its successor.
    HOOK_GENERATION.fetch_add(1, Ordering::SeqCst);
    TRACKED_HWND.store(0, Ordering::SeqCst);
    set_active_dialog(None);
    if thread_id != 0 {
        platform::post_quit(thread_id);
    }
}

#[tauri::command]
pub async fn file_dialog_assist_state() -> Result<FileDialogAssistState, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let store = AppStore::open().map_err(to_message)?;
        let config = store.load_config();
        let organizer_root = store.organizer_root();
        let mut shortcuts = Vec::new();
        if organizer_root.is_dir() {
            shortcuts.push(FileDialogShortcut {
                name: "收纳目录".to_owned(),
                path: organizer_root.display().to_string(),
            });
        }
        for category in &config.desktop_categories {
            let name = category.name.trim();
            if name.is_empty() {
                continue;
            }
            let dir = organizer_root.join(safe_windows_file_name(name));
            if dir.is_dir() {
                shortcuts.push(FileDialogShortcut {
                    name: name.to_owned(),
                    path: dir.display().to_string(),
                });
            }
        }
        Ok(FileDialogAssistState {
            dialog: active_dialog().map(|dialog| dialog.info()),
            organizer_root: organizer_root.display().to_string(),
            shortcuts,
        })
    })
    .await
    .map_err(to_message)?
}

#[tauri::command]
pub async fn file_dialog_assist_search(
    query: String,
    root: Option<String>,
) -> Result<Vec<FileDialogEntry>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let query_lower = query.trim().to_lowercase();
        if query_lower.is_empty() {
            return Ok(Vec::new());
        }
        let roots = search_roots(root.as_deref())?;
        Ok(search_entries(&roots, &query_lower))
    })
    .await
    .map_err(to_message)?
}

#[tauri::command]
pub async fn file_dialog_assist_list(path: Option<String>) -> Result<Vec<FileDialogEntry>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let dir = match path
            .as_deref()
            .map(str::trim)
            .filter(|path| !path.is_empty())
        {
            Some(path) => {
                search_roots(Some(path))?;
                PathBuf::from(path)
            }
            None => AppStore::open().map_err(to_message)?.organizer_root(),
        };
        list_entries(&dir)
    })
    .await
    .map_err(to_message)?
}

#[tauri::command]
pub async fn file_dialog_assist_execute(
    path: String,
    mode: DialogActionMode,
) -> Result<FileDialogActionOutcome, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let dialog = active_dialog().ok_or_else(|| "文件对话框已关闭".to_owned())?;
        let path = PathBuf::from(path.trim());
        let metadata = std::fs::metadata(&path).map_err(|_| "目标已不存在".to_owned())?;
        let action = plan_dialog_action(dialog.kind, mode, &path, metadata.is_dir())
            .ok_or_else(|| "无法确定要跳转的目录".to_owned())?;
        let result = platform::perform_action(&dialog, &action);
        if let Err(error) = &result {
            append_desktop_diagnostic(
                "file_dialog_assist_action_failed",
                &format!("path=\"{}\" error=\"{}\"", path.display(), error),
            );
        }
        result
    })
    .await
    .map_err(to_message)?
}

#[tauri::command]
pub async fn file_dialog_assist_focus_dialog() -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(|| {
        let dialog = active_dialog().ok_or_else(|| "文件对话框已关闭".to_owned())?;
        platform::activate_window(dialog.hwnd);
        Ok(())
    })
    .await
    .map_err(to_message)?
}

#[tauri::command]
pub async fn update_file_dialog_assist_settings(enabled: bool) -> Result<AppSettings, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let settings = with_config_mutation(|| {
            let store = AppStore::open().map_err(to_message)?;
            let mut config = store.load_config_strict().map_err(to_message)?;
            config.settings.file_dialog_assist_enabled = enabled;
            store.save_config(&config).map_err(to_message)?;
            Ok(config.settings)
        })?;
        // Applied after the config transaction so hook start/stop never runs under its lock.
        set_enabled(settings.file_dialog_assist_enabled);
        Ok(settings)
    })
    .await
    .map_err(to_message)?
}

#[cfg(windows)]
mod platform {
    use std::{
        ffi::c_void,
        ptr::null_mut,
        sync::{atomic::Ordering, mpsc},
        thread,
        time::{Duration, Instant},
    };

    use windows_sys::Win32::{
        Foundation::{GetLastError, ERROR_ACCESS_DENIED, HWND, LPARAM, RECT, WPARAM},
        Graphics::{
            Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS},
            Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST},
        },
        System::Threading::{AttachThreadInput, GetCurrentThreadId},
        UI::{
            Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK},
            HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI},
            Input::KeyboardAndMouse::{SetActiveWindow, SetFocus},
            WindowsAndMessaging::{
                AllowSetForegroundWindow, BringWindowToTop, DispatchMessageW, EnumChildWindows,
                GetClassNameW, GetDlgCtrlID, GetDlgItem, GetForegroundWindow, GetMessageW,
                GetWindowRect, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindow,
                IsWindowVisible, KillTimer, PeekMessageW, PostThreadMessageW, SendMessageTimeoutW,
                SetForegroundWindow, SetTimer, SetWindowPos, ShowWindow, TranslateMessage,
                ASFW_ANY, BM_CLICK, CHILDID_SELF, EVENT_OBJECT_DESTROY, EVENT_OBJECT_HIDE,
                EVENT_OBJECT_LOCATIONCHANGE, EVENT_SYSTEM_FOREGROUND, HWND_TOPMOST, MSG,
                OBJID_WINDOW, PM_NOREMOVE, SMTO_ABORTIFHUNG, SWP_NOACTIVATE, SWP_NOOWNERZORDER,
                SWP_SHOWWINDOW, SW_HIDE, WINEVENT_OUTOFCONTEXT, WM_APP, WM_GETTEXT,
                WM_GETTEXTLENGTH, WM_QUIT, WM_SETTEXT, WM_TIMER,
            },
        },
    };

    use super::{
        classify_dialog_kind, panel_placement, request_panel, set_active_dialog, DialogAction,
        DialogHandles, FileDialogActionOutcome, PhysicalRect, HOOK_GENERATION, LOCATION_PENDING,
        PANEL_GAP, PANEL_HEIGHT, PANEL_HWND, TRACKED_HWND,
    };

    const WM_ASSIST_EVENT: u32 = WM_APP + 0x31;
    const EM_SETSEL: u32 = 0x00B1;
    const FILE_NAME_COMBO_ID: i32 = 0x47C;
    const FILE_NAME_EDIT_ID: i32 = 0x480;
    const IDOK: i32 = 1;
    const DETECT_RETRY_INTERVAL_MS: u32 = 120;
    const DETECT_RETRY_LIMIT: u8 = 5;
    const VALIDATE_INTERVAL_MS: u32 = 1000;
    const MESSAGE_TIMEOUT_MS: u32 = 1500;
    const CLICK_TIMEOUT_MS: u32 = 3000;
    const NAVIGATE_WAIT: Duration = Duration::from_millis(900);
    const NAVIGATE_POLL: Duration = Duration::from_millis(40);
    const RESTORE_SETTLE: Duration = Duration::from_millis(150);
    const FOCUS_WAIT: Duration = Duration::from_millis(400);

    unsafe extern "system" fn win_event_proc(
        _hook: HWINEVENTHOOK,
        event: u32,
        hwnd: HWND,
        id_object: i32,
        id_child: i32,
        _event_thread: u32,
        _event_time: u32,
    ) {
        // Only post a message: no window operations or slow work inside the callback.
        if hwnd.is_null() || id_object != OBJID_WINDOW || id_child != CHILDID_SELF as i32 {
            return;
        }
        if event != EVENT_SYSTEM_FOREGROUND {
            if hwnd as isize != TRACKED_HWND.load(Ordering::SeqCst) {
                return;
            }
            if event == EVENT_OBJECT_LOCATIONCHANGE && LOCATION_PENDING.swap(true, Ordering::SeqCst)
            {
                return;
            }
        }
        unsafe {
            PostThreadMessageW(
                GetCurrentThreadId(),
                WM_ASSIST_EVENT,
                event as WPARAM,
                hwnd as LPARAM,
            );
        }
    }

    pub(super) fn post_quit(thread_id: u32) {
        unsafe {
            PostThreadMessageW(thread_id, WM_QUIT, 0, 0);
        }
    }

    struct TrackedDialog {
        handles: DialogHandles,
        hooks: Vec<HWINEVENTHOOK>,
    }

    struct Tracker {
        generation: u64,
        own_pid: u32,
        tracked: Option<TrackedDialog>,
        retry: Option<(usize, isize, u8)>,
        validate_timer: usize,
    }

    pub(super) fn run_hook_thread(generation: u64, ready: mpsc::SyncSender<u32>) {
        unsafe {
            // Creates the thread message queue before the id is published.
            let mut message: MSG = std::mem::zeroed();
            PeekMessageW(&mut message, null_mut(), 0, 0, PM_NOREMOVE);
        }
        let foreground_hook = unsafe {
            SetWinEventHook(
                EVENT_SYSTEM_FOREGROUND,
                EVENT_SYSTEM_FOREGROUND,
                null_mut(),
                Some(win_event_proc),
                0,
                0,
                WINEVENT_OUTOFCONTEXT,
            )
        };
        if foreground_hook.is_null() {
            crate::append_desktop_diagnostic(
                "file_dialog_assist_hook_failed",
                "SetWinEventHook(EVENT_SYSTEM_FOREGROUND) failed",
            );
            let _ = ready.send(0);
            return;
        }
        let _ = ready.send(unsafe { GetCurrentThreadId() });

        let mut tracker = Tracker {
            generation,
            own_pid: std::process::id(),
            tracked: None,
            retry: None,
            validate_timer: 0,
        };
        // Pick up a dialog that is already in the foreground when the assistant is enabled.
        tracker.on_foreground(unsafe { GetForegroundWindow() } as isize, 0);

        unsafe {
            let mut message: MSG = std::mem::zeroed();
            while GetMessageW(&mut message, null_mut(), 0, 0) > 0 {
                match message.message {
                    WM_ASSIST_EVENT => {
                        tracker.on_event(message.wParam as u32, message.lParam as isize)
                    }
                    WM_TIMER if message.hwnd.is_null() => tracker.on_timer(message.wParam),
                    _ => {
                        TranslateMessage(&message);
                        DispatchMessageW(&message);
                    }
                }
            }
            UnhookWinEvent(foreground_hook);
        }
        tracker.untrack();
        tracker.cancel_retry();
        if HOOK_GENERATION.load(Ordering::SeqCst) == generation {
            request_panel(None, None);
        }
    }

    impl Tracker {
        fn is_current(&self) -> bool {
            HOOK_GENERATION.load(Ordering::SeqCst) == self.generation
        }

        fn on_event(&mut self, event: u32, hwnd: isize) {
            match event {
                EVENT_SYSTEM_FOREGROUND => {
                    self.cancel_retry();
                    self.on_foreground(hwnd, 0);
                }
                EVENT_OBJECT_LOCATIONCHANGE => {
                    LOCATION_PENDING.store(false, Ordering::SeqCst);
                    self.refresh(false);
                }
                EVENT_OBJECT_DESTROY | EVENT_OBJECT_HIDE => {
                    if self.tracked_hwnd() == Some(hwnd) {
                        self.untrack();
                        self.publish_hidden();
                    }
                }
                _ => {}
            }
        }

        fn on_timer(&mut self, timer: usize) {
            if timer == self.validate_timer {
                self.refresh(false);
                return;
            }
            if let Some((retry_timer, hwnd, attempt)) = self.retry {
                if retry_timer == timer {
                    self.cancel_retry();
                    if unsafe { GetForegroundWindow() } as isize == hwnd {
                        self.on_foreground(hwnd, attempt);
                    }
                }
            }
        }

        fn on_foreground(&mut self, hwnd: isize, attempt: u8) {
            if hwnd == 0 {
                return;
            }
            let (pid, _) = window_process_thread(hwnd);
            if pid == self.own_pid {
                // Focus moving into the panel keeps it; any other DustDesk window hides it.
                if hwnd != PANEL_HWND.load(Ordering::SeqCst) {
                    self.publish_hidden();
                }
                return;
            }
            if self.tracked_hwnd() == Some(hwnd) {
                self.refresh(true);
                return;
            }
            if let Some(handles) = inspect_file_dialog(hwnd) {
                self.track(handles);
                self.refresh(true);
                return;
            }
            self.publish_hidden();
            // The dialog may still be building its controls when it first becomes foreground.
            if attempt < DETECT_RETRY_LIMIT && class_name(hwnd) == "#32770" {
                let timer = unsafe { SetTimer(null_mut(), 0, DETECT_RETRY_INTERVAL_MS, None) };
                if timer != 0 {
                    self.retry = Some((timer, hwnd, attempt + 1));
                }
            }
        }

        fn tracked_hwnd(&self) -> Option<isize> {
            self.tracked.as_ref().map(|tracked| tracked.handles.hwnd)
        }

        fn track(&mut self, handles: DialogHandles) {
            self.untrack();
            // Location and destroy hooks are scoped to the dialog's own process and thread, never
            // system-wide, so ordinary mouse movement elsewhere does not wake this thread.
            let ranges = [
                (EVENT_OBJECT_DESTROY, EVENT_OBJECT_HIDE),
                (EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_LOCATIONCHANGE),
            ];
            let hooks = ranges
                .iter()
                .filter_map(|(min, max)| {
                    let hook = unsafe {
                        SetWinEventHook(
                            *min,
                            *max,
                            null_mut(),
                            Some(win_event_proc),
                            handles.pid,
                            handles.tid,
                            WINEVENT_OUTOFCONTEXT,
                        )
                    };
                    (!hook.is_null()).then_some(hook)
                })
                .collect::<Vec<_>>();
            TRACKED_HWND.store(handles.hwnd, Ordering::SeqCst);
            LOCATION_PENDING.store(false, Ordering::SeqCst);
            if self.is_current() {
                set_active_dialog(Some(handles));
            }
            self.validate_timer = unsafe { SetTimer(null_mut(), 0, VALIDATE_INTERVAL_MS, None) };
            self.tracked = Some(TrackedDialog { handles, hooks });
        }

        fn untrack(&mut self) {
            let Some(tracked) = self.tracked.take() else {
                return;
            };
            for hook in tracked.hooks {
                unsafe {
                    UnhookWinEvent(hook);
                }
            }
            if self.validate_timer != 0 {
                unsafe {
                    KillTimer(null_mut(), self.validate_timer);
                }
                self.validate_timer = 0;
            }
            if self.is_current() {
                TRACKED_HWND.store(0, Ordering::SeqCst);
                set_active_dialog(None);
            }
        }

        fn cancel_retry(&mut self) {
            if let Some((timer, _, _)) = self.retry.take() {
                unsafe {
                    KillTimer(null_mut(), timer);
                }
            }
        }

        /// Shows or moves the panel for the tracked dialog, or hides it when the dialog is gone,
        /// hidden, minimized, or another program is in the foreground.
        fn refresh(&mut self, foreground_known: bool) {
            let Some(handles) = self.tracked.as_ref().map(|tracked| tracked.handles) else {
                return;
            };
            let hwnd = handles.hwnd as HWND;
            if unsafe { IsWindow(hwnd) } == 0 {
                self.untrack();
                self.publish_hidden();
                return;
            }
            let foreground = unsafe { GetForegroundWindow() } as isize;
            let in_front = foreground_known
                || foreground == handles.hwnd
                || (foreground != 0 && foreground == PANEL_HWND.load(Ordering::SeqCst));
            let usable = unsafe { IsWindowVisible(hwnd) != 0 && IsIconic(hwnd) == 0 };
            match (in_front && usable)
                .then(|| dialog_panel_rect(hwnd))
                .flatten()
            {
                Some(rect) => {
                    if self.is_current() {
                        request_panel(Some(rect), Some(handles));
                    }
                }
                None => self.publish_hidden(),
            }
        }

        fn publish_hidden(&self) {
            if self.is_current() {
                request_panel(None, self.tracked.as_ref().map(|tracked| tracked.handles));
            }
        }
    }

    fn window_process_thread(hwnd: isize) -> (u32, u32) {
        let mut pid = 0;
        let tid = unsafe { GetWindowThreadProcessId(hwnd as HWND, &mut pid) };
        (pid, tid)
    }

    fn class_name(hwnd: isize) -> String {
        let mut buffer = [0u16; 64];
        let len = unsafe { GetClassNameW(hwnd as HWND, buffer.as_mut_ptr(), buffer.len() as i32) };
        String::from_utf16_lossy(&buffer[..len.max(0) as usize])
    }

    fn window_caption(hwnd: isize) -> String {
        let mut buffer = [0u16; 256];
        let len = unsafe { GetWindowTextW(hwnd as HWND, buffer.as_mut_ptr(), buffer.len() as i32) };
        String::from_utf16_lossy(&buffer[..len.max(0) as usize])
    }

    struct ChildSearch {
        combo: isize,
        edit_1152: isize,
        ok: isize,
    }

    unsafe extern "system" fn collect_dialog_child(hwnd: HWND, lparam: LPARAM) -> i32 {
        let search = unsafe { &mut *(lparam as *mut ChildSearch) };
        let id = unsafe { GetDlgCtrlID(hwnd) };
        let class = class_name(hwnd as isize);
        if search.combo == 0
            && id == FILE_NAME_COMBO_ID
            && (class == "ComboBoxEx32" || class == "ComboBox")
        {
            search.combo = hwnd as isize;
        } else if search.edit_1152 == 0 && id == FILE_NAME_EDIT_ID && class == "Edit" {
            search.edit_1152 = hwnd as isize;
        } else if search.ok == 0 && id == IDOK && class == "Button" {
            search.ok = hwnd as isize;
        }
        1
    }

    unsafe extern "system" fn find_edit_child(hwnd: HWND, lparam: LPARAM) -> i32 {
        if class_name(hwnd as isize) == "Edit" {
            unsafe {
                *(lparam as *mut isize) = hwnd as isize;
            }
            return 0;
        }
        1
    }

    /// Recognises a standard open/save/folder dialog: a `#32770` top-level window with a file
    /// name edit (inside the 1148 combo box, or the 1152 edit) and an IDOK button.
    fn inspect_file_dialog(hwnd: isize) -> Option<DialogHandles> {
        if class_name(hwnd) != "#32770" || unsafe { IsWindowVisible(hwnd as HWND) } == 0 {
            return None;
        }
        let mut search = ChildSearch {
            combo: 0,
            edit_1152: 0,
            ok: 0,
        };
        unsafe {
            EnumChildWindows(
                hwnd as HWND,
                Some(collect_dialog_child),
                &mut search as *mut ChildSearch as LPARAM,
            );
        }
        let mut edit = 0isize;
        if search.combo != 0 {
            unsafe {
                EnumChildWindows(
                    search.combo as HWND,
                    Some(find_edit_child),
                    &mut edit as *mut isize as LPARAM,
                );
            }
        }
        if edit == 0 {
            edit = search.edit_1152;
        }
        let direct_ok = unsafe { GetDlgItem(hwnd as HWND, IDOK) } as isize;
        let ok = if direct_ok != 0 { direct_ok } else { search.ok };
        if edit == 0 || ok == 0 {
            return None;
        }
        let (pid, tid) = window_process_thread(hwnd);
        Some(DialogHandles {
            hwnd,
            edit,
            ok,
            kind: classify_dialog_kind(&window_caption(ok), &window_caption(hwnd)),
            pid,
            tid,
        })
    }

    fn dialog_panel_rect(hwnd: HWND) -> Option<PhysicalRect> {
        let mut frame = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        let frame_ok = unsafe {
            DwmGetWindowAttribute(
                hwnd,
                DWMWA_EXTENDED_FRAME_BOUNDS as u32,
                &mut frame as *mut RECT as *mut c_void,
                std::mem::size_of::<RECT>() as u32,
            )
        } >= 0;
        if !frame_ok && unsafe { GetWindowRect(hwnd, &mut frame) } == 0 {
            return None;
        }
        let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
        let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if monitor.is_null() || unsafe { GetMonitorInfoW(monitor, &mut info) } == 0 {
            return None;
        }
        let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
        if unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) } < 0 {
            dpi_x = 96;
        }
        let scale = f64::from(dpi_x.max(1)) / 96.0;
        let work = info.rcWork;
        Some(panel_placement(
            PhysicalRect {
                left: frame.left,
                top: frame.top,
                right: frame.right,
                bottom: frame.bottom,
            },
            PhysicalRect {
                left: work.left,
                top: work.top,
                right: work.right,
                bottom: work.bottom,
            },
            (PANEL_HEIGHT * scale).round() as i32,
            (PANEL_GAP * scale).round() as i32,
        ))
    }

    pub(super) fn show_panel(hwnd: isize, rect: PhysicalRect) {
        if hwnd == 0 {
            return;
        }
        let place = || unsafe {
            SetWindowPos(
                hwnd as HWND,
                HWND_TOPMOST,
                rect.left,
                rect.top,
                rect.width(),
                rect.height(),
                SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_SHOWWINDOW,
            );
        };
        place();
        // Crossing to a monitor with another scale makes WebView2 resize itself on
        // WM_DPICHANGED; a second pass restores the physical rectangle we asked for.
        let mut actual = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        if unsafe { GetWindowRect(hwnd as HWND, &mut actual) } != 0
            && (actual.left, actual.top, actual.right, actual.bottom)
                != (rect.left, rect.top, rect.right, rect.bottom)
        {
            place();
        }
    }

    pub(super) fn hide_panel(hwnd: isize) {
        if hwnd != 0 {
            unsafe {
                ShowWindow(hwnd as HWND, SW_HIDE);
            }
        }
    }

    pub(super) fn activate_window(hwnd: isize) {
        let hwnd = hwnd as HWND;
        if unsafe { IsWindow(hwnd) } == 0 {
            return;
        }
        let start = Instant::now();
        loop {
            unsafe {
                AllowSetForegroundWindow(ASFW_ANY);
                let current_thread = GetCurrentThreadId();
                let foreground = GetForegroundWindow();
                let foreground_thread = if foreground.is_null() {
                    0
                } else {
                    GetWindowThreadProcessId(foreground, null_mut())
                };
                let target_thread = GetWindowThreadProcessId(hwnd, null_mut());
                let attach_foreground =
                    foreground_thread != 0 && foreground_thread != current_thread;
                let attach_target = target_thread != 0 && target_thread != current_thread;
                if attach_foreground {
                    AttachThreadInput(current_thread, foreground_thread, 1);
                }
                if attach_target {
                    AttachThreadInput(current_thread, target_thread, 1);
                }
                BringWindowToTop(hwnd);
                SetForegroundWindow(hwnd);
                SetActiveWindow(hwnd);
                SetFocus(hwnd);
                if attach_target {
                    AttachThreadInput(current_thread, target_thread, 0);
                }
                if attach_foreground {
                    AttachThreadInput(current_thread, foreground_thread, 0);
                }
            }
            if unsafe { GetForegroundWindow() } == hwnd || start.elapsed() >= FOCUS_WAIT {
                return;
            }
            thread::sleep(Duration::from_millis(30));
        }
    }

    enum MessageError {
        Elevated,
        Failed(String),
    }

    fn send_message(
        hwnd: isize,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        timeout: u32,
    ) -> Result<usize, MessageError> {
        let mut result = 0usize;
        let ok = unsafe {
            SendMessageTimeoutW(
                hwnd as HWND,
                message,
                wparam,
                lparam,
                SMTO_ABORTIFHUNG,
                timeout,
                &mut result,
            )
        };
        if ok != 0 {
            return Ok(result);
        }
        // UIPI blocks messages to higher-integrity processes with ERROR_ACCESS_DENIED.
        if unsafe { GetLastError() } == ERROR_ACCESS_DENIED {
            Err(MessageError::Elevated)
        } else {
            Err(MessageError::Failed("文件对话框没有响应".to_owned()))
        }
    }

    fn get_text(hwnd: isize) -> Result<String, MessageError> {
        let len = send_message(hwnd, WM_GETTEXTLENGTH, 0, 0, MESSAGE_TIMEOUT_MS)?;
        let mut buffer = vec![0u16; len + 1];
        let copied = send_message(
            hwnd,
            WM_GETTEXT,
            buffer.len(),
            buffer.as_mut_ptr() as LPARAM,
            MESSAGE_TIMEOUT_MS,
        )?;
        Ok(String::from_utf16_lossy(&buffer[..copied.min(len)]))
    }

    fn set_text(hwnd: isize, text: &str) -> Result<(), MessageError> {
        let wide = text.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
        send_message(
            hwnd,
            WM_SETTEXT,
            0,
            wide.as_ptr() as LPARAM,
            MESSAGE_TIMEOUT_MS,
        )?;
        match get_text(hwnd)? {
            current if current == text => Ok(()),
            _ => Err(MessageError::Failed("无法写入文件名框".to_owned())),
        }
    }

    fn click(hwnd: isize) -> Result<(), MessageError> {
        match send_message(hwnd, BM_CLICK, 0, 0, CLICK_TIMEOUT_MS) {
            // A slow navigation can outlast the timeout after the click was delivered.
            Err(MessageError::Failed(_)) => Ok(()),
            other => other.map(|_| ()),
        }
    }

    fn folder_input(folder: &std::path::Path) -> String {
        // The trailing separator makes the dialog treat the input as a folder, never a file name.
        let mut text = folder.display().to_string();
        if !text.ends_with('\\') {
            text.push('\\');
        }
        text
    }

    fn navigate(
        dialog: &DialogHandles,
        folder: &std::path::Path,
        restore: &str,
    ) -> Result<(), MessageError> {
        let target = folder_input(folder);
        set_text(dialog.edit, &target)?;
        click(dialog.ok)?;
        let start = Instant::now();
        while start.elapsed() < NAVIGATE_WAIT {
            match get_text(dialog.edit) {
                Ok(current) if current != target => break,
                Ok(_) => thread::sleep(NAVIGATE_POLL),
                Err(_) => break,
            }
        }
        if unsafe { IsWindow(dialog.edit as HWND) } == 0 {
            return Ok(());
        }
        set_text(dialog.edit, restore)?;
        // Some dialogs reset the box once more after the folder view finishes loading.
        thread::sleep(RESTORE_SETTLE);
        if get_text(dialog.edit).is_ok_and(|current| current != restore) {
            set_text(dialog.edit, restore)?;
        }
        let _ = send_message(
            dialog.edit,
            EM_SETSEL,
            0,
            -1isize as LPARAM,
            MESSAGE_TIMEOUT_MS,
        );
        Ok(())
    }

    pub(super) fn perform_action(
        dialog: &DialogHandles,
        action: &DialogAction,
    ) -> Result<FileDialogActionOutcome, String> {
        if unsafe { IsWindow(dialog.hwnd as HWND) } == 0
            || unsafe { IsWindow(dialog.edit as HWND) } == 0
        {
            return Err("文件对话框已关闭".to_owned());
        }
        // Read the original name first: it is restored after navigating, and a UIPI block shows
        // up here before anything is changed in the dialog.
        let original = match get_text(dialog.edit) {
            Ok(text) => text,
            Err(MessageError::Elevated) => return Ok(FileDialogActionOutcome::Elevated),
            Err(MessageError::Failed(error)) => return Err(error),
        };
        // DustDesk is in the foreground here, so it may hand activation to the dialog; an active
        // dialog accepts BM_CLICK and gives activation back to its owner when it closes.
        activate_window(dialog.hwnd);
        let result = match action {
            DialogAction::Navigate(folder) => navigate(dialog, folder, &original),
            DialogAction::Locate { folder, file_name } => navigate(dialog, folder, file_name),
            DialogAction::Select(file) => {
                set_text(dialog.edit, &file.display().to_string()).and_then(|_| click(dialog.ok))
            }
        };
        match result {
            Ok(()) => Ok(FileDialogActionOutcome::Done),
            Err(MessageError::Elevated) => Ok(FileDialogActionOutcome::Elevated),
            Err(MessageError::Failed(error)) => Err(error),
        }
    }
}

#[cfg(not(windows))]
mod platform {
    use std::sync::mpsc;

    use super::{DialogAction, DialogHandles, FileDialogActionOutcome, PhysicalRect};

    pub(super) fn run_hook_thread(_generation: u64, ready: mpsc::SyncSender<u32>) {
        let _ = ready.send(0);
    }

    pub(super) fn post_quit(_thread_id: u32) {}

    pub(super) fn show_panel(_hwnd: isize, _rect: PhysicalRect) {}

    pub(super) fn hide_panel(_hwnd: isize) {}

    pub(super) fn activate_window(_hwnd: isize) {}

    pub(super) fn perform_action(
        _dialog: &DialogHandles,
        _action: &DialogAction,
    ) -> Result<FileDialogActionOutcome, String> {
        Err("仅支持 Windows".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> PhysicalRect {
        PhysicalRect {
            left,
            top,
            right,
            bottom,
        }
    }

    const WORK: PhysicalRect = PhysicalRect {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1040,
    };

    #[test]
    fn panel_docks_below_the_dialog_with_its_width() {
        let placed = panel_placement(rect(400, 100, 1200, 600), WORK, 300, 4);
        assert_eq!(placed, rect(400, 604, 1200, 904));
    }

    #[test]
    fn panel_moves_above_when_there_is_no_room_below() {
        let placed = panel_placement(rect(400, 500, 1200, 900), WORK, 300, 4);
        assert_eq!(placed, rect(400, 196, 1200, 496));
    }

    #[test]
    fn panel_falls_back_to_the_work_area_bottom() {
        let placed = panel_placement(rect(400, 100, 1200, 900), WORK, 300, 4);
        assert_eq!(placed, rect(400, 740, 1200, 1040));
    }

    #[test]
    fn panel_placement_works_on_a_secondary_monitor_with_negative_origin() {
        let work = rect(-2560, -200, 0, 1240);
        let placed = panel_placement(rect(-2000, 0, -800, 700), work, 450, 6);
        assert_eq!(placed, rect(-2000, 706, -800, 1156));
    }

    #[test]
    fn panel_is_kept_inside_the_work_area_horizontally() {
        let placed = panel_placement(rect(1500, 100, 2300, 600), WORK, 300, 4);
        assert_eq!(placed, rect(1120, 604, 1920, 904));
        let wide = panel_placement(rect(-100, 100, 2100, 600), WORK, 300, 4);
        assert_eq!(wide, rect(0, 604, 1920, 904));
    }

    #[test]
    fn dialog_kind_follows_the_ok_button_caption() {
        assert_eq!(classify_dialog_kind("打开(&O)", "打开"), DialogKind::Open);
        assert_eq!(classify_dialog_kind("&Open", ""), DialogKind::Open);
        assert_eq!(classify_dialog_kind("保存(&S)", "另存为"), DialogKind::Save);
        assert_eq!(classify_dialog_kind("&Save", "Save As"), DialogKind::Save);
        assert_eq!(
            classify_dialog_kind("选择文件夹", "选择文件夹"),
            DialogKind::Folder
        );
        assert_eq!(
            classify_dialog_kind("Select Folder", ""),
            DialogKind::Folder
        );
    }

    #[test]
    fn unknown_dialog_kind_uses_the_title_then_defaults_to_open() {
        assert_eq!(classify_dialog_kind("确定", "另存为"), DialogKind::Save);
        assert_eq!(classify_dialog_kind("确定", "导入"), DialogKind::Open);
        assert_eq!(classify_dialog_kind("", ""), DialogKind::Open);
    }

    #[test]
    fn substring_match_is_case_insensitive_and_tiered() {
        let exact = match_name("Report.docx", "report").unwrap();
        assert_eq!(exact.tier, MatchTier::Exact);
        let prefix = match_name("Report-2024.docx", "rep").unwrap();
        assert_eq!(prefix.tier, MatchTier::Prefix);
        assert_eq!(prefix.highlight, vec![0, 1, 2]);
        let substring = match_name("年度Report.docx", "port").unwrap();
        assert_eq!(substring.tier, MatchTier::Substring);
        assert_eq!(substring.highlight, vec![4, 5, 6, 7]);
        assert!(match_name("notes.txt", "xyz").is_none());
    }

    #[test]
    fn pinyin_initials_match_chinese_names() {
        let matched = match_name("年度报告.docx", "ndbg").unwrap();
        assert_eq!(matched.tier, MatchTier::Exact);
        assert_eq!(matched.highlight, vec![0, 1, 2, 3]);
        let prefix = match_name("年度 报告 2024.pdf", "ndbg").unwrap();
        assert_eq!(prefix.tier, MatchTier::Prefix);
        assert_eq!(prefix.highlight, vec![0, 1, 3, 4]);
        let substring = match_name("公司年度报告.xlsx", "bg").unwrap();
        assert_eq!(substring.tier, MatchTier::Substring);
        assert!(match_name("年度报告.docx", "nbd").is_none());
    }

    #[test]
    fn pinyin_initials_accept_heteronyms() {
        // 「行」 reads xing or hang.
        assert!(match_name("银行流水.pdf", "yhls").is_some());
        assert!(match_name("行程单.pdf", "xcd").is_some());
    }

    #[test]
    fn results_sort_by_tier_then_newest_first() {
        let entry = |name: &str, tier: MatchTier, modified: i64| FileDialogEntry {
            name: name.to_owned(),
            path: name.to_owned(),
            parent: String::new(),
            is_dir: false,
            extension: "TXT".to_owned(),
            modified_ms: Some(modified),
            highlight: Vec::new(),
            tier: Some(tier),
        };
        let mut entries = vec![
            entry("old-substring", MatchTier::Substring, 1),
            entry("new-substring", MatchTier::Substring, 9),
            entry("prefix", MatchTier::Prefix, 2),
            entry("exact", MatchTier::Exact, 0),
        ];
        sort_search_results(&mut entries);
        let names = entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec!["exact", "prefix", "new-substring", "old-substring"]
        );
    }

    #[test]
    fn save_dialogs_never_confirm_a_file() {
        let file = Path::new(r"D:\Organizer\文档\报告.docx");
        let folder = PathBuf::from(r"D:\Organizer\文档");
        assert_eq!(
            plan_dialog_action(DialogKind::Save, DialogActionMode::Auto, file, false),
            Some(DialogAction::Navigate(folder.clone()))
        );
        assert_eq!(
            plan_dialog_action(DialogKind::Folder, DialogActionMode::Auto, file, false),
            Some(DialogAction::Navigate(folder.clone()))
        );
        assert_eq!(
            plan_dialog_action(DialogKind::Open, DialogActionMode::Auto, file, false),
            Some(DialogAction::Select(file.to_path_buf()))
        );
        assert_eq!(
            plan_dialog_action(DialogKind::Open, DialogActionMode::Locate, file, false),
            Some(DialogAction::Locate {
                folder,
                file_name: "报告.docx".to_owned()
            })
        );
        assert_eq!(
            plan_dialog_action(DialogKind::Save, DialogActionMode::Locate, file, false),
            Some(DialogAction::Navigate(PathBuf::from(r"D:\Organizer\文档")))
        );
        assert_eq!(
            plan_dialog_action(
                DialogKind::Save,
                DialogActionMode::Auto,
                Path::new(r"D:\Organizer"),
                true
            ),
            Some(DialogAction::Navigate(PathBuf::from(r"D:\Organizer")))
        );
    }

    #[test]
    fn internal_transaction_entries_are_filtered() {
        assert!(is_internal_entry_name(".dustdesk-transfer-s1-0.tmp"));
        assert!(is_internal_entry_name(".DustDesk-source-transfer-1.tmp"));
        assert!(!is_internal_entry_name("dustdesk notes.txt"));
    }
}
