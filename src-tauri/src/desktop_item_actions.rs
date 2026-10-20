//! Windows-desktop-like actions for items shown in desktop boxes: copy files, copy paths,
//! delete category items to the Recycle Bin and remove launcher entries.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::{
    clipboard_bridge, emit_desktop_cards_changed, is_internal_transfer_path,
    normalize_path_for_compare, normalize_path_input,
    recover_related_transfer_quarantine_journals_strict,
    recover_transfer_quarantine_journals_strict, remove_category_path_from_config,
    remove_desktop_icon_position_for_path, remove_launcher_impl, repair_category_item_paths,
    same_path_text, to_message, with_config_mutation, AppStore,
};

#[derive(Debug, Default, Serialize)]
pub struct RecycleCategoryItemsResult {
    pub recycled: usize,
    pub skipped: usize,
    pub errors: Vec<String>,
}

#[tauri::command]
pub async fn copy_paths_to_clipboard(paths: Vec<String>) -> Result<usize, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let paths = existing_unique_paths(paths)?;
        if paths.is_empty() {
            return Err("选中的项目都不存在，无法复制".to_owned());
        }
        clipboard_bridge::copy_files_to_clipboard(&paths)?;
        Ok(paths.len())
    })
    .await
    .map_err(to_message)?
}

#[tauri::command]
pub async fn copy_text_to_clipboard(text: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || clipboard_bridge::copy_text_to_clipboard(&text))
        .await
        .map_err(to_message)?
}

#[tauri::command]
pub async fn delete_category_items_to_recycle_bin(
    app: tauri::AppHandle,
    index: usize,
    paths: Vec<String>,
) -> Result<RecycleCategoryItemsResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let result = delete_category_items_to_recycle_bin_impl(index, paths);
        emit_desktop_cards_changed(&app);
        result
    })
    .await
    .map_err(to_message)?
}

#[tauri::command]
pub async fn remove_launchers(app: tauri::AppHandle, paths: Vec<String>) -> Result<usize, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut removed = 0usize;
        let mut first_error = None;
        for path in paths {
            match remove_launcher_impl(path) {
                Ok(()) => removed += 1,
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        emit_desktop_cards_changed(&app);
        match first_error {
            Some(error) if removed == 0 => Err(error),
            _ => Ok(removed),
        }
    })
    .await
    .map_err(to_message)?
}

fn existing_unique_paths(paths: Vec<String>) -> Result<Vec<String>, String> {
    let mut seen = Vec::<String>::new();
    let mut result = Vec::new();
    for path in paths {
        let path = normalize_path_input(&path)?;
        let path_buf = Path::new(&path);
        if path.trim().is_empty() || !path_buf.exists() || is_internal_transfer_path(path_buf) {
            continue;
        }
        let key = normalize_path_for_compare(path_buf);
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        result.push(path);
    }
    Ok(result)
}

fn delete_category_items_to_recycle_bin_impl(
    index: usize,
    paths: Vec<String>,
) -> Result<RecycleCategoryItemsResult, String> {
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
        let category_paths = config
            .desktop_categories
            .get(index)
            .ok_or_else(|| "分类不存在".to_owned())?
            .item_paths
            .clone();

        let mut result = RecycleCategoryItemsResult::default();
        let mut seen = Vec::<String>::new();
        for path in paths {
            let path_buf = PathBuf::from(&path);
            let key = normalize_path_for_compare(&path_buf);
            if seen.contains(&key) {
                continue;
            }
            seen.push(key);

            // Only paths recorded in this category are eligible, so a forged or stale request
            // can never recycle arbitrary files.
            let Some(config_path) = category_paths
                .iter()
                .find(|item_path| same_path_text(item_path, &path))
                .cloned()
            else {
                result.skipped += 1;
                continue;
            };
            if is_internal_transfer_path(&path_buf) || !path_buf.exists() {
                result.skipped += 1;
                continue;
            }

            match recycle_path(&path_buf) {
                Ok(()) => {
                    remove_category_path_from_config(&mut config, index, &config_path)?;
                    remove_desktop_icon_position_for_path(
                        &mut config.desktop_icon_positions,
                        &config_path,
                    );
                    result.recycled += 1;
                }
                Err(error) => {
                    result.skipped += 1;
                    result
                        .errors
                        .push(format!("{}：{error}", display_name(&path_buf)));
                }
            }
        }

        if result.recycled > 0 {
            store.save_config(&config).map_err(to_message)?;
        }
        Ok(result)
    })
}

fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(not(windows))]
fn recycle_path(_path: &Path) -> Result<(), String> {
    Err("当前平台暂不支持移到回收站".to_owned())
}

/// Moves one item to the Recycle Bin, never deleting it permanently on its own.
///
/// Flags for `SHFileOperationW(FO_DELETE)`:
/// - `FOF_ALLOWUNDO`: delete with undo semantics, i.e. move to the Recycle Bin like the desktop.
/// - `FOF_NOCONFIRMATION`: the box already asked via `window.confirm`; skip the Shell's own
///   "move to Recycle Bin?" prompt.
/// - `FOF_WANTNUKEWARNING`: overrides `FOF_NOCONFIRMATION` for items that cannot be recycled
///   (too large, drive without Recycle Bin), so the Shell asks before destroying anything
///   instead of silently deleting it permanently.
/// - `FOF_NOERRORUI`: errors come back as a return code and are reported in the box notice.
///
/// Drives that report no Recycle Bin are rejected before the call, and the item is only
/// counted as recycled when the operation succeeded, was not aborted and the path is gone.
#[cfg(windows)]
fn recycle_path(path: &Path) -> Result<(), String> {
    use windows_sys::Win32::UI::Shell::{
        SHFileOperationW, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_WANTNUKEWARNING,
        FO_DELETE, SHFILEOPSTRUCTW,
    };

    // The Shell APIs expect plain Win32 paths, not `\\?\` extended-length ones.
    let path_text = plain_win32_path(path);
    if !drive_has_recycle_bin(Path::new(&path_text)) {
        return Err("所在磁盘没有回收站，已保留".to_owned());
    }

    // pFrom is a double-NUL terminated list.
    let from = path_text.encode_utf16().chain([0, 0]).collect::<Vec<u16>>();
    let mut operation = SHFILEOPSTRUCTW {
        wFunc: FO_DELETE,
        pFrom: from.as_ptr(),
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_WANTNUKEWARNING | FOF_NOERRORUI) as u16,
        ..Default::default()
    };
    let code = unsafe { SHFileOperationW(&mut operation) };
    if operation.fAnyOperationsAborted != 0 {
        return Err("已取消，未删除".to_owned());
    }
    if code != 0 {
        return Err(format!("无法移到回收站（错误码 {code:#x}），已保留"));
    }
    if path.exists() {
        return Err("未能移到回收站，已保留".to_owned());
    }
    Ok(())
}

#[cfg(windows)]
fn plain_win32_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = text.strip_prefix(r"\\?\") {
        rest.to_owned()
    } else {
        text.into_owned()
    }
}

#[cfg(windows)]
fn drive_has_recycle_bin(path: &Path) -> bool {
    use std::path::Component;
    use windows_sys::Win32::UI::Shell::{SHQueryRecycleBinW, SHQUERYRBINFO};

    let mut root = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => root.push(component.as_os_str()),
            _ => break,
        }
    }
    if root.as_os_str().is_empty() {
        return false;
    }
    let root = root
        .to_string_lossy()
        .encode_utf16()
        .chain([0])
        .collect::<Vec<u16>>();
    let mut info = SHQUERYRBINFO {
        cbSize: std::mem::size_of::<SHQUERYRBINFO>() as u32,
        ..Default::default()
    };
    unsafe { SHQueryRecycleBinW(root.as_ptr(), &mut info) >= 0 }
}
