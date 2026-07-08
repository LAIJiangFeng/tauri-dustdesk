use std::path::Path;

use chrono::Local;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSnapshot {
    pub data_dir: String,
    pub organizer_root: String,
    pub launchers_root: String,
    pub settings: AppSettings,
    pub categories: Vec<DeskCategory>,
    pub desktop_items: Vec<DesktopItem>,
    pub launchers: Vec<LaunchItem>,
    pub clipboard: Vec<ClipboardHistoryItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(rename = "DesktopCategories", default = "default_categories")]
    pub desktop_categories: Vec<DeskCategory>,

    #[serde(rename = "Settings", alias = "settings", default)]
    pub settings: AppSettings,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            desktop_categories: default_categories(),
            settings: AppSettings::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    #[serde(
        rename = "ClipboardShortcut",
        alias = "clipboard_shortcut",
        default = "default_clipboard_shortcut"
    )]
    pub clipboard_shortcut: String,

    #[serde(
        rename = "SearchEnabled",
        alias = "search_enabled",
        default = "default_search_enabled"
    )]
    pub search_enabled: bool,

    #[serde(
        rename = "SearchShortcut",
        alias = "search_shortcut",
        default = "default_search_shortcut"
    )]
    pub search_shortcut: String,

    #[serde(rename = "SearchPaths", alias = "search_paths", default)]
    pub search_paths: Vec<String>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            clipboard_shortcut: default_clipboard_shortcut(),
            search_enabled: default_search_enabled(),
            search_shortcut: default_search_shortcut(),
            search_paths: Vec::new(),
        }
    }
}

impl AppSettings {
    pub fn clipboard_shortcut_value(&self) -> String {
        if self.clipboard_shortcut.trim().is_empty() {
            default_clipboard_shortcut()
        } else {
            self.clipboard_shortcut.trim().to_owned()
        }
    }

    pub fn search_shortcut_value(&self) -> String {
        if self.search_shortcut.trim().is_empty() {
            default_search_shortcut()
        } else {
            self.search_shortcut.trim().to_owned()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DeskCategory {
    #[serde(rename = "Name", alias = "name", default)]
    pub name: String,

    #[serde(rename = "IsCollapsed", alias = "is_collapsed", default)]
    pub is_collapsed: bool,

    #[serde(rename = "ItemPaths", alias = "item_paths", default)]
    pub item_paths: Vec<String>,

    #[serde(
        rename = "ItemDetails",
        alias = "item_details",
        default,
        skip_deserializing,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub item_details: Vec<DesktopItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathIconResult {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon_data_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesktopItem {
    pub name: String,
    pub path: String,
    pub extension: String,
    pub is_dir: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon_data_url: Option<String>,
}

impl DesktopItem {
    pub fn from_path(path: &Path) -> Self {
        let is_dir = path.is_dir();
        Self {
            name: display_name(path),
            path: path.display().to_string(),
            extension: if is_dir {
                "DIR".to_owned()
            } else {
                file_extension(path)
            },
            is_dir,
            icon_data_url: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassifyResult {
    pub moved: usize,
    pub skipped: usize,
    pub category_counts: Vec<CategoryClassifyCount>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CategoryClassifyCount {
    pub name: String,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LaunchData {
    #[serde(rename = "Items", default)]
    pub items: Vec<LaunchItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LaunchItem {
    #[serde(rename = "Name", alias = "name", default)]
    pub name: String,

    #[serde(rename = "Path", alias = "path", default)]
    pub path: String,

    #[serde(
        rename = "IconDataUrl",
        alias = "icon_data_url",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub icon_data_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ClipboardData {
    #[serde(rename = "Items", default)]
    pub items: Vec<ClipboardHistoryItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SearchHistoryData {
    #[serde(rename = "Items", default)]
    pub items: Vec<SearchHistoryItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchHistoryItem {
    #[serde(rename = "Id", alias = "id", default = "new_id")]
    pub id: String,

    #[serde(rename = "Name", alias = "name", default)]
    pub name: String,

    #[serde(rename = "Path", alias = "path", default)]
    pub path: String,

    #[serde(rename = "Kind", alias = "kind", default)]
    pub kind: SearchItemKind,

    #[serde(rename = "Extension", alias = "extension", default)]
    pub extension: String,

    #[serde(rename = "OpenCount", alias = "open_count", default)]
    pub open_count: u32,

    #[serde(
        rename = "LastOpenedAt",
        alias = "last_opened_at",
        default = "now_local"
    )]
    pub last_opened_at: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum SearchItemKind {
    #[default]
    File,
    Directory,
    Launcher,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchItem {
    pub id: String,
    pub name: String,
    pub path: String,
    pub kind: SearchItemKind,
    pub extension: String,
    pub is_dir: bool,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon_data_url: Option<String>,
    pub open_count: u32,
    pub last_opened_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchOverlayData {
    pub settings: AppSettings,
    pub paths: Vec<String>,
    pub recent: Vec<SearchItem>,
    pub frequent: Vec<SearchItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipboardHistoryItem {
    #[serde(rename = "Id", alias = "id", default = "new_id")]
    pub id: String,

    #[serde(rename = "Kind", alias = "kind", default)]
    pub kind: ClipboardHistoryKind,

    #[serde(rename = "Text", alias = "text", default)]
    pub text: String,

    #[serde(rename = "ImagePngBase64", alias = "image_png_base64", default)]
    pub image_png_base64: String,

    #[serde(rename = "ImagePath", alias = "image_path", default)]
    pub image_path: String,

    #[serde(rename = "ImageThumbPath", alias = "image_thumb_path", default)]
    pub image_thumb_path: String,

    #[serde(rename = "ImageHash", alias = "image_hash", default)]
    pub image_hash: String,

    #[serde(rename = "CreatedAt", alias = "created_at", default = "now_local")]
    pub created_at: String,

    #[serde(rename = "IsLocked", alias = "is_locked", default)]
    pub is_locked: bool,

    #[serde(rename = "IsPinned", alias = "is_pinned", default)]
    pub is_pinned: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum ClipboardHistoryKind {
    #[default]
    Text,
    Image,
}

fn now_local() -> String {
    Local::now().format("%Y-%m-%dT%H:%M:%S%.3f").to_string()
}

fn new_id() -> String {
    let now = Local::now();
    format!("t{:x}{:x}", now.timestamp_millis(), std::process::id())
}

fn default_categories() -> Vec<DeskCategory> {
    [
        "开发",
        "工具",
        "文档",
        "社交",
        "游戏",
        "办公软件",
        "浏览器",
        "本机",
    ]
    .into_iter()
    .map(|name| DeskCategory {
        name: name.to_owned(),
        is_collapsed: false,
        item_paths: Vec::new(),
        item_details: Vec::new(),
    })
    .collect()
}

fn default_clipboard_shortcut() -> String {
    "Ctrl+Tab".to_owned()
}

fn default_search_enabled() -> bool {
    true
}

fn default_search_shortcut() -> String {
    "Ctrl+Space".to_owned()
}

fn display_name(path: &Path) -> String {
    path.file_stem()
        .or_else(|| path.file_name())
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string())
}

fn file_extension(path: &Path) -> String {
    path.extension()
        .map(|name| name.to_string_lossy().to_uppercase())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "FILE".to_owned())
        .chars()
        .take(6)
        .collect()
}
