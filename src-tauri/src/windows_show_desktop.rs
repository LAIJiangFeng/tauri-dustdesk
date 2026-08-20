use std::{
    ffi::c_void,
    ptr::{null, null_mut},
    sync::atomic::{AtomicBool, AtomicPtr, Ordering},
    thread,
    time::Duration,
};

use tauri::{Manager, WebviewWindow};
use windows_sys::{
    core::w,
    Win32::{
        Foundation::HWND,
        UI::WindowsAndMessaging::{
            CreateWindowExW, DestroyWindow, DispatchMessageW, GetAncestor, GetForegroundWindow,
            GetWindow, GetWindowLongPtrW, GetWindowThreadProcessId, IsIconic, IsWindow,
            IsWindowVisible, PeekMessageW, SetWindowLongPtrW, SetWindowPos, TranslateMessage,
            GA_ROOT, GWLP_HWNDPARENT, GWL_EXSTYLE, GWL_STYLE, GW_HWNDNEXT, GW_HWNDPREV,
            HWND_BOTTOM, HWND_TOPMOST, MSG, PM_REMOVE, SWP_FRAMECHANGED, SWP_NOACTIVATE,
            SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOSENDCHANGING, SWP_NOSIZE, SWP_NOZORDER,
            WS_DISABLED, WS_EX_APPWINDOW, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_MINIMIZEBOX,
            WS_POPUP,
        },
    },
};

const NORMAL_POLL_INTERVAL: Duration = Duration::from_millis(100);
const ACTIVE_POLL_INTERVAL: Duration = Duration::from_millis(50);
const AFTER_SHOW_DESKTOP_SETTLE: Duration = Duration::from_millis(300);
const Z_ORDER_FLAGS: u32 =
    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_NOSENDCHANGING;

static SHOW_DESKTOP_ACTIVE: AtomicBool = AtomicBool::new(false);
static SHOW_DESKTOP_MONITOR_STARTED: AtomicBool = AtomicBool::new(false);
static Z_ORDER_REFRESH_PENDING: AtomicBool = AtomicBool::new(false);
static POSITIONING_HELPER: AtomicPtr<c_void> = AtomicPtr::new(null_mut());

struct MonitorWindows {
    sentinel: HWND,
    positioning_helper: HWND,
}

impl MonitorWindows {
    fn create() -> Option<Self> {
        unsafe {
            let sentinel = CreateWindowExW(
                WS_EX_TOOLWINDOW,
                w!("Static"),
                w!("DustDeskShowDesktopSentinel"),
                WS_POPUP | WS_DISABLED,
                0,
                0,
                0,
                0,
                null_mut(),
                null_mut(),
                null_mut(),
                null(),
            );
            if sentinel.is_null() {
                return None;
            }

            let positioning_helper = CreateWindowExW(
                WS_EX_TOOLWINDOW,
                w!("Static"),
                w!("DustDeskShowDesktopPositioningHelper"),
                WS_POPUP | WS_DISABLED,
                0,
                0,
                0,
                0,
                null_mut(),
                null_mut(),
                null_mut(),
                null(),
            );
            if positioning_helper.is_null() {
                DestroyWindow(sentinel);
                return None;
            }

            move_window_to_bottom(sentinel);
            move_window_to_bottom(positioning_helper);
            Some(Self {
                sentinel,
                positioning_helper,
            })
        }
    }
}

impl Drop for MonitorWindows {
    fn drop(&mut self) {
        unsafe {
            DestroyWindow(self.positioning_helper);
            DestroyWindow(self.sentinel);
        }
    }
}

struct RefreshPendingGuard;

impl Drop for RefreshPendingGuard {
    fn drop(&mut self) {
        Z_ORDER_REFRESH_PENDING.store(false, Ordering::SeqCst);
    }
}

pub(crate) fn start(app: tauri::AppHandle) {
    if SHOW_DESKTOP_MONITOR_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    if let Err(error) = thread::Builder::new()
        .name("dustdesk-show-desktop-monitor".to_owned())
        .spawn(move || run_monitor(app))
    {
        SHOW_DESKTOP_MONITOR_STARTED.store(false, Ordering::SeqCst);
        eprintln!("无法启动 Windows 显示桌面监控：{error}");
    }
}

pub(crate) fn is_active() -> bool {
    SHOW_DESKTOP_ACTIVE.load(Ordering::SeqCst)
}

fn desktop_window_styles(style: isize, ex_style: isize) -> (isize, isize) {
    (
        style & !(WS_MINIMIZEBOX as isize),
        (ex_style & !(WS_EX_APPWINDOW as isize)) | WS_EX_TOOLWINDOW as isize,
    )
}

pub(crate) fn configure_desktop_card_window(window: &WebviewWindow) {
    let Ok(hwnd) = window.hwnd() else {
        return;
    };
    let Some(desktop_host) = desktop_icons_host_window() else {
        return;
    };

    unsafe {
        let style = GetWindowLongPtrW(hwnd.0, GWL_STYLE);
        let ex_style = GetWindowLongPtrW(hwnd.0, GWL_EXSTYLE);
        let (desktop_style, desktop_ex_style) = desktop_window_styles(style, ex_style);
        let owner = GetWindowLongPtrW(hwnd.0, GWLP_HWNDPARENT) as HWND;

        if style == desktop_style && ex_style == desktop_ex_style && owner == desktop_host {
            return;
        }

        SetWindowLongPtrW(hwnd.0, GWL_STYLE, desktop_style);
        SetWindowLongPtrW(hwnd.0, GWL_EXSTYLE, desktop_ex_style);
        SetWindowLongPtrW(hwnd.0, GWLP_HWNDPARENT, desktop_host as isize);
        SetWindowPos(
            hwnd.0,
            null_mut(),
            0,
            0,
            0,
            0,
            SWP_FRAMECHANGED
                | SWP_NOMOVE
                | SWP_NOSIZE
                | SWP_NOACTIVATE
                | SWP_NOOWNERZORDER
                | SWP_NOZORDER,
        );
    }
}

pub(crate) fn request_z_order_refresh(app: &tauri::AppHandle) {
    if Z_ORDER_REFRESH_PENDING.swap(true, Ordering::SeqCst) {
        return;
    }

    let task_app = app.clone();
    if let Err(error) = app.run_on_main_thread(move || {
        let _guard = RefreshPendingGuard;
        if SHOW_DESKTOP_ACTIVE.load(Ordering::SeqCst) {
            apply_show_desktop_z_order(&task_app);
        } else {
            restore_normal_z_order(&task_app);
        }
    }) {
        Z_ORDER_REFRESH_PENDING.store(false, Ordering::SeqCst);
        eprintln!("无法刷新桌面框窗口层级：{error}");
    }
}

fn run_monitor(app: tauri::AppHandle) {
    let Some(windows) = MonitorWindows::create() else {
        SHOW_DESKTOP_MONITOR_STARTED.store(false, Ordering::SeqCst);
        eprintln!("无法创建 Windows 显示桌面监控窗口");
        return;
    };
    POSITIONING_HELPER.store(windows.positioning_helper, Ordering::SeqCst);
    let mut last_external_foreground = null_mut();
    let mut last_desktop_host = null_mut();

    while !crate::REAL_EXIT_REQUESTED.load(Ordering::SeqCst) {
        pump_thread_messages();
        unsafe {
            move_window_to_bottom(windows.sentinel);
        }

        let desktop_host = desktop_icons_host_window();
        let foreground = unsafe { GetForegroundWindow() };
        let external_foreground =
            is_external_app_foreground(foreground, desktop_host, windows.positioning_helper);
        let detected_active =
            desktop_host.is_some_and(|host| is_sentinel_below_desktop_host(host, windows.sentinel));
        let current_desktop_host = desktop_host.unwrap_or(null_mut());
        let desktop_host_changed = current_desktop_host != last_desktop_host;
        let was_active = SHOW_DESKTOP_ACTIVE.load(Ordering::SeqCst);
        let active = next_show_desktop_state(was_active, detected_active, external_foreground);
        SHOW_DESKTOP_ACTIVE.store(active, Ordering::SeqCst);
        let changed = was_active != active;
        if changed {
            crate::append_desktop_diagnostic(
                "windows_show_desktop_changed",
                if active {
                    "active=true"
                } else {
                    "active=false"
                },
            );
        }
        if changed
            || desktop_host_changed
            || (external_foreground && foreground != last_external_foreground)
        {
            request_z_order_refresh(&app);
        }
        last_desktop_host = current_desktop_host;
        last_external_foreground = if external_foreground {
            foreground
        } else {
            null_mut()
        };

        if was_active && !active {
            thread::sleep(AFTER_SHOW_DESKTOP_SETTLE);
            request_z_order_refresh(&app);
        }

        thread::sleep(if active {
            ACTIVE_POLL_INTERVAL
        } else {
            NORMAL_POLL_INTERVAL
        });
    }

    SHOW_DESKTOP_ACTIVE.store(false, Ordering::SeqCst);
    request_z_order_refresh(&app);
    POSITIONING_HELPER.store(null_mut(), Ordering::SeqCst);
    SHOW_DESKTOP_MONITOR_STARTED.store(false, Ordering::SeqCst);
}

fn next_show_desktop_state(
    was_active: bool,
    detected_active: bool,
    regular_app_foreground: bool,
) -> bool {
    if regular_app_foreground {
        false
    } else {
        was_active || detected_active
    }
}

fn is_external_app_foreground(
    foreground: HWND,
    desktop_host: Option<HWND>,
    positioning_helper: HWND,
) -> bool {
    if foreground.is_null()
        || desktop_host == Some(foreground)
        || foreground == positioning_helper
        || unsafe { IsWindowVisible(foreground) } == 0
        || unsafe { IsIconic(foreground) } != 0
    {
        return false;
    }

    let mut process_id = 0;
    unsafe {
        GetWindowThreadProcessId(foreground, &mut process_id);
    }
    process_id != 0 && process_id != std::process::id()
}

fn desktop_icons_host_window() -> Option<HWND> {
    let listview = crate::desktop_listview_window()?;
    let host = unsafe { GetAncestor(listview, GA_ROOT) };
    if host.is_null() || unsafe { IsWindow(host) } == 0 {
        None
    } else {
        Some(host)
    }
}

fn is_sentinel_below_desktop_host(desktop_host: HWND, sentinel: HWND) -> bool {
    if desktop_host.is_null() || sentinel.is_null() || unsafe { IsWindowVisible(desktop_host) } == 0
    {
        return false;
    }

    let mut current = unsafe { GetWindow(desktop_host, GW_HWNDNEXT) };
    while !current.is_null() {
        if current == sentinel {
            return true;
        }
        current = unsafe { GetWindow(current, GW_HWNDNEXT) };
    }
    false
}

fn apply_show_desktop_z_order(app: &tauri::AppHandle) {
    let positioning_helper = POSITIONING_HELPER.load(Ordering::SeqCst);
    if positioning_helper.is_null() || unsafe { IsWindow(positioning_helper) } == 0 {
        return;
    }
    let Some(desktop_host) = desktop_icons_host_window() else {
        return;
    };

    unsafe {
        position_helper_above_desktop(positioning_helper, desktop_host);
    }

    for window in app.webview_windows().values() {
        if !crate::is_desktop_card_window_label(window.label()) {
            continue;
        }
        configure_desktop_card_window(window);
        let Ok(hwnd) = window.hwnd() else {
            continue;
        };
        let hwnd = hwnd.0;
        if unsafe { IsWindowVisible(hwnd) } == 0 {
            continue;
        }

        let _ = window.set_skip_taskbar(true);
        unsafe {
            SetWindowPos(hwnd, positioning_helper, 0, 0, 0, 0, Z_ORDER_FLAGS);
        }
    }
}

fn restore_normal_z_order(app: &tauri::AppHandle) {
    let positioning_helper = POSITIONING_HELPER.load(Ordering::SeqCst);
    if !positioning_helper.is_null() && unsafe { IsWindow(positioning_helper) } != 0 {
        unsafe {
            move_window_to_bottom(positioning_helper);
        }
    }

    for window in app.webview_windows().values() {
        if !crate::is_desktop_card_window_label(window.label()) {
            continue;
        }
        configure_desktop_card_window(window);
        let _ = window.set_skip_taskbar(true);
        if let Ok(hwnd) = window.hwnd() {
            unsafe {
                SetWindowPos(hwnd.0, HWND_BOTTOM, 0, 0, 0, 0, Z_ORDER_FLAGS);
            }
        }
    }
}

unsafe fn position_helper_above_desktop(positioning_helper: HWND, desktop_host: HWND) {
    unsafe {
        SetWindowPos(positioning_helper, HWND_TOPMOST, 0, 0, 0, 0, Z_ORDER_FLAGS);
    }

    let mut current = unsafe { GetWindow(desktop_host, GW_HWNDPREV) };
    while !current.is_null() {
        if current != positioning_helper
            && unsafe { GetWindowLongPtrW(current, GWL_EXSTYLE) } & WS_EX_TOPMOST as isize != 0
        {
            unsafe {
                SetWindowPos(positioning_helper, current, 0, 0, 0, 0, Z_ORDER_FLAGS);
            }
            return;
        }
        current = unsafe { GetWindow(current, GW_HWNDPREV) };
    }
}

unsafe fn move_window_to_bottom(hwnd: HWND) {
    unsafe {
        SetWindowPos(hwnd, HWND_BOTTOM, 0, 0, 0, 0, Z_ORDER_FLAGS);
    }
}

fn pump_thread_messages() {
    unsafe {
        let mut message: MSG = std::mem::zeroed();
        while PeekMessageW(&mut message, null_mut(), 0, 0, PM_REMOVE) != 0 {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{desktop_window_styles, next_show_desktop_state};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        WS_EX_APPWINDOW, WS_EX_TOOLWINDOW, WS_MINIMIZEBOX,
    };

    #[test]
    fn desktop_card_is_not_a_minimizable_app_window() {
        let (style, ex_style) =
            desktop_window_styles(WS_MINIMIZEBOX as isize, WS_EX_APPWINDOW as isize);

        assert_eq!(style & WS_MINIMIZEBOX as isize, 0);
        assert_eq!(ex_style & WS_EX_APPWINDOW as isize, 0);
        assert_ne!(ex_style & WS_EX_TOOLWINDOW as isize, 0);
    }

    #[test]
    fn show_desktop_detection_enters_the_latched_state() {
        assert!(next_show_desktop_state(false, true, false));
    }

    #[test]
    fn transient_detection_loss_does_not_hide_desktop_cards() {
        assert!(next_show_desktop_state(true, false, false));
    }

    #[test]
    fn a_restored_application_exits_the_latched_state() {
        assert!(!next_show_desktop_state(true, true, true));
    }
}
