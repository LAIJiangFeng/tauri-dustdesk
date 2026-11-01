export type AppPage = "home" | "organizer" | "launcher" | "search" | "clipboard" | "settings"

export interface DeskCategory {
  name: string
  is_collapsed: boolean
  sort_by_name: boolean
  item_paths: string[]
  item_details: DesktopItem[]
}

export interface CategoryOrderChangedEvent {
  from_index: number
  to_index: number
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

export interface RecycleCategoryItemsResult {
  recycled: number
  skipped: number
  errors: string[]
}

export interface ClipboardCutState {
  paths: string[]
  /** Clipboard sequence number right after the cut was written. */
  sequence: number
  startedAt: number
}

export interface RestoreItemsResult {
  restored: number
  errors: string[]
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

export interface DesktopWindowState {
  locked: boolean
  click_through: boolean
}

export interface DesktopWindowLayout {
  x: number
  y: number
  width: number
  height: number
  monitor_name?: string
  monitor_offset_x?: number
  monitor_offset_y?: number
  monitor_scale_factor?: number
}

export interface DesktopLayout {
  split_category_indices: number[]
  locked: boolean
  click_through: boolean
  windows: Record<string, DesktopWindowLayout>
}

export interface DesktopOperationEvent {
  kind: "classify" | "restore"
  scope: "manual" | "lifecycle"
  status: "started" | "progress" | "finished" | "failed"
  message: string
  moved: number
  skipped: number
  restored: number
  total: number
  current_path: string
  category_counts: CategoryClassifyCount[]
}

export interface DesktopOperationStatus {
  running: boolean
  last?: DesktopOperationEvent | null
}

export interface AppUpdateInfo {
  current_version: string
  latest_version: string
  update_available: boolean
  release_name: string
  release_url: string
  download_url: string
  asset_name: string
  published_at: string
  body: string
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
  clipboard_history_limit: number
  search_enabled: boolean
  search_shortcut: string
  search_paths: string[]
  launch_on_startup: boolean
  screenshot_shortcut: string
  screenshot_hide_window: boolean
  auto_check_updates: boolean
  skipped_update_version: string
  file_dialog_assist_enabled: boolean
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
  organizer_on_desktop_volume: boolean
}

export interface OrganizerDirectoryVolumeCheck {
  same_volume_as_desktop: boolean
  desktop_drive: string
}

export type RuntimeMigrationTarget = "data" | "organizer" | "launchers"

export type RuntimeMigrationPhase = "scanning" | "copying" | "verifying" | "moving" | "switching" | "cleaning" | "done"

/** Payload of `dustdesk://runtime-migration-progress`. */
export interface RuntimeMigrationProgress {
  target: RuntimeMigrationTarget | ""
  phase: RuntimeMigrationPhase
  bytes_done: number
  bytes_total: number
  files_done: number
  files_total: number
  /** Bytes already checked while copying: files by full content comparison, directory items by structure check. */
  verified_bytes: number
  current: string
}

export type DesktopTransferOperation = "archive" | "move" | "restore" | "classify" | "restore-all" | "exit-restore" | "restart-recollect"

/** Payload of `dustdesk://desktop-transfer-progress`, broadcast to every window. */
export interface DesktopTransferProgress {
  operation: DesktopTransferOperation
  status: "running" | "finished" | "failed"
  /** Destination category for archive/move. */
  target: string
  /** 1-based position of the item being processed. */
  item_index: number
  items_total: number
  current: string
  current_path: string
  /** True while an item is copied across volumes; byte counters are only meaningful then. */
  copying: boolean
  bytes_done: number
  bytes_total: number
  verified_bytes: number
}

export interface ScreenshotMonitor {
  index: number
  x: number
  y: number
  width: number
  height: number
  scale_factor: number
}

export interface ScreenshotWindowRect {
  x: number
  y: number
  width: number
  height: number
  title: string
}

export interface ScreenshotSession {
  session_id: string
  monitor: ScreenshotMonitor
  image_url: string
  windows: ScreenshotWindowRect[]
}

export interface ScreenshotStartEvent {
  session_id: string
  monitor_index: number
}

export interface OcrLine {
  text: string
  x: number
  y: number
  width: number
  height: number
}

export interface OcrResult {
  text: string
  lines: OcrLine[]
}

export interface ScreenshotPinInfo {
  pin_id: string
  image_url: string
  width: number
  height: number
}

export type DesktopEdge = "top" | "left" | "right" | "bottom"

export type DesktopEdgeHideMode = "auto" | DesktopEdge

export interface DesktopEdgeHideRect {
  x: number
  y: number
  width: number
  height: number
}

export interface DesktopEdgeHideState {
  label: string
  enabled: boolean
  mode: DesktopEdgeHideMode
  edge: DesktopEdge | null
  collapsed: boolean
  expanded: DesktopEdgeHideRect
  scale_factor: number
}

export interface DesktopEdgeHidePointer {
  inside: boolean
  primary_button_down: boolean
}
