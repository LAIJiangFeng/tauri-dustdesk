//! Windows-desktop-like actions for items shown in desktop boxes: copy / cut files, copy paths,
//! rename, show the Shell properties dialog, delete category items to the Recycle Bin and
//! remove launcher entries.

use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::Serialize;

use crate::{
    categories_contain_path, clipboard_bridge, display_path_name, emit_desktop_cards_changed,
    is_internal_transfer_path, is_path_within, normalize_path_for_compare, normalize_path_input,
    recover_related_transfer_quarantine_journals_strict,
    recover_transfer_quarantine_journals_strict, remove_category_path_from_config,
    remove_desktop_icon_position_for_path, remove_launcher_impl, repair_category_item_paths,
    same_path_text, set_desktop_icon_position_for_path, to_message, with_config_mutation,
    with_launcher_mutation, AppStore,
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

#[derive(Debug, Serialize)]
pub struct CutPathsResult {
    pub count: usize,
    pub sequence: u32,
}

#[derive(Debug, Serialize)]
pub struct ClipboardCutStatus {
    pub clipboard_changed: bool,
    pub missing: Vec<String>,
}

#[tauri::command]
pub async fn cut_paths_to_clipboard(paths: Vec<String>) -> Result<CutPathsResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let paths = existing_unique_paths(paths)?;
        if paths.is_empty() {
            return Err("选中的项目都不存在，无法剪切".to_owned());
        }
        let store = AppStore::open().map_err(to_message)?;
        let launchers_root = store.launchers_root();
        // Pasting a cut launcher entry would move it out of Launchers, i.e. migrate it back to
        // the desktop, which launcher semantics forbid.
        if paths
            .iter()
            .any(|path| is_path_within(Path::new(path), &launchers_root))
        {
            return Err("快捷启动项不支持剪切".to_owned());
        }
        let config = store.load_config_strict().map_err(to_message)?;
        if !paths
            .iter()
            .all(|path| categories_contain_path(&config.desktop_categories, path))
        {
            return Err("只能剪切收纳分类里的项目".to_owned());
        }
        let sequence = clipboard_bridge::cut_files_to_clipboard(&paths)?;
        Ok(CutPathsResult {
            count: paths.len(),
            sequence,
        })
    })
    .await
    .map_err(to_message)?
}

/// Polled by the boxes while a cut is pending. Once a cut item has left its place (Explorer
/// pasted and moved it), every window is told to refresh, including same-named boxes on other
/// monitors that never polled.
#[tauri::command]
pub async fn clipboard_cut_status(
    app: tauri::AppHandle,
    paths: Vec<String>,
    sequence: u32,
) -> Result<ClipboardCutStatus, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let clipboard_changed = clipboard_bridge::clipboard_sequence_number()
            .is_some_and(|current| current != sequence);
        let missing = paths
            .into_iter()
            .filter(|path| fs::symlink_metadata(path).is_err())
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            emit_desktop_cards_changed(&app);
        }
        Ok(ClipboardCutStatus {
            clipboard_changed,
            missing,
        })
    })
    .await
    .map_err(to_message)?
}

#[tauri::command]
pub async fn rename_category_item(
    app: tauri::AppHandle,
    index: usize,
    path: String,
    new_name: String,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let new_path = rename_category_item_impl(index, path, &new_name)?;
        emit_desktop_cards_changed(&app);
        Ok(new_path)
    })
    .await
    .map_err(to_message)?
}

#[tauri::command]
pub async fn rename_launcher(
    app: tauri::AppHandle,
    path: String,
    new_name: String,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let new_path = rename_launcher_impl(path, &new_name)?;
        emit_desktop_cards_changed(&app);
        Ok(new_path)
    })
    .await
    .map_err(to_message)?
}

#[tauri::command]
pub async fn show_item_properties(paths: Vec<String>) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let paths = existing_unique_paths(paths)?;
        if paths.is_empty() {
            return Err("选中的项目都不存在".to_owned());
        }
        show_properties_dialog(&paths)
    })
    .await
    .map_err(to_message)?
}

fn rename_category_item_impl(index: usize, path: String, new_name: &str) -> Result<String, String> {
    with_config_mutation(|| {
        let store = AppStore::open().map_err(to_message)?;
        store.ensure_runtime_dirs().map_err(to_message)?;
        let path = normalize_path_input(&path)?;
        recover_transfer_quarantine_journals_strict(true)?;
        recover_related_transfer_quarantine_journals_strict(&[PathBuf::from(&path)])?;
        let mut config = store.load_config_strict().map_err(to_message)?;
        repair_category_item_paths(&store, &mut config)?;
        let category = config
            .desktop_categories
            .get(index)
            .ok_or_else(|| "分类不存在".to_owned())?;
        // Only paths recorded in this category are eligible, like delete.
        let position = category
            .item_paths
            .iter()
            .position(|item_path| same_path_text(item_path, &path))
            .ok_or_else(|| "该项目不在此分类中".to_owned())?;
        let old_text = category.item_paths[position].clone();
        let old_path = PathBuf::from(&old_text);
        ensure_renamable_item(&old_path)?;

        let new_path = rename_destination(&old_path, new_name)?;
        let new_text = new_path.display().to_string();
        if new_text == old_text {
            return Ok(old_text);
        }
        rename_in_place(&old_path, &new_path)?;

        // Keep the category order and move the remembered desktop icon position to the new
        // path, so "移回桌面" still puts the item back where it was.
        config.desktop_categories[index].item_paths[position] = new_text.clone();
        if let Some(icon_position) =
            remove_desktop_icon_position_for_path(&mut config.desktop_icon_positions, &old_text)
        {
            set_desktop_icon_position_for_path(
                &mut config.desktop_icon_positions,
                new_text.clone(),
                icon_position,
            );
        }
        if let Err(error) = store.save_config(&config) {
            let _ = rename_in_place(&new_path, &old_path);
            return Err(to_message(error));
        }
        Ok(new_text)
    })
}

/// Renames only the entry inside Launchers and its `launch.json` record; the shortcut target
/// or original file is never touched.
fn rename_launcher_impl(path: String, new_name: &str) -> Result<String, String> {
    with_launcher_mutation(|| {
        let path = normalize_path_input(&path)?;
        let store = AppStore::open().map_err(to_message)?;
        store.ensure_runtime_dirs().map_err(to_message)?;
        let launchers_root = store.launchers_root();
        let path_buf = PathBuf::from(&path);
        let directly_in_launchers = path_buf.parent().is_some_and(|parent| {
            normalize_path_for_compare(parent) == normalize_path_for_compare(&launchers_root)
        });
        if !directly_in_launchers {
            return Err("只能重命名快捷启动目录里的入口".to_owned());
        }
        let mut launchers = store.load_launchers_strict().map_err(to_message)?;
        let position = launchers
            .items
            .iter()
            .position(|item| same_path_text(&item.path, &path))
            .ok_or_else(|| "快捷启动记录不存在".to_owned())?;
        let old_text = launchers.items[position].path.clone();
        let old_path = PathBuf::from(&old_text);
        ensure_renamable_item(&old_path)?;

        let new_path = rename_destination(&old_path, new_name)?;
        let new_text = new_path.display().to_string();
        if new_text == old_text {
            return Ok(old_text);
        }
        rename_in_place(&old_path, &new_path)?;

        let item = &mut launchers.items[position];
        item.path = new_text.clone();
        item.name = display_path_name(&new_path);
        item.icon_data_url = None;
        if let Err(error) = store.save_launchers(&launchers) {
            let _ = rename_in_place(&new_path, &old_path);
            return Err(to_message(error));
        }
        Ok(new_text)
    })
}

fn ensure_renamable_item(path: &Path) -> Result<(), String> {
    if is_internal_transfer_path(path) || fs::symlink_metadata(path).is_err() {
        return Err("项目不存在，可能已被移动或删除".to_owned());
    }
    Ok(())
}

/// Validates `new_name` and returns the destination next to `old_path`. A name that differs
/// only in letter case from the current one is allowed; any other existing item with the same
/// name (case-insensitive) is a conflict.
fn rename_destination(old_path: &Path, new_name: &str) -> Result<PathBuf, String> {
    let name = validate_item_name(new_name)?;
    let parent = old_path
        .parent()
        .ok_or_else(|| "无法识别项目所在目录".to_owned())?;
    let old_name = old_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let new_path = parent.join(&name);
    if is_internal_transfer_path(&new_path) {
        return Err("不能使用 DustDesk 内部保留的名称".to_owned());
    }
    if same_name_ignoring_case(&name, &old_name) {
        return Ok(new_path);
    }
    let siblings = fs::read_dir(parent)
        .map_err(to_message)?
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned());
    if name_conflicts(&name, &old_name, siblings) || fs::symlink_metadata(&new_path).is_ok() {
        return Err("已存在同名项目".to_owned());
    }
    Ok(new_path)
}

const INVALID_NAME_CHARS: [char; 9] = ['\\', '/', ':', '*', '?', '"', '<', '>', '|'];
const RESERVED_DEVICE_NAMES: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Authoritative file name check for renames, mirroring Explorer; returns the trimmed name.
fn validate_item_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("名称不能为空".to_owned());
    }
    if name == "." || name == ".." {
        return Err("名称不能是 . 或 ..".to_owned());
    }
    if name
        .chars()
        .any(|ch| ch.is_control() || INVALID_NAME_CHARS.contains(&ch))
    {
        return Err(r#"名称不能包含 \ / : * ? " < > | 或控制字符"#.to_owned());
    }
    if name.ends_with('.') {
        return Err("名称不能以点号结尾".to_owned());
    }
    // Windows treats "CON", "con.txt" and "Con .log" alike: the part before the first dot,
    // with trailing spaces ignored, must not be a device name.
    let base = name.split('.').next().unwrap_or_default().trim_end();
    if RESERVED_DEVICE_NAMES
        .iter()
        .any(|reserved| reserved.eq_ignore_ascii_case(base))
    {
        return Err(format!("{base} 是 Windows 保留名称，不能使用"));
    }
    Ok(name.to_owned())
}

fn same_name_ignoring_case(left: &str, right: &str) -> bool {
    left.to_lowercase() == right.to_lowercase()
}

/// True when another sibling already uses `new_name`; the item being renamed itself
/// (`old_name`) never conflicts, so a case-only rename passes.
fn name_conflicts(
    new_name: &str,
    old_name: &str,
    siblings: impl IntoIterator<Item = String>,
) -> bool {
    if same_name_ignoring_case(new_name, old_name) {
        return false;
    }
    siblings
        .into_iter()
        .any(|sibling| same_name_ignoring_case(&sibling, new_name))
}

/// Same-directory rename that never replaces an existing item and never falls back to a copy
/// (`MoveFileExW` without `MOVEFILE_REPLACE_EXISTING` / `MOVEFILE_COPY_ALLOWED`).
#[cfg(windows)]
fn rename_in_place(from: &Path, to: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::MoveFileExW;

    let from_wide = from
        .as_os_str()
        .encode_wide()
        .chain([0])
        .collect::<Vec<u16>>();
    let to_wide = to
        .as_os_str()
        .encode_wide()
        .chain([0])
        .collect::<Vec<u16>>();
    if unsafe { MoveFileExW(from_wide.as_ptr(), to_wide.as_ptr(), 0) } == 0 {
        let error = std::io::Error::last_os_error();
        return Err(match error.raw_os_error() {
            // ERROR_FILE_EXISTS / ERROR_ALREADY_EXISTS
            Some(80 | 183) => "已存在同名项目".to_owned(),
            // ERROR_ACCESS_DENIED / ERROR_SHARING_VIOLATION
            Some(5 | 32) => "项目正在被使用或没有权限，无法重命名".to_owned(),
            _ => format!("重命名失败：{error}"),
        });
    }
    Ok(())
}

#[cfg(not(windows))]
fn rename_in_place(from: &Path, to: &Path) -> Result<(), String> {
    fs::rename(from, to).map_err(to_message)
}

#[cfg(not(windows))]
fn show_properties_dialog(_paths: &[String]) -> Result<(), String> {
    Err("当前平台暂不支持查看属性".to_owned())
}

/// Opens the Shell's own properties dialog, the same one as the desktop's "属性".
///
/// `SHObjectProperties` / `SHMultiFileProperties` create the property sheet on a thread of
/// their own inside this process and return right away, so this blocking-pool thread is free
/// again at once and the WebView never waits on the dialog. The Shell item and data-object APIs
/// are COM-based and expect an apartment-threaded caller, hence
/// `CoInitializeEx(COINIT_APARTMENTTHREADED)` first; pool threads are reused by unrelated tasks,
/// so the matching `CoUninitialize` runs before returning. The sheet thread holds its own
/// reference to the data object, so releasing ours right after the call is safe.
#[cfg(windows)]
fn show_properties_dialog(paths: &[String]) -> Result<(), String> {
    use windows_sys::Win32::System::Com::{
        CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED,
    };

    let before = properties_windows::process_dialog_windows();
    let initialized = unsafe { CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32) };
    // S_OK and S_FALSE both need a matching CoUninitialize; RPC_E_CHANGED_MODE does not.
    let needs_uninitialize = initialized >= 0;
    let result = if paths.len() == 1 {
        properties_windows::show_single(&paths[0])
    } else {
        properties_windows::show_multiple(paths).or_else(|error| {
            eprintln!("multi-file properties failed, showing the first item: {error}");
            properties_windows::show_single(&paths[0])
        })
    };
    if needs_uninitialize {
        unsafe { CoUninitialize() };
    }
    result?;
    properties_windows::bring_new_dialog_to_front(before);
    Ok(())
}

#[cfg(windows)]
mod properties_windows {
    use std::{
        ffi::c_void,
        path::Path,
        ptr::{null, null_mut},
        thread,
        time::{Duration, Instant},
    };

    use windows_sys::{
        core::{BOOL, GUID},
        Win32::{
            Foundation::{HWND, LPARAM},
            System::Threading::GetCurrentProcessId,
            UI::{
                Shell::{
                    Common::ITEMIDLIST, ILCreateFromPathW, ILFree, SHCreateDataObject,
                    SHMultiFileProperties, SHObjectProperties, SHOP_FILEPATH,
                },
                WindowsAndMessaging::{
                    BringWindowToTop, EnumWindows, GetClassNameW, GetWindowThreadProcessId,
                    IsWindowVisible, SetForegroundWindow,
                },
            },
        },
    };

    use super::plain_win32_path;

    const IID_IDATAOBJECT: GUID = GUID::from_u128(0x0000010e_0000_0000_c000_000000000046);
    const DIALOG_WAIT_TIMEOUT: Duration = Duration::from_secs(5);
    const DIALOG_WAIT_STEP: Duration = Duration::from_millis(50);

    pub fn show_single(path: &str) -> Result<(), String> {
        let path = wide(path);
        let shown =
            unsafe { SHObjectProperties(null_mut(), SHOP_FILEPATH as u32, path.as_ptr(), null()) };
        if shown == 0 {
            return Err("无法打开属性对话框".to_owned());
        }
        Ok(())
    }

    /// Combined properties of several items: absolute PIDLs (`ILCreateFromPathW`) wrapped in a
    /// data object without a parent folder (`SHCreateDataObject(NULL, ...)`), which also works
    /// for items from different folders.
    pub fn show_multiple(paths: &[String]) -> Result<(), String> {
        let mut pidls = Vec::<*mut ITEMIDLIST>::with_capacity(paths.len());
        for path in paths {
            let pidl = unsafe { ILCreateFromPathW(wide(path).as_ptr()) };
            if pidl.is_null() {
                free_pidls(&pidls);
                return Err(format!("无法解析项目：{path}"));
            }
            pidls.push(pidl);
        }

        let mut data_object = null_mut::<c_void>();
        let created = unsafe {
            SHCreateDataObject(
                null(),
                pidls.len() as u32,
                pidls.as_ptr().cast(),
                null_mut(),
                &IID_IDATAOBJECT,
                &mut data_object,
            )
        };
        free_pidls(&pidls);
        if created < 0 || data_object.is_null() {
            return Err(format!("无法创建数据对象（{created:#x}）"));
        }
        let shown = unsafe { SHMultiFileProperties(data_object, 0) };
        unsafe { release(data_object) };
        if shown < 0 {
            return Err(format!("无法打开属性对话框（{shown:#x}）"));
        }
        Ok(())
    }

    /// The sheet is a new top-level dialog of this process. Desktop boxes stay at the bottom of
    /// the z-order, so the dialog is explicitly brought to the foreground once it appears; this
    /// process owns the foreground right after the user's click or key press, so the call is
    /// allowed. The wait runs on its own thread so the command returns immediately.
    pub fn bring_new_dialog_to_front(before: Vec<isize>) {
        thread::spawn(move || {
            let started = Instant::now();
            while started.elapsed() < DIALOG_WAIT_TIMEOUT {
                let fresh = process_dialog_windows()
                    .into_iter()
                    .filter(|hwnd| !before.contains(hwnd))
                    .collect::<Vec<_>>();
                if !fresh.is_empty() {
                    for hwnd in fresh {
                        unsafe {
                            BringWindowToTop(hwnd as HWND);
                            SetForegroundWindow(hwnd as HWND);
                        }
                    }
                    return;
                }
                thread::sleep(DIALOG_WAIT_STEP);
            }
        });
    }

    /// Visible top-level dialog-class (`#32770`) windows of this process.
    pub fn process_dialog_windows() -> Vec<isize> {
        let mut windows = Vec::<isize>::new();
        unsafe {
            EnumWindows(
                Some(collect_dialog_window),
                &mut windows as *mut Vec<isize> as LPARAM,
            );
        }
        windows
    }

    unsafe extern "system" fn collect_dialog_window(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let windows = unsafe { &mut *(lparam as *mut Vec<isize>) };
        let mut process_id = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd, &mut process_id) };
        if process_id != unsafe { GetCurrentProcessId() } || unsafe { IsWindowVisible(hwnd) } == 0 {
            return 1;
        }
        let mut class_name = [0u16; 16];
        let length =
            unsafe { GetClassNameW(hwnd, class_name.as_mut_ptr(), class_name.len() as i32) };
        if String::from_utf16_lossy(&class_name[..length.max(0) as usize]) == "#32770" {
            windows.push(hwnd as isize);
        }
        1
    }

    fn free_pidls(pidls: &[*mut ITEMIDLIST]) {
        for pidl in pidls {
            unsafe { ILFree(*pidl) };
        }
    }

    /// Calls `IUnknown::Release`, the third slot of every COM interface vtable.
    unsafe fn release(object: *mut c_void) {
        let vtable = unsafe { *(object as *const *const usize) };
        let release: unsafe extern "system" fn(*mut c_void) -> u32 =
            unsafe { std::mem::transmute(*vtable.add(2)) };
        unsafe { release(object) };
    }

    fn wide(path: &str) -> Vec<u16> {
        plain_win32_path(Path::new(path))
            .encode_utf16()
            .chain([0])
            .collect()
    }
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

#[cfg(test)]
mod tests {
    use super::{name_conflicts, validate_item_name};

    #[test]
    fn rejects_empty_and_dot_names() {
        for name in ["", "   ", ".", "..", " .. "] {
            assert!(
                validate_item_name(name).is_err(),
                "{name:?} should be rejected"
            );
        }
    }

    #[test]
    fn rejects_invalid_and_control_characters() {
        for name in [
            r"a\b", "a/b", "a:b", "a*b", "a?b", r#"a"b"#, "a<b", "a>b", "a|b", "a\tb", "a\u{1}b",
        ] {
            assert!(
                validate_item_name(name).is_err(),
                "{name:?} should be rejected"
            );
        }
    }

    #[test]
    fn rejects_trailing_dot_and_trims_spaces() {
        assert!(validate_item_name("report.").is_err());
        assert!(validate_item_name("report. ").is_err());
        assert_eq!(validate_item_name("  report.txt  ").unwrap(), "report.txt");
        assert_eq!(validate_item_name(".gitignore").unwrap(), ".gitignore");
        assert_eq!(
            validate_item_name("报告 v1.2.docx").unwrap(),
            "报告 v1.2.docx"
        );
    }

    #[test]
    fn rejects_reserved_device_names_with_or_without_extension() {
        for name in [
            "CON",
            "con",
            "Prn.txt",
            "aux.tar.gz",
            "NUL",
            "com1",
            "COM9.log",
            "lpt1",
            "LPT9",
            "Con .txt",
        ] {
            assert!(
                validate_item_name(name).is_err(),
                "{name:?} should be rejected"
            );
        }
        for name in [
            "CONSOLE",
            "com10",
            "lpt",
            "auxiliary.txt",
            "my con.txt",
            "COM0",
        ] {
            assert!(
                validate_item_name(name).is_ok(),
                "{name:?} should be allowed"
            );
        }
    }

    #[test]
    fn same_name_conflicts_ignore_case_but_case_only_rename_is_allowed() {
        let siblings = || vec!["Report.txt".to_owned(), "notes.txt".to_owned()];
        assert!(name_conflicts("NOTES.TXT", "Report.txt", siblings()));
        assert!(name_conflicts("notes.txt", "Report.txt", siblings()));
        assert!(!name_conflicts("report.TXT", "Report.txt", siblings()));
        assert!(!name_conflicts("summary.txt", "Report.txt", siblings()));
    }
}
