export type AppPage =
  | "home"
  | "organizer"
  | "launcher"
  | "search"
  | "clipboard"
  | "settings"

export interface DeskCategory {
  name: string
  is_collapsed: boolean
  item_paths: string[]
  item_details: DesktopItem[]
}

export interface DesktopItem {
  name: string
  path: string
  extension: string
  is_dir: boolean
  icon_data_url?: string
}

export interface LaunchItem {
  name: string
  path: string
  icon_data_url?: string
}

export interface ClassifyResult {
  moved: number
  skipped: number
  category_counts: CategoryClassifyCount[]
}

export interface DesktopFrameVisibility {
  organizer: boolean
  launcher: boolean
  any: boolean
}

export interface DesktopWindowLayout {
  x: number
  y: number
  width: number
  height: number
}

export interface DesktopLayout {
  split_category_indices: number[]
  windows: Record<string, DesktopWindowLayout>
}

export interface DesktopOperationEvent {
  kind: "classify" | "restore"
  status: "started" | "progress" | "finished" | "failed"
  message: string
  moved: number
  skipped: number
  restored: number
  total: number
  current_path: string
  category_counts: CategoryClassifyCount[]
}

export interface CategoryClassifyCount {
  name: string
  count: number
}

export interface ClipboardHistoryItem {
  id: string
  kind: "Text" | "Image"
  text: string
  image_png_base64: string
  image_path: string
  image_thumb_path: string
  image_hash: string
  created_at: string
  is_locked: boolean
  is_pinned: boolean
}

export type SearchItemKind = "File" | "Directory" | "Launcher"

export interface SearchItem {
  id: string
  name: string
  path: string
  kind: SearchItemKind
  extension: string
  is_dir: boolean
  source: string
  icon_data_url?: string
  open_count: number
  last_opened_at: string
}

export interface SearchOverlayData {
  settings: AppSettings
  paths: string[]
  recent: SearchItem[]
  frequent: SearchItem[]
}

export interface AppSettings {
  clipboard_shortcut: string
  search_enabled: boolean
  search_shortcut: string
  search_paths: string[]
  launch_on_startup: boolean
}

export interface AppSnapshot {
  data_dir: string
  organizer_root: string
  launchers_root: string
  settings: AppSettings
  desktop_layout: DesktopLayout
  categories: DeskCategory[]
  desktop_items: DesktopItem[]
  launchers: LaunchItem[]
  clipboard: ClipboardHistoryItem[]
}
