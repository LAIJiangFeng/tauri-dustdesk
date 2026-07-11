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
        let organizer_root = configured_path(
            &settings_root,
            "organizer-path.txt",
            install_root.join("DesktopOrganizer"),
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
    use windows_sys::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_REPLACE_EXISTING};

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
            MOVEFILE_REPLACE_EXISTING,
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
}
