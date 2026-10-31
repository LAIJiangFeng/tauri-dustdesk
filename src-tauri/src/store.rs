use std::{
    env, fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex, OnceLock,
    },
};

use anyhow::{Context, Result};
use serde::{de::DeserializeOwned, Serialize};

use crate::models::{AppConfig, ClipboardData, LaunchData, SearchHistoryData};

const ORGANIZER_DIR_NAME: &str = "DesktopOrganizer";
const ORGANIZER_VOLUME_SWITCH_MARKER_FILE: &str = "organizer-volume-auto-switch.json";

static ATOMIC_WRITE_COUNTER: AtomicU64 = AtomicU64::new(0);
static STORAGE_MUTATION_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

pub fn with_storage_mutation<T>(
    operation: impl FnOnce() -> std::result::Result<T, String>,
) -> std::result::Result<T, String> {
    let _guard = STORAGE_MUTATION_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| "存储事务锁已损坏".to_owned())?;
    operation()
}

#[derive(Debug, Clone)]
pub struct AppStore {
    data_dir: PathBuf,
    organizer_root: PathBuf,
    launchers_root: PathBuf,
}

impl AppStore {
    pub fn from_runtime_dirs(
        data_dir: PathBuf,
        organizer_root: PathBuf,
        launchers_root: PathBuf,
    ) -> Self {
        Self {
            data_dir,
            organizer_root,
            launchers_root,
        }
    }

    #[cfg(test)]
    pub fn for_test(data_dir: PathBuf, organizer_root: PathBuf, launchers_root: PathBuf) -> Self {
        Self::from_runtime_dirs(data_dir, organizer_root, launchers_root)
    }

    pub fn open() -> Result<Self> {
        let settings_root = app_settings_root()?;
        let install_root = app_install_root()?;
        fs::create_dir_all(&settings_root)
            .with_context(|| format!("create {}", settings_root.display()))?;

        let data_dir = configured_path(
            &settings_root,
            "data-path.txt",
            install_root.join("Data"),
            &[settings_root.join("Data")],
        )?;
        // `open()` runs for nearly every command, so the desktop is only resolved when no organizer
        // path has been persisted yet.
        let organizer_default = if settings_root.join("organizer-path.txt").exists() {
            install_root.join(ORGANIZER_DIR_NAME)
        } else {
            initial_organizer_root(&install_root)
        };
        let organizer_root = configured_path(
            &settings_root,
            "organizer-path.txt",
            organizer_default,
            &[settings_root.join("Data").join("DesktopOrganizer")],
        )?;
        let launchers_root = configured_path(
            &settings_root,
            "launchers-path.txt",
            install_root.join("Launchers"),
            &[settings_root.join("Data").join("Launchers")],
        )?;

        fs::create_dir_all(&data_dir).with_context(|| format!("create {}", data_dir.display()))?;
        Ok(Self {
            data_dir,
            organizer_root,
            launchers_root,
        })
    }

    pub fn set_runtime_directory(target: &str, path: &Path) -> Result<Self> {
        let current = Self::open()?;
        let next = current.with_runtime_directory(target, path)?;
        let normalized = match target {
            "data" => path_to_config_text(&next.data_dir),
            "organizer" => path_to_config_text(&next.organizer_root),
            "launchers" => path_to_config_text(&next.launchers_root),
            _ => anyhow::bail!("未知目录类型"),
        };
        let settings_root = app_settings_root()?;
        fs::create_dir_all(&settings_root)
            .with_context(|| format!("create {}", settings_root.display()))?;
        atomic_write(
            &settings_root.join(runtime_path_file_name(target)?),
            normalized.as_bytes(),
        )?;
        Self::open()
    }

    pub fn save_runtime_migration_journal(bytes: &[u8]) -> Result<()> {
        let path = runtime_migration_journal_path()?;
        atomic_write(&path, bytes)
    }

    pub fn load_runtime_migration_journal() -> Result<Option<String>> {
        let path = runtime_migration_journal_path()?;
        match fs::read_to_string(&path) {
            Ok(content) => Ok(Some(content)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error).with_context(|| format!("read {}", path.display())),
        }
    }

    pub fn remove_runtime_migration_journal() -> Result<()> {
        let path = runtime_migration_journal_path()?;
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error).with_context(|| format!("remove {}", path.display())),
        }
    }

    pub fn save_desktop_organization_restart(bytes: &[u8]) -> Result<()> {
        let path = desktop_organization_restart_path()?;
        atomic_write(&path, bytes)
    }

    pub fn load_desktop_organization_restart() -> Result<Option<String>> {
        let path = desktop_organization_restart_path()?;
        match fs::read_to_string(&path) {
            Ok(content) => Ok(Some(content)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error).with_context(|| format!("read {}", path.display())),
        }
    }

    pub fn remove_desktop_organization_restart() -> Result<()> {
        let path = desktop_organization_restart_path()?;
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error).with_context(|| format!("remove {}", path.display())),
        }
    }

    pub fn settings_root() -> Result<PathBuf> {
        app_settings_root()
    }

    pub fn install_root() -> Result<PathBuf> {
        app_install_root()
    }

    /// The one-time marker means the organizer root must no longer be switched automatically,
    /// either because the switch already happened or because the user picked a directory.
    pub fn organizer_volume_switch_marker_exists() -> Result<bool> {
        let path = organizer_volume_switch_marker_path()?;
        path.try_exists()
            .with_context(|| format!("check {}", path.display()))
    }

    pub fn save_organizer_volume_switch_marker(bytes: &[u8]) -> Result<()> {
        atomic_write(&organizer_volume_switch_marker_path()?, bytes)
    }

    pub fn with_runtime_directory(&self, target: &str, path: &Path) -> Result<Self> {
        if path.as_os_str().is_empty() {
            anyhow::bail!("目录不能为空");
        }

        fs::create_dir_all(path).with_context(|| format!("create {}", path.display()))?;
        let normalized = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let mut next = self.clone();
        match target {
            "data" => next.data_dir = normalized,
            "organizer" => next.organizer_root = normalized,
            "launchers" => next.launchers_root = normalized,
            _ => anyhow::bail!("未知目录类型"),
        };
        Ok(next)
    }

    pub fn config_path(&self) -> PathBuf {
        self.data_dir.join("config.json")
    }

    pub fn launch_path(&self) -> PathBuf {
        self.data_dir.join("launch.json")
    }

    pub fn clipboard_path(&self) -> PathBuf {
        self.data_dir.join("clipboard.json")
    }

    pub fn clipboard_images_root(&self) -> PathBuf {
        self.data_dir.join("ClipboardImages")
    }

    pub fn search_history_path(&self) -> PathBuf {
        self.data_dir.join("search-history.json")
    }

    pub fn data_dir(&self) -> PathBuf {
        self.data_dir.clone()
    }

    pub fn organizer_root(&self) -> PathBuf {
        self.organizer_root.clone()
    }

    pub fn launchers_root(&self) -> PathBuf {
        self.launchers_root.clone()
    }

    pub fn ensure_runtime_dirs(&self) -> Result<()> {
        fs::create_dir_all(self.organizer_root())?;
        fs::create_dir_all(self.launchers_root())?;
        fs::create_dir_all(self.clipboard_images_root())?;
        Ok(())
    }

    pub fn load_config(&self) -> AppConfig {
        self.load_json(self.config_path())
    }

    pub fn load_config_strict(&self) -> Result<AppConfig> {
        self.load_json_strict(self.config_path())
    }

    pub fn load_launchers(&self) -> LaunchData {
        self.load_json(self.launch_path())
    }

    pub fn load_launchers_strict(&self) -> Result<LaunchData> {
        self.load_json_strict(self.launch_path())
    }

    pub fn load_clipboard(&self) -> ClipboardData {
        self.load_json(self.clipboard_path())
    }

    pub fn load_clipboard_strict(&self) -> Result<ClipboardData> {
        self.load_json_strict(self.clipboard_path())
    }

    pub fn load_search_history(&self) -> SearchHistoryData {
        self.load_json(self.search_history_path())
    }

    pub fn load_search_history_strict(&self) -> Result<SearchHistoryData> {
        self.load_json_strict(self.search_history_path())
    }

    pub fn save_config(&self, config: &AppConfig) -> Result<()> {
        self.save_json(self.config_path(), config)
    }

    pub fn save_clipboard(&self, clipboard: &ClipboardData) -> Result<()> {
        self.save_json(self.clipboard_path(), clipboard)
    }

    pub fn save_launchers(&self, launchers: &LaunchData) -> Result<()> {
        self.save_json(self.launch_path(), launchers)
    }

    pub fn save_search_history(&self, history: &SearchHistoryData) -> Result<()> {
        self.save_json(self.search_history_path(), history)
    }

    fn load_json<T>(&self, path: PathBuf) -> T
    where
        T: DeserializeOwned + Default,
    {
        let Ok(json) = fs::read_to_string(&path) else {
            return T::default();
        };

        serde_json::from_str(&json).unwrap_or_default()
    }

    fn load_json_strict<T>(&self, path: PathBuf) -> Result<T>
    where
        T: DeserializeOwned + Default,
    {
        let json = match fs::read_to_string(&path) {
            Ok(json) => json,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(T::default()),
            Err(error) => {
                return Err(error).with_context(|| format!("read {}", path.display()));
            }
        };
        serde_json::from_str(&json).with_context(|| format!("parse {}", path.display()))
    }

    fn save_json<T>(&self, path: PathBuf, value: &T) -> Result<()>
    where
        T: Serialize,
    {
        fs::create_dir_all(&self.data_dir)
            .with_context(|| format!("create {}", self.data_dir.display()))?;
        let json = serde_json::to_string_pretty(value)?;
        atomic_write(&path, json.as_bytes())?;
        Ok(())
    }
}

fn app_settings_root() -> Result<PathBuf> {
    let appdata = env::var_os("APPDATA").context("APPDATA environment variable is missing")?;
    Ok(PathBuf::from(appdata).join("DustDesk"))
}

fn runtime_migration_journal_path() -> Result<PathBuf> {
    Ok(app_settings_root()?.join("runtime-migration.json"))
}

fn desktop_organization_restart_path() -> Result<PathBuf> {
    Ok(app_settings_root()?.join("desktop-organization-restart.json"))
}

fn organizer_volume_switch_marker_path() -> Result<PathBuf> {
    Ok(app_settings_root()?.join(ORGANIZER_VOLUME_SWITCH_MARKER_FILE))
}

fn app_install_root() -> Result<PathBuf> {
    let executable = env::current_exe().context("current executable path is unavailable")?;
    executable
        .parent()
        .map(Path::to_path_buf)
        .context("current executable parent directory is unavailable")
}

fn configured_path(
    settings_root: &Path,
    file_name: &str,
    default_dir: PathBuf,
    _legacy_defaults: &[PathBuf],
) -> Result<PathBuf> {
    let path_file = settings_root.join(file_name);
    if path_file.exists() {
        let configured = fs::read_to_string(&path_file)
            .with_context(|| format!("read {}", path_file.display()))?
            .trim()
            .to_owned();
        if !configured.is_empty() {
            let normalized = normalize_configured_path_text(&configured);
            let configured_path = PathBuf::from(&normalized);
            return Ok(configured_path);
        }
    } else {
        atomic_write(&path_file, path_to_config_text(&default_dir).as_bytes())
            .with_context(|| format!("write {}", path_file.display()))?;
    }

    Ok(default_dir)
}

/// Organizer root for an installation that never persisted one. Keeping it on the desktop volume
/// lets organizing and restoring use renames instead of cross-volume copies.
fn initial_organizer_root(install_root: &Path) -> PathBuf {
    let desktop = crate::user_desktop();
    let local_app_data = env::var_os("LOCALAPPDATA").map(PathBuf::from);
    let desktop_volume_root = desktop.as_deref().and_then(volume_root_path);
    select_default_organizer_root(
        desktop.as_deref(),
        install_root,
        local_app_data.as_deref(),
        desktop_volume_root.as_deref(),
        |left, right| paths_share_volume(left, right) == Some(true),
        can_create_directory,
    )
}

/// Picks the default organizer root: the first candidate that shares the desktop volume and can
/// be created, otherwise `<install>\DesktopOrganizer`.
pub fn select_default_organizer_root(
    desktop: Option<&Path>,
    install_root: &Path,
    local_app_data: Option<&Path>,
    desktop_volume_root: Option<&Path>,
    same_volume: impl Fn(&Path, &Path) -> bool,
    can_create: impl Fn(&Path) -> bool,
) -> PathBuf {
    desktop
        .and_then(|desktop| {
            desktop_volume_organizer_root(
                desktop,
                install_root,
                local_app_data,
                desktop_volume_root,
                same_volume,
                can_create,
            )
        })
        .unwrap_or_else(|| install_root.join(ORGANIZER_DIR_NAME))
}

/// First organizer root candidate on the desktop volume that can be created, in the order
/// `<install>\DesktopOrganizer`, `%LOCALAPPDATA%\DustDesk\DesktopOrganizer`,
/// `<desktop volume root>\DustDesk\DesktopOrganizer`.
pub fn desktop_volume_organizer_root(
    desktop: &Path,
    install_root: &Path,
    local_app_data: Option<&Path>,
    desktop_volume_root: Option<&Path>,
    same_volume: impl Fn(&Path, &Path) -> bool,
    can_create: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    let mut candidates = vec![install_root.join(ORGANIZER_DIR_NAME)];
    candidates.extend(local_app_data.map(|root| root.join("DustDesk").join(ORGANIZER_DIR_NAME)));
    candidates
        .extend(desktop_volume_root.map(|root| root.join("DustDesk").join(ORGANIZER_DIR_NAME)));
    candidates
        .into_iter()
        .find(|candidate| same_volume(candidate, desktop) && can_create(candidate))
}

pub fn can_create_directory(path: &Path) -> bool {
    fs::create_dir_all(path).is_ok() && path.is_dir()
}

/// Whether both paths are stored on the same volume, judging a missing path by its nearest
/// existing ancestor. `None` means a volume could not be identified.
#[cfg(windows)]
pub fn paths_share_volume(left: &Path, right: &Path) -> Option<bool> {
    Some(volume_serial_number(left)? == volume_serial_number(right)?)
}

#[cfg(not(windows))]
pub fn paths_share_volume(_left: &Path, _right: &Path) -> Option<bool> {
    Some(true)
}

#[cfg(windows)]
fn volume_serial_number(path: &Path) -> Option<u32> {
    use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    // Without FILE_FLAG_OPEN_REPARSE_POINT the open follows junctions, symlinks and mounted
    // folders, so the serial belongs to the volume that really stores the path.
    let file = fs::OpenOptions::new()
        .access_mode(0)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(nearest_existing_path(path)?)
        .ok()?;
    let mut information = BY_HANDLE_FILE_INFORMATION::default();
    let succeeded =
        unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, &mut information) } != 0;
    succeeded.then_some(information.dwVolumeSerialNumber)
}

/// Mount point of the volume that stores `path`, for example `C:\`.
#[cfg(windows)]
pub fn volume_root_path(path: &Path) -> Option<PathBuf> {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use windows_sys::Win32::Storage::FileSystem::GetVolumePathNameW;

    let existing = nearest_existing_path(path)?;
    // Resolve junctions and symlinks first so the mount point belongs to the stored location.
    let resolved = existing.canonicalize().unwrap_or(existing);
    let wide = resolved
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    // The API works in place, so the buffer must be able to hold the whole input path.
    let mut buffer = vec![0u16; wide.len().max(260) + 2];
    let length = u32::try_from(buffer.len()).ok()?;
    if unsafe { GetVolumePathNameW(wide.as_ptr(), buffer.as_mut_ptr(), length) } == 0 {
        return None;
    }
    let end = buffer.iter().position(|unit| *unit == 0)?;
    let root = std::ffi::OsString::from_wide(&buffer[..end]);
    Some(without_verbatim_prefix(Path::new(&root)))
}

#[cfg(not(windows))]
pub fn volume_root_path(path: &Path) -> Option<PathBuf> {
    path.ancestors().last().map(Path::to_path_buf)
}

#[cfg(windows)]
fn nearest_existing_path(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .find(|ancestor| !ancestor.as_os_str().is_empty() && ancestor.exists())
        .map(Path::to_path_buf)
}

/// `path` without the `\\?\` prefix that `canonicalize` adds on Windows.
pub fn without_verbatim_prefix(path: &Path) -> PathBuf {
    PathBuf::from(path_to_config_text(path))
}

fn runtime_path_file_name(target: &str) -> Result<&'static str> {
    match target {
        "data" => Ok("data-path.txt"),
        "organizer" => Ok("organizer-path.txt"),
        "launchers" => Ok("launchers-path.txt"),
        _ => anyhow::bail!("未知目录类型"),
    }
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .with_context(|| format!("missing parent for {}", path.display()))?;
    fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("dustdesk-data");

    for _ in 0..1000 {
        let sequence = ATOMIC_WRITE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let temporary = parent.join(format!(
            ".{file_name}.{}.{}.tmp",
            std::process::id(),
            sequence
        ));
        let mut file = match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        };

        let result = (|| -> Result<()> {
            file.write_all(bytes)?;
            file.sync_all()?;
            drop(file);
            replace_file_atomically(&temporary, path)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        return result;
    }

    anyhow::bail!("unable to reserve temporary file for {}", path.display())
}

#[cfg(windows)]
fn replace_file_atomically(source: &Path, destination: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    if unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

#[cfg(not(windows))]
fn replace_file_atomically(source: &Path, destination: &Path) -> Result<()> {
    fs::rename(source, destination)?;
    Ok(())
}

fn path_to_config_text(path: &Path) -> String {
    normalize_configured_path_text(&path.to_string_lossy())
}

fn normalize_configured_path_text(path: &str) -> String {
    let trimmed = path.trim();
    if let Some(rest) = trimmed.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = trimmed.strip_prefix(r"\\?\") {
        rest.to_owned()
    } else {
        trimmed.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn atomic_write_replaces_complete_file_without_leaving_temporary_files() {
        let root = std::env::temp_dir().join(format!(
            "dustdesk-store-atomic-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time")
                .as_nanos()
        ));
        fs::create_dir_all(&root).expect("create test root");
        let path = root.join("config.json");
        fs::write(&path, b"old-config").expect("write old config");

        atomic_write(&path, b"new-complete-config").expect("replace config atomically");

        assert_eq!(
            fs::read(&path).expect("read replaced config"),
            b"new-complete-config"
        );
        assert!(
            fs::read_dir(&root)
                .expect("read test root")
                .all(|entry| !entry
                    .expect("read entry")
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".tmp")),
            "successful atomic writes must not leave staging files"
        );

        let _ = fs::remove_dir_all(root);
    }

    fn same_test_drive(left: &Path, right: &Path) -> bool {
        let drive = |path: &Path| {
            path.to_string_lossy()
                .chars()
                .next()
                .map(|drive| drive.to_ascii_uppercase())
        };
        drive(left).is_some() && drive(left) == drive(right)
    }

    #[test]
    fn default_organizer_root_keeps_install_directory_on_desktop_volume() {
        let selected = select_default_organizer_root(
            Some(Path::new(r"C:\Users\me\Desktop")),
            Path::new(r"C:\Apps\DustDesk"),
            Some(Path::new(r"C:\Users\me\AppData\Local")),
            Some(Path::new(r"C:\")),
            same_test_drive,
            |_| true,
        );

        assert_eq!(
            selected,
            PathBuf::from(r"C:\Apps\DustDesk\DesktopOrganizer")
        );
    }

    #[test]
    fn default_organizer_root_uses_local_app_data_when_install_is_on_another_volume() {
        let selected = select_default_organizer_root(
            Some(Path::new(r"C:\Users\me\Desktop")),
            Path::new(r"D:\Software\DustDesk"),
            Some(Path::new(r"C:\Users\me\AppData\Local")),
            Some(Path::new(r"C:\")),
            same_test_drive,
            |_| true,
        );

        assert_eq!(
            selected,
            PathBuf::from(r"C:\Users\me\AppData\Local\DustDesk\DesktopOrganizer")
        );
    }

    #[test]
    fn default_organizer_root_uses_desktop_volume_root_when_other_candidates_differ() {
        let selected = select_default_organizer_root(
            Some(Path::new(r"E:\Redirected\Desktop")),
            Path::new(r"D:\Software\DustDesk"),
            Some(Path::new(r"C:\Users\me\AppData\Local")),
            Some(Path::new(r"E:\")),
            same_test_drive,
            |_| true,
        );

        assert_eq!(selected, PathBuf::from(r"E:\DustDesk\DesktopOrganizer"));
    }

    #[test]
    fn default_organizer_root_skips_desktop_volume_candidates_that_cannot_be_created() {
        let local_app_data = PathBuf::from(r"C:\Users\me\AppData\Local");
        let selected = select_default_organizer_root(
            Some(Path::new(r"C:\Users\me\Desktop")),
            Path::new(r"D:\Software\DustDesk"),
            Some(&local_app_data),
            Some(Path::new(r"C:\")),
            same_test_drive,
            |candidate| !candidate.starts_with(&local_app_data),
        );

        assert_eq!(selected, PathBuf::from(r"C:\DustDesk\DesktopOrganizer"));
    }

    #[test]
    fn default_organizer_root_falls_back_to_install_directory() {
        let install_root = Path::new(r"D:\Software\DustDesk");
        let fallback = install_root.join("DesktopOrganizer");
        let desktop = Path::new(r"C:\Users\me\Desktop");
        let local_app_data = Path::new(r"C:\Users\me\AppData\Local");

        let nothing_creatable = select_default_organizer_root(
            Some(desktop),
            install_root,
            Some(local_app_data),
            Some(Path::new(r"C:\")),
            same_test_drive,
            |_| false,
        );
        let nothing_on_desktop_volume = select_default_organizer_root(
            Some(Path::new(r"F:\Desktop")),
            install_root,
            Some(local_app_data),
            None,
            same_test_drive,
            |_| true,
        );
        let desktop_unknown = select_default_organizer_root(
            None,
            install_root,
            Some(local_app_data),
            Some(Path::new(r"C:\")),
            |_, _| true,
            |_| true,
        );

        assert_eq!(nothing_creatable, fallback);
        assert_eq!(nothing_on_desktop_volume, fallback);
        assert_eq!(desktop_unknown, fallback);
    }

    #[cfg(windows)]
    #[test]
    fn volume_helpers_judge_missing_paths_by_their_existing_ancestors() {
        let root = std::env::temp_dir().join(format!(
            "dustdesk-store-volume-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time")
                .as_nanos()
        ));
        let existing = root.join("existing");
        fs::create_dir_all(&existing).expect("create existing directory");
        let missing = root.join("missing").join("DesktopOrganizer");

        assert_eq!(paths_share_volume(&existing, &missing), Some(true));
        let volume_root = volume_root_path(&missing).expect("resolve volume root");
        assert!(!volume_root.to_string_lossy().starts_with(r"\\?\"));
        assert_eq!(paths_share_volume(&volume_root, &existing), Some(true));
        assert!(
            !missing.exists(),
            "volume checks must not create directories"
        );

        let _ = fs::remove_dir_all(root);
    }
}
