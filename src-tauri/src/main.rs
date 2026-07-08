#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod clipboard_bridge;
mod models;
mod store;
mod system_icon;

use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex, OnceLock,
    },
};

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use models::{
    AppConfig, AppSettings, AppSnapshot, CategoryClassifyCount, ClassifyResult,
    ClipboardHistoryItem, DeskCategory, DesktopItem, LaunchItem, PathIconResult, SearchHistoryData,
    SearchHistoryItem, SearchItem, SearchItemKind, SearchOverlayData,
};
use serde::Serialize;
use store::AppStore;
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    utils::config::Color, Emitter, Manager, PhysicalPosition, PhysicalSize, Position, Size,
    WebviewUrl, WebviewWindow, WebviewWindowBuilder,
};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

const SEARCH_RESULT_LIMIT: usize = 80;
const SEARCH_SCAN_LIMIT: usize = 8_000;
const SEARCH_HISTORY_LIMIT: usize = 500;
const CLIPBOARD_PREVIEW_IMAGE_MAX_BYTES: u64 = 800_000;
const TRAY_MENU_SHOW_MAIN: &str = "show-main-window";
const TRAY_MENU_QUIT: &str = "quit-app";

static CONFIG_MUTATION_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static LAUNCHER_MUTATION_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static REAL_EXIT_REQUESTED: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Copy, Serialize)]
struct DesktopFrameVisibility {
    organizer: bool,
    launcher: bool,
    any: bool,
}

#[tauri::command]
async fn load_snapshot() -> Result<AppSnapshot, String> {
    tauri::async_runtime::spawn_blocking(load_snapshot_impl)
        .await
        .map_err(to_message)?
}

fn load_snapshot_impl() -> Result<AppSnapshot, String> {
    let store = AppStore::open().map_err(to_message)?;
    store.ensure_runtime_dirs().map_err(to_message)?;
    let config = with_config_mutation(|| {
        let mut config = store.load_config();
        repair_category_item_paths(&store, &mut config)?;
        Ok(config)
    })?;
    let launchers = with_launcher_mutation(|| {
        let mut launchers = store.load_launchers();
        repair_launchers(&store, &mut launchers)?;
        Ok(launchers)
    })?;
    let mut clipboard = store.load_clipboard();
    if clipboard_bridge::normalize_clipboard_image_storage(&mut clipboard).unwrap_or(false) {
        let _ = store.save_clipboard(&clipboard);
    }

    Ok(AppSnapshot {
        data_dir: store.data_dir().display().to_string(),
        organizer_root: store.organizer_root().display().to_string(),
        launchers_root: store.launchers_root().display().to_string(),
        settings: config.settings,
        categories: categories_with_item_details(config.desktop_categories, false),
        desktop_items: desktop_items(false),
        launchers: launchers_with_optional_icons(launchers.items, false),
        clipboard: clipboard_preview(clipboard.items),
    })
}

#[tauri::command]
async fn load_desktop_snapshot() -> Result<AppSnapshot, String> {
    tauri::async_runtime::spawn_blocking(load_desktop_snapshot_impl)
        .await
        .map_err(to_message)?
}

fn load_desktop_snapshot_impl() -> Result<AppSnapshot, String> {
    let store = AppStore::open().map_err(to_message)?;
    store.ensure_runtime_dirs().map_err(to_message)?;
    let config = with_config_mutation(|| {
        let mut config = store.load_config();
        repair_category_item_paths(&store, &mut config)?;
        Ok(config)
    })?;
    let launchers = with_launcher_mutation(|| {
        let mut launchers = store.load_launchers();
        repair_launchers(&store, &mut launchers)?;
        Ok(launchers)
    })?;

    Ok(AppSnapshot {
        data_dir: store.data_dir().display().to_string(),
        organizer_root: store.organizer_root().display().to_string(),
        launchers_root: store.launchers_root().display().to_string(),
        settings: config.settings,
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
            is_collapsed: false,
            item_paths: Vec::new(),
            item_details: Vec::new(),
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
async fn add_item_to_category(
    app: tauri::AppHandle,
    index: usize,
    path: String,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        add_items_to_category_impl(index, vec![path])?;
        emit_desktop_cards_changed(&app);
        Ok(())
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
        let added = add_items_to_category_impl(index, paths)?;
        emit_desktop_cards_changed(&app);
        Ok(added)
    })
    .await
    .map_err(to_message)?
}

fn add_items_to_category_impl(index: usize, paths: Vec<String>) -> Result<usize, String> {
    with_config_mutation(|| {
        let store = AppStore::open().map_err(to_message)?;
        store.ensure_runtime_dirs().map_err(to_message)?;
        let mut config = store.load_config();
        repair_category_item_paths(&store, &mut config)?;
        config
            .desktop_categories
            .get(index)
            .ok_or_else(|| "分类不存在".to_owned())?;
        let category_name = config.desktop_categories[index].name.clone();
        let mut added = 0usize;

        for path in paths {
            let path = normalize_path_input(&path)?;
            if path.trim().is_empty() || !Path::new(&path).exists() {
                continue;
            }
            let archived_path = archive_item_path(&store, &category_name, &path)?;

            for category in &mut config.desktop_categories {
                category.item_paths.retain(|item_path| {
                    !same_path_text(item_path, &path) && !same_path_text(item_path, &archived_path)
                });
            }

            config.desktop_categories[index]
                .item_paths
                .push(archived_path);
            added += 1;
        }

        store.save_config(&config).map_err(to_message)?;
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
fn restore_item_to_desktop(
    app: tauri::AppHandle,
    index: usize,
    path: String,
) -> Result<String, String> {
    let restored_path = with_config_mutation(|| {
        let path = normalize_path_input(&path)?;
        let desktop = user_desktop().ok_or_else(|| "没有找到桌面路径".to_owned())?;
        fs::create_dir_all(&desktop).map_err(to_message)?;
        let restored_path = restore_path_to_desktop(Path::new(&path), &desktop)?;

        let store = AppStore::open().map_err(to_message)?;
        let mut config = store.load_config();
        remove_category_path_from_config(&mut config, index, &path)?;
        store.save_config(&config).map_err(to_message)?;
        Ok(restored_path)
    })?;
    emit_desktop_cards_changed(&app);
    Ok(restored_path.display().to_string())
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
        let mut launchers = store.load_launchers();
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
        let mut launchers = store.load_launchers();
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
    tauri::async_runtime::spawn_blocking(move || classify_desktop_items_impl(&app))
        .await
        .map_err(to_message)?
}

fn classify_desktop_items_impl(app: &tauri::AppHandle) -> Result<ClassifyResult, String> {
    let result = with_config_mutation(|| {
        let store = AppStore::open().map_err(to_message)?;
        store.ensure_runtime_dirs().map_err(to_message)?;
        let mut config = store.load_config();
        repair_category_item_paths(&store, &mut config)?;
        if config.desktop_categories.is_empty() {
            return Err("没有可用分类，请先创建分类".to_owned());
        }

        let items = desktop_items(false);
        let mut category_counts = vec![0usize; config.desktop_categories.len()];
        let mut moved = 0usize;
        let mut skipped = 0usize;

        for item in items {
            if should_skip_desktop_classify_item(&item) {
                skipped += 1;
                continue;
            }

            if categories_contain_path(&config.desktop_categories, &item.path) {
                skipped += 1;
                continue;
            }

            let Some(category_index) = classify_desktop_item(&item, &config.desktop_categories) else {
                skipped += 1;
                continue;
            };

            let original_path = item.path.clone();
            let category_name = config.desktop_categories[category_index].name.clone();
            let archived_path = archive_item_path(&store, &category_name, &original_path)?;

            for category in &mut config.desktop_categories {
                category.item_paths.retain(|item_path| {
                    !same_path_text(item_path, &original_path)
                        && !same_path_text(item_path, &archived_path)
                });
            }

            config.desktop_categories[category_index]
                .item_paths
                .push(archived_path);
            category_counts[category_index] += 1;
            moved += 1;
        }

        store.save_config(&config).map_err(to_message)?;

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
    })?;
    emit_desktop_cards_changed(&app);
    Ok(result)
}

#[tauri::command]
fn create_desktop_entries() -> Result<Vec<String>, String> {
    let store = AppStore::open().map_err(to_message)?;
    store.ensure_runtime_dirs().map_err(to_message)?;
    let desktop = user_desktop().ok_or_else(|| "没有找到桌面路径".to_owned())?;
    fs::create_dir_all(&desktop).map_err(to_message)?;

    let entries = [
        ("DustDesk 收纳箱", store.organizer_root()),
        ("DustDesk 快捷启动", store.launchers_root()),
    ];
    let mut created = Vec::with_capacity(entries.len());

    for (name, target) in entries {
        let shortcut = desktop.join(format!("{name}.lnk"));
        create_windows_shortcut(&shortcut, &target, name)?;
        created.push(shortcut.display().to_string());
    }

    Ok(created)
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
fn open_path(path: String) -> Result<(), String> {
    open_path_impl(Path::new(&path))
}

#[tauri::command]
fn start_all_launchers() -> Result<usize, String> {
    let store = AppStore::open().map_err(to_message)?;
    let launchers = store.load_launchers();
    let mut count = 0usize;
    for launcher in launchers.items {
        if launcher.path.trim().is_empty() {
            continue;
        }
        open_with_shell(&launcher.path)?;
        count += 1;
    }
    Ok(count)
}

#[tauri::command]
fn show_desktop_widget(app: tauri::AppHandle) -> Result<(), String> {
    show_merged_desktop_widget(&app)?;
    show_desktop_launcher(&app)
}

#[tauri::command]
async fn desktop_frame_visibility(
    app: tauri::AppHandle,
) -> Result<DesktopFrameVisibility, String> {
    tauri::async_runtime::spawn_blocking(move || Ok(desktop_frame_visibility_impl(&app)))
        .await
        .map_err(to_message)?
}

#[tauri::command]
async fn toggle_desktop_frames(
    app: tauri::AppHandle,
) -> Result<DesktopFrameVisibility, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let visibility = desktop_frame_visibility_impl(&app);
        if visibility.any {
            hide_desktop_card_windows(&app);
        } else {
            show_merged_desktop_widget(&app)?;
            show_desktop_launcher(&app)?;
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
            show_merged_desktop_widget(&app)?;
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

fn show_merged_desktop_widget(app: &tauri::AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("desktop-widget")
        .ok_or_else(|| "桌面框窗口不存在".to_owned())?;
    place_desktop_widget(&window).map_err(to_message)?;
    show_desktop_background_window(&window)
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
    let categories = store.load_config().desktop_categories;
    let category = categories
        .get(index)
        .ok_or_else(|| "分类不存在".to_owned())?;

    let label = desktop_category_label(index);
    let title = format!("DustDesk {}", category.name);
    let url = desktop_card_url("category", Some(index));
    show_or_create_desktop_card(app, &label, &title, &url, index)?;
    show_desktop_launcher(app)?;
    Ok(())
}

#[tauri::command]
fn merge_desktop_category(app: tauri::AppHandle, index: usize) -> Result<(), String> {
    hide_desktop_category_windows(&app, index);
    show_merged_desktop_widget(&app)?;
    show_desktop_launcher(&app)?;
    let _ = app.emit("dustdesk://desktop-cards-changed", ());
    Ok(())
}

fn show_split_desktop_widgets(app: &tauri::AppHandle) -> Result<Vec<usize>, String> {
    let store = AppStore::open().map_err(to_message)?;
    store.ensure_runtime_dirs().map_err(to_message)?;
    let categories = store.load_config().desktop_categories;

    let split_indices = (0..categories.len()).collect::<Vec<_>>();

    if split_indices.is_empty() {
        show_desktop_launcher(app)?;
        return Ok(split_indices);
    }

    for index in split_indices.iter().copied() {
        let Some(category) = categories.get(index) else {
            continue;
        };
        let label = desktop_category_label(index);
        let title = format!("DustDesk {}", category.name);
        let url = desktop_card_url("category", Some(index));
        show_or_create_desktop_card(app, &label, &title, &url, index)?;
    }

    show_desktop_launcher(app)?;

    if let Some(window) = app.get_webview_window("desktop-widget") {
        let _ = window.hide();
    }

    for window in app.webview_windows().values() {
        let label = window.label();
        let is_legacy_category_window = label
            .strip_prefix("desktop-category-")
            .and_then(|suffix| suffix.split_once('-'))
            .and_then(|(index, _)| index.parse::<usize>().ok())
            .is_some();
        if is_legacy_category_window {
            let _ = window.hide();
        }
    }

    Ok(split_indices)
}

#[tauri::command]
async fn merge_desktop_widgets(app: tauri::AppHandle) -> Result<(), String> {
    for window in app.webview_windows().values() {
        let label = window.label();
        if label.starts_with("desktop-category-") || label == "desktop-launcher" {
            let _ = window.hide();
        }
    }
    show_merged_desktop_widget(&app)?;
    show_desktop_launcher(&app)
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
fn open_search_item(app: tauri::AppHandle, item: SearchItem) -> Result<(), String> {
    let path = item.path.trim();
    if path.is_empty() {
        return Err("路径为空".to_owned());
    }

    if item.kind == SearchItemKind::Directory || item.is_dir {
        open_path_impl(Path::new(path))?;
    } else {
        open_with_shell(path)?;
        record_search_open(&item)?;
    }

    hide_search_overlay_impl(&app);
    Ok(())
}

#[tauri::command]
fn hide_search_overlay(app: tauri::AppHandle) -> Result<(), String> {
    hide_search_overlay_impl(&app);
    Ok(())
}

#[tauri::command]
fn hide_current_window(window: WebviewWindow) -> Result<(), String> {
    window.hide().map_err(to_message)
}

#[tauri::command]
async fn hide_main_window_to_tray(app: tauri::AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || hide_main_window_impl(&app))
        .await
        .map_err(to_message)?
}

#[tauri::command]
fn cleanup_desktop_card_windows(app: tauri::AppHandle) -> Result<(), String> {
    cleanup_desktop_card_windows_impl(&app);
    Ok(())
}

#[tauri::command]
fn repaint_current_window(window: WebviewWindow) -> Result<(), String> {
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
}

#[tauri::command]
async fn paste_clipboard_item(app: tauri::AppHandle, id: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        clipboard_bridge::paste_history_item(&id, || hide_clipboard_overlay_impl(&app))
    })
        .await
        .map_err(to_message)?
}

#[tauri::command]
fn hide_clipboard_overlay(app: tauri::AppHandle) -> Result<(), String> {
    hide_clipboard_overlay_impl(&app);
    Ok(())
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
        let mut config = store.load_config();
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
        let mut config = store.load_config();

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

fn with_config_mutation<T>(operation: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    let _guard = CONFIG_MUTATION_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| "配置写入锁已损坏".to_owned())?;
    operation()
}

fn with_launcher_mutation<T>(operation: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    let _guard = LAUNCHER_MUTATION_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| "快捷启动写入锁已损坏".to_owned())?;
    operation()
}

fn mutate_categories(
    mutator: impl FnOnce(&mut Vec<DeskCategory>) -> Result<(), String>,
) -> Result<(), String> {
    with_config_mutation(|| {
        let store = AppStore::open().map_err(to_message)?;
        let mut config = store.load_config();
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

fn restore_all_organized_items_to_desktop() -> Result<usize, String> {
    with_config_mutation(|| {
        let store = AppStore::open().map_err(to_message)?;
        store.ensure_runtime_dirs().map_err(to_message)?;
        let desktop = user_desktop().ok_or_else(|| "没有找到桌面路径".to_owned())?;
        fs::create_dir_all(&desktop).map_err(to_message)?;
        let organizer_root = store.organizer_root();
        let mut config = store.load_config();
        let mut restored = 0usize;
        let mut handled_paths = Vec::<String>::new();

        for category in &mut config.desktop_categories {
            let old_paths = std::mem::take(&mut category.item_paths);
            let mut retained_paths = Vec::new();

            for item_path in old_paths {
                let source = PathBuf::from(&item_path);
                if !source.exists() {
                    continue;
                }

                if is_path_within(&source, &organizer_root) || is_path_within(&source, &desktop) {
                    let normalized = normalize_path_for_compare(&source);
                    if !handled_paths.iter().any(|path| path == &normalized) {
                        let restored_path = restore_path_to_desktop(&source, &desktop)?;
                        handled_paths.push(normalized);
                        handled_paths.push(normalize_path_for_compare(&restored_path));
                        push_unique_category_path(
                            &mut retained_paths,
                            restored_path.display().to_string(),
                        );
                        restored += 1;
                    }
                } else {
                    // External references are not migrated desktop items, so do not move them on exit.
                    push_unique_category_path(&mut retained_paths, item_path);
                }
            }

            category.item_paths = retained_paths;
        }

        for path in organizer_contents(&organizer_root) {
            if !path.exists() {
                continue;
            }
            let normalized = normalize_path_for_compare(&path);
            if handled_paths.iter().any(|item| item == &normalized) {
                continue;
            }
            let restored_path = restore_path_to_desktop(&path, &desktop)?;
            handled_paths.push(normalized);
            handled_paths.push(normalize_path_for_compare(&restored_path));
            if let Some(index) =
                category_index_for_organizer_path(&path, &organizer_root, &config.desktop_categories)
            {
                if let Some(category) = config.desktop_categories.get_mut(index) {
                    push_unique_category_path(
                        &mut category.item_paths,
                        restored_path.display().to_string(),
                    );
                }
            }
            restored += 1;
        }

        cleanup_empty_organizer_dirs(&organizer_root);
        store.save_config(&config).map_err(to_message)?;
        Ok(restored)
    })
}

fn archive_marked_desktop_items_on_startup() -> Result<(), String> {
    with_config_mutation(|| {
        let store = AppStore::open().map_err(to_message)?;
        store.ensure_runtime_dirs().map_err(to_message)?;
        let mut config = store.load_config();
        repair_category_item_paths(&store, &mut config)
    })
}

fn push_unique_category_path(paths: &mut Vec<String>, path: String) {
    if paths
        .iter()
        .any(|existing| same_path_text(existing, &path))
    {
        return;
    }
    paths.push(path);
}

fn category_index_for_organizer_path(
    path: &Path,
    organizer_root: &Path,
    categories: &[DeskCategory],
) -> Option<usize> {
    let parent = path.parent()?;
    if !is_path_within(parent, organizer_root) {
        return None;
    }
    let folder_name = parent.file_name()?.to_string_lossy();
    categories.iter().position(|category| {
        safe_windows_file_name(&category.name).eq_ignore_ascii_case(folder_name.as_ref())
    })
}

fn restore_path_to_desktop(source: &Path, desktop: &Path) -> Result<PathBuf, String> {
    let file_name = source
        .file_name()
        .ok_or_else(|| "无法识别项目名称".to_owned())?;

    if !source.exists() {
        return Ok(desktop.join(file_name));
    }

    let existing_desktop_item = desktop.join(file_name);
    if existing_desktop_item.exists() {
        if !same_path_text(
            &source.display().to_string(),
            &existing_desktop_item.display().to_string(),
        ) {
            remove_existing_path(source)?;
        }
        return Ok(existing_desktop_item);
    }

    if is_path_within(source, desktop) {
        return Ok(source.to_path_buf());
    }

    let destination = unique_destination(desktop, source);
    move_path(source, &destination)?;
    Ok(destination)
}

fn organizer_contents(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };

    let mut paths = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let Ok(children) = fs::read_dir(&path) else {
                continue;
            };
            for child in children.flatten() {
                paths.push(child.path());
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
            launcher.icon_data_url = system_icon::icon_data_url(path);
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
            icon_data_url: system_icon::icon_data_url(Path::new(&path)),
            path,
        })
        .collect())
}

fn desktop_items(include_icons: bool) -> Vec<DesktopItem> {
    let mut unique = BTreeMap::<String, DesktopItem>::new();
    for root in desktop_roots() {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
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
        item.icon_data_url = system_icon::icon_data_url(path);
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

fn same_path_text(left: &str, right: &str) -> bool {
    left.trim().eq_ignore_ascii_case(right.trim())
}

fn should_skip_desktop_classify_item(item: &DesktopItem) -> bool {
    let name = item.name.trim();
    let lower_name = name.to_lowercase();
    let extension = item.extension.trim().to_lowercase();

    if lower_name == "desktop" && extension == "ini" {
        return true;
    }

    matches!(
        name,
        "DustDesk 收纳箱" | "DustDesk 快捷启动" | "DustDesk 桌面收纳"
    )
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
        return find_category_index(categories, &["本机", "文件", "文档"])
            .or_else(|| find_category_index(categories, &["工具"]))
            .or(Some(0));
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

fn has_any(value: &str, keywords: &[&str]) -> bool {
    keywords.iter().any(|keyword| value.contains(keyword))
}

fn archive_item_path(store: &AppStore, category_name: &str, path: &str) -> Result<String, String> {
    let source = PathBuf::from(path);
    if !source.exists() {
        return Ok(path.to_owned());
    }

    let organizer_root = store.organizer_root();
    let is_organizer_item = is_path_within(&source, &organizer_root);
    if !is_desktop_child(&source) && !is_organizer_item {
        return Ok(path.to_owned());
    }

    let category_dir = organizer_root.join(safe_windows_file_name(category_name));
    fs::create_dir_all(&category_dir).map_err(to_message)?;
    if is_path_within(&source, &category_dir) {
        return Ok(path.to_owned());
    }

    let file_name = source
        .file_name()
        .ok_or_else(|| "无法识别项目名称".to_owned())?;
    let destination = category_dir.join(file_name);
    if destination.exists() {
        if !same_path_text(path, &destination.display().to_string()) {
            remove_existing_path(&source)?;
        }
        return Ok(destination.display().to_string());
    }

    move_path(&source, &destination)?;
    Ok(destination.display().to_string())
}

fn repair_category_item_paths(store: &AppStore, config: &mut AppConfig) -> Result<(), String> {
    let organizer_root = store.organizer_root();
    let mut changed = false;
    let mut seen_paths = Vec::<String>::new();

    for category in &mut config.desktop_categories {
        let old_paths = std::mem::take(&mut category.item_paths);
        let mut repaired_paths = Vec::with_capacity(old_paths.len());
        let category_name = category.name.clone();

        for item_path in old_paths {
            let source = PathBuf::from(&item_path);
            if !source.exists() {
                changed = true;
                continue;
            }

            let repaired_text =
                if is_desktop_child(&source) && !is_path_within(&source, &organizer_root) {
                    archive_item_path(store, &category_name, &item_path)?
                } else {
                    item_path
                };

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

    for category in &mut config.desktop_categories {
        let category_dir = organizer_root.join(safe_windows_file_name(&category.name));
        let Ok(entries) = fs::read_dir(&category_dir) else {
            continue;
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.eq_ignore_ascii_case("desktop.ini"))
            {
                continue;
            }

            let path_text = path.display().to_string();
            if seen_paths
                .iter()
                .any(|seen_path| same_path_text(seen_path, &path_text))
            {
                continue;
            }

            seen_paths.push(path_text.clone());
            category.item_paths.push(path_text);
            changed = true;
        }
    }

    if changed {
        store.save_config(config).map_err(to_message)?;
    }

    Ok(())
}

fn push_unique_text_path(paths: &mut Vec<String>, path: String) -> bool {
    if paths.iter().any(|existing| same_path_text(existing, &path)) {
        true
    } else {
        paths.push(path);
        false
    }
}

fn is_desktop_child(path: &Path) -> bool {
    desktop_roots()
        .iter()
        .any(|root| is_path_within(path, root))
}

fn is_path_within(path: &Path, root: &Path) -> bool {
    let path = normalize_path_for_compare(path);
    let root = normalize_path_for_compare(root);
    path == root || path.starts_with(&format!("{root}\\"))
}

fn normalize_path_for_compare(path: &Path) -> String {
    path.to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_ascii_lowercase()
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
    if fs::rename(source, destination).is_ok() {
        return Ok(());
    }

    if source.is_dir() {
        copy_dir_recursive(source, destination)?;
        fs::remove_dir_all(source).map_err(to_message)
    } else {
        fs::copy(source, destination).map_err(to_message)?;
        fs::remove_file(source).map_err(to_message)
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

fn copy_dir_recursive(source: &Path, destination: &Path) -> Result<(), String> {
    fs::create_dir_all(destination).map_err(to_message)?;
    for entry in fs::read_dir(source).map_err(to_message)? {
        let entry = entry.map_err(to_message)?;
        let from = entry.path();
        let to = destination.join(entry.file_name());
        let file_type = entry.file_type().map_err(to_message)?;
        if file_type.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else {
            fs::copy(&from, &to).map_err(to_message)?;
        }
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

        let mut stack = vec![root_path];
        while let Some(path) = stack.pop() {
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
                if is_noise_search_entry(&name) {
                    continue;
                }

                let path_text = entry_path.display().to_string();
                if matches_search_query(&name, &path_text, query_lower) {
                    let kind = if file_type.is_dir() {
                        SearchItemKind::Directory
                    } else {
                        SearchItemKind::File
                    };
                    let key = format!("path:{}", path_text.to_lowercase());
                    unique
                        .entry(key)
                        .or_insert_with(|| path_search_item(&entry_path, kind));
                }

                if file_type.is_dir() {
                    stack.push(entry_path);
                }
            }
        }
    }
}

fn search_history_items(history: &SearchHistoryData) -> Vec<SearchItem> {
    history
        .items
        .iter()
        .filter(|item| item.kind != SearchItemKind::Directory && !item.path.trim().is_empty())
        .map(search_item_from_history)
        .collect()
}

fn record_search_open(item: &SearchItem) -> Result<(), String> {
    if item.kind == SearchItemKind::Directory || item.is_dir {
        return Ok(());
    }

    let store = AppStore::open().map_err(to_message)?;
    let mut history = store.load_search_history();
    let key = item.path.to_lowercase();
    let now = now_local_string();

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
            id: now_id(),
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
    store.save_search_history(&history).map_err(to_message)
}

fn launcher_search_item(launcher: &LaunchItem, name: String) -> SearchItem {
    let path = launcher.path.trim().to_owned();
    let icon_data_url = launcher
        .icon_data_url
        .clone()
        .or_else(|| system_icon::icon_data_url(Path::new(&path)));
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
        icon_data_url: system_icon::icon_data_url(path),
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
        icon_data_url: system_icon::icon_data_url(Path::new(&item.path)),
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

fn is_noise_search_entry(name: &str) -> bool {
    matches!(
        name,
        "." | ".." | "node_modules" | ".git" | "target" | "dist"
    )
}

fn clipboard_preview(mut items: Vec<ClipboardHistoryItem>) -> Vec<ClipboardHistoryItem> {
    items.truncate(30);
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

fn create_windows_shortcut(
    shortcut: &Path,
    target: &Path,
    description: &str,
) -> Result<(), String> {
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

fn shortcut_matches(shortcut: &Shortcut, configured: &str) -> bool {
    configured
        .parse::<Shortcut>()
        .map(|expected| expected.id() == shortcut.id())
        .unwrap_or(false)
}

fn show_clipboard_overlay(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("clipboard") {
        let _ = place_clipboard_overlay(&window);
        let _ = window.show();
        let _ = window.set_focus();
        let _ = window.emit("dustdesk://clipboard-shortcut", ());
    }
}

fn show_search_overlay(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("search") {
        let _ = place_search_overlay(&window);
        let _ = window.show();
        let _ = window.set_focus();
        let _ = window.emit("dustdesk://search-shortcut", ());
    }
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

fn place_desktop_widget(window: &WebviewWindow) -> tauri::Result<()> {
    const MIN_WIDTH: u32 = 520;
    const MIN_HEIGHT: u32 = 320;

    let Some(monitor) = window.current_monitor()?.or(window.primary_monitor()?) else {
        return Ok(());
    };

    let work_area = monitor.work_area();
    let margin = 28i32;
    let default_panel_width = work_area.size.width.min(760).max(620);
    let default_panel_height = work_area.size.height.min(520).max(360);
    let current_size = window.inner_size()?;
    let max_panel_width = work_area
        .size
        .width
        .saturating_sub((margin * 2) as u32)
        .max(MIN_WIDTH);
    let max_panel_height = work_area
        .size
        .height
        .saturating_sub((margin * 2) as u32)
        .max(MIN_HEIGHT);
    let panel_width = if current_size.width >= MIN_WIDTH {
        current_size.width.clamp(MIN_WIDTH, max_panel_width)
    } else {
        default_panel_width.clamp(MIN_WIDTH, max_panel_width)
    };
    let panel_height = if current_size.height >= MIN_HEIGHT {
        current_size.height.clamp(MIN_HEIGHT, max_panel_height)
    } else {
        default_panel_height.clamp(MIN_HEIGHT, max_panel_height)
    };
    let x = work_area.position.x + work_area.size.width as i32 - panel_width as i32 - margin;
    let y =
        work_area.position.y + ((work_area.size.height.saturating_sub(panel_height)) / 4) as i32;

    window.set_size(Size::Physical(PhysicalSize::new(panel_width, panel_height)))?;
    window.set_position(Position::Physical(PhysicalPosition::new(x, y)))?;
    Ok(())
}

fn show_or_create_desktop_card(
    app: &tauri::AppHandle,
    label: &str,
    title: &str,
    url: &str,
    index: usize,
) -> Result<(), String> {
    if let Some(window) = app.get_webview_window(label) {
        restore_desktop_card_size_if_collapsed(&window, label)?;
        return show_desktop_background_window(&window);
    }

    create_desktop_card(app, label, title, url, index)
}

fn restore_desktop_card_size_if_collapsed(
    window: &WebviewWindow,
    label: &str,
) -> Result<(), String> {
    const MIN_WIDTH: u32 = 240;
    const MIN_HEIGHT: u32 = 160;
    const DEFAULT_CATEGORY_WIDTH: u32 = 360;
    const DEFAULT_LAUNCHER_WIDTH: u32 = 320;
    const DEFAULT_HEIGHT: u32 = 260;

    let _ = window.set_min_size(Some(Size::Physical(PhysicalSize::new(
        MIN_WIDTH, MIN_HEIGHT,
    ))));

    let size = window.inner_size().map_err(to_message)?;
    if size.height >= MIN_HEIGHT {
        return Ok(());
    }

    let width = if label == "desktop-launcher" {
        DEFAULT_LAUNCHER_WIDTH
    } else {
        DEFAULT_CATEGORY_WIDTH
    };
    window
        .set_size(Size::Physical(PhysicalSize::new(width, DEFAULT_HEIGHT)))
        .map_err(to_message)
}

fn keep_desktop_window_behind_apps(window: &WebviewWindow) {
    let _ = window.set_always_on_top(false);
    let _ = window.set_always_on_bottom(true);
    let _ = window.set_skip_taskbar(true);
}

fn show_desktop_background_window(window: &WebviewWindow) -> Result<(), String> {
    keep_desktop_window_behind_apps(window);
    window.show().map_err(to_message)?;
    // Showing an existing Windows window can bump it in z-order, so reapply the
    // desktop-level placement after every show.
    keep_desktop_window_behind_apps(window);
    Ok(())
}

fn create_desktop_card(
    app: &tauri::AppHandle,
    label: &str,
    title: &str,
    url: &str,
    index: usize,
) -> Result<(), String> {
    let Some(monitor) = app.primary_monitor().map_err(to_message)? else {
        return Err("没有找到显示器".to_owned());
    };
    let work_area = monitor.work_area();
    let width = if label == "desktop-launcher" {
        320.0
    } else {
        360.0
    };
    let height = 260.0;
    let gap = 16.0;
    let columns = 3usize;
    let column = index % columns;
    let row = index / columns;
    let x = work_area.position.x as f64 + 28.0 + (width + gap) * column as f64;
    let y = work_area.position.y as f64 + 72.0 + (height + gap) * row as f64;

    WebviewWindowBuilder::new(app, label, WebviewUrl::App(url.into()))
        .title(title)
        .inner_size(width, height)
        .min_inner_size(240.0, 160.0)
        .position(x, y)
        .resizable(true)
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
        .and_then(|window| show_desktop_background_window(&window))
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

fn show_desktop_launcher(app: &tauri::AppHandle) -> Result<(), String> {
    let store = AppStore::open().map_err(to_message)?;
    store.ensure_runtime_dirs().map_err(to_message)?;
    let index = store.load_config().desktop_categories.len();
    show_or_create_desktop_card(
        app,
        "desktop-launcher",
        "DustDesk 快捷启动",
        &desktop_card_url("launcher", None),
        index,
    )
}

fn hide_desktop_category_windows(app: &tauri::AppHandle, index: usize) {
    for window in app.webview_windows().values() {
        if desktop_category_index_from_label(window.label()) == Some(index) {
            let _ = window.hide();
        }
    }
}

fn hide_desktop_organizer_windows(app: &tauri::AppHandle) {
    for window in app.webview_windows().values() {
        let label = window.label();
        if label == "desktop-widget" || label.starts_with("desktop-category-") {
            let _ = window.hide();
        }
    }
}

fn hide_desktop_launcher_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("desktop-launcher") {
        let _ = window.hide();
    }
}

fn hide_desktop_card_windows(app: &tauri::AppHandle) {
    for window in app.webview_windows().values() {
        if is_desktop_card_window_label(window.label()) {
            let _ = window.hide();
        }
    }
}

fn desktop_category_index_from_label(label: &str) -> Option<usize> {
    label
        .strip_prefix("desktop-category-")?
        .split('-')
        .next()?
        .parse::<usize>()
        .ok()
}

fn is_desktop_card_window_label(label: &str) -> bool {
    label == "desktop-widget"
        || label == "desktop-launcher"
        || label.starts_with("desktop-category-")
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
        if label == "desktop-widget" || label.starts_with("desktop-category-") {
            organizer = true;
        } else if label == "desktop-launcher" {
            launcher = true;
        }
    }

    DesktopFrameVisibility {
        organizer,
        launcher,
        any: organizer || launcher,
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
    if let Some(window) = app.get_webview_window("main") {
        window.show().map_err(to_message)?;
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
    Ok(())
}

fn hide_main_window_to_tray_async(app: tauri::AppHandle) {
    tauri::async_runtime::spawn_blocking(move || {
        if let Err(error) = hide_main_window_impl(&app) {
            eprintln!("failed to hide main window to tray: {error}");
        }
    });
}

fn show_main_window_from_tray_async(app: tauri::AppHandle) {
    tauri::async_runtime::spawn_blocking(move || {
        if let Err(error) = show_main_window_impl(&app) {
            eprintln!("failed to show main window from tray: {error}");
        }
    });
}

fn request_real_app_exit(app: tauri::AppHandle) {
    if REAL_EXIT_REQUESTED.swap(true, Ordering::SeqCst) {
        return;
    }

    tauri::async_runtime::spawn_blocking(move || {
        if let Err(error) = restore_all_organized_items_to_desktop() {
            eprintln!("failed to restore organized desktop items on exit: {error}");
        }
        cleanup_desktop_card_windows_impl(&app);
        app.exit(0);
    });
}

fn setup_system_tray(app: &tauri::AppHandle) -> tauri::Result<()> {
    let show_item = MenuItem::with_id(app, TRAY_MENU_SHOW_MAIN, "显示主窗口", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, TRAY_MENU_QUIT, "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show_item, &quit_item])?;

    let mut tray = TrayIconBuilder::with_id("desknest-main-tray")
        .menu(&menu)
        .tooltip("DeskNest 后台运行")
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| {
            if event.id() == TRAY_MENU_SHOW_MAIN {
                show_main_window_from_tray_async(app.clone());
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
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    if event.state() == ShortcutState::Pressed {
                        let settings = configured_settings();
                        if shortcut_matches(shortcut, &settings.search_shortcut_value()) {
                            if settings.search_enabled {
                                show_search_overlay(app);
                            }
                            return;
                        }

                        if !shortcut_matches(shortcut, &settings.clipboard_shortcut_value()) {
                            return;
                        }

                        if !is_clipboard_overlay_visible(app) {
                            clipboard_bridge::remember_foreground_window();
                            clipboard_bridge::sync_current_clipboard_once();
                        }
                        show_clipboard_overlay(app);
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
            }
        })
        .setup(|app| {
            let app_handle = app.handle().clone();
            setup_system_tray(&app_handle)?;
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

            if let Err(error) = archive_marked_desktop_items_on_startup() {
                eprintln!("failed to archive marked desktop items on startup: {error}");
            }

            if let Some(window) = app.get_webview_window("desktop-widget") {
                let _ = place_desktop_widget(&window);
            }
            let handle = app.handle().clone();
            if let Err(error) =
                show_merged_desktop_widget(&handle).and_then(|_| show_desktop_launcher(&handle))
            {
                eprintln!("failed to show desktop cards: {error}");
            }
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
            add_item_to_category,
            add_items_to_category,
            remove_item_from_category,
            restore_item_to_desktop,
            add_launcher,
            add_launchers,
            remove_launcher,
            show_path_in_folder,
            classify_desktop_items,
            create_desktop_entries,
            open_special,
            open_path,
            start_all_launchers,
            show_desktop_widget,
            desktop_frame_visibility,
            toggle_desktop_frames,
            toggle_desktop_organizer_frame,
            toggle_desktop_launcher_frame,
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
            update_search_settings
        ])
        .run(tauri::generate_context!())
        .expect("failed to run DustDesk Tauri app");
}
