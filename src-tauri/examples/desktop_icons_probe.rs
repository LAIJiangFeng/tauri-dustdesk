//! DustDesk 技术验证（spike）：隐藏 / 恢复 Windows 系统桌面图标，以及崩溃兜底。
//!
//! 独立探测工具，不属于应用业务代码，主程序不会引用它。运行方式：
//!
//! ```text
//! cargo run --manifest-path src-tauri/Cargo.toml --example desktop_icons_probe -- <command>
//! src-tauri\target\debug\examples\desktop_icons_probe.exe <command>
//! ```
//!
//! - M1：只隐藏桌面图标列表窗口（`SysListView32`），不改任何系统设置。
//! - M2：系统“显示桌面图标”设置（`WM_COMMAND 0x7402` → `SHELLDLL_DefView`，持久化到 `HideIcons`）。
//!
//! 命令一览见 `print_help`；结论报告见 `docs/spikes/desktop-icon-hiding.md`。

#[cfg(not(windows))]
fn main() {
    eprintln!("desktop_icons_probe only supports Windows.");
}

#[cfg(windows)]
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(probe::run(&args));
}

#[cfg(windows)]
mod probe {
    use std::{
        ffi::{c_void, OsString},
        fmt::Display,
        fs,
        io::{BufRead, BufReader, Write},
        mem::{size_of, zeroed},
        os::windows::{
            ffi::{OsStrExt, OsStringExt},
            io::AsRawHandle,
            process::CommandExt,
        },
        panic::{self, AssertUnwindSafe},
        path::{Path, PathBuf},
        process::{Child, ChildStdout, Command, Stdio},
        ptr::{null, null_mut},
        sync::{
            atomic::{AtomicBool, AtomicUsize, Ordering},
            mpsc::{self, Receiver},
            Mutex, MutexGuard,
        },
        thread,
        time::{Duration, Instant, SystemTime, UNIX_EPOCH},
    };

    use windows_sys::{
        core::{w, BOOL},
        Win32::{
            Foundation::{
                CloseHandle, ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, HANDLE, HWND, LPARAM, POINT,
                RECT, WAIT_OBJECT_0,
            },
            Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFOEXW},
            System::{
                Com::CoTaskMemFree,
                Diagnostics::Debug::{ReadProcessMemory, WriteProcessMemory},
                Memory::{
                    VirtualAllocEx, VirtualFreeEx, MEM_COMMIT, MEM_RELEASE, MEM_RESERVE,
                    PAGE_READWRITE,
                },
                Registry::{
                    RegCloseKey, RegDeleteKeyValueW, RegGetValueW, RegOpenKeyExW, RegSetValueExW,
                    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_SET_VALUE, REG_DWORD,
                    RRF_RT_REG_DWORD, RRF_RT_REG_SZ,
                },
                Threading::{
                    CreateEventW, GetCurrentProcess, GetCurrentThreadId, OpenEventW, OpenProcess,
                    SetEvent, TerminateProcess, WaitForSingleObject, CREATE_BREAKAWAY_FROM_JOB,
                    CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW, DETACHED_PROCESS,
                    EVENT_MODIFY_STATE, INFINITE, PROCESS_QUERY_INFORMATION,
                    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
                    PROCESS_VM_OPERATION, PROCESS_VM_READ, PROCESS_VM_WRITE,
                },
            },
            UI::{
                Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK},
                HiDpi::{
                    GetDpiForMonitor, SetProcessDpiAwarenessContext,
                    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, MDT_EFFECTIVE_DPI,
                },
                Shell::{
                    FOLDERID_Desktop, SHChangeNotify, SHGetKnownFolderPath, SHGetSetSettings,
                    KF_FLAG_DEFAULT, SHCNE_ASSOCCHANGED, SHCNE_UPDATEDIR, SHCNF_FLUSHNOWAIT,
                    SHCNF_IDLIST, SHCNF_PATHW, SHELLSTATEA, SSF_HIDEICONS, SSF_SHOWEXTENSIONS,
                },
                WindowsAndMessaging::{
                    DispatchMessageW, EnumWindows, FindWindowExW, FindWindowW, GetAncestor,
                    GetClassNameW, GetClientRect, GetForegroundWindow, GetMessageW,
                    GetSystemMetrics, GetWindow, GetWindowLongPtrW, GetWindowRect, GetWindowTextW,
                    GetWindowThreadProcessId, IsWindowVisible, PostMessageW, PostThreadMessageW,
                    SendMessageTimeoutW, ShowWindow, ShowWindowAsync, CHILDID_SELF,
                    EVENT_OBJECT_CREATE, EVENT_OBJECT_DESTROY, EVENT_OBJECT_HIDE,
                    EVENT_OBJECT_SHOW, GA_ROOT, GWL_STYLE, GW_CHILD, GW_HWNDNEXT, MSG,
                    OBJID_WINDOW, SMTO_ABORTIFHUNG, SM_CMONITORS, SM_CXVIRTUALSCREEN,
                    SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SW_HIDE, SW_SHOW,
                    SW_SHOWNA, WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS, WM_COMMAND, WM_QUIT,
                    WM_SETTINGCHANGE, WS_VISIBLE,
                },
            },
        },
    };

    const ADVANCED_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced";
    const CURRENT_VERSION_KEY: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
    /// 桌面右键菜单“查看 → 显示桌面图标”的命令 ID（切换语义）。
    const CMD_TOGGLE_DESKTOP_ICONS: usize = 0x7402;
    const SEND_TIMEOUT_MS: u32 = 5_000;
    const PROBE_PREFIX: &str = "dustdesk-probe-";
    const BASELINE_FILE: &str = "dustdesk-desktop-icons-probe-baseline.txt";
    /// SHELLSTATE 第一个位域：fShowExtensions 是第 1 位，fHideIcons 是第 12 位。
    const SHOW_EXTENSIONS_BIT: i32 = 1 << 1;
    const HIDE_ICONS_BIT: i32 = 1 << 12;
    const LVM_GETITEMCOUNT: u32 = 0x1000 + 4;
    const LVM_GETITEMPOSITION: u32 = 0x1000 + 16;
    const LVM_SETITEMPOSITION32: u32 = 0x1000 + 49;
    const LVM_GETITEMSPACING: u32 = 0x1000 + 51;
    const LVM_GETEXTENDEDLISTVIEWSTYLE: u32 = 0x1000 + 55;
    const LVM_GETITEMTEXTW: u32 = 0x1000 + 115;
    const LVS_AUTOARRANGE: u32 = 0x0100;
    const LVS_EX_SNAPTOGRID: u32 = 0x0008_0000;
    const ERROR_ACCESS_DENIED_CODE: i32 = 5;
    const POLL_FAST: Duration = Duration::from_millis(20);
    const SETTING_AREAS: [Option<&str>; 4] = [
        None,
        Some("Policy"),
        Some("ImmersiveColorSet"),
        Some("ShellState"),
    ];
    const MANUAL_RECOVERY: &str = "MANUAL RECOVERY: run `cargo run --manifest-path src-tauri/Cargo.toml \
--example desktop_icons_probe -- restore-all`, or right-click the desktop > View > Show desktop icons, \
or restart Windows Explorer.";

    // Win32_System_JobObjects / Win32_System_Console 没有在 Cargo.toml 启用（本任务不能改），手动声明。
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn IsProcessInJob(process: HANDLE, job: HANDLE, result: *mut BOOL) -> BOOL;
        fn SetConsoleCtrlHandler(
            handler: Option<unsafe extern "system" fn(u32) -> BOOL>,
            add: BOOL,
        ) -> BOOL;
    }

    pub fn run(args: &[String]) -> i32 {
        // 所有窗口矩形、列表坐标都按物理像素读取。
        unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        let command = args.first().map(String::as_str).unwrap_or("help");
        let rest = args.get(1..).unwrap_or(&[]);
        let result = match command {
            "status" => cmd_status(),
            "hide-window" => cmd_hide_window(),
            "show-window" => cmd_show_window(rest),
            "set-hide-icons" => cmd_set_hide_icons(rest),
            "restore-all" => cmd_restore_all(),
            "guard-demo" => cmd_guard_demo(rest),
            "watchdog" => cmd_watchdog(rest),
            "experiment-all" => cmd_experiment_all(),
            "help" | "-h" | "--help" => {
                print_help();
                Ok(0)
            }
            other => Err(format!("unknown command `{other}`; run `help`")),
        };
        match result {
            Ok(code) => code,
            Err(error) => {
                eprintln!("error: {error}");
                1
            }
        }
    }

    fn print_help() {
        println!(
            "desktop_icons_probe - DustDesk spike: hide/restore the Windows desktop icons

USAGE
  cargo run --manifest-path src-tauri/Cargo.toml --example desktop_icons_probe -- <command>
  src-tauri\\target\\debug\\examples\\desktop_icons_probe.exe <command>

COMMANDS
  status                                   Windows build, desktop window tree, monitors, HideIcons
  hide-window                              M1: ShowWindow(SysListView32, SW_HIDE); stays hidden
  show-window [showna|show|async]          M1 restore (default ShowWindow(SW_SHOWNA))
  set-hide-icons on|off [send|post|parent] M2: toggle 'Show desktop icons' via WM_COMMAND 0x7402
  restore-all                              one-shot recovery: HideIcons off + list view visible
  guard-demo m1|m2 [--max-secs N] [--log PATH]
                                           hide, arm a watchdog child, wait to be killed
                                           (restores itself after N s, default 300, 0 = never)
  experiment-all                           automated experiment matrix; restores the initial state
  watchdog ...                             internal, spawned by guard-demo / experiment-all"
        );
    }

    // ---------------------------------------------------------------------------------------
    // 通用小工具
    // ---------------------------------------------------------------------------------------

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn wide_path(path: &Path) -> Vec<u16> {
        path.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    fn b(value: bool) -> u8 {
        u8::from(value)
    }

    fn hex(value: usize) -> String {
        if value == 0 {
            "null".to_owned()
        } else {
            format!("0x{value:X}")
        }
    }

    fn ms_f(duration: Duration) -> f64 {
        duration.as_secs_f64() * 1000.0
    }

    fn fmt_ms(duration: Duration) -> String {
        format!("{:.1} ms", ms_f(duration))
    }

    fn fmt_opt_ms(duration: Option<Duration>) -> String {
        duration.map_or_else(|| "never".to_owned(), fmt_ms)
    }

    fn say(text: impl Display) {
        // guard-demo 的 stdout 可能是已被读端关闭的管道；println! 会 panic，这里吞掉写错误。
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{text}");
        let _ = out.flush();
    }

    fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        mutex
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn wait_for(
        timeout: Duration,
        poll: Duration,
        mut condition: impl FnMut() -> bool,
    ) -> Option<Duration> {
        let start = Instant::now();
        loop {
            if condition() {
                return Some(start.elapsed());
            }
            if start.elapsed() >= timeout {
                return None;
            }
            thread::sleep(poll);
        }
    }

    /// 只用于测量恢复耗时：忙等 + yield，避免 15.6 ms 的系统计时粒度。
    fn spin_until(timeout: Duration, mut condition: impl FnMut() -> bool) -> Option<Duration> {
        let start = Instant::now();
        loop {
            if condition() {
                return Some(start.elapsed());
            }
            if start.elapsed() >= timeout {
                return None;
            }
            thread::yield_now();
        }
    }

    fn unique_suffix() -> u128 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0)
    }

    // ---------------------------------------------------------------------------------------
    // 窗口
    // ---------------------------------------------------------------------------------------

    fn class_name(hwnd: HWND) -> String {
        let mut buffer = [0u16; 256];
        let length = unsafe { GetClassNameW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32) };
        String::from_utf16_lossy(&buffer[..length.max(0) as usize])
    }

    fn window_title(hwnd: HWND) -> String {
        let mut buffer = [0u16; 256];
        let length = unsafe { GetWindowTextW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32) };
        String::from_utf16_lossy(&buffer[..length.max(0) as usize])
    }

    fn window_rect(hwnd: HWND) -> Option<RECT> {
        let mut rect: RECT = unsafe { zeroed() };
        (unsafe { GetWindowRect(hwnd, &mut rect) } != 0).then_some(rect)
    }

    fn client_width(hwnd: HWND) -> i32 {
        let mut rect: RECT = unsafe { zeroed() };
        unsafe { GetClientRect(hwnd, &mut rect) };
        rect.right - rect.left
    }

    fn fmt_rect(rect: &RECT) -> String {
        format!(
            "({},{})-({},{}) {}x{}",
            rect.left,
            rect.top,
            rect.right,
            rect.bottom,
            rect.right - rect.left,
            rect.bottom - rect.top
        )
    }

    fn visible(hwnd: HWND) -> bool {
        !hwnd.is_null() && unsafe { IsWindowVisible(hwnd) } != 0
    }

    fn style_visible(hwnd: HWND) -> bool {
        !hwnd.is_null() && (unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) } as u32) & WS_VISIBLE != 0
    }

    fn window_pid(hwnd: HWND) -> u32 {
        let mut process_id = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd, &mut process_id) };
        process_id
    }

    fn foreground() -> usize {
        unsafe { GetForegroundWindow() as usize }
    }

    fn fg_change(before: usize, after: usize) -> String {
        if before == after {
            "foreground unchanged".to_owned()
        } else {
            format!(
                "foreground changed {} -> {} (may be user activity)",
                hex(before),
                hex(after)
            )
        }
    }

    fn describe(hwnd: HWND) -> String {
        format!(
            "{} {} visible={} WS_VISIBLE={} rect={} title=\"{}\" pid={}",
            class_name(hwnd),
            hex(hwnd as usize),
            b(visible(hwnd)),
            b(style_visible(hwnd)),
            window_rect(hwnd).map_or_else(|| "?".to_owned(), |rect| fmt_rect(&rect)),
            window_title(hwnd),
            window_pid(hwnd)
        )
    }

    #[derive(Clone, Copy)]
    struct DesktopWindows {
        progman: HWND,
        /// SHELLDLL_DefView 的父窗口：Progman 或某个 WorkerW。
        shell_parent: HWND,
        defview: HWND,
        listview: HWND,
        /// 与 windows_show_desktop.rs 相同：GetAncestor(listview, GA_ROOT)。
        host: HWND,
        via_progman: bool,
    }

    /// 复制自 main.rs `desktop_listview_under_window`：parent > SHELLDLL_DefView > SysListView32。
    unsafe fn desktop_listview_under_window(parent: HWND) -> Option<(HWND, HWND)> {
        if parent.is_null() {
            return None;
        }
        let shell_view =
            unsafe { FindWindowExW(parent, null_mut(), w!("SHELLDLL_DefView"), null()) };
        if shell_view.is_null() {
            return None;
        }
        let listview =
            unsafe { FindWindowExW(shell_view, null_mut(), w!("SysListView32"), null()) };
        if listview.is_null() {
            None
        } else {
            Some((shell_view, listview))
        }
    }

    unsafe extern "system" fn enum_desktop_listview_window(hwnd: HWND, lparam: LPARAM) -> BOOL {
        if let Some((defview, listview)) = unsafe { desktop_listview_under_window(hwnd) } {
            unsafe {
                *(lparam as *mut Option<(usize, usize, usize)>) =
                    Some((hwnd as usize, defview as usize, listview as usize));
            }
            return 0;
        }
        1
    }

    /// 复制自 main.rs `desktop_listview_window`：先 Progman，找不到再 EnumWindows（WorkerW 等）。
    fn find_desktop() -> Option<DesktopWindows> {
        unsafe {
            let progman = FindWindowW(w!("Progman"), null());
            if let Some((defview, listview)) = desktop_listview_under_window(progman) {
                return Some(DesktopWindows {
                    progman,
                    shell_parent: progman,
                    defview,
                    listview,
                    host: GetAncestor(listview, GA_ROOT),
                    via_progman: true,
                });
            }
            let mut found: Option<(usize, usize, usize)> = None;
            EnumWindows(
                Some(enum_desktop_listview_window),
                (&mut found as *mut Option<(usize, usize, usize)>) as LPARAM,
            );
            let (parent, defview, listview) = found?;
            let listview = listview as HWND;
            Some(DesktopWindows {
                progman,
                shell_parent: parent as HWND,
                defview: defview as HWND,
                listview,
                host: GetAncestor(listview, GA_ROOT),
                via_progman: false,
            })
        }
    }

    unsafe extern "system" fn collect_top_level(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let list = unsafe { &mut *(lparam as *mut Vec<usize>) };
        list.push(hwnd as usize);
        1
    }

    fn top_level_windows_of_class(class: &str) -> Vec<HWND> {
        let mut all: Vec<usize> = Vec::new();
        unsafe {
            EnumWindows(
                Some(collect_top_level),
                (&mut all as *mut Vec<usize>) as LPARAM,
            )
        };
        all.into_iter()
            .map(|raw| raw as HWND)
            .filter(|hwnd| class_name(*hwnd) == class)
            .collect()
    }

    // ---------------------------------------------------------------------------------------
    // 注册表与 SHGetSetSettings
    // ---------------------------------------------------------------------------------------

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum RegDword {
        Absent,
        Value(u32),
        Error(u32),
    }

    impl RegDword {
        fn token(self) -> String {
            match self {
                Self::Absent => "absent".to_owned(),
                Self::Value(value) => value.to_string(),
                Self::Error(code) => format!("error{code}"),
            }
        }

        fn parse(token: &str) -> Result<Self, String> {
            if token == "absent" {
                return Ok(Self::Absent);
            }
            token
                .parse()
                .map(Self::Value)
                .map_err(|_| format!("bad registry token `{token}`"))
        }
    }

    fn reg_dword(root: HKEY, subkey: &str, name: &str) -> RegDword {
        let subkey = wide(subkey);
        let name = wide(name);
        let mut data = 0u32;
        let mut size = size_of::<u32>() as u32;
        let status = unsafe {
            RegGetValueW(
                root,
                subkey.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_DWORD,
                null_mut(),
                (&mut data as *mut u32).cast(),
                &mut size,
            )
        };
        match status {
            ERROR_SUCCESS => RegDword::Value(data),
            ERROR_FILE_NOT_FOUND => RegDword::Absent,
            other => RegDword::Error(other),
        }
    }

    fn reg_string(root: HKEY, subkey: &str, name: &str) -> Option<String> {
        let subkey = wide(subkey);
        let name = wide(name);
        let mut buffer = vec![0u16; 512];
        let mut size = (buffer.len() * size_of::<u16>()) as u32;
        let status = unsafe {
            RegGetValueW(
                root,
                subkey.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_SZ,
                null_mut(),
                buffer.as_mut_ptr().cast(),
                &mut size,
            )
        };
        if status != ERROR_SUCCESS {
            return None;
        }
        let length = buffer
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(buffer.len());
        Some(String::from_utf16_lossy(&buffer[..length]))
    }

    fn reg_hide_icons() -> RegDword {
        reg_dword(HKEY_CURRENT_USER, ADVANCED_KEY, "HideIcons")
    }

    fn reg_write_hide_icons(value: u32) -> Result<(), String> {
        let subkey = wide(ADVANCED_KEY);
        let name = wide("HideIcons");
        let mut key: HKEY = null_mut();
        let status = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                subkey.as_ptr(),
                0,
                KEY_SET_VALUE,
                &mut key,
            )
        };
        if status != ERROR_SUCCESS {
            return Err(format!("RegOpenKeyExW failed: {status}"));
        }
        let status = unsafe {
            RegSetValueExW(
                key,
                name.as_ptr(),
                0,
                REG_DWORD,
                (&value as *const u32).cast(),
                size_of::<u32>() as u32,
            )
        };
        unsafe { RegCloseKey(key) };
        if status == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(format!("RegSetValueExW failed: {status}"))
        }
    }

    fn reg_delete_hide_icons() -> Result<(), String> {
        let subkey = wide(ADVANCED_KEY);
        let name = wide("HideIcons");
        let status =
            unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, subkey.as_ptr(), name.as_ptr()) };
        if status == ERROR_SUCCESS || status == ERROR_FILE_NOT_FOUND {
            Ok(())
        } else {
            Err(format!("RegDeleteKeyValueW failed: {status}"))
        }
    }

    fn api_shell_flags() -> i32 {
        let mut state: SHELLSTATEA = unsafe { zeroed() };
        unsafe { SHGetSetSettings(&mut state, SSF_HIDEICONS | SSF_SHOWEXTENSIONS, 0) };
        state._bitfield1
    }

    fn api_hide_icons() -> bool {
        api_shell_flags() & HIDE_ICONS_BIT != 0
    }

    /// 当前“显示桌面图标”设置是否为隐藏：注册表优先（Explorer 持久化的值），没有值时看 API。
    fn setting_hidden_now() -> bool {
        match reg_hide_icons() {
            RegDword::Value(value) => value != 0,
            _ => api_hide_icons(),
        }
    }

    // ---------------------------------------------------------------------------------------
    // 状态快照与恢复
    // ---------------------------------------------------------------------------------------

    #[derive(Clone, Debug)]
    struct IconState {
        found: bool,
        listview: usize,
        defview: usize,
        host: usize,
        host_class: String,
        listview_visible: bool,
        listview_style_visible: bool,
        defview_visible: bool,
        host_visible: bool,
        reg: RegDword,
        api_hidden: bool,
    }

    impl IconState {
        fn capture() -> Self {
            let desktop = find_desktop();
            let (listview, defview, host) = desktop
                .map_or((null_mut(), null_mut(), null_mut()), |desktop| {
                    (desktop.listview, desktop.defview, desktop.host)
                });
            Self {
                found: desktop.is_some(),
                listview: listview as usize,
                defview: defview as usize,
                host: host as usize,
                host_class: if host.is_null() {
                    String::new()
                } else {
                    class_name(host)
                },
                listview_visible: visible(listview),
                listview_style_visible: style_visible(listview),
                defview_visible: visible(defview),
                host_visible: visible(host),
                reg: reg_hide_icons(),
                api_hidden: api_hide_icons(),
            }
        }

        fn setting_hidden(&self) -> bool {
            match self.reg {
                RegDword::Value(value) => value != 0,
                _ => self.api_hidden,
            }
        }

        fn icons_visible(&self) -> bool {
            self.found && self.listview_visible && !self.setting_hidden()
        }

        fn core(&self) -> String {
            format!(
                "listview_visible={} hide_icons_reg={} hide_icons_api={}",
                b(self.listview_visible),
                self.reg.token(),
                b(self.api_hidden)
            )
        }

        fn summary(&self) -> String {
            format!(
                "{} defview_visible={} host_visible={} listview={} host={}({})",
                self.core(),
                b(self.defview_visible),
                b(self.host_visible),
                hex(self.listview),
                hex(self.host),
                self.host_class
            )
        }
    }

    #[derive(Clone, Copy, Debug)]
    struct RestoreTarget {
        setting_hidden: bool,
        reg: RegDword,
        listview_visible: bool,
    }

    impl RestoreTarget {
        fn from_state(state: &IconState) -> Self {
            Self {
                setting_hidden: state.setting_hidden(),
                reg: state.reg,
                listview_visible: state.listview_visible,
            }
        }

        fn matches(&self, state: &IconState) -> bool {
            state.found
                && state.listview_visible == self.listview_visible
                && state.setting_hidden() == self.setting_hidden
                && state.api_hidden == self.setting_hidden
                && state.reg == self.reg
        }

        fn text(&self) -> String {
            format!(
                "listview_visible={} hide_icons_reg={} hide_icons_api={}",
                b(self.listview_visible),
                self.reg.token(),
                b(self.setting_hidden)
            )
        }
    }

    struct RestoreOutcome {
        state: IconState,
        acted: bool,
    }

    /// 恢复到目标状态：先恢复 M2 设置（切换语义，先读后切、绝不盲目重发），
    /// 再恢复注册表的精确表示，最后恢复 M1 列表窗口可见性。不持锁，调用方负责串行化。
    fn restore_to(
        target: &RestoreTarget,
        log: &mut dyn FnMut(String),
    ) -> Result<RestoreOutcome, String> {
        let state = IconState::capture();
        if target.matches(&state) {
            return Ok(RestoreOutcome {
                state,
                acted: false,
            });
        }
        log(format!(
            "current {} -> target {}",
            state.core(),
            target.text()
        ));
        if !state.found {
            return Err("desktop list view not found (is Explorer running?)".to_owned());
        }

        let mut toggles = 0;
        while setting_hidden_now() != target.setting_hidden {
            if toggles == 2 {
                return Err(format!(
                    "HideIcons is still {} after 2 toggles",
                    b(!target.setting_hidden)
                ));
            }
            toggles += 1;
            let before = IconState::capture();
            let desktop = find_desktop().ok_or("desktop list view not found")?;
            log(format!(
                "WM_COMMAND 0x7402 -> SHELLDLL_DefView (want HideIcons={})",
                b(target.setting_hidden)
            ));
            raw_toggle(&desktop, ToggleVia::SendDefView)?;
            if wait_for(Duration::from_secs(3), Duration::from_millis(2), || {
                setting_hidden_now() == target.setting_hidden
            })
            .is_some()
            {
                break;
            }
            let after = IconState::capture();
            if after.reg != before.reg
                || after.api_hidden != before.api_hidden
                || after.listview_visible != before.listview_visible
            {
                // 有变化但没到目标：继续等，不重发切换命令，避免双重切换。
                if wait_for(Duration::from_secs(3), Duration::from_millis(5), || {
                    setting_hidden_now() == target.setting_hidden
                })
                .is_some()
                {
                    break;
                }
                return Err(format!(
                    "the toggle changed something but HideIcons is not {}: {}",
                    b(target.setting_hidden),
                    IconState::capture().summary()
                ));
            }
            log("the toggle had no observable effect; retrying once".to_owned());
        }

        let reg = reg_hide_icons();
        if setting_hidden_now() == target.setting_hidden && reg != target.reg {
            match target.reg {
                RegDword::Absent => {
                    log("deleting the HideIcons value (it did not exist initially)".to_owned());
                    reg_delete_hide_icons()?;
                }
                RegDword::Value(value) => {
                    log(format!("writing HideIcons={value} (exact initial value)"));
                    reg_write_hide_icons(value)?;
                }
                RegDword::Error(_) => {}
            }
        }

        if let Some(desktop) = find_desktop() {
            let is_visible = visible(desktop.listview);
            if target.listview_visible && !is_visible {
                for how in [ShowHow::ShowNa, ShowHow::Show] {
                    log(format!("{} on SysListView32", how.label()));
                    raw_show(desktop.listview, how);
                    if wait_for(Duration::from_secs(1), Duration::from_millis(1), || {
                        visible(desktop.listview)
                    })
                    .is_some()
                    {
                        break;
                    }
                }
            } else if !target.listview_visible && is_visible && !target.setting_hidden {
                log(
                    "ShowWindow(SW_HIDE) on SysListView32 (the initial state was M1-hidden)"
                        .to_owned(),
                );
                raw_hide(desktop.listview);
                let _ = wait_for(Duration::from_secs(1), Duration::from_millis(1), || {
                    !visible(desktop.listview)
                });
            }
        }

        // SHGetSetSettings 在本进程有缓存，给它一点时间跟上 Explorer 写入的值。
        let _ = wait_for(
            Duration::from_millis(1500),
            Duration::from_millis(5),
            || target.matches(&IconState::capture()),
        );
        let state = IconState::capture();
        if target.matches(&state) {
            Ok(RestoreOutcome { state, acted: true })
        } else {
            Err(format!(
                "state still differs after restore: {}",
                state.summary()
            ))
        }
    }

    static MUTATION_LOCK: Mutex<()> = Mutex::new(());
    static ABORTING: AtomicBool = AtomicBool::new(false);
    static EMERGENCY_TARGET: Mutex<Option<RestoreTarget>> = Mutex::new(None);
    static PROBE_REGISTRY: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

    /// 所有改变图标状态的调用都持有这把锁；Ctrl+C 处理器拿到锁后恢复并直接退出进程。
    fn mutation_guard() -> Result<MutexGuard<'static, ()>, String> {
        let guard = lock(&MUTATION_LOCK);
        if ABORTING.load(Ordering::SeqCst) {
            return Err("aborting: emergency restore in progress".to_owned());
        }
        Ok(guard)
    }

    fn restore_locked(
        target: &RestoreTarget,
        log: &mut dyn FnMut(String),
    ) -> Result<RestoreOutcome, String> {
        let _guard = mutation_guard()?;
        restore_to(target, log)
    }

    unsafe extern "system" fn console_ctrl_handler(ctrl_type: u32) -> BOOL {
        ABORTING.store(true, Ordering::SeqCst);
        let _guard = lock(&MUTATION_LOCK);
        eprintln!("\n[emergency] console control event {ctrl_type}: restoring the desktop icons");
        let target = *lock(&EMERGENCY_TARGET);
        if let Some(target) = target {
            match restore_to(&target, &mut |line| eprintln!("[emergency] {line}")) {
                Ok(outcome) => eprintln!("[emergency] restored: {}", outcome.state.summary()),
                Err(error) => eprintln!("[emergency] RESTORE FAILED: {error}\n{MANUAL_RECOVERY}"),
            }
        }
        cleanup_probe_files(&mut |line| eprintln!("[emergency] {line}"));
        std::process::exit(130);
    }

    fn install_ctrl_handler() {
        unsafe { SetConsoleCtrlHandler(Some(console_ctrl_handler), 1) };
    }

    fn in_job() -> Option<bool> {
        let mut result: BOOL = 0;
        let ok = unsafe { IsProcessInJob(GetCurrentProcess(), null_mut(), &mut result) };
        (ok != 0).then_some(result != 0)
    }

    fn fmt_job(value: Option<bool>) -> &'static str {
        match value {
            Some(true) => "yes",
            Some(false) => "no",
            None => "unknown",
        }
    }

    // ---------------------------------------------------------------------------------------
    // M1 / M2 操作
    // ---------------------------------------------------------------------------------------

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum ShowHow {
        ShowNa,
        Show,
        AsyncShowNa,
    }

    impl ShowHow {
        fn label(self) -> &'static str {
            match self {
                Self::ShowNa => "ShowWindow(SW_SHOWNA)",
                Self::Show => "ShowWindow(SW_SHOW)",
                Self::AsyncShowNa => "ShowWindowAsync(SW_SHOWNA)",
            }
        }

        fn short(self) -> &'static str {
            match self {
                Self::ShowNa => "showna",
                Self::Show => "show",
                Self::AsyncShowNa => "async-showna",
            }
        }
    }

    fn raw_hide(listview: HWND) {
        unsafe { ShowWindow(listview, SW_HIDE) };
    }

    fn raw_show(listview: HWND, how: ShowHow) {
        unsafe {
            match how {
                ShowHow::ShowNa => ShowWindow(listview, SW_SHOWNA),
                ShowHow::Show => ShowWindow(listview, SW_SHOW),
                ShowHow::AsyncShowNa => ShowWindowAsync(listview, SW_SHOWNA),
            }
        };
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum ToggleVia {
        SendDefView,
        PostDefView,
        SendShellParent,
    }

    impl ToggleVia {
        fn label(self) -> &'static str {
            match self {
                Self::SendDefView => "SendMessageTimeout -> SHELLDLL_DefView",
                Self::PostDefView => "PostMessage -> SHELLDLL_DefView",
                Self::SendShellParent => "SendMessageTimeout -> DefView parent (Progman/WorkerW)",
            }
        }

        fn short(self) -> &'static str {
            match self {
                Self::SendDefView => "send-defview",
                Self::PostDefView => "post-defview",
                Self::SendShellParent => "send-parent",
            }
        }
    }

    fn send_timeout(hwnd: HWND, message: u32, wparam: usize, lparam: isize) -> Option<usize> {
        let mut result = 0usize;
        let ok = unsafe {
            SendMessageTimeoutW(
                hwnd,
                message,
                wparam,
                lparam,
                SMTO_ABORTIFHUNG,
                SEND_TIMEOUT_MS,
                &mut result,
            )
        };
        (ok != 0).then_some(result)
    }

    fn raw_toggle(desktop: &DesktopWindows, via: ToggleVia) -> Result<(), String> {
        let ok = match via {
            ToggleVia::SendDefView => {
                send_timeout(desktop.defview, WM_COMMAND, CMD_TOGGLE_DESKTOP_ICONS, 0).is_some()
            }
            ToggleVia::SendShellParent => send_timeout(
                desktop.shell_parent,
                WM_COMMAND,
                CMD_TOGGLE_DESKTOP_ICONS,
                0,
            )
            .is_some(),
            ToggleVia::PostDefView => unsafe {
                PostMessageW(desktop.defview, WM_COMMAND, CMD_TOGGLE_DESKTOP_ICONS, 0) != 0
            },
        };
        if ok {
            Ok(())
        } else {
            Err(format!(
                "{} failed: {}",
                via.label(),
                std::io::Error::last_os_error()
            ))
        }
    }

    fn m1_hide() -> Result<(DesktopWindows, Duration), String> {
        let desktop = find_desktop().ok_or("desktop list view not found")?;
        let call = {
            let _guard = mutation_guard()?;
            let start = Instant::now();
            raw_hide(desktop.listview);
            start.elapsed()
        };
        if wait_for(Duration::from_secs(1), Duration::from_millis(2), || {
            !visible(desktop.listview)
        })
        .is_none()
        {
            return Err("the list view is still visible 1 s after ShowWindow(SW_HIDE)".to_owned());
        }
        Ok((desktop, call))
    }

    #[derive(Default)]
    struct Transition {
        call: Duration,
        listview: Option<Duration>,
        reg: Option<Duration>,
        api: Option<Duration>,
    }

    impl Transition {
        fn text(&self) -> String {
            format!(
                "call {}, list view changed after {}, HideIcons registry {}, SHGetSetSettings {}",
                fmt_ms(self.call),
                fmt_opt_ms(self.listview),
                fmt_opt_ms(self.reg),
                fmt_opt_ms(self.api)
            )
        }
    }

    /// M2：发送一次切换命令并测量列表窗口 / 注册表 / API 何时反映新状态。不会重发。
    fn m2_set(hidden: bool, via: ToggleVia, timeout: Duration) -> Result<Transition, String> {
        let desktop = find_desktop().ok_or("desktop list view not found")?;
        if setting_hidden_now() == hidden {
            return Err(format!("HideIcons is already {}", b(hidden)));
        }
        let start;
        let call = {
            let _guard = mutation_guard()?;
            start = Instant::now();
            raw_toggle(&desktop, via)?;
            start.elapsed()
        };
        let mut transition = Transition {
            call,
            ..Transition::default()
        };
        let want = RegDword::Value(u32::from(hidden));
        loop {
            let elapsed = start.elapsed();
            if transition.listview.is_none() && visible(desktop.listview) != hidden {
                transition.listview = Some(elapsed);
            }
            if transition.reg.is_none() && reg_hide_icons() == want {
                transition.reg = Some(elapsed);
            }
            if transition.api.is_none() && api_hide_icons() == hidden {
                transition.api = Some(elapsed);
            }
            let complete = transition.listview.is_some()
                && transition.reg.is_some()
                && transition.api.is_some();
            if complete || elapsed >= timeout {
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
        Ok(transition)
    }

    // ---------------------------------------------------------------------------------------
    // 跨进程读写桌面列表（与 main.rs 的 LVM_* 代码同一做法）
    // ---------------------------------------------------------------------------------------

    struct RemoteProcess(HANDLE);

    impl Drop for RemoteProcess {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.0) };
        }
    }

    struct RemoteBlock<'a> {
        process: &'a RemoteProcess,
        address: *mut c_void,
    }

    impl Drop for RemoteBlock<'_> {
        fn drop(&mut self) {
            unsafe { VirtualFreeEx(self.process.0, self.address, 0, MEM_RELEASE) };
        }
    }

    impl RemoteProcess {
        fn open_for(hwnd: HWND) -> Option<Self> {
            let process_id = window_pid(hwnd);
            if process_id == 0 {
                return None;
            }
            let handle = unsafe {
                OpenProcess(
                    PROCESS_QUERY_INFORMATION
                        | PROCESS_VM_OPERATION
                        | PROCESS_VM_READ
                        | PROCESS_VM_WRITE,
                    0,
                    process_id,
                )
            };
            (!handle.is_null()).then_some(Self(handle))
        }

        fn alloc(&self, size: usize) -> Option<RemoteBlock<'_>> {
            let address = unsafe {
                VirtualAllocEx(
                    self.0,
                    null(),
                    size,
                    MEM_COMMIT | MEM_RESERVE,
                    PAGE_READWRITE,
                )
            };
            (!address.is_null()).then_some(RemoteBlock {
                process: self,
                address,
            })
        }
    }

    impl RemoteBlock<'_> {
        fn at(&self, offset: usize) -> isize {
            self.address as isize + offset as isize
        }

        fn write<T>(&self, offset: usize, value: &T) -> bool {
            unsafe {
                WriteProcessMemory(
                    self.process.0,
                    self.at(offset) as *const c_void,
                    (value as *const T).cast(),
                    size_of::<T>(),
                    null_mut(),
                ) != 0
            }
        }

        fn read_into(&self, offset: usize, buffer: *mut c_void, length: usize) -> bool {
            unsafe {
                ReadProcessMemory(
                    self.process.0,
                    self.at(offset) as *const c_void,
                    buffer,
                    length,
                    null_mut(),
                ) != 0
            }
        }
    }

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

    fn listview_item_count(listview: HWND) -> Option<usize> {
        send_timeout(listview, LVM_GETITEMCOUNT, 0, 0)
    }

    fn listview_item_texts(listview: HWND) -> Option<Vec<(i32, String)>> {
        const CAPACITY: usize = 1024;
        let count = listview_item_count(listview)?.min(i32::MAX as usize);
        let process = RemoteProcess::open_for(listview)?;
        let item_size = size_of::<LvItemTextW>();
        let block = process.alloc(item_size + CAPACITY * size_of::<u16>())?;
        let remote_text = block.at(item_size) as *mut u16;
        let mut texts = Vec::with_capacity(count);
        for index in 0..count as i32 {
            let item = LvItemTextW {
                mask: 0,
                item: index,
                sub_item: 0,
                state: 0,
                state_mask: 0,
                text: remote_text,
                text_capacity: CAPACITY as i32,
            };
            if !block.write(0, &item) {
                continue;
            }
            let Some(characters) =
                send_timeout(listview, LVM_GETITEMTEXTW, index as usize, block.at(0))
            else {
                continue;
            };
            let characters = characters.min(CAPACITY - 1);
            if characters == 0 {
                continue;
            }
            let mut text = vec![0u16; characters];
            if block.read_into(
                item_size,
                text.as_mut_ptr().cast(),
                characters * size_of::<u16>(),
            ) {
                texts.push((index, String::from_utf16_lossy(&text)));
            }
        }
        Some(texts)
    }

    fn listview_find_item(listview: HWND, path: &Path) -> Option<i32> {
        let name = path.file_name()?.to_string_lossy().to_lowercase();
        let stem = path.file_stem()?.to_string_lossy().to_lowercase();
        listview_item_texts(listview)?
            .into_iter()
            .find(|(_, text)| {
                let text = text.to_lowercase();
                text == name || text == stem
            })
            .map(|(index, _)| index)
    }

    fn listview_item_position(listview: HWND, index: i32) -> Option<(i32, i32)> {
        let process = RemoteProcess::open_for(listview)?;
        let block = process.alloc(size_of::<POINT>())?;
        let ok = send_timeout(listview, LVM_GETITEMPOSITION, index as usize, block.at(0))?;
        if ok == 0 {
            return None;
        }
        let mut point = POINT { x: 0, y: 0 };
        block
            .read_into(0, (&mut point as *mut POINT).cast(), size_of::<POINT>())
            .then_some((point.x, point.y))
    }

    fn listview_set_item_position(listview: HWND, index: i32, x: i32, y: i32) -> bool {
        let Some(process) = RemoteProcess::open_for(listview) else {
            return false;
        };
        let Some(block) = process.alloc(size_of::<POINT>()) else {
            return false;
        };
        if !block.write(0, &POINT { x, y }) {
            return false;
        }
        send_timeout(listview, LVM_SETITEMPOSITION32, index as usize, block.at(0)).is_some()
    }

    fn listview_item_spacing(listview: HWND) -> Option<(i32, i32)> {
        send_timeout(listview, LVM_GETITEMSPACING, 0, 0)
            .map(|value| ((value & 0xFFFF) as i32, ((value >> 16) & 0xFFFF) as i32))
    }

    fn listview_styles(listview: HWND) -> (bool, Option<bool>) {
        let style = unsafe { GetWindowLongPtrW(listview, GWL_STYLE) } as u32;
        let extended = send_timeout(listview, LVM_GETEXTENDEDLISTVIEWSTYLE, 0, 0);
        (
            style & LVS_AUTOARRANGE != 0,
            extended.map(|value| value as u32 & LVS_EX_SNAPTOGRID != 0),
        )
    }

    // ---------------------------------------------------------------------------------------
    // 外部触发
    // ---------------------------------------------------------------------------------------

    fn notify_assoc_changed() {
        unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED as i32, SHCNF_IDLIST, null(), null()) };
    }

    fn notify_updatedir(dir: &Path) {
        let path = wide_path(dir);
        unsafe {
            SHChangeNotify(
                SHCNE_UPDATEDIR as i32,
                SHCNF_PATHW | SHCNF_FLUSHNOWAIT,
                path.as_ptr().cast(),
                null(),
            )
        };
    }

    /// 只发给桌面相关的顶层窗口（Progman / 宿主），不广播。
    fn send_setting_change(desktop: &DesktopWindows, area: Option<&str>) -> String {
        let text = area.map(wide);
        let lparam = text.as_ref().map_or(0, |text| text.as_ptr() as isize);
        let mut targets = Vec::new();
        if !desktop.progman.is_null() {
            targets.push(desktop.progman);
        }
        if !desktop.host.is_null() && desktop.host != desktop.progman {
            targets.push(desktop.host);
        }
        let parts: Vec<String> = targets
            .iter()
            .map(|target| {
                let sent = send_timeout(*target, WM_SETTINGCHANGE, 0, lparam).is_some();
                format!(
                    "{}({}) sent={}",
                    class_name(*target),
                    hex(*target as usize),
                    b(sent)
                )
            })
            .collect();
        format!(
            "WM_SETTINGCHANGE(wParam=0, lParam={}) -> {}",
            area.map_or_else(|| "NULL".to_owned(), |area| format!("\"{area}\"")),
            parts.join(", ")
        )
    }

    fn desktop_folder() -> Option<PathBuf> {
        let mut raw = null_mut();
        let result = unsafe {
            SHGetKnownFolderPath(
                &FOLDERID_Desktop,
                KF_FLAG_DEFAULT as u32,
                null_mut(),
                &mut raw,
            )
        };
        if result < 0 || raw.is_null() {
            return None;
        }
        let mut length = 0usize;
        while unsafe { *raw.add(length) } != 0 {
            length += 1;
        }
        let path = OsString::from_wide(unsafe { std::slice::from_raw_parts(raw, length) });
        unsafe { CoTaskMemFree(raw.cast()) };
        Some(PathBuf::from(path))
    }

    // ---------------------------------------------------------------------------------------
    // 桌面测试文件：只允许 dustdesk-probe-<pid>-<序号>
    // ---------------------------------------------------------------------------------------

    struct ProbeFiles {
        dir: PathBuf,
        prefix: String,
        next: u32,
    }

    impl ProbeFiles {
        fn new(dir: PathBuf) -> Self {
            Self {
                dir,
                prefix: format!("{PROBE_PREFIX}{}-", std::process::id()),
                next: 0,
            }
        }

        fn next_path(&mut self) -> PathBuf {
            self.next += 1;
            self.dir.join(format!("{}{}.txt", self.prefix, self.next))
        }

        fn check(&self, path: &Path) -> Result<(), String> {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            if path.parent() == Some(self.dir.as_path()) && name.starts_with(&self.prefix) {
                Ok(())
            } else {
                Err(format!(
                    "refusing to touch non-probe path {}",
                    path.display()
                ))
            }
        }

        fn create(&mut self) -> Result<PathBuf, String> {
            let path = self.next_path();
            self.check(&path)?;
            lock(&PROBE_REGISTRY).push(path.clone());
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .map_err(|error| format!("create {}: {error}", path.display()))?;
            file.write_all(b"DustDesk desktop icon probe file. Safe to delete.\r\n")
                .map_err(|error| format!("write {}: {error}", path.display()))?;
            Ok(path)
        }

        fn rename(&mut self, from: &Path) -> Result<PathBuf, String> {
            self.check(from)?;
            let to = self.next_path();
            self.check(&to)?;
            if to.exists() {
                return Err(format!("{} already exists", to.display()));
            }
            lock(&PROBE_REGISTRY).push(to.clone());
            fs::rename(from, &to).map_err(|error| format!("rename {}: {error}", from.display()))?;
            Ok(to)
        }

        fn delete(&self, path: &Path) -> Result<(), String> {
            self.check(path)?;
            fs::remove_file(path).map_err(|error| format!("delete {}: {error}", path.display()))
        }
    }

    fn file_name(path: &Path) -> String {
        path.file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    fn cleanup_probe_files(log: &mut dyn FnMut(String)) {
        let own_prefix = format!("{PROBE_PREFIX}{}-", std::process::id());
        let paths = lock(&PROBE_REGISTRY).clone();
        for path in paths {
            let is_probe = path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(&own_prefix));
            if is_probe && path.exists() {
                match fs::remove_file(&path) {
                    Ok(()) => log(format!("removed leftover {}", path.display())),
                    Err(error) => log(format!("could not remove {}: {error}", path.display())),
                }
            }
        }
    }

    fn sweep_probe_files(dir: &Path, log: &mut dyn FnMut(String)) {
        let own_prefix = format!("{PROBE_PREFIX}{}-", std::process::id());
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with(&own_prefix) {
                match fs::remove_file(entry.path()) {
                    Ok(()) => log(format!("removed leftover {name}")),
                    Err(error) => log(format!("could not remove {name}: {error}")),
                }
            }
        }
    }

    // ---------------------------------------------------------------------------------------
    // WinEvent 监视（WINEVENT_OUTOFCONTEXT，只看 Explorer 进程，不注入）
    // ---------------------------------------------------------------------------------------

    static EVENT_LOG: Mutex<Vec<EventRecord>> = Mutex::new(Vec::new());
    static WATCH_LISTVIEW: AtomicUsize = AtomicUsize::new(0);
    static WATCH_DEFVIEW: AtomicUsize = AtomicUsize::new(0);

    #[derive(Clone)]
    struct EventRecord {
        at: Instant,
        event: u32,
        what: &'static str,
    }

    unsafe extern "system" fn win_event_proc(
        _hook: HWINEVENTHOOK,
        event: u32,
        hwnd: HWND,
        id_object: i32,
        id_child: i32,
        _thread: u32,
        _time: u32,
    ) {
        if hwnd.is_null() || id_object != OBJID_WINDOW || id_child != CHILDID_SELF as i32 {
            return;
        }
        let raw = hwnd as usize;
        let what = if raw == WATCH_LISTVIEW.load(Ordering::Relaxed) {
            "listview"
        } else if raw == WATCH_DEFVIEW.load(Ordering::Relaxed) {
            "defview"
        } else if event == EVENT_OBJECT_CREATE {
            let root_class = class_name(unsafe { GetAncestor(hwnd, GA_ROOT) });
            if root_class != "Progman" && root_class != "WorkerW" {
                return;
            }
            match class_name(hwnd).as_str() {
                "SysListView32" => "NEW desktop SysListView32",
                "SHELLDLL_DefView" => "NEW desktop SHELLDLL_DefView",
                _ => return,
            }
        } else {
            return;
        };
        lock(&EVENT_LOG).push(EventRecord {
            at: Instant::now(),
            event,
            what,
        });
    }

    struct EventMonitor {
        thread_id: u32,
        join: Option<thread::JoinHandle<()>>,
    }

    impl EventMonitor {
        fn start(process_id: u32) -> Option<Self> {
            let (sender, receiver) = mpsc::channel();
            let join = thread::Builder::new()
                .name("winevent-monitor".to_owned())
                .spawn(move || {
                    let hook = unsafe {
                        SetWinEventHook(
                            EVENT_OBJECT_CREATE,
                            EVENT_OBJECT_HIDE,
                            null_mut(),
                            Some(win_event_proc),
                            process_id,
                            0,
                            WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
                        )
                    };
                    let _ = sender.send((unsafe { GetCurrentThreadId() }, !hook.is_null()));
                    if hook.is_null() {
                        return;
                    }
                    let mut message: MSG = unsafe { zeroed() };
                    while unsafe { GetMessageW(&mut message, null_mut(), 0, 0) } > 0 {
                        unsafe { DispatchMessageW(&message) };
                    }
                    unsafe { UnhookWinEvent(hook) };
                })
                .ok()?;
            let (thread_id, hooked) = receiver.recv_timeout(Duration::from_secs(3)).ok()?;
            if !hooked {
                let _ = join.join();
                return None;
            }
            Some(Self {
                thread_id,
                join: Some(join),
            })
        }
    }

    impl Drop for EventMonitor {
        fn drop(&mut self) {
            unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0) };
            if let Some(join) = self.join.take() {
                let _ = join.join();
            }
        }
    }

    fn event_mark() -> usize {
        lock(&EVENT_LOG).len()
    }

    fn event_name(event: u32) -> &'static str {
        match event {
            EVENT_OBJECT_CREATE => "CREATE",
            EVENT_OBJECT_DESTROY => "DESTROY",
            EVENT_OBJECT_SHOW => "SHOW",
            EVENT_OBJECT_HIDE => "HIDE",
            _ => "?",
        }
    }

    fn events_text(mark: usize, origin: Instant) -> String {
        // 回调经由监视线程的消息循环投递，留一点时间让最后的事件进日志。
        thread::sleep(Duration::from_millis(30));
        let log = lock(&EVENT_LOG);
        let items: Vec<String> = log
            .iter()
            .skip(mark)
            .map(|record| {
                let offset = if record.at >= origin {
                    ms_f(record.at - origin)
                } else {
                    -ms_f(origin - record.at)
                };
                format!(
                    "{offset:+.0}ms {} {}",
                    event_name(record.event),
                    record.what
                )
            })
            .collect();
        if items.is_empty() {
            "none".to_owned()
        } else {
            items.join(", ")
        }
    }

    // ---------------------------------------------------------------------------------------
    // 看门狗
    // ---------------------------------------------------------------------------------------

    struct WatchdogProcess {
        child: Child,
        breakaway: bool,
        armed_in: Duration,
    }

    /// 同一个 exe 以 `watchdog` 参数启动：DETACHED_PROCESS（不随控制台 Ctrl+C / 关窗口一起死）、
    /// 独立进程组，尽量 CREATE_BREAKAWAY_FROM_JOB；用命名事件确认已经打开父进程句柄后才返回。
    fn spawn_watchdog(
        parent_pid: u32,
        label: &str,
        target: &RestoreTarget,
        log: Option<&Path>,
        quiet_noop: bool,
    ) -> Result<WatchdogProcess, String> {
        let exe = std::env::current_exe().map_err(|error| error.to_string())?;
        let event_name = format!(
            "Local\\DustDeskIconProbeArmed-{parent_pid}-{}",
            unique_suffix()
        );
        let event_wide = wide(&event_name);
        let event = unsafe { CreateEventW(null(), 1, 0, event_wide.as_ptr()) };
        if event.is_null() {
            return Err(format!(
                "CreateEventW failed: {}",
                std::io::Error::last_os_error()
            ));
        }
        let mut args: Vec<String> = vec![
            "watchdog".to_owned(),
            parent_pid.to_string(),
            label.to_owned(),
            b(target.setting_hidden).to_string(),
            target.reg.token(),
            "--event".to_owned(),
            event_name,
        ];
        if let Some(log) = log {
            args.push("--log".to_owned());
            args.push(log.to_string_lossy().into_owned());
        }
        if quiet_noop {
            args.push("--quiet-noop".to_owned());
        }
        let spawn = |flags: u32| {
            Command::new(&exe)
                .args(&args)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .creation_flags(flags)
                .spawn()
        };
        let base = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP;
        let started = Instant::now();
        let spawned = match spawn(base | CREATE_BREAKAWAY_FROM_JOB) {
            Ok(child) => Ok((child, true)),
            Err(error) if error.raw_os_error() == Some(ERROR_ACCESS_DENIED_CODE) => {
                spawn(base).map(|child| (child, false))
            }
            Err(error) => Err(error),
        };
        let (mut child, breakaway) = match spawned {
            Ok(spawned) => spawned,
            Err(error) => {
                unsafe { CloseHandle(event) };
                return Err(format!("cannot start the watchdog: {error}"));
            }
        };
        let armed = unsafe { WaitForSingleObject(event, 10_000) } == WAIT_OBJECT_0;
        unsafe { CloseHandle(event) };
        if !armed {
            let _ = child.kill();
            let _ = child.wait();
            return Err("the watchdog did not arm within 10 s".to_owned());
        }
        Ok(WatchdogProcess {
            child,
            breakaway,
            armed_in: started.elapsed(),
        })
    }

    struct WatchdogLog {
        path: Option<PathBuf>,
        file: Option<fs::File>,
        origin: Instant,
    }

    impl WatchdogLog {
        fn new(path: Option<PathBuf>) -> Self {
            let file = path.as_ref().and_then(|path| fs::File::create(path).ok());
            Self {
                path,
                file,
                origin: Instant::now(),
            }
        }

        fn line(&mut self, text: String) {
            let line = format!("[+{:.3} ms] {text}", ms_f(self.origin.elapsed()));
            if let Some(file) = self.file.as_mut() {
                let _ = writeln!(file, "{line}");
                let _ = file.flush();
            } else {
                let _ = writeln!(std::io::stderr(), "{line}");
            }
        }

        fn discard(&mut self) {
            self.file = None;
            if let Some(path) = &self.path {
                let _ = fs::remove_file(path);
            }
        }
    }

    fn cmd_watchdog(args: &[String]) -> Result<i32, String> {
        if args.len() < 4 {
            return Err(
                "usage: watchdog <parent-pid> <label> <hide-icons 0|1> <registry-token> \
[--event NAME] [--log PATH] [--quiet-noop]"
                    .to_owned(),
            );
        }
        let parent_pid: u32 = args[0].parse().map_err(|_| "bad parent pid".to_owned())?;
        let label = args[1].clone();
        let setting_hidden = args[2] == "1";
        let reg = RegDword::parse(&args[3])?;
        let mut event_name = None;
        let mut log_path = None;
        let mut quiet_noop = false;
        let mut index = 4;
        while index < args.len() {
            match args[index].as_str() {
                "--event" => {
                    event_name = args.get(index + 1).cloned();
                    index += 2;
                }
                "--log" => {
                    log_path = args.get(index + 1).map(PathBuf::from);
                    index += 2;
                }
                "--quiet-noop" => {
                    quiet_noop = true;
                    index += 1;
                }
                other => return Err(format!("unknown watchdog option {other}")),
            }
        }

        let mut log = WatchdogLog::new(log_path);
        let parent = unsafe {
            OpenProcess(
                PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
                0,
                parent_pid,
            )
        };
        if parent.is_null() {
            log.line(format!(
                "cannot open the parent process {parent_pid}: {}",
                std::io::Error::last_os_error()
            ));
            return Ok(1);
        }
        // 预热：提前加载 shell32、找一次窗口，缩短父进程死后的恢复耗时。
        let warm = IconState::capture();
        if let Some(name) = event_name {
            let name = wide(&name);
            let event = unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, name.as_ptr()) };
            if !event.is_null() {
                unsafe {
                    SetEvent(event);
                    CloseHandle(event);
                }
            }
        }
        log.line(format!(
            "armed: parent={parent_pid} label={label} target HideIcons={} registry={} in_job={}; now {}",
            b(setting_hidden),
            reg.token(),
            fmt_job(in_job()),
            warm.summary()
        ));

        let wait = unsafe { WaitForSingleObject(parent, INFINITE) };
        let woke = Instant::now();
        unsafe { CloseHandle(parent) };
        log.origin = woke;
        let target = RestoreTarget {
            setting_hidden,
            reg,
            listview_visible: !setting_hidden,
        };
        let result = restore_to(&target, &mut |line| log.line(line));
        let elapsed = woke.elapsed();
        match result {
            Ok(outcome) if !outcome.acted => {
                log.line(format!(
                    "parent exited (wait={wait}); state already matched the target"
                ));
                if quiet_noop {
                    log.discard();
                }
                Ok(0)
            }
            Ok(outcome) => {
                log.line(format!(
                    "RESTORED_MS={:.3} parent exited (wait={wait}); now {}",
                    ms_f(elapsed),
                    outcome.state.summary()
                ));
                Ok(0)
            }
            Err(error) => {
                log.line(format!("RESTORE_FAILED after {}: {error}", fmt_ms(elapsed)));
                Ok(2)
            }
        }
    }

    // ---------------------------------------------------------------------------------------
    // 命令：status / hide-window / show-window / set-hide-icons / restore-all / guard-demo
    // ---------------------------------------------------------------------------------------

    struct MonitorRow {
        name: String,
        rect: RECT,
        work: RECT,
        primary: bool,
        dpi: u32,
    }

    unsafe extern "system" fn collect_monitor(
        monitor: HMONITOR,
        _hdc: HDC,
        _rect: *mut RECT,
        data: LPARAM,
    ) -> BOOL {
        let mut info: MONITORINFOEXW = unsafe { zeroed() };
        info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
        if unsafe { GetMonitorInfoW(monitor, (&mut info as *mut MONITORINFOEXW).cast()) } == 0 {
            return 1;
        }
        let (mut dpi_x, mut dpi_y) = (0u32, 0u32);
        let dpi = if unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) }
            >= 0
        {
            dpi_x
        } else {
            0
        };
        let length = info
            .szDevice
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(info.szDevice.len());
        let rows = unsafe { &mut *(data as *mut Vec<MonitorRow>) };
        rows.push(MonitorRow {
            name: String::from_utf16_lossy(&info.szDevice[..length]),
            rect: info.monitorInfo.rcMonitor,
            work: info.monitorInfo.rcWork,
            primary: info.monitorInfo.dwFlags & 1 != 0,
            dpi,
        });
        1
    }

    fn monitors() -> Vec<MonitorRow> {
        let mut rows: Vec<MonitorRow> = Vec::new();
        unsafe {
            EnumDisplayMonitors(
                null_mut(),
                null(),
                Some(collect_monitor),
                (&mut rows as *mut Vec<MonitorRow>) as LPARAM,
            )
        };
        rows
    }

    fn windows_version() -> String {
        let text = |name: &str| {
            reg_string(HKEY_LOCAL_MACHINE, CURRENT_VERSION_KEY, name).unwrap_or_default()
        };
        let build = text("CurrentBuildNumber");
        let ubr = match reg_dword(HKEY_LOCAL_MACHINE, CURRENT_VERSION_KEY, "UBR") {
            RegDword::Value(value) => value.to_string(),
            _ => "?".to_owned(),
        };
        let family = if build.parse::<u32>().is_ok_and(|number| number >= 22000) {
            "Windows 11"
        } else {
            "Windows 10 or older"
        };
        format!(
            "{family} {} build {build}.{ubr} edition={} (registry ProductName=\"{}\")",
            text("DisplayVersion"),
            text("EditionID"),
            text("ProductName")
        )
    }

    fn print_environment() {
        println!("Windows: {}", windows_version());
        println!(
            "Probe: pid={} in_job={} dpi_awareness=per-monitor-v2 (coordinates are physical pixels)",
            std::process::id(),
            fmt_job(in_job())
        );
        let monitors = monitors();
        println!(
            "Monitors: {} (GetSystemMetrics(SM_CMONITORS)={})",
            monitors.len(),
            unsafe { GetSystemMetrics(SM_CMONITORS) }
        );
        for (index, monitor) in monitors.iter().enumerate() {
            println!(
                "  #{} {} primary={} rect={} work={} dpi={} ({}%)",
                index + 1,
                monitor.name,
                b(monitor.primary),
                fmt_rect(&monitor.rect),
                fmt_rect(&monitor.work),
                monitor.dpi,
                monitor.dpi * 100 / 96
            );
        }
        let (x, y, width, height) = unsafe {
            (
                GetSystemMetrics(SM_XVIRTUALSCREEN),
                GetSystemMetrics(SM_YVIRTUALSCREEN),
                GetSystemMetrics(SM_CXVIRTUALSCREEN),
                GetSystemMetrics(SM_CYVIRTUALSCREEN),
            )
        };
        println!("Virtual screen: ({x},{y}) {width}x{height}");
        println!(
            "Desktop folder (FOLDERID_Desktop): {}",
            desktop_folder()
                .map_or_else(|| "<unknown>".to_owned(), |path| path.display().to_string())
        );
    }

    fn print_children(parent: HWND, indent: usize, depth: usize) {
        if depth == 0 {
            return;
        }
        let mut child = unsafe { GetWindow(parent, GW_CHILD) };
        while !child.is_null() {
            println!("{}{}", "  ".repeat(indent), describe(child));
            print_children(child, indent + 1, depth - 1);
            child = unsafe { GetWindow(child, GW_HWNDNEXT) };
        }
    }

    fn print_hierarchy() {
        println!("Desktop window hierarchy:");
        let progman = unsafe { FindWindowW(w!("Progman"), null()) };
        if progman.is_null() {
            println!("  Progman: NOT FOUND");
        } else {
            println!("  {}", describe(progman));
            print_children(progman, 2, 3);
        }
        let workers = top_level_windows_of_class("WorkerW");
        println!("  Top-level WorkerW windows: {}", workers.len());
        for worker in workers {
            println!("    {}", describe(worker));
            print_children(worker, 3, 2);
        }
    }

    fn print_listview_details() {
        let Some(desktop) = find_desktop() else {
            println!("Desktop list view: NOT FOUND");
            return;
        };
        println!(
            "Desktop list view: {} found via {} (same algorithm as main.rs desktop_listview_window)",
            hex(desktop.listview as usize),
            if desktop.via_progman {
                "Progman > SHELLDLL_DefView > SysListView32"
            } else {
                "EnumWindows > SHELLDLL_DefView > SysListView32"
            }
        );
        println!("  DefView parent: {}", describe(desktop.shell_parent));
        println!("  DefView:        {}", describe(desktop.defview));
        println!("  ListView:       {}", describe(desktop.listview));
        let (auto_arrange, snap) = listview_styles(desktop.listview);
        println!(
            "  items={} LVS_AUTOARRANGE={} LVS_EX_SNAPTOGRID={} icon spacing={}",
            listview_item_count(desktop.listview)
                .map_or_else(|| "?".to_owned(), |count| count.to_string()),
            b(auto_arrange),
            snap.map_or_else(|| "?".to_owned(), |value| b(value).to_string()),
            listview_item_spacing(desktop.listview)
                .map_or_else(|| "?".to_owned(), |(x, y)| format!("{x}x{y}"))
        );
        println!(
            "  Desktop host = GetAncestor(list view, GA_ROOT) (windows_show_desktop.rs): {}",
            describe(desktop.host)
        );
    }

    fn baseline_path() -> PathBuf {
        std::env::temp_dir().join(BASELINE_FILE)
    }

    fn cmd_status() -> Result<i32, String> {
        println!("== desktop_icons_probe status ==");
        print_environment();
        print_hierarchy();
        print_listview_details();
        let state = IconState::capture();
        println!(
            "HideIcons: registry HKCU\\{ADVANCED_KEY}\\HideIcons = {} | SHGetSetSettings(SSF_HIDEICONS).fHideIcons = {}",
            state.reg.token(),
            b(state.api_hidden)
        );
        println!(
            "Bit-layout self-check: SHELLSTATE.fShowExtensions={} vs registry HideFileExt={} (expect opposite values)",
            b(api_shell_flags() & SHOW_EXTENSIONS_BIT != 0),
            reg_dword(HKEY_CURRENT_USER, ADVANCED_KEY, "HideFileExt").token()
        );
        println!(
            "List view IsWindowVisible={} WS_VISIBLE={} | desktop host IsWindowVisible={}",
            b(state.listview_visible),
            b(state.listview_style_visible),
            b(state.host_visible)
        );
        println!("STATE {}", state.core());
        println!(
            "ICONS_VISIBLE={}",
            if state.icons_visible() { "yes" } else { "NO" }
        );
        if let Ok(baseline) = fs::read_to_string(baseline_path()) {
            println!(
                "Baseline recorded at the start of the last experiment-all: {}",
                baseline.trim()
            );
            println!(
                "CURRENT_EQUALS_BASELINE={}",
                if baseline.trim() == state.core() {
                    "yes"
                } else {
                    "NO"
                }
            );
        }
        Ok(0)
    }

    fn cmd_hide_window() -> Result<i32, String> {
        let before = IconState::capture();
        if !before.found {
            return Err("desktop list view not found".to_owned());
        }
        if !before.listview_visible {
            println!("The list view is already hidden: {}", before.summary());
            return Ok(0);
        }
        let (_, call) = m1_hide()?;
        println!(
            "M1: ShowWindow(SysListView32, SW_HIDE) took {}; now {}",
            fmt_ms(call),
            IconState::capture().summary()
        );
        println!(
            "The icons stay hidden until `show-window` / `restore-all` runs or Explorer restarts."
        );
        Ok(0)
    }

    fn cmd_show_window(args: &[String]) -> Result<i32, String> {
        let how = match args.first().map(String::as_str) {
            None | Some("showna") => ShowHow::ShowNa,
            Some("show") => ShowHow::Show,
            Some("async") => ShowHow::AsyncShowNa,
            Some(other) => return Err(format!("unknown show mode `{other}` (showna|show|async)")),
        };
        let desktop = find_desktop().ok_or("desktop list view not found")?;
        if setting_hidden_now() {
            println!("The system setting HideIcons=1 (M2) is active; Explorer hides the list view itself.");
            println!("Use `set-hide-icons off` or `restore-all`. The list view was not touched.");
            return Ok(1);
        }
        if visible(desktop.listview) {
            println!(
                "The list view is already visible: {}",
                IconState::capture().summary()
            );
            return Ok(0);
        }
        let foreground_before = foreground();
        let start = Instant::now();
        {
            let _guard = mutation_guard()?;
            raw_show(desktop.listview, how);
        }
        let call = start.elapsed();
        let shown = wait_for(Duration::from_secs(2), Duration::from_millis(1), || {
            visible(desktop.listview)
        });
        println!(
            "M1 restore: {} took {}, visible after {}; {}",
            how.label(),
            fmt_ms(call),
            fmt_opt_ms(shown),
            fg_change(foreground_before, foreground())
        );
        println!("{}", IconState::capture().summary());
        Ok(if shown.is_some() { 0 } else { 1 })
    }

    fn cmd_set_hide_icons(args: &[String]) -> Result<i32, String> {
        let hidden = match args.first().map(String::as_str) {
            Some("on") => true,
            Some("off") => false,
            _ => return Err("usage: set-hide-icons on|off [send|post|parent]".to_owned()),
        };
        let via = match args.get(1).map(String::as_str) {
            None | Some("send") => ToggleVia::SendDefView,
            Some("post") => ToggleVia::PostDefView,
            Some("parent") => ToggleVia::SendShellParent,
            Some(other) => {
                return Err(format!("unknown toggle route `{other}` (send|post|parent)"))
            }
        };
        let before = IconState::capture();
        if !before.found {
            return Err("desktop list view not found".to_owned());
        }
        if before.setting_hidden() == hidden && before.api_hidden == hidden {
            println!("HideIcons is already {}: {}", b(hidden), before.summary());
            return Ok(0);
        }
        let transition = m2_set(hidden, via, Duration::from_secs(3))?;
        let after = IconState::capture();
        println!(
            "M2: WM_COMMAND 0x7402 via {}: {}",
            via.label(),
            transition.text()
        );
        println!("{}", after.summary());
        if after.setting_hidden() != hidden {
            println!(
                "WARNING: the setting did not change within 3 s; not retrying (toggle semantics)."
            );
            return Ok(1);
        }
        Ok(0)
    }

    fn cmd_restore_all() -> Result<i32, String> {
        let state = IconState::capture();
        let reg = match state.reg {
            RegDword::Absent => RegDword::Absent,
            _ => RegDword::Value(0),
        };
        let target = RestoreTarget {
            setting_hidden: false,
            reg,
            listview_visible: true,
        };
        println!("before: {}", state.summary());
        let outcome = restore_to(&target, &mut |line| println!("  {line}"))?;
        println!("after:  {}", outcome.state.summary());
        println!(
            "ICONS_VISIBLE={}",
            if outcome.state.icons_visible() {
                "yes"
            } else {
                "NO"
            }
        );
        Ok(0)
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Method {
        M1,
        M2,
    }

    impl Method {
        fn parse(text: Option<&String>) -> Result<Self, String> {
            match text.map(String::as_str) {
                Some("m1") => Ok(Self::M1),
                Some("m2") => Ok(Self::M2),
                _ => Err("expected m1 or m2".to_owned()),
            }
        }

        fn arg(self) -> &'static str {
            match self {
                Self::M1 => "m1",
                Self::M2 => "m2",
            }
        }

        fn label(self) -> &'static str {
            match self {
                Self::M1 => "M1",
                Self::M2 => "M2",
            }
        }
    }

    fn cmd_guard_demo(args: &[String]) -> Result<i32, String> {
        let method = Method::parse(args.first())?;
        let mut max_secs: u64 = 300;
        let mut log_path: Option<PathBuf> = None;
        let mut index = 1;
        while index < args.len() {
            match args[index].as_str() {
                "--max-secs" => {
                    max_secs = args
                        .get(index + 1)
                        .and_then(|value| value.parse().ok())
                        .ok_or("--max-secs needs a number")?;
                    index += 2;
                }
                "--log" => {
                    log_path = Some(PathBuf::from(
                        args.get(index + 1).ok_or("--log needs a path")?,
                    ));
                    index += 2;
                }
                other => return Err(format!("unknown guard-demo option {other}")),
            }
        }
        let initial = IconState::capture();
        if !initial.icons_visible() {
            return Err(format!(
                "the desktop icons are not visible right now ({}); run `restore-all` first",
                initial.summary()
            ));
        }
        let target = RestoreTarget::from_state(&initial);
        let pid = std::process::id();
        let log_path = log_path.unwrap_or_else(|| {
            std::env::temp_dir().join(format!("dustdesk-probe-watchdog-{pid}.log"))
        });
        let watchdog = spawn_watchdog(pid, method.arg(), &target, Some(&log_path), false)?;
        say(format!(
            "guard-demo pid={pid} method={} in_job={}",
            method.label(),
            fmt_job(in_job())
        ));
        say(format!(
            "watchdog pid={} armed in {} (detached, own process group, CREATE_BREAKAWAY_FROM_JOB accepted={})",
            watchdog.child.id(),
            fmt_ms(watchdog.armed_in),
            b(watchdog.breakaway)
        ));
        say(format!("watchdog log: {}", log_path.display()));

        let hidden = match method {
            Method::M1 => {
                m1_hide().map(|(_, call)| format!("ShowWindow(SW_HIDE) took {}", fmt_ms(call)))
            }
            Method::M2 => m2_set(true, ToggleVia::SendDefView, Duration::from_secs(3)).and_then(
                |transition| {
                    if setting_hidden_now() {
                        Ok(format!("WM_COMMAND 0x7402: {}", transition.text()))
                    } else {
                        Err("HideIcons did not switch on within 3 s".to_owned())
                    }
                },
            ),
        }?;
        say(format!(
            "hidden: {hidden}; now {}",
            IconState::capture().summary()
        ));
        say(format!(
            "@@ready watchdog_pid={} breakaway={} pid={pid}",
            watchdog.child.id(),
            b(watchdog.breakaway)
        ));
        say(format!(
            "Kill this process to test the watchdog, e.g.  Stop-Process -Id {pid} -Force"
        ));
        if max_secs > 0 {
            say(format!(
                "(safety: guard-demo restores the icons itself after {max_secs} s)"
            ));
        }

        let watchdog_handle = watchdog.child.as_raw_handle() as HANDLE;
        let started = Instant::now();
        let mut warned = false;
        loop {
            if !warned && unsafe { WaitForSingleObject(watchdog_handle, 0) } == WAIT_OBJECT_0 {
                say("@@watchdog_exited the watchdog is gone; if this process is killed now nothing restores the icons");
                warned = true;
            }
            if max_secs > 0 && started.elapsed() >= Duration::from_secs(max_secs) {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
        say("max-secs reached; restoring the icons myself");
        let outcome = restore_to(&target, &mut |line| say(format!("restore: {line}")))?;
        say(format!("restored: {}", outcome.state.summary()));
        Ok(0)
    }

    // ---------------------------------------------------------------------------------------
    // experiment-all
    // ---------------------------------------------------------------------------------------

    #[derive(Clone, Copy)]
    enum Trigger {
        Create,
        Rename,
        Delete,
        AssocChanged,
        UpdateDir,
        SettingChange(Option<&'static str>),
        Idle,
    }

    impl Trigger {
        fn label(self) -> String {
            match self {
                Self::Create => "a. create desktop file dustdesk-probe-*".to_owned(),
                Self::Rename => "a. rename desktop file dustdesk-probe-*".to_owned(),
                Self::Delete => "a. delete desktop file dustdesk-probe-*".to_owned(),
                Self::AssocChanged => "b. SHChangeNotify(SHCNE_ASSOCCHANGED)".to_owned(),
                Self::UpdateDir => "c. SHChangeNotify(SHCNE_UPDATEDIR, desktop)".to_owned(),
                Self::SettingChange(area) => format!(
                    "f. WM_SETTINGCHANGE lParam={} to Progman/host only",
                    area.map_or_else(|| "NULL".to_owned(), |area| format!("\"{area}\""))
                ),
                Self::Idle => "d. idle 30 s (no action)".to_owned(),
            }
        }
    }

    #[derive(Default)]
    struct Observation {
        duration: Duration,
        reshown: Option<Duration>,
        reg_changed: Option<(Duration, RegDword)>,
        item: Option<Duration>,
    }

    impl Observation {
        fn outcome(&self) -> String {
            match self.reshown {
                Some(time) => format!("RE-SHOWN by Explorer after {}", fmt_ms(time)),
                None => format!(
                    "stayed hidden ({:.1} s observed)",
                    self.duration.as_secs_f64()
                ),
            }
        }

        fn item_text(&self, item: &Option<(PathBuf, bool)>) -> Option<String> {
            let (_, present) = item.as_ref()?;
            let verb = if *present { "received" } else { "removed" };
            Some(format!(
                "hidden list view {verb} the item: {}",
                self.item.map_or_else(
                    || "NO".to_owned(),
                    |time| format!("yes after {}", fmt_ms(time))
                )
            ))
        }

        fn reg_text(&self) -> String {
            match self.reg_changed {
                None => "HideIcons unchanged".to_owned(),
                Some((time, value)) => {
                    format!(
                        "HideIcons CHANGED to {} after {}",
                        value.token(),
                        fmt_ms(time)
                    )
                }
            }
        }
    }

    fn observe(
        listview: HWND,
        duration: Duration,
        poll: Duration,
        expected_reg: RegDword,
        item: Option<(&Path, bool)>,
    ) -> Observation {
        let start = Instant::now();
        let mut observation = Observation {
            duration,
            ..Observation::default()
        };
        let mut next_item_check = start;
        while start.elapsed() < duration {
            let elapsed = start.elapsed();
            if observation.reshown.is_none() && visible(listview) {
                observation.reshown = Some(elapsed);
            }
            if observation.reg_changed.is_none() {
                let reg = reg_hide_icons();
                if reg != expected_reg {
                    observation.reg_changed = Some((elapsed, reg));
                }
            }
            if let Some((path, present)) = item {
                if observation.item.is_none() && Instant::now() >= next_item_check {
                    next_item_check = Instant::now() + Duration::from_millis(150);
                    if listview_find_item(listview, path).is_some() == present {
                        observation.item = Some(start.elapsed());
                    }
                }
            }
            thread::sleep(poll);
        }
        observation
    }

    struct Row {
        id: String,
        method: String,
        trigger: String,
        outcome: String,
        detail: String,
    }

    struct CrashSample {
        method: Method,
        runner_ms: f64,
        watchdog_ms: Option<f64>,
    }

    struct Runner {
        initial: IconState,
        target: RestoreTarget,
        desktop_dir: PathBuf,
        probes: ProbeFiles,
        current_probe: Option<PathBuf>,
        rows: Vec<Row>,
        crash_samples: Vec<CrashSample>,
        exe: PathBuf,
        temp_seq: u32,
    }

    struct KillOutcome {
        how: String,
        death: Instant,
        helper: Option<Child>,
    }

    impl KillOutcome {
        fn reap(mut self) {
            if let Some(helper) = self.helper.as_mut() {
                let _ = helper.wait();
            }
        }
    }

    /// 按任务要求用 `Stop-Process -Force` 结束进程；以本进程观察到目标句柄 signaled 的时刻为“死亡时刻”。
    fn kill_with_stop_process(pid: u32, handle: HANDLE) -> Result<KillOutcome, String> {
        let helper = Command::new("powershell.exe")
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                &format!("Stop-Process -Id {pid} -Force"),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
        let issued = Instant::now();
        let died =
            helper.is_ok() && unsafe { WaitForSingleObject(handle, 15_000) } == WAIT_OBJECT_0;
        let mut death = Instant::now();
        let how = if died {
            format!(
                "Stop-Process -Force (exit observed {} after launching PowerShell)",
                fmt_ms(death - issued)
            )
        } else {
            let reason = match &helper {
                Ok(_) => "Stop-Process did not finish in 15 s".to_owned(),
                Err(error) => format!("PowerShell unavailable: {error}"),
            };
            unsafe { TerminateProcess(handle, 1) };
            if unsafe { WaitForSingleObject(handle, 5_000) } != WAIT_OBJECT_0 {
                return Err(format!("process {pid} did not exit"));
            }
            death = Instant::now();
            format!("TerminateProcess fallback ({reason})")
        };
        Ok(KillOutcome {
            how,
            death,
            helper: helper.ok(),
        })
    }

    fn spawn_line_reader(stdout: ChildStdout) -> Receiver<String> {
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else {
                    break;
                };
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        receiver
    }

    fn wait_line(
        lines: &Receiver<String>,
        prefix: &str,
        timeout: Duration,
    ) -> Result<String, String> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match lines.recv_timeout(remaining) {
                Ok(line) if line.starts_with(prefix) => return Ok(line),
                Ok(_) => continue,
                Err(_) => {
                    return Err(format!(
                        "did not see `{prefix}` within {:.0} s",
                        timeout.as_secs_f64()
                    ))
                }
            }
        }
    }

    fn field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
        line.split_whitespace()
            .find_map(|part| part.strip_prefix(key)?.strip_prefix('='))
    }

    /// 一次崩溃兜底试验里的 guard-demo 子进程及其看门狗；Drop 时保证两者都已结束。
    struct CrashTrial {
        child: Child,
        lines: Receiver<String>,
        watchdog_pid: u32,
        watchdog: HANDLE,
        breakaway: String,
        parent_dead: bool,
    }

    impl CrashTrial {
        fn start(exe: &Path, method: Method, log_path: &Path) -> Result<Self, String> {
            let mut child = Command::new(exe)
                .args(["guard-demo", method.arg(), "--max-secs", "20", "--log"])
                .arg(log_path)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|error| format!("cannot start guard-demo: {error}"))?;
            let Some(stdout) = child.stdout.take() else {
                let _ = child.kill();
                let _ = child.wait();
                return Err("guard-demo stdout is missing".to_owned());
            };
            // 从这里开始由 Drop 负责结束 guard-demo 并等待看门狗退出。
            let mut trial = Self {
                child,
                lines: spawn_line_reader(stdout),
                watchdog_pid: 0,
                watchdog: null_mut(),
                breakaway: String::new(),
                parent_dead: false,
            };
            let ready = wait_line(&trial.lines, "@@ready", Duration::from_secs(20))?;
            trial.watchdog_pid = field(&ready, "watchdog_pid")
                .and_then(|value| value.parse().ok())
                .ok_or("guard-demo did not report watchdog_pid")?;
            trial.breakaway = field(&ready, "breakaway").unwrap_or("?").to_owned();
            trial.watchdog = unsafe {
                OpenProcess(
                    PROCESS_SYNCHRONIZE | PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION,
                    0,
                    trial.watchdog_pid,
                )
            };
            if trial.watchdog.is_null() {
                return Err(format!(
                    "cannot open the watchdog process {}",
                    trial.watchdog_pid
                ));
            }
            Ok(trial)
        }

        fn kill_parent(&mut self) -> Result<KillOutcome, String> {
            let handle = self.child.as_raw_handle() as HANDLE;
            let outcome = kill_with_stop_process(self.child.id(), handle)?;
            self.parent_dead = true;
            Ok(outcome)
        }

        fn wait_watchdog_exit(&self, timeout: Duration) -> bool {
            unsafe {
                WaitForSingleObject(self.watchdog, timeout.as_millis() as u32) == WAIT_OBJECT_0
            }
        }
    }

    impl Drop for CrashTrial {
        fn drop(&mut self) {
            if !self.parent_dead {
                let _ = self.child.kill();
            }
            let _ = self.child.wait();
            if !self.watchdog.is_null() {
                unsafe {
                    if WaitForSingleObject(self.watchdog, 6_000) != WAIT_OBJECT_0 {
                        TerminateProcess(self.watchdog, 1);
                        WaitForSingleObject(self.watchdog, 2_000);
                    }
                    CloseHandle(self.watchdog);
                }
            }
        }
    }

    impl Runner {
        fn record(
            &mut self,
            id: &str,
            method: &str,
            trigger: impl Into<String>,
            outcome: impl Into<String>,
            detail: impl Into<String>,
        ) {
            let row = Row {
                id: id.to_owned(),
                method: method.to_owned(),
                trigger: trigger.into(),
                outcome: outcome.into(),
                detail: detail.into(),
            };
            println!(
                "[{}] {} => {}\n        {}",
                row.id, row.trigger, row.outcome, row.detail
            );
            self.rows.push(row);
        }

        fn temp_path(&mut self, tag: &str) -> PathBuf {
            self.temp_seq += 1;
            std::env::temp_dir().join(format!(
                "dustdesk-probe-{}-{tag}-{}.log",
                std::process::id(),
                self.temp_seq
            ))
        }

        /// 每个实验：执行 → 恢复到初始状态并核对 → 再等一下核对迟到的变化。恢复失败就终止全部实验。
        fn step(
            &mut self,
            id: &str,
            body: impl FnOnce(&mut Self) -> Result<(), String>,
        ) -> Result<(), String> {
            println!("[{id}] running");
            if let Err(error) = body(self) {
                println!("[{id}] ERROR: {error}");
                self.record(
                    id,
                    "-",
                    "(experiment failed)",
                    format!("ERROR: {error}"),
                    "",
                );
            }
            let target = self.target;
            let mut log = |line: String| println!("[{id}] restore: {line}");
            restore_locked(&target, &mut log)
                .map_err(|error| format!("[{id}] could not restore the initial state: {error}"))?;
            thread::sleep(Duration::from_millis(400));
            if !target.matches(&IconState::capture()) {
                println!("[{id}] late state change detected after restore");
                restore_locked(&target, &mut log).map_err(|error| {
                    format!("[{id}] could not restore the initial state: {error}")
                })?;
            }
            Ok(())
        }

        fn run_all(&mut self) -> Result<(), String> {
            for how in [ShowHow::ShowNa, ShowHow::Show, ShowHow::AsyncShowNa] {
                self.m1_basic(how)?;
            }
            self.m1_trigger(
                "M1-a-create",
                Trigger::Create,
                Duration::from_millis(2500),
                POLL_FAST,
            )?;
            self.m1_trigger(
                "M1-a-rename",
                Trigger::Rename,
                Duration::from_millis(2500),
                POLL_FAST,
            )?;
            self.m1_trigger(
                "M1-a-delete",
                Trigger::Delete,
                Duration::from_millis(2500),
                POLL_FAST,
            )?;
            self.m1_trigger(
                "M1-b-assoc",
                Trigger::AssocChanged,
                Duration::from_secs(3),
                POLL_FAST,
            )?;
            self.m1_trigger(
                "M1-c-updatedir",
                Trigger::UpdateDir,
                Duration::from_secs(3),
                POLL_FAST,
            )?;
            for area in SETTING_AREAS {
                let id = format!("M1-f-settingchange-{}", area.unwrap_or("null"));
                self.m1_trigger(
                    &id,
                    Trigger::SettingChange(area),
                    Duration::from_millis(1500),
                    POLL_FAST,
                )?;
            }
            self.m1_with_m2_toggle()?;
            self.m1_trigger(
                "M1-d-idle-30s",
                Trigger::Idle,
                Duration::from_secs(30),
                Duration::from_millis(100),
            )?;

            for via in [
                ToggleVia::SendDefView,
                ToggleVia::PostDefView,
                ToggleVia::SendShellParent,
            ] {
                self.m2_toggle_variant(via)?;
            }
            self.m2_trigger("M2-a-create", Trigger::Create, Duration::from_millis(2500))?;
            self.m2_trigger("M2-a-rename", Trigger::Rename, Duration::from_millis(2500))?;
            self.m2_trigger("M2-a-delete", Trigger::Delete, Duration::from_millis(2500))?;
            self.m2_trigger("M2-b-assoc", Trigger::AssocChanged, Duration::from_secs(3))?;
            self.m2_trigger("M2-c-updatedir", Trigger::UpdateDir, Duration::from_secs(3))?;
            self.m2_trigger(
                "M2-f-settingchange-null",
                Trigger::SettingChange(None),
                Duration::from_millis(1500),
            )?;

            for method in [Method::M1, Method::M2] {
                for run in 1..=3 {
                    self.crash_trial(method, false, run)?;
                }
                self.crash_trial(method, true, 1)?;
            }

            for method in [Method::M1, Method::M2] {
                self.lvm_while_hidden(method)?;
            }
            Ok(())
        }

        fn m1_basic(&mut self, how: ShowHow) -> Result<(), String> {
            let id = format!("M1-basic-{}", how.short());
            self.step(&id, |r| {
                let foreground_before = foreground();
                let mark = event_mark();
                let origin = Instant::now();
                let before = IconState::capture();
                let (desktop, hide_call) = m1_hide()?;
                let hidden = IconState::capture();
                let items = listview_item_count(desktop.listview);
                thread::sleep(Duration::from_millis(300));
                let show_start = Instant::now();
                {
                    let _guard = mutation_guard()?;
                    raw_show(desktop.listview, how);
                }
                let show_call = show_start.elapsed();
                let shown = wait_for(Duration::from_secs(2), Duration::from_millis(1), || {
                    visible(desktop.listview)
                });
                let outcome = match shown {
                    Some(time) => format!(
                        "hidden OK; visible again {} after the restore call started",
                        fmt_ms(show_call + time)
                    ),
                    None => "NOT visible 2 s after the restore call".to_owned(),
                };
                let detail = format!(
                    "hide call {}; while hidden: IsWindowVisible={} WS_VISIBLE={} defview_visible={} host_visible={} same HWNDs={} HideIcons reg={} api={} item_count={}; restore call {}; {}; events: {}",
                    fmt_ms(hide_call),
                    b(hidden.listview_visible),
                    b(hidden.listview_style_visible),
                    b(hidden.defview_visible),
                    b(hidden.host_visible),
                    b(hidden.listview == before.listview && hidden.host == before.host),
                    hidden.reg.token(),
                    b(hidden.api_hidden),
                    items.map_or_else(|| "n/a".to_owned(), |count| count.to_string()),
                    fmt_ms(show_call),
                    fg_change(foreground_before, foreground()),
                    events_text(mark, origin)
                );
                r.record(
                    &id,
                    "M1",
                    format!("ShowWindow(SW_HIDE), restore with {}", how.label()),
                    outcome,
                    detail,
                );
                Ok(())
            })
        }

        fn prepare_trigger(&mut self, trigger: Trigger) -> Result<(), String> {
            let needs_file = matches!(trigger, Trigger::Rename | Trigger::Delete);
            if needs_file
                && self
                    .current_probe
                    .as_ref()
                    .is_none_or(|path| !path.exists())
            {
                let path = self.probes.create()?;
                self.current_probe = Some(path.clone());
                let listview = find_desktop()
                    .ok_or("desktop list view not found")?
                    .listview;
                let _ = wait_for(Duration::from_secs(4), Duration::from_millis(100), || {
                    listview_find_item(listview, &path).is_some()
                });
            }
            // 让上一个实验触发的 Explorer 刷新先平静下来。
            thread::sleep(Duration::from_millis(300));
            Ok(())
        }

        fn fire_trigger(
            &mut self,
            trigger: Trigger,
            desktop: &DesktopWindows,
        ) -> Result<(String, Option<(PathBuf, bool)>), String> {
            match trigger {
                Trigger::Create => {
                    let path = self.probes.create()?;
                    self.current_probe = Some(path.clone());
                    Ok((format!("created {}", file_name(&path)), Some((path, true))))
                }
                Trigger::Rename => {
                    let from = self.current_probe.take().ok_or("no probe file to rename")?;
                    let to = self.probes.rename(&from)?;
                    self.current_probe = Some(to.clone());
                    Ok((
                        format!("renamed {} -> {}", file_name(&from), file_name(&to)),
                        Some((to, true)),
                    ))
                }
                Trigger::Delete => {
                    let path = self.current_probe.take().ok_or("no probe file to delete")?;
                    self.probes.delete(&path)?;
                    Ok((format!("deleted {}", file_name(&path)), Some((path, false))))
                }
                Trigger::AssocChanged => {
                    notify_assoc_changed();
                    Ok((
                        "SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST)".to_owned(),
                        None,
                    ))
                }
                Trigger::UpdateDir => {
                    notify_updatedir(&self.desktop_dir);
                    Ok((
                        "SHChangeNotify(SHCNE_UPDATEDIR, SHCNF_PATHW|SHCNF_FLUSHNOWAIT, desktop)"
                            .to_owned(),
                        None,
                    ))
                }
                Trigger::SettingChange(area) => Ok((send_setting_change(desktop, area), None)),
                Trigger::Idle => Ok(("no action".to_owned(), None)),
            }
        }

        fn m1_trigger(
            &mut self,
            id: &str,
            trigger: Trigger,
            observe_for: Duration,
            poll: Duration,
        ) -> Result<(), String> {
            self.step(id, |r| {
                r.prepare_trigger(trigger)?;
                let mark = event_mark();
                let origin = Instant::now();
                let (desktop, _) = m1_hide()?;
                let (action, item) = r.fire_trigger(trigger, &desktop)?;
                let observation = observe(
                    desktop.listview,
                    observe_for,
                    poll,
                    r.initial.reg,
                    item.as_ref()
                        .map(|(path, present)| (path.as_path(), *present)),
                );
                let mut parts = vec![action];
                parts.extend(observation.item_text(&item));
                parts.push(observation.reg_text());
                parts.push(format!("host_visible={}", b(visible(desktop.host))));
                parts.push(format!(
                    "hidden for {:.1} s",
                    origin.elapsed().as_secs_f64()
                ));
                parts.push(format!("events: {}", events_text(mark, origin)));
                r.record(
                    id,
                    "M1",
                    trigger.label(),
                    observation.outcome(),
                    parts.join("; "),
                );
                Ok(())
            })
        }

        fn m1_with_m2_toggle(&mut self) -> Result<(), String> {
            let id = "M1-e-m2-toggle";
            self.step(id, |r| {
                let mark = event_mark();
                let origin = Instant::now();
                let (desktop, _) = m1_hide()?;
                let off = m2_set(true, ToggleVia::SendDefView, Duration::from_secs(2))?;
                let after_off = IconState::capture();
                let mut parts = vec![format!(
                    "M1 hidden, then M2 off: {} -> {}",
                    off.text(),
                    after_off.core()
                )];
                let outcome = if after_off.setting_hidden() {
                    thread::sleep(Duration::from_millis(700));
                    let still_hidden = !visible(desktop.listview);
                    let on = m2_set(false, ToggleVia::SendDefView, Duration::from_secs(2))?;
                    let late = observe(
                        desktop.listview,
                        Duration::from_millis(1500),
                        POLL_FAST,
                        r.initial.reg,
                        None,
                    );
                    let after_on = IconState::capture();
                    parts.push(format!("list view hidden while M1+M2={}", b(still_hidden)));
                    parts.push(format!("M2 back on: {} -> {}", on.text(), after_on.core()));
                    match (on.listview, late.reshown) {
                        (Some(time), _) => format!(
                            "M1 hide UNDONE: Explorer re-showed the list view {} after M2 was switched back",
                            fmt_ms(time)
                        ),
                        (None, Some(time)) => format!(
                            "M1 hide UNDONE: list view re-shown {} after the 2 s transition window",
                            fmt_ms(time)
                        ),
                        (None, None) => {
                            "M1 hide survived: the list view stayed hidden after M2 was switched back"
                                .to_owned()
                        }
                    }
                } else {
                    format!(
                        "the toggle while M1-hidden did NOT switch HideIcons on ({})",
                        after_off.core()
                    )
                };
                parts.push(format!("events: {}", events_text(mark, origin)));
                r.record(
                    id,
                    "M1+M2",
                    "e. M2 off, then back to the original value, while M1-hidden",
                    outcome,
                    parts.join("; "),
                );
                Ok(())
            })
        }

        fn m2_toggle_variant(&mut self, via: ToggleVia) -> Result<(), String> {
            let id = format!("M2-toggle-{}", via.short());
            self.step(&id, |r| {
                let foreground_before = foreground();
                let mark = event_mark();
                let origin = Instant::now();
                let before = IconState::capture();
                let off = m2_set(true, via, Duration::from_secs(3))?;
                let hidden = IconState::capture();
                if !hidden.setting_hidden() && hidden.listview_visible {
                    let detail = format!(
                        "call {}; state {}; events: {}",
                        fmt_ms(off.call),
                        hidden.core(),
                        events_text(mark, origin)
                    );
                    r.record(
                        &id,
                        "M2",
                        format!("WM_COMMAND 0x7402 via {}", via.label()),
                        "NO EFFECT within 3 s (setting and list view unchanged)",
                        detail,
                    );
                    return Ok(());
                }
                let desktop = find_desktop().ok_or("desktop list view not found")?;
                let items = listview_item_count(desktop.listview);
                let same_windows = hidden.listview == before.listview
                    && hidden.defview == before.defview
                    && hidden.host == before.host;
                thread::sleep(Duration::from_millis(300));
                let on = if hidden.setting_hidden() {
                    Some(m2_set(false, via, Duration::from_secs(3))?)
                } else {
                    None
                };
                let shown = IconState::capture();
                let outcome = format!(
                    "OFF: list view hidden after {}, HideIcons=1 after {}, API after {} | ON: list view visible after {}, HideIcons=0 after {}",
                    fmt_opt_ms(off.listview),
                    fmt_opt_ms(off.reg),
                    fmt_opt_ms(off.api),
                    fmt_opt_ms(on.as_ref().and_then(|on| on.listview)),
                    fmt_opt_ms(on.as_ref().and_then(|on| on.reg))
                );
                let detail = format!(
                    "call {}; while hidden: {} WS_VISIBLE={} defview_visible={} host_visible={} same HWNDs={} item_count={}; after switching back: {}; {}; events: {}",
                    fmt_ms(off.call),
                    hidden.core(),
                    b(hidden.listview_style_visible),
                    b(hidden.defview_visible),
                    b(hidden.host_visible),
                    b(same_windows),
                    items.map_or_else(|| "n/a".to_owned(), |count| count.to_string()),
                    shown.core(),
                    fg_change(foreground_before, foreground()),
                    events_text(mark, origin)
                );
                r.record(
                    &id,
                    "M2",
                    format!("WM_COMMAND 0x7402 via {}", via.label()),
                    outcome,
                    detail,
                );
                Ok(())
            })
        }

        fn m2_trigger(
            &mut self,
            id: &str,
            trigger: Trigger,
            observe_for: Duration,
        ) -> Result<(), String> {
            self.step(id, |r| {
                r.prepare_trigger(trigger)?;
                let mark = event_mark();
                let origin = Instant::now();
                let off = m2_set(true, ToggleVia::SendDefView, Duration::from_secs(3))?;
                if !setting_hidden_now() {
                    return Err(format!("HideIcons did not switch on: {}", off.text()));
                }
                let desktop = find_desktop().ok_or("desktop list view not found")?;
                let (action, item) = r.fire_trigger(trigger, &desktop)?;
                let observation = observe(
                    desktop.listview,
                    observe_for,
                    POLL_FAST,
                    RegDword::Value(1),
                    item.as_ref()
                        .map(|(path, present)| (path.as_path(), *present)),
                );
                let api_while_hidden = api_hide_icons();
                let on = m2_set(false, ToggleVia::SendDefView, Duration::from_secs(3))?;
                let mut parts = vec![action];
                parts.extend(observation.item_text(&item));
                parts.push(observation.reg_text());
                parts.push(format!(
                    "API fHideIcons while hidden={}",
                    b(api_while_hidden)
                ));
                parts.push(format!(
                    "switch back: list view visible after {}",
                    fmt_opt_ms(on.listview)
                ));
                parts.push(format!("events: {}", events_text(mark, origin)));
                r.record(
                    id,
                    "M2",
                    trigger.label(),
                    observation.outcome(),
                    parts.join("; "),
                );
                Ok(())
            })
        }

        fn crash_trial(
            &mut self,
            method: Method,
            watchdog_first: bool,
            run: u32,
        ) -> Result<(), String> {
            let id = if watchdog_first {
                format!("E3-{}-watchdog-killed-first", method.label())
            } else {
                format!("E3-{}-kill-parent-{run}", method.label())
            };
            self.step(&id, |r| {
                let log_path = r.temp_path("watchdog");
                let result = r.crash_trial_body(&id, method, watchdog_first, &log_path);
                let _ = fs::remove_file(&log_path);
                result
            })
        }

        fn crash_trial_body(
            &mut self,
            id: &str,
            method: Method,
            watchdog_first: bool,
            log_path: &Path,
        ) -> Result<(), String> {
            let listview = find_desktop()
                .ok_or("desktop list view not found")?
                .listview;
            let mut trial = CrashTrial::start(&self.exe, method, log_path)?;
            let hidden = IconState::capture();
            if hidden.listview_visible || (method == Method::M2 && !hidden.setting_hidden()) {
                return Err(format!(
                    "guard-demo did not hide the icons: {}",
                    hidden.summary()
                ));
            }
            thread::sleep(Duration::from_millis(300));
            let mut notes = vec![format!(
                "guard-demo pid={} watchdog pid={} breakaway_flag={}",
                trial.child.id(),
                trial.watchdog_pid,
                trial.breakaway
            )];
            if watchdog_first {
                let kill = kill_with_stop_process(trial.watchdog_pid, trial.watchdog)?;
                notes.push(format!("watchdog killed first: {}", kill.how));
                kill.reap();
                let noticed =
                    wait_line(&trial.lines, "@@watchdog_exited", Duration::from_secs(2)).is_ok();
                let state = IconState::capture();
                notes.push(format!(
                    "guard-demo still alive, noticed the watchdog exit={}, icons still hidden={}",
                    b(noticed),
                    b(!state.listview_visible)
                ));
            }
            let kill = trial.kill_parent()?;
            let death = kill.death;
            let target = self.target;
            let restored = spin_until(
                Duration::from_secs(if watchdog_first { 3 } else { 6 }),
                || visible(listview) && (method == Method::M1 || reg_hide_icons() == target.reg),
            )
            .map(|_| death.elapsed());
            let after = IconState::capture();
            notes.push(format!("guard-demo killed: {}", kill.how));
            kill.reap();
            let watchdog_exited =
                watchdog_first || trial.wait_watchdog_exit(Duration::from_secs(6));
            let log = fs::read_to_string(log_path).unwrap_or_default();
            let watchdog_ms = log
                .lines()
                .find_map(|line| line.split("RESTORED_MS=").nth(1))
                .and_then(|rest| rest.split_whitespace().next())
                .and_then(|value| value.parse::<f64>().ok());
            notes.push(format!("watchdog exited={}", b(watchdog_exited)));
            if !watchdog_first {
                for line in log.lines().filter(|line| !line.trim().is_empty()) {
                    notes.push(format!("watchdog log: {}", line.trim()));
                }
            }
            let outcome = match (watchdog_first, restored) {
                (false, Some(time)) => {
                    self.crash_samples.push(CrashSample {
                        method,
                        runner_ms: ms_f(time),
                        watchdog_ms,
                    });
                    format!(
                        "RESTORED by the watchdog {} after guard-demo died (watchdog internal {})",
                        fmt_ms(time),
                        watchdog_ms
                            .map_or_else(|| "?".to_owned(), |value| format!("{value:.2} ms"))
                    )
                }
                (false, None) => format!("NOT restored within 6 s: {}", after.core()),
                (true, None) => format!(
                    "NOT restored (no watchdog): still hidden 3 s after guard-demo died; {}",
                    after.core()
                ),
                (true, Some(time)) => format!("unexpectedly restored after {}", fmt_ms(time)),
            };
            drop(trial);
            self.record(
                id,
                method.label(),
                if watchdog_first {
                    "Stop-Process -Force the watchdog, then guard-demo"
                } else {
                    "Stop-Process -Force guard-demo (watchdog alive)"
                },
                outcome,
                notes.join("; "),
            );
            Ok(())
        }

        fn lvm_while_hidden(&mut self, method: Method) -> Result<(), String> {
            let id = format!("E4-lvm-{}", method.label());
            self.step(&id, |r| {
                let desktop = find_desktop().ok_or("desktop list view not found")?;
                let path = r.probes.create()?;
                let appeared = wait_for(Duration::from_secs(4), Duration::from_millis(100), || {
                    listview_find_item(desktop.listview, &path).is_some()
                });
                if appeared.is_none() {
                    let _ = r.probes.delete(&path);
                    return Err("the probe item did not appear in the visible list view within 4 s".to_owned());
                }
                let index = listview_find_item(desktop.listview, &path).ok_or("the probe item vanished")?;
                let original = listview_item_position(desktop.listview, index);
                match method {
                    Method::M1 => {
                        m1_hide()?;
                    }
                    Method::M2 => {
                        m2_set(true, ToggleVia::SendDefView, Duration::from_secs(3))?;
                        if !setting_hidden_now() {
                            let _ = r.probes.delete(&path);
                            return Err("HideIcons did not switch on".to_owned());
                        }
                    }
                }
                let hidden_count = listview_item_count(desktop.listview);
                let hidden_index = listview_find_item(desktop.listview, &path);
                let hidden_position =
                    hidden_index.and_then(|index| listview_item_position(desktop.listview, index));
                let (spacing_x, _) = listview_item_spacing(desktop.listview).unwrap_or((96, 96));
                let width = client_width(desktop.listview);
                let requested = original.map(|(x, y)| {
                    if x + 2 * spacing_x < width {
                        (x + spacing_x, y)
                    } else {
                        (x - spacing_x, y)
                    }
                });
                let sent = match (hidden_index, requested) {
                    (Some(index), Some((x, y))) => {
                        listview_set_item_position(desktop.listview, index, x, y)
                    }
                    _ => false,
                };
                // 隐藏期间多次回读，区分“暂态坐标”和“真正生效的坐标”。
                let mut readbacks = Vec::new();
                let set_at = Instant::now();
                for delay_ms in [0u64, 300, 1000] {
                    let due = set_at + Duration::from_millis(delay_ms);
                    thread::sleep(due.saturating_duration_since(Instant::now()));
                    readbacks.push((
                        delay_ms,
                        hidden_index
                            .and_then(|index| listview_item_position(desktop.listview, index)),
                    ));
                }
                let after_set = readbacks.last().and_then(|(_, position)| *position);
                match method {
                    Method::M1 => {
                        let _guard = mutation_guard()?;
                        raw_show(desktop.listview, ShowHow::ShowNa);
                    }
                    Method::M2 => {
                        m2_set(false, ToggleVia::SendDefView, Duration::from_secs(3))?;
                    }
                }
                let _ = wait_for(Duration::from_secs(1), Duration::from_millis(5), || {
                    visible(desktop.listview)
                });
                thread::sleep(Duration::from_millis(300));
                let visible_index = listview_find_item(desktop.listview, &path);
                let visible_position =
                    visible_index.and_then(|index| listview_item_position(desktop.listview, index));
                r.probes.delete(&path)?;
                let removed = wait_for(Duration::from_secs(4), Duration::from_millis(100), || {
                    listview_find_item(desktop.listview, &path).is_none()
                });
                let readback_matches = after_set.is_some() && after_set == requested;
                let kept = visible_position.is_some() && visible_position == requested;
                let outcome = format!(
                    "while hidden: item text readable={} GETITEMPOSITION ok={} readback==requested={} | after showing: item at requested position={}",
                    b(hidden_index.is_some()),
                    b(hidden_position.is_some()),
                    b(readback_matches),
                    b(kept)
                );
                let readbacks = readbacks
                    .iter()
                    .map(|(delay, position)| format!("+{delay}ms {position:?}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                let detail = format!(
                    "{}; item_count while hidden={}; position before={:?} while hidden={:?}; LVM_SETITEMPOSITION32 requested={:?} (sent={}); read back while hidden: {}; after showing={:?}; probe removed from list view after {}",
                    file_name(&path),
                    hidden_count.map_or_else(|| "n/a".to_owned(), |count| count.to_string()),
                    original,
                    hidden_position,
                    requested,
                    b(sent),
                    readbacks,
                    visible_position,
                    fmt_opt_ms(removed)
                );
                r.record(
                    &id,
                    method.label(),
                    "LVM_GETITEMTEXTW / LVM_GETITEMPOSITION / LVM_SETITEMPOSITION32 on a probe icon while hidden",
                    outcome,
                    detail,
                );
                Ok(())
            })
        }
    }

    fn md(text: &str) -> String {
        text.replace('|', "\\|").replace('\n', " ")
    }

    fn stats(values: &[f64]) -> String {
        if values.is_empty() {
            return "n/a".to_owned();
        }
        let min = values.iter().copied().fold(f64::INFINITY, f64::min);
        let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let avg = values.iter().sum::<f64>() / values.len() as f64;
        format!(
            "min {min:.1} / avg {avg:.1} / max {max:.1} ms (n={})",
            values.len()
        )
    }

    fn print_report(runner: &Runner) {
        println!("\n== RESULTS (markdown) ==");
        println!("| ID | Method | Trigger | Outcome | Details |");
        println!("|---|---|---|---|---|");
        for row in &runner.rows {
            println!(
                "| {} | {} | {} | {} | {} |",
                md(&row.id),
                md(&row.method),
                md(&row.trigger),
                md(&row.outcome),
                md(&row.detail)
            );
        }
        println!();
        for method in [Method::M1, Method::M2] {
            let runner_ms: Vec<f64> = runner
                .crash_samples
                .iter()
                .filter(|sample| sample.method == method)
                .map(|sample| sample.runner_ms)
                .collect();
            let watchdog_ms: Vec<f64> = runner
                .crash_samples
                .iter()
                .filter(|sample| sample.method == method)
                .filter_map(|sample| sample.watchdog_ms)
                .collect();
            println!(
                "CRASH_FALLBACK {}: parent death -> icons visible (observed by runner): {}; watchdog wake -> restore verified (watchdog log): {}",
                method.label(),
                stats(&runner_ms),
                stats(&watchdog_ms)
            );
        }
    }

    fn cmd_experiment_all() -> Result<i32, String> {
        println!("== desktop_icons_probe experiment-all ==");
        print_environment();
        let initial = IconState::capture();
        println!("INITIAL {}", initial.summary());
        if !initial.icons_visible()
            || initial.api_hidden
            || matches!(initial.reg, RegDword::Error(_))
        {
            return Err(format!(
                "the experiments need visible desktop icons as the baseline; current: {}",
                initial.summary()
            ));
        }
        let desktop = find_desktop().ok_or("desktop list view not found")?;
        let desktop_dir = desktop_folder().ok_or("FOLDERID_Desktop is not available")?;
        let target = RestoreTarget::from_state(&initial);
        let _ = fs::write(baseline_path(), initial.core());
        *lock(&EMERGENCY_TARGET) = Some(target);
        install_ctrl_handler();

        let safety_log =
            std::env::temp_dir().join(format!("dustdesk-probe-safety-{}.log", std::process::id()));
        let safety = spawn_watchdog(
            std::process::id(),
            "experiment-all-safety",
            &target,
            Some(&safety_log),
            true,
        )?;
        println!(
            "Safety watchdog pid={} armed in {} (CREATE_BREAKAWAY_FROM_JOB accepted={}); log {} (deleted when nothing had to be restored)",
            safety.child.id(),
            fmt_ms(safety.armed_in),
            b(safety.breakaway),
            safety_log.display()
        );
        WATCH_LISTVIEW.store(desktop.listview as usize, Ordering::SeqCst);
        WATCH_DEFVIEW.store(desktop.defview as usize, Ordering::SeqCst);
        let monitor = EventMonitor::start(window_pid(desktop.listview));
        println!(
            "WinEvent monitor (EVENT_OBJECT_CREATE..HIDE, WINEVENT_OUTOFCONTEXT, explorer pid {}): {}",
            window_pid(desktop.listview),
            if monitor.is_some() { "on" } else { "OFF" }
        );
        print_listview_details();

        let mut runner = Runner {
            initial: initial.clone(),
            target,
            desktop_dir: desktop_dir.clone(),
            probes: ProbeFiles::new(desktop_dir.clone()),
            current_probe: None,
            rows: Vec::new(),
            crash_samples: Vec::new(),
            exe: std::env::current_exe().map_err(|error| error.to_string())?,
            temp_seq: 0,
        };
        let started = Instant::now();
        let outcome = panic::catch_unwind(AssertUnwindSafe(|| runner.run_all()));
        let run_error = match outcome {
            Ok(Ok(())) => None,
            Ok(Err(error)) => Some(error),
            Err(_) => Some("panic (see stderr)".to_owned()),
        };

        // 无论成功与否都做最后一次显式恢复与清理。
        let final_restore =
            restore_locked(&target, &mut |line| println!("[final] restore: {line}"));
        cleanup_probe_files(&mut |line| println!("[final] {line}"));
        sweep_probe_files(&desktop_dir, &mut |line| println!("[final] {line}"));
        drop(monitor);
        print_report(&runner);

        let final_state = IconState::capture();
        let equal = target.matches(&final_state);
        println!("INITIAL {}", initial.core());
        println!("FINAL   {}", final_state.core());
        println!("FINAL_EQUALS_INITIAL={}", if equal { "yes" } else { "NO" });
        println!("total time {:.1} s", started.elapsed().as_secs_f64());
        drop(safety);
        if let Some(error) = &run_error {
            println!("RUN ERROR: {error}");
        }
        if let Err(error) = final_restore {
            println!("FINAL RESTORE ERROR: {error}\n{MANUAL_RECOVERY}");
            return Ok(2);
        }
        Ok(if equal && run_error.is_none() { 0 } else { 2 })
    }
}
