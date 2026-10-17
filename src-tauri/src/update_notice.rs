//! Background update checks, the bottom-right update notice window and the tray update entry.
//!
//! Auto checks run on a dedicated background thread; network failures are only logged.

use std::{
    collections::BTreeSet,
    sync::{Mutex, OnceLock},
    thread,
    time::Duration,
};

use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    Emitter, Manager, PhysicalPosition, PhysicalSize, Position, Size, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder,
};

use crate::{
    check_for_updates_impl, configured_settings, is_trusted_update_url, open_with_shell,
    to_message, with_config_mutation, with_lazy_window_operation, with_webview_window_creation,
    AppSettings, AppStore, AppUpdateInfo,
};

pub const TRAY_MENU_UPDATE: &str = "open-update-download";
pub const UPDATE_NOTICE_CHANGED_EVENT: &str = "dustdesk://update-notice-changed";
const UPDATE_NOTICE_LABEL: &str = "update-notice";
const AUTO_CHECK_INITIAL_DELAY: Duration = Duration::from_secs(15);
const AUTO_CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
const NOTICE_WIDTH: f64 = 360.0;
const NOTICE_HEIGHT: f64 = 170.0;
const NOTICE_MARGIN: f64 = 16.0;

static UPDATE_STATE: OnceLock<Mutex<UpdateState>> = OnceLock::new();
static TRAY_UPDATE_MENU: OnceLock<Mutex<TrayUpdateMenu>> = OnceLock::new();

#[derive(Default)]
struct UpdateState {
    latest: Option<AppUpdateInfo>,
    // Versions already announced in this run; "稍后提醒" relies on this to stay quiet until restart.
    notified_versions: BTreeSet<String>,
}

struct TrayUpdateMenu {
    menu: Menu<tauri::Wry>,
    entry: Option<(MenuItem<tauri::Wry>, PredefinedMenuItem<tauri::Wry>)>,
}

fn update_state() -> &'static Mutex<UpdateState> {
    UPDATE_STATE.get_or_init(|| Mutex::new(UpdateState::default()))
}

/// Version compared against the latest release. Debug builds may override it with
/// `DUSTDESK_FAKE_CURRENT_VERSION` to exercise the notice flow.
pub fn current_version() -> String {
    #[cfg(debug_assertions)]
    if let Ok(version) = std::env::var("DUSTDESK_FAKE_CURRENT_VERSION") {
        let version = version.trim();
        if !version.is_empty() {
            return version.to_owned();
        }
    }
    env!("CARGO_PKG_VERSION").to_owned()
}

pub fn register_tray_menu(menu: Menu<tauri::Wry>) {
    let _ = TRAY_UPDATE_MENU.set(Mutex::new(TrayUpdateMenu { menu, entry: None }));
}

pub fn start_auto_check(app: tauri::AppHandle) {
    let spawned = thread::Builder::new()
        .name("dustdesk-update-check".to_owned())
        .spawn(move || {
            thread::sleep(AUTO_CHECK_INITIAL_DELAY);
            loop {
                run_auto_check(&app);
                thread::sleep(AUTO_CHECK_INTERVAL);
            }
        });
    if let Err(error) = spawned {
        eprintln!("failed to start update check thread: {error}");
    }
}

fn run_auto_check(app: &tauri::AppHandle) {
    let settings = configured_settings();
    if !settings.auto_check_updates {
        return;
    }
    let info = match check_for_updates_impl() {
        Ok(info) => info,
        Err(error) => {
            eprintln!("automatic update check failed: {error}");
            return;
        }
    };
    record_update(app, &info);
    if !info.update_available || is_skipped(&settings, &info.latest_version) {
        return;
    }
    let first_notice = update_state()
        .lock()
        .map(|mut state| state.notified_versions.insert(info.latest_version.clone()))
        .unwrap_or(false);
    if first_notice {
        if let Err(error) = with_lazy_window_operation(|| show_update_notice_impl(app)) {
            eprintln!("failed to show update notice: {error}");
        }
    }
}

fn is_skipped(settings: &AppSettings, version: &str) -> bool {
    let skipped = settings.skipped_update_version.trim();
    !skipped.is_empty() && skipped == version
}

/// Remembers the latest check result (auto or manual) and refreshes the tray entry.
pub fn record_update(app: &tauri::AppHandle, info: &AppUpdateInfo) {
    let latest = info.update_available.then(|| info.clone());
    if let Ok(mut state) = update_state().lock() {
        state.latest = latest.clone();
    }
    sync_tray_entry(app);
    let _ = app.emit(UPDATE_NOTICE_CHANGED_EVENT, latest);
}

fn latest_update() -> Option<AppUpdateInfo> {
    update_state()
        .lock()
        .ok()
        .and_then(|state| state.latest.clone())
}

fn sync_tray_entry(app: &tauri::AppHandle) {
    let Some(tray) = TRAY_UPDATE_MENU.get() else {
        return;
    };
    let Ok(mut tray) = tray.lock() else {
        return;
    };
    let settings = configured_settings();
    let pending = latest_update().filter(|info| !is_skipped(&settings, &info.latest_version));

    match (pending, tray.entry.as_ref()) {
        (Some(info), Some((item, _))) => {
            let _ = item.set_text(tray_entry_text(&info));
        }
        (Some(info), None) => {
            let item = MenuItem::with_id(
                app,
                TRAY_MENU_UPDATE,
                tray_entry_text(&info),
                true,
                None::<&str>,
            );
            let separator = PredefinedMenuItem::separator(app);
            if let (Ok(item), Ok(separator)) = (item, separator) {
                if tray.menu.insert(&item, 0).is_ok() {
                    let _ = tray.menu.insert(&separator, 1);
                    tray.entry = Some((item, separator));
                }
            }
        }
        (None, Some(_)) => {
            if let Some((item, separator)) = tray.entry.take() {
                let _ = tray.menu.remove(&item);
                let _ = tray.menu.remove(&separator);
            }
        }
        (None, None) => {}
    }
}

fn tray_entry_text(info: &AppUpdateInfo) -> String {
    format!("发现新版本 {}，点击更新", info.latest_version)
}

pub fn open_update_from_tray_async(app: tauri::AppHandle) {
    tauri::async_runtime::spawn_blocking(move || {
        let Some(info) = latest_update() else {
            return;
        };
        if let Err(error) = open_download(&info.download_url) {
            eprintln!("failed to open update download from tray: {error}");
            return;
        }
        let _ = with_lazy_window_operation(|| {
            close_update_notice_impl(&app);
            Ok(())
        });
    });
}

fn open_download(download_url: &str) -> Result<(), String> {
    let trimmed = download_url.trim();
    if !is_trusted_update_url(trimmed) {
        return Err("更新下载地址不可信".to_owned());
    }
    open_with_shell(trimmed)
}

fn create_update_notice(app: &tauri::AppHandle) -> Result<WebviewWindow, String> {
    with_webview_window_creation(|| {
        WebviewWindowBuilder::new(
            app,
            UPDATE_NOTICE_LABEL,
            WebviewUrl::App("index.html#/update-notice".into()),
        )
        .title("DustDesk 更新")
        .inner_size(NOTICE_WIDTH, NOTICE_HEIGHT)
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

fn show_update_notice_impl(app: &tauri::AppHandle) -> Result<(), String> {
    // The notice is destroyed on dismiss, so every appearance goes through a fresh
    // `focused(false)` creation and never steals focus via `show()`.
    let window = match app.get_webview_window(UPDATE_NOTICE_LABEL) {
        Some(window) => window,
        None => create_update_notice(app)?,
    };
    place_update_notice(app, &window).map_err(to_message)?;
    let _ = window.emit(UPDATE_NOTICE_CHANGED_EVENT, latest_update());
    Ok(())
}

fn place_update_notice(app: &tauri::AppHandle, window: &WebviewWindow) -> tauri::Result<()> {
    let cursor_monitor = app
        .cursor_position()
        .ok()
        .and_then(|cursor| app.monitor_from_point(cursor.x, cursor.y).ok().flatten());
    let Some(monitor) = cursor_monitor.or(app.primary_monitor()?) else {
        return Ok(());
    };

    let scale = monitor.scale_factor();
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    let work_area = monitor.work_area();
    let width = ((NOTICE_WIDTH * scale).round() as u32).min(work_area.size.width);
    let height = ((NOTICE_HEIGHT * scale).round() as u32).min(work_area.size.height);
    let margin = (NOTICE_MARGIN * scale).round() as i32;
    let x = work_area.position.x + work_area.size.width as i32 - width as i32 - margin;
    let y = work_area.position.y + work_area.size.height as i32 - height as i32 - margin;

    window.set_size(Size::Physical(PhysicalSize::new(width, height)))?;
    window.set_position(Position::Physical(PhysicalPosition::new(
        x.max(work_area.position.x),
        y.max(work_area.position.y),
    )))?;
    Ok(())
}

fn close_update_notice_impl(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window(UPDATE_NOTICE_LABEL) {
        let _ = window.destroy();
    }
}

#[tauri::command]
pub fn get_update_notice() -> Option<AppUpdateInfo> {
    latest_update()
}

#[tauri::command]
pub async fn dismiss_update_notice(app: tauri::AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        with_lazy_window_operation(|| {
            close_update_notice_impl(&app);
            Ok(())
        })
    })
    .await
    .map_err(to_message)?
}

#[tauri::command]
pub async fn update_update_settings(
    app: tauri::AppHandle,
    auto_check_updates: Option<bool>,
    skipped_update_version: Option<String>,
) -> Result<AppSettings, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let settings = with_config_mutation(|| {
            let store = AppStore::open().map_err(to_message)?;
            let mut config = store.load_config_strict().map_err(to_message)?;
            if let Some(enabled) = auto_check_updates {
                config.settings.auto_check_updates = enabled;
            }
            if let Some(version) = skipped_update_version {
                config.settings.skipped_update_version = version.trim().to_owned();
            }
            store.save_config(&config).map_err(to_message)?;
            Ok(config.settings)
        })?;
        // The config transaction has ended; the tray entry re-reads the skipped version.
        sync_tray_entry(&app);
        Ok(settings)
    })
    .await
    .map_err(to_message)?
}
