#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod clipboard_bridge;
mod models;
mod store;
mod system_icon;
#[cfg(windows)]
mod windows_show_desktop;

use std::{
    cmp::Ordering as CmpOrdering,
    collections::{BTreeMap, BTreeSet, VecDeque},
    env,
    ffi::OsString,
    fs::{self, File},
    io::{self, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc, Arc, Condvar, Mutex, OnceLock,
    },
    time::{Duration, Instant, SystemTime},
};

#[cfg(windows)]
use std::os::windows::{
    ffi::{OsStrExt, OsStringExt},
    fs::OpenOptionsExt,
    io::AsRawHandle,
    process::CommandExt,
};

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use models::{
    AppConfig, AppSettings, AppSnapshot, CategoryClassifyCount, ClassifyResult,
    ClipboardHistoryItem, DeskCategory, DesktopIconPosition, DesktopItem, DesktopWindowLayout,
    LaunchItem, PathIconResult, SearchHistoryData, SearchHistoryItem, SearchItem, SearchItemKind,
    SearchOverlayData, MAX_CLIPBOARD_HISTORY_LIMIT, MIN_CLIPBOARD_HISTORY_LIMIT,
};
use serde::{Deserialize, Serialize};
use store::AppStore;
use tauri::{
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    utils::config::Color,
    Emitter, Manager, Monitor, PhysicalPosition, PhysicalSize, Position, Size, WebviewUrl,
    WebviewWindow, WebviewWindowBuilder,
};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

const SEARCH_RESULT_LIMIT: usize = 80;
const SEARCH_SCAN_LIMIT: usize = 8_000;
const SEARCH_HISTORY_LIMIT: usize = 500;
const CLIPBOARD_PREVIEW_IMAGE_MAX_BYTES: u64 = 800_000;
const MAX_LOCKED_TRANSFER_TREE_ENTRIES: usize = 256;
const ICON_DATA_CACHE_LIMIT: usize = 1024;
const ICON_FAILURE_CACHE_LIMIT: usize = 1024;
const ICON_FAILURE_CACHE_TTL: Duration = Duration::from_millis(750);
#[cfg(not(test))]
const DIAGNOSTIC_LOG_MAX_BYTES: u64 = 5 * 1024 * 1024;
const TRAY_MENU_SHOW_MAIN: &str = "show-main-window";
const TRAY_MENU_DESKTOP_VISIBLE: &str = "desktop-frames-visible";
const TRAY_MENU_DESKTOP_LOCKED: &str = "desktop-frames-locked";
const TRAY_MENU_DESKTOP_CLICK_THROUGH: &str = "desktop-frames-click-through";
const TRAY_MENU_QUIT: &str = "quit-app";
const DESKTOP_OPERATION_EVENT: &str = "dustdesk://desktop-operation";
const CATEGORY_ORDER_CHANGED_EVENT: &str = "dustdesk://category-order-changed";
const DESKTOP_WINDOW_STATE_CHANGED_EVENT: &str = "dustdesk://desktop-window-state-changed";
const DESKTOP_ORGANIZATION_RESTART_SCHEMA_VERSION: u32 = 1;
const UPDATE_RELEASE_API_URL: &str =
    "https://api.github.com/repos/LAIJiangFeng/tauri-dustdesk/releases/latest";
const UPDATE_RELEASES_URL_PREFIX: &str = "https://github.com/LAIJiangFeng/tauri-dustdesk/releases";
const UPDATE_LATEST_INSTALLER_URL: &str = "https://github.com/LAIJiangFeng/tauri-dustdesk/releases/latest/download/DustDesk-latest-windows-x64-setup.exe";

static CONFIG_MUTATION_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static LAUNCHER_MUTATION_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static RUNTIME_DIRECTORY_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static TRANSFER_RECOVERY_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static LAZY_WINDOW_OPERATION_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static WEBVIEW_WINDOW_CREATION_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static ICON_DATA_URL_CACHE: OnceLock<Mutex<BTreeMap<String, String>>> = OnceLock::new();
static ICON_FAILURE_CACHE: OnceLock<Mutex<BTreeMap<String, Instant>>> = OnceLock::new();
static ICON_RESOLUTION_LOCKS: OnceLock<Mutex<BTreeMap<String, Arc<Mutex<()>>>>> = OnceLock::new();
#[cfg(not(test))]
static DIAGNOSTIC_LOG_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static STARTUP_RECOVERY_STATE: OnceLock<(Mutex<Option<Result<(), String>>>, Condvar)> =
    OnceLock::new();
static REAL_EXIT_REQUESTED: AtomicBool = AtomicBool::new(false);
static EXIT_RESTORE_COMPLETED: AtomicBool = AtomicBool::new(false);
static DESKTOP_OPERATION_RUNNING: AtomicBool = AtomicBool::new(false);
static DESKTOP_WINDOW_SETTLE_SCHEDULED: AtomicBool = AtomicBool::new(false);
static DESKTOP_MONITOR_WATCH_STARTED: AtomicBool = AtomicBool::new(false);
static DESKTOP_WINDOW_SETTLE_GENERATION: AtomicU64 = AtomicU64::new(0);
static DESKTOP_OPERATION_LAST: OnceLock<Mutex<Option<DesktopOperationPayload>>> = OnceLock::new();
static INTERNAL_PATH_DROP_ACCEPTANCES: OnceLock<Mutex<Vec<InternalPathDropAcceptance>>> =
    OnceLock::new();
static DESKTOP_DROP_CATEGORY_TARGETS: OnceLock<Mutex<BTreeMap<String, usize>>> = OnceLock::new();
static TRAY_MENU_ITEMS: OnceLock<TrayMenuItems> = OnceLock::new();

#[derive(Clone)]
struct TrayMenuItems {
    desktop_visible: CheckMenuItem<tauri::Wry>,
    desktop_locked: CheckMenuItem<tauri::Wry>,
    desktop_click_through: CheckMenuItem<tauri::Wry>,
}

#[derive(Debug, Clone, Copy, Serialize)]
struct DesktopFrameVisibility {
    organizer: bool,
    launcher: bool,
    any: bool,
}

#[derive(Debug, Clone, Copy, Serialize)]
struct DesktopWindowState {
    locked: bool,
    click_through: bool,
}

#[derive(Debug, Clone, PartialEq)]
struct DesktopMonitorWorkArea {
    name: Option<String>,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    scale_factor: f64,
}

struct DesktopMonitorTarget {
    monitor: Monitor,
    work_area: DesktopMonitorWorkArea,
    stable_id: String,
    is_primary: bool,
}

impl DesktopMonitorWorkArea {
    fn from_monitor(monitor: &Monitor) -> Self {
        let work_area = monitor.work_area();
        Self {
            name: monitor.name().cloned(),
            x: work_area.position.x,
            y: work_area.position.y,
            width: work_area.size.width,
            height: work_area.size.height,
            scale_factor: valid_scale_factor(monitor.scale_factor()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DesktopWindowPlacement {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

#[derive(Debug, Clone, Copy, Serialize)]
struct CategoryOrderChangedPayload {
    from_index: usize,
    to_index: usize,
}

#[derive(Debug, Clone, Serialize)]
struct DesktopOperationPayload {
    kind: &'static str,
    scope: &'static str,
    status: &'static str,
    message: String,
    moved: usize,
    skipped: usize,
    restored: usize,
    total: usize,
    current_path: String,
    category_counts: Vec<CategoryClassifyCount>,
}

#[derive(Debug, Clone, Serialize)]
struct DesktopOperationStatus {
    running: bool,
    last: Option<DesktopOperationPayload>,
}

#[derive(Debug, Clone, Serialize)]
struct AppUpdateInfo {
    current_version: String,
    latest_version: String,
    update_available: bool,
    release_name: String,
    release_url: String,
    download_url: String,
    asset_name: String,
    published_at: String,
    body: String,
}

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    name: Option<String>,
    html_url: String,
    published_at: Option<String>,
    body: Option<String>,
    assets: Vec<GitHubReleaseAsset>,
}

#[derive(Debug, Deserialize)]
struct GitHubReleaseAsset {
    name: String,
    browser_download_url: String,
}

#[derive(Debug, Clone)]
struct ClassifyCandidate {
    original_path: String,
    category_index: usize,
    category_name: String,
    is_dir: bool,
    work_estimate: u64,
    desktop_icon_position: Option<DesktopIconPosition>,
}

#[derive(Debug)]
enum ClassifyWorkerMessage {
    Started(ClassifyCandidate),
    Finished(ClassifyCandidate, Result<String, String>, Duration),
    Failed(String),
}

#[derive(Debug, Clone)]
struct RestoreCandidate {
    source: PathBuf,
    original_config_path: Option<String>,
    category_index: Option<usize>,
    desktop_icon_position: Option<DesktopIconPosition>,
}

#[derive(Debug, Default)]
struct ExitDesktopRestoreOutcome {
    restored: usize,
    icon_positions: Vec<(PathBuf, DesktopIconPosition)>,
    restore_errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeMigrationJournal {
    target: String,
    phase: String,
    requested_path: String,
    old_data_dir: String,
    old_organizer_root: String,
    old_launchers_root: String,
    new_data_dir: String,
    new_organizer_root: String,
    new_launchers_root: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct DesktopOrganizationRestartMarker {
    schema_version: u32,
    categories: Vec<RestartCategoryMarker>,
}

impl DesktopOrganizationRestartMarker {
    fn empty() -> Self {
        Self {
            schema_version: DESKTOP_ORGANIZATION_RESTART_SCHEMA_VERSION,
            categories: Vec::new(),
        }
    }

    fn is_empty(&self) -> bool {
        self.categories
            .iter()
            .all(|category| category.items.is_empty())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct RestartCategoryMarker {
    category_name: String,
    #[serde(default)]
    category_index: Option<usize>,
    items: Vec<RestartItemMarker>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct RestartItemMarker {
    source_path: String,
    desktop_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    desktop_icon_position: Option<DesktopIconPosition>,
}

trait DesktopOrganizationRestartPersistence {
    fn load(&self) -> Result<Option<DesktopOrganizationRestartMarker>, String>;
    fn save(&self, marker: &DesktopOrganizationRestartMarker) -> Result<(), String>;
    fn remove(&self) -> Result<(), String>;
}

struct AppDesktopOrganizationRestartPersistence;

impl DesktopOrganizationRestartPersistence for AppDesktopOrganizationRestartPersistence {
    fn load(&self) -> Result<Option<DesktopOrganizationRestartMarker>, String> {
        let Some(content) = AppStore::load_desktop_organization_restart().map_err(to_message)?
        else {
            return Ok(None);
        };
        serde_json::from_str(&content).map(Some).map_err(to_message)
    }

    fn save(&self, marker: &DesktopOrganizationRestartMarker) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(marker).map_err(to_message)?;
        AppStore::save_desktop_organization_restart(&bytes).map_err(to_message)
    }

    fn remove(&self) -> Result<(), String> {
        AppStore::remove_desktop_organization_restart().map_err(to_message)
    }
}

fn clear_desktop_organization_restart_marker(
    persistence: &impl DesktopOrganizationRestartPersistence,
) -> Result<(), String> {
    // Persist a tombstone before deletion. If power is lost before the directory entry deletion is
    // durable, startup can only observe this empty marker, never the previous recollection intent.
    persistence.save(&DesktopOrganizationRestartMarker::empty())?;
    persistence.remove()
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DesktopDropPosition {
    screen_x: f64,
    screen_y: f64,
    scale_factor: Option<f64>,
}

#[derive(Debug)]
struct InternalPathDropAcceptance {
    path: String,
    session_id: Option<String>,
    accepted_at: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InternalPathDropTarget {
    Category(usize),
    Launcher,
    Box,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct InternalPathDragOutcome {
    action: String,
    target_category_index: Option<usize>,
    affected: usize,
    restored_path: Option<String>,
}

fn wait_for_startup_recovery() -> Result<(), String> {
    let (state, ready) = STARTUP_RECOVERY_STATE.get_or_init(|| (Mutex::new(None), Condvar::new()));
    let mut result = state
        .lock()
        .map_err(|_| "启动恢复状态锁已损坏".to_owned())?;
    while result.is_none() {
        result = ready
            .wait(result)
            .map_err(|_| "启动恢复状态锁已损坏".to_owned())?;
    }
    result
        .as_ref()
        .expect("startup recovery result checked")
        .clone()
}

fn mark_startup_recovery_complete(result: Result<(), String>) {
    let (state, ready) = STARTUP_RECOVERY_STATE.get_or_init(|| (Mutex::new(None), Condvar::new()));
    if let Ok(mut stored_result) = state.lock() {
        *stored_result = Some(result);
        ready.notify_all();
    }
}

struct StartupRecoveryCompletionGuard {
    completed: bool,
}

impl StartupRecoveryCompletionGuard {
    fn new() -> Self {
        Self { completed: false }
    }

    fn complete(&mut self, result: Result<(), String>) {
        mark_startup_recovery_complete(result);
        self.completed = true;
    }
}

impl Drop for StartupRecoveryCompletionGuard {
    fn drop(&mut self) {
        if !self.completed {
            mark_startup_recovery_complete(Err(
                "启动恢复任务意外中断，请重新启动 DustDesk".to_owned()
            ));
        }
    }
}

#[tauri::command]
async fn load_snapshot() -> Result<AppSnapshot, String> {
    tauri::async_runtime::spawn_blocking(load_snapshot_impl)
        .await
        .map_err(to_message)?
}

fn load_snapshot_impl() -> Result<AppSnapshot, String> {
    wait_for_startup_recovery()?;
    let store = AppStore::open().map_err(to_message)?;
    store.ensure_runtime_dirs().map_err(to_message)?;
    let config = with_config_mutation(|| {
        let mut config = store.load_config_strict().map_err(to_message)?;
        repair_category_item_paths(&store, &mut config)?;
        Ok(config)
    })?;
    let launchers = with_launcher_mutation(|| {
        let mut launchers = store.load_launchers_strict().map_err(to_message)?;
        repair_launchers(&store, &mut launchers)?;
        Ok(launchers)
    })?;
    let clipboard_history_limit = config.settings.clipboard_history_limit_value();
    let clipboard = clipboard_bridge::with_clipboard_history_lock(|| {
        let mut clipboard = store.load_clipboard_strict().map_err(to_message)?;
        let images_changed =
            clipboard_bridge::normalize_clipboard_image_storage(&mut clipboard).unwrap_or(false);
        let history_changed =
            clipboard_bridge::truncate_history(&mut clipboard, clipboard_history_limit);
        if images_changed || history_changed {
            store.save_clipboard(&clipboard).map_err(to_message)?;
        }
        Ok(clipboard)
    })?;

    let desktop_layout = config.desktop_layout.clone();

    Ok(AppSnapshot {
        data_dir: store.data_dir().display().to_string(),
        organizer_root: store.organizer_root().display().to_string(),
        launchers_root: store.launchers_root().display().to_string(),
        settings: config.settings,
        desktop_layout,
        categories: categories_with_item_details(config.desktop_categories, false),
        desktop_items: desktop_items(false),
        launchers: launchers_with_optional_icons(launchers.items, false),
        clipboard: clipboard_preview(clipboard.items, clipboard_history_limit),
    })
}

#[tauri::command]
async fn load_desktop_snapshot() -> Result<AppSnapshot, String> {
    tauri::async_runtime::spawn_blocking(load_desktop_snapshot_impl)
        .await
        .map_err(to_message)?
}

fn load_desktop_snapshot_impl() -> Result<AppSnapshot, String> {
    wait_for_startup_recovery()?;
    let store = AppStore::open().map_err(to_message)?;
    store.ensure_runtime_dirs().map_err(to_message)?;
    let config = with_config_mutation(|| {
        let mut config = store.load_config_strict().map_err(to_message)?;
        repair_category_item_paths(&store, &mut config)?;
        Ok(config)
    })?;
    let launchers = with_launcher_mutation(|| {
        let mut launchers = store.load_launchers_strict().map_err(to_message)?;
        repair_launchers(&store, &mut launchers)?;
        Ok(launchers)
    })?;

    let desktop_layout = config.desktop_layout.clone();

    Ok(AppSnapshot {
        data_dir: store.data_dir().display().to_string(),
        organizer_root: store.organizer_root().display().to_string(),
        launchers_root: store.launchers_root().display().to_string(),
        settings: config.settings,
        desktop_layout,
        categories: categories_with_item_details(config.desktop_categories, false),
        desktop_items: Vec::new(),
        launchers: launchers_with_optional_icons(launchers.items, false),
        clipboard: Vec::new(),
    })
}

#[tauri::command]
async fn resolve_path_icons(paths: Vec<String>) -> Result<Vec<PathIconResult>, String> {
    tauri::async_runtime::spawn_blocking(move || resolve_path_icons_impl(paths))
        .await
        .map_err(to_message)?
}

#[tauri::command]
fn create_category(app: tauri::AppHandle, name: String) -> Result<(), String> {
    mutate_categories(|categories| {
        categories.push(DeskCategory {
            name: normalize_name(&name, categories.len() + 1),
            ..DeskCategory::default()
        });
        Ok(())
    })?;
    emit_desktop_cards_changed(&app);
    Ok(())
}

#[tauri::command]
fn rename_category(app: tauri::AppHandle, index: usize, name: String) -> Result<(), String> {
    mutate_categories(|categories| {
        let fallback = index + 1;
        let category = categories
            .get_mut(index)
            .ok_or_else(|| "分类不存在".to_owned())?;
        category.name = normalize_name(&name, fallback);
        Ok(())
    })?;
    emit_desktop_cards_changed(&app);
    Ok(())
}

#[tauri::command]
fn delete_category(app: tauri::AppHandle, index: usize) -> Result<(), String> {
    mutate_categories(|categories| {
        if categories.len() <= 1 {
            return Err("至少保留一个分类".to_owned());
        }
        let category = categories
            .get(index)
            .ok_or_else(|| "分类不存在".to_owned())?;
        if !category.item_paths.is_empty() {
            return Err("只能删除空分类".to_owned());
        }
        categories.remove(index);
        Ok(())
    })?;
    emit_desktop_cards_changed(&app);
    Ok(())
}

#[tauri::command]
fn toggle_category(app: tauri::AppHandle, index: usize) -> Result<(), String> {
    mutate_categories(|categories| {
        let category = categories
            .get_mut(index)
            .ok_or_else(|| "分类不存在".to_owned())?;
        category.is_collapsed = !category.is_collapsed;
        Ok(())
    })?;
    emit_desktop_cards_changed(&app);
    Ok(())
}

#[tauri::command]
fn set_category_sort_by_name(
    app: tauri::AppHandle,
    index: usize,
    enabled: bool,
) -> Result<(), String> {
    mutate_categories(|categories| {
        let category = categories
            .get_mut(index)
            .ok_or_else(|| "分类不存在".to_owned())?;
        category.sort_by_name = enabled;
        Ok(())
    })?;
    emit_desktop_cards_changed(&app);
    Ok(())
}

#[tauri::command]
async fn reorder_category(
    app: tauri::AppHandle,
    from_index: usize,
    to_index: usize,
) -> Result<usize, String> {
    tauri::async_runtime::spawn_blocking(move || {
        with_lazy_window_operation(|| reorder_category_impl(&app, from_index, to_index))
    })
    .await
    .map_err(to_message)?
}

fn reorder_category_impl(
    app: &tauri::AppHandle,
    from_index: usize,
    to_index: usize,
) -> Result<usize, String> {
    let desktop_organizer_was_visible = app.webview_windows().values().any(|window| {
        window.is_visible().unwrap_or(false) && is_desktop_organizer_window_label(window.label())
    });

    let (categories, split_category_indices) = with_config_mutation(|| {
        let store = AppStore::open().map_err(to_message)?;
        let mut config = store.load_config_strict().map_err(to_message)?;
        reorder_category_config(&mut config, from_index, to_index)?;
        let categories = config.desktop_categories.clone();
        let split_category_indices = config.desktop_layout.split_category_indices.clone();
        store.save_config(&config).map_err(to_message)?;
        Ok((categories, split_category_indices))
    })?;

    if from_index != to_index {
        reconcile_reordered_category_windows(
            app,
            &categories,
            &split_category_indices,
            desktop_organizer_was_visible,
        )?;
    }

    let payload = CategoryOrderChangedPayload {
        from_index,
        to_index,
    };
    let _ = app.emit(CATEGORY_ORDER_CHANGED_EVENT, payload);
    emit_desktop_cards_changed(app);
    Ok(to_index)
}

fn reconcile_reordered_category_windows(
    app: &tauri::AppHandle,
    categories: &[DeskCategory],
    split_category_indices: &[usize],
    desktop_organizer_was_visible: bool,
) -> Result<(), String> {
    for window in app.webview_windows().values() {
        let Some(index) = desktop_category_index_from_label(window.label()) else {
            continue;
        };
        if !desktop_organizer_was_visible || !split_category_indices.contains(&index) {
            window.hide().map_err(to_message)?;
        }
    }

    if !desktop_organizer_was_visible {
        return Ok(());
    }

    for index in split_category_indices.iter().copied() {
        let category = categories
            .get(index)
            .ok_or_else(|| "分类不存在".to_owned())?;
        show_desktop_category_on_all_monitors(app, index, category)?;
    }
    Ok(())
}

fn reorder_category_config(
    config: &mut AppConfig,
    from_index: usize,
    to_index: usize,
) -> Result<(), String> {
    let category_count = config.desktop_categories.len();
    if from_index >= category_count || to_index >= category_count {
        return Err("分类不存在".to_owned());
    }
    if from_index == to_index {
        return Ok(());
    }

    let category = config.desktop_categories.remove(from_index);
    config.desktop_categories.insert(to_index, category);
    config.desktop_layout.split_category_indices = normalize_desktop_split_indices(
        config
            .desktop_layout
            .split_category_indices
            .iter()
            .copied()
            .map(|index| remap_category_index(index, from_index, to_index))
            .collect(),
        category_count,
    );

    let previous_layouts = std::mem::take(&mut config.desktop_layout.windows);
    config.desktop_layout.windows = previous_layouts
        .into_iter()
        .map(|(label, layout)| {
            let mapped_label =
                remap_desktop_category_window_label(&label, from_index, to_index).unwrap_or(label);
            (mapped_label, layout)
        })
        .collect();
    Ok(())
}

fn remap_category_index(index: usize, from_index: usize, to_index: usize) -> usize {
    if index == from_index {
        return to_index;
    }
    if from_index < to_index && index > from_index && index <= to_index {
        return index - 1;
    }
    if from_index > to_index && index >= to_index && index < from_index {
        return index + 1;
    }
    index
}

#[tauri::command]
async fn add_item_to_category(
    app: tauri::AppHandle,
    index: usize,
    path: String,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let result = add_items_to_category_impl(index, vec![path]);
        emit_desktop_cards_changed(&app);
        result.map(|_| ())
    })
    .await
    .map_err(to_message)?
}

#[tauri::command]
async fn add_items_to_category(
    app: tauri::AppHandle,
    index: usize,
    paths: Vec<String>,
) -> Result<usize, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let result = add_items_to_category_impl(index, paths);
        emit_desktop_cards_changed(&app);
        result
    })
    .await
    .map_err(to_message)?
}

fn add_items_to_category_impl(index: usize, paths: Vec<String>) -> Result<usize, String> {
    with_config_mutation(|| {
        let store = AppStore::open().map_err(to_message)?;
        store.ensure_runtime_dirs().map_err(to_message)?;
        let paths = paths
            .into_iter()
            .map(|path| normalize_path_input(&path))
            .collect::<Result<Vec<_>, _>>()?;
        recover_transfer_quarantine_journals_strict(true)?;
        recover_related_transfer_quarantine_journals_strict(
            &paths.iter().map(PathBuf::from).collect::<Vec<_>>(),
        )?;
        let mut config = store.load_config_strict().map_err(to_message)?;
        repair_category_item_paths(&store, &mut config)?;
        config
            .desktop_categories
            .get(index)
            .ok_or_else(|| "分类不存在".to_owned())?;
        let category_name = config.desktop_categories[index].name.clone();
        let mut seen_source_paths = Vec::<String>::new();
        let source_paths = paths
            .into_iter()
            .filter(|path| !path.trim().is_empty() && Path::new(path).exists())
            .filter(|path| {
                let normalized = normalize_path_for_compare(Path::new(path));
                if seen_source_paths.iter().any(|item| item == &normalized) {
                    return false;
                }
                seen_source_paths.push(normalized);
                true
            })
            .collect::<Vec<_>>();
        // Read the Explorer ListView text and coordinates before moving the first source. Exact
        // text matching is required because LVM_FINDITEM treats names such as `QQ`/`QQ飞车` and
        // `Tabbit`/`tabbit2api` as ambiguous prefix matches.
        let mut captured_positions = capture_desktop_icon_positions(
            &source_paths.iter().map(PathBuf::from).collect::<Vec<_>>(),
        );
        let paths_with_positions = source_paths
            .into_iter()
            .map(|path| {
                let position =
                    desktop_icon_position_for_path(&config.desktop_icon_positions, &path).or_else(
                        || captured_positions.remove(&normalize_path_for_compare(Path::new(&path))),
                    );
                (path, position)
            })
            .collect::<Vec<_>>();
        let mut added = 0usize;

        for (path, desktop_icon_position) in paths_with_positions {
            if !Path::new(&path).exists() {
                continue;
            }
            let archived_path = archive_item_path(&store, &category_name, &path)?;

            for category in &mut config.desktop_categories {
                category.item_paths.retain(|item_path| {
                    !same_path_text(item_path, &path) && !same_path_text(item_path, &archived_path)
                });
            }
            remove_desktop_icon_position_for_path(&mut config.desktop_icon_positions, &path);
            remove_desktop_icon_position_for_path(
                &mut config.desktop_icon_positions,
                &archived_path,
            );
            if let Some(position) = desktop_icon_position {
                set_desktop_icon_position_for_path(
                    &mut config.desktop_icon_positions,
                    archived_path.clone(),
                    position,
                );
            }

            config.desktop_categories[index]
                .item_paths
                .push(archived_path);
            added += 1;
        }

        if added > 0 {
            store.save_config(&config).map_err(to_message)?;
        }
        Ok(added)
    })
}

#[tauri::command]
fn remove_item_from_category(
    app: tauri::AppHandle,
    index: usize,
    path: String,
) -> Result<(), String> {
    let path = normalize_path_input(&path)?;
    mutate_categories(|categories| {
        let category = categories
            .get_mut(index)
            .ok_or_else(|| "分类不存在".to_owned())?;
        category
            .item_paths
            .retain(|item_path| !same_path_text(item_path, &path));
        Ok(())
    })?;
    emit_desktop_cards_changed(&app);
    Ok(())
}

#[tauri::command]
fn register_internal_path_drop_category(
    window: WebviewWindow,
    index: Option<usize>,
) -> Result<(), String> {
    let targets = DESKTOP_DROP_CATEGORY_TARGETS.get_or_init(|| Mutex::new(BTreeMap::new()));
    let mut targets = targets
        .lock()
        .map_err(|_| "桌面框拖拽目标状态锁已损坏".to_owned())?;
    if let Some(index) = index {
        targets.insert(window.label().to_owned(), index);
    } else {
        targets.remove(window.label());
    }
    Ok(())
}

#[tauri::command]
fn mark_internal_path_drop_accepted(
    paths: Vec<String>,
    session_id: Option<String>,
) -> Result<(), String> {
    let now = Instant::now();
    let acceptances = INTERNAL_PATH_DROP_ACCEPTANCES.get_or_init(|| Mutex::new(Vec::new()));
    let mut acceptances = acceptances
        .lock()
        .map_err(|_| "内部项目拖拽状态锁已损坏".to_owned())?;
    acceptances.retain(|acceptance| {
        now.saturating_duration_since(acceptance.accepted_at) <= Duration::from_secs(3)
    });
    for path in paths {
        let path = normalize_path_for_compare(Path::new(path.trim()));
        if path.is_empty() {
            continue;
        }
        acceptances.retain(|acceptance| acceptance.path != path);
        acceptances.push(InternalPathDropAcceptance {
            path,
            session_id: session_id.clone().filter(|value| !value.trim().is_empty()),
            accepted_at: now,
        });
    }
    Ok(())
}

#[tauri::command]
async fn complete_internal_path_drag(
    app: tauri::AppHandle,
    window: WebviewWindow,
    source_index: usize,
    path: String,
    position: DesktopDropPosition,
    drag_session_id: Option<String>,
) -> Result<InternalPathDragOutcome, String> {
    let source_label = window.label().to_owned();
    tauri::async_runtime::spawn_blocking(move || {
        complete_internal_path_drag_impl(
            &app,
            &source_label,
            source_index,
            path,
            position,
            drag_session_id,
        )
    })
    .await
    .map_err(to_message)?
}

fn complete_internal_path_drag_impl(
    app: &tauri::AppHandle,
    source_label: &str,
    source_index: usize,
    path: String,
    position: DesktopDropPosition,
    drag_session_id: Option<String>,
) -> Result<InternalPathDragOutcome, String> {
    if let Some(target) = internal_path_drop_target_at_position(app, source_label, position) {
        return match target {
            InternalPathDropTarget::Category(target_index) => {
                let affected = if target_index == source_index {
                    0
                } else {
                    let affected = add_items_to_category_impl(target_index, vec![path])?;
                    emit_desktop_cards_changed(app);
                    affected
                };
                Ok(InternalPathDragOutcome {
                    action: "moved_to_category".to_owned(),
                    target_category_index: Some(target_index),
                    affected,
                    restored_path: None,
                })
            }
            InternalPathDropTarget::Launcher => {
                let affected = add_launchers_impl(vec![path])?;
                emit_desktop_cards_changed(app);
                Ok(InternalPathDragOutcome {
                    action: "added_to_launcher".to_owned(),
                    target_category_index: None,
                    affected,
                    restored_path: None,
                })
            }
            InternalPathDropTarget::Box => Ok(InternalPathDragOutcome {
                action: "accepted_by_box".to_owned(),
                target_category_index: None,
                affected: 0,
                restored_path: None,
            }),
        };
    }

    // A main-window drop can be accepted without exposing a native file payload to the source
    // WebView. Give that target command a short window to acknowledge before restoring to Desktop.
    for attempt in 0..6 {
        if take_recent_internal_path_drop_acceptance(&path, drag_session_id.as_deref())? {
            return Ok(InternalPathDragOutcome {
                action: "accepted_by_box".to_owned(),
                target_category_index: None,
                affected: 0,
                restored_path: None,
            });
        }
        if attempt < 5 {
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    let restored = restore_item_to_desktop_impl(app, source_index, path, Some(position))?;
    Ok(InternalPathDragOutcome {
        action: "restored_to_desktop".to_owned(),
        target_category_index: None,
        affected: 1,
        restored_path: Some(restored),
    })
}

fn take_recent_internal_path_drop_acceptance(
    path: &str,
    session_id: Option<&str>,
) -> Result<bool, String> {
    let path = normalize_path_for_compare(Path::new(path.trim()));
    let now = Instant::now();
    let acceptances = INTERNAL_PATH_DROP_ACCEPTANCES.get_or_init(|| Mutex::new(Vec::new()));
    let mut acceptances = acceptances
        .lock()
        .map_err(|_| "内部项目拖拽状态锁已损坏".to_owned())?;
    acceptances.retain(|acceptance| {
        now.saturating_duration_since(acceptance.accepted_at) <= Duration::from_secs(3)
    });
    let Some(index) = acceptances.iter().position(|acceptance| {
        acceptance.path == path
            && match session_id {
                Some(session_id) => acceptance.session_id.as_deref() == Some(session_id),
                None => true,
            }
    }) else {
        return Ok(false);
    };
    acceptances.remove(index);
    Ok(true)
}

#[tauri::command]
async fn restore_item_to_desktop(
    app: tauri::AppHandle,
    index: usize,
    path: String,
    position: Option<DesktopDropPosition>,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        restore_item_to_desktop_impl(&app, index, path, position)
    })
    .await
    .map_err(to_message)?
}

fn restore_item_to_desktop_impl(
    app: &tauri::AppHandle,
    index: usize,
    path: String,
    position: Option<DesktopDropPosition>,
) -> Result<String, String> {
    let requested_icon_position = desktop_drop_position_to_icon_position(position);
    let (restored_path, stored_icon_position) = with_config_mutation(|| {
        let path = normalize_path_input(&path)?;
        let store = AppStore::open().map_err(to_message)?;
        clear_desktop_organization_restart_marker(&AppDesktopOrganizationRestartPersistence)?;
        recover_transfer_quarantine_journals_strict(true)?;
        recover_related_transfer_quarantine_journals_strict(&[PathBuf::from(&path)])?;
        let mut config = store.load_config_strict().map_err(to_message)?;
        let desktop = user_desktop().ok_or_else(|| "没有找到桌面路径".to_owned())?;
        fs::create_dir_all(&desktop).map_err(to_message)?;
        let source = recover_existing_path_from_corrupted_text(&path)
            .unwrap_or_else(|| PathBuf::from(&path));
        let stored_icon_position =
            desktop_icon_position_for_path(&config.desktop_icon_positions, &path).or_else(|| {
                desktop_icon_position_for_path(
                    &config.desktop_icon_positions,
                    &source.display().to_string(),
                )
            });
        let restored_path = restore_path_to_desktop(&source, &desktop)?;
        remove_category_path_from_config(&mut config, index, &path)?;
        remove_desktop_icon_position_for_path(&mut config.desktop_icon_positions, &path);
        remove_desktop_icon_position_for_path(
            &mut config.desktop_icon_positions,
            &source.display().to_string(),
        );
        store.save_config(&config).map_err(to_message)?;
        Ok((restored_path, stored_icon_position))
    })?;
    position_desktop_icon_at(
        &restored_path,
        requested_icon_position.or(stored_icon_position),
    );
    emit_desktop_cards_changed(app);
    Ok(restored_path.display().to_string())
}

#[tauri::command]
async fn restore_all_to_desktop(app: tauri::AppHandle) -> Result<usize, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let restored = restore_all_organized_items_to_desktop_and_clear_markers()?;
        emit_desktop_cards_changed(&app);
        Ok(restored)
    })
    .await
    .map_err(to_message)?
}

#[tauri::command]
async fn start_restore_all_to_desktop_task(app: tauri::AppHandle) -> Result<(), String> {
    start_restore_background_operation(app)
}

fn start_restore_background_operation(app: tauri::AppHandle) -> Result<(), String> {
    start_desktop_background_operation(app.clone(), "restore", move || {
        let progress_app = app.clone();
        let restored =
            restore_all_organized_items_to_desktop_with_progress(move |current, total, path| {
                emit_desktop_operation(
                    &progress_app,
                    DesktopOperationPayload {
                        kind: "restore",
                        scope: "manual",
                        status: "progress",
                        message: restore_progress_message(current, total, path),
                        moved: 0,
                        skipped: 0,
                        restored: current,
                        total,
                        current_path: path.display().to_string(),
                        category_counts: Vec::new(),
                    },
                );
            })?;
        Ok(DesktopOperationPayload {
            kind: "restore",
            scope: "manual",
            status: "finished",
            message: if restored > 0 {
                format!("已还原 {restored} 项到桌面")
            } else {
                "没有需要还原到桌面的收纳项目".to_owned()
            },
            moved: 0,
            skipped: 0,
            restored,
            total: restored,
            current_path: String::new(),
            category_counts: Vec::new(),
        })
    })
}

#[tauri::command]
async fn add_launcher(app: tauri::AppHandle, path: String, name: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        add_launcher_impl(path, name)?;
        emit_desktop_cards_changed(&app);
        Ok(())
    })
    .await
    .map_err(to_message)?
}

#[tauri::command]
async fn add_launchers(app: tauri::AppHandle, paths: Vec<String>) -> Result<usize, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let added = add_launchers_impl(paths)?;
        emit_desktop_cards_changed(&app);
        Ok(added)
    })
    .await
    .map_err(to_message)?
}

fn add_launcher_impl(path: String, name: String) -> Result<(), String> {
    let path = normalize_path_input(&path)?;
    add_launchers_with_names(vec![(path, name)]).map(|_| ())
}

fn add_launchers_impl(paths: Vec<String>) -> Result<usize, String> {
    let mut items = Vec::with_capacity(paths.len());
    for path in paths {
        let path = normalize_path_input(&path)?;
        if path.trim().is_empty() || !Path::new(&path).exists() {
            continue;
        }
        let name = display_path_name(Path::new(&path));
        items.push((path, name));
    }
    add_launchers_with_names(items)
}

fn add_launchers_with_names(items: Vec<(String, String)>) -> Result<usize, String> {
    with_launcher_mutation(|| {
        let store = AppStore::open().map_err(to_message)?;
        store.ensure_runtime_dirs().map_err(to_message)?;
        let mut launchers = store.load_launchers_strict().map_err(to_message)?;
        repair_launchers(&store, &mut launchers)?;
        let mut added = 0usize;

        for (path, name) in items {
            let entry_path = create_launcher_entry(&store, Path::new(&path), &name)?;
            let key = entry_path.to_string_lossy().to_lowercase();

            if let Some(item) = launchers
                .items
                .iter_mut()
                .find(|item| item.path.to_lowercase() == key)
            {
                item.name = normalized_launch_name(&name, &entry_path);
                item.icon_data_url = None;
            } else {
                launchers.items.push(LaunchItem {
                    name: normalized_launch_name(&name, &entry_path),
                    path: entry_path.display().to_string(),
                    icon_data_url: None,
                });
            }
            added += 1;
        }

        store.save_launchers(&launchers).map_err(to_message)?;
        Ok(added)
    })
}

#[tauri::command]
fn remove_launcher(app: tauri::AppHandle, path: String) -> Result<(), String> {
    with_launcher_mutation(|| {
        let path = normalize_path_input(&path)?;
        let store = AppStore::open().map_err(to_message)?;
        store.ensure_runtime_dirs().map_err(to_message)?;
        let mut launchers = store.load_launchers_strict().map_err(to_message)?;
        launchers
            .items
            .retain(|item| !same_path_text(&item.path, &path));
        let path_buf = PathBuf::from(&path);
        if is_path_within(&path_buf, &store.launchers_root()) {
            remove_existing_path(&path_buf)?;
        }
        store.save_launchers(&launchers).map_err(to_message)
    })?;
    emit_desktop_cards_changed(&app);
    Ok(())
}

#[tauri::command]
fn show_path_in_folder(path: String) -> Result<(), String> {
    let path = normalize_path_input(&path)?;
    show_path_in_folder_impl(Path::new(&path))
}

#[tauri::command]
async fn classify_desktop_items(app: tauri::AppHandle) -> Result<ClassifyResult, String> {
    let result = tauri::async_runtime::spawn_blocking(|| classify_desktop_items_impl())
        .await
        .map_err(to_message)?;
    emit_desktop_cards_changed(&app);
    result
}

#[tauri::command]
async fn start_classify_desktop_items_task(app: tauri::AppHandle) -> Result<(), String> {
    start_desktop_background_operation(app.clone(), "classify", move || {
        let progress_app = app.clone();
        let result =
            classify_desktop_items_with_progress(move |current, total, path, moved, skipped| {
                emit_desktop_operation(
                    &progress_app,
                    DesktopOperationPayload {
                        kind: "classify",
                        scope: "manual",
                        status: "progress",
                        message: classify_progress_message(current, total, path),
                        moved,
                        skipped,
                        restored: 0,
                        total,
                        current_path: path.display().to_string(),
                        category_counts: Vec::new(),
                    },
                );
            })?;
        let detail = result
            .category_counts
            .iter()
            .map(|item| format!("{} {}", item.name, item.count))
            .collect::<Vec<_>>()
            .join("、");
        Ok(DesktopOperationPayload {
            kind: "classify",
            scope: "manual",
            status: "finished",
            message: format!(
                "已智能收纳 {} 项{}{}{}",
                result.moved,
                if detail.is_empty() { "" } else { "：" },
                detail,
                if result.skipped > 0 {
                    format!("，跳过 {} 项", result.skipped)
                } else {
                    String::new()
                }
            ),
            moved: result.moved,
            skipped: result.skipped,
            restored: 0,
            total: result.moved + result.skipped,
            current_path: String::new(),
            category_counts: result.category_counts,
        })
    })
}

fn classify_desktop_items_impl() -> Result<ClassifyResult, String> {
    classify_desktop_items_with_progress(|_, _, _, _, _| {})
}

fn classify_desktop_items_with_progress(
    mut on_progress: impl FnMut(usize, usize, &Path, usize, usize),
) -> Result<ClassifyResult, String> {
    let overall_started = Instant::now();
    append_desktop_diagnostic(
        "classify_task_started",
        &format!(
            "current_exe=\"{}\"",
            diagnostic_value(&current_exe_diagnostic_value())
        ),
    );
    let result = with_config_mutation(|| {
        let store = AppStore::open().map_err(to_message)?;
        store.ensure_runtime_dirs().map_err(to_message)?;
        recover_transfer_quarantine_journals_strict(true)?;
        let mut config = store.load_config_strict().map_err(to_message)?;
        repair_category_item_paths(&store, &mut config)?;
        if config.desktop_categories.is_empty() {
            return Err("没有可用分类，请先创建分类".to_owned());
        }

        let mut category_counts = vec![0usize; config.desktop_categories.len()];
        let mut skipped = 0usize;
        let mut candidates = Vec::new();
        let items = desktop_items(false);
        let mut captured_positions = capture_desktop_icon_positions(
            &items
                .iter()
                .map(|item| PathBuf::from(&item.path))
                .collect::<Vec<_>>(),
        );
        for item in items {
            if should_skip_desktop_classify_item(&item) {
                skipped += 1;
                continue;
            }

            if categories_contain_path(&config.desktop_categories, &item.path) {
                skipped += 1;
                continue;
            }

            let Some(category_index) = classify_desktop_item(&item, &config.desktop_categories)
            else {
                skipped += 1;
                continue;
            };

            let original_path = item.path.clone();
            let category_name = config.desktop_categories[category_index].name.clone();
            let work_estimate = estimate_transfer_work(Path::new(&original_path));
            candidates.push(ClassifyCandidate {
                desktop_icon_position: desktop_icon_position_for_path(
                    &config.desktop_icon_positions,
                    &original_path,
                )
                .or_else(|| {
                    captured_positions
                        .remove(&normalize_path_for_compare(Path::new(&original_path)))
                }),
                original_path,
                category_index,
                category_name,
                is_dir: item.is_dir,
                work_estimate,
            });
        }

        let candidate_paths = candidates
            .iter()
            .map(|candidate| PathBuf::from(&candidate.original_path))
            .collect::<Vec<_>>();
        recover_related_transfer_quarantine_journals_strict(&candidate_paths)?;

        let total = candidates.len();
        let mut directory_candidates = Vec::new();
        let mut file_candidates = Vec::new();
        for candidate in candidates {
            if candidate.is_dir {
                directory_candidates.push(candidate);
            } else {
                file_candidates.push(candidate);
            }
        }
        directory_candidates.sort_by(|left, right| {
            right
                .work_estimate
                .cmp(&left.work_estimate)
                .then_with(|| left.original_path.cmp(&right.original_path))
        });
        file_candidates.sort_by(|left, right| {
            right
                .work_estimate
                .cmp(&left.work_estimate)
                .then_with(|| left.original_path.cmp(&right.original_path))
        });

        let directory_count = directory_candidates.len();
        let file_count = file_candidates.len();
        let max_workers = desktop_operation_worker_count(total);
        let directory_worker_count = usize::from(directory_count > 0);
        let file_worker_count = if file_count > 0 {
            max_workers
                .saturating_sub(directory_worker_count)
                .max(1)
                .min(file_count)
        } else {
            0
        };
        let queues = [
            (
                Arc::new(Mutex::new(VecDeque::from(directory_candidates))),
                directory_worker_count,
            ),
            (
                Arc::new(Mutex::new(VecDeque::from(file_candidates))),
                file_worker_count,
            ),
        ];
        let (sender, receiver) = mpsc::channel();
        let mut moved = 0usize;
        let mut handled = 0usize;
        let mut active = Vec::<ClassifyCandidate>::new();

        std::thread::scope(|scope| -> Result<(), String> {
            for (queue, worker_count) in queues {
                for _ in 0..worker_count {
                    let queue = Arc::clone(&queue);
                    let sender = sender.clone();
                    let store = store.clone();
                    scope.spawn(move || loop {
                        let candidate = match queue.lock() {
                            Ok(mut queue) => queue.pop_front(),
                            Err(_) => {
                                let _ = sender.send(ClassifyWorkerMessage::Failed(
                                    "收纳任务队列已损坏".to_owned(),
                                ));
                                return;
                            }
                        };
                        let Some(candidate) = candidate else {
                            return;
                        };
                        let _ = sender.send(ClassifyWorkerMessage::Started(candidate.clone()));
                        let item_started = Instant::now();
                        let result = archive_item_path(
                            &store,
                            &candidate.category_name,
                            &candidate.original_path,
                        );
                        let elapsed = item_started.elapsed();
                        if sender
                            .send(ClassifyWorkerMessage::Finished(candidate, result, elapsed))
                            .is_err()
                        {
                            return;
                        }
                    });
                }
            }
            drop(sender);

            for message in receiver {
                let (candidate, archive_result, elapsed) = match message {
                    ClassifyWorkerMessage::Started(candidate) => {
                        on_progress(
                            handled.saturating_add(1).min(total),
                            total,
                            Path::new(&candidate.original_path),
                            moved,
                            skipped,
                        );
                        active.push(candidate);
                        continue;
                    }
                    ClassifyWorkerMessage::Finished(candidate, result, elapsed) => {
                        handled += 1;
                        active.retain(|item| {
                            !same_path_text(&item.original_path, &candidate.original_path)
                        });
                        (candidate, result, elapsed)
                    }
                    ClassifyWorkerMessage::Failed(error) => {
                        handled += 1;
                        skipped += 1;
                        append_desktop_diagnostic(
                            "classify_worker_failed",
                            &format!("error=\"{}\"", diagnostic_value(&error)),
                        );
                        eprintln!("failed to classify desktop item: {error}");
                        continue;
                    }
                };

                match &archive_result {
                    Ok(path) => eprintln!(
                        "[desktop-classify] ok elapsed_ms={} source={} destination={}",
                        elapsed.as_millis(),
                        candidate.original_path,
                        path
                    ),
                    Err(error) => eprintln!(
                        "[desktop-classify] failed elapsed_ms={} source={} error={}",
                        elapsed.as_millis(),
                        candidate.original_path,
                        error
                    ),
                }

                match archive_result {
                    Ok(archived_path) => {
                        for category in &mut config.desktop_categories {
                            category.item_paths.retain(|item_path| {
                                !same_path_text(item_path, &candidate.original_path)
                                    && !same_path_text(item_path, &archived_path)
                            });
                        }
                        remove_desktop_icon_position_for_path(
                            &mut config.desktop_icon_positions,
                            &candidate.original_path,
                        );
                        remove_desktop_icon_position_for_path(
                            &mut config.desktop_icon_positions,
                            &archived_path,
                        );
                        if let Some(position) = candidate.desktop_icon_position {
                            set_desktop_icon_position_for_path(
                                &mut config.desktop_icon_positions,
                                archived_path.clone(),
                                position,
                            );
                        }

                        push_unique_text_path(
                            &mut config.desktop_categories[candidate.category_index].item_paths,
                            archived_path.clone(),
                        );
                        category_counts[candidate.category_index] += 1;
                        moved += 1;
                        on_progress(handled, total, Path::new(&archived_path), moved, skipped);
                    }
                    Err(error) => {
                        eprintln!(
                            "failed to classify desktop item {}: {error}",
                            candidate.original_path
                        );
                        skipped += 1;
                        on_progress(
                            handled,
                            total,
                            Path::new(&candidate.original_path),
                            moved,
                            skipped,
                        );
                    }
                }

                if handled < total {
                    if let Some(current) = active.iter().max_by_key(|item| item.work_estimate) {
                        on_progress(
                            handled + 1,
                            total,
                            Path::new(&current.original_path),
                            moved,
                            skipped,
                        );
                    }
                }
            }
            Ok(())
        })?;

        let save_started = Instant::now();
        store.save_config(&config).map_err(to_message)?;
        eprintln!(
            "[desktop-classify] complete total_ms={} save_ms={} moved={} skipped={} directories={} files={}",
            overall_started.elapsed().as_millis(),
            save_started.elapsed().as_millis(),
            moved,
            skipped,
            directory_count,
            file_count
        );

        Ok(ClassifyResult {
            moved,
            skipped,
            category_counts: config
                .desktop_categories
                .iter()
                .enumerate()
                .filter_map(|(index, category)| {
                    let count = category_counts.get(index).copied().unwrap_or_default();
                    (count > 0).then(|| CategoryClassifyCount {
                        name: category.name.clone(),
                        count,
                    })
                })
                .collect(),
        })
    });
    match &result {
        Ok(result) => append_desktop_diagnostic(
            "classify_task_finished",
            &format!(
                "elapsed_ms={} moved={} skipped={}",
                overall_started.elapsed().as_millis(),
                result.moved,
                result.skipped
            ),
        ),
        Err(error) => append_desktop_diagnostic(
            "classify_task_failed",
            &format!(
                "elapsed_ms={} error=\"{}\"",
                overall_started.elapsed().as_millis(),
                diagnostic_value(error)
            ),
        ),
    }
    result
}

#[tauri::command]
fn create_desktop_entries() -> Result<Vec<String>, String> {
    let store = AppStore::open().map_err(to_message)?;
    store.ensure_runtime_dirs().map_err(to_message)?;
    let desktop = user_desktop().ok_or_else(|| "没有找到桌面路径".to_owned())?;
    fs::create_dir_all(&desktop).map_err(to_message)?;

    let entries = app_desktop_entries(&store);
    let mut created = Vec::with_capacity(entries.len());

    for (name, target) in entries {
        let shortcut = desktop.join(format!("{name}.lnk"));
        create_windows_shortcut(&shortcut, target.as_path(), name)?;
        created.push(shortcut.display().to_string());
    }

    Ok(created)
}

fn app_desktop_entries(store: &AppStore) -> Vec<(&'static str, PathBuf)> {
    vec![
        ("DustDesk 收纳箱", store.organizer_root()),
        ("DustDesk 快捷启动", store.launchers_root()),
    ]
}

fn repair_existing_app_desktop_entries(store: &AppStore) -> Result<(), String> {
    let Some(desktop) = user_desktop() else {
        return Ok(());
    };
    fs::create_dir_all(&desktop).map_err(to_message)?;

    for (name, target) in app_desktop_entries(store) {
        for alias in app_desktop_entry_aliases(name) {
            let shortcut = desktop.join(format!("{alias}.lnk"));
            if shortcut.exists() {
                create_windows_shortcut(&shortcut, target.as_path(), name)?;
            }
        }
    }

    Ok(())
}

fn app_desktop_entry_aliases(name: &str) -> &'static [&'static str] {
    match name {
        "DustDesk 收纳箱" => &[
            "DustDesk 收纳箱",
            "DustDesk 桌面收纳",
            "DustDusk 收纳箱",
            "DustDusk 桌面收纳",
            "DeskNest 收纳箱",
            "DeskNest 桌面收纳",
        ],
        "DustDesk 快捷启动" => &[
            "DustDesk 快捷启动",
            "DustDusk 快捷启动",
            "DeskNest 快捷启动",
        ],
        _ => &[],
    }
}

#[tauri::command]
fn open_special(target: String) -> Result<(), String> {
    let store = AppStore::open().map_err(to_message)?;
    let path = match target.as_str() {
        "organizer" => {
            store.ensure_runtime_dirs().map_err(to_message)?;
            store.organizer_root()
        }
        "launchers" => {
            store.ensure_runtime_dirs().map_err(to_message)?;
            store.launchers_root()
        }
        "data" => store.data_dir(),
        "desktop" => user_desktop().ok_or_else(|| "没有找到桌面路径".to_owned())?,
        _ => return Err("未知入口".to_owned()),
    };
    open_path_impl(&path)
}

#[tauri::command]
async fn check_for_updates() -> Result<AppUpdateInfo, String> {
    tauri::async_runtime::spawn_blocking(check_for_updates_impl)
        .await
        .map_err(to_message)?
}

#[tauri::command]
async fn open_update_download(download_url: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let trimmed = download_url.trim();
        if !is_trusted_update_url(trimmed) {
            return Err("更新下载地址不可信".to_owned());
        }
        open_with_shell(trimmed)
    })
    .await
    .map_err(to_message)?
}

fn check_for_updates_impl() -> Result<AppUpdateInfo, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(to_message)?;
    let response = client
        .get(UPDATE_RELEASE_API_URL)
        .header(reqwest::header::USER_AGENT, "DustDesk updater")
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .map_err(|error| format!("检查更新失败：{error}"))?;

    if !response.status().is_success() {
        return Err(format!("检查更新失败：GitHub 返回 {}", response.status()));
    }

    let release = response
        .json::<GitHubRelease>()
        .map_err(|error| format!("读取更新信息失败：{error}"))?;
    let latest_version = release_version_from_tag(&release.tag_name)
        .ok_or_else(|| format!("无法识别最新版本号：{}", release.tag_name))?;
    let current_version = env!("CARGO_PKG_VERSION").to_owned();
    let update_available = compare_semver_like(&latest_version, &current_version).is_gt();
    let asset = release
        .assets
        .iter()
        .find(|asset| {
            asset
                .name
                .eq_ignore_ascii_case("DustDesk-latest-windows-x64-setup.exe")
        })
        .or_else(|| {
            release.assets.iter().find(|asset| {
                let name = asset.name.to_ascii_lowercase();
                name.ends_with(".exe") && name.contains("setup")
            })
        });
    let download_url = asset
        .map(|asset| asset.browser_download_url.clone())
        .unwrap_or_else(|| UPDATE_LATEST_INSTALLER_URL.to_owned());
    let asset_name = asset.map(|asset| asset.name.clone()).unwrap_or_default();

    Ok(AppUpdateInfo {
        current_version,
        latest_version,
        update_available,
        release_name: release
            .name
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| release.tag_name),
        release_url: release.html_url,
        download_url,
        asset_name,
        published_at: release.published_at.unwrap_or_default(),
        body: release.body.unwrap_or_default(),
    })
}

fn is_trusted_update_url(url: &str) -> bool {
    if !url.starts_with(UPDATE_RELEASES_URL_PREFIX) {
        return false;
    }
    url.starts_with("https://")
}

fn release_version_from_tag(tag: &str) -> Option<String> {
    let trimmed = tag.trim().trim_start_matches('v').trim_start_matches('V');
    let version = trimmed
        .split(|character: char| !(character.is_ascii_digit() || character == '.'))
        .next()
        .unwrap_or_default()
        .trim_matches('.');
    if version.is_empty() {
        return None;
    }
    let parts: Vec<_> = version.split('.').collect();
    if parts.len() < 3
        || !parts.iter().all(|part| {
            !part.is_empty() && part.chars().all(|character| character.is_ascii_digit())
        })
    {
        return None;
    }
    Some(parts[..3].join("."))
}

fn compare_semver_like(left: &str, right: &str) -> CmpOrdering {
    let left = parse_semver_like(left);
    let right = parse_semver_like(right);
    left.cmp(&right)
}

fn parse_semver_like(version: &str) -> [u64; 3] {
    let mut output = [0, 0, 0];
    for (index, part) in version
        .trim()
        .trim_start_matches('v')
        .trim_start_matches('V')
        .split('.')
        .take(3)
        .enumerate()
    {
        output[index] = part
            .chars()
            .take_while(|character| character.is_ascii_digit())
            .collect::<String>()
            .parse::<u64>()
            .unwrap_or(0);
    }
    output
}

#[tauri::command]
async fn update_runtime_directory(target: String, path: String) -> Result<AppSnapshot, String> {
    tauri::async_runtime::spawn_blocking(move || update_runtime_directory_impl(target, path))
        .await
        .map_err(to_message)?
}

fn update_runtime_directory_impl(target: String, path: String) -> Result<AppSnapshot, String> {
    store::with_storage_mutation(move || update_runtime_directory_locked(target, path))?;
    load_snapshot_impl().map_err(|error| format!("运行目录已切换成功，但刷新界面失败：{error}"))
}

fn update_runtime_directory_locked(target: String, path: String) -> Result<(), String> {
    let _guard = RUNTIME_DIRECTORY_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| "运行目录迁移锁已损坏".to_owned())?;
    let target = target.trim().to_owned();
    if !matches!(target.as_str(), "data" | "organizer" | "launchers") {
        return Err("未知目录类型".to_owned());
    }

    let path = path.trim();
    if path.is_empty() {
        return Err("目录不能为空".to_owned());
    }

    recover_pending_runtime_migration_locked()?;

    let old_store = AppStore::open().map_err(to_message)?;
    let old_data_dir = old_store.data_dir();
    let old_organizer_root = old_store.organizer_root();
    let old_launchers_root = old_store.launchers_root();
    let requested_path = PathBuf::from(path);
    validate_runtime_directory_target(&target, &requested_path, &old_store)?;

    let prospective_store = old_store
        .with_runtime_directory(&target, &requested_path)
        .map_err(to_message)?;
    prospective_store
        .ensure_runtime_dirs()
        .map_err(to_message)?;
    let new_data_dir = prospective_store.data_dir();
    let new_organizer_root = prospective_store.organizer_root();
    let new_launchers_root = prospective_store.launchers_root();

    let journal = RuntimeMigrationJournal {
        target,
        phase: "prepared".to_owned(),
        requested_path: requested_path.display().to_string(),
        old_data_dir: old_data_dir.display().to_string(),
        old_organizer_root: old_organizer_root.display().to_string(),
        old_launchers_root: old_launchers_root.display().to_string(),
        new_data_dir: new_data_dir.display().to_string(),
        new_organizer_root: new_organizer_root.display().to_string(),
        new_launchers_root: new_launchers_root.display().to_string(),
    };
    save_runtime_migration_journal(&journal)?;
    complete_runtime_migration_journal(journal)?;

    Ok(())
}

fn recover_pending_runtime_migration_locked() -> Result<(), String> {
    let Some(content) = AppStore::load_runtime_migration_journal().map_err(to_message)? else {
        return Ok(());
    };
    let journal: RuntimeMigrationJournal = serde_json::from_str(&content).map_err(to_message)?;
    complete_runtime_migration_journal(journal)
}

fn complete_runtime_migration_journal(mut journal: RuntimeMigrationJournal) -> Result<(), String> {
    let prospective_store = AppStore::from_runtime_dirs(
        PathBuf::from(&journal.new_data_dir),
        PathBuf::from(&journal.new_organizer_root),
        PathBuf::from(&journal.new_launchers_root),
    );
    prospective_store
        .ensure_runtime_dirs()
        .map_err(to_message)?;

    if runtime_migration_phase_index(&journal.phase) < runtime_migration_phase_index("copied") {
        copy_runtime_migration_contents(&journal)?;
        verify_runtime_migration_contents(&journal)?;
        set_runtime_migration_phase(&mut journal, "copied")?;
    }

    if runtime_migration_phase_index(&journal.phase) < runtime_migration_phase_index("rewritten") {
        rewrite_runtime_migration_paths(&journal, &prospective_store)?;
        set_runtime_migration_phase(&mut journal, "rewritten")?;
    }

    if runtime_migration_phase_index(&journal.phase) < runtime_migration_phase_index("switched") {
        let new_store =
            AppStore::set_runtime_directory(&journal.target, Path::new(&journal.requested_path))
                .map_err(to_message)?;
        if let Err(error) = repair_existing_app_desktop_entries(&new_store) {
            eprintln!("DustDesk 运行目录已切换，但桌面入口暂未更新：{error}");
        }
        set_runtime_migration_phase(&mut journal, "switched")?;
    }

    if runtime_migration_phase_index(&journal.phase) < runtime_migration_phase_index("cleaned") {
        if let Err(error) = cleanup_runtime_migration_sources(&journal) {
            eprintln!("DustDesk 运行目录已安全切换，旧目录暂未完全清理：{error}");
        }
        set_runtime_migration_phase(&mut journal, "cleaned")?;
    }

    AppStore::remove_runtime_migration_journal().map_err(to_message)
}

fn save_runtime_migration_journal(journal: &RuntimeMigrationJournal) -> Result<(), String> {
    let json = serde_json::to_vec_pretty(journal).map_err(to_message)?;
    AppStore::save_runtime_migration_journal(&json).map_err(to_message)
}

fn set_runtime_migration_phase(
    journal: &mut RuntimeMigrationJournal,
    phase: &str,
) -> Result<(), String> {
    journal.phase = phase.to_owned();
    save_runtime_migration_journal(journal)
}

fn runtime_migration_phase_index(phase: &str) -> usize {
    match phase {
        "prepared" => 0,
        "copied" => 1,
        "rewritten" => 2,
        "switched" => 3,
        "cleaned" => 4,
        _ => 0,
    }
}

fn copy_runtime_migration_contents(journal: &RuntimeMigrationJournal) -> Result<(), String> {
    for (source, target) in runtime_migration_copy_pairs(journal) {
        copy_directory_contents_preserving_source(&source, &target)?;
    }
    Ok(())
}

fn verify_runtime_migration_contents(journal: &RuntimeMigrationJournal) -> Result<(), String> {
    for (source, target) in runtime_migration_copy_pairs(journal) {
        verify_directory_contents_contained(&source, &target)?;
    }
    Ok(())
}

fn cleanup_runtime_migration_sources(journal: &RuntimeMigrationJournal) -> Result<(), String> {
    for (source, target) in runtime_migration_copy_pairs(journal) {
        remove_verified_directory_contents(&source, &target)?;
    }
    Ok(())
}

fn rewrite_runtime_migration_paths(
    journal: &RuntimeMigrationJournal,
    store: &AppStore,
) -> Result<(), String> {
    let old_data_dir = PathBuf::from(&journal.old_data_dir);
    let old_organizer_root = PathBuf::from(&journal.old_organizer_root);
    let old_launchers_root = PathBuf::from(&journal.old_launchers_root);
    let new_data_dir = PathBuf::from(&journal.new_data_dir);
    let new_organizer_root = PathBuf::from(&journal.new_organizer_root);
    let new_launchers_root = PathBuf::from(&journal.new_launchers_root);

    match journal.target.as_str() {
        "data" => {
            rewrite_runtime_path_prefixes(
                store,
                &[
                    (&old_data_dir, &new_data_dir),
                    (&old_organizer_root, &new_organizer_root),
                    (&old_launchers_root, &new_launchers_root),
                ],
            )?;
            rewrite_launchers_path_prefixes(
                store,
                &[
                    (&old_data_dir, &new_data_dir),
                    (&old_launchers_root, &new_launchers_root),
                ],
            )?;
            rewrite_clipboard_path_prefixes(store, &[(&old_data_dir, &new_data_dir)])?;
            rewrite_search_history_path_prefixes(
                store,
                &[
                    (&old_data_dir, &new_data_dir),
                    (&old_organizer_root, &new_organizer_root),
                    (&old_launchers_root, &new_launchers_root),
                ],
            )
        }
        "organizer" => {
            rewrite_runtime_path_prefixes(store, &[(&old_organizer_root, &new_organizer_root)])?;
            rewrite_search_history_path_prefixes(
                store,
                &[(&old_organizer_root, &new_organizer_root)],
            )
        }
        "launchers" => {
            rewrite_launchers_path_prefixes(store, &[(&old_launchers_root, &new_launchers_root)])?;
            rewrite_search_history_path_prefixes(
                store,
                &[(&old_launchers_root, &new_launchers_root)],
            )
        }
        _ => Ok(()),
    }
}

fn runtime_migration_copy_pairs(journal: &RuntimeMigrationJournal) -> Vec<(PathBuf, PathBuf)> {
    let old_data_dir = PathBuf::from(&journal.old_data_dir);
    let old_organizer_root = PathBuf::from(&journal.old_organizer_root);
    let old_launchers_root = PathBuf::from(&journal.old_launchers_root);
    let new_data_dir = PathBuf::from(&journal.new_data_dir);
    let new_organizer_root = PathBuf::from(&journal.new_organizer_root);
    let new_launchers_root = PathBuf::from(&journal.new_launchers_root);

    match journal.target.as_str() {
        "data" => vec![
            (old_data_dir, new_data_dir),
            (old_organizer_root, new_organizer_root),
            (old_launchers_root, new_launchers_root),
        ],
        "organizer" => vec![(old_organizer_root, new_organizer_root)],
        "launchers" => vec![(old_launchers_root, new_launchers_root)],
        _ => Vec::new(),
    }
}

#[tauri::command]
async fn open_path(app: tauri::AppHandle, path: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let path = Path::new(&path);
        repair_dustdesk_shortcut_if_needed(path)?;
        open_path_impl(path)?;
        settle_desktop_windows_after_launch(&app);
        Ok(())
    })
    .await
    .map_err(to_message)?
}

#[tauri::command]
async fn open_path_as_administrator(app: tauri::AppHandle, path: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let path = Path::new(&path);
        repair_dustdesk_shortcut_if_needed(path)?;
        open_path_as_administrator_impl(path)?;
        settle_desktop_windows_after_launch(&app);
        Ok(())
    })
    .await
    .map_err(to_message)?
}

#[tauri::command]
async fn start_all_launchers(app: tauri::AppHandle) -> Result<usize, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let store = AppStore::open().map_err(to_message)?;
        let launchers = store.load_launchers();
        let mut count = 0usize;
        for launcher in launchers.items {
            if launcher.path.trim().is_empty() {
                continue;
            }
            repair_dustdesk_shortcut_if_needed(Path::new(&launcher.path))?;
            open_with_shell(&launcher.path)?;
            keep_desktop_windows_behind_apps(&app);
            count += 1;
        }
        if count > 0 {
            settle_desktop_windows_after_launch(&app);
        }
        Ok(count)
    })
    .await
    .map_err(to_message)?
}

#[tauri::command]
async fn start_all_launchers_as_administrator(app: tauri::AppHandle) -> Result<usize, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let store = AppStore::open().map_err(to_message)?;
        let launchers = store.load_launchers();
        let mut paths = Vec::new();
        for launcher in launchers.items {
            if launcher.path.trim().is_empty() {
                continue;
            }
            let path = PathBuf::from(&launcher.path);
            repair_dustdesk_shortcut_if_needed(&path)?;
            paths.push(path);
        }
        if paths.is_empty() {
            return Ok(0);
        }

        open_paths_as_administrator(&paths)?;
        settle_desktop_windows_after_launch(&app);
        Ok(paths.len())
    })
    .await
    .map_err(to_message)?
}

#[tauri::command]
async fn show_desktop_widget(app: tauri::AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || show_persisted_desktop_layout(&app))
        .await
        .map_err(to_message)?
}

#[tauri::command]
async fn desktop_frame_visibility(app: tauri::AppHandle) -> Result<DesktopFrameVisibility, String> {
    tauri::async_runtime::spawn_blocking(move || Ok(desktop_frame_visibility_impl(&app)))
        .await
        .map_err(to_message)?
}

#[tauri::command]
async fn desktop_window_state() -> Result<DesktopWindowState, String> {
    tauri::async_runtime::spawn_blocking(load_desktop_window_state)
        .await
        .map_err(to_message)?
}

#[tauri::command]
async fn set_desktop_windows_locked(
    app: tauri::AppHandle,
    locked: bool,
) -> Result<DesktopWindowState, String> {
    tauri::async_runtime::spawn_blocking(move || {
        set_desktop_window_state_impl(&app, Some(locked), None)
    })
    .await
    .map_err(to_message)?
}

#[tauri::command]
async fn set_desktop_windows_click_through(
    app: tauri::AppHandle,
    click_through: bool,
) -> Result<DesktopWindowState, String> {
    tauri::async_runtime::spawn_blocking(move || {
        set_desktop_window_state_impl(&app, None, Some(click_through))
    })
    .await
    .map_err(to_message)?
}

#[tauri::command]
async fn toggle_desktop_frames(app: tauri::AppHandle) -> Result<DesktopFrameVisibility, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let visibility = desktop_frame_visibility_impl(&app);
        if visibility.any {
            hide_desktop_card_windows(&app);
        } else {
            show_persisted_desktop_layout(&app)?;
        }
        Ok(desktop_frame_visibility_impl(&app))
    })
    .await
    .map_err(to_message)?
}

#[tauri::command]
async fn toggle_desktop_organizer_frame(
    app: tauri::AppHandle,
) -> Result<DesktopFrameVisibility, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let visibility = desktop_frame_visibility_impl(&app);
        if visibility.organizer {
            hide_desktop_organizer_windows(&app);
        } else {
            show_persisted_desktop_organizer(&app)?;
        }
        Ok(desktop_frame_visibility_impl(&app))
    })
    .await
    .map_err(to_message)?
}

#[tauri::command]
async fn toggle_desktop_launcher_frame(
    app: tauri::AppHandle,
) -> Result<DesktopFrameVisibility, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let visibility = desktop_frame_visibility_impl(&app);
        if visibility.launcher {
            hide_desktop_launcher_window(&app);
        } else {
            show_desktop_launcher(&app)?;
        }
        Ok(desktop_frame_visibility_impl(&app))
    })
    .await
    .map_err(to_message)?
}

#[tauri::command]
async fn save_desktop_window_layout(app: tauri::AppHandle, label: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        if !is_desktop_card_window_label(&label) {
            return Ok(());
        }
        let Some(window) = app.get_webview_window(&label) else {
            return Ok(());
        };
        if window.is_maximized().unwrap_or(false) {
            return Ok(());
        }
        let layout = capture_desktop_window_layout(&window).map_err(to_message)?;
        if !is_valid_desktop_window_layout(&layout) {
            return Ok(());
        }

        with_config_mutation(|| {
            let store = AppStore::open().map_err(to_message)?;
            let mut config = store.load_config_strict().map_err(to_message)?;
            config.desktop_layout.windows.insert(label, layout);
            store.save_config(&config).map_err(to_message)
        })
    })
    .await
    .map_err(to_message)?
}

#[tauri::command]
async fn save_desktop_split_indices(indices: Vec<usize>) -> Result<Vec<usize>, String> {
    tauri::async_runtime::spawn_blocking(move || save_desktop_split_indices_impl(indices))
        .await
        .map_err(to_message)?
}

fn show_merged_desktop_widget(app: &tauri::AppHandle) -> Result<(), String> {
    for target in desktop_monitor_targets(app).map_err(to_message)? {
        show_or_create_desktop_widget(app, &target)?;
    }
    Ok(())
}

fn show_persisted_desktop_organizer(app: &tauri::AppHandle) -> Result<(), String> {
    let store = AppStore::open().map_err(to_message)?;
    store.ensure_runtime_dirs().map_err(to_message)?;
    let config = store.load_config();
    let categories = config.desktop_categories;
    let split_indices = normalize_desktop_split_indices(
        config.desktop_layout.split_category_indices,
        categories.len(),
    );

    if split_indices.is_empty() {
        show_merged_desktop_widget(app)?;
    } else {
        let hide_merged_after_split = split_indices.len() >= categories.len();
        if split_indices.len() < categories.len() {
            show_merged_desktop_widget(app)?;
        }

        for index in split_indices.iter().copied() {
            let Some(category) = categories.get(index) else {
                continue;
            };
            show_desktop_category_on_all_monitors(app, index, category)?;
        }
        if hide_merged_after_split {
            hide_desktop_widget_windows(app);
        }
    }

    Ok(())
}

fn show_persisted_desktop_layout(app: &tauri::AppHandle) -> Result<(), String> {
    show_persisted_desktop_organizer(app)?;

    show_desktop_launcher(app)
}

#[tauri::command]
async fn split_desktop_widgets(app: tauri::AppHandle) -> Result<Vec<usize>, String> {
    tauri::async_runtime::spawn_blocking(move || show_split_desktop_widgets(&app))
        .await
        .map_err(to_message)?
}

#[tauri::command]
async fn split_desktop_category(app: tauri::AppHandle, index: usize) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || split_desktop_category_impl(&app, index))
        .await
        .map_err(to_message)?
}

fn split_desktop_category_impl(app: &tauri::AppHandle, index: usize) -> Result<(), String> {
    let store = AppStore::open().map_err(to_message)?;
    store.ensure_runtime_dirs().map_err(to_message)?;
    let category = with_config_mutation(|| {
        let mut config = store.load_config_strict().map_err(to_message)?;
        let category = config
            .desktop_categories
            .get(index)
            .cloned()
            .ok_or_else(|| "分类不存在".to_owned())?;
        let mut split_indices = config.desktop_layout.split_category_indices.clone();
        split_indices.push(index);
        config.desktop_layout.split_category_indices =
            normalize_desktop_split_indices(split_indices, config.desktop_categories.len());
        store.save_config(&config).map_err(to_message)?;
        Ok(category)
    })?;

    show_desktop_category_on_all_monitors(app, index, &category)?;
    show_desktop_launcher(app)?;
    Ok(())
}

#[tauri::command]
async fn merge_desktop_category(app: tauri::AppHandle, index: usize) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || merge_desktop_category_impl(&app, index))
        .await
        .map_err(to_message)?
}

fn merge_desktop_category_impl(app: &tauri::AppHandle, index: usize) -> Result<(), String> {
    remove_desktop_split_index(index)?;
    hide_desktop_category_windows(app, index);
    show_merged_desktop_widget(app)?;
    show_desktop_launcher(app)?;
    let _ = app.emit("dustdesk://desktop-cards-changed", ());
    Ok(())
}

fn show_split_desktop_widgets(app: &tauri::AppHandle) -> Result<Vec<usize>, String> {
    let store = AppStore::open().map_err(to_message)?;
    store.ensure_runtime_dirs().map_err(to_message)?;
    let categories = with_config_mutation(|| {
        let mut config = store.load_config_strict().map_err(to_message)?;
        repair_category_item_paths(&store, &mut config)?;
        Ok(config.desktop_categories)
    })?;
    let split_indices = categories
        .iter()
        .enumerate()
        .filter_map(|(index, category)| (!category.item_paths.is_empty()).then_some(index))
        .collect::<Vec<_>>();

    if split_indices.is_empty() {
        show_desktop_launcher(app)?;
        return Ok(split_indices);
    }

    let mut shown_indices = Vec::new();
    for index in split_indices.iter().copied() {
        let Some(category) = categories.get(index) else {
            continue;
        };
        if let Err(error) = show_desktop_category_on_all_monitors(app, index, category) {
            for shown_index in shown_indices {
                hide_desktop_category_windows(app, shown_index);
            }
            return Err(error);
        }
        shown_indices.push(index);
        std::thread::sleep(std::time::Duration::from_millis(120));
    }

    if let Err(error) = show_desktop_launcher(app) {
        for shown_index in shown_indices {
            hide_desktop_category_windows(app, shown_index);
        }
        return Err(error);
    }

    with_config_mutation(|| {
        let mut config = store.load_config_strict().map_err(to_message)?;
        config.desktop_layout.split_category_indices =
            normalize_desktop_split_indices(split_indices.clone(), config.desktop_categories.len());
        store.save_config(&config).map_err(to_message)
    })?;

    hide_desktop_widget_windows(app);

    for window in app.webview_windows().values() {
        let label = window.label();
        let is_legacy_category_window = is_legacy_desktop_category_window_label(label);
        if is_legacy_category_window {
            let _ = window.hide();
        }
    }

    Ok(split_indices)
}

fn save_desktop_split_indices_impl(indices: Vec<usize>) -> Result<Vec<usize>, String> {
    with_config_mutation(|| {
        let store = AppStore::open().map_err(to_message)?;
        let mut config = store.load_config_strict().map_err(to_message)?;
        let split_indices =
            normalize_desktop_split_indices(indices, config.desktop_categories.len());
        config.desktop_layout.split_category_indices = split_indices.clone();
        store.save_config(&config).map_err(to_message)?;
        Ok(split_indices)
    })
}

fn remove_desktop_split_index(index: usize) -> Result<Vec<usize>, String> {
    with_config_mutation(|| {
        let store = AppStore::open().map_err(to_message)?;
        let mut config = store.load_config_strict().map_err(to_message)?;
        let split_indices = config
            .desktop_layout
            .split_category_indices
            .iter()
            .copied()
            .filter(|item| *item != index)
            .collect::<Vec<_>>();
        let split_indices =
            normalize_desktop_split_indices(split_indices, config.desktop_categories.len());
        config.desktop_layout.split_category_indices = split_indices.clone();
        store.save_config(&config).map_err(to_message)?;
        Ok(split_indices)
    })
}

fn normalize_desktop_split_indices(indices: Vec<usize>, category_count: usize) -> Vec<usize> {
    let mut unique = indices
        .into_iter()
        .filter(|index| *index < category_count)
        .collect::<Vec<_>>();
    unique.sort_unstable();
    unique.dedup();
    unique
}

#[tauri::command]
async fn merge_desktop_widgets(app: tauri::AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        save_desktop_split_indices_impl(Vec::new())?;
        for window in app.webview_windows().values() {
            let label = window.label();
            if label.starts_with("desktop-category-") || is_desktop_launcher_window_label(label) {
                let _ = window.hide();
            }
        }
        show_merged_desktop_widget(&app)?;
        show_desktop_launcher(&app)?;
        let _ = app.emit("dustdesk://desktop-cards-changed", ());
        Ok(())
    })
    .await
    .map_err(to_message)?
}

#[tauri::command]
fn load_search_overlay() -> Result<SearchOverlayData, String> {
    let store = AppStore::open().map_err(to_message)?;
    store.ensure_runtime_dirs().map_err(to_message)?;
    let config = store.load_config();
    let paths = effective_search_paths(&store, &config.settings);
    let history = store.load_search_history();

    let mut recent = search_history_items(&history);
    recent.sort_by(|left, right| right.last_opened_at.cmp(&left.last_opened_at));
    recent.truncate(30);

    let mut frequent = search_history_items(&history);
    frequent.sort_by(|left, right| {
        right
            .open_count
            .cmp(&left.open_count)
            .then_with(|| right.last_opened_at.cmp(&left.last_opened_at))
    });
    frequent.truncate(30);

    Ok(SearchOverlayData {
        settings: config.settings,
        paths,
        recent,
        frequent,
    })
}

#[tauri::command]
fn search_items(query: String) -> Result<Vec<SearchItem>, String> {
    let query = query.trim().to_owned();
    if query.is_empty() {
        return Ok(Vec::new());
    }

    let store = AppStore::open().map_err(to_message)?;
    store.ensure_runtime_dirs().map_err(to_message)?;
    let config = store.load_config();
    if !config.settings.search_enabled {
        return Ok(Vec::new());
    }

    let query_lower = query.to_lowercase();
    let mut unique = BTreeMap::<String, SearchItem>::new();

    add_matching_launchers(&store, &query_lower, &mut unique);
    add_matching_paths(
        &effective_search_paths(&store, &config.settings),
        &query_lower,
        &mut unique,
    );

    let mut items = unique.into_values().collect::<Vec<_>>();
    items.sort_by(|left, right| {
        search_type_rank(left)
            .cmp(&search_type_rank(right))
            .then_with(|| {
                rank_search_item(left, &query_lower).cmp(&rank_search_item(right, &query_lower))
            })
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });
    items.truncate(SEARCH_RESULT_LIMIT);
    Ok(items)
}

#[tauri::command]
async fn open_search_item(app: tauri::AppHandle, item: SearchItem) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let path = item.path.trim();
        if path.is_empty() {
            return Err("路径为空".to_owned());
        }
        repair_dustdesk_shortcut_if_needed(Path::new(path))?;

        if item.kind == SearchItemKind::Directory || item.is_dir {
            open_path_impl(Path::new(path))?;
        } else {
            open_with_shell(path)?;
        }
        record_search_open(&item)?;

        with_lazy_window_operation(|| {
            hide_search_overlay_impl(&app);
            Ok(())
        })?;
        settle_desktop_windows_after_launch(&app);
        Ok(())
    })
    .await
    .map_err(to_message)?
}

#[tauri::command]
async fn hide_search_overlay(app: tauri::AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        with_lazy_window_operation(|| {
            hide_search_overlay_impl(&app);
            Ok(())
        })
    })
    .await
    .map_err(to_message)?
}

#[tauri::command]
async fn hide_current_window(window: WebviewWindow) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let is_desktop_window = is_desktop_card_window_label(window.label());
        window.hide().map_err(to_message)?;
        if is_desktop_window {
            sync_system_tray_menu(window.app_handle());
        }
        Ok(())
    })
    .await
    .map_err(to_message)?
}

#[tauri::command]
async fn hide_main_window_to_tray(app: tauri::AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        with_lazy_window_operation(|| hide_main_window_impl(&app))
    })
    .await
    .map_err(to_message)?
}

#[tauri::command]
async fn cleanup_desktop_card_windows(app: tauri::AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        cleanup_desktop_card_windows_impl(&app);
        Ok(())
    })
    .await
    .map_err(to_message)?
}

#[tauri::command]
async fn repaint_current_window(window: WebviewWindow) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let size = window.inner_size().map_err(to_message)?;
        let width = size.width;
        let height = size.height;
        if width == 0 || height == 0 {
            return Ok(());
        }

        window
            .set_size(Size::Physical(PhysicalSize::new(
                width.saturating_add(1),
                height,
            )))
            .map_err(to_message)?;
        window
            .set_size(Size::Physical(PhysicalSize::new(width, height)))
            .map_err(to_message)
    })
    .await
    .map_err(to_message)?
}

#[tauri::command]
async fn paste_clipboard_item(app: tauri::AppHandle, id: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        clipboard_bridge::paste_history_item(&id, || {
            let _ = with_lazy_window_operation(|| {
                hide_clipboard_overlay_impl(&app);
                Ok(())
            });
        })
    })
    .await
    .map_err(to_message)?
}

#[tauri::command]
async fn hide_clipboard_overlay(app: tauri::AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        with_lazy_window_operation(|| {
            hide_clipboard_overlay_impl(&app);
            Ok(())
        })
    })
    .await
    .map_err(to_message)?
}

#[tauri::command]
async fn clipboard_image_base64(id: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || clipboard_bridge::image_base64(&id))
        .await
        .map_err(to_message)?
}

#[tauri::command]
fn update_clipboard_shortcut(
    app: tauri::AppHandle,
    shortcut: String,
) -> Result<AppSettings, String> {
    with_config_mutation(|| {
        let shortcut = normalize_shortcut_input(&shortcut)?;
        let store = AppStore::open().map_err(to_message)?;
        let mut config = store.load_config_strict().map_err(to_message)?;
        if config.settings.search_enabled && shortcut == config.settings.search_shortcut_value() {
            return Err("剪贴板快捷键不能和搜索快捷键相同".to_owned());
        }

        let previous = config.settings.clipboard_shortcut_value();

        replace_global_shortcut(&app, Some(&previous), Some(&shortcut))?;

        config.settings.clipboard_shortcut = shortcut;
        if let Err(error) = store.save_config(&config) {
            let _ = replace_global_shortcut(
                &app,
                Some(&config.settings.clipboard_shortcut),
                Some(&previous),
            );
            return Err(to_message(error));
        }

        Ok(config.settings)
    })
}

#[tauri::command]
async fn update_clipboard_history_limit(limit: usize) -> Result<AppSettings, String> {
    tauri::async_runtime::spawn_blocking(move || {
        if !(MIN_CLIPBOARD_HISTORY_LIMIT..=MAX_CLIPBOARD_HISTORY_LIMIT).contains(&limit) {
            return Err(format!(
                "剪贴板最大保留条数必须在 {MIN_CLIPBOARD_HISTORY_LIMIT} 到 {MAX_CLIPBOARD_HISTORY_LIMIT} 之间"
            ));
        }

        let settings = with_config_mutation(|| {
            let store = AppStore::open().map_err(to_message)?;
            let mut config = store.load_config_strict().map_err(to_message)?;
            config.settings.clipboard_history_limit = limit;
            store.save_config(&config).map_err(to_message)?;
            Ok(config.settings)
        })?;

        // The config transaction has ended before taking the clipboard lock, so settings and
        // history writes keep a consistent lock order.
        clipboard_bridge::enforce_history_limit(settings.clipboard_history_limit_value())?;
        Ok(settings)
    })
    .await
    .map_err(to_message)?
}

#[tauri::command]
fn update_search_settings(
    app: tauri::AppHandle,
    enabled: bool,
    shortcut: String,
    paths: Vec<String>,
) -> Result<AppSettings, String> {
    with_config_mutation(|| {
        let shortcut = if shortcut.trim().is_empty() {
            AppSettings::default().search_shortcut
        } else {
            normalize_shortcut_input(&shortcut)?
        };
        let store = AppStore::open().map_err(to_message)?;
        store.ensure_runtime_dirs().map_err(to_message)?;
        let mut config = store.load_config_strict().map_err(to_message)?;

        if enabled && shortcut == config.settings.clipboard_shortcut_value() {
            return Err("搜索快捷键不能和剪贴板快捷键相同".to_owned());
        }

        let previous_enabled = config.settings.search_enabled;
        let previous_shortcut = config.settings.search_shortcut_value();
        let next_paths = normalize_search_paths(paths);
        let previous_registered = previous_enabled.then_some(previous_shortcut.as_str());
        let next_registered = enabled.then_some(shortcut.as_str());

        replace_global_shortcut(&app, previous_registered, next_registered)?;

        config.settings.search_enabled = enabled;
        config.settings.search_shortcut = shortcut;
        config.settings.search_paths = next_paths;

        if let Err(error) = store.save_config(&config) {
            let rollback_next = enabled.then_some(config.settings.search_shortcut.as_str());
            let rollback_previous = previous_enabled.then_some(previous_shortcut.as_str());
            let _ = replace_global_shortcut(&app, rollback_next, rollback_previous);
            return Err(to_message(error));
        }

        Ok(config.settings)
    })
}

#[tauri::command]
async fn update_launch_on_startup(enabled: bool) -> Result<AppSettings, String> {
    tauri::async_runtime::spawn_blocking(move || {
        with_config_mutation(|| {
            let store = AppStore::open().map_err(to_message)?;
            store.ensure_runtime_dirs().map_err(to_message)?;
            let mut config = store.load_config_strict().map_err(to_message)?;
            set_launch_on_startup_entry(enabled)?;
            config.settings.launch_on_startup = enabled;
            store.save_config(&config).map_err(to_message)?;
            Ok(config.settings)
        })
    })
    .await
    .map_err(to_message)?
}

fn with_config_mutation<T>(operation: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    store::with_storage_mutation(|| {
        let _guard = CONFIG_MUTATION_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .map_err(|_| "配置写入锁已损坏".to_owned())?;
        operation()
    })
}

fn with_launcher_mutation<T>(operation: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    store::with_storage_mutation(|| {
        let _guard = LAUNCHER_MUTATION_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .map_err(|_| "快捷启动写入锁已损坏".to_owned())?;
        operation()
    })
}

fn mutate_categories(
    mutator: impl FnOnce(&mut Vec<DeskCategory>) -> Result<(), String>,
) -> Result<(), String> {
    with_config_mutation(|| {
        let store = AppStore::open().map_err(to_message)?;
        let mut config = store.load_config_strict().map_err(to_message)?;
        mutator(&mut config.desktop_categories)?;
        store.save_config(&config).map_err(to_message)
    })
}

fn remove_category_path_from_config(
    config: &mut AppConfig,
    index: usize,
    path: &str,
) -> Result<(), String> {
    let category = config
        .desktop_categories
        .get_mut(index)
        .ok_or_else(|| "分类不存在".to_owned())?;
    category
        .item_paths
        .retain(|item_path| !same_path_text(item_path, path));
    Ok(())
}

fn restore_desktop_organization_for_exit() -> Result<usize, String> {
    restore_desktop_organization_for_exit_with_progress(|_, _, _| {})
}

fn restore_desktop_organization_for_exit_with_progress(
    mut on_progress: impl FnMut(usize, usize, &Path),
) -> Result<usize, String> {
    with_config_mutation(|| {
        let store = AppStore::open().map_err(to_message)?;
        store.ensure_runtime_dirs().map_err(to_message)?;
        recover_transfer_quarantine_journals_strict(false)?;
        let desktop = user_desktop().ok_or_else(|| "没有找到桌面路径".to_owned())?;
        let persistence = AppDesktopOrganizationRestartPersistence;
        let result = restore_desktop_organization_for_exit_core_with_progress(
            &store,
            &desktop,
            &persistence,
            &mut on_progress,
        )
        .and_then(|outcome| finish_exit_desktop_restore_outcome(outcome, true));

        // Cross-volume directory moves commit before their quarantine cleanup. Do not let the
        // process exit while a newly restored desktop destination still has an active journal.
        let recovery_result = recover_transfer_quarantine_journals_strict(false);
        match (result, recovery_result) {
            (Ok(restored), Ok(())) => Ok(restored),
            (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
            (Err(error), Err(recovery_error)) => {
                Err(format!("{error}；退出迁移日志收尾失败：{recovery_error}"))
            }
        }
    })
}

#[cfg(test)]
fn restore_desktop_organization_for_exit_core(
    store: &AppStore,
    desktop: &Path,
    persistence: &impl DesktopOrganizationRestartPersistence,
) -> Result<usize, String> {
    restore_desktop_organization_for_exit_core_with_progress(
        store,
        desktop,
        persistence,
        &mut |_, _, _| {},
    )
    .and_then(|outcome| finish_exit_desktop_restore_outcome(outcome, false))
}

fn finish_exit_desktop_restore_outcome(
    outcome: ExitDesktopRestoreOutcome,
    apply_icon_positions: bool,
) -> Result<usize, String> {
    if apply_icon_positions {
        position_desktop_icons(&outcome.icon_positions);
    }
    if outcome.restore_errors.is_empty() {
        Ok(outcome.restored)
    } else {
        Err(format!(
            "有 {} 个收纳项目未能还原到桌面：{}",
            outcome.restore_errors.len(),
            outcome.restore_errors.join("；")
        ))
    }
}

fn restore_desktop_organization_for_exit_core_with_progress(
    store: &AppStore,
    desktop: &Path,
    persistence: &impl DesktopOrganizationRestartPersistence,
    on_progress: &mut impl FnMut(usize, usize, &Path),
) -> Result<ExitDesktopRestoreOutcome, String> {
    store.ensure_runtime_dirs().map_err(to_message)?;
    fs::create_dir_all(desktop).map_err(to_message)?;
    let mut config = store.load_config_strict().map_err(to_message)?;
    repair_category_item_paths_for_desktop(store, &mut config, desktop)?;

    let existing_marker = persistence.load()?;
    let mut marker =
        prepare_exit_restart_marker(&config, &store.organizer_root(), desktop, existing_marker)?;
    if marker.is_empty() {
        clear_desktop_organization_restart_marker(persistence)?;
        store.save_config(&config).map_err(to_message)?;
        return Ok(ExitDesktopRestoreOutcome::default());
    }

    // The complete intent is durable before the first physical move.
    persistence.save(&marker)?;
    let organizer_root = store.organizer_root();
    let mut restored_sources = Vec::<String>::new();
    let mut restore_errors = Vec::<String>::new();
    let mut icon_positions = Vec::<(PathBuf, DesktopIconPosition)>::new();
    let mut restored = 0usize;
    let total = marker
        .categories
        .iter()
        .flat_map(|category| &category.items)
        .filter(|item| {
            valid_restart_source_path(Path::new(&item.source_path), &organizer_root)
                && !item
                    .desktop_path
                    .as_deref()
                    .map(PathBuf::from)
                    .is_some_and(|path| valid_restart_desktop_path(&path, desktop))
        })
        .count();

    for category_index in 0..marker.categories.len() {
        for item_index in 0..marker.categories[category_index].items.len() {
            let source = PathBuf::from(
                marker.categories[category_index].items[item_index]
                    .source_path
                    .clone(),
            );
            if !valid_restart_source_path(&source, &organizer_root) {
                continue;
            }
            if marker.categories[category_index].items[item_index]
                .desktop_path
                .as_deref()
                .map(PathBuf::from)
                .is_some_and(|path| valid_restart_desktop_path(&path, desktop))
            {
                // Both copies exist and paths alone cannot prove which one owns the marker. Keep
                // the ambiguity durable instead of moving or deleting either copy automatically.
                eprintln!(
                    "keeping ambiguous restart marker because organizer and desktop copies both exist: {}",
                    source.display()
                );
                continue;
            }

            let mut moved = false;
            let error_count_before_move = restore_errors.len();
            for _ in 0..1000 {
                let destination = unique_destination(desktop, &source);
                marker.categories[category_index].items[item_index].desktop_path =
                    Some(destination.display().to_string());
                // Persist the exact no-overwrite target before moving this item.
                persistence.save(&marker)?;

                match move_path(&source, &destination) {
                    Ok(()) => {
                        notify_shell_path_moved(&source, &destination);
                        if let Some(position) = marker.categories[category_index].items[item_index]
                            .desktop_icon_position
                            .clone()
                        {
                            icon_positions.push((destination.clone(), position));
                        }
                        restored_sources.push(source.display().to_string());
                        restored += 1;
                        on_progress(restored, total, &destination);
                        moved = true;
                        break;
                    }
                    Err(error)
                        if source.exists()
                            && destination.exists()
                            && error.contains("目标项目已存在") =>
                    {
                        // An external process won the destination race. Never leave that unrelated
                        // desktop path marked as ours; choose and persist a fresh target.
                        marker.categories[category_index].items[item_index].desktop_path = None;
                        persistence.save(&marker)?;
                    }
                    Err(error) => {
                        if source.exists() && !destination.exists() {
                            marker.categories[category_index].items[item_index].desktop_path = None;
                            persistence.save(&marker)?;
                        }
                        eprintln!(
                            "failed to restore desktop item {}: {error}",
                            source.display()
                        );
                        restore_errors.push(format!("{}: {error}", source.display()));
                        break;
                    }
                }
            }

            if !moved {
                if restore_errors.len() == error_count_before_move {
                    restore_errors.push(format!(
                        "{}: 无法生成无冲突的桌面目标路径",
                        source.display()
                    ));
                }
                continue;
            }
        }
    }

    for category in &mut config.desktop_categories {
        category.item_paths.retain(|item_path| {
            !restored_sources
                .iter()
                .any(|source| same_path_text(source, item_path))
        });
    }
    for source in &restored_sources {
        remove_desktop_icon_position_for_path(&mut config.desktop_icon_positions, source);
    }
    store.save_config(&config).map_err(to_message)?;
    cleanup_empty_organizer_dirs(&organizer_root);
    Ok(ExitDesktopRestoreOutcome {
        restored,
        icon_positions,
        restore_errors,
    })
}

fn recollect_desktop_organization_from_restart_marker_with_progress(
    mut on_progress: impl FnMut(usize, usize, &Path),
) -> Result<usize, String> {
    with_config_mutation(|| {
        let store = AppStore::open().map_err(to_message)?;
        store.ensure_runtime_dirs().map_err(to_message)?;
        let desktop = user_desktop().ok_or_else(|| "没有找到桌面路径".to_owned())?;
        let persistence = AppDesktopOrganizationRestartPersistence;
        // Always recover uncommitted transfers before reading or parsing lifecycle metadata.
        recover_transfer_quarantine_journals_strict(true)?;
        let marker_content = AppStore::load_desktop_organization_restart().map_err(to_message)?;

        let marker = marker_content
            .map(|content| serde_json::from_str(&content).map_err(to_message))
            .transpose()?;
        // A marked destination must remain in place until committed quarantine records that refer
        // to it are cleaned. Unrelated committed cleanup stays deferred so a large old transfer
        // cannot delay normal startup.
        if let Some(marker) = &marker {
            let related_paths = restart_marker_paths(marker);
            recover_related_transfer_quarantine_journals_strict(&related_paths)?;
        }
        recollect_desktop_organization_from_restart_marker_core_with_progress(
            &store,
            &desktop,
            marker,
            &persistence,
            &mut on_progress,
        )
    })
}

#[cfg(test)]
fn recollect_desktop_organization_from_restart_marker_core(
    store: &AppStore,
    desktop: &Path,
    marker: Option<DesktopOrganizationRestartMarker>,
    persistence: &impl DesktopOrganizationRestartPersistence,
) -> Result<usize, String> {
    recollect_desktop_organization_from_restart_marker_core_with_progress(
        store,
        desktop,
        marker,
        persistence,
        &mut |_, _, _| {},
    )
}

fn recollect_desktop_organization_from_restart_marker_core_with_progress(
    store: &AppStore,
    desktop: &Path,
    marker: Option<DesktopOrganizationRestartMarker>,
    persistence: &impl DesktopOrganizationRestartPersistence,
    on_progress: &mut impl FnMut(usize, usize, &Path),
) -> Result<usize, String> {
    if let Some(marker) = &marker {
        validate_restart_marker(marker)?;
    }

    store.ensure_runtime_dirs().map_err(to_message)?;
    let mut config = store.load_config_strict().map_err(to_message)?;
    repair_category_item_paths_for_desktop(store, &mut config, desktop)?;
    let Some(marker) = marker else {
        store.save_config(&config).map_err(to_message)?;
        return Ok(0);
    };

    let organizer_root = store.organizer_root();
    let mut remaining = DesktopOrganizationRestartMarker::empty();
    let mut recollected = 0usize;
    let total = marker
        .categories
        .iter()
        .map(|category| category.items.len())
        .sum();
    let mut processed = 0usize;
    let mut claimed_category_indices = Vec::new();

    for category_marker in marker.categories {
        let target_category_index =
            restart_category_index(&mut config, &category_marker, &mut claimed_category_indices);
        let mut failed_items = Vec::new();

        for mut item in category_marker.items {
            processed += 1;
            let source = PathBuf::from(&item.source_path);
            let desktop_path = item.desktop_path.as_deref().map(PathBuf::from);
            let source_exists = valid_restart_source_path(&source, &organizer_root);
            let desktop_source = desktop_path
                .as_ref()
                .filter(|path| valid_restart_desktop_path(path, desktop));
            let desktop_icon_position = item
                .desktop_icon_position
                .clone()
                .or_else(|| desktop_source.and_then(|path| capture_desktop_icon_position(path)));
            if item.desktop_icon_position.is_none() {
                item.desktop_icon_position = desktop_icon_position.clone();
            }

            if source_exists && desktop_source.is_some() {
                // Paths alone cannot distinguish a destination race from an externally recreated
                // organizer source. Preserve both and retry rather than collecting the wrong item.
                on_progress(
                    processed,
                    total,
                    desktop_source.expect("desktop source checked"),
                );
                failed_items.push(item);
                continue;
            }

            if source_exists {
                remove_restart_item_paths_from_config(
                    &mut config,
                    &item.source_path,
                    item.desktop_path.as_deref(),
                );
                push_unique_text_path(
                    &mut config.desktop_categories[target_category_index].item_paths,
                    source.display().to_string(),
                );
                if let Some(position) = desktop_icon_position {
                    set_desktop_icon_position_for_path(
                        &mut config.desktop_icon_positions,
                        source.display().to_string(),
                        position,
                    );
                }
                recollected += 1;
                on_progress(processed, total, &source);
                continue;
            }

            if let Some(desktop_source) = desktop_source {
                match recollect_marked_desktop_item(store, &source, desktop_source) {
                    Ok(archived_path) => {
                        let archived_path_buf = PathBuf::from(&archived_path);
                        remove_restart_item_paths_from_config(
                            &mut config,
                            &item.source_path,
                            item.desktop_path.as_deref(),
                        );
                        push_unique_text_path(
                            &mut config.desktop_categories[target_category_index].item_paths,
                            archived_path.clone(),
                        );
                        if let Some(position) = desktop_icon_position {
                            set_desktop_icon_position_for_path(
                                &mut config.desktop_icon_positions,
                                archived_path,
                                position,
                            );
                        }
                        recollected += 1;
                        on_progress(processed, total, &archived_path_buf);
                    }
                    Err(error) => {
                        eprintln!(
                            "failed to recollect desktop item {}: {error}",
                            desktop_source.display()
                        );
                        on_progress(processed, total, desktop_source);
                        failed_items.push(item);
                    }
                }
                continue;
            }

            let desktop_path_still_exists = desktop_path.as_ref().is_some_and(|path| path.exists());
            if source.exists() || desktop_path_still_exists {
                // Existing but invalid paths are never moved automatically; retain them for a
                // future retry instead of losing the user's restart intent.
                failed_items.push(item);
            }
            on_progress(processed, total, &source);
        }

        if !failed_items.is_empty() {
            remaining.categories.push(RestartCategoryMarker {
                category_name: category_marker.category_name,
                category_index: category_marker.category_index,
                items: failed_items,
            });
        }
    }

    store.save_config(&config).map_err(to_message)?;
    if remaining.is_empty() {
        clear_desktop_organization_restart_marker(persistence)?;
    } else {
        persistence.save(&remaining)?;
    }
    Ok(recollected)
}

fn prepare_exit_restart_marker(
    config: &AppConfig,
    organizer_root: &Path,
    desktop: &Path,
    existing: Option<DesktopOrganizationRestartMarker>,
) -> Result<DesktopOrganizationRestartMarker, String> {
    let mut marker = existing.unwrap_or_else(DesktopOrganizationRestartMarker::empty);
    validate_restart_marker(&marker)?;

    for category in &mut marker.categories {
        category.items.retain(|item| {
            let source = PathBuf::from(&item.source_path);
            let source_exists = valid_restart_source_path(&source, organizer_root);
            let desktop_exists = item
                .desktop_path
                .as_deref()
                .map(PathBuf::from)
                .is_some_and(|path| valid_restart_desktop_path(&path, desktop));
            source_exists || desktop_exists
        });
    }
    marker
        .categories
        .retain(|category| !category.items.is_empty());

    for (category_index, category) in config.desktop_categories.iter().enumerate() {
        for item_path in &category.item_paths {
            let source = recover_existing_path_from_corrupted_text(item_path)
                .unwrap_or_else(|| PathBuf::from(item_path));
            if !valid_restart_source_path(&source, organizer_root) {
                continue;
            }
            let existing_item = take_restart_marker_item(&mut marker, &source);

            let marker_category_index = marker
                .categories
                .iter()
                .position(|candidate| {
                    candidate.category_index == Some(category_index)
                        && candidate.category_name == category.name
                })
                .or_else(|| {
                    marker.categories.iter().position(|candidate| {
                        candidate.category_index.is_none()
                            && candidate.category_name == category.name
                    })
                })
                .unwrap_or_else(|| {
                    marker.categories.push(RestartCategoryMarker {
                        category_name: category.name.clone(),
                        category_index: Some(category_index),
                        items: Vec::new(),
                    });
                    marker.categories.len() - 1
                });
            let source_path = source.display().to_string();
            let mut marker_item = existing_item.unwrap_or_else(|| RestartItemMarker {
                source_path: source_path.clone(),
                desktop_path: None,
                desktop_icon_position: None,
            });
            if marker_item.desktop_icon_position.is_none() {
                marker_item.desktop_icon_position =
                    desktop_icon_position_for_path(&config.desktop_icon_positions, &source_path);
            }
            marker.categories[marker_category_index]
                .items
                .push(marker_item);
        }
    }

    marker
        .categories
        .retain(|category| !category.items.is_empty());

    Ok(marker)
}

fn validate_restart_marker(marker: &DesktopOrganizationRestartMarker) -> Result<(), String> {
    if marker.schema_version != DESKTOP_ORGANIZATION_RESTART_SCHEMA_VERSION {
        return Err(format!(
            "不支持的桌面收纳重启标记版本：{}",
            marker.schema_version
        ));
    }
    Ok(())
}

fn restart_marker_paths(marker: &DesktopOrganizationRestartMarker) -> Vec<PathBuf> {
    marker
        .categories
        .iter()
        .flat_map(|category| &category.items)
        .flat_map(|item| {
            std::iter::once(PathBuf::from(&item.source_path))
                .chain(item.desktop_path.as_deref().map(PathBuf::from))
        })
        .collect()
}

fn valid_restart_source_path(path: &Path, organizer_root: &Path) -> bool {
    path.exists()
        && !same_path_for_move(path, organizer_root)
        && is_path_within(path, organizer_root)
        && !is_internal_transfer_path(path)
        && !is_desktop_ini_path(path)
}

fn valid_restart_desktop_path(path: &Path, desktop: &Path) -> bool {
    path.exists()
        && !same_path_for_move(path, desktop)
        && is_path_within(path, desktop)
        && !is_internal_transfer_path(path)
        && !is_desktop_ini_path(path)
}

fn take_restart_marker_item(
    marker: &mut DesktopOrganizationRestartMarker,
    source: &Path,
) -> Option<RestartItemMarker> {
    for category in &mut marker.categories {
        if let Some(index) = category
            .items
            .iter()
            .position(|item| same_path_text(&item.source_path, &source.display().to_string()))
        {
            return Some(category.items.remove(index));
        }
    }
    None
}

fn restart_category_index(
    config: &mut AppConfig,
    marker: &RestartCategoryMarker,
    claimed_indices: &mut Vec<usize>,
) -> usize {
    if let Some(index) = marker.category_index {
        if config
            .desktop_categories
            .get(index)
            .is_some_and(|category| category.name == marker.category_name)
            && !claimed_indices.contains(&index)
        {
            claimed_indices.push(index);
            return index;
        }
    }
    if let Some(index) =
        config
            .desktop_categories
            .iter()
            .enumerate()
            .find_map(|(index, category)| {
                (category.name == marker.category_name && !claimed_indices.contains(&index))
                    .then_some(index)
            })
    {
        claimed_indices.push(index);
        return index;
    }

    config.desktop_categories.push(DeskCategory {
        name: normalize_name(&marker.category_name, config.desktop_categories.len() + 1),
        ..DeskCategory::default()
    });
    let index = config.desktop_categories.len() - 1;
    claimed_indices.push(index);
    index
}

fn recollect_marked_desktop_item(
    store: &AppStore,
    original_source: &Path,
    desktop_source: &Path,
) -> Result<String, String> {
    let organizer_root = store.organizer_root();
    if same_path_for_move(original_source, &organizer_root)
        || !is_path_within(original_source, &organizer_root)
        || is_internal_transfer_path(original_source)
        || is_desktop_ini_path(original_source)
    {
        return Err(format!(
            "重启标记中的收纳目标无效：{}",
            original_source.display()
        ));
    }
    let parent = original_source
        .parent()
        .ok_or_else(|| "无法识别原收纳分类目录".to_owned())?;
    fs::create_dir_all(parent).map_err(to_message)?;
    move_path(desktop_source, original_source)?;
    notify_shell_path_moved(desktop_source, original_source);
    repair_dustdesk_shortcut_best_effort(original_source);
    Ok(original_source.display().to_string())
}

fn remove_restart_item_paths_from_config(
    config: &mut AppConfig,
    source_path: &str,
    desktop_path: Option<&str>,
) {
    for category in &mut config.desktop_categories {
        category.item_paths.retain(|item_path| {
            !same_path_text(item_path, source_path)
                && desktop_path.is_none_or(|path| !same_path_text(item_path, path))
        });
    }
}

fn restore_all_organized_items_to_desktop_and_clear_markers() -> Result<usize, String> {
    restore_all_organized_items_to_desktop_with_progress(|_, _, _| {})
}

fn restore_all_organized_items_to_desktop_with_progress(
    on_progress: impl FnMut(usize, usize, &Path),
) -> Result<usize, String> {
    restore_all_organized_items_to_desktop_impl(on_progress)
}

fn restore_all_organized_items_to_desktop_impl(
    mut on_progress: impl FnMut(usize, usize, &Path),
) -> Result<usize, String> {
    let restore_result = with_config_mutation(|| {
        let store = AppStore::open().map_err(to_message)?;
        clear_desktop_organization_restart_marker(&AppDesktopOrganizationRestartPersistence)?;
        store.ensure_runtime_dirs().map_err(to_message)?;
        recover_transfer_quarantine_journals_strict(false)?;
        let desktop = user_desktop().ok_or_else(|| "没有找到桌面路径".to_owned())?;
        fs::create_dir_all(&desktop).map_err(to_message)?;
        let organizer_root = store.organizer_root();
        let (mut config, config_is_writable) = match store.load_config_strict() {
            Ok(config) => (config, true),
            Err(error) => {
                eprintln!("DustDesk 配置无法读取，仍将按收纳目录实物还原且不会覆盖原配置：{error}");
                (AppConfig::default(), false)
            }
        };
        let total = restore_candidate_count(&config, &organizer_root, &desktop);
        let mut seen_paths = Vec::<String>::new();
        let mut candidates = Vec::<RestoreCandidate>::new();
        let stored_icon_positions = config.desktop_icon_positions.clone();

        for (category_index, category) in config.desktop_categories.iter_mut().enumerate() {
            let old_paths = std::mem::take(&mut category.item_paths);
            let mut retained_paths = Vec::new();

            for item_path in old_paths {
                let source = recover_existing_path_from_corrupted_text(&item_path)
                    .unwrap_or_else(|| PathBuf::from(&item_path));
                if is_internal_transfer_path(&source) {
                    continue;
                }
                if !source.exists() {
                    continue;
                }

                if is_path_within(&source, &organizer_root) || is_path_within(&source, &desktop) {
                    let normalized = normalize_path_for_compare(&source);
                    if !seen_paths.iter().any(|path| path == &normalized) {
                        seen_paths.push(normalized);
                        let desktop_icon_position =
                            desktop_icon_position_for_path(&stored_icon_positions, &item_path)
                                .or_else(|| {
                                    desktop_icon_position_for_path(
                                        &stored_icon_positions,
                                        &source.display().to_string(),
                                    )
                                });
                        candidates.push(RestoreCandidate {
                            source,
                            original_config_path: Some(item_path),
                            category_index: Some(category_index),
                            desktop_icon_position,
                        });
                    }
                } else {
                    // External references are not migrated desktop items, so do not move them on exit.
                    retained_paths.push(item_path);
                }
            }

            category.item_paths = retained_paths;
        }

        for path in organizer_contents(&organizer_root) {
            if !path.exists() {
                continue;
            }
            let normalized = normalize_path_for_compare(&path);
            if seen_paths.iter().any(|item| item == &normalized) {
                continue;
            }
            seen_paths.push(normalized);
            candidates.push(RestoreCandidate {
                desktop_icon_position: desktop_icon_position_for_path(
                    &stored_icon_positions,
                    &path.display().to_string(),
                ),
                source: path,
                original_config_path: None,
                category_index: None,
            });
        }

        let worker_count = desktop_operation_worker_count(candidates.len());
        let queue = Arc::new(Mutex::new(VecDeque::from(candidates)));
        let (sender, receiver) = mpsc::channel();
        let mut restored = 0usize;
        let mut restored_icon_positions = Vec::<(PathBuf, DesktopIconPosition)>::new();

        std::thread::scope(|scope| -> Result<(), String> {
            for _ in 0..worker_count {
                let queue = Arc::clone(&queue);
                let sender = sender.clone();
                let desktop = desktop.clone();
                scope.spawn(move || loop {
                    let candidate = match queue.lock() {
                        Ok(mut queue) => queue.pop_front(),
                        Err(_) => {
                            let _ = sender.send(Err("还原任务队列已损坏".to_owned()));
                            return;
                        }
                    };
                    let Some(candidate) = candidate else {
                        return;
                    };
                    let result = restore_path_to_desktop_silent(&candidate.source, &desktop);
                    if sender.send(Ok((candidate, result))).is_err() {
                        return;
                    }
                });
            }
            drop(sender);

            for message in receiver {
                let (candidate, restore_result) = match message {
                    Ok(value) => value,
                    Err(error) => {
                        eprintln!("failed to restore desktop item: {error}");
                        continue;
                    }
                };

                match restore_result {
                    Ok(restored_path) => {
                        remove_desktop_icon_position_for_path(
                            &mut config.desktop_icon_positions,
                            &candidate.source.display().to_string(),
                        );
                        if let Some(original_path) = candidate.original_config_path.as_deref() {
                            remove_desktop_icon_position_for_path(
                                &mut config.desktop_icon_positions,
                                original_path,
                            );
                        }
                        if let Some(position) = candidate.desktop_icon_position {
                            restored_icon_positions.push((restored_path.clone(), position));
                        }
                        restored += 1;
                        on_progress(restored, total, &restored_path);
                    }
                    Err(error) => {
                        eprintln!(
                            "failed to restore desktop item {}: {error}",
                            candidate.source.display()
                        );
                        if let (Some(category_index), Some(original_path)) =
                            (candidate.category_index, candidate.original_config_path)
                        {
                            if let Some(category) =
                                config.desktop_categories.get_mut(category_index)
                            {
                                push_unique_text_path(&mut category.item_paths, original_path);
                            }
                        }
                    }
                }
            }
            Ok(())
        })?;

        cleanup_empty_organizer_dirs(&organizer_root);
        config.desktop_layout.split_category_indices.clear();
        retain_desktop_icon_positions_for_categories(&mut config);
        if config_is_writable {
            store.save_config(&config).map_err(to_message)?;
        }
        Ok((restored, desktop, organizer_root, restored_icon_positions))
    })?;
    let (restored, desktop, organizer_root, restored_icon_positions) = restore_result;

    if restored > 0 {
        notify_shell_directory_updated(&desktop);
    }
    position_desktop_icons(&restored_icon_positions);

    if restored > 0 {
        std::thread::spawn(move || {
            cleanup_stale_transfer_staging_dirs(&desktop, Duration::ZERO);
            cleanup_stale_transfer_staging_dirs(&organizer_root, Duration::ZERO);
            notify_shell_directory_updated(&desktop);
            notify_shell_directory_updated(&organizer_root);
        });
    }

    Ok(restored)
}

fn restore_candidate_count(config: &AppConfig, organizer_root: &Path, desktop: &Path) -> usize {
    let mut seen = Vec::<String>::new();
    for category in &config.desktop_categories {
        for item_path in &category.item_paths {
            let source = recover_existing_path_from_corrupted_text(item_path)
                .unwrap_or_else(|| PathBuf::from(item_path));
            if is_internal_transfer_path(&source) {
                continue;
            }
            if !source.exists() {
                continue;
            }
            if !is_path_within(&source, organizer_root) && !is_path_within(&source, desktop) {
                continue;
            }
            let normalized = normalize_path_for_compare(&source);
            if !seen.iter().any(|path| path == &normalized) {
                seen.push(normalized);
            }
        }
    }

    for path in organizer_contents(organizer_root) {
        if !path.exists() {
            continue;
        }
        let normalized = normalize_path_for_compare(&path);
        if !seen.iter().any(|item| item == &normalized) {
            seen.push(normalized);
        }
    }

    seen.len()
}

fn restore_progress_message(current: usize, total: usize, path: &Path) -> String {
    let name = display_path_name(path);
    if total > 0 {
        format!("正在还原 {current}/{total}：{name}")
    } else {
        format!("正在还原：{name}")
    }
}

fn classify_progress_message(current: usize, total: usize, path: &Path) -> String {
    let name = display_path_name(path);
    if total > 0 {
        format!("正在收纳 {current}/{total}：{name}")
    } else {
        format!("正在收纳：{name}")
    }
}

fn desktop_operation_worker_count(total: usize) -> usize {
    if total <= 1 {
        return total;
    }
    std::thread::available_parallelism()
        .map(|value| value.get())
        .unwrap_or(2)
        .clamp(1, 4)
        .min(total)
}

fn estimate_transfer_work(path: &Path) -> u64 {
    const ENTRY_OVERHEAD: u64 = 64 * 1024;

    let mut score = 0u64;
    let mut pending = vec![path.to_path_buf()];
    while let Some(current) = pending.pop() {
        score = score.saturating_add(ENTRY_OVERHEAD);
        let Ok(metadata) = fs::symlink_metadata(&current) else {
            continue;
        };
        if metadata.is_file() {
            score = score.saturating_add(metadata.len());
            continue;
        }
        if !metadata.is_dir()
            || metadata.file_type().is_symlink()
            || metadata_is_reparse_point(&metadata)
        {
            continue;
        }
        if let Ok(entries) = fs::read_dir(&current) {
            pending.extend(entries.flatten().map(|entry| entry.path()));
        }
    }
    score.max(1)
}

fn restore_path_to_desktop(source: &Path, desktop: &Path) -> Result<PathBuf, String> {
    restore_path_to_desktop_impl(source, desktop, true)
}

fn restore_path_to_desktop_silent(source: &Path, desktop: &Path) -> Result<PathBuf, String> {
    restore_path_to_desktop_impl(source, desktop, false)
}

fn restore_path_to_desktop_impl(
    source: &Path,
    desktop: &Path,
    notify_shell: bool,
) -> Result<PathBuf, String> {
    let file_name = source
        .file_name()
        .ok_or_else(|| "无法识别项目名称".to_owned())?;

    if !source.exists() {
        return Ok(desktop.join(file_name));
    }

    let existing_desktop_item = desktop.join(file_name);
    if existing_desktop_item.exists() {
        if same_path_text(
            &source.display().to_string(),
            &existing_desktop_item.display().to_string(),
        ) {
            if notify_shell {
                notify_shell_desktop_restore(&existing_desktop_item, desktop);
            }
            return Ok(existing_desktop_item);
        }

        if remove_duplicate_path_if_identical(source, &existing_desktop_item)? {
            if notify_shell {
                notify_shell_path_removed(source);
                notify_shell_desktop_restore(&existing_desktop_item, desktop);
            }
            return Ok(existing_desktop_item);
        }

        let destination = unique_destination(desktop, source);
        move_path(source, &destination)?;
        if notify_shell {
            notify_shell_path_removed(source);
            notify_shell_desktop_restore(&destination, desktop);
        }
        return Ok(destination);
    }

    if is_path_within(source, desktop) {
        if notify_shell {
            notify_shell_desktop_restore(source, desktop);
        }
        return Ok(source.to_path_buf());
    }

    let destination = unique_destination(desktop, source);
    move_path(source, &destination)?;
    if notify_shell {
        notify_shell_path_removed(source);
        notify_shell_desktop_restore(&destination, desktop);
    }
    Ok(destination)
}

fn remove_duplicate_path_if_identical(source: &Path, existing: &Path) -> Result<bool, String> {
    let locked_source = match lock_transfer_source_tree(source) {
        Ok(locked) => locked,
        Err(_) => return Ok(false),
    };
    let _locked_existing = match lock_transfer_source_tree(existing) {
        Ok(locked) => locked,
        Err(_) => return Ok(false),
    };
    if verify_copied_path(source, existing).is_err() {
        return Ok(false);
    }

    delete_locked_source_tree(locked_source)?;
    Ok(true)
}

fn notify_shell_desktop_restore(path: &Path, desktop: &Path) {
    notify_shell_path_created(path);
    notify_shell_directory_updated(desktop);
}

fn desktop_drop_position_to_icon_position(
    position: Option<DesktopDropPosition>,
) -> Option<DesktopIconPosition> {
    let (screen_x, screen_y) = desktop_drop_position_to_physical_point(position?)?;
    #[cfg(windows)]
    let monitors = windows_desktop_monitor_work_areas();
    #[cfg(not(windows))]
    let monitors = Vec::new();
    Some(desktop_icon_position_from_screen_point(
        screen_x, screen_y, &monitors,
    ))
}

fn desktop_drop_position_to_physical_point(position: DesktopDropPosition) -> Option<(i32, i32)> {
    if !position.screen_x.is_finite() || !position.screen_y.is_finite() {
        return None;
    }

    let scale_factor = position
        .scale_factor
        .filter(|value| value.is_finite() && *value > 0.0)
        .unwrap_or(1.0);
    Some((
        f64_to_i32_saturating((position.screen_x * scale_factor).round()),
        f64_to_i32_saturating((position.screen_y * scale_factor).round()),
    ))
}

fn f64_to_i32_saturating(value: f64) -> i32 {
    if value < i32::MIN as f64 {
        i32::MIN
    } else if value > i32::MAX as f64 {
        i32::MAX
    } else {
        value as i32
    }
}

fn desktop_icon_position_from_screen_point(
    screen_x: i32,
    screen_y: i32,
    monitors: &[DesktopMonitorWorkArea],
) -> DesktopIconPosition {
    let monitor = monitors
        .iter()
        .find(|monitor| desktop_monitor_contains_point(monitor, screen_x, screen_y));
    DesktopIconPosition {
        screen_x,
        screen_y,
        monitor_name: monitor.and_then(|monitor| monitor.name.clone()),
        monitor_offset_x: monitor.map(|monitor| screen_x.saturating_sub(monitor.x)),
        monitor_offset_y: monitor.map(|monitor| screen_y.saturating_sub(monitor.y)),
    }
}

fn resolve_desktop_icon_position(
    position: DesktopIconPosition,
    monitors: &[DesktopMonitorWorkArea],
) -> (i32, i32) {
    if let (Some(monitor_name), Some(offset_x), Some(offset_y)) = (
        position.monitor_name.as_deref(),
        position.monitor_offset_x,
        position.monitor_offset_y,
    ) {
        if let Some(monitor) = monitors.iter().find(|monitor| {
            monitor
                .name
                .as_deref()
                .is_some_and(|name| name.eq_ignore_ascii_case(monitor_name))
        }) {
            return clamp_desktop_icon_point_to_monitor(
                monitor,
                monitor.x.saturating_add(offset_x),
                monitor.y.saturating_add(offset_y),
            );
        }
    }

    if monitors.iter().any(|monitor| {
        desktop_monitor_contains_point(monitor, position.screen_x, position.screen_y)
    }) {
        return (position.screen_x, position.screen_y);
    }

    monitors
        .iter()
        .min_by_key(|monitor| {
            let (x, y) =
                clamp_desktop_icon_point_to_monitor(monitor, position.screen_x, position.screen_y);
            let distance_x = i128::from(position.screen_x) - i128::from(x);
            let distance_y = i128::from(position.screen_y) - i128::from(y);
            distance_x * distance_x + distance_y * distance_y
        })
        .map(|monitor| {
            clamp_desktop_icon_point_to_monitor(monitor, position.screen_x, position.screen_y)
        })
        .unwrap_or((position.screen_x, position.screen_y))
}

fn desktop_monitor_contains_point(
    monitor: &DesktopMonitorWorkArea,
    screen_x: i32,
    screen_y: i32,
) -> bool {
    let right = i64::from(monitor.x) + i64::from(monitor.width);
    let bottom = i64::from(monitor.y) + i64::from(monitor.height);
    i64::from(screen_x) >= i64::from(monitor.x)
        && i64::from(screen_x) < right
        && i64::from(screen_y) >= i64::from(monitor.y)
        && i64::from(screen_y) < bottom
}

fn clamp_desktop_icon_point_to_monitor(
    monitor: &DesktopMonitorWorkArea,
    screen_x: i32,
    screen_y: i32,
) -> (i32, i32) {
    let right = (i64::from(monitor.x) + i64::from(monitor.width.saturating_sub(1)))
        .clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
    let bottom = (i64::from(monitor.y) + i64::from(monitor.height.saturating_sub(1)))
        .clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
    (
        screen_x.clamp(monitor.x, right.max(monitor.x)),
        screen_y.clamp(monitor.y, bottom.max(monitor.y)),
    )
}

#[cfg(windows)]
fn capture_desktop_icon_position(path: &Path) -> Option<DesktopIconPosition> {
    let key = normalize_path_for_compare(path);
    capture_desktop_icon_positions(&[path.to_path_buf()]).remove(&key)
}

#[cfg(not(windows))]
fn capture_desktop_icon_position(_path: &Path) -> Option<DesktopIconPosition> {
    None
}

#[cfg(windows)]
fn capture_desktop_icon_positions(paths: &[PathBuf]) -> BTreeMap<String, DesktopIconPosition> {
    let Some(listview) = desktop_listview_window() else {
        return BTreeMap::new();
    };
    let Some(item_texts) = desktop_listview_item_texts(listview) else {
        return BTreeMap::new();
    };
    let desktop_roots = desktop_roots();
    let mut positions = BTreeMap::new();
    for path in paths {
        let Some(parent) = path.parent() else {
            continue;
        };
        if !desktop_roots
            .iter()
            .any(|desktop| same_path_for_move(parent, desktop))
        {
            continue;
        }
        let Some(index) = desktop_listview_find_item_in_texts(path, &item_texts) else {
            continue;
        };
        let Some(position) = desktop_listview_get_item_position(listview, index) else {
            continue;
        };
        positions.insert(normalize_path_for_compare(path), position);
    }
    positions
}

#[cfg(not(windows))]
fn capture_desktop_icon_positions(_paths: &[PathBuf]) -> BTreeMap<String, DesktopIconPosition> {
    BTreeMap::new()
}

#[cfg(windows)]
fn position_desktop_icon_at(path: &Path, position: Option<DesktopIconPosition>) {
    let Some(position) = position else {
        return;
    };
    position_desktop_icons(&[(path.to_path_buf(), position)]);
}

#[cfg(not(windows))]
fn position_desktop_icon_at(_path: &Path, _position: Option<DesktopIconPosition>) {}

#[cfg(windows)]
fn position_desktop_icons(items: &[(PathBuf, DesktopIconPosition)]) {
    const MAX_ATTEMPTS: usize = 100;
    const RETRY_DELAY: Duration = Duration::from_millis(100);

    let monitors = windows_desktop_monitor_work_areas();
    let mut pending = items.to_vec();
    for attempt in 0..MAX_ATTEMPTS {
        if let Some(listview) = desktop_listview_window() {
            if let Some(item_texts) = desktop_listview_item_texts(listview) {
                pending.retain(|(path, position)| {
                    let Some(index) = desktop_listview_find_item_in_texts(path, &item_texts) else {
                        return true;
                    };
                    let (screen_x, screen_y) =
                        resolve_desktop_icon_position(position.clone(), &monitors);
                    !desktop_listview_set_item_position(listview, index, screen_x, screen_y)
                });
            }
            if pending.is_empty() {
                // Explorer can publish a large batch incrementally. Reapply after it settles so a
                // late icon insertion cannot keep one of the already positioned shortcuts in the
                // temporary slot Explorer selected while restoring the batch.
                std::thread::sleep(Duration::from_millis(250));
                position_visible_desktop_icons_once(listview, items, &monitors);
                return;
            }
        }
        if attempt + 1 < MAX_ATTEMPTS {
            std::thread::sleep(RETRY_DELAY);
        }
    }

    // Apply every entry that eventually became visible even if a single Shell item never exposed
    // its display text. The unresolved count is retained in stderr for field diagnostics.
    if let Some(listview) = desktop_listview_window() {
        position_visible_desktop_icons_once(listview, items, &monitors);
    }
    if !pending.is_empty() {
        eprintln!(
            "desktop icon positioning timed out with {} unresolved item(s)",
            pending.len()
        );
    }
}

#[cfg(windows)]
fn position_visible_desktop_icons_once(
    listview: windows_sys::Win32::Foundation::HWND,
    items: &[(PathBuf, DesktopIconPosition)],
    monitors: &[DesktopMonitorWorkArea],
) {
    let Some(item_texts) = desktop_listview_item_texts(listview) else {
        return;
    };
    for (path, position) in items {
        let Some(index) = desktop_listview_find_item_in_texts(path, &item_texts) else {
            continue;
        };
        let (screen_x, screen_y) = resolve_desktop_icon_position(position.clone(), monitors);
        desktop_listview_set_item_position(listview, index, screen_x, screen_y);
    }
}

#[cfg(not(windows))]
fn position_desktop_icons(_items: &[(PathBuf, DesktopIconPosition)]) {}

#[cfg(windows)]
fn windows_desktop_monitor_work_areas() -> Vec<DesktopMonitorWorkArea> {
    use std::{
        mem::size_of,
        ptr::{null, null_mut},
    };

    use windows_sys::Win32::Graphics::Gdi::{
        EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFOEXW,
    };

    unsafe extern "system" fn collect_monitor(
        monitor: HMONITOR,
        _hdc: HDC,
        _rect: *mut windows_sys::Win32::Foundation::RECT,
        data: windows_sys::Win32::Foundation::LPARAM,
    ) -> windows_sys::core::BOOL {
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
        if unsafe { GetMonitorInfoW(monitor, &mut info.monitorInfo) } == 0 {
            return 1;
        }

        let name_len = info
            .szDevice
            .iter()
            .position(|character| *character == 0)
            .unwrap_or(info.szDevice.len());
        let name = (name_len > 0).then(|| String::from_utf16_lossy(&info.szDevice[..name_len]));
        let work = info.monitorInfo.rcWork;
        let monitors = unsafe { &mut *(data as *mut Vec<DesktopMonitorWorkArea>) };
        monitors.push(DesktopMonitorWorkArea {
            name,
            x: work.left,
            y: work.top,
            width: work.right.saturating_sub(work.left) as u32,
            height: work.bottom.saturating_sub(work.top) as u32,
            scale_factor: 1.0,
        });
        1
    }

    let mut monitors = Vec::new();
    unsafe {
        EnumDisplayMonitors(
            null_mut(),
            null(),
            Some(collect_monitor),
            (&mut monitors as *mut Vec<DesktopMonitorWorkArea>) as isize,
        );
    }
    monitors
}

#[cfg(windows)]
fn desktop_listview_window() -> Option<windows_sys::Win32::Foundation::HWND> {
    use std::ptr::{null, null_mut};

    use windows_sys::Win32::UI::WindowsAndMessaging::{EnumWindows, FindWindowW};

    unsafe {
        let progman = FindWindowW(windows_sys::core::w!("Progman"), null());
        if let Some(listview) = desktop_listview_under_window(progman) {
            return Some(listview);
        }

        let mut listview: windows_sys::Win32::Foundation::HWND = null_mut();
        EnumWindows(
            Some(enum_desktop_listview_window),
            (&mut listview as *mut _) as isize,
        );
        if listview.is_null() {
            return None;
        }
        Some(listview)
    }
}

#[cfg(windows)]
unsafe extern "system" fn enum_desktop_listview_window(
    hwnd: windows_sys::Win32::Foundation::HWND,
    lparam: windows_sys::Win32::Foundation::LPARAM,
) -> windows_sys::core::BOOL {
    if let Some(listview) = unsafe { desktop_listview_under_window(hwnd) } {
        unsafe {
            *(lparam as *mut windows_sys::Win32::Foundation::HWND) = listview;
        }
        return 0;
    }
    1
}

#[cfg(windows)]
unsafe fn desktop_listview_under_window(
    parent: windows_sys::Win32::Foundation::HWND,
) -> Option<windows_sys::Win32::Foundation::HWND> {
    use std::ptr::{null, null_mut};

    use windows_sys::Win32::UI::WindowsAndMessaging::FindWindowExW;

    if parent.is_null() {
        return None;
    }

    let shell_view = unsafe {
        FindWindowExW(
            parent,
            null_mut(),
            windows_sys::core::w!("SHELLDLL_DefView"),
            null(),
        )
    };
    if shell_view.is_null() {
        return None;
    }

    let listview = unsafe {
        FindWindowExW(
            shell_view,
            null_mut(),
            windows_sys::core::w!("SysListView32"),
            null(),
        )
    };
    if listview.is_null() {
        None
    } else {
        Some(listview)
    }
}

fn desktop_listview_find_item_in_texts(path: &Path, item_texts: &[(i32, String)]) -> Option<i32> {
    let candidates = desktop_icon_name_candidates(path);
    item_texts.iter().find_map(|(index, item_text)| {
        candidates
            .iter()
            .any(|candidate| candidate.to_lowercase() == item_text.to_lowercase())
            .then_some(*index)
    })
}

#[cfg(windows)]
fn desktop_listview_item_texts(
    listview: windows_sys::Win32::Foundation::HWND,
) -> Option<Vec<(i32, String)>> {
    use std::{
        ffi::c_void,
        mem::size_of,
        ptr::{null, null_mut},
    };

    use windows_sys::Win32::{
        Foundation::CloseHandle,
        System::{
            Diagnostics::Debug::{ReadProcessMemory, WriteProcessMemory},
            Memory::{
                VirtualAllocEx, VirtualFreeEx, MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE,
            },
            Threading::{
                OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_OPERATION, PROCESS_VM_READ,
                PROCESS_VM_WRITE,
            },
        },
        UI::WindowsAndMessaging::{GetWindowThreadProcessId, SendMessageW},
    };

    const LVM_GETITEMCOUNT: u32 = 0x1000 + 4;
    const LVM_GETITEMTEXTW: u32 = 0x1000 + 115;
    const TEXT_CAPACITY: usize = 1024;

    #[repr(C)]
    struct LvItemTextW {
        mask: u32,
        item: i32,
        sub_item: i32,
        state: u32,
        state_mask: u32,
        text: *mut u16,
        text_capacity: i32,
    }

    unsafe {
        let count = SendMessageW(listview, LVM_GETITEMCOUNT, 0, 0);
        if count <= 0 || count > i32::MAX as isize {
            return Some(Vec::new());
        }

        let mut process_id = 0u32;
        GetWindowThreadProcessId(listview, &mut process_id);
        if process_id == 0 {
            return None;
        }

        let process = OpenProcess(
            PROCESS_QUERY_INFORMATION | PROCESS_VM_OPERATION | PROCESS_VM_READ | PROCESS_VM_WRITE,
            0,
            process_id,
        );
        if process.is_null() {
            return None;
        }

        let item_size = size_of::<LvItemTextW>();
        let text_size = TEXT_CAPACITY * size_of::<u16>();
        let remote_block = VirtualAllocEx(
            process,
            null(),
            item_size + text_size,
            MEM_COMMIT | MEM_RESERVE,
            PAGE_READWRITE,
        );
        if remote_block.is_null() {
            CloseHandle(process);
            return None;
        }
        let remote_text = (remote_block as usize + item_size) as *mut u16;
        let mut item_texts = Vec::with_capacity(count as usize);

        for index in 0..count as i32 {
            let item = LvItemTextW {
                mask: 0,
                item: index,
                sub_item: 0,
                state: 0,
                state_mask: 0,
                text: remote_text,
                text_capacity: TEXT_CAPACITY as i32,
            };
            if WriteProcessMemory(
                process,
                remote_block,
                (&item as *const LvItemTextW).cast::<c_void>(),
                item_size,
                null_mut(),
            ) == 0
            {
                continue;
            }

            let characters = SendMessageW(
                listview,
                LVM_GETITEMTEXTW,
                index as usize,
                remote_block as isize,
            );
            if characters <= 0 {
                continue;
            }
            let characters = (characters as usize).min(TEXT_CAPACITY.saturating_sub(1));
            let mut text = vec![0u16; characters];
            if ReadProcessMemory(
                process,
                remote_text.cast(),
                text.as_mut_ptr().cast(),
                characters * size_of::<u16>(),
                null_mut(),
            ) == 0
            {
                continue;
            }
            item_texts.push((index, String::from_utf16_lossy(&text)));
        }

        VirtualFreeEx(process, remote_block, 0, MEM_RELEASE);
        CloseHandle(process);
        Some(item_texts)
    }
}

#[cfg(windows)]
fn desktop_listview_get_item_position(
    listview: windows_sys::Win32::Foundation::HWND,
    index: i32,
) -> Option<DesktopIconPosition> {
    use std::{
        mem::size_of,
        ptr::{null, null_mut},
    };

    use windows_sys::Win32::{
        Foundation::{CloseHandle, POINT},
        Graphics::Gdi::ClientToScreen,
        System::{
            Diagnostics::Debug::ReadProcessMemory,
            Memory::{
                VirtualAllocEx, VirtualFreeEx, MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE,
            },
            Threading::{
                OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_OPERATION, PROCESS_VM_READ,
            },
        },
        UI::WindowsAndMessaging::{GetWindowThreadProcessId, SendMessageW},
    };

    const LVM_GETITEMPOSITION: u32 = 0x1000 + 16;

    unsafe {
        let mut process_id = 0u32;
        GetWindowThreadProcessId(listview, &mut process_id);
        if process_id == 0 {
            return None;
        }

        let process = OpenProcess(
            PROCESS_QUERY_INFORMATION | PROCESS_VM_OPERATION | PROCESS_VM_READ,
            0,
            process_id,
        );
        if process.is_null() {
            return None;
        }

        let point_size = size_of::<POINT>();
        let remote_point = VirtualAllocEx(
            process,
            null(),
            point_size,
            MEM_COMMIT | MEM_RESERVE,
            PAGE_READWRITE,
        );
        if remote_point.is_null() {
            CloseHandle(process);
            return None;
        }

        let result = SendMessageW(
            listview,
            LVM_GETITEMPOSITION,
            index as usize,
            remote_point as isize,
        );
        let mut point = POINT::default();
        let read_position = result != 0
            && ReadProcessMemory(
                process,
                remote_point,
                (&mut point as *mut POINT).cast(),
                point_size,
                null_mut(),
            ) != 0;

        VirtualFreeEx(process, remote_point, 0, MEM_RELEASE);
        CloseHandle(process);
        if !read_position || ClientToScreen(listview, &mut point) == 0 {
            return None;
        }

        Some(desktop_icon_position_from_screen_point(
            point.x,
            point.y,
            &windows_desktop_monitor_work_areas(),
        ))
    }
}

fn desktop_icon_name_candidates(path: &Path) -> Vec<String> {
    let mut candidates = Vec::new();
    if let Some(file_name) = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
    {
        push_unique_string(&mut candidates, file_name);
    }
    if let Some(stem) = path
        .file_stem()
        .map(|name| name.to_string_lossy().to_string())
    {
        push_unique_string(&mut candidates, stem);
    }
    candidates
}

fn push_unique_string(values: &mut Vec<String>, value: String) {
    if value.is_empty() || values.iter().any(|existing| existing == &value) {
        return;
    }
    values.push(value);
}

#[cfg(windows)]
fn desktop_listview_set_item_position(
    listview: windows_sys::Win32::Foundation::HWND,
    index: i32,
    screen_x: i32,
    screen_y: i32,
) -> bool {
    use std::{
        ffi::c_void,
        mem::size_of,
        ptr::{null, null_mut},
    };

    use windows_sys::Win32::{
        Foundation::{CloseHandle, POINT},
        Graphics::Gdi::ScreenToClient,
        System::{
            Diagnostics::Debug::WriteProcessMemory,
            Memory::{
                VirtualAllocEx, VirtualFreeEx, MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE,
            },
            Threading::{
                OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_OPERATION, PROCESS_VM_WRITE,
            },
        },
        UI::WindowsAndMessaging::{GetWindowThreadProcessId, SendMessageW},
    };

    const LVM_SETITEMPOSITION32: u32 = 0x1000 + 49;

    unsafe {
        let mut point = POINT {
            x: screen_x,
            y: screen_y,
        };
        if ScreenToClient(listview, &mut point) == 0 {
            return false;
        }

        let mut process_id = 0u32;
        GetWindowThreadProcessId(listview, &mut process_id);
        if process_id == 0 {
            return false;
        }

        let process = OpenProcess(
            PROCESS_QUERY_INFORMATION | PROCESS_VM_OPERATION | PROCESS_VM_WRITE,
            0,
            process_id,
        );
        if process.is_null() {
            return false;
        }

        let point_size = size_of::<POINT>();
        let remote_point = VirtualAllocEx(
            process,
            null(),
            point_size,
            MEM_COMMIT | MEM_RESERVE,
            PAGE_READWRITE,
        );
        if remote_point.is_null() {
            CloseHandle(process);
            return false;
        }

        let wrote_point = WriteProcessMemory(
            process,
            remote_point,
            (&point as *const POINT).cast::<c_void>(),
            point_size,
            null_mut(),
        ) != 0;
        let sent = if wrote_point {
            // 该消息没有可用返回值，发送完成即表示请求已交给桌面列表处理。
            SendMessageW(
                listview,
                LVM_SETITEMPOSITION32,
                index as usize,
                remote_point as isize,
            );
            true
        } else {
            false
        };

        VirtualFreeEx(process, remote_point, 0, MEM_RELEASE);
        CloseHandle(process);
        sent
    }
}

#[cfg(windows)]
fn notify_shell_path_created(path: &Path) {
    notify_shell_path(path, windows_sys::Win32::UI::Shell::SHCNE_CREATE);
}

#[cfg(not(windows))]
fn notify_shell_path_created(_path: &Path) {}

#[cfg(windows)]
fn notify_shell_path_deleted(path: &Path) {
    notify_shell_path(path, windows_sys::Win32::UI::Shell::SHCNE_DELETE);
}

#[cfg(not(windows))]
fn notify_shell_path_deleted(_path: &Path) {}

#[cfg(windows)]
fn notify_shell_directory_updated(path: &Path) {
    notify_shell_path(path, windows_sys::Win32::UI::Shell::SHCNE_UPDATEDIR);
}

#[cfg(not(windows))]
fn notify_shell_directory_updated(_path: &Path) {}

#[cfg(windows)]
fn notify_shell_path(path: &Path, event: u32) {
    use std::{os::windows::ffi::OsStrExt, ptr::null};

    use windows_sys::Win32::UI::Shell::{SHChangeNotify, SHCNF_FLUSHNOWAIT, SHCNF_PATHW};

    let wide_path = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    unsafe {
        SHChangeNotify(
            event as i32,
            SHCNF_PATHW | SHCNF_FLUSHNOWAIT,
            wide_path.as_ptr().cast(),
            null(),
        );
    }
}

fn notify_shell_parent_updated(path: &Path) {
    if let Some(parent) = path.parent() {
        notify_shell_directory_updated(parent);
    }
}

fn notify_shell_path_removed(path: &Path) {
    notify_shell_path_deleted(path);
    notify_shell_parent_updated(path);
}

fn notify_shell_path_moved(source: &Path, destination: &Path) {
    notify_shell_path_removed(source);
    notify_shell_path_created(destination);
    notify_shell_parent_updated(destination);
}

fn organizer_contents(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };

    let mut paths = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if is_internal_transfer_path(&path) {
            continue;
        }
        if path.is_dir() {
            let Ok(children) = fs::read_dir(&path) else {
                continue;
            };
            for child in children.flatten() {
                let child_path = child.path();
                if !is_internal_transfer_path(&child_path) {
                    paths.push(child_path);
                }
            }
        } else {
            paths.push(path);
        }
    }
    paths
}

fn cleanup_empty_organizer_dirs(root: &Path) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if fs::read_dir(&path)
            .map(|mut children| children.next().is_none())
            .unwrap_or(false)
        {
            let _ = fs::remove_dir(path);
        }
    }
}

fn categories_with_item_details(
    mut categories: Vec<DeskCategory>,
    include_icons: bool,
) -> Vec<DeskCategory> {
    for category in &mut categories {
        category.item_details = category
            .item_paths
            .iter()
            .map(|path| desktop_item_for_path(Path::new(path), include_icons))
            .collect();
        if category.sort_by_name {
            category.item_details.sort_by(|left, right| {
                left.name
                    .to_lowercase()
                    .cmp(&right.name.to_lowercase())
                    .then_with(|| left.name.cmp(&right.name))
                    .then_with(|| left.path.to_lowercase().cmp(&right.path.to_lowercase()))
            });
        }
    }
    categories
}

fn launchers_with_optional_icons(
    mut launchers: Vec<LaunchItem>,
    include_icons: bool,
) -> Vec<LaunchItem> {
    for launcher in &mut launchers {
        let path = Path::new(&launcher.path);
        launcher.name = normalized_launch_name(&launcher.name, path);
        if include_icons {
            launcher.icon_data_url = cached_icon_data_url(path);
        } else {
            launcher.icon_data_url = None;
        }
    }
    launchers
}

fn repair_launchers(store: &AppStore, launchers: &mut models::LaunchData) -> Result<(), String> {
    let repaired = launcher_items_from_directory(store, &launchers.items)?;
    if !same_launcher_items(&launchers.items, &repaired) {
        launchers.items = repaired;
        store.save_launchers(launchers).map_err(to_message)?;
    }

    Ok(())
}

fn launcher_items_from_directory(
    store: &AppStore,
    previous: &[LaunchItem],
) -> Result<Vec<LaunchItem>, String> {
    let launcher_root = store.launchers_root();
    fs::create_dir_all(&launcher_root).map_err(to_message)?;
    let previous_names = previous
        .iter()
        .map(|item| (item.path.to_lowercase(), item.name.clone()))
        .collect::<BTreeMap<_, _>>();
    let mut items = Vec::new();

    for entry in fs::read_dir(&launcher_root).map_err(to_message)?.flatten() {
        let path = entry.path();
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if file_name.eq_ignore_ascii_case("desktop.ini") {
            continue;
        }
        repair_dustdesk_shortcut_best_effort(&path);

        let path_text = path.display().to_string();
        let name = previous_names
            .get(&path_text.to_lowercase())
            .map(|name| normalized_launch_name(name, &path))
            .unwrap_or_else(|| display_path_name(&path));
        items.push(LaunchItem {
            name,
            path: path_text,
            icon_data_url: None,
        });
    }

    items.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.path.to_lowercase().cmp(&right.path.to_lowercase()))
    });
    Ok(items)
}

fn same_launcher_items(left: &[LaunchItem], right: &[LaunchItem]) -> bool {
    left.len() == right.len()
        && left.iter().zip(right.iter()).all(|(left, right)| {
            left.name == right.name
                && same_path_text(&left.path, &right.path)
                && left.icon_data_url.is_none()
                && right.icon_data_url.is_none()
        })
}

fn create_launcher_entry(store: &AppStore, source: &Path, name: &str) -> Result<PathBuf, String> {
    if !source.exists() {
        return Err("路径不存在".to_owned());
    }

    let launcher_root = store.launchers_root();
    fs::create_dir_all(&launcher_root).map_err(to_message)?;
    if is_path_within(source, &launcher_root) {
        return Ok(source.to_path_buf());
    }

    let extension = source
        .extension()
        .map(|extension| extension.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if source.is_file() && matches!(extension.as_str(), "lnk" | "url" | "appref-ms") {
        let destination = launcher_root.join(
            source
                .file_name()
                .ok_or_else(|| "无法识别启动项名称".to_owned())?,
        );
        if !destination.exists() {
            fs::copy(source, &destination).map_err(to_message)?;
        }
        return Ok(destination);
    }

    let display_name = normalized_launch_name(name, source);
    let shortcut_name = format!("{}.lnk", safe_windows_file_name(&display_name));
    let shortcut = launcher_root.join(shortcut_name);
    if !shortcut.exists() {
        create_windows_shortcut(&shortcut, source, &display_name)?;
    }
    Ok(shortcut)
}

fn normalized_launch_name(name: &str, path: &Path) -> String {
    let trimmed = name.trim();
    let display_name = display_path_name(path);
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();

    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case(&file_name) {
        display_name
    } else {
        trimmed.to_owned()
    }
}

fn resolve_path_icons_impl(paths: Vec<String>) -> Result<Vec<PathIconResult>, String> {
    const ICON_RESOLVE_LIMIT: usize = 256;

    let mut unique = BTreeMap::<String, String>::new();
    for path in paths {
        let trimmed = path.trim();
        if trimmed.is_empty() {
            continue;
        }
        unique
            .entry(trimmed.to_lowercase())
            .or_insert_with(|| trimmed.to_owned());
        if unique.len() >= ICON_RESOLVE_LIMIT {
            break;
        }
    }

    Ok(unique
        .into_values()
        .map(|path| PathIconResult {
            icon_data_url: cached_icon_data_url(Path::new(&path)),
            path,
        })
        .collect())
}

fn cached_icon_data_url(path: &Path) -> Option<String> {
    cached_icon_data_url_with(path, system_icon::icon_data_url)
}

fn cached_icon_data_url_with(
    path: &Path,
    resolve: impl FnOnce(&Path) -> Option<String>,
) -> Option<String> {
    cached_icon_data_url_with_cooldown(path, ICON_FAILURE_CACHE_TTL, resolve)
}

fn cached_icon_data_url_with_cooldown(
    path: &Path,
    failure_cooldown: Duration,
    resolve: impl FnOnce(&Path) -> Option<String>,
) -> Option<String> {
    let key = normalize_path_for_compare(path);
    if key.is_empty() || !path.exists() {
        return None;
    }

    if let Some(icon) = cached_icon_value(&key) {
        return Some(icon);
    }
    if icon_failure_is_fresh(&key, failure_cooldown) {
        return None;
    }

    // Multiple WebViews can request the same path at once. Deduplicate only matching paths so a
    // slow Shell handler cannot block unrelated icons or search requests.
    let resolution_lock = {
        let mut locks = ICON_RESOLUTION_LOCKS
            .get_or_init(|| Mutex::new(BTreeMap::new()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        locks.retain(|_, lock| Arc::strong_count(lock) > 1);
        locks
            .entry(key.clone())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    };
    let resolution_guard = resolution_lock
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let icon = if let Some(icon) = cached_icon_value(&key) {
        Some(icon)
    } else if icon_failure_is_fresh(&key, failure_cooldown) {
        None
    } else {
        let icon = resolve(path);
        remember_icon_result(&key, icon.as_ref());
        icon
    };
    drop(resolution_guard);

    if let Ok(mut locks) = ICON_RESOLUTION_LOCKS
        .get_or_init(|| Mutex::new(BTreeMap::new()))
        .lock()
    {
        if Arc::strong_count(&resolution_lock) == 2
            && locks
                .get(&key)
                .is_some_and(|current| Arc::ptr_eq(current, &resolution_lock))
        {
            locks.remove(&key);
        }
    }

    icon
}

fn cached_icon_value(key: &str) -> Option<String> {
    ICON_DATA_URL_CACHE
        .get_or_init(|| Mutex::new(BTreeMap::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(key)
        .cloned()
}

fn icon_failure_is_fresh(key: &str, cooldown: Duration) -> bool {
    let now = Instant::now();
    let mut failures = ICON_FAILURE_CACHE
        .get_or_init(|| Mutex::new(BTreeMap::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(failed_at) = failures.get(key).copied() else {
        return false;
    };
    if now.saturating_duration_since(failed_at) < cooldown {
        true
    } else {
        failures.remove(key);
        false
    }
}

fn remember_icon_result(key: &str, icon: Option<&String>) {
    if let Some(icon) = icon {
        let mut cache = ICON_DATA_URL_CACHE
            .get_or_init(|| Mutex::new(BTreeMap::new()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if cache.len() >= ICON_DATA_CACHE_LIMIT && !cache.contains_key(key) {
            cache.clear();
        }
        cache.insert(key.to_owned(), icon.clone());
        drop(cache);
        ICON_FAILURE_CACHE
            .get_or_init(|| Mutex::new(BTreeMap::new()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(key);
        return;
    }

    let mut failures = ICON_FAILURE_CACHE
        .get_or_init(|| Mutex::new(BTreeMap::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if failures.len() >= ICON_FAILURE_CACHE_LIMIT && !failures.contains_key(key) {
        failures.clear();
    }
    failures.insert(key.to_owned(), Instant::now());
}

fn desktop_items(include_icons: bool) -> Vec<DesktopItem> {
    let mut unique = BTreeMap::<String, DesktopItem>::new();
    for root in desktop_roots() {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if is_desktop_ini_path(&path) || is_internal_transfer_path(&path) {
                continue;
            }
            let key = path.to_string_lossy().to_lowercase();
            unique
                .entry(key)
                .or_insert_with(|| desktop_item_for_path(&path, include_icons));
        }
    }
    unique.into_values().collect()
}

fn desktop_item_for_path(path: &Path, include_icon: bool) -> DesktopItem {
    let mut item = DesktopItem::from_path(path);
    if include_icon {
        item.icon_data_url = cached_icon_data_url(path);
    }
    item
}

fn effective_search_paths(store: &AppStore, settings: &AppSettings) -> Vec<String> {
    let mut paths = normalize_search_paths(settings.search_paths.clone());
    if paths.is_empty() {
        paths.push(store.organizer_root().display().to_string());
    }
    paths
}

fn normalize_search_paths(paths: Vec<String>) -> Vec<String> {
    let mut unique = BTreeMap::<String, String>::new();
    for path in paths {
        let trimmed = path.trim();
        if trimmed.is_empty() {
            continue;
        }
        unique
            .entry(trimmed.to_lowercase())
            .or_insert_with(|| trimmed.to_owned());
    }
    unique.into_values().collect()
}

fn normalize_path_input(path: &str) -> Result<String, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("路径为空".to_owned());
    }
    Ok(trimmed.to_owned())
}

fn validate_runtime_directory_target(
    target: &str,
    requested_path: &Path,
    old_store: &AppStore,
) -> Result<(), String> {
    let source = match target {
        "data" => old_store.data_dir(),
        "organizer" => old_store.organizer_root(),
        "launchers" => old_store.launchers_root(),
        _ => return Ok(()),
    };

    if is_target_inside_source(&source, requested_path) {
        return Err("新目录不能设置在原目录内部，请选择原目录外的位置后再迁移".to_owned());
    }

    if target == "data"
        && (paths_overlap(&old_store.data_dir(), &old_store.organizer_root())
            || paths_overlap(&old_store.data_dir(), &old_store.launchers_root()))
    {
        return Err(
            "当前收纳或快捷启动目录位于 Data 内部，请先分别迁移到独立目录，再迁移 Data".to_owned(),
        );
    }

    let mut prospective_roots = [
        old_store.data_dir(),
        old_store.organizer_root(),
        old_store.launchers_root(),
    ];
    prospective_roots[match target {
        "data" => 0,
        "organizer" => 1,
        "launchers" => 2,
        _ => return Ok(()),
    }] = requested_path.to_path_buf();
    for left in 0..prospective_roots.len() {
        for right in (left + 1)..prospective_roots.len() {
            if paths_overlap(&prospective_roots[left], &prospective_roots[right]) {
                return Err(format!(
                    "Data、DesktopOrganizer、Launchers 必须使用互不包含的独立目录：{} 与 {}",
                    prospective_roots[left].display(),
                    prospective_roots[right].display()
                ));
            }
        }
    }

    Ok(())
}

fn paths_overlap(left: &Path, right: &Path) -> bool {
    same_path_for_move(left, right)
        || is_target_inside_source(left, right)
        || is_target_inside_source(right, left)
}

fn copy_directory_contents_preserving_source(source: &Path, target: &Path) -> Result<(), String> {
    let source_metadata = match fs::symlink_metadata(source) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(to_message(error)),
    };
    validate_transfer_metadata(source, &source_metadata)?;
    if !source_metadata.is_dir() {
        return Err(format!("源路径不是目录：{}", source.display()));
    }
    if same_path_for_move(source, target) {
        return Ok(());
    }
    if is_target_inside_source(source, target) {
        return Err("新目录不能设置在原目录内部，请选择原目录外的位置后再迁移".to_owned());
    }

    match fs::symlink_metadata(target) {
        Ok(metadata) => {
            validate_transfer_metadata(target, &metadata)?;
            if !metadata.is_dir() {
                return Err(format!("目标路径不是目录：{}", target.display()));
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir_all(target).map_err(to_message)?;
        }
        Err(error) => return Err(to_message(error)),
    }

    for entry in fs::read_dir(source).map_err(to_message)? {
        let entry = entry.map_err(to_message)?;
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        let source_metadata = fs::symlink_metadata(&source_path).map_err(to_message)?;
        validate_transfer_metadata(&source_path, &source_metadata)?;

        match fs::symlink_metadata(&target_path) {
            Ok(target_metadata) => {
                validate_transfer_metadata(&target_path, &target_metadata)?;
                if source_metadata.is_dir() && target_metadata.is_dir() {
                    copy_directory_contents_preserving_source(&source_path, &target_path)?;
                    continue;
                }
                let _source_lock = lock_transfer_source_tree(&source_path)?;
                let _target_lock = lock_transfer_source_tree(&target_path)?;
                verify_copied_path(&source_path, &target_path).map_err(|_| {
                    format!(
                        "迁移目标存在不同内容，已保留源项目：{} -> {}",
                        source_path.display(),
                        target_path.display()
                    )
                })?;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                copy_path_via_staging(&source_path, &target_path)?;
            }
            Err(error) => return Err(to_message(error)),
        }
    }

    Ok(())
}

fn verify_directory_contents_contained(source: &Path, target: &Path) -> Result<(), String> {
    if same_path_for_move(source, target) {
        return Ok(());
    }
    let source_metadata = fs::symlink_metadata(source).map_err(to_message)?;
    let target_metadata = fs::symlink_metadata(target).map_err(to_message)?;
    validate_transfer_metadata(source, &source_metadata)?;
    validate_transfer_metadata(target, &target_metadata)?;
    if !source_metadata.is_dir() || !target_metadata.is_dir() {
        return Err(format!(
            "运行目录迁移校验失败：{} 或 {} 不是目录",
            source.display(),
            target.display()
        ));
    }

    for entry in fs::read_dir(source).map_err(to_message)? {
        let entry = entry.map_err(to_message)?;
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        let source_metadata = fs::symlink_metadata(&source_path).map_err(to_message)?;
        let target_metadata = fs::symlink_metadata(&target_path).map_err(|error| {
            format!(
                "运行目录迁移校验失败，目标缺少 {}：{}",
                source_path.display(),
                error
            )
        })?;
        validate_transfer_metadata(&source_path, &source_metadata)?;
        validate_transfer_metadata(&target_path, &target_metadata)?;
        if source_metadata.is_dir() && target_metadata.is_dir() {
            verify_directory_contents_contained(&source_path, &target_path)?;
            continue;
        }

        let _source_lock = lock_transfer_source_tree(&source_path)?;
        let _target_lock = lock_transfer_source_tree(&target_path)?;
        verify_copied_path(&source_path, &target_path)?;
    }

    Ok(())
}

fn remove_verified_directory_contents(source: &Path, target: &Path) -> Result<(), String> {
    if same_path_for_move(source, target) || !source.exists() {
        return Ok(());
    }
    if is_target_inside_source(source, target) {
        return Err("不能从源目录内部清理运行目录迁移副本".to_owned());
    }

    for entry in fs::read_dir(source).map_err(to_message)? {
        let entry = entry.map_err(to_message)?;
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        let source_metadata = fs::symlink_metadata(&source_path).map_err(to_message)?;
        let target_metadata = fs::symlink_metadata(&target_path).map_err(|error| {
            format!(
                "新目录缺少已迁移项目，已保留旧目录 {}：{}",
                source_path.display(),
                error
            )
        })?;
        validate_transfer_metadata(&source_path, &source_metadata)?;
        validate_transfer_metadata(&target_path, &target_metadata)?;

        if source_metadata.is_dir() && target_metadata.is_dir() {
            remove_verified_directory_contents(&source_path, &target_path)?;
            continue;
        }

        let locked_source = lock_transfer_source_tree(&source_path)?;
        let _locked_target = lock_transfer_source_tree(&target_path)?;
        verify_copied_path(&source_path, &target_path)?;
        delete_locked_source_tree(locked_source)?;
    }

    match fs::remove_dir(source) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "新目录已启用，但旧目录仍有未迁移内容，已保留 {}：{}",
            source.display(),
            error
        )),
    }
}

#[cfg(test)]
fn move_directory_contents(source: &Path, target: &Path) -> Result<(), String> {
    let source_metadata = match fs::symlink_metadata(source) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(to_message(error)),
    };
    validate_transfer_metadata(source, &source_metadata)?;
    if !source_metadata.is_dir() {
        return Err(format!("源路径不是目录：{}", source.display()));
    }

    if same_path_for_move(source, target) {
        return Ok(());
    }
    if is_target_inside_source(source, target) {
        return Err("新目录不能设置在原目录内部，请选择原目录外的位置后再迁移".to_owned());
    }
    match fs::symlink_metadata(target) {
        Ok(metadata) => {
            validate_transfer_metadata(target, &metadata)?;
            if !metadata.is_dir() {
                return Err(format!("目标路径不是目录：{}", target.display()));
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir_all(target).map_err(to_message)?;
            let metadata = fs::symlink_metadata(target).map_err(to_message)?;
            validate_transfer_metadata(target, &metadata)?;
        }
        Err(error) => return Err(to_message(error)),
    }

    for entry in fs::read_dir(source).map_err(to_message)? {
        let entry = entry.map_err(to_message)?;
        let source_path = entry.path();
        let Some(file_name) = source_path.file_name() else {
            continue;
        };
        let target_path = target.join(file_name);
        let source_metadata = fs::symlink_metadata(&source_path).map_err(to_message)?;
        validate_transfer_metadata(&source_path, &source_metadata)?;

        match fs::symlink_metadata(&target_path) {
            Ok(target_metadata) => {
                validate_transfer_metadata(&target_path, &target_metadata)?;
                if source_metadata.is_dir() && target_metadata.is_dir() {
                    move_directory_contents(&source_path, &target_path)?;
                    continue;
                }
                return Err(format!(
                    "迁移目标已存在，已保留源项目：{} -> {}",
                    source_path.display(),
                    target_path.display()
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(to_message(error)),
        }

        move_path(&source_path, &target_path)?;
    }

    match fs::remove_dir(source) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "迁移后源目录仍有未处理内容，已保留该目录 {}：{}",
            source.display(),
            error
        )),
    }
}

fn same_path_for_move(left: &Path, right: &Path) -> bool {
    normalize_resolved_path_for_compare(left) == normalize_resolved_path_for_compare(right)
}

fn is_target_inside_source(source: &Path, target: &Path) -> bool {
    let source_key = normalize_resolved_path_for_compare(source);
    let target_key = normalize_resolved_path_for_compare(target);
    !source_key.is_empty()
        && source_key != target_key
        && target_key.starts_with(&format!("{source_key}\\"))
}

fn normalize_resolved_path_for_compare(path: &Path) -> String {
    let mut existing = path.to_path_buf();
    let mut missing_names = Vec::new();
    loop {
        if let Ok(mut resolved) = existing.canonicalize() {
            for name in missing_names.iter().rev() {
                resolved.push(name);
            }
            return normalize_path_for_compare(&resolved);
        }

        let Some(name) = existing.file_name().map(|name| name.to_os_string()) else {
            return normalize_path_for_compare(path);
        };
        missing_names.push(name);
        if !existing.pop() {
            return normalize_path_for_compare(path);
        }
    }
}

fn rewrite_runtime_path_prefixes(
    store: &AppStore,
    replacements: &[(&PathBuf, &PathBuf)],
) -> Result<(), String> {
    let mut config = store.load_config_strict().map_err(to_message)?;
    let mut changed = false;

    for category in &mut config.desktop_categories {
        for item_path in &mut category.item_paths {
            if let Some(next_path) = rewrite_path_prefix(item_path, replacements) {
                *item_path = next_path;
                changed = true;
            }
        }
    }

    let previous_icon_positions = std::mem::take(&mut config.desktop_icon_positions);
    for (path, position) in previous_icon_positions {
        let next_path = rewrite_path_prefix(&path, replacements).unwrap_or_else(|| path.clone());
        changed |= !same_path_text(&path, &next_path);
        set_desktop_icon_position_for_path(&mut config.desktop_icon_positions, next_path, position);
    }

    for search_path in &mut config.settings.search_paths {
        if let Some(next_path) = rewrite_path_prefix(search_path, replacements) {
            *search_path = next_path;
            changed = true;
        }
    }

    if changed {
        store.save_config(&config).map_err(to_message)?;
    }

    Ok(())
}

fn rewrite_launchers_path_prefixes(
    store: &AppStore,
    replacements: &[(&PathBuf, &PathBuf)],
) -> Result<(), String> {
    let mut launchers = store.load_launchers_strict().map_err(to_message)?;
    let mut changed = false;

    for item in &mut launchers.items {
        if let Some(next_path) = rewrite_path_prefix(&item.path, replacements) {
            item.path = next_path;
            item.icon_data_url = None;
            changed = true;
        }
    }

    if changed {
        store.save_launchers(&launchers).map_err(to_message)?;
    }

    Ok(())
}

fn rewrite_clipboard_path_prefixes(
    store: &AppStore,
    replacements: &[(&PathBuf, &PathBuf)],
) -> Result<(), String> {
    let mut clipboard = store.load_clipboard_strict().map_err(to_message)?;
    let mut changed = false;

    for item in &mut clipboard.items {
        if let Some(next_path) = rewrite_path_prefix(&item.image_path, replacements) {
            item.image_path = next_path;
            changed = true;
        }
        if let Some(next_path) = rewrite_path_prefix(&item.image_thumb_path, replacements) {
            item.image_thumb_path = next_path;
            changed = true;
        }
    }

    if changed {
        store.save_clipboard(&clipboard).map_err(to_message)?;
    }

    Ok(())
}

fn rewrite_search_history_path_prefixes(
    store: &AppStore,
    replacements: &[(&PathBuf, &PathBuf)],
) -> Result<(), String> {
    let mut history = store.load_search_history_strict().map_err(to_message)?;
    let mut changed = false;

    for item in &mut history.items {
        if let Some(next_path) = rewrite_path_prefix(&item.path, replacements) {
            item.path = next_path;
            changed = true;
        }
    }

    if changed {
        store.save_search_history(&history).map_err(to_message)?;
    }

    Ok(())
}

fn rewrite_path_prefix(path_text: &str, replacements: &[(&PathBuf, &PathBuf)]) -> Option<String> {
    let path = PathBuf::from(path_text);
    for (old_root, new_root) in replacements {
        if same_path_text(
            &old_root.display().to_string(),
            &new_root.display().to_string(),
        ) || !is_path_within(&path, old_root)
        {
            continue;
        }

        if let Some(relative) = strip_path_prefix_case_insensitive(&path, old_root) {
            return Some(new_root.join(relative).display().to_string());
        }
    }
    None
}

fn strip_path_prefix_case_insensitive(path: &Path, root: &Path) -> Option<PathBuf> {
    let path_components = path.components().collect::<Vec<_>>();
    let root_components = root.components().collect::<Vec<_>>();
    if root_components.len() > path_components.len()
        || !root_components
            .iter()
            .zip(&path_components)
            .all(|(left, right)| {
                left.as_os_str()
                    .to_string_lossy()
                    .eq_ignore_ascii_case(&right.as_os_str().to_string_lossy())
            })
    {
        return None;
    }

    let mut relative = PathBuf::new();
    for component in &path_components[root_components.len()..] {
        relative.push(component.as_os_str());
    }
    Some(relative)
}

fn same_path_text(left: &str, right: &str) -> bool {
    left.trim().eq_ignore_ascii_case(right.trim())
}

fn is_desktop_ini_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("desktop.ini"))
}

fn should_skip_desktop_classify_item(item: &DesktopItem) -> bool {
    if is_internal_transfer_path(Path::new(&item.path)) {
        return true;
    }

    let name = item.name.trim();
    let lower_name = name.to_lowercase();
    let extension = item.extension.trim().to_lowercase();

    if lower_name == "desktop" && extension == "ini" {
        return true;
    }

    if is_app_desktop_entry(&lower_name, &extension) {
        return true;
    }

    false
}

fn is_app_desktop_entry(lower_name: &str, extension: &str) -> bool {
    if !matches!(extension, "lnk" | "dir") {
        return false;
    }

    let compact_name = lower_name
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect::<String>();

    matches!(
        compact_name.as_str(),
        "dustdesk收纳箱"
            | "dustdesk快捷启动"
            | "dustdusk收纳箱"
            | "dustdusk快捷启动"
            | "desknest收纳箱"
            | "desknest快捷启动"
    )
}

fn is_windows_shortcut_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("lnk"))
}

fn is_dustdesk_shortcut_name(path: &Path) -> bool {
    let compact_name = path
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .chars()
        .filter(|ch| !ch.is_whitespace() && !matches!(ch, '-' | '_'))
        .flat_map(char::to_lowercase)
        .collect::<String>();
    matches!(
        compact_name.as_str(),
        "dustdesk"
            | "dustdesk快捷方式"
            | "dustdusk"
            | "dustdusk快捷方式"
            | "desknest"
            | "desknest快捷方式"
    )
}

fn is_dustdesk_executable_target(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            matches!(
                name.to_ascii_lowercase().as_str(),
                "dustdesk-tauri.exe" | "dustdesk.exe" | "dustdusk.exe" | "desknest.exe"
            )
        })
}

fn is_dustdesk_shortcut(path: &Path, target: Option<&Path>) -> bool {
    is_windows_shortcut_path(path)
        && (is_dustdesk_shortcut_name(path) || target.is_some_and(is_dustdesk_executable_target))
}

fn dustdesk_shortcut_target_needs_repair(target: Option<&Path>, current_exe: &Path) -> bool {
    match target {
        None => true,
        Some(target) if same_path_for_move(target, current_exe) => false,
        Some(target) => {
            !target.exists()
                || is_dustdesk_executable_target(target)
                || is_likely_temporary_directory(target)
        }
    }
}

fn is_likely_temporary_directory(path: &Path) -> bool {
    env::var_os("TEMP")
        .into_iter()
        .chain(env::var_os("TMP"))
        .map(PathBuf::from)
        .any(|root| is_path_within(path, &root))
}

fn repair_dustdesk_shortcut_if_needed(path: &Path) -> Result<bool, String> {
    if !is_windows_shortcut_path(path) || !path.exists() {
        return Ok(false);
    }
    let current_exe =
        env::current_exe().map_err(|error| format!("无法定位当前 DustDesk 程序：{error}"))?;
    repair_dustdesk_shortcut_target(path, &current_exe)
}

fn repair_dustdesk_shortcut_best_effort(path: &Path) {
    if let Err(error) = repair_dustdesk_shortcut_if_needed(path) {
        append_desktop_diagnostic(
            "shortcut_repair_deferred",
            &format!(
                "shortcut=\"{}\" error=\"{}\"",
                diagnostic_value(&path.display().to_string()),
                diagnostic_value(&error)
            ),
        );
        eprintln!(
            "DustDesk 快捷方式暂时无法自动修复 {}：{error}",
            path.display()
        );
    }
}

fn repair_dustdesk_shortcut_target(path: &Path, current_exe: &Path) -> Result<bool, String> {
    let old_target = system_icon::shortcut_target_path(path);
    if !is_dustdesk_shortcut(path, old_target.as_deref()) {
        return Ok(false);
    }
    if !dustdesk_shortcut_target_needs_repair(old_target.as_deref(), current_exe) {
        return Ok(false);
    }
    if !current_exe.is_file() {
        return Err(format!(
            "当前 DustDesk 程序不存在，无法修复快捷方式：{}",
            current_exe.display()
        ));
    }

    let old_target_text = old_target
        .as_deref()
        .map(|target| target.display().to_string())
        .unwrap_or_else(|| "<无法读取>".to_owned());
    append_desktop_diagnostic(
        "shortcut_repair_started",
        &format!(
            "shortcut=\"{}\" old_target=\"{}\" new_target=\"{}\"",
            diagnostic_value(&path.display().to_string()),
            diagnostic_value(&old_target_text),
            diagnostic_value(&current_exe.display().to_string())
        ),
    );

    if let Err(error) = system_icon::update_shortcut_target(path, current_exe) {
        append_desktop_diagnostic(
            "shortcut_repair_failed",
            &format!(
                "shortcut=\"{}\" error=\"{}\"",
                diagnostic_value(&path.display().to_string()),
                diagnostic_value(&error)
            ),
        );
        return Err(format!("DustDesk 快捷方式自动修复失败：{error}"));
    }

    let verified_target = system_icon::shortcut_target_path(path)
        .ok_or_else(|| format!("修复后仍无法读取 DustDesk 快捷方式目标：{}", path.display()))?;
    if !same_path_for_move(&verified_target, current_exe) {
        let error = format!(
            "修复后的 DustDesk 快捷方式仍指向 {}",
            verified_target.display()
        );
        append_desktop_diagnostic(
            "shortcut_repair_failed",
            &format!(
                "shortcut=\"{}\" error=\"{}\"",
                diagnostic_value(&path.display().to_string()),
                diagnostic_value(&error)
            ),
        );
        return Err(error);
    }

    append_desktop_diagnostic(
        "shortcut_repair_finished",
        &format!(
            "shortcut=\"{}\" old_target=\"{}\" new_target=\"{}\"",
            diagnostic_value(&path.display().to_string()),
            diagnostic_value(&old_target_text),
            diagnostic_value(&verified_target.display().to_string())
        ),
    );
    Ok(true)
}

fn classify_desktop_item(item: &DesktopItem, categories: &[DeskCategory]) -> Option<usize> {
    let name = item.name.to_lowercase();
    let extension = item.extension.trim().to_lowercase();

    if has_any(
        &name,
        &[
            "此电脑",
            "回收站",
            "控制面板",
            "network",
            "computer",
            "recycle",
        ],
    ) {
        return find_category_index(categories, &["本机", "系统", "电脑"]);
    }

    if has_any(
        &name,
        &[
            "chrome",
            "edge",
            "firefox",
            "brave",
            "opera",
            "browser",
            "adspower",
            "safari",
            "浏览器",
        ],
    ) {
        return find_category_index(categories, &["浏览器", "浏览"]);
    }

    if has_any(
        &name,
        &[
            "微信", "wechat", "qq", "telegram", "飞书", "lark", "钉钉", "dingtalk", "discord",
            "会议", "meeting", "k歌",
        ],
    ) {
        return find_category_index(categories, &["社交", "通讯", "沟通"]);
    }

    if has_any(
        &name,
        &[
            "steam",
            "wegame",
            "game",
            "游戏",
            "qq飞车",
            "三角洲",
            "overcooked",
            "naraka",
            "launcher",
            "demo",
        ],
    ) {
        return find_category_index(categories, &["游戏", "娱乐"]);
    }

    if has_any(
        &name,
        &[
            "visual studio",
            "vscode",
            "code",
            "cursor",
            "devin",
            "orca",
            "pycharm",
            "intellij",
            "idea",
            "jetbrains",
            "hbuilder",
            "git",
            "github",
            "terminal",
            "powershell",
            "xterminal",
            "navicat",
            "apifox",
            "postman",
            "mcp",
            "tabbit",
            "augment",
            "agent",
            "coding",
            "project",
            "项目",
            "开发",
        ],
    ) {
        return find_category_index(categories, &["开发", "编程", "代码"]);
    }

    if document_extension(&extension)
        || has_any(
            &name,
            &[
                "pdf", "doc", "word", "excel", "ppt", "简历", "resume", "文档", "方案", "资料",
                "接口", "标准", "文件", "设计", "模板",
            ],
        )
    {
        return find_category_index(categories, &["文档", "资料", "文件"]);
    }

    if has_any(
        &name,
        &[
            "wps",
            "office",
            "word",
            "excel",
            "powerpoint",
            "pdfgear",
            "typora",
            "microsoft",
            "办公",
        ],
    ) {
        return find_category_index(categories, &["办公软件", "办公"]);
    }

    if has_any(
        &name,
        &[
            "tool",
            "tools",
            "manager",
            "viewer",
            "清理",
            "工具",
            "geek",
            "vnc",
            "mihomo",
            "logitech",
            "openless",
            "easyconnect",
            "golutra",
            "regable",
        ],
    ) {
        return find_category_index(categories, &["工具", "实用"]);
    }

    if item.is_dir {
        return find_category_index(categories, &["文档", "资料", "文件"]).or(Some(0));
    }

    find_category_index(categories, &["工具", "本机"]).or(Some(0))
}

fn document_extension(extension: &str) -> bool {
    matches!(
        extension,
        "pdf"
            | "doc"
            | "docx"
            | "xls"
            | "xlsx"
            | "ppt"
            | "pptx"
            | "txt"
            | "md"
            | "rtf"
            | "csv"
            | "html"
            | "htm"
            | "png"
            | "jpg"
            | "jpeg"
            | "webp"
            | "gif"
            | "svg"
    )
}

fn find_category_index(categories: &[DeskCategory], aliases: &[&str]) -> Option<usize> {
    categories.iter().position(|category| {
        let name = category.name.to_lowercase();
        aliases.iter().any(|alias| name.contains(alias))
    })
}

fn categories_contain_path(categories: &[DeskCategory], path: &str) -> bool {
    categories.iter().any(|category| {
        category
            .item_paths
            .iter()
            .any(|item_path| same_path_text(item_path, path))
    })
}

fn desktop_icon_position_for_path(
    positions: &BTreeMap<String, DesktopIconPosition>,
    path: &str,
) -> Option<DesktopIconPosition> {
    positions
        .iter()
        .find_map(|(candidate, position)| same_path_text(candidate, path).then(|| position.clone()))
}

fn remove_desktop_icon_position_for_path(
    positions: &mut BTreeMap<String, DesktopIconPosition>,
    path: &str,
) -> Option<DesktopIconPosition> {
    let key = positions
        .keys()
        .find(|candidate| same_path_text(candidate, path))
        .cloned()?;
    positions.remove(&key)
}

fn set_desktop_icon_position_for_path(
    positions: &mut BTreeMap<String, DesktopIconPosition>,
    path: String,
    position: DesktopIconPosition,
) {
    remove_desktop_icon_position_for_path(positions, &path);
    positions.insert(path, position);
}

fn retain_desktop_icon_positions_for_categories(config: &mut AppConfig) -> bool {
    let organized_paths = config
        .desktop_categories
        .iter()
        .flat_map(|category| category.item_paths.iter().cloned())
        .collect::<Vec<_>>();
    let previous_len = config.desktop_icon_positions.len();
    config.desktop_icon_positions.retain(|path, _| {
        organized_paths
            .iter()
            .any(|organized_path| same_path_text(organized_path, path))
    });
    config.desktop_icon_positions.len() != previous_len
}

fn has_any(value: &str, keywords: &[&str]) -> bool {
    keywords.iter().any(|keyword| value.contains(keyword))
}

fn archive_item_path(store: &AppStore, category_name: &str, path: &str) -> Result<String, String> {
    let source = PathBuf::from(path);
    if !source.exists() {
        append_desktop_diagnostic(
            "archive_failed",
            &format!(
                "source=\"{}\" error=\"源项目不存在\"",
                diagnostic_value(&source.display().to_string())
            ),
        );
        return Err(format!("源项目不存在：{}", source.display()));
    }

    let organizer_root = store.organizer_root();
    let category_dir = organizer_root.join(safe_windows_file_name(category_name));
    if let Err(error) = validate_archive_source(store, &source, &organizer_root, &category_dir) {
        append_desktop_diagnostic(
            "archive_rejected",
            &format!(
                "source=\"{}\" category=\"{}\" error=\"{}\"",
                diagnostic_value(&source.display().to_string()),
                diagnostic_value(category_name),
                diagnostic_value(&error)
            ),
        );
        return Err(error);
    }
    if is_path_within(&source, &category_dir) {
        return Ok(path.to_owned());
    }
    fs::create_dir_all(&category_dir).map_err(to_message)?;

    for _ in 0..1000 {
        let destination = archive_destination(&category_dir, &source)?;
        append_desktop_diagnostic(
            "archive_started",
            &format!(
                "source=\"{}\" destination=\"{}\" current_exe=\"{}\"",
                diagnostic_value(&source.display().to_string()),
                diagnostic_value(&destination.display().to_string()),
                diagnostic_value(&current_exe_diagnostic_value())
            ),
        );
        match move_path(&source, &destination) {
            Ok(()) => {
                notify_shell_path_moved(&source, &destination);
                if let Err(error) = repair_dustdesk_shortcut_if_needed(&destination) {
                    eprintln!(
                        "failed to repair archived DustDesk shortcut {}: {error}",
                        destination.display()
                    );
                }
                append_desktop_diagnostic(
                    "archive_committed",
                    &format!(
                        "source=\"{}\" destination=\"{}\"",
                        diagnostic_value(&source.display().to_string()),
                        diagnostic_value(&destination.display().to_string())
                    ),
                );
                return Ok(destination.display().to_string());
            }
            Err(error) if error.contains("目标项目已存在") => continue,
            Err(error) => {
                append_desktop_diagnostic(
                    "archive_failed",
                    &format!(
                        "source=\"{}\" destination=\"{}\" error=\"{}\"",
                        diagnostic_value(&source.display().to_string()),
                        diagnostic_value(&destination.display().to_string()),
                        diagnostic_value(&error)
                    ),
                );
                return Err(error);
            }
        }
    }

    Err(format!("无法生成可用的收纳目标名称：{}", source.display()))
}

fn validate_archive_source(
    store: &AppStore,
    source: &Path,
    organizer_root: &Path,
    category_dir: &Path,
) -> Result<(), String> {
    let current_exe =
        env::current_exe().map_err(|error| format!("无法验证 DustDesk 安装目录：{error}"))?;
    if is_protected_app_runtime_source(source, &current_exe) {
        return Err(format!(
            "不能收纳当前 DustDesk 程序或其安装目录：{}",
            source.display()
        ));
    }

    let protected_roots = [
        store.data_dir(),
        organizer_root.to_path_buf(),
        store.launchers_root(),
    ];
    if protected_roots
        .iter()
        .any(|root| same_path_for_move(source, root))
        || desktop_roots()
            .iter()
            .any(|root| same_path_for_move(source, root))
    {
        return Err(format!(
            "不能收纳系统或 DustDesk 结构目录：{}",
            source.display()
        ));
    }
    if same_path_for_move(source, category_dir) || is_target_inside_source(source, category_dir) {
        return Err(format!("不能把目录迁移到其自身内部：{}", source.display()));
    }
    Ok(())
}

fn is_protected_app_runtime_source(source: &Path, current_exe: &Path) -> bool {
    same_path_for_move(source, current_exe) || is_target_inside_source(source, current_exe)
}

fn archive_destination(category_dir: &Path, source: &Path) -> Result<PathBuf, String> {
    source
        .file_name()
        .ok_or_else(|| "无法识别项目名称".to_owned())?;
    Ok(unique_destination(category_dir, source))
}

fn repair_category_item_paths(store: &AppStore, config: &mut AppConfig) -> Result<(), String> {
    repair_category_item_paths_with_desktop_roots(store, config, &desktop_roots())
}

fn repair_category_item_paths_for_desktop(
    store: &AppStore,
    config: &mut AppConfig,
    desktop: &Path,
) -> Result<(), String> {
    repair_category_item_paths_with_desktop_roots(store, config, &[desktop.to_path_buf()])
}

fn repair_category_item_paths_with_desktop_roots(
    store: &AppStore,
    config: &mut AppConfig,
    desktop_roots: &[PathBuf],
) -> Result<(), String> {
    let organizer_root = store.organizer_root();
    let mut changed = recover_missing_categories_from_organizer(&organizer_root, config);
    let mut seen_paths = Vec::<String>::new();

    for category in &mut config.desktop_categories {
        let old_paths = std::mem::take(&mut category.item_paths);
        let mut repaired_paths = Vec::with_capacity(old_paths.len());

        for item_path in old_paths {
            let source = recover_existing_path_from_corrupted_text(&item_path)
                .unwrap_or_else(|| PathBuf::from(&item_path));
            if is_desktop_ini_path(&source) || is_internal_transfer_path(&source) {
                changed = true;
                continue;
            }
            if !source.exists() {
                changed = true;
                continue;
            }
            repair_dustdesk_shortcut_best_effort(&source);
            if !same_path_text(&item_path, &source.display().to_string()) {
                changed = true;
            }

            if desktop_roots
                .iter()
                .any(|desktop| is_path_within(&source, desktop))
                && !is_path_within(&source, &organizer_root)
            {
                // Desktop paths are not organization markers. Only the independent restart marker
                // may recollect an item after launch; stale config references are removed here.
                changed = true;
                continue;
            }
            let repaired_text = source.display().to_string();

            if !same_path_text(&source.display().to_string(), &repaired_text) {
                changed = true;
            }
            if seen_paths
                .iter()
                .any(|seen_path| same_path_text(seen_path, &repaired_text))
            {
                changed = true;
                continue;
            }
            seen_paths.push(repaired_text.clone());
            changed |= push_unique_text_path(&mut repaired_paths, repaired_text);
        }

        category.item_paths = repaired_paths;
    }

    let category_dirs = fs::read_dir(&organizer_root)
        .ok()
        .into_iter()
        .flat_map(|entries| entries.flatten())
        .map(|entry| entry.path())
        .filter(|path| path.is_dir() && !is_internal_transfer_path(path))
        .collect::<Vec<_>>();
    for category_dir in category_dirs {
        let Some(category_index) = category_index_for_organizer_dir(
            &organizer_root,
            &config.desktop_categories,
            &category_dir,
        ) else {
            continue;
        };
        let Ok(entries) = fs::read_dir(&category_dir) else {
            continue;
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if is_desktop_ini_path(&path) || is_internal_transfer_path(&path) {
                continue;
            }
            repair_dustdesk_shortcut_best_effort(&path);

            let path_text = path.display().to_string();
            if seen_paths
                .iter()
                .any(|seen_path| same_path_text(seen_path, &path_text))
            {
                continue;
            }

            seen_paths.push(path_text.clone());
            config.desktop_categories[category_index]
                .item_paths
                .push(path_text);
            changed = true;
        }
    }

    changed |= retain_desktop_icon_positions_for_categories(config);

    if changed {
        store.save_config(config).map_err(to_message)?;
    }

    Ok(())
}

fn recover_missing_categories_from_organizer(
    organizer_root: &Path,
    config: &mut AppConfig,
) -> bool {
    let Ok(entries) = fs::read_dir(organizer_root) else {
        return false;
    };
    let mut changed = false;

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() || is_internal_transfer_path(&path) {
            continue;
        }
        let has_user_items = fs::read_dir(&path)
            .ok()
            .into_iter()
            .flat_map(|items| items.flatten())
            .any(|item| {
                let item_path = item.path();
                !is_internal_transfer_path(&item_path) && !is_desktop_ini_path(&item_path)
            });
        if !has_user_items {
            continue;
        }
        if category_index_for_organizer_dir(organizer_root, &config.desktop_categories, &path)
            .is_some()
        {
            continue;
        }
        let Some(name) = path
            .file_name()
            .map(|name| name.to_string_lossy().trim().to_owned())
            .filter(|name| !name.is_empty())
        else {
            continue;
        };
        config.desktop_categories.push(DeskCategory {
            name,
            ..DeskCategory::default()
        });
        changed = true;
    }

    changed
}

fn category_index_for_organizer_dir(
    organizer_root: &Path,
    categories: &[DeskCategory],
    category_dir: &Path,
) -> Option<usize> {
    categories.iter().position(|category| {
        same_path_for_move(
            &organizer_root.join(safe_windows_file_name(&category.name)),
            category_dir,
        ) || category
            .item_paths
            .iter()
            .any(|item_path| is_path_within(Path::new(item_path), category_dir))
    })
}

fn push_unique_text_path(paths: &mut Vec<String>, path: String) -> bool {
    if paths.iter().any(|existing| same_path_text(existing, &path)) {
        true
    } else {
        paths.push(path);
        false
    }
}

fn recover_existing_path_from_corrupted_text(path_text: &str) -> Option<PathBuf> {
    let direct = PathBuf::from(path_text);
    if direct.exists() {
        return Some(direct);
    }

    let lower = path_text.to_ascii_lowercase();
    for extension in [".lnk", ".url", ".exe", ".bat", ".cmd", ".ps1", ".msi"] {
        let Some(index) = lower.find(extension) else {
            continue;
        };
        let end = index + extension.len();
        if end >= path_text.len() {
            continue;
        }
        let candidate = PathBuf::from(&path_text[..end]);
        if candidate.exists() {
            return Some(candidate);
        }
    }

    None
}

fn is_path_within(path: &Path, root: &Path) -> bool {
    let path = normalize_path_for_compare(path);
    let root = normalize_path_for_compare(root);
    path == root || path.starts_with(&format!("{root}\\"))
}

fn normalize_path_for_compare(path: &Path) -> String {
    let normalized = path
        .to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_owned();
    let without_extended_prefix = if let Some(rest) = normalized.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = normalized.strip_prefix(r"\\?\") {
        rest.to_owned()
    } else {
        normalized
    };
    without_extended_prefix.to_ascii_lowercase()
}

fn safe_windows_file_name(value: &str) -> String {
    let mut name = value
        .chars()
        .map(|ch| match ch {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            _ => ch,
        })
        .collect::<String>()
        .trim()
        .trim_matches('.')
        .to_owned();

    if name.is_empty() {
        name = "启动项".to_owned();
    }

    name
}

fn unique_destination(destination_dir: &Path, source: &Path) -> PathBuf {
    let file_name = source
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| "项目".to_owned());
    let first = destination_dir.join(&file_name);
    if !first.exists() {
        return first;
    }

    let stem = source
        .file_stem()
        .map(|name| name.to_string_lossy().to_string())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| file_name.clone());
    let extension = source
        .extension()
        .map(|name| name.to_string_lossy().to_string())
        .filter(|name| !name.trim().is_empty());

    for index in 2..1000 {
        let name = if source.is_dir() {
            format!("{stem} {index}")
        } else if let Some(extension) = &extension {
            format!("{stem} {index}.{extension}")
        } else {
            format!("{stem} {index}")
        };
        let candidate = destination_dir.join(name);
        if !candidate.exists() {
            return candidate;
        }
    }

    destination_dir.join(format!("{stem} {}", now_id()))
}

fn move_path(source: &Path, destination: &Path) -> Result<(), String> {
    if same_path_for_move(source, destination) {
        return Ok(());
    }
    if !source.exists() {
        return Err(format!("源项目不存在：{}", source.display()));
    }
    if is_target_inside_source(source, destination) {
        return Err(format!(
            "不能把目录迁移到其自身内部：{} -> {}",
            source.display(),
            destination.display()
        ));
    }
    if destination.exists() {
        return Err(format!("目标项目已存在：{}", destination.display()));
    }
    validate_transfer_source_tree(source)?;

    if rename_path_no_replace(source, destination).is_ok() {
        return Ok(());
    }
    if destination.exists() {
        return Err(format!("目标项目已存在：{}", destination.display()));
    }

    move_path_via_staging(source, destination)
}

fn move_path_via_staging(source: &Path, destination: &Path) -> Result<(), String> {
    if source.is_dir() {
        transfer_path_via_staging_with_verifier(source, destination, true, |_, _| Ok(()))
    } else {
        transfer_path_via_staging_with_verifier(source, destination, true, verify_copied_path)
    }
}

#[cfg(test)]
fn move_path_via_staging_with_verifier(
    source: &Path,
    destination: &Path,
    verifier: impl FnOnce(&Path, &Path) -> Result<(), String>,
) -> Result<(), String> {
    transfer_path_via_staging_with_verifier(source, destination, true, verifier)
}

fn copy_path_via_staging(source: &Path, destination: &Path) -> Result<(), String> {
    transfer_path_via_staging_with_verifier(source, destination, false, verify_copied_path)
}

fn transfer_path_via_staging_with_verifier(
    source: &Path,
    destination: &Path,
    remove_source: bool,
    verifier: impl FnOnce(&Path, &Path) -> Result<(), String>,
) -> Result<(), String> {
    let mut quarantine = if cfg!(windows) && remove_source && source.is_dir() {
        Some(QuarantinedTransferSource::reserve(source, destination)?)
    } else {
        None
    };
    let transfer_source = quarantine
        .as_ref()
        .map(|item| item.payload.clone())
        .unwrap_or_else(|| source.to_path_buf());
    let result = transfer_reserved_path_via_staging(
        &transfer_source,
        destination,
        remove_source,
        quarantine.as_mut(),
        verifier,
    );

    if let Some(quarantine) = quarantine.as_mut() {
        if result.is_err() && !quarantine.destination_committed {
            if let Err(restore_error) = quarantine.restore() {
                return Err(format!(
                    "{}；隔离源目录恢复失败：{restore_error}",
                    result.expect_err("checked transfer error")
                ));
            }
        } else if quarantine.source_deleted {
            if let Err(error) = quarantine.cleanup_metadata() {
                eprintln!(
                    "failed to cleanup committed transfer quarantine {}: {error}",
                    quarantine.container.display()
                );
            }
        }
    }
    result
}

fn transfer_reserved_path_via_staging(
    source: &Path,
    destination: &Path,
    remove_source: bool,
    mut quarantine: Option<&mut QuarantinedTransferSource>,
    verifier: impl FnOnce(&Path, &Path) -> Result<(), String>,
) -> Result<(), String> {
    let locked_source = lock_transfer_source_tree(source)?;
    let mut reserved_staging = reserve_transfer_staging_path(source, destination)?;
    let staging = reserved_staging.path.clone();
    let mut staging_commit_root = None;
    let mut committed = false;
    let transfer_result = (|| {
        if source.is_dir() {
            copy_directory_for_transfer(source, &staging)
                .map_err(|error| format!("复制目录到临时位置失败：{error}"))?;
            drop(reserved_staging.root.take());
            reserved_staging.root =
                Some(open_reserved_transfer_directory(&staging).map_err(|error| {
                    format!("重新锁定临时目录失败 {}：{error}", staging.display())
                })?);
        } else if source.is_file() {
            copy_file_contents(source, &staging)?;
            reserved_staging.root =
                Some(open_reserved_transfer_file(&staging).map_err(to_message)?);
        } else {
            return Err(format!("不支持移动此项目：{}", source.display()));
        }

        verifier(source, &staging)?;
        verify_copied_path_shape(source, &staging)
            .map_err(|error| format!("迁移结构校验失败：{error}"))?;
        staging_commit_root = reserved_staging.root.take();
        commit_staging_handle_no_replace(
            staging_commit_root
                .as_ref()
                .ok_or_else(|| "无法锁定临时迁移项目".to_owned())?,
            &staging,
            destination,
        )
        .map_err(|error| format!("提交临时迁移目录失败：{error}"))?;
        committed = true;
        if remove_source {
            if let Some(quarantine) = quarantine.as_deref_mut() {
                quarantine.destination_committed = true;
                if let Err(error) = quarantine.mark_committed() {
                    eprintln!(
                        "failed to persist committed transfer marker for {}: {error}",
                        quarantine.original.display()
                    );
                    drop(locked_source);
                    return Ok(());
                }
                #[cfg(test)]
                {
                    if let Err(error) = quarantine.mark_cleanup_started() {
                        eprintln!(
                            "failed to persist transfer cleanup marker for {}: {error}",
                            quarantine.original.display()
                        );
                        drop(locked_source);
                    } else {
                        match delete_locked_source_tree(locked_source) {
                            Ok(()) => quarantine.source_deleted = true,
                            Err(error) => eprintln!(
                                "committed transfer source cleanup deferred for {}: {error}",
                                quarantine.original.display()
                            ),
                        }
                    }
                }
                #[cfg(not(test))]
                {
                    drop(locked_source);
                    let journal_path = quarantine.journal_path.clone();
                    if let Err(error) = std::thread::Builder::new()
                        .name("dustdesk-transfer-cleanup".to_owned())
                        .spawn(move || {
                            if let Err(error) =
                                recover_transfer_quarantine_journal_serialized(&journal_path)
                            {
                                eprintln!(
                                    "failed to cleanup committed transfer quarantine {}: {error}",
                                    journal_path.display()
                                );
                            }
                        })
                    {
                        eprintln!("failed to start committed transfer cleanup: {error}");
                    }
                }
                Ok(())
            } else {
                delete_locked_source_tree(locked_source)
            }
        } else {
            drop(locked_source);
            Ok(())
        }
    })();

    if !committed {
        drop(reserved_staging.root.take());
        drop(staging_commit_root.take());
        let cleanup_root = if staging.is_dir() {
            open_reserved_transfer_directory(&staging).ok()
        } else if staging.is_file() {
            open_reserved_transfer_file(&staging).ok()
        } else {
            None
        };
        let cleanup_tree =
            cleanup_root.and_then(|root| lock_transfer_source_tree_from_root(&staging, root).ok());
        if let Some(locked) = cleanup_tree {
            let _ = delete_locked_source_tree(locked);
        }
    }
    if let Some((container_path, container_root)) = reserved_staging.container.take() {
        if let Ok(locked) = lock_transfer_source_tree_from_root(&container_path, container_root) {
            let _ = delete_locked_source_tree(locked);
        }
    }
    transfer_result
}

struct ReservedTransferStaging {
    path: PathBuf,
    root: Option<File>,
    container: Option<(PathBuf, File)>,
}

#[derive(Debug, Serialize, Deserialize)]
struct TransferQuarantineJournal {
    version: u32,
    original_path_utf16: Vec<u16>,
    destination_path_utf16: Vec<u16>,
    container_path_utf16: Vec<u16>,
}

struct QuarantinedTransferSource {
    original: PathBuf,
    destination: PathBuf,
    container: PathBuf,
    payload: PathBuf,
    journal_path: PathBuf,
    committed_marker_path: PathBuf,
    cleanup_started_marker_path: PathBuf,
    destination_committed: bool,
    source_deleted: bool,
}

impl QuarantinedTransferSource {
    fn reserve(source: &Path, destination: &Path) -> Result<Self, String> {
        let parent = source
            .parent()
            .ok_or_else(|| "无法识别源目录位置".to_owned())?;
        let file_name = source
            .file_name()
            .ok_or_else(|| "无法识别源目录名称".to_owned())?;
        let journal_root = transfer_quarantine_journal_root()?;
        fs::create_dir_all(&journal_root).map_err(to_message)?;
        let id = now_id();

        for index in 0..1000 {
            let container = parent.join(format!(".dustdesk-source-transfer-{id}-{index}.tmp"));
            match fs::create_dir(&container) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(to_message(error)),
            }
            hide_transfer_staging_path(&container);

            let payload = container.join(file_name);
            let journal_path = journal_root.join(format!("source-transfer-{id}-{index}.json"));
            let committed_marker_path = journal_path.with_extension("committed");
            let cleanup_started_marker_path = journal_path.with_extension("cleaning");
            let journal = TransferQuarantineJournal {
                version: 1,
                original_path_utf16: path_to_utf16(source),
                destination_path_utf16: path_to_utf16(destination),
                container_path_utf16: path_to_utf16(&container),
            };
            if let Err(error) = write_new_transfer_quarantine_journal(&journal_path, &journal) {
                let _ = remove_existing_path(&container);
                if error.kind() == io::ErrorKind::AlreadyExists {
                    continue;
                }
                return Err(to_message(error));
            }

            if let Err(error) = rename_path_no_replace(source, &payload) {
                let _ = fs::remove_file(&journal_path);
                let _ = remove_existing_path(&container);
                return Err(error);
            }

            return Ok(Self {
                original: source.to_path_buf(),
                destination: destination.to_path_buf(),
                container,
                payload,
                journal_path,
                committed_marker_path,
                cleanup_started_marker_path,
                destination_committed: false,
                source_deleted: false,
            });
        }

        Err("无法创建源目录隔离区".to_owned())
    }

    fn mark_committed(&self) -> Result<(), String> {
        write_transfer_marker(&self.committed_marker_path, b"committed\n")
    }

    fn mark_cleanup_started(&self) -> Result<(), String> {
        write_transfer_marker(&self.cleanup_started_marker_path, b"cleaning\n")
    }

    fn restore(&mut self) -> Result<(), String> {
        if self.cleanup_started_marker_path.exists() {
            return Err(format!(
                "隔离源清理已经开始，为避免恢复不完整目录，数据保留在 {}",
                self.container.display()
            ));
        }
        if self.payload.exists() {
            if self.original.exists() {
                return Err(format!(
                    "原位置已存在同名项目，隔离数据保留在 {}",
                    self.container.display()
                ));
            }
            rename_path_no_replace(&self.payload, &self.original)?;
            notify_shell_parent_updated(&self.original);
        } else if !self.original.exists() {
            return Err(format!(
                "隔离源目录和原目录都不存在：{}",
                self.original.display()
            ));
        }
        self.cleanup_metadata()
    }

    fn cleanup_metadata(&self) -> Result<(), String> {
        if self.payload.exists() {
            return Err(format!("隔离源目录仍有数据：{}", self.payload.display()));
        }
        match fs::remove_dir(&self.container) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(to_message(error)),
        }
        match fs::remove_file(&self.cleanup_started_marker_path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(to_message(error)),
        }
        match fs::remove_file(&self.committed_marker_path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(to_message(error)),
        }
        match fs::remove_file(&self.journal_path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(to_message(error)),
        }
    }

    fn from_journal(
        journal_path: PathBuf,
        journal: TransferQuarantineJournal,
    ) -> Result<Self, String> {
        if journal.version != 1 {
            return Err("不支持的迁移隔离日志版本".to_owned());
        }
        let original = path_from_utf16(&journal.original_path_utf16)?;
        let destination = path_from_utf16(&journal.destination_path_utf16)?;
        let container = path_from_utf16(&journal.container_path_utf16)?;
        if !is_source_transfer_staging_path(&container)
            || original.parent().map(normalize_path_for_compare)
                != container.parent().map(normalize_path_for_compare)
        {
            return Err("迁移隔离日志路径校验失败".to_owned());
        }
        let file_name = original
            .file_name()
            .ok_or_else(|| "迁移隔离日志缺少源目录名称".to_owned())?;
        let payload = container.join(file_name);
        Ok(Self {
            original,
            destination,
            container,
            payload,
            committed_marker_path: journal_path.with_extension("committed"),
            cleanup_started_marker_path: journal_path.with_extension("cleaning"),
            journal_path,
            destination_committed: false,
            source_deleted: false,
        })
    }
}

fn write_transfer_marker(path: &Path, contents: &[u8]) -> Result<(), String> {
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(mut file) => {
            file.write_all(contents).map_err(to_message)?;
            file.sync_all().map_err(to_message)
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(to_message(error)),
    }
}

fn transfer_quarantine_journal_root() -> Result<PathBuf, String> {
    #[cfg(test)]
    {
        return Ok(env::temp_dir().join("dustdesk-transfer-journals-test"));
    }
    #[cfg(not(test))]
    {
        let appdata = env::var_os("APPDATA").ok_or_else(|| "没有找到 APPDATA 目录".to_owned())?;
        Ok(PathBuf::from(appdata)
            .join("DustDesk")
            .join("transfer-journals"))
    }
}

fn write_new_transfer_quarantine_journal(
    path: &Path,
    journal: &TransferQuarantineJournal,
) -> io::Result<()> {
    let bytes = serde_json::to_vec(journal).map_err(io::Error::other)?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    if let Err(error) = file.write_all(&bytes).and_then(|()| file.sync_all()) {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(error);
    }
    Ok(())
}

#[cfg(windows)]
fn path_to_utf16(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().collect()
}

#[cfg(not(windows))]
fn path_to_utf16(path: &Path) -> Vec<u16> {
    path.to_string_lossy().encode_utf16().collect()
}

#[cfg(windows)]
fn path_from_utf16(value: &[u16]) -> Result<PathBuf, String> {
    if value.is_empty() || value.contains(&0) {
        return Err("迁移隔离日志包含无效路径".to_owned());
    }
    Ok(PathBuf::from(OsString::from_wide(value)))
}

#[cfg(not(windows))]
fn path_from_utf16(value: &[u16]) -> Result<PathBuf, String> {
    String::from_utf16(value)
        .map(PathBuf::from)
        .map_err(to_message)
}

fn reserve_transfer_staging_path(
    source: &Path,
    destination: &Path,
) -> Result<ReservedTransferStaging, String> {
    let parent = destination
        .parent()
        .ok_or_else(|| "无法识别目标目录".to_owned())?;
    let source_metadata = fs::symlink_metadata(source).map_err(to_message)?;
    validate_transfer_metadata(source, &source_metadata)?;
    for index in 0..1000 {
        let candidate = parent.join(format!(".dustdesk-transfer-{}-{index}.tmp", now_id()));
        if source_metadata.is_dir() {
            match fs::create_dir(&candidate) {
                Ok(()) => {
                    hide_transfer_staging_path(&candidate);
                    let root = open_transfer_directory_guard(&candidate).map_err(to_message)?;
                    return Ok(ReservedTransferStaging {
                        path: candidate,
                        root: Some(root),
                        container: None,
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(to_message(error)),
            }
        } else if source_metadata.is_file() {
            match fs::create_dir(&candidate) {
                Ok(()) => {
                    hide_transfer_staging_path(&candidate);
                    let container_root =
                        open_reserved_transfer_directory(&candidate).map_err(to_message)?;
                    return Ok(ReservedTransferStaging {
                        path: candidate.join("payload"),
                        root: None,
                        container: Some((candidate, container_root)),
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(to_message(error)),
            }
        } else {
            return Err(format!("不支持移动此项目：{}", source.display()));
        }
    }
    Err("无法创建临时迁移项目".to_owned())
}

#[cfg(windows)]
fn hide_transfer_staging_path(path: &Path) {
    use windows_sys::Win32::Storage::FileSystem::{
        SetFileAttributesW, FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_SYSTEM, FILE_ATTRIBUTE_TEMPORARY,
    };

    let wide_path = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    unsafe {
        let _ = SetFileAttributesW(
            wide_path.as_ptr(),
            FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM | FILE_ATTRIBUTE_TEMPORARY,
        );
    }
}

#[cfg(not(windows))]
fn hide_transfer_staging_path(_path: &Path) {}

fn cleanup_stale_transfer_staging_dirs(root: &Path, minimum_age: Duration) -> usize {
    let Ok(entries) = fs::read_dir(root) else {
        return 0;
    };
    let now = SystemTime::now();
    let mut removed = 0usize;

    for entry in entries.flatten() {
        let path = entry.path();
        if !is_transfer_staging_path(&path) {
            continue;
        }
        let is_old_enough = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_none_or(|age| age >= minimum_age);
        if !is_old_enough {
            continue;
        }
        match remove_existing_path(&path) {
            Ok(()) => removed += 1,
            Err(error) => eprintln!(
                "failed to cleanup transfer staging {}: {error}",
                path.display()
            ),
        }
    }

    removed
}

fn recover_transfer_quarantine_journals() {
    recover_transfer_quarantine_journals_with_mode(false);
}

fn recover_transfer_quarantine_journals_with_mode(defer_committed_cleanup: bool) {
    if let Err(error) = recover_transfer_quarantine_journals_strict(defer_committed_cleanup) {
        eprintln!("failed to recover transfer quarantine journals: {error}");
    }
}

fn recover_transfer_quarantine_journals_strict(
    defer_committed_cleanup: bool,
) -> Result<(), String> {
    recover_transfer_quarantine_journals_filtered_strict(defer_committed_cleanup, None)
}

fn recover_related_transfer_quarantine_journals_strict(
    related_paths: &[PathBuf],
) -> Result<(), String> {
    if related_paths.is_empty() {
        return Ok(());
    }
    recover_transfer_quarantine_journals_filtered_strict(false, Some(related_paths))
}

fn recover_transfer_quarantine_journals_filtered_strict(
    defer_committed_cleanup: bool,
    related_paths: Option<&[PathBuf]>,
) -> Result<(), String> {
    let _guard = TRANSFER_RECOVERY_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| "迁移隔离恢复锁已损坏".to_owned())?;
    let journal_root = transfer_quarantine_journal_root()?;
    let entries = match fs::read_dir(&journal_root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(to_message(error)),
    };
    let mut errors = Vec::new();

    for entry in entries.flatten() {
        let journal_path = entry.path();
        let is_journal = journal_path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("source-transfer-") && name.ends_with(".json"));
        if !is_journal {
            continue;
        }
        if let Some(related_paths) = related_paths {
            match transfer_quarantine_journal_matches_paths(&journal_path, related_paths) {
                Ok(true) => {}
                Ok(false) => continue,
                Err(error) => {
                    errors.push(format!("{}: {error}", journal_path.display()));
                    continue;
                }
            }
        }
        if let Err(error) =
            recover_transfer_quarantine_journal_with_mode(&journal_path, defer_committed_cleanup)
        {
            errors.push(format!("{}: {error}", journal_path.display()));
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("；"))
    }
}

fn transfer_quarantine_journal_matches_paths(
    journal_path: &Path,
    related_paths: &[PathBuf],
) -> Result<bool, String> {
    let bytes = fs::read(journal_path).map_err(to_message)?;
    let journal =
        serde_json::from_slice::<TransferQuarantineJournal>(&bytes).map_err(to_message)?;
    let original = path_from_utf16(&journal.original_path_utf16)?;
    let destination = path_from_utf16(&journal.destination_path_utf16)?;
    Ok(related_paths.iter().any(|path| {
        paths_overlap_for_transfer_recovery(path, &original)
            || paths_overlap_for_transfer_recovery(path, &destination)
    }))
}

fn paths_overlap_for_transfer_recovery(first: &Path, second: &Path) -> bool {
    is_path_within(first, second) || is_path_within(second, first)
}

#[cfg(not(test))]
fn recover_transfer_quarantine_journal_serialized(journal_path: &Path) -> Result<(), String> {
    let _guard = TRANSFER_RECOVERY_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| "迁移隔离恢复锁已损坏".to_owned())?;
    if !journal_path.exists() {
        return Ok(());
    }
    recover_transfer_quarantine_journal(journal_path)
}

fn recover_transfer_quarantine_journal(journal_path: &Path) -> Result<(), String> {
    recover_transfer_quarantine_journal_with_mode(journal_path, false)
}

fn recover_transfer_quarantine_journal_with_mode(
    journal_path: &Path,
    defer_committed_cleanup: bool,
) -> Result<(), String> {
    let bytes = fs::read(journal_path).map_err(to_message)?;
    let journal =
        serde_json::from_slice::<TransferQuarantineJournal>(&bytes).map_err(to_message)?;
    let mut quarantine =
        QuarantinedTransferSource::from_journal(journal_path.to_path_buf(), journal)?;
    let committed = quarantine.committed_marker_path.exists();
    let cleanup_started = quarantine.cleanup_started_marker_path.exists();
    quarantine.destination_committed = committed;

    if committed {
        if !quarantine.destination.exists() {
            if cleanup_started {
                return Err(format!(
                    "目标目录已不存在且隔离源清理已经开始，已保留剩余数据：{}",
                    quarantine.container.display()
                ));
            }
            return quarantine.restore();
        }
        if defer_committed_cleanup {
            return Ok(());
        }
        if quarantine.payload.exists() {
            if !cleanup_started {
                verify_copied_path_shape(&quarantine.payload, &quarantine.destination).map_err(
                    |error| {
                        format!(
                            "提交后的目标复核失败，已保留隔离源 {}：{error}",
                            quarantine.container.display()
                        )
                    },
                )?;
                quarantine.mark_cleanup_started()?;
            }
            delete_transfer_quarantine_payload(&quarantine.payload)?;
        }
        quarantine.source_deleted = true;
        return quarantine.cleanup_metadata();
    }

    if !quarantine.payload.exists() {
        if quarantine.original.exists() || quarantine.destination.exists() {
            return quarantine.cleanup_metadata();
        }
        return Err(format!(
            "迁移隔离数据、原目录和目标目录均不存在：{}",
            quarantine.original.display()
        ));
    }

    if quarantine.destination.exists()
        && verify_copied_path_shape(&quarantine.payload, &quarantine.destination).is_ok()
    {
        quarantine.mark_committed()?;
        quarantine.destination_committed = true;
        if defer_committed_cleanup {
            return Ok(());
        }
        quarantine.mark_cleanup_started()?;
        delete_transfer_quarantine_payload(&quarantine.payload)?;
        quarantine.source_deleted = true;
        quarantine.cleanup_metadata()
    } else {
        quarantine.restore()
    }
}

fn delete_transfer_quarantine_payload(path: &Path) -> Result<(), String> {
    let locked = lock_transfer_source_tree(path)?;
    delete_locked_source_tree(locked)
}

fn cleanup_transfer_staging_dirs(store: &AppStore, minimum_age: Duration) {
    for root in desktop_roots() {
        let removed = cleanup_stale_transfer_staging_dirs(&root, minimum_age);
        if removed > 0 {
            notify_shell_directory_updated(&root);
        }
    }

    let organizer_root = store.organizer_root();
    let removed = cleanup_stale_transfer_staging_dirs(&organizer_root, minimum_age);
    if removed > 0 {
        notify_shell_directory_updated(&organizer_root);
    }

    if let Ok(categories) = fs::read_dir(&organizer_root) {
        for entry in categories.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let removed = cleanup_stale_transfer_staging_dirs(&path, minimum_age);
            if removed > 0 {
                notify_shell_directory_updated(&path);
            }
        }
    }
}

fn schedule_stale_transfer_staging_cleanup() {
    tauri::async_runtime::spawn_blocking(|| {
        let Ok(store) = AppStore::open() else {
            return;
        };
        recover_transfer_quarantine_journals();
        cleanup_transfer_staging_dirs(&store, Duration::from_secs(10 * 60));
    });
}

#[cfg(windows)]
fn open_reserved_transfer_directory(path: &Path) -> io::Result<File> {
    use windows_sys::Win32::{
        Foundation::GENERIC_READ,
        Storage::FileSystem::{
            DELETE, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
            FILE_SHARE_WRITE,
        },
    };

    let file = fs::OpenOptions::new()
        .access_mode(GENERIC_READ | DELETE)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let metadata = fs::symlink_metadata(path)?;
    validate_transfer_metadata(path, &metadata).map_err(io::Error::other)?;
    if !metadata.is_dir() {
        return Err(io::Error::other("临时迁移项目不是目录"));
    }
    Ok(file)
}

#[cfg(not(windows))]
fn open_reserved_transfer_directory(path: &Path) -> io::Result<File> {
    File::open(path)
}

#[cfg(windows)]
fn open_reserved_transfer_file(path: &Path) -> io::Result<File> {
    use windows_sys::Win32::{
        Foundation::GENERIC_READ,
        Storage::FileSystem::{DELETE, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ},
    };

    fs::OpenOptions::new()
        .read(true)
        .access_mode(GENERIC_READ | DELETE)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

#[cfg(not(windows))]
fn open_reserved_transfer_file(path: &Path) -> io::Result<File> {
    File::open(path)
}

fn is_transfer_staging_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| {
            name.strip_prefix(".dustdesk-transfer-")
                .and_then(|name| name.strip_suffix(".tmp"))
        })
        .and_then(|name| name.rsplit_once('-'))
        .map(|(id, index)| {
            id.strip_prefix('s')
                .map(|hex| !hex.is_empty() && hex.chars().all(|ch| ch.is_ascii_hexdigit()))
                .unwrap_or(false)
                && !index.is_empty()
                && index.chars().all(|ch| ch.is_ascii_digit())
        })
        .unwrap_or(false)
}

fn is_source_transfer_staging_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| {
            name.strip_prefix(".dustdesk-source-transfer-")
                .and_then(|name| name.strip_suffix(".tmp"))
        })
        .and_then(|name| name.rsplit_once('-'))
        .map(|(id, index)| {
            id.strip_prefix('s')
                .map(|hex| !hex.is_empty() && hex.chars().all(|ch| ch.is_ascii_hexdigit()))
                .unwrap_or(false)
                && !index.is_empty()
                && index.chars().all(|ch| ch.is_ascii_digit())
        })
        .unwrap_or(false)
}

fn is_internal_transfer_path(path: &Path) -> bool {
    is_transfer_staging_path(path) || is_source_transfer_staging_path(path)
}

#[cfg(windows)]
fn rename_path_no_replace(source: &Path, destination: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(source).map_err(to_message)?;
    validate_transfer_metadata(source, &metadata)?;
    let source_handle = if metadata.is_dir() {
        open_reserved_transfer_directory(source).map_err(to_message)?
    } else if metadata.is_file() {
        open_reserved_transfer_file(source).map_err(to_message)?
    } else {
        return Err(format!("不支持移动此项目：{}", source.display()));
    };
    commit_staging_handle_no_replace(&source_handle, source, destination)
}

#[cfg(not(windows))]
fn rename_path_no_replace(source: &Path, destination: &Path) -> Result<(), String> {
    if destination.exists() {
        return Err(format!("目标项目已存在：{}", destination.display()));
    }
    fs::rename(source, destination).map_err(to_message)
}

fn validate_transfer_source_tree(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(to_message)?;
    validate_transfer_metadata(path, &metadata)?;
    if metadata.is_dir() {
        for entry in fs::read_dir(path).map_err(to_message)? {
            validate_transfer_source_tree(&entry.map_err(to_message)?.path())?;
        }
    } else if !metadata.is_file() {
        return Err(format!("不支持移动此项目：{}", path.display()));
    }
    Ok(())
}

fn validate_transfer_metadata(path: &Path, metadata: &fs::Metadata) -> Result<(), String> {
    if metadata.file_type().is_symlink() || metadata_is_reparse_point(metadata) {
        return Err(format!(
            "为避免误操作链接目标，不能移动符号链接或目录联接：{}",
            path.display()
        ));
    }
    validate_windows_extended_metadata(path, metadata)?;
    Ok(())
}

#[cfg(windows)]
fn validate_windows_extended_metadata(path: &Path, metadata: &fs::Metadata) -> Result<(), String> {
    use std::os::windows::fs::MetadataExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_EA;

    if metadata.file_attributes() & FILE_ATTRIBUTE_EA != 0 {
        return Err(format!(
            "检测到无法可靠校验的 Windows 扩展属性，已保留源项目：{}",
            path.display()
        ));
    }
    let directory_streams = if metadata.is_dir() {
        windows_named_streams(path)
            .map_err(|error| format!("无法检查目录的 Windows 命名流 {}：{error}", path.display()))?
    } else {
        Vec::new()
    };
    if !directory_streams.is_empty() {
        return Err(format!(
            "暂不迁移带 NTFS 命名流的目录，已保留源项目：{}",
            path.display()
        ));
    }
    Ok(())
}

#[cfg(not(windows))]
fn validate_windows_extended_metadata(
    _path: &Path,
    _metadata: &fs::Metadata,
) -> Result<(), String> {
    Ok(())
}

#[cfg(windows)]
fn metadata_is_reparse_point(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn metadata_is_reparse_point(_metadata: &fs::Metadata) -> bool {
    false
}

#[cfg(windows)]
fn transfer_tree_exceeds_lock_limit(source: &Path) -> Result<bool, String> {
    let mut pending = vec![source.to_path_buf()];
    let mut count = 0usize;
    let mut exceeds_limit = false;
    while let Some(path) = pending.pop() {
        let metadata = fs::symlink_metadata(&path).map_err(to_message)?;
        validate_transfer_metadata(&path, &metadata)?;
        count += 1;
        if count > MAX_LOCKED_TRANSFER_TREE_ENTRIES {
            exceeds_limit = true;
        }
        if metadata.is_dir() {
            for entry in fs::read_dir(&path).map_err(to_message)? {
                pending.push(entry.map_err(to_message)?.path());
            }
        } else if !metadata.is_file() {
            return Err(format!("不支持移动此项目：{}", path.display()));
        }
    }
    Ok(exceeds_limit)
}

#[cfg(windows)]
struct LockedSourceEntry {
    path: PathBuf,
    file: File,
}

#[cfg(windows)]
struct LockedSourceTree {
    entries: Vec<LockedSourceEntry>,
    fully_locked: bool,
}

#[cfg(windows)]
fn lock_transfer_source_tree(source: &Path) -> Result<LockedSourceTree, String> {
    let fully_locked = !transfer_tree_exceeds_lock_limit(source)?;
    let mut entries = Vec::new();
    if fully_locked {
        lock_transfer_source_entry(source, &mut entries)?;
    } else {
        let file = open_transfer_directory_guard(source).map_err(to_message)?;
        lock_transfer_source_entry_from_file(source, file, &mut entries, false)?;
    }
    Ok(LockedSourceTree {
        entries,
        fully_locked,
    })
}

#[cfg(windows)]
fn open_transfer_directory_guard(path: &Path) -> io::Result<File> {
    use windows_sys::Win32::{
        Foundation::GENERIC_READ,
        Storage::FileSystem::{
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
            FILE_SHARE_WRITE,
        },
    };

    let file = fs::OpenOptions::new()
        .access_mode(GENERIC_READ)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let metadata = fs::symlink_metadata(path)?;
    validate_transfer_metadata(path, &metadata).map_err(io::Error::other)?;
    if !metadata.is_dir() {
        return Err(io::Error::other("临时迁移项目不是目录"));
    }
    Ok(file)
}

#[cfg(windows)]
fn lock_transfer_source_entry(
    path: &Path,
    entries: &mut Vec<LockedSourceEntry>,
) -> Result<(), String> {
    let file = open_locked_transfer_source_entry(path)?;
    lock_transfer_source_entry_from_file(path, file, entries, true)
}

#[cfg(windows)]
fn open_locked_transfer_source_entry(path: &Path) -> Result<File, String> {
    use windows_sys::Win32::{
        Foundation::GENERIC_READ,
        Storage::FileSystem::{
            DELETE, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
        },
    };

    fs::OpenOptions::new()
        .access_mode(GENERIC_READ | DELETE)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .map_err(to_message)
}

#[cfg(windows)]
fn lock_transfer_source_entry_from_file(
    path: &Path,
    file: File,
    entries: &mut Vec<LockedSourceEntry>,
    recurse: bool,
) -> Result<(), String> {
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DIRECTORY,
        FILE_ATTRIBUTE_REPARSE_POINT,
    };

    let mut information = BY_HANDLE_FILE_INFORMATION::default();
    if unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, &mut information) } == 0 {
        return Err(io::Error::last_os_error().to_string());
    }
    if information.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(format!(
            "为避免误操作链接目标，不能移动符号链接或目录联接：{}",
            path.display()
        ));
    }

    let is_directory = information.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0;
    entries.push(LockedSourceEntry {
        path: path.to_path_buf(),
        file,
    });
    if recurse && is_directory {
        for entry in fs::read_dir(path).map_err(to_message)? {
            lock_transfer_source_entry(&entry.map_err(to_message)?.path(), entries)?;
        }
    }
    Ok(())
}

#[cfg(windows)]
fn lock_transfer_source_tree_from_root(
    source: &Path,
    root: File,
) -> Result<LockedSourceTree, String> {
    let fully_locked = !transfer_tree_exceeds_lock_limit(source)?;
    let mut entries = Vec::new();
    lock_transfer_source_entry_from_file(source, root, &mut entries, fully_locked)?;
    Ok(LockedSourceTree {
        entries,
        fully_locked,
    })
}

#[cfg(windows)]
fn commit_staging_handle_no_replace(
    root: &File,
    _staging: &Path,
    destination: &Path,
) -> Result<(), String> {
    use std::{mem::offset_of, ptr::null_mut};
    use windows_sys::Win32::Storage::FileSystem::{
        FileRenameInfoEx, SetFileInformationByHandle, FILE_RENAME_INFO,
    };
    let mut name = destination.as_os_str().encode_wide().collect::<Vec<_>>();
    let name_bytes = name
        .len()
        .checked_mul(std::mem::size_of::<u16>())
        .ok_or_else(|| "目标路径过长".to_owned())?;
    let buffer_bytes = offset_of!(FILE_RENAME_INFO, FileName)
        .checked_add(name_bytes)
        .and_then(|length| length.checked_add(std::mem::size_of::<u16>()))
        .ok_or_else(|| "目标路径过长".to_owned())?;
    name.push(0);
    let mut buffer = vec![0usize; buffer_bytes.div_ceil(std::mem::size_of::<usize>())];
    let information = buffer.as_mut_ptr().cast::<FILE_RENAME_INFO>();

    unsafe {
        (*information).Anonymous.Flags = 0;
        (*information).RootDirectory = null_mut();
        (*information).FileNameLength = name_bytes as u32;
        std::ptr::copy_nonoverlapping(
            name.as_ptr(),
            std::ptr::addr_of_mut!((*information).FileName).cast::<u16>(),
            name.len(),
        );
        if SetFileInformationByHandle(
            root.as_raw_handle() as _,
            FileRenameInfoEx,
            information.cast(),
            buffer_bytes as u32,
        ) == 0
        {
            return Err(io::Error::last_os_error().to_string());
        }
    }
    Ok(())
}

#[cfg(windows)]
fn delete_locked_source_tree(locked: LockedSourceTree) -> Result<(), String> {
    use std::mem::size_of;
    use windows_sys::Win32::Storage::FileSystem::{
        FileDispositionInfoEx, SetFileInformationByHandle, FILE_DISPOSITION_FLAG_DELETE,
        FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE, FILE_DISPOSITION_INFO_EX,
    };

    let LockedSourceTree {
        entries,
        fully_locked,
    } = locked;
    if !fully_locked {
        let source = entries
            .first()
            .map(|entry| entry.path.clone())
            .ok_or_else(|| "无法识别待删除的源项目".to_owned())?;
        drop(entries);
        return remove_existing_path(&source)
            .map_err(|error| format!("已安全复制但无法删除源项目 {}：{error}", source.display()));
    }

    // Delete exact handles child-first. Unverified late entries keep their parent non-empty.
    for entry in entries.into_iter().rev() {
        let disposition = FILE_DISPOSITION_INFO_EX {
            Flags: FILE_DISPOSITION_FLAG_DELETE | FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE,
        };
        if unsafe {
            SetFileInformationByHandle(
                entry.file.as_raw_handle() as _,
                FileDispositionInfoEx,
                (&disposition as *const FILE_DISPOSITION_INFO_EX).cast(),
                size_of::<FILE_DISPOSITION_INFO_EX>() as u32,
            )
        } == 0
        {
            return Err(format!(
                "已安全复制但无法删除源项目 {}：{}",
                entry.path.display(),
                io::Error::last_os_error()
            ));
        }
        drop(entry.file);
    }
    Ok(())
}

#[cfg(not(windows))]
struct LockedSourceTree {
    source: PathBuf,
}

#[cfg(not(windows))]
fn lock_transfer_source_tree(source: &Path) -> Result<LockedSourceTree, String> {
    validate_transfer_source_tree(source)?;
    Ok(LockedSourceTree {
        source: source.to_path_buf(),
    })
}

#[cfg(not(windows))]
fn lock_transfer_source_tree_from_root(
    source: &Path,
    _root: File,
) -> Result<LockedSourceTree, String> {
    lock_transfer_source_tree(source)
}

#[cfg(not(windows))]
fn commit_staging_handle_no_replace(
    _root: &File,
    staging: &Path,
    destination: &Path,
) -> Result<(), String> {
    rename_path_no_replace(staging, destination)
}

#[cfg(not(windows))]
fn delete_locked_source_tree(locked: LockedSourceTree) -> Result<(), String> {
    remove_existing_path(&locked.source)
}

fn verify_copied_path_shape(source: &Path, destination: &Path) -> Result<(), String> {
    let source_metadata = fs::symlink_metadata(source).map_err(to_message)?;
    let destination_metadata = fs::symlink_metadata(destination).map_err(to_message)?;
    validate_transfer_metadata(source, &source_metadata)?;
    validate_transfer_metadata(destination, &destination_metadata)?;

    if source_metadata.is_dir() != destination_metadata.is_dir()
        || source_metadata.is_file() != destination_metadata.is_file()
    {
        return Err(format!("迁移校验失败：项目类型不一致 {}", source.display()));
    }
    if source_metadata.is_file() {
        let modified_matches = match (source_metadata.modified(), destination_metadata.modified()) {
            (Ok(source_modified), Ok(destination_modified)) => {
                source_modified == destination_modified
            }
            _ => false,
        };
        return (source_metadata.len() == destination_metadata.len() && modified_matches)
            .then_some(())
            .ok_or_else(|| format!("迁移校验失败：文件大小或时间不一致 {}", source.display()));
    }

    let source_entries = fs::read_dir(source)
        .map_err(to_message)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(to_message)?;
    let destination_entry_count = fs::read_dir(destination)
        .map_err(to_message)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(to_message)?
        .len();
    if source_entries.len() != destination_entry_count {
        return Err(format!(
            "迁移校验失败：目录内容不完整 {}（源 {} 项，目标 {} 项）",
            source.display(),
            source_entries.len(),
            destination_entry_count
        ));
    }
    for entry in source_entries {
        verify_copied_path_shape(&entry.path(), &destination.join(entry.file_name()))?;
    }
    Ok(())
}

fn verify_copied_path(source: &Path, destination: &Path) -> Result<(), String> {
    let source_metadata = fs::symlink_metadata(source).map_err(to_message)?;
    let destination_metadata = fs::symlink_metadata(destination).map_err(to_message)?;
    validate_transfer_metadata(source, &source_metadata)?;
    validate_transfer_metadata(destination, &destination_metadata)?;

    if source_metadata.is_dir() != destination_metadata.is_dir()
        || source_metadata.is_file() != destination_metadata.is_file()
    {
        return Err(format!("迁移校验失败：项目类型不一致 {}", source.display()));
    }

    if source_metadata.is_dir() {
        let source_entries = fs::read_dir(source)
            .map_err(to_message)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(to_message)?;
        let destination_entry_count = fs::read_dir(destination)
            .map_err(to_message)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(to_message)?
            .len();
        if source_entries.len() != destination_entry_count {
            return Err(format!("迁移校验失败：目录内容不完整 {}", source.display()));
        }

        for entry in source_entries {
            verify_copied_path(&entry.path(), &destination.join(entry.file_name()))?;
        }
        return Ok(());
    }

    if source_metadata.len() != destination_metadata.len()
        || !files_have_same_contents(source, destination)?
        || !named_streams_have_same_contents(source, destination)?
    {
        return Err(format!("迁移校验失败：文件内容不一致 {}", source.display()));
    }
    Ok(())
}

#[cfg(windows)]
#[derive(Debug)]
struct WindowsNamedStream {
    name: String,
    size: i64,
}

#[cfg(windows)]
fn named_streams_have_same_contents(left: &Path, right: &Path) -> Result<bool, String> {
    let left_streams = windows_named_streams(left)?;
    let right_streams = windows_named_streams(right)?;
    if left_streams.len() != right_streams.len() {
        return Ok(false);
    }

    for (left_stream, right_stream) in left_streams.iter().zip(&right_streams) {
        if !left_stream.name.eq_ignore_ascii_case(&right_stream.name)
            || left_stream.size != right_stream.size
            || !files_have_same_contents(
                &path_with_windows_stream(left, &left_stream.name),
                &path_with_windows_stream(right, &right_stream.name),
            )?
        {
            return Ok(false);
        }
    }
    Ok(true)
}

#[cfg(not(windows))]
fn named_streams_have_same_contents(_left: &Path, _right: &Path) -> Result<bool, String> {
    Ok(true)
}

#[cfg(windows)]
fn windows_named_streams(path: &Path) -> Result<Vec<WindowsNamedStream>, String> {
    use windows_sys::Win32::{
        Foundation::{ERROR_HANDLE_EOF, ERROR_NO_MORE_FILES, INVALID_HANDLE_VALUE},
        Storage::FileSystem::{
            FindClose, FindFirstStreamW, FindNextStreamW, FindStreamInfoStandard,
            WIN32_FIND_STREAM_DATA,
        },
    };

    struct FindStreamHandle(windows_sys::Win32::Foundation::HANDLE);
    impl Drop for FindStreamHandle {
        fn drop(&mut self) {
            unsafe {
                FindClose(self.0);
            }
        }
    }

    let wide = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let mut data = WIN32_FIND_STREAM_DATA::default();
    let handle = unsafe {
        FindFirstStreamW(
            wide.as_ptr(),
            FindStreamInfoStandard,
            (&mut data as *mut WIN32_FIND_STREAM_DATA).cast(),
            0,
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(ERROR_HANDLE_EOF as i32) {
            return Ok(Vec::new());
        }
        return Err(format!(
            "无法枚举 NTFS 命名流，已保留源项目 {}：{}",
            path.display(),
            error
        ));
    }
    let _guard = FindStreamHandle(handle);
    let mut streams = Vec::new();
    loop {
        let length = data
            .cStreamName
            .iter()
            .position(|ch| *ch == 0)
            .unwrap_or(data.cStreamName.len());
        let name = String::from_utf16(&data.cStreamName[..length])
            .map_err(|_| format!("NTFS 命名流名称无效：{}", path.display()))?;
        if !name.eq_ignore_ascii_case("::$DATA") {
            streams.push(WindowsNamedStream {
                name,
                size: data.StreamSize,
            });
        }

        data = WIN32_FIND_STREAM_DATA::default();
        if unsafe { FindNextStreamW(handle, (&mut data as *mut WIN32_FIND_STREAM_DATA).cast()) }
            == 0
        {
            let error = io::Error::last_os_error();
            if matches!(
                error.raw_os_error(),
                Some(code) if code == ERROR_HANDLE_EOF as i32 || code == ERROR_NO_MORE_FILES as i32
            ) {
                break;
            }
            return Err(format!(
                "无法枚举 NTFS 命名流，已保留源项目 {}：{}",
                path.display(),
                error
            ));
        }
    }
    streams.sort_by_key(|stream| stream.name.to_ascii_lowercase());
    Ok(streams)
}

#[cfg(windows)]
fn path_with_windows_stream(path: &Path, stream: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(stream);
    PathBuf::from(value)
}

fn files_have_same_contents(left: &Path, right: &Path) -> Result<bool, String> {
    let mut left = BufReader::new(File::open(left).map_err(to_message)?);
    let mut right = BufReader::new(File::open(right).map_err(to_message)?);
    let mut left_buffer = [0u8; 64 * 1024];
    let mut right_buffer = [0u8; 64 * 1024];

    loop {
        let left_read = left.read(&mut left_buffer).map_err(to_message)?;
        let right_read = right.read(&mut right_buffer).map_err(to_message)?;
        if left_read != right_read || left_buffer[..left_read] != right_buffer[..right_read] {
            return Ok(false);
        }
        if left_read == 0 {
            return Ok(true);
        }
    }
}

fn remove_existing_path(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    if path.is_dir() {
        fs::remove_dir_all(path).map_err(to_message)
    } else {
        fs::remove_file(path).map_err(to_message)
    }
}

fn copy_directory_for_transfer(source: &Path, destination: &Path) -> Result<(), String> {
    #[cfg(windows)]
    if directory_entry_count_exceeds(source, MAX_LOCKED_TRANSFER_TREE_ENTRIES)?
        && fs::read_dir(destination)
            .map_err(to_message)?
            .next()
            .transpose()
            .map_err(to_message)?
            .is_none()
    {
        match try_copy_directory_with_robocopy(source, destination)? {
            true => return copy_directory_basic_metadata(source, destination),
            false => {}
        }
    }

    copy_dir_recursive(source, destination)
}

#[cfg(windows)]
fn directory_entry_count_exceeds(source: &Path, limit: usize) -> Result<bool, String> {
    let mut pending = vec![source.to_path_buf()];
    let mut count = 0usize;
    while let Some(path) = pending.pop() {
        for entry in fs::read_dir(&path).map_err(to_message)? {
            let path = entry.map_err(to_message)?.path();
            let metadata = fs::symlink_metadata(&path).map_err(to_message)?;
            validate_transfer_metadata(&path, &metadata)?;
            count += 1;
            if count > limit {
                return Ok(true);
            }
            if metadata.is_dir() {
                pending.push(path);
            } else if !metadata.is_file() {
                return Err(format!("不支持移动此项目：{}", path.display()));
            }
        }
    }
    Ok(false)
}

#[cfg(windows)]
fn try_copy_directory_with_robocopy(source: &Path, destination: &Path) -> Result<bool, String> {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let started = Instant::now();
    let status = Command::new("robocopy.exe")
        .arg(source)
        .arg(destination)
        .args([
            "/E",
            "/COPY:DAT",
            "/DCOPY:DAT",
            "/R:1",
            "/W:1",
            "/MT:8",
            "/XJ",
            "/NFL",
            "/NDL",
            "/NJH",
            "/NJS",
            "/NP",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .status();

    let status = match status {
        Ok(status) => status,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(format!("启动 Windows 多线程目录复制失败：{error}")),
    };
    eprintln!(
        "[desktop-transfer] robocopy elapsed_ms={} exit_code={:?} source={} destination={}",
        started.elapsed().as_millis(),
        status.code(),
        source.display(),
        destination.display()
    );
    match status.code() {
        Some(code) if (0..4).contains(&code) => Ok(true),
        Some(code) => Err(format!("Windows 多线程目录复制校验失败，退出码 {code}")),
        None => Err("Windows 多线程目录复制异常终止".to_owned()),
    }
}

fn copy_dir_recursive(source: &Path, destination: &Path) -> Result<(), String> {
    let source_metadata = fs::symlink_metadata(source).map_err(to_message)?;
    let destination_metadata = fs::symlink_metadata(destination).map_err(to_message)?;
    validate_transfer_metadata(source, &source_metadata)?;
    validate_transfer_metadata(destination, &destination_metadata)?;
    if !source_metadata.is_dir() || !destination_metadata.is_dir() {
        return Err(format!("迁移目录类型不正确：{}", source.display()));
    }
    for entry in fs::read_dir(source).map_err(to_message)? {
        let entry = entry.map_err(to_message)?;
        let from = entry.path();
        let to = destination.join(entry.file_name());
        let metadata = fs::symlink_metadata(&from).map_err(to_message)?;
        validate_transfer_metadata(&from, &metadata)?;
        if metadata.is_dir() {
            fs::create_dir(&to).map_err(to_message)?;
            copy_dir_recursive(&from, &to)?;
        } else if metadata.is_file() {
            copy_file_contents(&from, &to)?;
        } else {
            return Err(format!("不支持移动此项目：{}", from.display()));
        }
    }
    copy_directory_basic_metadata(source, destination)?;
    Ok(())
}

#[cfg(windows)]
fn copy_file_contents(source: &Path, destination: &Path) -> Result<(), String> {
    use std::mem::size_of;
    use windows_sys::Win32::Storage::FileSystem::{
        CopyFile2, COPYFILE2_EXTENDED_PARAMETERS, COPY_FILE_FAIL_IF_EXISTS,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        FILE_WRITE_ATTRIBUTES,
    };

    let metadata = fs::symlink_metadata(source).map_err(to_message)?;
    validate_transfer_metadata(source, &metadata)?;
    if !metadata.is_file() {
        return Err(format!("源项目不是文件：{}", source.display()));
    }

    let source_wide = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination_wide = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let mut parameters = COPYFILE2_EXTENDED_PARAMETERS::default();
    parameters.dwSize = size_of::<COPYFILE2_EXTENDED_PARAMETERS>() as u32;
    parameters.dwCopyFlags = COPY_FILE_FAIL_IF_EXISTS;
    let result = unsafe { CopyFile2(source_wide.as_ptr(), destination_wide.as_ptr(), &parameters) };
    if result < 0 {
        return Err(format!("CopyFile2 失败：HRESULT 0x{:08X}", result as u32));
    }

    let destination_file = fs::OpenOptions::new()
        .access_mode(FILE_WRITE_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(destination)
        .map_err(to_message)?;
    copy_file_basic_metadata(source, &destination_file)
}

#[cfg(not(windows))]
fn copy_file_contents(source: &Path, destination: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(source).map_err(to_message)?;
    validate_transfer_metadata(source, &metadata)?;
    if !metadata.is_file() {
        return Err(format!("源项目不是文件：{}", source.display()));
    }

    let mut source_file = File::open(source).map_err(to_message)?;
    let mut destination_file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(to_message)?;
    io::copy(&mut source_file, &mut destination_file).map_err(to_message)?;
    destination_file.sync_all().map_err(to_message)?;
    copy_file_basic_metadata(source, &destination_file)
}

#[cfg(windows)]
fn copy_file_basic_metadata(source: &Path, destination: &File) -> Result<(), String> {
    let source = File::open(source).map_err(to_message)?;
    copy_windows_basic_metadata(&source, destination)
}

#[cfg(not(windows))]
fn copy_file_basic_metadata(source: &Path, destination: &File) -> Result<(), String> {
    destination
        .set_permissions(fs::metadata(source).map_err(to_message)?.permissions())
        .map_err(to_message)
}

#[cfg(windows)]
fn copy_directory_basic_metadata(source: &Path, destination: &Path) -> Result<(), String> {
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_WRITE_ATTRIBUTES,
    };

    let source = fs::OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(source)
        .map_err(to_message)?;
    let destination = fs::OpenOptions::new()
        .access_mode(FILE_WRITE_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(destination)
        .map_err(to_message)?;
    copy_windows_basic_metadata(&source, &destination)
}

#[cfg(not(windows))]
fn copy_directory_basic_metadata(source: &Path, destination: &Path) -> Result<(), String> {
    fs::set_permissions(
        destination,
        fs::metadata(source).map_err(to_message)?.permissions(),
    )
    .map_err(to_message)
}

#[cfg(windows)]
fn copy_windows_basic_metadata(source: &File, destination: &File) -> Result<(), String> {
    use std::mem::size_of;
    use windows_sys::Win32::Storage::FileSystem::{
        FileBasicInfo, GetFileInformationByHandleEx, SetFileInformationByHandle,
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_BASIC_INFO,
    };

    let mut source_info = FILE_BASIC_INFO::default();
    if unsafe {
        GetFileInformationByHandleEx(
            source.as_raw_handle() as _,
            FileBasicInfo,
            (&mut source_info as *mut FILE_BASIC_INFO).cast(),
            size_of::<FILE_BASIC_INFO>() as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error().to_string());
    }
    if source_info.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err("不能复制符号链接或目录联接的元数据".to_owned());
    }
    if unsafe {
        SetFileInformationByHandle(
            destination.as_raw_handle() as _,
            FileBasicInfo,
            (&source_info as *const FILE_BASIC_INFO).cast(),
            size_of::<FILE_BASIC_INFO>() as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error().to_string());
    }
    Ok(())
}

fn add_matching_launchers(
    store: &AppStore,
    query_lower: &str,
    unique: &mut BTreeMap<String, SearchItem>,
) {
    let launchers = store.load_launchers();
    for launcher in launchers.items {
        let name = if launcher.name.trim().is_empty() {
            display_path_name(Path::new(&launcher.path))
        } else {
            launcher.name.clone()
        };

        if !matches_search_query(&name, &launcher.path, query_lower) {
            continue;
        }

        let key = format!("launcher:{}", launcher.path.to_lowercase());
        unique
            .entry(key)
            .or_insert_with(|| launcher_search_item(&launcher, name));
    }
}

fn add_matching_paths(
    paths: &[String],
    query_lower: &str,
    unique: &mut BTreeMap<String, SearchItem>,
) {
    let mut scanned = 0usize;

    for root in paths {
        if scanned >= SEARCH_SCAN_LIMIT || unique.len() >= SEARCH_RESULT_LIMIT * 2 {
            break;
        }

        let root_path = PathBuf::from(root);
        if !root_path.exists() {
            continue;
        }

        let mut queue = VecDeque::from([root_path]);
        while let Some(path) = queue.pop_front() {
            if scanned >= SEARCH_SCAN_LIMIT || unique.len() >= SEARCH_RESULT_LIMIT * 2 {
                break;
            }

            let Ok(entries) = std::fs::read_dir(&path) else {
                continue;
            };

            for entry in entries.flatten() {
                if scanned >= SEARCH_SCAN_LIMIT || unique.len() >= SEARCH_RESULT_LIMIT * 2 {
                    break;
                }

                let entry_path = entry.path();
                scanned += 1;

                let Ok(file_type) = entry.file_type() else {
                    continue;
                };

                let name = display_path_name(&entry_path);
                let is_dir = file_type.is_dir();
                if is_dir && is_noise_search_dir(&name) {
                    continue;
                }

                let path_text = entry_path.display().to_string();
                if matches_search_query(&name, &path_text, query_lower) {
                    let kind = if is_dir {
                        SearchItemKind::Directory
                    } else {
                        SearchItemKind::File
                    };
                    let key = format!("path:{}", path_text.to_lowercase());
                    unique
                        .entry(key)
                        .or_insert_with(|| path_search_item(&entry_path, kind));
                }

                if is_dir {
                    queue.push_back(entry_path);
                }
            }
        }
    }
}

fn search_history_items(history: &SearchHistoryData) -> Vec<SearchItem> {
    history
        .items
        .iter()
        .filter(|item| !item.path.trim().is_empty())
        .map(search_item_from_history)
        .collect()
}

fn record_search_open(item: &SearchItem) -> Result<(), String> {
    store::with_storage_mutation(|| {
        let store = AppStore::open().map_err(to_message)?;
        let mut history = store.load_search_history_strict().map_err(to_message)?;
        let now = now_local_string();
        upsert_search_history_open(&mut history, item, now, now_id());
        store.save_search_history(&history).map_err(to_message)
    })
}

fn upsert_search_history_open(
    history: &mut SearchHistoryData,
    item: &SearchItem,
    now: String,
    new_id: String,
) {
    let key = item.path.to_lowercase();
    if let Some(record) = history
        .items
        .iter_mut()
        .find(|record| record.path.to_lowercase() == key)
    {
        record.name = item.name.clone();
        record.kind = item.kind;
        record.extension = item.extension.clone();
        record.open_count = record.open_count.saturating_add(1);
        record.last_opened_at = now;
    } else {
        history.items.push(SearchHistoryItem {
            id: new_id,
            name: item.name.clone(),
            path: item.path.clone(),
            kind: item.kind,
            extension: item.extension.clone(),
            open_count: 1,
            last_opened_at: now,
        });
    }

    history
        .items
        .sort_by(|left, right| right.last_opened_at.cmp(&left.last_opened_at));
    history.items.truncate(SEARCH_HISTORY_LIMIT);
}

fn launcher_search_item(launcher: &LaunchItem, name: String) -> SearchItem {
    let path = launcher.path.trim().to_owned();
    let icon_data_url = launcher
        .icon_data_url
        .clone()
        .or_else(|| cached_icon_data_url(Path::new(&path)));
    SearchItem {
        id: format!("launcher:{}", path.to_lowercase()),
        name,
        path: path.clone(),
        kind: SearchItemKind::Launcher,
        extension: path_extension(Path::new(&path)),
        is_dir: false,
        source: "快捷启动".to_owned(),
        icon_data_url,
        open_count: 0,
        last_opened_at: String::new(),
    }
}

fn path_search_item(path: &Path, kind: SearchItemKind) -> SearchItem {
    let is_dir = kind == SearchItemKind::Directory;
    let path_text = path.display().to_string();
    SearchItem {
        id: format!("path:{}", path_text.to_lowercase()),
        name: display_path_name(path),
        path: path_text,
        kind,
        extension: if is_dir {
            "DIR".to_owned()
        } else {
            path_extension(path)
        },
        is_dir,
        source: path
            .parent()
            .map(|parent| parent.display().to_string())
            .unwrap_or_else(|| "本地路径".to_owned()),
        icon_data_url: cached_icon_data_url(path),
        open_count: 0,
        last_opened_at: String::new(),
    }
}

fn search_item_from_history(item: &SearchHistoryItem) -> SearchItem {
    SearchItem {
        id: item.id.clone(),
        name: item.name.clone(),
        path: item.path.clone(),
        kind: item.kind,
        extension: if item.extension.trim().is_empty() {
            path_extension(Path::new(&item.path))
        } else {
            item.extension.clone()
        },
        is_dir: item.kind == SearchItemKind::Directory,
        source: match item.kind {
            SearchItemKind::Launcher => "快捷启动".to_owned(),
            SearchItemKind::Directory => "目录".to_owned(),
            SearchItemKind::File => "最近文件".to_owned(),
        },
        icon_data_url: cached_icon_data_url(Path::new(&item.path)),
        open_count: item.open_count,
        last_opened_at: item.last_opened_at.clone(),
    }
}

fn matches_search_query(name: &str, path: &str, query_lower: &str) -> bool {
    name.to_lowercase().contains(query_lower) || path.to_lowercase().contains(query_lower)
}

fn rank_search_item(item: &SearchItem, query_lower: &str) -> u8 {
    let name = item.name.to_lowercase();
    if name == query_lower {
        return 0;
    }
    if name.starts_with(query_lower) {
        return 1;
    }
    if item.kind == SearchItemKind::Launcher {
        return 2;
    }
    if item.kind == SearchItemKind::Directory {
        return 3;
    }
    4
}

fn search_type_rank(item: &SearchItem) -> u8 {
    if item.kind == SearchItemKind::Launcher {
        return 0;
    }
    if is_shortcut_or_app(item) {
        return 1;
    }
    if item.kind == SearchItemKind::Directory || item.is_dir {
        return 2;
    }
    3
}

fn is_shortcut_or_app(item: &SearchItem) -> bool {
    matches!(
        item.extension.trim().to_ascii_lowercase().as_str(),
        "lnk" | "exe" | "appref-ms" | "url" | "bat" | "cmd" | "ps1" | "msi"
    )
}

fn is_noise_search_dir(name: &str) -> bool {
    matches!(
        name,
        "." | ".."
            | ".git"
            | ".hg"
            | ".svn"
            | ".cache"
            | ".next"
            | ".nuxt"
            | ".pytest_cache"
            | ".ruff_cache"
            | ".venv"
            | "__pycache__"
            | "build"
            | "dist"
            | "node_modules"
            | "target"
            | "venv"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::DesktopLayout;
    use std::cell::{Cell, RefCell};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn desktop_test_work_area(
        name: &str,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        scale_factor: f64,
    ) -> DesktopMonitorWorkArea {
        DesktopMonitorWorkArea {
            name: Some(name.to_owned()),
            x,
            y,
            width,
            height,
            scale_factor,
        }
    }

    fn desktop_test_layout(x: i32, y: i32, width: u32, height: u32) -> DesktopWindowLayout {
        DesktopWindowLayout {
            x,
            y,
            width,
            height,
            ..DesktopWindowLayout::default()
        }
    }

    #[test]
    fn category_name_sorting_is_independent_per_box() {
        let categories = vec![
            DeskCategory {
                name: "sorted".to_owned(),
                sort_by_name: true,
                item_paths: vec![
                    r"C:\Desktop\zeta.txt".to_owned(),
                    r"C:\Desktop\Alpha.txt".to_owned(),
                ],
                ..DeskCategory::default()
            },
            DeskCategory {
                name: "original".to_owned(),
                item_paths: vec![
                    r"C:\Desktop\zeta.txt".to_owned(),
                    r"C:\Desktop\Alpha.txt".to_owned(),
                ],
                ..DeskCategory::default()
            },
        ];

        let categories = categories_with_item_details(categories, false);
        assert_eq!(
            categories[0]
                .item_details
                .iter()
                .map(|item| item.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Alpha", "zeta"]
        );
        assert_eq!(
            categories[1]
                .item_details
                .iter()
                .map(|item| item.name.as_str())
                .collect::<Vec<_>>(),
            vec!["zeta", "Alpha"]
        );
    }

    #[test]
    fn clipboard_history_limit_defaults_and_truncates() {
        let settings: AppSettings =
            serde_json::from_str("{}").expect("legacy settings should load");
        assert_eq!(settings.clipboard_history_limit_value(), 30);

        let mut clipboard = models::ClipboardData {
            items: (0..5)
                .map(|index| ClipboardHistoryItem {
                    id: index.to_string(),
                    kind: models::ClipboardHistoryKind::Text,
                    text: index.to_string(),
                    image_png_base64: String::new(),
                    image_path: String::new(),
                    image_thumb_path: String::new(),
                    image_hash: String::new(),
                    created_at: String::new(),
                    is_locked: false,
                    is_pinned: false,
                })
                .collect(),
        };
        assert!(clipboard_bridge::truncate_history(&mut clipboard, 3));
        assert_eq!(clipboard.items.len(), 3);
        assert_eq!(clipboard.items[2].text, "2");
        assert!(!clipboard_bridge::truncate_history(&mut clipboard, 3));
    }

    #[test]
    fn desktop_layout_preserves_negative_coordinates_on_left_monitor() {
        let work_areas = vec![
            desktop_test_work_area("DISPLAY1", 0, 0, 1920, 1040, 1.0),
            desktop_test_work_area("DISPLAY2", -2560, 0, 2560, 1400, 1.25),
        ];
        let layout = DesktopWindowLayout {
            monitor_name: Some("DISPLAY2".to_owned()),
            monitor_offset_x: Some(160),
            monitor_offset_y: Some(100),
            monitor_scale_factor: Some(1.25),
            ..desktop_test_layout(-2400, 100, 600, 400)
        };

        let placement = resolve_desktop_window_placement(&layout, &work_areas, Some(0), 240, 160)
            .expect("应恢复到左侧显示器");

        assert_eq!(placement.x, -2400);
        assert_eq!(placement.y, 100);
        assert_eq!(placement.width, 600);
        assert_eq!(placement.height, 400);
    }

    #[test]
    fn desktop_layout_tracks_monitor_when_display_order_changes() {
        let work_areas = vec![
            desktop_test_work_area("DISPLAY2", -2560, 0, 2560, 1400, 1.0),
            desktop_test_work_area("DISPLAY1", 0, 0, 1920, 1040, 1.0),
        ];
        let layout = DesktopWindowLayout {
            monitor_name: Some("DISPLAY2".to_owned()),
            monitor_offset_x: Some(120),
            monitor_offset_y: Some(80),
            monitor_scale_factor: Some(1.0),
            ..desktop_test_layout(2040, 80, 600, 400)
        };

        let placement = resolve_desktop_window_placement(&layout, &work_areas, Some(1), 240, 160)
            .expect("应按显示器相对坐标恢复");

        assert_eq!(placement.x, -2440);
        assert_eq!(placement.y, 80);
    }

    #[test]
    fn desktop_icon_lookup_requires_an_exact_visible_name() {
        let item_texts = vec![
            (3, "tabbit2api".to_owned()),
            (7, "Tabbit".to_owned()),
            (9, "QQ飞车".to_owned()),
            (12, "QQ".to_owned()),
        ];

        assert_eq!(
            desktop_listview_find_item_in_texts(Path::new("Tabbit.lnk"), &item_texts),
            Some(7)
        );
        assert_eq!(
            desktop_listview_find_item_in_texts(Path::new("QQ.lnk"), &item_texts),
            Some(12)
        );
        assert_eq!(
            desktop_listview_find_item_in_texts(Path::new("QQ.lnk"), &[(9, "QQ飞车".to_owned())],),
            None
        );
    }

    #[test]
    fn desktop_icon_position_tracks_original_secondary_monitor() {
        let original_monitors = vec![
            desktop_test_work_area("DISPLAY1", 0, 0, 1707, 1019, 1.5),
            desktop_test_work_area("DISPLAY5", -1920, 184, 1920, 1032, 1.0),
        ];
        let position = desktop_icon_position_from_screen_point(-1500, 300, &original_monitors);

        assert_eq!(position.monitor_name.as_deref(), Some("DISPLAY5"));
        assert_eq!(position.monitor_offset_x, Some(420));
        assert_eq!(position.monitor_offset_y, Some(116));

        let moved_monitors = vec![
            desktop_test_work_area("DISPLAY1", 0, 0, 1707, 1019, 1.5),
            desktop_test_work_area("DISPLAY5", 1707, 0, 1920, 1032, 1.0),
        ];
        assert_eq!(
            resolve_desktop_icon_position(position, &moved_monitors),
            (2127, 116)
        );
    }

    #[test]
    fn desktop_icon_position_falls_back_to_visible_monitor() {
        let position = DesktopIconPosition {
            screen_x: -1500,
            screen_y: 300,
            monitor_name: Some("DISPLAY5".to_owned()),
            monitor_offset_x: Some(420),
            monitor_offset_y: Some(116),
        };
        let available_monitors = vec![desktop_test_work_area("DISPLAY1", 0, 0, 1707, 1019, 1.5)];

        assert_eq!(
            resolve_desktop_icon_position(position, &available_monitors),
            (0, 300)
        );
    }

    #[test]
    fn desktop_layout_falls_back_when_saved_monitor_is_disconnected() {
        let work_areas = vec![desktop_test_work_area("DISPLAY1", 0, 0, 1920, 1040, 1.0)];
        let layout = DesktopWindowLayout {
            monitor_name: Some("DISPLAY2".to_owned()),
            monitor_offset_x: Some(150),
            monitor_offset_y: Some(120),
            monitor_scale_factor: Some(1.5),
            ..desktop_test_layout(2070, 120, 720, 480)
        };

        let placement = resolve_desktop_window_placement(&layout, &work_areas, Some(0), 240, 160)
            .expect("应回退到主显示器");

        assert_eq!(placement.x, 100);
        assert_eq!(placement.y, 80);
        assert_eq!(placement.width, 480);
        assert_eq!(placement.height, 320);
    }

    #[test]
    fn desktop_layout_scales_for_target_monitor_dpi() {
        let work_areas = vec![desktop_test_work_area("DISPLAY2", 1920, 0, 2560, 1400, 1.5)];
        let layout = DesktopWindowLayout {
            monitor_name: Some("DISPLAY2".to_owned()),
            monitor_offset_x: Some(100),
            monitor_offset_y: Some(80),
            monitor_scale_factor: Some(1.0),
            ..desktop_test_layout(2020, 80, 360, 260)
        };

        let placement = resolve_desktop_window_placement(&layout, &work_areas, Some(0), 240, 160)
            .expect("应按目标显示器 DPI 缩放");

        assert_eq!(placement.x, 2070);
        assert_eq!(placement.y, 120);
        assert_eq!(placement.width, 540);
        assert_eq!(placement.height, 390);
    }

    #[test]
    fn inherited_desktop_layout_fills_a_wider_monitor_proportionally() {
        let source = desktop_test_work_area("DISPLAY1", 0, 0, 1700, 1000, 1.5);
        let target = desktop_test_work_area("DISPLAY5", 2560, 0, 1920, 1200, 1.0);
        let layout = DesktopWindowLayout {
            monitor_offset_x: Some(850),
            monitor_offset_y: Some(250),
            ..desktop_test_layout(850, 250, 680, 500)
        };

        let placement =
            map_desktop_window_layout_between_work_areas(&layout, &source, &target, 240, 160);

        assert_eq!(placement.x, 3520);
        assert_eq!(placement.y, 300);
        assert_eq!(placement.width, 768);
        assert_eq!(placement.height, 600);
    }

    #[test]
    fn legacy_desktop_layout_is_kept_on_secondary_monitor_and_clamped_if_missing() {
        let dual_work_areas = vec![
            desktop_test_work_area("DISPLAY1", 0, 0, 1920, 1040, 1.0),
            desktop_test_work_area("DISPLAY2", -1920, 0, 1920, 1040, 1.0),
        ];
        let secondary_layout = desktop_test_layout(-1800, 100, 600, 400);
        let placement = resolve_desktop_window_placement(
            &secondary_layout,
            &dual_work_areas,
            Some(0),
            240,
            160,
        )
        .expect("旧布局应保留在副屏");
        assert_eq!(placement.x, -1800);

        let missing_layout = desktop_test_layout(5000, 5000, 600, 400);
        let placement = resolve_desktop_window_placement(
            &missing_layout,
            &dual_work_areas[..1],
            Some(0),
            240,
            160,
        )
        .expect("失效旧布局应回到可见区域");
        assert_eq!(placement.x, 1320);
        assert_eq!(placement.y, 640);
    }

    #[test]
    fn desktop_monitor_id_is_stable_when_named_monitor_moves() {
        let original = desktop_test_work_area(r"\\.\DISPLAY5", 2560, 0, 1920, 1032, 1.0);
        let moved = desktop_test_work_area(r"\\.\DISPLAY5", -1920, 120, 1920, 1032, 1.25);
        let other = desktop_test_work_area(r"\\.\DISPLAY1", 0, 0, 1707, 1019, 1.5);

        assert_eq!(
            desktop_monitor_stable_id(&original),
            desktop_monitor_stable_id(&moved)
        );
        assert_ne!(
            desktop_monitor_stable_id(&original),
            desktop_monitor_stable_id(&other)
        );
    }

    #[test]
    fn multi_monitor_desktop_labels_are_recognized_separately() {
        assert!(is_desktop_widget_window_label(
            "desktop-widget-monitor-a1b2"
        ));
        assert!(is_desktop_launcher_window_label(
            "desktop-launcher-monitor-a1b2"
        ));
        assert_eq!(
            desktop_category_index_from_label("desktop-category-3-monitor-a1b2"),
            Some(3)
        );
        assert_eq!(
            secondary_monitor_id_from_desktop_label("desktop-category-3-monitor-a1b2"),
            Some("a1b2")
        );
        assert_eq!(
            primary_desktop_window_label_for("desktop-widget-monitor-a1b2").as_deref(),
            Some("desktop-widget")
        );
        assert_eq!(
            primary_desktop_window_label_for("desktop-category-3-monitor-a1b2").as_deref(),
            Some("desktop-category-3")
        );
        assert_eq!(
            primary_desktop_window_label_for("desktop-launcher-monitor-a1b2").as_deref(),
            Some("desktop-launcher")
        );
        assert!(!is_legacy_desktop_category_window_label(
            "desktop-category-3-monitor-a1b2"
        ));
        assert!(is_legacy_desktop_category_window_label(
            "desktop-category-3-legacy"
        ));
    }

    #[test]
    fn release_version_parsing_accepts_standard_tags() {
        assert_eq!(release_version_from_tag("v0.1.2"), Some("0.1.2".to_owned()));
        assert_eq!(
            release_version_from_tag("v0.1.2-15"),
            Some("0.1.2".to_owned())
        );
        assert_eq!(release_version_from_tag("build"), None);
    }

    #[test]
    fn semver_like_compare_orders_patch_versions() {
        assert_eq!(compare_semver_like("0.1.2", "0.1.1"), CmpOrdering::Greater);
        assert_eq!(compare_semver_like("0.1.1", "0.1.1"), CmpOrdering::Equal);
        assert_eq!(compare_semver_like("0.1.0", "0.1.1"), CmpOrdering::Less);
    }

    #[test]
    fn dustdesk_shortcut_detection_does_not_match_unrelated_shortcuts() {
        assert!(is_dustdesk_shortcut(
            Path::new(r"C:\Desktop\DustDesk.lnk"),
            None
        ));
        assert!(is_dustdesk_shortcut(
            Path::new(r"C:\Desktop\任意名称.lnk"),
            Some(Path::new(r"D:\OldDustDesk\dustdesk-tauri.exe"))
        ));
        assert!(!is_dustdesk_shortcut(
            Path::new(r"C:\Desktop\普通工具.lnk"),
            Some(Path::new(r"D:\Tools\ordinary.exe"))
        ));
        assert!(!is_dustdesk_shortcut(
            Path::new(r"C:\Desktop\DustDesk.url"),
            Some(Path::new(r"D:\OldDustDesk\dustdesk-tauri.exe"))
        ));
        assert!(dustdesk_shortcut_target_needs_repair(
            Some(Path::new(r"D:\OldDustDesk\dustdesk-tauri.exe")),
            Path::new(r"D:\CurrentDustDesk\dustdesk-tauri.exe")
        ));
        let existing_target = env::current_exe().expect("测试进程应有可执行文件路径");
        assert!(!dustdesk_shortcut_target_needs_repair(
            Some(&existing_target),
            Path::new(r"D:\CurrentDustDesk\dustdesk-tauri.exe")
        ));
    }

    #[test]
    fn archive_source_protection_covers_running_binary_and_ancestor_directories() {
        let current_exe = Path::new(r"D:\Apps\DustDesk\dustdesk-tauri.exe");
        assert!(is_protected_app_runtime_source(current_exe, current_exe));
        assert!(is_protected_app_runtime_source(
            Path::new(r"D:\Apps\DustDesk"),
            current_exe
        ));
        assert!(is_protected_app_runtime_source(
            Path::new(r"D:\Apps"),
            current_exe
        ));
        assert!(!is_protected_app_runtime_source(
            Path::new(r"D:\Apps\DustDesk\readme.txt"),
            current_exe
        ));
        assert!(!is_protected_app_runtime_source(
            Path::new(r"C:\Desktop\DustDesk.lnk"),
            current_exe
        ));
    }

    #[cfg(windows)]
    #[test]
    fn stale_dustdesk_shortcut_is_retargeted_without_moving_executables() {
        let root = unique_test_dir("dustdesk-shortcut-repair");
        let old_target = root.join("old").join("dustdesk-tauri.exe");
        let current_target = root.join("current").join("dustdesk-tauri.exe");
        let shortcut = root.join("DustDesk.lnk");
        fs::create_dir_all(old_target.parent().expect("旧目标应有父目录")).expect("创建旧目标目录");
        fs::create_dir_all(current_target.parent().expect("新目标应有父目录"))
            .expect("创建新目标目录");
        fs::write(&old_target, b"old").expect("创建旧目标");
        fs::write(&current_target, b"current").expect("创建新目标");
        create_windows_shortcut(&shortcut, &old_target, "DustDesk").expect("创建测试快捷方式");

        assert!(repair_dustdesk_shortcut_target(&shortcut, &current_target).expect("修复快捷方式"));
        let repaired_target =
            system_icon::shortcut_target_path(&shortcut).expect("读取修复后的目标");
        assert!(same_path_for_move(&repaired_target, &current_target));
        assert!(old_target.exists());
        assert!(current_target.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn category_reorder_remaps_split_indices_and_window_layouts() {
        let category = |name: &str| DeskCategory {
            name: name.to_owned(),
            ..DeskCategory::default()
        };
        let layout = |x| DesktopWindowLayout {
            x,
            y: 0,
            width: 320,
            height: 240,
            ..DesktopWindowLayout::default()
        };
        let mut config = AppConfig {
            desktop_categories: vec![category("A"), category("B"), category("C"), category("D")],
            desktop_icon_positions: BTreeMap::new(),
            settings: AppSettings::default(),
            desktop_layout: DesktopLayout {
                split_category_indices: vec![0, 2],
                locked: false,
                click_through: false,
                windows: BTreeMap::from([
                    ("desktop-category-0".to_owned(), layout(10)),
                    ("desktop-category-0-monitor-a1b2".to_owned(), layout(11)),
                    ("desktop-category-2".to_owned(), layout(30)),
                    ("desktop-category-2-monitor-a1b2".to_owned(), layout(31)),
                    ("desktop-widget".to_owned(), layout(90)),
                ]),
            },
        };

        reorder_category_config(&mut config, 0, 2).expect("分类重排应成功");

        assert_eq!(
            config
                .desktop_categories
                .iter()
                .map(|category| category.name.as_str())
                .collect::<Vec<_>>(),
            vec!["B", "C", "A", "D"]
        );
        assert_eq!(config.desktop_layout.split_category_indices, vec![1, 2]);
        assert_eq!(config.desktop_layout.windows["desktop-category-1"].x, 30);
        assert_eq!(config.desktop_layout.windows["desktop-category-2"].x, 10);
        assert_eq!(
            config.desktop_layout.windows["desktop-category-1-monitor-a1b2"].x,
            31
        );
        assert_eq!(
            config.desktop_layout.windows["desktop-category-2-monitor-a1b2"].x,
            11
        );
        assert_eq!(config.desktop_layout.windows["desktop-widget"].x, 90);
    }

    #[test]
    fn category_index_remapping_handles_both_move_directions() {
        assert_eq!(
            (0..4)
                .map(|index| remap_category_index(index, 0, 2))
                .collect::<Vec<_>>(),
            vec![2, 0, 1, 3]
        );
        assert_eq!(
            (0..4)
                .map(|index| remap_category_index(index, 3, 1))
                .collect::<Vec<_>>(),
            vec![0, 2, 3, 1]
        );
    }

    #[test]
    fn icon_cache_retries_after_a_transient_resolution_failure() {
        let root = unique_test_dir("dustdesk-icon-cache-retry");
        let path = root.join("item.bin");
        fs::create_dir_all(&root).expect("create icon test root");
        fs::write(&path, b"item").expect("create icon test item");
        let attempts = Cell::new(0usize);
        let resolve = |_: &Path| {
            let attempt = attempts.get() + 1;
            attempts.set(attempt);
            (attempt > 1).then(|| "data:image/png;base64,test".to_owned())
        };

        assert_eq!(
            cached_icon_data_url_with_cooldown(&path, Duration::ZERO, &resolve),
            None
        );
        assert_eq!(
            cached_icon_data_url_with_cooldown(&path, Duration::ZERO, &resolve).as_deref(),
            Some("data:image/png;base64,test")
        );
        assert_eq!(
            cached_icon_data_url_with_cooldown(&path, Duration::ZERO, &resolve).as_deref(),
            Some("data:image/png;base64,test")
        );
        assert_eq!(
            attempts.get(),
            2,
            "successes should remain positively cached"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn icon_cache_coalesces_concurrent_failures_for_the_same_path() {
        let root = unique_test_dir("dustdesk-icon-cache-failure-coalescing");
        let path = root.join("item.bin");
        fs::create_dir_all(&root).expect("create icon failure root");
        fs::write(&path, b"item").expect("create icon failure item");
        let path_key = normalize_path_for_compare(&path);
        let attempts = Arc::new(AtomicU64::new(0));
        let start = Arc::new(std::sync::Barrier::new(3));
        let threads = [path.clone(), path.clone()].map(|path| {
            let attempts = Arc::clone(&attempts);
            let start = Arc::clone(&start);
            std::thread::spawn(move || {
                start.wait();
                cached_icon_data_url_with(&path, move |_| {
                    attempts.fetch_add(1, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(100));
                    None
                })
            })
        });
        start.wait();

        for thread in threads {
            assert_eq!(thread.join().expect("join failed icon resolution"), None);
        }
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            1,
            "same-path waiters should share the short failure cooldown"
        );

        let cleanup_path = root.join("cleanup.bin");
        fs::write(&cleanup_path, b"cleanup").expect("create lock cleanup item");
        assert!(cached_icon_data_url_with(&cleanup_path, |_| {
            Some("data:image/png;base64,cleanup".to_owned())
        })
        .is_some());
        let locks = ICON_RESOLUTION_LOCKS
            .get_or_init(|| Mutex::new(BTreeMap::new()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert!(
            !locks.contains_key(&path_key),
            "the next miss should prune an unreferenced path lock"
        );
        drop(locks);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn icon_cache_resolves_unrelated_paths_concurrently() {
        let root = unique_test_dir("dustdesk-icon-cache-concurrency");
        fs::create_dir_all(&root).expect("create icon concurrency root");
        let paths = [root.join("first.bin"), root.join("second.bin")];
        for path in &paths {
            fs::write(path, b"item").expect("create icon concurrency item");
        }

        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let release_rx = Arc::new(Mutex::new(release_rx));
        let threads = paths.map(|path| {
            let started_tx = started_tx.clone();
            let release_rx = Arc::clone(&release_rx);
            std::thread::spawn(move || {
                cached_icon_data_url_with(&path, move |_| {
                    started_tx.send(()).expect("signal icon resolution start");
                    release_rx
                        .lock()
                        .expect("lock icon resolution release")
                        .recv_timeout(Duration::from_secs(5))
                        .expect("release icon resolution");
                    Some("data:image/png;base64,test".to_owned())
                })
            })
        });

        started_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("first icon resolution should start");
        let unrelated_resolution_started = started_rx.recv_timeout(Duration::from_secs(2)).is_ok();
        release_tx.send(()).expect("release first icon resolution");
        release_tx.send(()).expect("release second icon resolution");
        for thread in threads {
            assert!(thread.join().expect("join icon resolution").is_some());
        }
        assert!(
            unrelated_resolution_started,
            "different icon paths must not share one global resolution lock"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn recursive_search_finds_nested_shortcut_before_noisy_dirs() {
        let root = unique_test_dir("dustdesk-search");
        let shortcut_dir = root.join("apps").join("nested");
        let noisy_dir = root
            .join("projects")
            .join("demo")
            .join(".venv")
            .join("Scripts");
        fs::create_dir_all(&shortcut_dir).expect("create shortcut dir");
        fs::create_dir_all(&noisy_dir).expect("create noisy dir");
        fs::write(shortcut_dir.join("Cursor.lnk"), b"shortcut").expect("write shortcut");
        fs::write(noisy_dir.join("cursor.py"), b"print('cursor')").expect("write noisy file");

        let mut unique = BTreeMap::new();
        add_matching_paths(&[root.display().to_string()], "cursor", &mut unique);

        let items = unique.into_values().collect::<Vec<_>>();
        assert!(
            items
                .iter()
                .any(|item| item.name == "Cursor" && item.extension.eq_ignore_ascii_case("lnk")),
            "nested shortcut should be searchable"
        );
        assert!(
            items.iter().all(|item| !item.path.contains(".venv")),
            "virtual environment files should not pollute search results"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn search_history_items_include_opened_directories() {
        let history = SearchHistoryData {
            items: vec![SearchHistoryItem {
                id: "dir-1".to_owned(),
                name: "Projects".to_owned(),
                path: r"C:\Users\15407\Projects".to_owned(),
                kind: SearchItemKind::Directory,
                extension: String::new(),
                open_count: 2,
                last_opened_at: "2026-07-12T18:00:00.000".to_owned(),
            }],
        };

        let items = search_history_items(&history);

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].kind, SearchItemKind::Directory);
        assert!(items[0].is_dir);
        assert_eq!(items[0].open_count, 2);
    }

    #[test]
    fn search_history_upsert_counts_directory_opens() {
        let mut history = SearchHistoryData::default();
        let item = SearchItem {
            id: "path:c:\\users\\15407\\projects".to_owned(),
            name: "Projects".to_owned(),
            path: r"C:\Users\15407\Projects".to_owned(),
            kind: SearchItemKind::Directory,
            extension: String::new(),
            is_dir: true,
            source: "目录".to_owned(),
            icon_data_url: None,
            open_count: 0,
            last_opened_at: String::new(),
        };

        upsert_search_history_open(
            &mut history,
            &item,
            "2026-07-12T18:00:00.000".to_owned(),
            "dir-1".to_owned(),
        );
        upsert_search_history_open(
            &mut history,
            &item,
            "2026-07-12T18:05:00.000".to_owned(),
            "dir-2".to_owned(),
        );

        assert_eq!(history.items.len(), 1);
        assert_eq!(history.items[0].id, "dir-1");
        assert_eq!(history.items[0].kind, SearchItemKind::Directory);
        assert_eq!(history.items[0].open_count, 2);
        assert_eq!(history.items[0].last_opened_at, "2026-07-12T18:05:00.000");
    }

    #[test]
    fn archive_collision_uses_unique_destination_without_deleting_either_item() {
        let root = unique_test_dir("dustdesk-archive-collision");
        let incoming_dir = root.join("incoming");
        let category_dir = root.join("category");
        fs::create_dir_all(&incoming_dir).expect("create incoming dir");
        fs::create_dir_all(&category_dir).expect("create category dir");
        let source = incoming_dir.join("report.txt");
        let existing = category_dir.join("report.txt");
        fs::write(&source, b"new report").expect("write source");
        fs::write(&existing, b"existing report").expect("write existing destination");

        let destination = archive_destination(&category_dir, &source).expect("choose destination");
        assert_ne!(destination, existing);
        move_path(&source, &destination).expect("move to unique destination");

        assert_eq!(
            fs::read(&existing).expect("read existing"),
            b"existing report"
        );
        assert_eq!(fs::read(&destination).expect("read moved"), b"new report");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn archiving_external_item_moves_it_into_the_category_instead_of_storing_a_reference() {
        let root = unique_test_dir("dustdesk-archive-external");
        let data = root.join("data");
        let organizer = root.join("organizer");
        let launchers = root.join("launchers");
        let incoming = root.join("incoming");
        fs::create_dir_all(&data).expect("create data root");
        fs::create_dir_all(&organizer).expect("create organizer root");
        fs::create_dir_all(&launchers).expect("create launcher root");
        fs::create_dir_all(&incoming).expect("create incoming root");
        let source = incoming.join("important.txt");
        fs::write(&source, b"important data").expect("write source");
        let store = AppStore::for_test(data, organizer.clone(), launchers);

        let archived = archive_item_path(&store, "documents", &source.display().to_string())
            .expect("archive external item");
        let archived = PathBuf::from(archived);

        assert!(!source.exists(), "archive must use cut semantics");
        assert!(is_path_within(&archived, &organizer.join("documents")));
        assert_eq!(
            fs::read(&archived).expect("read archived item"),
            b"important data"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn archive_item_path_moves_an_item_between_category_boxes() {
        let root = unique_test_dir("dustdesk-archive-between-categories");
        let data = root.join("data");
        let organizer = root.join("organizer");
        let launchers = root.join("launchers");
        let source_category = organizer.join("source");
        fs::create_dir_all(&data).expect("create data root");
        fs::create_dir_all(&source_category).expect("create source category");
        fs::create_dir_all(&launchers).expect("create launcher root");
        let source = source_category.join("important.txt");
        fs::write(&source, b"important data").expect("write source");
        let store = AppStore::for_test(data, organizer.clone(), launchers);

        let archived = archive_item_path(&store, "target", &source.display().to_string())
            .expect("move item between categories");
        let archived = PathBuf::from(archived);

        assert!(
            !source.exists(),
            "the old category must no longer contain the item"
        );
        assert!(is_path_within(&archived, &organizer.join("target")));
        assert_eq!(
            fs::read(&archived).expect("read moved item"),
            b"important data"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn organized_item_marker_survives_store_reopen_and_snapshot_repair() {
        let root = unique_test_dir("dustdesk-organized-marker-restart");
        let data = root.join("data");
        let organizer = root.join("organizer");
        let launchers = root.join("launchers");
        let category_dir = organizer.join("documents");
        fs::create_dir_all(&category_dir).expect("create category directory");
        let organized_item = category_dir.join("important.txt");
        fs::write(&organized_item, b"important data").expect("write organized item");

        let store = AppStore::for_test(data.clone(), organizer.clone(), launchers.clone());
        let marker = organized_item.display().to_string();
        let config = AppConfig {
            desktop_categories: vec![DeskCategory {
                name: "documents".to_owned(),
                item_paths: vec![marker.clone()],
                ..DeskCategory::default()
            }],
            desktop_layout: DesktopLayout {
                split_category_indices: vec![0],
                ..DesktopLayout::default()
            },
            ..AppConfig::default()
        };
        store
            .save_config(&config)
            .expect("persist organized marker");
        drop(store);

        let reopened = AppStore::for_test(data, organizer, launchers);
        let mut reloaded = reopened.load_config_strict().expect("reload config");
        repair_category_item_paths(&reopened, &mut reloaded).expect("repair reopened snapshot");

        assert_eq!(reloaded.desktop_categories[0].item_paths, vec![marker]);
        assert_eq!(reloaded.desktop_layout.split_category_indices, vec![0]);
        assert_eq!(
            fs::read(&organized_item).expect("organized item remains stored"),
            b"important data"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn snapshot_repair_rebuilds_missing_marker_from_organizer_contents() {
        let root = unique_test_dir("dustdesk-organized-marker-recovery");
        let data = root.join("data");
        let organizer = root.join("organizer");
        let launchers = root.join("launchers");
        let category_dir = organizer.join("documents");
        fs::create_dir_all(&category_dir).expect("create category directory");
        let organized_item = category_dir.join("recovered.txt");
        fs::write(&organized_item, b"recover me").expect("write unmarked organized item");

        let store = AppStore::for_test(data, organizer, launchers);
        let mut config = AppConfig {
            desktop_categories: vec![DeskCategory {
                name: "documents".to_owned(),
                ..DeskCategory::default()
            }],
            ..AppConfig::default()
        };
        store
            .save_config(&config)
            .expect("persist config without marker");

        repair_category_item_paths(&store, &mut config).expect("recover missing marker");
        let recovered_marker = organized_item.display().to_string();
        assert_eq!(
            config.desktop_categories[0].item_paths,
            vec![recovered_marker.clone()]
        );
        assert_eq!(
            store
                .load_config_strict()
                .expect("reload repaired config")
                .desktop_categories[0]
                .item_paths,
            vec![recovered_marker]
        );
        assert_eq!(
            fs::read(&organized_item).expect("organized item remains stored"),
            b"recover me"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn snapshot_repair_recovers_unknown_physical_category_after_marker_loss() {
        let root = unique_test_dir("dustdesk-organized-category-recovery");
        let data = root.join("data");
        let organizer = root.join("organizer");
        let launchers = root.join("launchers");
        let category_dir = organizer.join("Recovered Custom");
        fs::create_dir_all(&category_dir).expect("create recovered category");
        let organized_item = category_dir.join("important.txt");
        fs::write(&organized_item, b"recover custom category").expect("write organized item");

        let store = AppStore::for_test(data, organizer, launchers);
        let mut config = AppConfig {
            desktop_categories: Vec::new(),
            ..AppConfig::default()
        };

        repair_category_item_paths(&store, &mut config).expect("recover physical category");

        assert_eq!(config.desktop_categories.len(), 1);
        assert_eq!(config.desktop_categories[0].name, "Recovered Custom");
        assert_eq!(
            config.desktop_categories[0].item_paths,
            vec![organized_item.display().to_string()]
        );
        let persisted = store.load_config_strict().expect("reload recovered marker");
        assert_eq!(persisted.desktop_categories[0].name, "Recovered Custom");
        assert!(
            organized_item.exists(),
            "recovery must not move stored data"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn snapshot_repair_does_not_duplicate_a_renamed_category_directory() {
        let root = unique_test_dir("dustdesk-renamed-category-recovery");
        let data = root.join("data");
        let organizer = root.join("organizer");
        let launchers = root.join("launchers");
        let original_category_dir = organizer.join("Original Name");
        fs::create_dir_all(&original_category_dir).expect("create original category");
        let organized_item = original_category_dir.join("important.txt");
        let unmarked_item = original_category_dir.join("also-important.txt");
        fs::write(&organized_item, b"renamed category data").expect("write organized item");
        fs::write(&unmarked_item, b"unmarked renamed data").expect("write unmarked item");

        let store = AppStore::for_test(data, organizer, launchers);
        let mut config = AppConfig {
            desktop_categories: vec![DeskCategory {
                name: "Renamed Category".to_owned(),
                item_paths: vec![organized_item.display().to_string()],
                ..DeskCategory::default()
            }],
            ..AppConfig::default()
        };

        repair_category_item_paths(&store, &mut config).expect("repair renamed category");

        assert_eq!(config.desktop_categories.len(), 1);
        assert_eq!(config.desktop_categories[0].name, "Renamed Category");
        assert_eq!(config.desktop_categories[0].item_paths.len(), 2);
        assert!(config.desktop_categories[0]
            .item_paths
            .iter()
            .any(|path| same_path_text(path, &organized_item.display().to_string())));
        assert!(config.desktop_categories[0]
            .item_paths
            .iter()
            .any(|path| same_path_text(path, &unmarked_item.display().to_string())));
        assert!(organized_item.exists());
        assert!(unmarked_item.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn exit_restore_persists_complete_marker_and_unique_desktop_plans() {
        let root = unique_test_dir("dustdesk-exit-restart-marker");
        let (store, organizer, desktop) = lifecycle_test_store(&root);
        let documents = organizer.join("documents");
        let media = organizer.join("media");
        fs::create_dir_all(&documents).expect("create documents category");
        fs::create_dir_all(&media).expect("create media category");
        let document_source = documents.join("report.txt");
        let media_source = media.join("report.txt");
        let existing_desktop = desktop.join("report.txt");
        for path in [&document_source, &media_source, &existing_desktop] {
            fs::write(path, b"identical bytes").expect("write collision fixture");
        }
        store
            .save_config(&AppConfig {
                desktop_categories: vec![
                    DeskCategory {
                        name: "documents".to_owned(),
                        item_paths: vec![document_source.display().to_string()],
                        ..DeskCategory::default()
                    },
                    DeskCategory {
                        name: "media".to_owned(),
                        item_paths: vec![media_source.display().to_string()],
                        ..DeskCategory::default()
                    },
                ],
                ..AppConfig::default()
            })
            .expect("save organized config");
        let persistence = MemoryRestartPersistence::default();

        let restored = restore_desktop_organization_for_exit_core(&store, &desktop, &persistence)
            .expect("restore organized items for exit");

        assert_eq!(restored, 2);
        assert_eq!(
            fs::read(&existing_desktop).expect("read original"),
            b"identical bytes"
        );
        let marker = persistence.current().expect("restart marker remains");
        assert_eq!(marker.categories.len(), 2);
        assert_eq!(marker.categories[0].category_index, Some(0));
        assert_eq!(marker.categories[1].category_index, Some(1));
        let planned_paths = marker
            .categories
            .iter()
            .flat_map(|category| &category.items)
            .map(|item| PathBuf::from(item.desktop_path.as_ref().expect("planned desktop path")))
            .collect::<Vec<_>>();
        assert_eq!(planned_paths.len(), 2);
        assert_ne!(planned_paths[0], planned_paths[1]);
        assert!(planned_paths.iter().all(|path| path.exists()));
        assert!(planned_paths.iter().all(|path| path != &existing_desktop));
        assert!(!document_source.exists());
        assert!(!media_source.exists());

        let saves = persistence.saves.borrow();
        assert!(saves.len() >= 3, "complete marker plus one plan per item");
        assert!(saves[0]
            .categories
            .iter()
            .flat_map(|category| &category.items)
            .all(|item| item.desktop_path.is_none()));
        assert!(persistence.source_states_at_save.borrow()[0]
            .iter()
            .all(|exists| *exists));
        let persisted = store.load_config_strict().expect("reload exit config");
        assert!(persisted
            .desktop_categories
            .iter()
            .all(|category| category.item_paths.is_empty()));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn exit_never_moves_an_item_before_its_marker_plan_is_durable() {
        for failed_save in 1..=3 {
            let root = unique_test_dir(&format!("dustdesk-marker-save-failure-{failed_save}"));
            let (store, organizer, desktop) = lifecycle_test_store(&root);
            let category_dir = organizer.join("documents");
            fs::create_dir_all(&category_dir).expect("create category");
            let first_source = category_dir.join("first.txt");
            let second_source = category_dir.join("second.txt");
            fs::write(&first_source, b"first").expect("write first source");
            fs::write(&second_source, b"second").expect("write second source");
            store
                .save_config(&AppConfig {
                    desktop_categories: vec![DeskCategory {
                        name: "documents".to_owned(),
                        item_paths: vec![
                            first_source.display().to_string(),
                            second_source.display().to_string(),
                        ],
                        ..DeskCategory::default()
                    }],
                    ..AppConfig::default()
                })
                .expect("save config");
            let persistence = MemoryRestartPersistence::failing_on_save(failed_save);

            let error = restore_desktop_organization_for_exit_core(&store, &desktop, &persistence)
                .expect_err("inject marker persistence failure");

            assert!(error.contains("injected marker save failure"));
            if failed_save <= 2 {
                assert!(first_source.exists());
                assert!(!desktop.join("first.txt").exists());
            } else {
                assert!(!first_source.exists());
                assert!(desktop.join("first.txt").exists());
            }
            assert!(
                second_source.exists(),
                "failed plan must precede second move"
            );
            assert!(!desktop.join("second.txt").exists());
            assert_eq!(
                first_source.exists() as usize + desktop.join("first.txt").exists() as usize,
                1
            );
            assert_eq!(
                second_source.exists() as usize + desktop.join("second.txt").exists() as usize,
                1
            );
            let _ = fs::remove_dir_all(root);
        }
    }

    #[test]
    fn restart_round_trip_recollects_only_marked_item_and_removes_marker() {
        let root = unique_test_dir("dustdesk-restart-round-trip");
        let (store, organizer, desktop) = lifecycle_test_store(&root);
        let category_dir = organizer.join("documents");
        fs::create_dir_all(&category_dir).expect("create category");
        let source = category_dir.join("report.txt");
        let unmarked_desktop_item = desktop.join("report.txt");
        let icon_position = DesktopIconPosition {
            screen_x: -1440,
            screen_y: 360,
            monitor_name: Some(r"\\.\DISPLAY5".to_owned()),
            monitor_offset_x: Some(480),
            monitor_offset_y: Some(176),
        };
        fs::write(&source, b"organized copy").expect("write organized source");
        fs::write(&unmarked_desktop_item, b"keep desktop copy").expect("write desktop collision");
        store
            .save_config(&AppConfig {
                desktop_categories: vec![DeskCategory {
                    name: "documents".to_owned(),
                    item_paths: vec![source.display().to_string()],
                    ..DeskCategory::default()
                }],
                desktop_icon_positions: BTreeMap::from([(
                    source.display().to_string(),
                    icon_position.clone(),
                )]),
                ..AppConfig::default()
            })
            .expect("save config");
        let persistence = MemoryRestartPersistence::default();
        restore_desktop_organization_for_exit_core(&store, &desktop, &persistence)
            .expect("exit restore");
        let marker = persistence.current().expect("marker");
        assert_eq!(
            marker.categories[0].items[0].desktop_icon_position,
            Some(icon_position.clone())
        );
        let planned = PathBuf::from(
            marker.categories[0].items[0]
                .desktop_path
                .clone()
                .expect("planned path"),
        );
        assert!(planned.exists());

        let recollected =
            run_startup_recollect(&store, &desktop, &persistence).expect("startup recollect");

        assert_eq!(recollected, 1);
        assert!(source.exists());
        assert!(!planned.exists());
        assert_eq!(
            fs::read(&source).expect("read recollected"),
            b"organized copy"
        );
        assert_eq!(
            fs::read(&unmarked_desktop_item).expect("read unmarked desktop item"),
            b"keep desktop copy"
        );
        assert!(persistence.current().is_none());
        assert_eq!(persistence.remove_calls.get(), 1);
        let config = store
            .load_config_strict()
            .expect("reload recollected config");
        assert_eq!(config.desktop_categories[0].item_paths.len(), 1);
        assert!(same_path_text(
            &config.desktop_categories[0].item_paths[0],
            &source.display().to_string()
        ));
        assert_eq!(
            desktop_icon_position_for_path(
                &config.desktop_icon_positions,
                &source.display().to_string(),
            ),
            Some(icon_position)
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn startup_without_restart_marker_never_recollects_desktop_reference() {
        let root = unique_test_dir("dustdesk-no-restart-marker");
        let (store, organizer, desktop) = lifecycle_test_store(&root);
        let desktop_item = desktop.join("manual.txt");
        fs::write(&desktop_item, b"manual desktop item").expect("write desktop item");
        store
            .save_config(&AppConfig {
                desktop_categories: vec![DeskCategory {
                    name: "documents".to_owned(),
                    item_paths: vec![desktop_item.display().to_string()],
                    ..DeskCategory::default()
                }],
                ..AppConfig::default()
            })
            .expect("save stale desktop reference");
        let persistence = MemoryRestartPersistence::default();

        assert_eq!(
            run_startup_recollect(&store, &desktop, &persistence).expect("startup without marker"),
            0
        );
        assert!(desktop_item.exists());
        assert!(organizer_contents(&organizer).is_empty());
        assert!(store
            .load_config_strict()
            .expect("reload config")
            .desktop_categories[0]
            .item_paths
            .is_empty());
        assert_eq!(persistence.remove_calls.get(), 0);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn startup_retains_only_failed_marker_items_and_exit_merges_them() {
        let root = unique_test_dir("dustdesk-recollect-partial-failure");
        let (store, organizer, desktop) = lifecycle_test_store(&root);
        let ok_source = organizer.join("ok").join("ok.txt");
        let blocked_source = organizer.join("blocked").join("bad.txt");
        let ok_desktop = desktop.join("ok.txt");
        let blocked_desktop = desktop.join("bad.txt");
        fs::write(&ok_desktop, b"ok").expect("write successful desktop item");
        fs::write(&blocked_desktop, b"blocked").expect("write blocked desktop item");
        fs::write(organizer.join("blocked"), b"parent is a file")
            .expect("block target category directory");
        store
            .save_config(&AppConfig {
                desktop_categories: vec![
                    DeskCategory {
                        name: "ok".to_owned(),
                        ..DeskCategory::default()
                    },
                    DeskCategory {
                        name: "blocked".to_owned(),
                        ..DeskCategory::default()
                    },
                ],
                ..AppConfig::default()
            })
            .expect("save config");
        let persistence = MemoryRestartPersistence::with_marker(DesktopOrganizationRestartMarker {
            schema_version: DESKTOP_ORGANIZATION_RESTART_SCHEMA_VERSION,
            categories: vec![
                RestartCategoryMarker {
                    category_name: "ok".to_owned(),
                    category_index: Some(0),
                    items: vec![RestartItemMarker {
                        source_path: ok_source.display().to_string(),
                        desktop_path: Some(ok_desktop.display().to_string()),
                        desktop_icon_position: None,
                    }],
                },
                RestartCategoryMarker {
                    category_name: "blocked".to_owned(),
                    category_index: Some(1),
                    items: vec![RestartItemMarker {
                        source_path: blocked_source.display().to_string(),
                        desktop_path: Some(blocked_desktop.display().to_string()),
                        desktop_icon_position: None,
                    }],
                },
            ],
        });

        assert_eq!(
            run_startup_recollect(&store, &desktop, &persistence)
                .expect("partial startup recollect"),
            1
        );
        assert!(ok_source.exists());
        assert!(!ok_desktop.exists());
        assert!(blocked_desktop.exists());
        let failed_marker = persistence.current().expect("failed marker retained");
        assert_eq!(failed_marker.categories.len(), 1);
        assert_eq!(failed_marker.categories[0].category_name, "blocked");
        assert_eq!(failed_marker.categories[0].items.len(), 1);

        let current_dir = organizer.join("current");
        fs::create_dir_all(&current_dir).expect("create current category");
        let current_source = current_dir.join("current.txt");
        fs::write(&current_source, b"current organized item").expect("write current source");
        let mut config = store.load_config_strict().expect("load partial config");
        config.desktop_categories.push(DeskCategory {
            name: "current".to_owned(),
            item_paths: vec![current_source.display().to_string()],
            ..DeskCategory::default()
        });
        store.save_config(&config).expect("save current config");

        assert_eq!(
            restore_desktop_organization_for_exit_core(&store, &desktop, &persistence)
                .expect("merge failed marker on exit"),
            2
        );
        let merged = persistence.current().expect("merged marker");
        assert!(merged.categories.iter().any(|category| {
            category.category_name == "blocked"
                && category.items.iter().any(|item| {
                    same_path_text(&item.source_path, &blocked_source.display().to_string())
                })
        }));
        assert!(merged.categories.iter().any(|category| {
            category.category_name == "current"
                && category.items.iter().any(|item| {
                    same_path_text(&item.source_path, &current_source.display().to_string())
                })
        }));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn startup_is_idempotent_across_restart_crash_boundaries() {
        for state in 0..3 {
            let root = unique_test_dir(&format!("dustdesk-restart-crash-{state}"));
            let (store, organizer, desktop) = lifecycle_test_store(&root);
            let source = organizer.join("documents").join("item.txt");
            let desktop_path = desktop.join("item.txt");
            fs::create_dir_all(source.parent().expect("source parent"))
                .expect("create source parent");
            if state != 1 {
                fs::write(&source, b"crash-safe data").expect("write organizer source");
            } else {
                fs::write(&desktop_path, b"crash-safe data").expect("write moved desktop item");
            }
            let config_paths = if state == 2 {
                Vec::new()
            } else {
                vec![source.display().to_string()]
            };
            store
                .save_config(&AppConfig {
                    desktop_categories: vec![DeskCategory {
                        name: "documents".to_owned(),
                        item_paths: config_paths,
                        ..DeskCategory::default()
                    }],
                    ..AppConfig::default()
                })
                .expect("save crash state config");
            let persistence =
                MemoryRestartPersistence::with_marker(DesktopOrganizationRestartMarker {
                    schema_version: DESKTOP_ORGANIZATION_RESTART_SCHEMA_VERSION,
                    categories: vec![RestartCategoryMarker {
                        category_name: "documents".to_owned(),
                        category_index: Some(0),
                        items: vec![RestartItemMarker {
                            source_path: source.display().to_string(),
                            desktop_path: (state != 0).then(|| desktop_path.display().to_string()),
                            desktop_icon_position: None,
                        }],
                    }],
                });

            run_startup_recollect(&store, &desktop, &persistence).expect("recover crash boundary");
            run_startup_recollect(&store, &desktop, &persistence)
                .expect("repeat recovery idempotently");

            assert!(source.exists(), "state {state} source should be organized");
            assert!(
                !desktop_path.exists(),
                "state {state} desktop should be clean"
            );
            assert_eq!(
                fs::read(&source).expect("read recovered data"),
                b"crash-safe data"
            );
            assert!(persistence.current().is_none());
            let config = store.load_config_strict().expect("reload crash config");
            assert_eq!(config.desktop_categories[0].item_paths.len(), 1);
            let _ = fs::remove_dir_all(root);
        }
    }

    #[test]
    fn startup_preserves_marker_when_source_and_desktop_both_exist() {
        let root = unique_test_dir("dustdesk-restart-ambiguous-copies");
        let (store, organizer, desktop) = lifecycle_test_store(&root);
        let source = organizer.join("documents").join("item.txt");
        let desktop_path = desktop.join("item.txt");
        fs::create_dir_all(source.parent().expect("source parent")).expect("create category");
        fs::write(&source, b"organizer copy").expect("write organizer copy");
        fs::write(&desktop_path, b"desktop copy").expect("write desktop copy");
        store
            .save_config(&AppConfig {
                desktop_categories: vec![DeskCategory {
                    name: "documents".to_owned(),
                    item_paths: vec![source.display().to_string()],
                    ..DeskCategory::default()
                }],
                ..AppConfig::default()
            })
            .expect("save ambiguous config");
        let persistence = MemoryRestartPersistence::with_marker(DesktopOrganizationRestartMarker {
            schema_version: DESKTOP_ORGANIZATION_RESTART_SCHEMA_VERSION,
            categories: vec![RestartCategoryMarker {
                category_name: "documents".to_owned(),
                category_index: Some(0),
                items: vec![RestartItemMarker {
                    source_path: source.display().to_string(),
                    desktop_path: Some(desktop_path.display().to_string()),
                    desktop_icon_position: None,
                }],
            }],
        });

        assert_eq!(
            run_startup_recollect(&store, &desktop, &persistence)
                .expect("preserve ambiguous state"),
            0
        );
        assert_eq!(fs::read(&source).expect("read source"), b"organizer copy");
        assert_eq!(
            fs::read(&desktop_path).expect("read desktop"),
            b"desktop copy"
        );
        assert!(persistence.current().is_some());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn startup_keeps_duplicate_named_categories_distinct_when_indices_drift() {
        let root = unique_test_dir("dustdesk-duplicate-category-marker");
        let (store, organizer, desktop) = lifecycle_test_store(&root);
        let first_source = organizer.join("first-physical").join("first.txt");
        let second_source = organizer.join("second-physical").join("second.txt");
        let first_desktop = desktop.join("first.txt");
        let second_desktop = desktop.join("second.txt");
        fs::write(&first_desktop, b"first").expect("write first desktop item");
        fs::write(&second_desktop, b"second").expect("write second desktop item");
        store
            .save_config(&AppConfig {
                desktop_categories: vec![
                    DeskCategory {
                        name: "same".to_owned(),
                        ..DeskCategory::default()
                    },
                    DeskCategory {
                        name: "same".to_owned(),
                        ..DeskCategory::default()
                    },
                ],
                ..AppConfig::default()
            })
            .expect("save duplicate categories");
        let persistence = MemoryRestartPersistence::with_marker(DesktopOrganizationRestartMarker {
            schema_version: DESKTOP_ORGANIZATION_RESTART_SCHEMA_VERSION,
            categories: vec![
                RestartCategoryMarker {
                    category_name: "same".to_owned(),
                    category_index: Some(10),
                    items: vec![RestartItemMarker {
                        source_path: first_source.display().to_string(),
                        desktop_path: Some(first_desktop.display().to_string()),
                        desktop_icon_position: None,
                    }],
                },
                RestartCategoryMarker {
                    category_name: "same".to_owned(),
                    category_index: Some(11),
                    items: vec![RestartItemMarker {
                        source_path: second_source.display().to_string(),
                        desktop_path: Some(second_desktop.display().to_string()),
                        desktop_icon_position: None,
                    }],
                },
            ],
        });

        assert_eq!(
            run_startup_recollect(&store, &desktop, &persistence)
                .expect("recollect duplicate categories"),
            2
        );
        let config = store
            .load_config_strict()
            .expect("reload duplicate categories");
        assert_eq!(config.desktop_categories[0].item_paths.len(), 1);
        assert_eq!(config.desktop_categories[1].item_paths.len(), 1);
        assert!(same_path_text(
            &config.desktop_categories[0].item_paths[0],
            &first_source.display().to_string()
        ));
        assert!(same_path_text(
            &config.desktop_categories[1].item_paths[0],
            &second_source.display().to_string()
        ));
        assert!(persistence.current().is_none());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn exit_rehomes_source_marker_to_its_current_category() {
        let root = unique_test_dir("dustdesk-rehome-restart-marker");
        let (store, organizer, desktop) = lifecycle_test_store(&root);
        let source = organizer.join("physical").join("item.txt");
        fs::create_dir_all(source.parent().expect("source parent")).expect("create source parent");
        fs::write(&source, b"reassigned item").expect("write source");
        store
            .save_config(&AppConfig {
                desktop_categories: vec![DeskCategory {
                    name: "current category".to_owned(),
                    item_paths: vec![source.display().to_string()],
                    ..DeskCategory::default()
                }],
                ..AppConfig::default()
            })
            .expect("save reassigned config");
        let persistence = MemoryRestartPersistence::with_marker(DesktopOrganizationRestartMarker {
            schema_version: DESKTOP_ORGANIZATION_RESTART_SCHEMA_VERSION,
            categories: vec![RestartCategoryMarker {
                category_name: "stale category".to_owned(),
                category_index: Some(9),
                items: vec![RestartItemMarker {
                    source_path: source.display().to_string(),
                    desktop_path: None,
                    desktop_icon_position: None,
                }],
            }],
        });

        assert_eq!(
            restore_desktop_organization_for_exit_core(&store, &desktop, &persistence)
                .expect("restore reassigned item"),
            1
        );
        let marker = persistence.current().expect("reassigned marker");
        assert_eq!(marker.categories.len(), 1);
        assert_eq!(marker.categories[0].category_name, "current category");
        assert_eq!(marker.categories[0].category_index, Some(0));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn explicit_restore_intent_clears_restart_marker() {
        let persistence = MemoryRestartPersistence::with_marker(DesktopOrganizationRestartMarker {
            schema_version: DESKTOP_ORGANIZATION_RESTART_SCHEMA_VERSION,
            categories: vec![RestartCategoryMarker {
                category_name: "documents".to_owned(),
                category_index: Some(0),
                items: vec![RestartItemMarker {
                    source_path: r"C:\DesktopOrganizer\documents\item.txt".to_owned(),
                    desktop_path: Some(r"C:\Users\User\Desktop\item.txt".to_owned()),
                    desktop_icon_position: None,
                }],
            }],
        });

        clear_desktop_organization_restart_marker(&persistence)
            .expect("clear marker before explicit restore");

        assert!(persistence.current().is_none());
        assert_eq!(persistence.remove_calls.get(), 1);
        assert_eq!(persistence.saves.borrow().len(), 1);
        assert!(persistence.saves.borrow()[0].is_empty());
    }

    #[test]
    fn move_rejects_destination_inside_source_and_keeps_the_source_tree() {
        let root = unique_test_dir("dustdesk-move-inside-source");
        let source = root.join("source");
        let destination = source.join("nested").join("destination");
        fs::create_dir_all(source.join("nested")).expect("create source tree");
        fs::write(source.join("important.txt"), b"keep this data").expect("write source");

        let error = move_path(&source, &destination).expect_err("reject recursive move");

        assert!(error.contains("自身内部"));
        assert_eq!(
            fs::read(source.join("important.txt")).expect("source remains"),
            b"keep this data"
        );
        assert!(!destination.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(windows)]
    #[test]
    fn move_rejects_destination_inside_source_through_a_directory_alias() {
        let root = unique_test_dir("dustdesk-move-alias-inside-source");
        let source = root.join("source");
        let alias = root.join("source-alias");
        fs::create_dir_all(&source).expect("create source");
        fs::write(source.join("important.txt"), b"keep this data").expect("write source");
        create_test_directory_reparse_point(&alias, &source);
        let destination = alias.join("nested").join("destination");

        let error = move_path(&source, &destination).expect_err("reject aliased recursive move");

        assert!(error.contains("自身内部"));
        assert_eq!(
            fs::read(source.join("important.txt")).expect("source remains"),
            b"keep this data"
        );
        assert!(!destination.exists());
        fs::remove_dir(&alias).expect("remove source alias");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn runtime_directory_copy_keeps_source_until_verified_cleanup() {
        let root = unique_test_dir("dustdesk-runtime-copy");
        let source = root.join("source");
        let target = root.join("target");
        fs::create_dir_all(source.join("nested")).expect("create source tree");
        fs::write(source.join("nested").join("important.txt"), b"runtime data")
            .expect("write source data");

        copy_directory_contents_preserving_source(&source, &target)
            .expect("copy complete directory before switching");

        assert_eq!(
            fs::read(source.join("nested").join("important.txt")).expect("source remains"),
            b"runtime data"
        );
        assert_eq!(
            fs::read(target.join("nested").join("important.txt")).expect("target copied"),
            b"runtime data"
        );

        remove_verified_directory_contents(&source, &target)
            .expect("remove old directory only after verification");
        assert!(!source.exists());
        assert_eq!(
            fs::read(target.join("nested").join("important.txt")).expect("target remains"),
            b"runtime data"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn runtime_directory_copy_rejects_conflict_and_preserves_both_versions() {
        let root = unique_test_dir("dustdesk-runtime-conflict");
        let source = root.join("source");
        let target = root.join("target");
        fs::create_dir_all(&source).expect("create source");
        fs::create_dir_all(&target).expect("create target");
        fs::write(source.join("config.json"), b"source config").expect("write source config");
        fs::write(target.join("config.json"), b"target config").expect("write target config");

        let error = copy_directory_contents_preserving_source(&source, &target)
            .expect_err("reject conflicting migration target");

        assert!(error.contains("不同内容"));
        assert_eq!(
            fs::read(source.join("config.json")).expect("source preserved"),
            b"source config"
        );
        assert_eq!(
            fs::read(target.join("config.json")).expect("target preserved"),
            b"target config"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn path_rewrite_preserves_nested_components_when_windows_path_case_differs() {
        let old_root = PathBuf::from(r"D:\Data");
        let new_root = PathBuf::from(r"E:\DustDeskData");
        let rewritten = rewrite_path_prefix(
            r"d:\data\ClipboardImages\nested\image.png",
            &[(&old_root, &new_root)],
        )
        .expect("rewrite nested path");

        assert!(same_path_text(
            &rewritten,
            r"E:\DustDeskData\ClipboardImages\nested\image.png"
        ));
    }

    #[test]
    fn runtime_roots_must_not_overlap() {
        let data = PathBuf::from(r"D:\DustDesk\Data");
        let organizer = data.join("DesktopOrganizer");
        let launchers = PathBuf::from(r"D:\DustDesk\Launchers");

        assert!(paths_overlap(&data, &organizer));
        assert!(paths_overlap(&organizer, &data));
        assert!(!paths_overlap(&data, &launchers));
    }

    #[test]
    fn restore_collision_preserves_both_items_with_unique_desktop_name() {
        let root = unique_test_dir("dustdesk-restore-collision");
        let organizer = root.join("organizer");
        let desktop = root.join("desktop");
        fs::create_dir_all(&organizer).expect("create organizer");
        fs::create_dir_all(&desktop).expect("create desktop");
        let source = organizer.join("report.txt");
        let existing = desktop.join("report.txt");
        fs::write(&source, b"organized report").expect("write organized source");
        fs::write(&existing, b"desktop report").expect("write desktop item");

        let restored =
            restore_path_to_desktop_silent(&source, &desktop).expect("restore collision safely");

        assert_ne!(restored, existing);
        assert_eq!(
            fs::read(&existing).expect("read desktop item"),
            b"desktop report"
        );
        assert_eq!(
            fs::read(&restored).expect("read restored item"),
            b"organized report"
        );
        assert!(
            !source.exists(),
            "organized data should be moved, not discarded"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn restore_collision_removes_only_a_byte_identical_organized_duplicate() {
        let root = unique_test_dir("dustdesk-restore-identical");
        let organizer = root.join("organizer");
        let desktop = root.join("desktop");
        fs::create_dir_all(&organizer).expect("create organizer");
        fs::create_dir_all(&desktop).expect("create desktop");
        let source = organizer.join("report.txt");
        let existing = desktop.join("report.txt");
        fs::write(&source, b"identical report").expect("write organized source");
        fs::write(&existing, b"identical report").expect("write desktop item");

        let restored =
            restore_path_to_desktop_silent(&source, &desktop).expect("deduplicate identical item");

        assert_eq!(restored, existing);
        assert!(!source.exists(), "verified duplicate should be removed");
        assert_eq!(
            fs::read(&existing).expect("desktop item remains"),
            b"identical report"
        );
        assert!(!desktop.join("report 2.txt").exists());
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(windows)]
    #[test]
    fn restore_collision_preserves_items_when_named_streams_differ() {
        let root = unique_test_dir("dustdesk-restore-ads-different");
        let organizer = root.join("organizer");
        let desktop = root.join("desktop");
        fs::create_dir_all(&organizer).expect("create organizer");
        fs::create_dir_all(&desktop).expect("create desktop");
        let source = organizer.join("report.txt");
        let existing = desktop.join("report.txt");
        fs::write(&source, b"same default stream").expect("write organized source");
        fs::write(&existing, b"same default stream").expect("write desktop item");
        fs::write(
            path_with_windows_stream(&source, ":critical"),
            b"source ADS",
        )
        .expect("write source ADS");
        fs::write(
            path_with_windows_stream(&existing, ":critical"),
            b"desktop ADS",
        )
        .expect("write desktop ADS");

        let restored = restore_path_to_desktop_silent(&source, &desktop)
            .expect("preserve different named streams");

        assert_ne!(restored, existing);
        assert_eq!(
            fs::read(path_with_windows_stream(&existing, ":critical")).expect("read desktop ADS"),
            b"desktop ADS"
        );
        assert_eq!(
            fs::read(path_with_windows_stream(&restored, ":critical")).expect("read restored ADS"),
            b"source ADS"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn staged_directory_move_verifies_nested_contents_before_source_removal() {
        let root = unique_test_dir("dustdesk-staged-move");
        let source = root.join("source");
        let destination = root.join("destination");
        fs::create_dir_all(source.join("nested")).expect("create source tree");
        fs::write(source.join("root.bin"), [0, 1, 2, 3]).expect("write root file");
        fs::write(source.join("nested").join("child.txt"), b"nested content")
            .expect("write nested file");

        move_path_via_staging(&source, &destination).expect("move through staging");

        assert!(!source.exists());
        assert_eq!(
            fs::read(destination.join("root.bin")).expect("read root file"),
            [0, 1, 2, 3]
        );
        assert_eq!(
            fs::read(destination.join("nested").join("child.txt")).expect("read nested file"),
            b"nested content"
        );

        let file_source = root.join("source.txt");
        let file_destination = root.join("destination.txt");
        fs::write(&file_source, b"file content").expect("write file source");
        #[cfg(windows)]
        fs::write(
            path_with_windows_stream(&file_source, ":critical"),
            b"critical ADS content",
        )
        .expect("write source ADS");
        let mut source_permissions = fs::metadata(&file_source)
            .expect("read source metadata")
            .permissions();
        source_permissions.set_readonly(true);
        fs::set_permissions(&file_source, source_permissions).expect("make source read-only");
        let source_modified = fs::metadata(&file_source)
            .expect("read source metadata")
            .modified()
            .expect("read source last-write time");
        move_path_via_staging(&file_source, &file_destination)
            .expect("move file through reserved staging");
        assert!(!file_source.exists());
        assert_eq!(
            fs::read(file_destination).expect("read moved file"),
            b"file content"
        );
        #[cfg(windows)]
        assert_eq!(
            fs::read(path_with_windows_stream(
                &root.join("destination.txt"),
                ":critical"
            ))
            .expect("read moved ADS"),
            b"critical ADS content"
        );
        let destination_metadata =
            fs::metadata(root.join("destination.txt")).expect("read destination metadata");
        assert!(destination_metadata.permissions().readonly());
        assert_eq!(
            destination_metadata
                .modified()
                .expect("read destination last-write time"),
            source_modified
        );
        let mut cleanup_permissions = destination_metadata.permissions();
        cleanup_permissions.set_readonly(false);
        fs::set_permissions(root.join("destination.txt"), cleanup_permissions)
            .expect("clear destination read-only flag");
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(windows)]
    #[test]
    fn large_transfer_tree_uses_a_bounded_root_lock() {
        let root = unique_test_dir("dustdesk-bounded-transfer-lock");
        let source = root.join("source");
        fs::create_dir_all(&source).expect("create source");
        for index in 0..=MAX_LOCKED_TRANSFER_TREE_ENTRIES {
            fs::write(source.join(format!("item-{index}.txt")), b"data")
                .expect("write source item");
        }

        let locked = lock_transfer_source_tree(&source).expect("lock large source tree");
        assert!(!locked.fully_locked);
        assert_eq!(locked.entries.len(), 1);
        drop(locked);

        let destination = root.join("destination");
        move_path_via_staging(&source, &destination).expect("move large source tree");
        assert!(!source.exists());
        assert_eq!(
            fs::read(destination.join("item-0.txt")).expect("read moved item"),
            b"data"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[cfg(windows)]
    #[test]
    fn uncommitted_quarantine_recovers_the_original_directory() {
        let root = unique_test_dir("dustdesk-uncommitted-quarantine");
        let source = root.join("source");
        let destination = root.join("destination");
        fs::create_dir_all(&source).expect("create source");
        fs::write(source.join("important.txt"), b"keep this data").expect("write source");

        let quarantine =
            QuarantinedTransferSource::reserve(&source, &destination).expect("reserve quarantine");
        let journal_path = quarantine.journal_path.clone();
        assert!(!source.exists());
        assert!(quarantine.payload.exists());
        drop(quarantine);

        recover_transfer_quarantine_journal(&journal_path).expect("recover quarantine");

        assert_eq!(
            fs::read(source.join("important.txt")).expect("read restored source"),
            b"keep this data"
        );
        assert!(!destination.exists());
        assert!(!journal_path.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(windows)]
    #[test]
    fn committed_quarantine_finishes_cleanup_after_interruption() {
        let root = unique_test_dir("dustdesk-committed-quarantine");
        let source = root.join("source");
        let destination = root.join("destination");
        fs::create_dir_all(&source).expect("create source");
        fs::write(source.join("first.txt"), b"first").expect("write first source");
        fs::write(source.join("second.txt"), b"second").expect("write second source");

        let quarantine =
            QuarantinedTransferSource::reserve(&source, &destination).expect("reserve quarantine");
        fs::create_dir_all(&destination).expect("create destination");
        copy_dir_recursive(&quarantine.payload, &destination).expect("copy quarantined source");
        quarantine
            .mark_committed()
            .expect("mark destination committed");
        quarantine
            .mark_cleanup_started()
            .expect("mark source cleanup started");
        fs::remove_file(quarantine.payload.join("first.txt"))
            .expect("simulate interrupted source cleanup");
        let journal_path = quarantine.journal_path.clone();
        let remaining_payload = quarantine.payload.clone();
        drop(quarantine);

        recover_transfer_quarantine_journal_with_mode(&journal_path, true)
            .expect("defer committed cleanup during startup");
        assert!(journal_path.exists());
        assert!(remaining_payload.join("second.txt").exists());

        recover_transfer_quarantine_journal(&journal_path).expect("finish committed cleanup");

        assert!(!source.exists());
        assert_eq!(
            fs::read(destination.join("first.txt")).expect("read committed first file"),
            b"first"
        );
        assert_eq!(
            fs::read(destination.join("second.txt")).expect("read committed second file"),
            b"second"
        );
        assert!(!journal_path.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn committed_exit_journal_is_cleaned_before_marker_recollection() {
        let root = unique_test_dir("dustdesk-committed-marker-recollect");
        let (store, organizer, desktop) = lifecycle_test_store(&root);
        let source = organizer.join("documents").join("folder");
        let destination = desktop.join("folder");
        fs::create_dir_all(&source).expect("create organized directory");
        fs::write(source.join("important.txt"), b"journal protected data")
            .expect("write organized data");
        store
            .save_config(&AppConfig {
                desktop_categories: vec![DeskCategory {
                    name: "documents".to_owned(),
                    item_paths: vec![source.display().to_string()],
                    ..DeskCategory::default()
                }],
                ..AppConfig::default()
            })
            .expect("save organized config");

        let quarantine = QuarantinedTransferSource::reserve(&source, &destination)
            .expect("reserve exit quarantine");
        fs::create_dir_all(&destination).expect("create committed desktop destination");
        copy_dir_recursive(&quarantine.payload, &destination).expect("copy committed destination");
        quarantine
            .mark_committed()
            .expect("persist committed marker");
        let journal_path = quarantine.journal_path.clone();
        drop(quarantine);
        assert!(transfer_quarantine_journal_matches_paths(
            &journal_path,
            &[source.clone(), destination.clone()]
        )
        .expect("match lifecycle paths"));
        recover_transfer_quarantine_journal_with_mode(&journal_path, true)
            .expect("defer committed cleanup");
        assert!(journal_path.exists());

        recover_transfer_quarantine_journal(&journal_path)
            .expect("finish related cleanup before recollect");
        assert!(!journal_path.exists());
        assert!(destination.exists());
        let persistence = MemoryRestartPersistence::with_marker(DesktopOrganizationRestartMarker {
            schema_version: DESKTOP_ORGANIZATION_RESTART_SCHEMA_VERSION,
            categories: vec![RestartCategoryMarker {
                category_name: "documents".to_owned(),
                category_index: Some(0),
                items: vec![RestartItemMarker {
                    source_path: source.display().to_string(),
                    desktop_path: Some(destination.display().to_string()),
                    desktop_icon_position: None,
                }],
            }],
        });

        assert_eq!(
            run_startup_recollect(&store, &desktop, &persistence)
                .expect("recollect after committed cleanup"),
            1
        );
        assert_eq!(
            fs::read(source.join("important.txt")).expect("read recollected data"),
            b"journal protected data"
        );
        assert!(!destination.exists());
        assert!(persistence.current().is_none());
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(windows)]
    #[test]
    fn committed_quarantine_keeps_source_when_destination_changes_before_cleanup() {
        let root = unique_test_dir("dustdesk-changed-committed-destination");
        let source = root.join("source");
        let destination = root.join("destination");
        fs::create_dir_all(&source).expect("create source");
        fs::write(source.join("important.txt"), b"original data").expect("write source");

        let quarantine =
            QuarantinedTransferSource::reserve(&source, &destination).expect("reserve quarantine");
        fs::create_dir_all(&destination).expect("create destination");
        copy_dir_recursive(&quarantine.payload, &destination).expect("copy quarantined source");
        quarantine
            .mark_committed()
            .expect("mark destination committed");
        fs::write(destination.join("important.txt"), b"changed destination")
            .expect("change committed destination");
        let journal_path = quarantine.journal_path.clone();
        let payload = quarantine.payload.clone();
        drop(quarantine);

        let error = recover_transfer_quarantine_journal(&journal_path)
            .expect_err("changed destination must keep isolated source");
        assert!(error.contains("目标复核失败"));
        assert_eq!(
            fs::read(payload.join("important.txt")).expect("read preserved isolated source"),
            b"original data"
        );
        assert!(journal_path.exists());

        fs::remove_dir_all(&destination).expect("remove changed destination");
        recover_transfer_quarantine_journal(&journal_path).expect("restore preserved source");
        assert_eq!(
            fs::read(source.join("important.txt")).expect("read restored source"),
            b"original data"
        );
        assert!(!journal_path.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn staged_move_keeps_source_when_copy_verification_fails() {
        let root = unique_test_dir("dustdesk-staged-move-failure");
        let source = root.join("source");
        let destination = root.join("destination");
        fs::create_dir_all(&source).expect("create source");
        fs::write(source.join("important.txt"), b"keep this data").expect("write source data");

        let error = move_path_via_staging_with_verifier(&source, &destination, |_, _| {
            Err("injected verification failure".to_owned())
        })
        .expect_err("abort failed verification");

        assert_eq!(error, "injected verification failure");
        assert_eq!(
            fs::read(source.join("important.txt")).expect("source remains intact"),
            b"keep this data"
        );
        assert!(!destination.exists(), "destination must not be committed");
        assert!(
            fs::read_dir(&root)
                .expect("read test root")
                .all(|entry| !is_internal_transfer_path(&entry.expect("read entry").path())),
            "failed staging copy should be cleaned"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn copy_verification_rejects_same_length_content_mismatch() {
        let root = unique_test_dir("dustdesk-copy-verification");
        fs::create_dir_all(&root).expect("create test dir");
        let source = root.join("source.bin");
        let destination = root.join("destination.bin");
        fs::write(&source, b"source").expect("write source");
        fs::write(&destination, b"target").expect("write destination");

        let error = verify_copied_path(&source, &destination).expect_err("detect corruption");

        assert!(error.contains("文件内容不一致"));
        assert_eq!(fs::read(&source).expect("source remains"), b"source");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn move_refuses_existing_destination_and_keeps_source() {
        let root = unique_test_dir("dustdesk-move-collision");
        fs::create_dir_all(&root).expect("create test dir");
        let source = root.join("source.txt");
        let destination = root.join("destination.txt");
        fs::write(&source, b"source").expect("write source");
        fs::write(&destination, b"destination").expect("write destination");

        let error = move_path(&source, &destination).expect_err("reject destination collision");

        assert!(error.contains("目标项目已存在"));
        assert_eq!(fs::read(&source).expect("source remains"), b"source");
        assert_eq!(
            fs::read(&destination).expect("destination remains"),
            b"destination"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn directory_migration_reports_nested_collision_and_preserves_both_files() {
        let root = unique_test_dir("dustdesk-directory-collision");
        let source = root.join("source");
        let target = root.join("target");
        fs::create_dir_all(source.join("nested")).expect("create source tree");
        fs::create_dir_all(target.join("nested")).expect("create target tree");
        let source_file = source.join("nested").join("report.txt");
        let target_file = target.join("nested").join("report.txt");
        fs::write(&source_file, b"source report").expect("write source report");
        fs::write(&target_file, b"target report").expect("write target report");

        let error = move_directory_contents(&source, &target).expect_err("report collision");

        assert!(error.contains("迁移目标已存在"));
        assert_eq!(
            fs::read(&source_file).expect("source remains"),
            b"source report"
        );
        assert_eq!(
            fs::read(&target_file).expect("target remains"),
            b"target report"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(windows)]
    #[test]
    fn windows_no_replace_rename_preserves_existing_destination() {
        let root = unique_test_dir("dustdesk-no-replace");
        fs::create_dir_all(&root).expect("create test dir");
        let source = root.join("source.txt");
        let destination = root.join("destination.txt");
        fs::write(&source, b"source").expect("write source");
        fs::write(&destination, b"destination").expect("write destination");

        rename_path_no_replace(&source, &destination).expect_err("must not replace destination");

        assert_eq!(fs::read(&source).expect("source remains"), b"source");
        assert_eq!(
            fs::read(&destination).expect("destination remains"),
            b"destination"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn staged_commit_does_not_replace_destination_created_during_copy() {
        let root = unique_test_dir("dustdesk-staged-race");
        let source = root.join("source");
        let destination = root.join("destination");
        fs::create_dir_all(&source).expect("create source");
        fs::write(source.join("important.txt"), b"source data").expect("write source data");

        let error =
            move_path_via_staging_with_verifier(&source, &destination, |source, staging| {
                verify_copied_path(source, staging)?;
                fs::write(&destination, b"late destination").map_err(to_message)
            })
            .expect_err("no-replace commit must reject late destination");

        assert!(!error.is_empty());
        assert_eq!(
            fs::read(source.join("important.txt")).expect("source remains"),
            b"source data"
        );
        assert_eq!(
            fs::read(&destination).expect("late destination remains"),
            b"late destination"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(windows)]
    #[test]
    fn staged_move_rejects_new_source_entries_created_after_verification() {
        let root = unique_test_dir("dustdesk-source-lock");
        let source = root.join("source");
        let destination = root.join("destination");
        let renamed_source = root.join("renamed-source");
        fs::create_dir_all(&source).expect("create source");
        fs::write(source.join("important.txt"), b"stable data").expect("write source data");

        let error =
            move_path_via_staging_with_verifier(&source, &destination, |source, staging| {
                verify_copied_path(source, staging)?;
                assert!(
                    rename_path_no_replace(source, &renamed_source).is_err(),
                    "locked source path must not be replaceable"
                );
                assert!(
                    fs::write(source.join("important.txt"), b"changed data").is_err(),
                    "locked source file must not be writable"
                );
                fs::write(source.join("late.txt"), b"late data").map_err(to_message)?;
                Ok(())
            })
            .expect_err("source cleanup must stop when an unverified entry appears");

        assert!(error.contains("目录内容不完整"));
        assert!(source.exists());
        assert!(!renamed_source.exists());
        assert_eq!(
            fs::read(source.join("important.txt")).expect("verified source data remains"),
            b"stable data"
        );
        assert!(!destination.exists());
        assert_eq!(
            fs::read(source.join("late.txt")).expect("late source data remains"),
            b"late data"
        );
        assert!(!destination.join("late.txt").exists());
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(windows)]
    #[test]
    fn transfer_rejects_top_level_and_nested_directory_reparse_points() {
        let root = unique_test_dir("dustdesk-reparse");
        let target = root.join("target");
        fs::create_dir_all(&target).expect("create target");
        fs::write(target.join("secret.txt"), b"target data").expect("write target data");

        let top_level_link = root.join("top-level-link");
        create_test_directory_reparse_point(&top_level_link, &target);
        let top_level_destination = root.join("top-level-destination");
        let top_level_error = move_path(&top_level_link, &top_level_destination)
            .expect_err("reject top-level reparse point");
        assert!(top_level_error.contains("符号链接或目录联接"));
        assert!(!top_level_destination.exists());

        let nested_source = root.join("nested-source");
        fs::create_dir_all(&nested_source).expect("create nested source");
        let nested_link = nested_source.join("nested-link");
        create_test_directory_reparse_point(&nested_link, &target);
        let nested_destination = root.join("nested-destination");
        let nested_error = move_path(&nested_source, &nested_destination)
            .expect_err("reject nested reparse point");
        assert!(nested_error.contains("符号链接或目录联接"));
        assert!(nested_source.exists());
        assert!(!nested_destination.exists());
        assert_eq!(
            fs::read(target.join("secret.txt")).expect("target remains"),
            b"target data"
        );

        fs::remove_dir(&nested_link).expect("remove nested reparse point");
        fs::remove_dir(&top_level_link).expect("remove top-level reparse point");
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(windows)]
    #[test]
    fn staging_copy_and_cleanup_never_follow_preexisting_paths_or_nested_reparse_points() {
        let root = unique_test_dir("dustdesk-staging-reparse");
        let target = root.join("target");
        fs::create_dir_all(&target).expect("create target");
        fs::write(target.join("secret.txt"), b"target data").expect("write target data");

        let directory_source = root.join("directory-source");
        let directory_staging = root.join("directory-staging");
        fs::create_dir_all(directory_source.join("nested")).expect("create directory source");
        fs::create_dir_all(&directory_staging).expect("reserve directory staging");
        let nested_reparse = directory_staging.join("nested");
        create_test_directory_reparse_point(&nested_reparse, &target);

        copy_dir_recursive(&directory_source, &directory_staging)
            .expect_err("preexisting nested path must abort copy");
        assert!(
            lock_transfer_source_tree(&directory_staging).is_err(),
            "cleanup validation must reject nested reparse points"
        );
        assert!(
            directory_staging.exists(),
            "cleanup must retain a tree containing a reparse point"
        );
        assert_eq!(
            fs::read(target.join("secret.txt")).expect("target remains"),
            b"target data"
        );

        let file_source = root.join("file-source");
        let file_staging = root.join("file-staging");
        fs::create_dir_all(&file_source).expect("create file source");
        fs::create_dir_all(&file_staging).expect("reserve file staging");
        fs::write(file_source.join("data.txt"), b"source data").expect("write source file");
        fs::write(file_staging.join("data.txt"), b"existing data")
            .expect("write preexisting staging file");

        copy_dir_recursive(&file_source, &file_staging)
            .expect_err("create_new must reject preexisting file");
        assert_eq!(
            fs::read(file_staging.join("data.txt")).expect("existing file remains"),
            b"existing data"
        );

        fs::remove_dir(&nested_reparse).expect("remove nested reparse point");
        delete_locked_source_tree(
            lock_transfer_source_tree(&directory_staging).expect("lock safe staging tree"),
        )
        .expect("clean safe staging tree");
        assert!(!directory_staging.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn organizer_recovery_scan_excludes_transfer_staging_paths() {
        let root = unique_test_dir("dustdesk-staging-scan");
        let category = root.join("documents");
        let real_item = category.join("report.txt");
        let staging = category.join(".dustdesk-transfer-s123-0.tmp");
        let source_staging = category.join(".dustdesk-source-transfer-s456-0.tmp");
        fs::create_dir_all(&staging).expect("create staging dir");
        fs::create_dir_all(&source_staging).expect("create source staging dir");
        fs::write(&real_item, b"report").expect("write real item");
        fs::write(staging.join("partial.txt"), b"partial").expect("write staging item");
        fs::write(source_staging.join("preserved.txt"), b"preserved")
            .expect("write source staging item");

        let items = organizer_contents(&root);

        assert_eq!(items, vec![real_item]);
        assert!(is_transfer_staging_path(&staging));
        assert!(is_source_transfer_staging_path(&source_staging));
        let config = AppConfig {
            desktop_categories: vec![DeskCategory {
                name: "documents".to_owned(),
                item_paths: vec![
                    category.join("report.txt").display().to_string(),
                    staging.display().to_string(),
                    source_staging.display().to_string(),
                ],
                ..DeskCategory::default()
            }],
            ..AppConfig::default()
        };
        assert_eq!(
            restore_candidate_count(&config, &root, &root.join("desktop")),
            1
        );
        let _ = fs::remove_dir_all(root);
    }

    #[derive(Default)]
    struct MemoryRestartPersistence {
        marker: RefCell<Option<DesktopOrganizationRestartMarker>>,
        saves: RefCell<Vec<DesktopOrganizationRestartMarker>>,
        source_states_at_save: RefCell<Vec<Vec<bool>>>,
        save_calls: Cell<usize>,
        fail_on_save: Cell<Option<usize>>,
        remove_calls: Cell<usize>,
    }

    impl MemoryRestartPersistence {
        fn with_marker(marker: DesktopOrganizationRestartMarker) -> Self {
            Self {
                marker: RefCell::new(Some(marker)),
                ..Self::default()
            }
        }

        fn current(&self) -> Option<DesktopOrganizationRestartMarker> {
            self.marker.borrow().clone()
        }

        fn failing_on_save(save_number: usize) -> Self {
            let persistence = Self::default();
            persistence.fail_on_save.set(Some(save_number));
            persistence
        }
    }

    impl DesktopOrganizationRestartPersistence for MemoryRestartPersistence {
        fn load(&self) -> Result<Option<DesktopOrganizationRestartMarker>, String> {
            Ok(self.current())
        }

        fn save(&self, marker: &DesktopOrganizationRestartMarker) -> Result<(), String> {
            let save_number = self.save_calls.get() + 1;
            self.save_calls.set(save_number);
            if self.fail_on_save.get() == Some(save_number) {
                return Err(format!("injected marker save failure {save_number}"));
            }
            let source_states = marker
                .categories
                .iter()
                .flat_map(|category| &category.items)
                .map(|item| Path::new(&item.source_path).exists())
                .collect();
            self.source_states_at_save.borrow_mut().push(source_states);
            self.saves.borrow_mut().push(marker.clone());
            self.marker.replace(Some(marker.clone()));
            Ok(())
        }

        fn remove(&self) -> Result<(), String> {
            self.remove_calls.set(self.remove_calls.get() + 1);
            self.marker.replace(None);
            Ok(())
        }
    }

    fn lifecycle_test_store(root: &Path) -> (AppStore, PathBuf, PathBuf) {
        let data = root.join("data");
        let organizer = root.join("organizer");
        let launchers = root.join("launchers");
        let desktop = root.join("desktop");
        for path in [&data, &organizer, &launchers, &desktop] {
            fs::create_dir_all(path).expect("create lifecycle test directory");
        }
        (
            AppStore::for_test(data, organizer.clone(), launchers),
            organizer,
            desktop,
        )
    }

    fn run_startup_recollect(
        store: &AppStore,
        desktop: &Path,
        persistence: &MemoryRestartPersistence,
    ) -> Result<usize, String> {
        let marker = persistence.load()?;
        recollect_desktop_organization_from_restart_marker_core(store, desktop, marker, persistence)
    }

    #[cfg(windows)]
    fn create_test_directory_reparse_point(link: &Path, target: &Path) {
        if std::os::windows::fs::symlink_dir(target, link).is_ok() {
            return;
        }

        let output = Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .expect("create directory junction");
        assert!(
            output.status.success(),
            "unable to create test reparse point: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn unique_test_dir(prefix: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        env::temp_dir().join(format!("{prefix}-{}-{nonce}", std::process::id()))
    }
}

fn clipboard_preview(
    mut items: Vec<ClipboardHistoryItem>,
    limit: usize,
) -> Vec<ClipboardHistoryItem> {
    items.truncate(limit);
    for item in &mut items {
        item.text = truncate_chars(&item.text, 500);
        if item.kind == models::ClipboardHistoryKind::Image && item.text.trim().is_empty() {
            item.text = "[图片剪贴内容]".to_owned();
        }
        if item.kind == models::ClipboardHistoryKind::Image {
            item.image_png_base64 = image_preview_base64(item);
        } else {
            item.image_png_base64.clear();
        }
    }
    items
}

fn image_preview_base64(item: &ClipboardHistoryItem) -> String {
    read_bounded_file_base64(&item.image_thumb_path)
        .or_else(|| bounded_inline_base64(&item.image_png_base64))
        .unwrap_or_default()
}

fn read_bounded_file_base64(path: &str) -> Option<String> {
    let path = path.trim();
    if path.is_empty() {
        return None;
    }

    let metadata = fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > CLIPBOARD_PREVIEW_IMAGE_MAX_BYTES {
        return None;
    }

    fs::read(path).ok().map(|bytes| BASE64.encode(bytes))
}

fn bounded_inline_base64(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }

    let estimated_bytes = ((value.len() as u64) * 3) / 4;
    if estimated_bytes > CLIPBOARD_PREVIEW_IMAGE_MAX_BYTES {
        return None;
    }

    Some(value.to_owned())
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    let mut output = String::new();
    for (index, ch) in value.chars().enumerate() {
        if index >= max_chars {
            output.push_str("...");
            return output;
        }
        output.push(ch);
    }
    output
}

fn desktop_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(path) = user_desktop() {
        push_unique_path(&mut roots, path);
    }
    if let Some(path) = public_desktop() {
        push_unique_path(&mut roots, path);
    }
    if let Some(public) = env::var_os("PUBLIC") {
        push_unique_path(&mut roots, PathBuf::from(public).join("Desktop"));
    }
    roots
}

fn user_desktop() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        if let Some(path) =
            windows_known_folder_path(&windows_sys::Win32::UI::Shell::FOLDERID_Desktop)
        {
            return Some(path);
        }
    }

    env::var_os("USERPROFILE").map(|home| PathBuf::from(home).join("Desktop"))
}

fn public_desktop() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        if let Some(path) =
            windows_known_folder_path(&windows_sys::Win32::UI::Shell::FOLDERID_PublicDesktop)
        {
            return Some(path);
        }
    }

    env::var_os("PUBLIC").map(|public| PathBuf::from(public).join("Desktop"))
}

fn push_unique_path(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if paths
        .iter()
        .any(|existing| normalize_path_for_compare(existing) == normalize_path_for_compare(&path))
    {
        return;
    }
    paths.push(path);
}

#[cfg(windows)]
fn windows_known_folder_path(folder_id: &windows_sys::core::GUID) -> Option<PathBuf> {
    use std::{ffi::OsString, os::windows::ffi::OsStringExt, ptr::null_mut};

    use windows_sys::Win32::{
        System::Com::CoTaskMemFree,
        UI::Shell::{SHGetKnownFolderPath, KF_FLAG_DEFAULT},
    };

    let mut raw = null_mut();
    let result =
        unsafe { SHGetKnownFolderPath(folder_id, KF_FLAG_DEFAULT as u32, null_mut(), &mut raw) };
    if result < 0 || raw.is_null() {
        return None;
    }

    let mut len = 0usize;
    unsafe {
        while *raw.add(len) != 0 {
            len += 1;
        }
    }
    let slice = unsafe { std::slice::from_raw_parts(raw, len) };
    let path = OsString::from_wide(slice);
    unsafe {
        CoTaskMemFree(raw.cast());
    }
    Some(PathBuf::from(path))
}

fn open_path_impl(path: &Path) -> Result<(), String> {
    Command::new("explorer")
        .arg(path)
        .spawn()
        .map(|_| ())
        .map_err(to_message)
}

fn show_path_in_folder_impl(path: &Path) -> Result<(), String> {
    if path.exists() && path.is_file() {
        Command::new("explorer")
            .arg(format!("/select,{}", path.display()))
            .spawn()
            .map(|_| ())
            .map_err(to_message)
    } else if path.exists() && path.is_dir() {
        open_path_impl(path)
    } else if let Some(parent) = path.parent().filter(|parent| parent.exists()) {
        open_path_impl(parent)
    } else {
        Err("路径不存在".to_owned())
    }
}

fn open_with_shell(path: &str) -> Result<(), String> {
    Command::new("cmd")
        .args(["/C", "start", "", path])
        .spawn()
        .map(|_| ())
        .map_err(to_message)
}

#[cfg(windows)]
fn open_path_as_administrator_impl(path: &Path) -> Result<(), String> {
    shell_execute_as_administrator(path, None)
}

#[cfg(not(windows))]
fn open_path_as_administrator_impl(path: &Path) -> Result<(), String> {
    open_with_shell(&path.display().to_string())
}

#[cfg(windows)]
fn open_paths_as_administrator(paths: &[PathBuf]) -> Result<(), String> {
    if let [path] = paths {
        return open_path_as_administrator_impl(path);
    }

    let script_root = env::temp_dir().join("DustDesk").join("admin-launch");
    fs::create_dir_all(&script_root).map_err(to_message)?;
    let created_at = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let script_path = script_root.join(format!("launch-{}-{created_at}.ps1", std::process::id()));
    let items = paths
        .iter()
        .map(|path| format!("  {}", ps_quote(&path.display().to_string())))
        .collect::<Vec<_>>()
        .join(",\r\n");
    let script = format!(
        "$ErrorActionPreference = 'Continue'\r\n\
         $items = @(\r\n{items}\r\n)\r\n\
         try {{\r\n\
           foreach ($item in $items) {{\r\n\
             try {{ Start-Process -FilePath $item -ErrorAction Stop | Out-Null }} catch {{ }}\r\n\
           }}\r\n\
         }} finally {{\r\n\
           Remove-Item -LiteralPath $PSCommandPath -Force -ErrorAction SilentlyContinue\r\n\
         }}\r\n"
    );
    let mut encoded_script = vec![0xef, 0xbb, 0xbf];
    encoded_script.extend_from_slice(script.as_bytes());
    fs::write(&script_path, encoded_script).map_err(to_message)?;

    let parameters = format!(
        "-NoProfile -NonInteractive -ExecutionPolicy Bypass -File \"{}\"",
        script_path.display()
    );
    if let Err(error) =
        shell_execute_as_administrator(Path::new("powershell.exe"), Some(&parameters))
    {
        let _ = fs::remove_file(&script_path);
        return Err(error);
    }
    Ok(())
}

#[cfg(not(windows))]
fn open_paths_as_administrator(paths: &[PathBuf]) -> Result<(), String> {
    for path in paths {
        open_path_as_administrator_impl(path)?;
    }
    Ok(())
}

#[cfg(windows)]
fn shell_execute_as_administrator(path: &Path, parameters: Option<&str>) -> Result<(), String> {
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL};

    let operation = wide_null("runas");
    let file = wide_null(&path.display().to_string());
    let parameters = parameters.map(wide_null);
    let parameters_ptr = parameters
        .as_ref()
        .map_or(null(), |parameters| parameters.as_ptr());
    let result = unsafe {
        ShellExecuteW(
            null_mut(),
            operation.as_ptr(),
            file.as_ptr(),
            parameters_ptr,
            null(),
            SW_SHOWNORMAL,
        )
    } as isize;

    if result > 32 {
        Ok(())
    } else if result == 5 {
        Err("已取消管理员授权".to_owned())
    } else {
        Err(format!("管理员启动失败，Windows 错误码：{result}"))
    }
}

fn set_launch_on_startup_entry(enabled: bool) -> Result<(), String> {
    remove_legacy_startup_shortcut();
    set_launch_on_startup_run_entry(enabled)
}

#[cfg(windows)]
fn set_launch_on_startup_run_entry(enabled: bool) -> Result<(), String> {
    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const VALUE_NAME: &str = "DeskNest";

    use std::ptr::null_mut;

    use windows_sys::Win32::{
        Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS},
        System::Registry::{
            RegCloseKey, RegCreateKeyW, RegDeleteValueW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
            REG_SZ,
        },
    };

    let mut key: HKEY = null_mut();
    let run_key = wide_null(RUN_KEY);
    let status = unsafe { RegCreateKeyW(HKEY_CURRENT_USER, run_key.as_ptr(), &mut key) };
    if status != ERROR_SUCCESS {
        return Err(format!("打开开机自启注册表失败：{status}"));
    }

    let value_name = wide_null(VALUE_NAME);
    let result = if enabled {
        let exe = env::current_exe().map_err(to_message)?;
        let command = format!("\"{}\" --startup", exe.display());
        let command = wide_null(&command);
        unsafe {
            RegSetValueExW(
                key,
                value_name.as_ptr(),
                0,
                REG_SZ,
                command.as_ptr().cast::<u8>(),
                (command.len() * std::mem::size_of::<u16>()) as u32,
            )
        }
    } else {
        let status = unsafe { RegDeleteValueW(key, value_name.as_ptr()) };
        if status == ERROR_FILE_NOT_FOUND {
            ERROR_SUCCESS
        } else {
            status
        }
    };

    unsafe {
        RegCloseKey(key);
    }

    if result != ERROR_SUCCESS {
        return Err(format!("更新开机自启注册表失败：{result}"));
    }
    Ok(())
}

#[cfg(windows)]
fn wide_null(value: &str) -> Vec<u16> {
    use std::{ffi::OsStr, os::windows::ffi::OsStrExt};

    OsStr::new(value)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(not(windows))]
fn set_launch_on_startup_run_entry(enabled: bool) -> Result<(), String> {
    if enabled {
        let exe = env::current_exe().map_err(to_message)?;
        let shortcut = startup_shortcut_path()?;
        create_windows_shortcut(&shortcut, &exe, "DeskNest 开机自启")?;
    } else {
        remove_legacy_startup_shortcut();
    }
    Ok(())
}

fn remove_legacy_startup_shortcut() {
    let Ok(shortcut) = startup_shortcut_path() else {
        return;
    };
    if shortcut.exists() {
        let _ = fs::remove_file(shortcut);
    }
}

fn startup_shortcut_path() -> Result<PathBuf, String> {
    let app_data = env::var_os("APPDATA").ok_or_else(|| "没有找到 APPDATA 目录".to_owned())?;
    Ok(PathBuf::from(app_data)
        .join("Microsoft")
        .join("Windows")
        .join("Start Menu")
        .join("Programs")
        .join("Startup")
        .join("DeskNest.lnk"))
}

fn create_windows_shortcut(
    shortcut: &Path,
    target: &Path,
    description: &str,
) -> Result<(), String> {
    if let Some(parent) = shortcut.parent() {
        fs::create_dir_all(parent).map_err(to_message)?;
    }
    let working_dir = target.parent().unwrap_or(target).display().to_string();
    let shortcut = ps_quote(&shortcut.display().to_string());
    let target = ps_quote(&target.display().to_string());
    let working_dir = ps_quote(&working_dir);
    let description = ps_quote(description);
    let script = format!(
        "$shell = New-Object -ComObject WScript.Shell; \
         $shortcut = $shell.CreateShortcut({shortcut}); \
         $shortcut.TargetPath = {target}; \
         $shortcut.WorkingDirectory = {working_dir}; \
         $shortcut.Description = {description}; \
         $shortcut.Save()"
    );

    let status = Command::new("powershell")
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &script,
        ])
        .status()
        .map_err(to_message)?;

    if status.success() {
        Ok(())
    } else {
        Err("创建桌面入口失败".to_owned())
    }
}

fn ps_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn display_path_name(path: &Path) -> String {
    path.file_stem()
        .or_else(|| path.file_name())
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string())
}

fn path_extension(path: &Path) -> String {
    path.extension()
        .map(|name| name.to_string_lossy().to_uppercase())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "FILE".to_owned())
        .chars()
        .take(8)
        .collect()
}

fn now_local_string() -> String {
    chrono::Local::now()
        .format("%Y-%m-%dT%H:%M:%S%.3f")
        .to_string()
}

fn now_id() -> String {
    let now = chrono::Local::now();
    format!("s{:x}{:x}", now.timestamp_millis(), std::process::id())
}

fn current_exe_diagnostic_value() -> String {
    env::current_exe()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|error| format!("<无法读取：{error}>"))
}

fn diagnostic_value(value: &str) -> String {
    value.replace(['\r', '\n', '\t'], " ").replace('"', "'")
}

#[cfg(not(test))]
fn desktop_diagnostic_log_path() -> Option<PathBuf> {
    env::var_os("APPDATA").map(|app_data| {
        PathBuf::from(app_data)
            .join("DustDesk")
            .join("logs")
            .join("desktop-operations.log")
    })
}

#[cfg(not(test))]
fn append_desktop_diagnostic(event: &str, details: &str) {
    let Some(path) = desktop_diagnostic_log_path() else {
        return;
    };
    let lock = DIAGNOSTIC_LOG_LOCK.get_or_init(|| Mutex::new(()));
    let _guard = match lock.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    let Some(parent) = path.parent() else {
        return;
    };
    if fs::create_dir_all(parent).is_err() {
        return;
    }

    if fs::metadata(&path).is_ok_and(|metadata| metadata.len() >= DIAGNOSTIC_LOG_MAX_BYTES) {
        let rotated = path.with_extension("log.1");
        let _ = fs::remove_file(&rotated);
        let _ = fs::rename(&path, rotated);
    }

    let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(path) else {
        return;
    };
    let _ = writeln!(
        file,
        "{} event={} {}",
        chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%.3f%:z"),
        diagnostic_value(event),
        diagnostic_value(details)
    );
}

#[cfg(test)]
fn append_desktop_diagnostic(_event: &str, _details: &str) {}

fn install_persistent_panic_hook() {
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let thread = std::thread::current();
        let thread_name = thread.name().unwrap_or("未命名线程");
        append_desktop_diagnostic(
            "panic",
            &format!(
                "thread=\"{}\" current_exe=\"{}\" message=\"{}\"",
                diagnostic_value(thread_name),
                diagnostic_value(&current_exe_diagnostic_value()),
                diagnostic_value(&panic_info.to_string())
            ),
        );
        previous_hook(panic_info);
    }));
}

fn normalize_name(name: &str, fallback_index: usize) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        format!("新分类 {fallback_index}")
    } else {
        trimmed.to_owned()
    }
}

fn to_message(error: impl std::fmt::Display) -> String {
    error.to_string()
}

fn normalize_shortcut_input(shortcut: &str) -> Result<String, String> {
    let parts = shortcut
        .split('+')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(normalize_shortcut_token)
        .collect::<Vec<_>>();

    if parts.len() < 2 {
        return Err("快捷键至少需要一个修饰键和一个主键，例如 Ctrl+Tab".to_owned());
    }

    let key = parts.last().expect("shortcut parts cannot be empty");
    if is_shortcut_modifier(key) {
        return Err("快捷键需要一个非修饰键作为主键".to_owned());
    }

    if !parts[..parts.len() - 1]
        .iter()
        .any(|part| is_shortcut_modifier(part))
    {
        return Err("快捷键必须包含 Ctrl / Alt / Shift / Super 中的一个修饰键".to_owned());
    }

    let normalized = parts.join("+");
    normalized
        .parse::<Shortcut>()
        .map_err(|error| format!("快捷键格式不可用：{error}"))?;
    Ok(normalized)
}

fn normalize_shortcut_token(token: &str) -> String {
    match token.to_ascii_uppercase().as_str() {
        "CONTROL" | "CTRL" => "Ctrl".to_owned(),
        "OPTION" | "ALT" => "Alt".to_owned(),
        "SHIFT" => "Shift".to_owned(),
        "COMMAND" | "CMD" | "SUPER" | "META" | "WIN" | "WINDOWS" => "Super".to_owned(),
        "ESC" => "Escape".to_owned(),
        "SPACEBAR" => "Space".to_owned(),
        upper if upper.len() == 1 => upper.to_owned(),
        _ => token.trim().to_owned(),
    }
}

fn is_shortcut_modifier(token: &str) -> bool {
    matches!(token, "Ctrl" | "Alt" | "Shift" | "Super")
}

fn with_lazy_window_operation<T>(
    operation: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let _guard = LAZY_WINDOW_OPERATION_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| "窗口操作锁已损坏".to_owned())?;
    operation()
}

fn with_webview_window_creation<T>(
    operation: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let _guard = WEBVIEW_WINDOW_CREATION_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| "WebView 创建锁已损坏".to_owned())?;
    operation()
}

fn is_clipboard_overlay_visible(app: &tauri::AppHandle) -> bool {
    app.get_webview_window("clipboard")
        .and_then(|window| window.is_visible().ok())
        .unwrap_or(false)
}

fn replace_global_shortcut(
    app: &tauri::AppHandle,
    previous: Option<&str>,
    next: Option<&str>,
) -> Result<(), String> {
    let shortcuts = app.global_shortcut();
    let previous = previous.filter(|shortcut| !shortcut.trim().is_empty());
    let next = next.filter(|shortcut| !shortcut.trim().is_empty());

    if previous == next {
        if let Some(next) = next {
            if !shortcuts.is_registered(next) {
                shortcuts
                    .register(next)
                    .map_err(|error| format!("无法注册快捷键 {next}: {error}"))?;
            }
        }
        return Ok(());
    }

    if let Some(previous) = previous {
        if shortcuts.is_registered(previous) {
            shortcuts
                .unregister(previous)
                .map_err(|error| format!("无法取消旧快捷键 {previous}: {error}"))?;
        }
    }

    if let Some(next) = next {
        if let Err(error) = shortcuts.register(next) {
            if let Some(previous) = previous {
                if !shortcuts.is_registered(previous) {
                    let _ = shortcuts.register(previous);
                }
            }
            return Err(format!("无法注册快捷键 {next}: {error}"));
        }
    }

    Ok(())
}

fn configured_settings() -> AppSettings {
    AppStore::open()
        .map(|store| store.load_config().settings)
        .unwrap_or_default()
}

fn sync_launch_on_startup_setting(enabled: bool) {
    if let Err(error) = set_launch_on_startup_entry(enabled) {
        eprintln!("failed to sync launch on startup entry: {error}");
    }
}

fn initialize_runtime_services(app: &tauri::AppHandle) {
    clipboard_bridge::spawn_text_history_monitor();
    let settings = configured_settings();
    let clipboard_shortcut = settings.clipboard_shortcut_value();
    if let Err(error) = app.global_shortcut().register(clipboard_shortcut.as_str()) {
        eprintln!("failed to register {clipboard_shortcut} shortcut: {error}");
    }

    if settings.search_enabled {
        let search_shortcut = settings.search_shortcut_value();
        if search_shortcut != clipboard_shortcut {
            if let Err(error) = app.global_shortcut().register(search_shortcut.as_str()) {
                eprintln!("failed to register {search_shortcut} shortcut: {error}");
            }
        }
    }

    let launch_on_startup = settings.launch_on_startup;
    tauri::async_runtime::spawn_blocking(move || sync_launch_on_startup_setting(launch_on_startup));
}

fn is_startup_launch_invocation() -> bool {
    env::args().any(|arg| arg == "--startup")
}

fn shortcut_matches(shortcut: &Shortcut, configured: &str) -> bool {
    configured
        .parse::<Shortcut>()
        .map(|expected| expected.id() == shortcut.id())
        .unwrap_or(false)
}

fn create_main_window(app: &tauri::AppHandle) -> Result<WebviewWindow, String> {
    with_webview_window_creation(|| {
        WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
            .title("DeskNest")
            .inner_size(1320.0, 820.0)
            .min_inner_size(1080.0, 680.0)
            .resizable(true)
            .center()
            .decorations(false)
            .transparent(false)
            .visible(true)
            .build()
            .map_err(to_message)
    })
}

fn create_clipboard_overlay(app: &tauri::AppHandle) -> Result<WebviewWindow, String> {
    with_webview_window_creation(|| {
        WebviewWindowBuilder::new(
            app,
            "clipboard",
            WebviewUrl::App("index.html#/clipboard-overlay".into()),
        )
        .title("DeskNest Clipboard")
        .inner_size(1600.0, 300.0)
        .position(-32_000.0, -32_000.0)
        .resizable(false)
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .focused(false)
        .transparent(false)
        .visible(true)
        .build()
        .map_err(to_message)
    })
}

fn create_search_overlay(app: &tauri::AppHandle) -> Result<WebviewWindow, String> {
    with_webview_window_creation(|| {
        WebviewWindowBuilder::new(
            app,
            "search",
            WebviewUrl::App("index.html#/search-overlay".into()),
        )
        .title("DeskNest Search")
        .inner_size(860.0, 620.0)
        .position(-32_000.0, -32_000.0)
        .resizable(false)
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .focused(false)
        .transparent(false)
        .visible(true)
        .build()
        .map_err(to_message)
    })
}

fn show_clipboard_overlay_impl(app: &tauri::AppHandle) -> Result<(), String> {
    let window = match app.get_webview_window("clipboard") {
        Some(window) => window,
        None => create_clipboard_overlay(app)?,
    };
    place_clipboard_overlay(&window).map_err(to_message)?;
    window.show().map_err(to_message)?;
    let _ = window.set_focus();
    let _ = window.emit("dustdesk://clipboard-shortcut", ());
    Ok(())
}

fn show_search_overlay_impl(app: &tauri::AppHandle) -> Result<(), String> {
    let window = match app.get_webview_window("search") {
        Some(window) => window,
        None => create_search_overlay(app)?,
    };
    place_search_overlay(&window).map_err(to_message)?;
    window.show().map_err(to_message)?;
    let _ = window.set_focus();
    let _ = window.emit("dustdesk://search-shortcut", ());
    Ok(())
}

fn show_clipboard_overlay_async(app: tauri::AppHandle, sync_clipboard: bool) {
    tauri::async_runtime::spawn_blocking(move || {
        if sync_clipboard {
            clipboard_bridge::sync_current_clipboard_once();
        }
        let result = with_lazy_window_operation(|| show_clipboard_overlay_impl(&app));
        if let Err(error) = result {
            eprintln!("failed to show clipboard overlay: {error}");
        }
    });
}

fn show_search_overlay_async(app: tauri::AppHandle) {
    tauri::async_runtime::spawn_blocking(move || {
        if let Err(error) = with_lazy_window_operation(|| show_search_overlay_impl(&app)) {
            eprintln!("failed to show search overlay: {error}");
        }
    });
}

fn hide_search_overlay_impl(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("search") {
        let _ = window.hide();
    }
}

fn hide_clipboard_overlay_impl(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("clipboard") {
        let _ = window.hide();
    }
}

fn emit_desktop_cards_changed(app: &tauri::AppHandle) {
    let _ = app.emit("dustdesk://desktop-cards-changed", ());
}

fn start_desktop_background_operation(
    app: tauri::AppHandle,
    kind: &'static str,
    operation: impl FnOnce() -> Result<DesktopOperationPayload, String> + Send + 'static,
) -> Result<(), String> {
    if DESKTOP_OPERATION_RUNNING.swap(true, Ordering::SeqCst) {
        return Err("已有桌面整理任务正在执行，请稍后再试".to_owned());
    }

    let started_payload = DesktopOperationPayload {
        kind,
        scope: "manual",
        status: "started",
        message: match kind {
            "classify" => "正在智能收纳桌面...".to_owned(),
            "restore" => "正在还原桌面...".to_owned(),
            _ => "正在处理桌面项目...".to_owned(),
        },
        moved: 0,
        skipped: 0,
        restored: 0,
        total: 0,
        current_path: String::new(),
        category_counts: Vec::new(),
    };
    set_last_desktop_operation(started_payload.clone());
    emit_desktop_operation(&app, started_payload);

    tauri::async_runtime::spawn_blocking(move || {
        let payload = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(operation)) {
            Ok(Ok(payload)) => payload,
            Ok(Err(error)) => DesktopOperationPayload {
                kind,
                scope: "manual",
                status: "failed",
                message: error,
                moved: 0,
                skipped: 0,
                restored: 0,
                total: 0,
                current_path: String::new(),
                category_counts: Vec::new(),
            },
            Err(_) => DesktopOperationPayload {
                kind,
                scope: "manual",
                status: "failed",
                message: "桌面整理任务异常中断，请刷新后重试".to_owned(),
                moved: 0,
                skipped: 0,
                restored: 0,
                total: 0,
                current_path: String::new(),
                category_counts: Vec::new(),
            },
        };
        DESKTOP_OPERATION_RUNNING.store(false, Ordering::SeqCst);
        set_last_desktop_operation(payload.clone());
        emit_desktop_operation(&app, payload);
        emit_desktop_cards_changed(&app);
    });

    Ok(())
}

#[tauri::command]
fn desktop_operation_status() -> DesktopOperationStatus {
    DesktopOperationStatus {
        running: DESKTOP_OPERATION_RUNNING.load(Ordering::SeqCst),
        last: DESKTOP_OPERATION_LAST
            .get_or_init(|| Mutex::new(None))
            .lock()
            .ok()
            .and_then(|payload| payload.clone()),
    }
}

fn set_last_desktop_operation(payload: DesktopOperationPayload) {
    if let Ok(mut last) = DESKTOP_OPERATION_LAST
        .get_or_init(|| Mutex::new(None))
        .lock()
    {
        *last = Some(payload);
    }
}

fn emit_desktop_operation(app: &tauri::AppHandle, payload: DesktopOperationPayload) {
    let _ = app.emit(DESKTOP_OPERATION_EVENT, payload);
}

fn publish_lifecycle_desktop_operation(
    app: &tauri::AppHandle,
    payload: DesktopOperationPayload,
    running: bool,
) {
    DESKTOP_OPERATION_RUNNING.store(running, Ordering::SeqCst);
    set_last_desktop_operation(payload.clone());
    emit_desktop_operation(app, payload);
}

fn wait_for_desktop_operation_idle() {
    let started_at = Instant::now();
    while DESKTOP_OPERATION_RUNNING.load(Ordering::SeqCst) {
        if started_at.elapsed() >= Duration::from_secs(120) {
            eprintln!(
                "desktop operation status remained busy for 120 seconds; continuing exit recovery"
            );
            break;
        }
        std::thread::sleep(Duration::from_millis(40));
    }
}

struct DesktopOperationRunningResetGuard;

impl Drop for DesktopOperationRunningResetGuard {
    fn drop(&mut self) {
        DESKTOP_OPERATION_RUNNING.store(false, Ordering::SeqCst);
    }
}

fn keep_loading_effect_visible(started_at: Instant, minimum: Duration) {
    if let Some(remaining) = minimum.checked_sub(started_at.elapsed()) {
        std::thread::sleep(remaining);
    }
}

fn restart_recollect_progress_message(current: usize, total: usize, path: &Path) -> String {
    let name = display_path_name(path);
    if total > 0 {
        format!("正在恢复收纳分类 {current}/{total}：{name}")
    } else {
        format!("正在恢复收纳分类：{name}")
    }
}

fn place_clipboard_overlay(window: &WebviewWindow) -> tauri::Result<()> {
    let Some(monitor) = window.current_monitor()?.or(window.primary_monitor()?) else {
        return Ok(());
    };

    let work_area = monitor.work_area();
    let margin = 18i32;
    let panel_height = 300u32;
    let panel_width = work_area
        .size
        .width
        .saturating_sub((margin * 2) as u32)
        .max(720);
    let x = work_area.position.x + margin;
    let y = work_area.position.y + work_area.size.height as i32 - panel_height as i32 - margin;

    window.set_size(Size::Physical(PhysicalSize::new(panel_width, panel_height)))?;
    window.set_position(Position::Physical(PhysicalPosition::new(x, y)))?;
    Ok(())
}

fn place_search_overlay(window: &WebviewWindow) -> tauri::Result<()> {
    let Some(monitor) = window.current_monitor()?.or(window.primary_monitor()?) else {
        return Ok(());
    };

    let work_area = monitor.work_area();
    let panel_width = work_area.size.width.min(860).max(640);
    let panel_height = work_area.size.height.min(620).max(460);
    let x = work_area.position.x + ((work_area.size.width.saturating_sub(panel_width)) / 2) as i32;
    let y =
        work_area.position.y + ((work_area.size.height.saturating_sub(panel_height)) / 3) as i32;

    window.set_size(Size::Physical(PhysicalSize::new(panel_width, panel_height)))?;
    window.set_position(Position::Physical(PhysicalPosition::new(x, y)))?;
    Ok(())
}

fn place_desktop_widget(
    window: &WebviewWindow,
    label: &str,
    target: &DesktopMonitorTarget,
) -> tauri::Result<()> {
    if apply_saved_desktop_window_layout_to_monitor(window, label, &target.work_area)
        .map_err(|error| tauri::Error::Anyhow(anyhow::anyhow!(error)))?
    {
        return Ok(());
    }

    let work_area = target.monitor.work_area();
    let target_scale = valid_scale_factor(target.monitor.scale_factor());
    let source_scale = valid_scale_factor(window.scale_factor()?);
    let margin = scale_i32(28, target_scale);
    let min_width = scale_u32(520, target_scale).min(work_area.size.width);
    let min_height = scale_u32(320, target_scale).min(work_area.size.height);
    let default_panel_width = scale_u32(760, target_scale)
        .min(work_area.size.width)
        .max(min_width);
    let default_panel_height = scale_u32(520, target_scale)
        .min(work_area.size.height)
        .max(min_height);
    let current_size = window.inner_size()?;
    let current_width = scale_u32(current_size.width, target_scale / source_scale);
    let current_height = scale_u32(current_size.height, target_scale / source_scale);
    let max_panel_width = work_area
        .size
        .width
        .saturating_sub((margin * 2) as u32)
        .max(min_width);
    let max_panel_height = work_area
        .size
        .height
        .saturating_sub((margin * 2) as u32)
        .max(min_height);
    let panel_width = if current_width >= min_width {
        current_width.clamp(min_width, max_panel_width)
    } else {
        default_panel_width.clamp(min_width, max_panel_width)
    };
    let panel_height = if current_height >= min_height {
        current_height.clamp(min_height, max_panel_height)
    } else {
        default_panel_height.clamp(min_height, max_panel_height)
    };
    let desired_x = i64::from(work_area.position.x) + i64::from(work_area.size.width)
        - i64::from(panel_width)
        - i64::from(margin);
    let desired_y = i64::from(work_area.position.y)
        + i64::from(work_area.size.height.saturating_sub(panel_height) / 4);
    let placement = fit_desktop_window_to_work_area(
        desired_x,
        desired_y,
        panel_width,
        panel_height,
        min_width,
        min_height,
        &target.work_area,
    );

    window.set_position(Position::Physical(PhysicalPosition::new(
        placement.x,
        placement.y,
    )))?;
    window.set_size(Size::Physical(PhysicalSize::new(
        placement.width,
        placement.height,
    )))?;
    Ok(())
}

fn show_or_create_desktop_widget(
    app: &tauri::AppHandle,
    target: &DesktopMonitorTarget,
) -> Result<(), String> {
    let label = desktop_widget_label(target);
    if let Some(window) = app.get_webview_window(&label) {
        place_desktop_widget(&window, &label, target).map_err(to_message)?;
        return show_desktop_background_window(&window);
    }

    let scale_factor = valid_scale_factor(target.monitor.scale_factor());
    let work_area = target.monitor.work_area();
    let initial_x = work_area.position.x as f64 / scale_factor;
    let initial_y = work_area.position.y as f64 / scale_factor;
    let window = with_webview_window_creation(|| {
        if let Some(window) = app.get_webview_window(&label) {
            return Ok(window);
        }
        WebviewWindowBuilder::new(
            app,
            &label,
            WebviewUrl::App("index.html?dustdeskRoute=desktop-widget".into()),
        )
        .title("DustDesk Desktop Widget")
        .inner_size(720.0, 460.0)
        .min_inner_size(520.0, 320.0)
        .position(initial_x, initial_y)
        .resizable(true)
        .maximizable(false)
        .minimizable(false)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .background_color(Color(0, 0, 0, 0))
        .skip_taskbar(true)
        .always_on_top(false)
        .always_on_bottom(true)
        .focused(false)
        .visible(true)
        .build()
        .map_err(to_message)
    })?;
    place_desktop_widget(&window, &label, target).map_err(to_message)?;
    show_desktop_background_window(&window)
}

fn show_or_create_desktop_card(
    app: &tauri::AppHandle,
    label: &str,
    title: &str,
    url: &str,
    index: usize,
    target: &DesktopMonitorTarget,
) -> Result<(), String> {
    if let Some(window) = app.get_webview_window(label) {
        window.set_title(title).map_err(to_message)?;
        if !apply_saved_desktop_window_layout_to_monitor(&window, label, &target.work_area)? {
            place_desktop_card_on_monitor(&window, label, index, target)?;
        }
        return show_desktop_background_window(&window);
    }

    create_desktop_card(app, label, title, url, index, target)
}

fn desktop_card_default_placement(
    label: &str,
    index: usize,
    target: &DesktopMonitorTarget,
) -> DesktopWindowPlacement {
    let work_area = target.monitor.work_area();
    let scale_factor = valid_scale_factor(target.monitor.scale_factor());
    let logical_width = if is_desktop_launcher_window_label(label) {
        320
    } else {
        360
    };
    let width = scale_u32(logical_width, scale_factor);
    let height = scale_u32(260, scale_factor);
    let gap = scale_u32(16, scale_factor);
    let horizontal_margin = scale_u32(28, scale_factor);
    let top_margin = scale_u32(72, scale_factor);
    let available_width = work_area
        .size
        .width
        .saturating_sub(horizontal_margin.saturating_mul(2));
    let columns =
        ((available_width.saturating_add(gap)) / width.saturating_add(gap)).max(1) as usize;
    let column = index % columns;
    let row = index / columns;
    let desired_x = work_area.position.x as i64
        + horizontal_margin as i64
        + width.saturating_add(gap) as i64 * column as i64;
    let desired_y = work_area.position.y as i64
        + top_margin as i64
        + height.saturating_add(gap) as i64 * row as i64;

    fit_desktop_window_to_work_area(
        desired_x,
        desired_y,
        width,
        height,
        scale_u32(240, scale_factor),
        scale_u32(160, scale_factor),
        &target.work_area,
    )
}

fn place_desktop_card_on_monitor(
    window: &WebviewWindow,
    label: &str,
    index: usize,
    target: &DesktopMonitorTarget,
) -> Result<(), String> {
    let scale_factor = valid_scale_factor(target.monitor.scale_factor());
    let placement = desktop_card_default_placement(label, index, target);
    let _ = window.set_min_size(Some(Size::Physical(PhysicalSize::new(
        scale_u32(240, scale_factor),
        scale_u32(160, scale_factor),
    ))));
    window
        .set_position(Position::Physical(PhysicalPosition::new(
            placement.x,
            placement.y,
        )))
        .map_err(to_message)?;
    window
        .set_size(Size::Physical(PhysicalSize::new(
            placement.width,
            placement.height,
        )))
        .map_err(to_message)
}

fn keep_desktop_window_behind_apps(window: &WebviewWindow) {
    let _ = window.set_skip_taskbar(true);
    #[cfg(windows)]
    {
        windows_show_desktop::configure_desktop_card_window(window);
        if windows_show_desktop::is_active() {
            windows_show_desktop::request_z_order_refresh(window.app_handle());
            return;
        }
    }

    let _ = window.set_always_on_top(false);
    let _ = window.set_always_on_bottom(true);
}

fn keep_desktop_windows_behind_apps(app: &tauri::AppHandle) {
    for window in app.webview_windows().values() {
        if is_desktop_card_window_label(window.label()) {
            keep_desktop_window_behind_apps(window);
        }
    }
}

fn settle_desktop_windows_after_launch(app: &tauri::AppHandle) {
    keep_desktop_windows_behind_apps(app);
    DESKTOP_WINDOW_SETTLE_GENERATION.fetch_add(1, Ordering::SeqCst);
    if DESKTOP_WINDOW_SETTLE_SCHEDULED.swap(true, Ordering::SeqCst) {
        return;
    }

    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || 'reschedule: loop {
        let generation = DESKTOP_WINDOW_SETTLE_GENERATION.load(Ordering::SeqCst);
        let mut elapsed = 0;
        for deadline in [150, 500, 1_500] {
            std::thread::sleep(Duration::from_millis(deadline - elapsed));
            if DESKTOP_WINDOW_SETTLE_GENERATION.load(Ordering::SeqCst) != generation {
                continue 'reschedule;
            }
            keep_desktop_windows_behind_apps(&app);
            elapsed = deadline;
        }

        DESKTOP_WINDOW_SETTLE_SCHEDULED.store(false, Ordering::SeqCst);
        if DESKTOP_WINDOW_SETTLE_GENERATION.load(Ordering::SeqCst) == generation {
            return;
        }
        if DESKTOP_WINDOW_SETTLE_SCHEDULED.swap(true, Ordering::SeqCst) {
            return;
        }
    });
}

fn show_desktop_background_window(window: &WebviewWindow) -> Result<(), String> {
    let state = load_desktop_window_state()?;
    apply_desktop_window_state(window, state)?;
    keep_desktop_window_behind_apps(window);
    window.show().map_err(to_message)?;
    keep_desktop_window_behind_apps(window);
    sync_system_tray_menu(window.app_handle());
    Ok(())
}

fn create_desktop_card(
    app: &tauri::AppHandle,
    label: &str,
    title: &str,
    url: &str,
    index: usize,
    target: &DesktopMonitorTarget,
) -> Result<(), String> {
    let scale_factor = valid_scale_factor(target.monitor.scale_factor());
    let logical_width = if is_desktop_launcher_window_label(label) {
        320.0
    } else {
        360.0
    };
    let logical_height = 260.0;
    let placement = desktop_card_default_placement(label, index, target);
    let logical_x = placement.x as f64 / scale_factor;
    let logical_y = placement.y as f64 / scale_factor;
    let window = with_webview_window_creation(|| {
        if let Some(window) = app.get_webview_window(label) {
            return Ok(window);
        }
        WebviewWindowBuilder::new(app, label, WebviewUrl::App(url.into()))
            .title(title)
            .inner_size(logical_width, logical_height)
            .min_inner_size(240.0, 160.0)
            .position(logical_x, logical_y)
            .resizable(true)
            .maximizable(false)
            .minimizable(false)
            .decorations(false)
            .transparent(true)
            .shadow(false)
            .background_color(Color(0, 0, 0, 0))
            .skip_taskbar(true)
            .always_on_top(false)
            .always_on_bottom(true)
            .focused(false)
            .visible(true)
            .build()
            .map_err(to_message)
    })?;
    window
        .set_position(Position::Physical(PhysicalPosition::new(
            placement.x,
            placement.y,
        )))
        .map_err(to_message)?;
    window
        .set_size(Size::Physical(PhysicalSize::new(
            placement.width,
            placement.height,
        )))
        .map_err(to_message)?;
    if !apply_saved_desktop_window_layout_to_monitor(&window, label, &target.work_area)? {
        place_desktop_card_on_monitor(&window, label, index, target)?;
    }
    show_desktop_background_window(&window)
}

fn desktop_card_url(kind: &str, index: Option<usize>) -> String {
    let route = match index {
        Some(index) => format!("desktop-card/{kind}/{index}"),
        None => format!("desktop-card/{kind}"),
    };
    format!("index.html?dustdeskRoute={}", route.replace('/', "%2F"))
}

fn desktop_category_label(index: usize) -> String {
    format!("desktop-category-{index}")
}

fn desktop_widget_label(target: &DesktopMonitorTarget) -> String {
    if target.is_primary {
        "desktop-widget".to_owned()
    } else {
        format!("desktop-widget-monitor-{}", target.stable_id)
    }
}

fn desktop_category_label_for_monitor(index: usize, target: &DesktopMonitorTarget) -> String {
    if target.is_primary {
        desktop_category_label(index)
    } else {
        format!("desktop-category-{index}-monitor-{}", target.stable_id)
    }
}

fn desktop_launcher_label(target: &DesktopMonitorTarget) -> String {
    if target.is_primary {
        "desktop-launcher".to_owned()
    } else {
        format!("desktop-launcher-monitor-{}", target.stable_id)
    }
}

fn show_desktop_category_on_all_monitors(
    app: &tauri::AppHandle,
    index: usize,
    category: &DeskCategory,
) -> Result<(), String> {
    let title = format!("DustDesk {}", category.name);
    let url = desktop_card_url("category", Some(index));
    for target in desktop_monitor_targets(app).map_err(to_message)? {
        let label = desktop_category_label_for_monitor(index, &target);
        show_or_create_desktop_card(app, &label, &title, &url, index, &target)?;
    }
    Ok(())
}

fn show_desktop_launcher(app: &tauri::AppHandle) -> Result<(), String> {
    let store = AppStore::open().map_err(to_message)?;
    store.ensure_runtime_dirs().map_err(to_message)?;
    let index = store.load_config().desktop_categories.len();
    let url = desktop_card_url("launcher", None);
    for target in desktop_monitor_targets(app).map_err(to_message)? {
        let label = desktop_launcher_label(&target);
        show_or_create_desktop_card(app, &label, "DustDesk 快捷启动", &url, index, &target)?;
    }
    Ok(())
}

fn hide_desktop_category_windows(app: &tauri::AppHandle, index: usize) {
    for window in app.webview_windows().values() {
        if desktop_category_index_from_label(window.label()) == Some(index) {
            let _ = window.hide();
        }
    }
    sync_system_tray_menu(app);
}

fn hide_desktop_organizer_windows(app: &tauri::AppHandle) {
    for window in app.webview_windows().values() {
        if is_desktop_organizer_window_label(window.label()) {
            let _ = window.hide();
        }
    }
    sync_system_tray_menu(app);
}

fn hide_desktop_launcher_window(app: &tauri::AppHandle) {
    for window in app.webview_windows().values() {
        if is_desktop_launcher_window_label(window.label()) {
            let _ = window.hide();
        }
    }
    sync_system_tray_menu(app);
}

fn hide_desktop_widget_windows(app: &tauri::AppHandle) {
    for window in app.webview_windows().values() {
        if is_desktop_widget_window_label(window.label()) {
            let _ = window.hide();
        }
    }
    sync_system_tray_menu(app);
}

fn hide_desktop_card_windows(app: &tauri::AppHandle) {
    for window in app.webview_windows().values() {
        if is_desktop_card_window_label(window.label()) {
            let _ = window.hide();
        }
    }
    sync_system_tray_menu(app);
}

fn desktop_category_index_from_label(label: &str) -> Option<usize> {
    label
        .strip_prefix("desktop-category-")?
        .split('-')
        .next()?
        .parse::<usize>()
        .ok()
}

fn desktop_category_monitor_id_from_label(label: &str) -> Option<&str> {
    let index = desktop_category_index_from_label(label)?;
    label
        .strip_prefix(&format!("desktop-category-{index}-monitor-"))
        .filter(|monitor_id| !monitor_id.is_empty())
}

fn primary_desktop_window_label_for(label: &str) -> Option<String> {
    if label
        .strip_prefix("desktop-widget-monitor-")
        .is_some_and(|monitor_id| !monitor_id.is_empty())
    {
        return Some("desktop-widget".to_owned());
    }
    if label
        .strip_prefix("desktop-launcher-monitor-")
        .is_some_and(|monitor_id| !monitor_id.is_empty())
    {
        return Some("desktop-launcher".to_owned());
    }

    let index = desktop_category_index_from_label(label)?;
    desktop_category_monitor_id_from_label(label).map(|_| desktop_category_label(index))
}

fn remap_desktop_category_window_label(
    label: &str,
    from_index: usize,
    to_index: usize,
) -> Option<String> {
    let index = desktop_category_index_from_label(label)?;
    let mapped_index = remap_category_index(index, from_index, to_index);
    if label == desktop_category_label(index) {
        return Some(desktop_category_label(mapped_index));
    }

    desktop_category_monitor_id_from_label(label)
        .map(|monitor_id| format!("desktop-category-{mapped_index}-monitor-{monitor_id}"))
}

fn is_legacy_desktop_category_window_label(label: &str) -> bool {
    let Some(index) = desktop_category_index_from_label(label) else {
        return false;
    };
    label != desktop_category_label(index)
        && desktop_category_monitor_id_from_label(label).is_none()
}

fn is_desktop_widget_window_label(label: &str) -> bool {
    label == "desktop-widget"
        || label
            .strip_prefix("desktop-widget-monitor-")
            .is_some_and(|monitor_id| !monitor_id.is_empty())
}

fn is_desktop_launcher_window_label(label: &str) -> bool {
    label == "desktop-launcher"
        || label
            .strip_prefix("desktop-launcher-monitor-")
            .is_some_and(|monitor_id| !monitor_id.is_empty())
}

fn is_desktop_organizer_window_label(label: &str) -> bool {
    is_desktop_widget_window_label(label) || label.starts_with("desktop-category-")
}

fn is_primary_desktop_window_label(label: &str) -> bool {
    if label == "desktop-widget" || label == "desktop-launcher" {
        return true;
    }
    desktop_category_index_from_label(label)
        .is_some_and(|index| label == desktop_category_label(index))
}

fn secondary_monitor_id_from_desktop_label(label: &str) -> Option<&str> {
    label
        .strip_prefix("desktop-widget-monitor-")
        .or_else(|| label.strip_prefix("desktop-launcher-monitor-"))
        .filter(|monitor_id| !monitor_id.is_empty())
        .or_else(|| desktop_category_monitor_id_from_label(label))
}

fn is_desktop_card_window_label(label: &str) -> bool {
    is_desktop_organizer_window_label(label) || is_desktop_launcher_window_label(label)
}

fn internal_path_drop_target_at_position(
    app: &tauri::AppHandle,
    source_label: &str,
    position: DesktopDropPosition,
) -> Option<InternalPathDropTarget> {
    let point = desktop_drop_position_to_physical_point(position)?;
    let target_label = desktop_card_window_label_at_point(app, source_label, point)?;
    if is_desktop_launcher_window_label(&target_label) {
        return Some(InternalPathDropTarget::Launcher);
    }
    if let Some(index) = desktop_category_index_from_label(&target_label) {
        return Some(InternalPathDropTarget::Category(index));
    }
    if is_desktop_widget_window_label(&target_label) {
        let targets = DESKTOP_DROP_CATEGORY_TARGETS.get_or_init(|| Mutex::new(BTreeMap::new()));
        return targets
            .lock()
            .ok()
            .and_then(|targets| targets.get(&target_label).copied())
            .map(InternalPathDropTarget::Category)
            .or(Some(InternalPathDropTarget::Box));
    }
    Some(InternalPathDropTarget::Box)
}

#[cfg(windows)]
fn desktop_card_window_label_at_point(
    app: &tauri::AppHandle,
    source_label: &str,
    point: (i32, i32),
) -> Option<String> {
    use windows_sys::Win32::{
        Foundation::POINT,
        UI::WindowsAndMessaging::{GetAncestor, IsWindowVisible, WindowFromPoint, GA_ROOT},
    };

    let hit = unsafe {
        let hit = WindowFromPoint(POINT {
            x: point.0,
            y: point.1,
        });
        if hit.is_null() {
            return None;
        }
        GetAncestor(hit, GA_ROOT)
    };
    if hit.is_null() {
        return None;
    }

    if let Some(label) = app.webview_windows().values().find_map(|window| {
        if window.label() == source_label || !is_desktop_card_window_label(window.label()) {
            return None;
        }
        let hwnd = window.hwnd().ok()?.0;
        (hwnd == hit && unsafe { IsWindowVisible(hwnd) } != 0).then(|| window.label().to_owned())
    }) {
        return Some(label);
    }

    // WebView2 may place a transient drag-image/OLE window under the pointer. In that case
    // WindowFromPoint cannot see the underlying Box even though the release point is inside its
    // physical bounds, so use those bounds as the definitive fallback requested by the UI.
    app.webview_windows().values().find_map(|window| {
        if window.label() == source_label
            || !is_desktop_card_window_label(window.label())
            || !window.is_visible().unwrap_or(false)
        {
            return None;
        }
        let position = window.outer_position().ok()?;
        let size = window.outer_size().ok()?;
        point_in_physical_window(point, position, size).then(|| window.label().to_owned())
    })
}

#[cfg(not(windows))]
fn desktop_card_window_label_at_point(
    app: &tauri::AppHandle,
    source_label: &str,
    point: (i32, i32),
) -> Option<String> {
    app.webview_windows().values().find_map(|window| {
        if window.label() == source_label
            || !is_desktop_card_window_label(window.label())
            || !window.is_visible().unwrap_or(false)
        {
            return None;
        }
        let position = window.outer_position().ok()?;
        let size = window.outer_size().ok()?;
        point_in_physical_window(point, position, size).then(|| window.label().to_owned())
    })
}

fn point_in_physical_window(
    point: (i32, i32),
    position: PhysicalPosition<i32>,
    size: PhysicalSize<u32>,
) -> bool {
    let right = i64::from(position.x) + i64::from(size.width);
    let bottom = i64::from(position.y) + i64::from(size.height);
    i64::from(point.0) >= i64::from(position.x)
        && i64::from(point.0) < right
        && i64::from(point.1) >= i64::from(position.y)
        && i64::from(point.1) < bottom
}

fn desktop_frame_visibility_impl(app: &tauri::AppHandle) -> DesktopFrameVisibility {
    let mut organizer = false;
    let mut launcher = false;

    for window in app.webview_windows().values() {
        let visible = window.is_visible().unwrap_or(false);
        if !visible {
            continue;
        }

        let label = window.label();
        if is_desktop_organizer_window_label(label) {
            organizer = true;
        } else if is_desktop_launcher_window_label(label) {
            launcher = true;
        }
    }

    DesktopFrameVisibility {
        organizer,
        launcher,
        any: organizer || launcher,
    }
}

fn load_desktop_window_state() -> Result<DesktopWindowState, String> {
    let store = AppStore::open().map_err(to_message)?;
    let config = store.load_config();
    Ok(DesktopWindowState {
        locked: config.desktop_layout.locked,
        click_through: config.desktop_layout.click_through,
    })
}

fn set_desktop_window_state_impl(
    app: &tauri::AppHandle,
    locked: Option<bool>,
    click_through: Option<bool>,
) -> Result<DesktopWindowState, String> {
    let state = with_config_mutation(|| {
        let store = AppStore::open().map_err(to_message)?;
        let mut config = store.load_config_strict().map_err(to_message)?;

        if let Some(locked) = locked {
            config.desktop_layout.locked = locked;
            if !locked {
                config.desktop_layout.click_through = false;
            }
        }
        if let Some(click_through) = click_through {
            config.desktop_layout.click_through = click_through;
            if click_through {
                config.desktop_layout.locked = true;
            }
        }

        let state = DesktopWindowState {
            locked: config.desktop_layout.locked,
            click_through: config.desktop_layout.click_through,
        };
        store.save_config(&config).map_err(to_message)?;
        Ok(state)
    })?;

    apply_desktop_window_state_to_all(app, state)?;
    sync_system_tray_menu(app);
    let _ = app.emit(DESKTOP_WINDOW_STATE_CHANGED_EVENT, state);
    Ok(state)
}

fn apply_desktop_window_state_to_all(
    app: &tauri::AppHandle,
    state: DesktopWindowState,
) -> Result<(), String> {
    for window in app.webview_windows().values() {
        if is_desktop_card_window_label(window.label()) {
            apply_desktop_window_state(window, state)?;
        }
    }
    Ok(())
}

fn apply_desktop_window_state(
    window: &WebviewWindow,
    state: DesktopWindowState,
) -> Result<(), String> {
    if window.is_maximized().unwrap_or(false) {
        window.unmaximize().map_err(to_message)?;
        let label = window.label().to_owned();
        let _ = apply_saved_desktop_window_layout(window, &label);
    }
    window.set_maximizable(false).map_err(to_message)?;
    window.set_resizable(!state.locked).map_err(to_message)?;
    window
        .set_ignore_cursor_events(state.click_through)
        .map_err(to_message)?;
    Ok(())
}

fn sync_system_tray_menu(app: &tauri::AppHandle) {
    let Some(items) = TRAY_MENU_ITEMS.get() else {
        return;
    };
    let visibility = desktop_frame_visibility_impl(app);
    let state = load_desktop_window_state().unwrap_or(DesktopWindowState {
        locked: false,
        click_through: false,
    });
    let _ = items.desktop_visible.set_checked(visibility.any);
    let _ = items.desktop_locked.set_text(if state.locked {
        "解锁全部桌面框"
    } else {
        "锁定全部桌面框"
    });
    let _ = items.desktop_locked.set_checked(state.locked);
    let _ = items.desktop_click_through.set_checked(state.click_through);
}

fn saved_desktop_window_layout(label: &str) -> Option<DesktopWindowLayout> {
    if !is_desktop_card_window_label(label) {
        return None;
    }

    let store = AppStore::open().ok()?;
    let config = store.load_config();
    let layout = config.desktop_layout.windows.get(label)?.clone();
    is_valid_desktop_window_layout(&layout).then_some(layout)
}

fn primary_desktop_window_layout_template(
    app: &tauri::AppHandle,
    label: &str,
) -> Option<(DesktopWindowLayout, DesktopMonitorWorkArea)> {
    let primary_label = primary_desktop_window_label_for(label)?;
    let primary = desktop_monitor_targets(app)
        .ok()?
        .into_iter()
        .find(|target| target.is_primary)?;
    if let Some(window) = app.get_webview_window(&primary_label) {
        if let Ok(layout) = capture_desktop_window_layout(&window) {
            if is_valid_desktop_window_layout(&layout) {
                return Some((layout, primary.work_area));
            }
        }
    }

    let mut layout = saved_desktop_window_layout(&primary_label)?;
    layout.monitor_name = primary.work_area.name.clone();
    layout
        .monitor_offset_x
        .get_or_insert(layout.x.saturating_sub(primary.work_area.x));
    layout
        .monitor_offset_y
        .get_or_insert(layout.y.saturating_sub(primary.work_area.y));
    layout
        .monitor_scale_factor
        .get_or_insert(primary.work_area.scale_factor);
    Some((layout, primary.work_area))
}

fn valid_scale_factor(scale_factor: f64) -> f64 {
    if scale_factor.is_finite() && scale_factor > 0.0 {
        scale_factor
    } else {
        1.0
    }
}

fn scale_u32(value: u32, scale_factor: f64) -> u32 {
    ((value as f64 * valid_scale_factor(scale_factor)).round()).clamp(1.0, u32::MAX as f64) as u32
}

fn scale_i32(value: i32, scale_factor: f64) -> i32 {
    ((value as f64 * valid_scale_factor(scale_factor)).round())
        .clamp(i32::MIN as f64, i32::MAX as f64) as i32
}

fn desktop_window_min_logical_size(label: &str) -> (u32, u32) {
    if is_desktop_widget_window_label(label) {
        (520, 320)
    } else {
        (240, 160)
    }
}

fn desktop_monitor_work_areas(monitors: &[Monitor]) -> Vec<DesktopMonitorWorkArea> {
    monitors
        .iter()
        .map(DesktopMonitorWorkArea::from_monitor)
        .collect()
}

fn monitor_work_area_index(
    work_areas: &[DesktopMonitorWorkArea],
    monitor: &Monitor,
) -> Option<usize> {
    let target = DesktopMonitorWorkArea::from_monitor(monitor);
    work_areas
        .iter()
        .position(|work_area| match (&work_area.name, &target.name) {
            (Some(left), Some(right)) => left == right,
            _ => {
                work_area.x == target.x
                    && work_area.y == target.y
                    && work_area.width == target.width
                    && work_area.height == target.height
            }
        })
}

fn desktop_window_intersection_area(
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    work_area: &DesktopMonitorWorkArea,
) -> u64 {
    let left = i64::from(x).max(i64::from(work_area.x));
    let top = i64::from(y).max(i64::from(work_area.y));
    let right =
        (i64::from(x) + i64::from(width)).min(i64::from(work_area.x) + i64::from(work_area.width));
    let bottom = (i64::from(y) + i64::from(height))
        .min(i64::from(work_area.y) + i64::from(work_area.height));

    if right <= left || bottom <= top {
        0
    } else {
        (right - left) as u64 * (bottom - top) as u64
    }
}

fn best_intersecting_monitor_index(
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    work_areas: &[DesktopMonitorWorkArea],
) -> Option<usize> {
    work_areas
        .iter()
        .enumerate()
        .map(|(index, work_area)| {
            (
                index,
                desktop_window_intersection_area(x, y, width, height, work_area),
            )
        })
        .filter(|(_, area)| *area > 0)
        .max_by_key(|(_, area)| *area)
        .map(|(index, _)| index)
}

fn select_desktop_monitor_index(
    layout: &DesktopWindowLayout,
    work_areas: &[DesktopMonitorWorkArea],
    primary_index: Option<usize>,
) -> Option<usize> {
    if let Some(monitor_name) = layout.monitor_name.as_deref() {
        if let Some(index) = work_areas
            .iter()
            .position(|work_area| work_area.name.as_deref() == Some(monitor_name))
        {
            return Some(index);
        }
    }

    best_intersecting_monitor_index(layout.x, layout.y, layout.width, layout.height, work_areas)
        .or(primary_index)
        .or_else(|| (!work_areas.is_empty()).then_some(0))
}

fn fit_desktop_window_to_work_area(
    desired_x: i64,
    desired_y: i64,
    width: u32,
    height: u32,
    min_width: u32,
    min_height: u32,
    work_area: &DesktopMonitorWorkArea,
) -> DesktopWindowPlacement {
    let available_width = work_area.width.max(1);
    let available_height = work_area.height.max(1);
    let width = width
        .max(min_width.min(available_width))
        .min(available_width);
    let height = height
        .max(min_height.min(available_height))
        .min(available_height);
    let min_x = i64::from(work_area.x);
    let min_y = i64::from(work_area.y);
    let max_x = min_x + i64::from(available_width) - i64::from(width);
    let max_y = min_y + i64::from(available_height) - i64::from(height);
    let x = desired_x.clamp(min_x, max_x) as i32;
    let y = desired_y.clamp(min_y, max_y) as i32;

    DesktopWindowPlacement {
        x,
        y,
        width,
        height,
    }
}

fn resolve_desktop_window_placement(
    layout: &DesktopWindowLayout,
    work_areas: &[DesktopMonitorWorkArea],
    primary_index: Option<usize>,
    min_logical_width: u32,
    min_logical_height: u32,
) -> Option<DesktopWindowPlacement> {
    let monitor_index = select_desktop_monitor_index(layout, work_areas, primary_index)?;
    let work_area = work_areas.get(monitor_index)?;
    let target_scale = valid_scale_factor(work_area.scale_factor);
    let saved_scale = layout
        .monitor_scale_factor
        .filter(|value| value.is_finite() && *value > 0.0)
        .unwrap_or(target_scale);
    let scale_ratio = target_scale / saved_scale;
    let width = scale_u32(layout.width, scale_ratio);
    let height = scale_u32(layout.height, scale_ratio);
    let desired_position = layout
        .monitor_offset_x
        .zip(layout.monitor_offset_y)
        .map(|(offset_x, offset_y)| {
            (
                i64::from(work_area.x) + i64::from(scale_i32(offset_x, scale_ratio)),
                i64::from(work_area.y) + i64::from(scale_i32(offset_y, scale_ratio)),
            )
        })
        .unwrap_or((i64::from(layout.x), i64::from(layout.y)));

    Some(fit_desktop_window_to_work_area(
        desired_position.0,
        desired_position.1,
        width,
        height,
        scale_u32(min_logical_width, target_scale),
        scale_u32(min_logical_height, target_scale),
        work_area,
    ))
}

fn map_desktop_window_layout_between_work_areas(
    layout: &DesktopWindowLayout,
    source: &DesktopMonitorWorkArea,
    target: &DesktopMonitorWorkArea,
    min_logical_width: u32,
    min_logical_height: u32,
) -> DesktopWindowPlacement {
    let horizontal_ratio = f64::from(target.width.max(1)) / f64::from(source.width.max(1));
    let vertical_ratio = f64::from(target.height.max(1)) / f64::from(source.height.max(1));
    let offset_x = layout
        .monitor_offset_x
        .unwrap_or_else(|| layout.x.saturating_sub(source.x));
    let offset_y = layout
        .monitor_offset_y
        .unwrap_or_else(|| layout.y.saturating_sub(source.y));
    let desired_x = i64::from(target.x) + i64::from(scale_i32(offset_x, horizontal_ratio));
    let desired_y = i64::from(target.y) + i64::from(scale_i32(offset_y, vertical_ratio));
    let width = scale_u32(layout.width, horizontal_ratio);
    let height = scale_u32(layout.height, vertical_ratio);
    let target_scale = valid_scale_factor(target.scale_factor);

    fit_desktop_window_to_work_area(
        desired_x,
        desired_y,
        width,
        height,
        scale_u32(min_logical_width, target_scale),
        scale_u32(min_logical_height, target_scale),
        target,
    )
}

fn capture_desktop_window_layout(window: &WebviewWindow) -> tauri::Result<DesktopWindowLayout> {
    let position = window.outer_position()?;
    let size = window.inner_size()?;
    let monitors = window.available_monitors().unwrap_or_default();
    let work_areas = desktop_monitor_work_areas(&monitors);
    let current_index = window
        .current_monitor()
        .ok()
        .flatten()
        .as_ref()
        .and_then(|monitor| monitor_work_area_index(&work_areas, monitor));
    let primary_index = window
        .primary_monitor()
        .ok()
        .flatten()
        .as_ref()
        .and_then(|monitor| monitor_work_area_index(&work_areas, monitor));
    let monitor_index = best_intersecting_monitor_index(
        position.x,
        position.y,
        size.width,
        size.height,
        &work_areas,
    )
    .or(current_index)
    .or(primary_index);
    let monitor = monitor_index.and_then(|index| work_areas.get(index));

    Ok(DesktopWindowLayout {
        x: position.x,
        y: position.y,
        width: size.width,
        height: size.height,
        monitor_name: monitor.and_then(|work_area| work_area.name.clone()),
        monitor_offset_x: monitor.map(|work_area| position.x.saturating_sub(work_area.x)),
        monitor_offset_y: monitor.map(|work_area| position.y.saturating_sub(work_area.y)),
        monitor_scale_factor: monitor.map(|work_area| work_area.scale_factor),
    })
}

fn desktop_window_placement_for_layout(
    window: &WebviewWindow,
    label: &str,
    layout: &DesktopWindowLayout,
) -> DesktopWindowPlacement {
    let monitors = window.available_monitors().unwrap_or_default();
    let work_areas = desktop_monitor_work_areas(&monitors);
    let primary_index = window
        .primary_monitor()
        .ok()
        .flatten()
        .as_ref()
        .and_then(|monitor| monitor_work_area_index(&work_areas, monitor));
    let (min_width, min_height) = desktop_window_min_logical_size(label);

    resolve_desktop_window_placement(layout, &work_areas, primary_index, min_width, min_height)
        .unwrap_or(DesktopWindowPlacement {
            x: layout.x,
            y: layout.y,
            width: layout.width.max(min_width),
            height: layout.height.max(min_height),
        })
}

fn apply_saved_desktop_window_layout(window: &WebviewWindow, label: &str) -> Result<bool, String> {
    let Some(layout) = saved_desktop_window_layout(label) else {
        return Ok(false);
    };

    let (min_width, min_height) = desktop_window_min_logical_size(label);
    let placement = desktop_window_target_work_area(window.app_handle(), label)
        .and_then(|work_area| {
            resolve_desktop_window_placement(
                &layout,
                std::slice::from_ref(&work_area),
                Some(0),
                min_width,
                min_height,
            )
        })
        .unwrap_or_else(|| desktop_window_placement_for_layout(window, label, &layout));
    window
        .set_position(Position::Physical(PhysicalPosition::new(
            placement.x,
            placement.y,
        )))
        .map_err(to_message)?;
    window
        .set_size(Size::Physical(PhysicalSize::new(
            placement.width,
            placement.height,
        )))
        .map_err(to_message)?;
    Ok(true)
}

fn apply_saved_desktop_window_layout_to_monitor(
    window: &WebviewWindow,
    label: &str,
    work_area: &DesktopMonitorWorkArea,
) -> Result<bool, String> {
    let (min_width, min_height) = desktop_window_min_logical_size(label);
    let placement = if let Some(layout) = saved_desktop_window_layout(label) {
        resolve_desktop_window_placement(
            &layout,
            std::slice::from_ref(work_area),
            Some(0),
            min_width,
            min_height,
        )
        .unwrap_or(DesktopWindowPlacement {
            x: work_area.x,
            y: work_area.y,
            width: layout.width.max(min_width),
            height: layout.height.max(min_height),
        })
    } else if let Some((layout, source_work_area)) =
        primary_desktop_window_layout_template(window.app_handle(), label)
    {
        map_desktop_window_layout_between_work_areas(
            &layout,
            &source_work_area,
            work_area,
            min_width,
            min_height,
        )
    } else {
        return Ok(false);
    };
    window
        .set_position(Position::Physical(PhysicalPosition::new(
            placement.x,
            placement.y,
        )))
        .map_err(to_message)?;
    window
        .set_size(Size::Physical(PhysicalSize::new(
            placement.width,
            placement.height,
        )))
        .map_err(to_message)?;
    Ok(true)
}

fn is_valid_desktop_window_layout(layout: &DesktopWindowLayout) -> bool {
    layout.width >= 120 && layout.height >= 100
}

fn desktop_monitor_stable_id(work_area: &DesktopMonitorWorkArea) -> String {
    let identity = work_area
        .name
        .as_deref()
        .filter(|name| !name.trim().is_empty())
        .map(|name| format!("name:{}", name.to_lowercase()))
        .unwrap_or_else(|| {
            format!(
                "area:{}:{}:{}:{}",
                work_area.x, work_area.y, work_area.width, work_area.height
            )
        });
    let hash = identity
        .as_bytes()
        .iter()
        .fold(0xcbf29ce484222325u64, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
        });
    format!("{hash:016x}")
}

fn desktop_monitor_targets(app: &tauri::AppHandle) -> tauri::Result<Vec<DesktopMonitorTarget>> {
    let monitors = app.available_monitors()?;
    if monitors.is_empty() {
        return Ok(Vec::new());
    }

    let primary_work_area = app
        .primary_monitor()?
        .as_ref()
        .map(DesktopMonitorWorkArea::from_monitor);
    let primary_index = primary_work_area
        .as_ref()
        .and_then(|primary| {
            monitors.iter().position(|monitor| {
                let candidate = DesktopMonitorWorkArea::from_monitor(monitor);
                match (&primary.name, &candidate.name) {
                    (Some(left), Some(right)) => left == right,
                    _ => primary == &candidate,
                }
            })
        })
        .unwrap_or(0);

    let mut targets = monitors
        .into_iter()
        .enumerate()
        .map(|(index, monitor)| {
            let work_area = DesktopMonitorWorkArea::from_monitor(&monitor);
            DesktopMonitorTarget {
                stable_id: desktop_monitor_stable_id(&work_area),
                monitor,
                work_area,
                is_primary: index == primary_index,
            }
        })
        .collect::<Vec<_>>();
    targets.sort_by(|left, right| {
        right
            .is_primary
            .cmp(&left.is_primary)
            .then(left.work_area.x.cmp(&right.work_area.x))
            .then(left.work_area.y.cmp(&right.work_area.y))
            .then(left.stable_id.cmp(&right.stable_id))
    });
    Ok(targets)
}

fn desktop_window_target_work_area(
    app: &tauri::AppHandle,
    label: &str,
) -> Option<DesktopMonitorWorkArea> {
    let targets = desktop_monitor_targets(app).ok()?;
    if is_primary_desktop_window_label(label) {
        return targets
            .into_iter()
            .find(|target| target.is_primary)
            .map(|target| target.work_area);
    }

    let monitor_id = secondary_monitor_id_from_desktop_label(label)?;
    targets
        .into_iter()
        .find(|target| !target.is_primary && target.stable_id == monitor_id)
        .map(|target| target.work_area)
}

fn desktop_monitor_signature(app: &tauri::AppHandle) -> Option<Vec<DesktopMonitorWorkArea>> {
    let monitors = app.available_monitors().ok()?;
    let mut work_areas = desktop_monitor_work_areas(&monitors);
    work_areas.sort_by(|left, right| {
        left.x
            .cmp(&right.x)
            .then(left.y.cmp(&right.y))
            .then(left.width.cmp(&right.width))
            .then(left.height.cmp(&right.height))
            .then(left.name.cmp(&right.name))
    });
    Some(work_areas)
}

fn destroy_disconnected_desktop_monitor_windows(
    app: &tauri::AppHandle,
    targets: &[DesktopMonitorTarget],
) {
    let active_secondary_ids = targets
        .iter()
        .filter(|target| !target.is_primary)
        .map(|target| target.stable_id.as_str())
        .collect::<BTreeSet<_>>();
    for window in app.webview_windows().values() {
        let Some(monitor_id) = secondary_monitor_id_from_desktop_label(window.label()) else {
            continue;
        };
        if !active_secondary_ids.contains(monitor_id) {
            let _ = window.destroy();
        }
    }
}

fn repair_desktop_windows_after_monitor_change(app: &tauri::AppHandle) -> Result<(), String> {
    let visibility = desktop_frame_visibility_impl(app);
    let targets = desktop_monitor_targets(app).map_err(to_message)?;
    destroy_disconnected_desktop_monitor_windows(app, &targets);

    if visibility.organizer {
        show_persisted_desktop_organizer(app)?;
    }
    if visibility.launcher {
        show_desktop_launcher(app)?;
    }
    for window in app.webview_windows().values() {
        if is_desktop_card_window_label(window.label()) {
            keep_desktop_window_behind_apps(window);
        }
    }
    Ok(())
}

fn start_desktop_monitor_watch(app: tauri::AppHandle) {
    if DESKTOP_MONITOR_WATCH_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }

    if let Err(error) = std::thread::Builder::new()
        .name("dustdesk-monitor-layout".to_owned())
        .spawn(move || {
            let mut previous_signature = desktop_monitor_signature(&app);
            while !REAL_EXIT_REQUESTED.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_secs(1));
                let Some(current_signature) = desktop_monitor_signature(&app) else {
                    continue;
                };

                if previous_signature
                    .as_ref()
                    .is_some_and(|previous| previous != &current_signature)
                {
                    std::thread::sleep(Duration::from_millis(300));
                    if let Err(error) = with_lazy_window_operation(|| {
                        repair_desktop_windows_after_monitor_change(&app)
                    }) {
                        eprintln!("无法同步桌面框多显示器实例：{error}");
                    }
                    append_desktop_diagnostic(
                        "desktop_monitor_topology_changed",
                        &format!("monitor_count={}", current_signature.len()),
                    );
                }
                previous_signature = Some(current_signature);
            }
            DESKTOP_MONITOR_WATCH_STARTED.store(false, Ordering::SeqCst);
        })
    {
        DESKTOP_MONITOR_WATCH_STARTED.store(false, Ordering::SeqCst);
        eprintln!("无法启动桌面框显示器监控：{error}");
    }
}

fn cleanup_desktop_card_windows_impl(app: &tauri::AppHandle) {
    for window in app.webview_windows().values() {
        if is_desktop_card_window_label(window.label()) {
            let _ = window.destroy();
        }
    }
}

fn hide_main_window_impl(app: &tauri::AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("main") {
        window.hide().map_err(to_message)?;
    }
    Ok(())
}

fn show_main_window_impl(app: &tauri::AppHandle) -> Result<(), String> {
    let window = match app.get_webview_window("main") {
        Some(window) => window,
        None => create_main_window(app)?,
    };
    window.show().map_err(to_message)?;
    let _ = window.unminimize();
    let _ = window.set_focus();
    let _ = app.emit("dustdesk://main-window-shown", ());
    Ok(())
}

fn hide_main_window_to_tray_async(app: tauri::AppHandle) {
    tauri::async_runtime::spawn_blocking(move || {
        if let Err(error) = with_lazy_window_operation(|| hide_main_window_impl(&app)) {
            eprintln!("failed to hide main window to tray: {error}");
        }
    });
}

fn show_main_window_from_tray_async(app: tauri::AppHandle) {
    tauri::async_runtime::spawn_blocking(move || {
        if let Err(error) = with_lazy_window_operation(|| show_main_window_impl(&app)) {
            eprintln!("failed to show main window from tray: {error}");
        }
    });
}

fn request_real_app_exit(app: tauri::AppHandle) {
    if REAL_EXIT_REQUESTED.swap(true, Ordering::SeqCst) {
        return;
    }

    tauri::async_runtime::spawn_blocking(move || {
        wait_for_desktop_operation_idle();
        if let Err(error) = with_lazy_window_operation(|| show_persisted_desktop_layout(&app)) {
            eprintln!("failed to show exit operation overlay: {error}");
        }
        let loading_started_at = Instant::now();
        publish_lifecycle_desktop_operation(
            &app,
            DesktopOperationPayload {
                kind: "restore",
                scope: "lifecycle",
                status: "started",
                message: "正在退出并还原桌面...".to_owned(),
                moved: 0,
                skipped: 0,
                restored: 0,
                total: 0,
                current_path: String::new(),
                category_counts: Vec::new(),
            },
            true,
        );
        let _running_reset_guard = DesktopOperationRunningResetGuard;
        let progress_app = app.clone();
        let restore_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            restore_desktop_organization_for_exit_with_progress(move |current, total, path| {
                publish_lifecycle_desktop_operation(
                    &progress_app,
                    DesktopOperationPayload {
                        kind: "restore",
                        scope: "lifecycle",
                        status: "progress",
                        message: restore_progress_message(current, total, path),
                        moved: 0,
                        skipped: 0,
                        restored: current,
                        total,
                        current_path: path.display().to_string(),
                        category_counts: Vec::new(),
                    },
                    true,
                );
            })
        }))
        .unwrap_or_else(|_| Err("退出前还原桌面任务异常中断".to_owned()));
        keep_loading_effect_visible(loading_started_at, Duration::from_millis(850));

        let restored = match restore_result {
            Ok(restored) => restored,
            Err(error) => {
                eprintln!("failed to restore organized desktop items on exit: {error}");
                REAL_EXIT_REQUESTED.store(false, Ordering::SeqCst);
                publish_lifecycle_desktop_operation(
                    &app,
                    DesktopOperationPayload {
                        kind: "restore",
                        scope: "lifecycle",
                        status: "failed",
                        message: format!("退出前还原桌面失败，程序仍在运行：{error}"),
                        moved: 0,
                        skipped: 0,
                        restored: 0,
                        total: 0,
                        current_path: String::new(),
                        category_counts: Vec::new(),
                    },
                    false,
                );
                emit_desktop_cards_changed(&app);
                if let Err(show_error) = with_lazy_window_operation(|| show_main_window_impl(&app))
                {
                    eprintln!("failed to show exit restore error: {show_error}");
                }
                return;
            }
        };
        publish_lifecycle_desktop_operation(
            &app,
            DesktopOperationPayload {
                kind: "restore",
                scope: "lifecycle",
                status: "finished",
                message: if restored > 0 {
                    format!("退出前已还原 {restored} 项到桌面")
                } else {
                    "桌面已还原，正在退出...".to_owned()
                },
                moved: 0,
                skipped: 0,
                restored,
                total: restored,
                current_path: String::new(),
                category_counts: Vec::new(),
            },
            false,
        );
        EXIT_RESTORE_COMPLETED.store(true, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(120));
        cleanup_desktop_card_windows_impl(&app);
        app.exit(0);
    });
}

fn setup_system_tray(app: &tauri::AppHandle) -> tauri::Result<()> {
    let show_item = MenuItem::with_id(app, TRAY_MENU_SHOW_MAIN, "显示主窗口", true, None::<&str>)?;
    let visibility = desktop_frame_visibility_impl(app);
    let state = load_desktop_window_state().unwrap_or(DesktopWindowState {
        locked: false,
        click_through: false,
    });
    let desktop_visible_item = CheckMenuItem::with_id(
        app,
        TRAY_MENU_DESKTOP_VISIBLE,
        "显示桌面框",
        true,
        visibility.any,
        None::<&str>,
    )?;
    let desktop_locked_item = CheckMenuItem::with_id(
        app,
        TRAY_MENU_DESKTOP_LOCKED,
        if state.locked {
            "解锁全部桌面框"
        } else {
            "锁定全部桌面框"
        },
        true,
        state.locked,
        None::<&str>,
    )?;
    let desktop_click_through_item = CheckMenuItem::with_id(
        app,
        TRAY_MENU_DESKTOP_CLICK_THROUGH,
        "桌面框鼠标穿透",
        true,
        state.click_through,
        None::<&str>,
    )?;
    let separator = PredefinedMenuItem::separator(app)?;
    let exit_separator = PredefinedMenuItem::separator(app)?;
    let quit_item = MenuItem::with_id(app, TRAY_MENU_QUIT, "退出", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &show_item,
            &separator,
            &desktop_visible_item,
            &desktop_locked_item,
            &desktop_click_through_item,
            &exit_separator,
            &quit_item,
        ],
    )?;

    let _ = TRAY_MENU_ITEMS.set(TrayMenuItems {
        desktop_visible: desktop_visible_item,
        desktop_locked: desktop_locked_item,
        desktop_click_through: desktop_click_through_item,
    });

    let mut tray = TrayIconBuilder::with_id("desknest-main-tray")
        .menu(&menu)
        .tooltip("DeskNest 后台运行")
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| {
            if event.id() == TRAY_MENU_SHOW_MAIN {
                show_main_window_from_tray_async(app.clone());
            } else if event.id() == TRAY_MENU_DESKTOP_VISIBLE {
                let app = app.clone();
                tauri::async_runtime::spawn_blocking(move || {
                    let result = if desktop_frame_visibility_impl(&app).any {
                        hide_desktop_card_windows(&app);
                        Ok(())
                    } else {
                        show_persisted_desktop_layout(&app)
                    };
                    if let Err(error) = result {
                        eprintln!("failed to toggle desktop frames from tray: {error}");
                    }
                    sync_system_tray_menu(&app);
                });
            } else if event.id() == TRAY_MENU_DESKTOP_LOCKED {
                let app = app.clone();
                tauri::async_runtime::spawn_blocking(move || {
                    let state = load_desktop_window_state().unwrap_or(DesktopWindowState {
                        locked: false,
                        click_through: false,
                    });
                    if let Err(error) =
                        set_desktop_window_state_impl(&app, Some(!state.locked), None)
                    {
                        eprintln!("failed to toggle desktop frame lock from tray: {error}");
                    }
                    sync_system_tray_menu(&app);
                });
            } else if event.id() == TRAY_MENU_DESKTOP_CLICK_THROUGH {
                let app = app.clone();
                tauri::async_runtime::spawn_blocking(move || {
                    let state = load_desktop_window_state().unwrap_or(DesktopWindowState {
                        locked: false,
                        click_through: false,
                    });
                    if let Err(error) =
                        set_desktop_window_state_impl(&app, None, Some(!state.click_through))
                    {
                        eprintln!(
                            "failed to toggle desktop frame click-through from tray: {error}"
                        );
                    }
                    sync_system_tray_menu(&app);
                });
            } else if event.id() == TRAY_MENU_QUIT {
                request_real_app_exit(app.clone());
            }
        })
        .on_tray_icon_event(|tray, event| match event {
            TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            }
            | TrayIconEvent::DoubleClick {
                button: MouseButton::Left,
                ..
            } => show_main_window_from_tray_async(tray.app_handle().clone()),
            _ => {}
        });

    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }

    tray.build(app)?;
    Ok(())
}

fn main() {
    install_persistent_panic_hook();
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            if !args.iter().any(|arg| arg == "--startup") {
                show_main_window_from_tray_async(app.clone());
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    if event.state() == ShortcutState::Pressed {
                        let settings = configured_settings();
                        if shortcut_matches(shortcut, &settings.search_shortcut_value()) {
                            if settings.search_enabled {
                                show_search_overlay_async(app.clone());
                            }
                            return;
                        }

                        if !shortcut_matches(shortcut, &settings.clipboard_shortcut_value()) {
                            return;
                        }

                        let sync_clipboard = !is_clipboard_overlay_visible(app);
                        if sync_clipboard {
                            clipboard_bridge::remember_foreground_window();
                        }
                        show_clipboard_overlay_async(app.clone(), sync_clipboard);
                    }
                })
                .build(),
        )
        .on_window_event(|window, event| {
            if window.label() == "main" {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    if !REAL_EXIT_REQUESTED.load(Ordering::SeqCst) {
                        api.prevent_close();
                        hide_main_window_to_tray_async(window.app_handle().clone());
                    }
                }
                return;
            }

            if is_desktop_card_window_label(window.label())
                && matches!(event, tauri::WindowEvent::Resized(_))
                && window.is_maximized().unwrap_or(false)
            {
                let app = window.app_handle().clone();
                let label = window.label().to_owned();
                tauri::async_runtime::spawn_blocking(move || {
                    let Some(window) = app.get_webview_window(&label) else {
                        return;
                    };
                    let _ = window.unmaximize();
                    let _ = apply_saved_desktop_window_layout(&window, &label);
                    if let Ok(state) = load_desktop_window_state() {
                        let _ = apply_desktop_window_state(&window, state);
                    }
                });
            }
        })
        .setup(|app| {
            let app_handle = app.handle().clone();
            setup_system_tray(&app_handle)?;
            #[cfg(windows)]
            windows_show_desktop::start(app_handle.clone());
            start_desktop_monitor_watch(app_handle.clone());
            let show_main_on_start = !is_startup_launch_invocation();
            let handle = app.handle().clone();
            tauri::async_runtime::spawn_blocking(move || {
                let mut completion_guard = StartupRecoveryCompletionGuard::new();
                let recovery_started = Instant::now();
                append_desktop_diagnostic(
                    "startup_recovery_started",
                    &format!(
                        "current_exe=\"{}\"",
                        diagnostic_value(&current_exe_diagnostic_value())
                    ),
                );
                publish_lifecycle_desktop_operation(
                    &handle,
                    DesktopOperationPayload {
                        kind: "classify",
                        scope: "lifecycle",
                        status: "started",
                        message: "正在检查并恢复上次收纳布局...".to_owned(),
                        moved: 0,
                        skipped: 0,
                        restored: 0,
                        total: 0,
                        current_path: String::new(),
                        category_counts: Vec::new(),
                    },
                    true,
                );
                let _running_reset_guard = DesktopOperationRunningResetGuard;
                if show_main_on_start {
                    if let Err(error) =
                        with_lazy_window_operation(|| show_main_window_impl(&handle))
                    {
                        eprintln!("failed to show startup loading window: {error}");
                    }
                }

                let progress_handle = handle.clone();
                let mut loading_visible_at = None;
                let recovery_result =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        store::with_storage_mutation(recover_pending_runtime_migration_locked)?;
                        if let Err(error) =
                            with_lazy_window_operation(|| show_persisted_desktop_layout(&handle))
                        {
                            eprintln!("failed to show startup operation overlay: {error}");
                        }
                        loading_visible_at = Some(Instant::now());
                        recollect_desktop_organization_from_restart_marker_with_progress(
                            move |current, total, path| {
                                publish_lifecycle_desktop_operation(
                                    &progress_handle,
                                    DesktopOperationPayload {
                                        kind: "classify",
                                        scope: "lifecycle",
                                        status: "progress",
                                        message: restart_recollect_progress_message(
                                            current, total, path,
                                        ),
                                        moved: current,
                                        skipped: 0,
                                        restored: 0,
                                        total,
                                        current_path: path.display().to_string(),
                                        category_counts: Vec::new(),
                                    },
                                    true,
                                );
                            },
                        )
                    }))
                    .unwrap_or_else(|_| Err("启动恢复任务异常中断".to_owned()));
                if let Some(visible_at) = loading_visible_at {
                    keep_loading_effect_visible(visible_at, Duration::from_millis(850));
                }
                completion_guard.complete(recovery_result.clone().map(|_| ()));

                match recovery_result {
                    Ok(recollected) => {
                        append_desktop_diagnostic(
                            "startup_recovery_finished",
                            &format!(
                                "elapsed_ms={} recollected={}",
                                recovery_started.elapsed().as_millis(),
                                recollected
                            ),
                        );
                        initialize_runtime_services(&handle);
                        emit_desktop_cards_changed(&handle);
                        if let Err(error) = with_lazy_window_operation(|| {
                            if show_main_on_start {
                                show_main_window_impl(&handle)?;
                            }
                            show_persisted_desktop_layout(&handle)
                        }) {
                            eprintln!("failed to show desktop cards: {error}");
                        }
                        publish_lifecycle_desktop_operation(
                            &handle,
                            DesktopOperationPayload {
                                kind: "classify",
                                scope: "lifecycle",
                                status: "finished",
                                message: if recollected > 0 {
                                    format!("已恢复上次收纳布局，共 {recollected} 项")
                                } else {
                                    "收纳布局已就绪".to_owned()
                                },
                                moved: recollected,
                                skipped: 0,
                                restored: 0,
                                total: recollected,
                                current_path: String::new(),
                                category_counts: Vec::new(),
                            },
                            false,
                        );
                        schedule_stale_transfer_staging_cleanup();
                    }
                    Err(error) => {
                        append_desktop_diagnostic(
                            "startup_recovery_failed",
                            &format!(
                                "elapsed_ms={} error=\"{}\"",
                                recovery_started.elapsed().as_millis(),
                                diagnostic_value(&error)
                            ),
                        );
                        eprintln!("failed to complete startup recovery: {error}");
                        publish_lifecycle_desktop_operation(
                            &handle,
                            DesktopOperationPayload {
                                kind: "classify",
                                scope: "lifecycle",
                                status: "failed",
                                message: format!("启动恢复失败，请重新启动 DustDesk：{error}"),
                                moved: 0,
                                skipped: 0,
                                restored: 0,
                                total: 0,
                                current_path: String::new(),
                                category_counts: Vec::new(),
                            },
                            false,
                        );
                        if let Err(show_error) =
                            with_lazy_window_operation(|| show_main_window_impl(&handle))
                        {
                            eprintln!("failed to show startup recovery error: {show_error}");
                        }
                    }
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            load_snapshot,
            load_desktop_snapshot,
            resolve_path_icons,
            create_category,
            rename_category,
            delete_category,
            toggle_category,
            set_category_sort_by_name,
            reorder_category,
            add_item_to_category,
            add_items_to_category,
            remove_item_from_category,
            register_internal_path_drop_category,
            mark_internal_path_drop_accepted,
            complete_internal_path_drag,
            restore_item_to_desktop,
            restore_all_to_desktop,
            start_restore_all_to_desktop_task,
            desktop_operation_status,
            add_launcher,
            add_launchers,
            remove_launcher,
            show_path_in_folder,
            classify_desktop_items,
            start_classify_desktop_items_task,
            create_desktop_entries,
            open_special,
            update_runtime_directory,
            check_for_updates,
            open_update_download,
            open_path,
            open_path_as_administrator,
            start_all_launchers,
            start_all_launchers_as_administrator,
            show_desktop_widget,
            desktop_frame_visibility,
            desktop_window_state,
            set_desktop_windows_locked,
            set_desktop_windows_click_through,
            toggle_desktop_frames,
            toggle_desktop_organizer_frame,
            toggle_desktop_launcher_frame,
            save_desktop_window_layout,
            save_desktop_split_indices,
            split_desktop_widgets,
            split_desktop_category,
            merge_desktop_category,
            merge_desktop_widgets,
            cleanup_desktop_card_windows,
            load_search_overlay,
            search_items,
            open_search_item,
            hide_search_overlay,
            hide_current_window,
            hide_main_window_to_tray,
            repaint_current_window,
            clipboard_image_base64,
            paste_clipboard_item,
            hide_clipboard_overlay,
            update_clipboard_shortcut,
            update_clipboard_history_limit,
            update_search_settings,
            update_launch_on_startup
        ])
        .build(tauri::generate_context!())
        .expect("failed to build DustDesk Tauri app");

    app.run(|app_handle, event| match event {
        tauri::RunEvent::ExitRequested { api, .. } => {
            if !REAL_EXIT_REQUESTED.load(Ordering::SeqCst) {
                api.prevent_exit();
                request_real_app_exit(app_handle.clone());
            }
        }
        tauri::RunEvent::Exit => {
            // Windows session shutdown may bypass the preventable exit request. This synchronous
            // last chance keeps the same marker-first safety ordering before the process dies.
            if !EXIT_RESTORE_COMPLETED.load(Ordering::SeqCst) {
                REAL_EXIT_REQUESTED.store(true, Ordering::SeqCst);
                if let Err(error) = restore_desktop_organization_for_exit() {
                    eprintln!(
                        "failed to restore organized desktop items on terminal exit: {error}"
                    );
                } else {
                    EXIT_RESTORE_COMPLETED.store(true, Ordering::SeqCst);
                }
            }
        }
        _ => {}
    });
}
