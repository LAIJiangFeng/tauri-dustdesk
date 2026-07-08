use std::{
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::{de::DeserializeOwned, Serialize};

use crate::models::{AppConfig, ClipboardData, LaunchData, SearchHistoryData};

#[derive(Debug, Clone)]
pub struct AppStore {
    data_dir: PathBuf,
    organizer_root: PathBuf,
    launchers_root: PathBuf,
}

impl AppStore {
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
        if path.as_os_str().is_empty() {
            anyhow::bail!("目录不能为空");
        }

        fs::create_dir_all(path).with_context(|| format!("create {}", path.display()))?;
        let normalized = path
            .canonicalize()
            .unwrap_or_else(|_| path.to_path_buf())
            .display()
            .to_string();
        let settings_root = app_settings_root()?;
        fs::create_dir_all(&settings_root)
            .with_context(|| format!("create {}", settings_root.display()))?;
        let file_name = match target {
            "data" => "data-path.txt",
            "organizer" => "organizer-path.txt",
            "launchers" => "launchers-path.txt",
            _ => anyhow::bail!("未知目录类型"),
        };

        fs::write(settings_root.join(file_name), normalized.as_bytes())
            .with_context(|| format!("write {}", file_name))?;
        Self::open()
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

    pub fn load_launchers(&self) -> LaunchData {
        self.load_json(self.launch_path())
    }

    pub fn load_clipboard(&self) -> ClipboardData {
        self.load_json(self.clipboard_path())
    }

    pub fn load_search_history(&self) -> SearchHistoryData {
        self.load_json(self.search_history_path())
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

    fn save_json<T>(&self, path: PathBuf, value: &T) -> Result<()>
    where
        T: Serialize,
    {
        fs::create_dir_all(&self.data_dir)
            .with_context(|| format!("create {}", self.data_dir.display()))?;
        let json = serde_json::to_string_pretty(value)?;
        fs::write(path, json)?;
        Ok(())
    }
}

fn app_settings_root() -> Result<PathBuf> {
    let appdata = env::var_os("APPDATA").context("APPDATA environment variable is missing")?;
    Ok(PathBuf::from(appdata).join("DustDesk"))
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
    legacy_defaults: &[PathBuf],
) -> Result<PathBuf> {
    let path_file = settings_root.join(file_name);
    if path_file.exists() {
        let configured = fs::read_to_string(&path_file)
            .with_context(|| format!("read {}", path_file.display()))?
            .trim()
            .to_owned();
        if !configured.is_empty() {
            let configured_path = PathBuf::from(&configured);
            if legacy_defaults
                .iter()
                .any(|legacy| same_path_for_config(&configured_path, legacy))
            {
                fs::write(&path_file, default_dir.to_string_lossy().as_bytes())
                    .with_context(|| format!("write {}", path_file.display()))?;
                return Ok(default_dir);
            }

            return Ok(configured_path);
        }
    } else {
        fs::write(&path_file, default_dir.to_string_lossy().as_bytes())
            .with_context(|| format!("write {}", path_file.display()))?;
    }

    Ok(default_dir)
}

fn same_path_for_config(left: &Path, right: &Path) -> bool {
    normalize_path_for_config(left) == normalize_path_for_config(right)
}

fn normalize_path_for_config(path: &Path) -> String {
    path.to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_ascii_lowercase()
}
