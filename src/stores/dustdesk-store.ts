import { invoke } from "@tauri-apps/api/core"
import { create } from "zustand"
import { immer } from "zustand/middleware/immer"
import type { DesktopDropPosition } from "@/lib/dustdesk-dnd"
import { displayPathName, displayWindowsEntryName, extensionFromPath } from "@/lib/utils"
import type {
  AppPage,
  AppSettings,
  AppSnapshot,
  AppUpdateInfo,
  ClassifyResult,
  ClipboardHistoryItem,
  DeskCategory,
  DesktopFrameVisibility,
  DesktopLayout,
  DesktopOperationStatus,
  DesktopWindowLayout,
  LaunchItem,
  SearchItem,
  SearchItemKind,
  SearchOverlayData,
} from "@/types"

const defaultSettings: AppSettings = {
  clipboard_shortcut: "Ctrl+Tab",
  search_enabled: true,
  search_shortcut: "Ctrl+Space",
  search_paths: [],
  launch_on_startup: false,
}

const emptyDesktopLayout: DesktopLayout = {
  split_category_indices: [],
  windows: {},
}

const emptySnapshot: AppSnapshot = {
  data_dir: "",
  organizer_root: "",
  launchers_root: "",
  settings: defaultSettings,
  desktop_layout: emptyDesktopLayout,
  categories: [],
  desktop_items: [],
  launchers: [],
  clipboard: [],
}

const hiddenDesktopFrames: DesktopFrameVisibility = {
  organizer: false,
  launcher: false,
  any: false,
}

interface PathIconResult {
  path: string
  icon_data_url?: string
}

interface IconResolutionOptions {
  includeDesktopItems?: boolean
  includeLaunchers?: boolean
  categoryIndices?: number[]
}

interface IconFailureState {
  attempts: number
  retryAfter: number
}

interface DesktopSnapshotLoadOptions {
  force?: boolean
  preserveDesktopItems?: boolean
  iconOptions?: IconResolutionOptions | false
}

interface SnapshotLoadOptions {
  force?: boolean
}

interface RestoreDesktopArgs extends Record<string, unknown> {
  index: number
  path: string
  position?: DesktopDropPosition
}

const iconBatchSize = 24
const iconPathLimit = 512
const iconFailureRetryDelayMs = 900
const iconFailureRetryLimit = 2
const iconFailureEntryLimit = 2048
const iconFailureCooldownMs = 5000
const snapshotReloadDedupeMs = 120
const desktopSnapshotReloadDedupeMs = 120
let iconResolutionToken = 0
let iconResolutionActive = false
let queuedIconResolutionOptions: IconResolutionOptions | null = null
let queuedIconRetryOptions: IconResolutionOptions | null = null
let iconRetryTimer: ReturnType<typeof setTimeout> | null = null
const iconCache = new Map<string, string>()
const iconFailures = new Map<string, IconFailureState>()
let snapshotLoadPromise: Promise<void> | null = null
let snapshotForceReloadQueued = false
let snapshotLoadedAt = 0
let desktopSnapshotLoadPromise: Promise<void> | null = null
let desktopSnapshotForceReloadQueued = false
let desktopSnapshotLoadedAt = 0

function resetRuntimeCaches() {
  iconResolutionToken += 1
  queuedIconResolutionOptions = null
  queuedIconRetryOptions = null
  if (iconRetryTimer !== null) {
    clearTimeout(iconRetryTimer)
    iconRetryTimer = null
  }
  iconCache.clear()
  iconFailures.clear()
  snapshotLoadPromise = null
  snapshotForceReloadQueued = false
  snapshotLoadedAt = 0
  desktopSnapshotLoadPromise = null
  desktopSnapshotForceReloadQueued = false
  desktopSnapshotLoadedAt = 0
}

function waitForNextPaint() {
  if (typeof globalThis.requestAnimationFrame !== "function") {
    return Promise.resolve()
  }

  return new Promise<void>((resolve) => {
    globalThis.requestAnimationFrame(() => resolve())
  })
}

function restoreDesktopArgs(index: number, path: string, position?: DesktopDropPosition): RestoreDesktopArgs {
  const args: RestoreDesktopArgs = { index, path }
  if (position) args.position = position
  return args
}

const demoSnapshot: AppSnapshot = {
  data_dir: "<安装目录>\\Data",
  organizer_root: "<安装目录>\\DesktopOrganizer",
  launchers_root: "<安装目录>\\Launchers",
  settings: defaultSettings,
  desktop_layout: emptyDesktopLayout,
  categories: [
    { name: "开发", is_collapsed: false, item_paths: [], item_details: [] },
    { name: "工具", is_collapsed: false, item_paths: [], item_details: [] },
    { name: "文档", is_collapsed: false, item_paths: [], item_details: [] },
    { name: "社交", is_collapsed: false, item_paths: [], item_details: [] },
    { name: "游戏", is_collapsed: false, item_paths: [], item_details: [] },
  ],
  desktop_items: [
    {
      name: "Apifox",
      path: "C:\\Users\\Desktop\\Apifox.lnk",
      extension: "LNK",
      is_dir: false,
    },
    {
      name: "Cursor",
      path: "C:\\Users\\Desktop\\Cursor.lnk",
      extension: "LNK",
      is_dir: false,
    },
    {
      name: "Project Assets",
      path: "C:\\Users\\Desktop\\Project Assets",
      extension: "DIR",
      is_dir: true,
    },
    {
      name: "design-reference",
      path: "C:\\Users\\Desktop\\design-reference.png",
      extension: "PNG",
      is_dir: false,
    },
  ],
  launchers: [
    { name: "Cursor", path: "C:\\Users\\Desktop\\Cursor.lnk" },
    { name: "Apifox", path: "C:\\Users\\Desktop\\Apifox.lnk" },
  ],
  clipboard: [],
}

type InvokeArgs = Record<string, unknown>

function isTauriRuntime() {
  return "__TAURI_INTERNALS__" in window
}

async function call<T>(command: string, args?: InvokeArgs): Promise<T> {
  if (!isTauriRuntime()) {
    if (command === "load_snapshot" || command === "load_desktop_snapshot" || command === "update_runtime_directory") {
      return normalizeSnapshot(demoSnapshot) as T
    }
    if (command === "resolve_path_icons") {
      return asArray(args?.paths).map((path) => ({
        path: asPathString(path),
      })) as T
    }
    if (command === "load_search_overlay") {
      return demoSearchOverlay() as T
    }
    if (command === "search_items") {
      return demoSearchItems(asString(args?.query)) as T
    }
    if (command === "update_clipboard_shortcut") {
      return normalizeSettings({
        ...defaultSettings,
        clipboard_shortcut: asString(args?.shortcut, defaultSettings.clipboard_shortcut),
      }) as T
    }
    if (command === "update_search_settings") {
      return normalizeSettings({
        ...defaultSettings,
        search_enabled: asBoolean(args?.enabled, defaultSettings.search_enabled),
        search_shortcut: asString(args?.shortcut, defaultSettings.search_shortcut),
        search_paths: asArray(args?.paths)
          .map((item) => asString(item))
          .filter(Boolean),
      }) as T
    }
    if (command === "update_launch_on_startup") {
      return normalizeSettings({
        ...defaultSettings,
        launch_on_startup: asBoolean(args?.enabled),
      }) as T
    }
    if (command === "check_for_updates") {
      return normalizeUpdateInfo({
        current_version: "0.1.0",
        latest_version: "0.1.0",
        update_available: false,
        release_name: "DustDesk 0.1.0",
        release_url: "https://github.com/LAIJiangFeng/tauri-dustdesk/releases",
        download_url: "https://github.com/LAIJiangFeng/tauri-dustdesk/releases",
      }) as T
    }
    if (command === "open_update_download") {
      return undefined as T
    }
    if (command === "classify_desktop_items") {
      return {
        moved: demoSnapshot.desktop_items.length,
        skipped: 0,
        category_counts: [{ name: "演示分类", count: demoSnapshot.desktop_items.length }],
      } as T
    }
    if (command === "restore_all_to_desktop") {
      return demoSnapshot.categories.reduce((sum, category) => sum + category.item_paths.length, 0) as T
    }
    if (command === "restore_item_to_desktop") {
      return asPathString(args?.path) as T
    }
    if (command === "start_classify_desktop_items_task" || command === "start_restore_all_to_desktop_task") {
      return undefined as T
    }
    if (command === "save_desktop_window_layout") {
      return undefined as T
    }
    if (command === "save_desktop_split_indices") {
      return normalizeSplitIndices(args?.indices) as T
    }
    if (
      command === "desktop_frame_visibility" ||
      command === "toggle_desktop_frames" ||
      command === "toggle_desktop_organizer_frame" ||
      command === "toggle_desktop_launcher_frame"
    ) {
      return hiddenDesktopFrames as T
    }
    return undefined as T
  }

  const result = await invoke<T>(command, args)
  if (command === "load_snapshot" || command === "load_desktop_snapshot" || command === "update_runtime_directory") {
    return normalizeSnapshot(result) as T
  }
  if (
    command === "desktop_frame_visibility" ||
    command === "toggle_desktop_frames" ||
    command === "toggle_desktop_organizer_frame" ||
    command === "toggle_desktop_launcher_frame"
  ) {
    return normalizeDesktopFrameVisibility(result) as T
  }
  return result
}

type RawRecord = Record<string, unknown>

function asRecord(value: unknown): RawRecord {
  return value && typeof value === "object" ? (value as RawRecord) : {}
}

function asString(value: unknown, fallback = "") {
  return typeof value === "string" ? value : fallback
}

function asPathString(value: unknown, fallback = "") {
  return stripWindowsVerbatimPrefix(asString(value, fallback))
}

function stripWindowsVerbatimPrefix(value: string) {
  const trimmed = value.trim()
  if (trimmed.startsWith("\\\\?\\UNC\\")) {
    return `\\\\${trimmed.slice("\\\\?\\UNC\\".length)}`
  }
  if (trimmed.startsWith("\\\\?\\")) {
    return trimmed.slice("\\\\?\\".length)
  }
  return trimmed
}

function asBoolean(value: unknown, fallback = false) {
  return typeof value === "boolean" ? value : fallback
}

function asArray(value: unknown): unknown[] {
  return Array.isArray(value) ? value : []
}

function normalizeSplitIndices(value: unknown): number[] {
  return [
    ...new Set(
      asArray(value)
        .map(Number)
        .filter((index) => Number.isInteger(index) && index >= 0),
    ),
  ].sort((left, right) => left - right)
}

function normalizeDesktopWindowLayout(value: unknown): DesktopWindowLayout | null {
  const raw = asRecord(value)
  const x = Math.round(Number(raw.x ?? raw.X))
  const y = Math.round(Number(raw.y ?? raw.Y))
  const width = Math.round(Number(raw.width ?? raw.Width))
  const height = Math.round(Number(raw.height ?? raw.Height))
  if (![x, y, width, height].every(Number.isFinite) || width < 120 || height < 100) {
    return null
  }
  return { x, y, width, height }
}

function normalizeDesktopLayout(value: unknown): DesktopLayout {
  const raw = asRecord(value)
  const rawWindows = asRecord(raw.windows ?? raw.Windows)
  const windows = Object.fromEntries(
    Object.entries(rawWindows)
      .map(([label, layout]) => [label, normalizeDesktopWindowLayout(layout)] as const)
      .filter((entry): entry is [string, DesktopWindowLayout] => Boolean(entry[1])),
  )

  return {
    split_category_indices: normalizeSplitIndices(raw.split_category_indices ?? raw.SplitCategoryIndices),
    windows,
  }
}

function normalizeDesktopFrameVisibility(value: unknown): DesktopFrameVisibility {
  const raw = asRecord(value)
  const organizer = asBoolean(raw.organizer ?? raw.Organizer)
  const launcher = asBoolean(raw.launcher ?? raw.Launcher)
  return {
    organizer,
    launcher,
    any: asBoolean(raw.any ?? raw.Any, organizer || launcher),
  }
}

function normalizeSnapshot(value: unknown): AppSnapshot {
  const raw = asRecord(value)
  const snapshot = {
    data_dir: asPathString(raw.data_dir ?? raw.DataDir),
    organizer_root: asPathString(raw.organizer_root ?? raw.OrganizerRoot),
    launchers_root: asPathString(raw.launchers_root ?? raw.LaunchersRoot),
    settings: normalizeSettings(raw.settings ?? raw.Settings),
    desktop_layout: normalizeDesktopLayout(raw.desktop_layout ?? raw.DesktopLayout),
    categories: asArray(raw.categories ?? raw.DesktopCategories).map(normalizeCategory),
    desktop_items: asArray(raw.desktop_items ?? raw.DesktopItems).map((item) => {
      const rawItem = asRecord(item)
      return {
        name: asString(rawItem.name ?? rawItem.Name, "未命名"),
        path: asPathString(rawItem.path ?? rawItem.Path),
        extension: asString(rawItem.extension ?? rawItem.Extension, "FILE"),
        is_dir: asBoolean(rawItem.is_dir ?? rawItem.IsDir),
        icon_data_url: asString(rawItem.icon_data_url ?? rawItem.IconDataUrl) || undefined,
      }
    }),
    launchers: asArray(raw.launchers ?? raw.Launchers).map(normalizeLauncher),
    clipboard: asArray(raw.clipboard ?? raw.Clipboard).map(normalizeClipboardItem),
  }
  hydrateSnapshotIcons(snapshot)
  return snapshot
}

function normalizeSettings(value: unknown): AppSettings {
  const raw = asRecord(value)
  return {
    clipboard_shortcut: asString(raw.clipboard_shortcut ?? raw.ClipboardShortcut, defaultSettings.clipboard_shortcut),
    search_enabled: asBoolean(raw.search_enabled ?? raw.SearchEnabled, defaultSettings.search_enabled),
    search_shortcut: asString(raw.search_shortcut ?? raw.SearchShortcut, defaultSettings.search_shortcut),
    search_paths: asArray(raw.search_paths ?? raw.SearchPaths)
      .map((item) => asPathString(item))
      .filter(Boolean),
    launch_on_startup: asBoolean(raw.launch_on_startup ?? raw.LaunchOnStartup, defaultSettings.launch_on_startup),
  }
}

function normalizeCategory(value: unknown): DeskCategory {
  const raw = asRecord(value)
  const itemPaths = asArray(raw.item_paths ?? raw.ItemPaths)
    .map((item) => asPathString(item))
    .filter(Boolean)
  const itemDetails = asArray(raw.item_details ?? raw.ItemDetails)
    .map(normalizeDesktopItem)
    .filter((item) => item.path)
  return {
    name: asString(raw.name ?? raw.Name, "未命名分类"),
    is_collapsed: asBoolean(raw.is_collapsed ?? raw.IsCollapsed),
    item_paths: itemPaths,
    item_details: itemDetails.length > 0 ? itemDetails : itemPaths.map(pathToDesktopItem),
  }
}

function normalizeDesktopItem(value: unknown) {
  const raw = asRecord(value)
  const path = asPathString(raw.path ?? raw.Path)
  const rawName = asString(raw.name ?? raw.Name)
  return {
    name: displayWindowsEntryName(rawName, path),
    path,
    extension: asString(raw.extension ?? raw.Extension) || extensionFromPath(path),
    is_dir: asBoolean(raw.is_dir ?? raw.IsDir),
    icon_data_url: asString(raw.icon_data_url ?? raw.IconDataUrl) || undefined,
  }
}

function pathToDesktopItem(path: string) {
  return {
    name: displayWindowsEntryName(undefined, path),
    path,
    extension: extensionFromPath(path),
    is_dir: false,
  }
}

function normalizeLauncher(value: unknown): LaunchItem {
  const raw = asRecord(value)
  const path = asPathString(raw.path ?? raw.Path)
  return {
    name: displayWindowsEntryName(asString(raw.name ?? raw.Name), path),
    path,
    icon_data_url: asString(raw.icon_data_url ?? raw.IconDataUrl) || undefined,
  }
}

function normalizeClipboardItem(value: unknown): ClipboardHistoryItem {
  const raw = asRecord(value)
  return {
    id: asString(raw.id ?? raw.Id),
    kind: asString(raw.kind ?? raw.Kind, "Text") === "Image" ? "Image" : "Text",
    text: asString(raw.text ?? raw.Text),
    image_png_base64: asString(raw.image_png_base64 ?? raw.ImagePngBase64),
    image_path: asPathString(raw.image_path ?? raw.ImagePath),
    image_thumb_path: asPathString(raw.image_thumb_path ?? raw.ImageThumbPath),
    image_hash: asString(raw.image_hash ?? raw.ImageHash),
    created_at: asString(raw.created_at ?? raw.CreatedAt),
    is_locked: asBoolean(raw.is_locked ?? raw.IsLocked),
    is_pinned: asBoolean(raw.is_pinned ?? raw.IsPinned),
  }
}

function normalizeSearchOverlay(value: unknown): SearchOverlayData {
  const raw = asRecord(value)
  return {
    settings: normalizeSettings(raw.settings ?? raw.Settings),
    paths: asArray(raw.paths ?? raw.Paths)
      .map((item) => asPathString(item))
      .filter(Boolean),
    recent: asArray(raw.recent ?? raw.Recent).map(normalizeSearchItem),
    frequent: asArray(raw.frequent ?? raw.Frequent).map(normalizeSearchItem),
  }
}

function normalizeUpdateInfo(value: unknown): AppUpdateInfo {
  const raw = asRecord(value)
  return {
    current_version: asString(raw.current_version ?? raw.CurrentVersion),
    latest_version: asString(raw.latest_version ?? raw.LatestVersion),
    update_available: asBoolean(raw.update_available ?? raw.UpdateAvailable),
    release_name: asString(raw.release_name ?? raw.ReleaseName),
    release_url: asString(raw.release_url ?? raw.ReleaseUrl),
    download_url: asString(raw.download_url ?? raw.DownloadUrl),
    asset_name: asString(raw.asset_name ?? raw.AssetName),
    published_at: asString(raw.published_at ?? raw.PublishedAt),
    body: asString(raw.body ?? raw.Body),
  }
}

function normalizeSearchItem(value: unknown): SearchItem {
  const raw = asRecord(value)
  const path = asPathString(raw.path ?? raw.Path)
  return {
    id: asString(raw.id ?? raw.Id),
    name: displayWindowsEntryName(asString(raw.name ?? raw.Name, "未命名"), path),
    path,
    kind: normalizeSearchKind(raw.kind ?? raw.Kind),
    extension: asString(raw.extension ?? raw.Extension, "FILE"),
    is_dir: asBoolean(raw.is_dir ?? raw.IsDir),
    source: asString(raw.source ?? raw.Source),
    icon_data_url: asString(raw.icon_data_url ?? raw.IconDataUrl) || undefined,
    open_count: Number(raw.open_count ?? raw.OpenCount ?? 0) || 0,
    last_opened_at: asString(raw.last_opened_at ?? raw.LastOpenedAt),
  }
}

function normalizeSearchKind(value: unknown): SearchItemKind {
  const kind = asString(value)
  if (kind === "Directory" || kind === "Launcher") return kind
  return "File"
}

function normalizePathIcon(value: unknown): PathIconResult {
  const raw = asRecord(value)
  return {
    path: asPathString(raw.path ?? raw.Path),
    icon_data_url: asString(raw.icon_data_url ?? raw.IconDataUrl) || undefined,
  }
}

function iconCacheKey(path: string) {
  return path.trim().toLowerCase()
}

function hydrateIcon<T extends { path: string; icon_data_url?: string }>(item: T) {
  if (item.icon_data_url) return
  const cached = iconCache.get(iconCacheKey(item.path))
  if (cached) {
    item.icon_data_url = cached
  }
}

function hydrateSnapshotIcons(snapshot: AppSnapshot) {
  for (const category of snapshot.categories) {
    for (const item of category.item_details) hydrateIcon(item)
  }
  for (const launcher of snapshot.launchers) hydrateIcon(launcher)
  for (const item of snapshot.desktop_items) hydrateIcon(item)
}

function rememberResolvedIcons(icons: PathIconResult[]) {
  for (const icon of icons) {
    if (!icon.path || !icon.icon_data_url) continue
    const key = iconCacheKey(icon.path)
    iconCache.set(key, icon.icon_data_url)
    iconFailures.delete(key)
  }
}

function collectMissingIconPaths(snapshot: AppSnapshot, options: IconResolutionOptions, excludedKeys = new Set<string>()) {
  const includeDesktopItems = options.includeDesktopItems ?? true
  const includeLaunchers = options.includeLaunchers ?? true
  const categoryIndexSet = options.categoryIndices ? new Set(options.categoryIndices.filter((index) => Number.isInteger(index) && index >= 0)) : null
  const paths: string[] = []
  const seen = new Set<string>()

  const addPath = (path: string, iconDataUrl?: string) => {
    const trimmed = path.trim()
    if (!trimmed || iconDataUrl) return
    const key = trimmed.toLowerCase()
    const cached = iconCache.get(key)
    if (cached) return
    const failure = iconFailures.get(key)
    if (failure && failure.retryAfter > Date.now()) return
    if (seen.has(key) || excludedKeys.has(key)) return
    seen.add(key)
    paths.push(trimmed)
  }

  for (const [index, category] of snapshot.categories.entries()) {
    if (categoryIndexSet && !categoryIndexSet.has(index)) continue
    for (const item of category.item_details) {
      addPath(item.path, item.icon_data_url)
    }
  }
  if (includeLaunchers) {
    for (const launcher of snapshot.launchers) {
      addPath(launcher.path, launcher.icon_data_url)
    }
  }
  if (includeDesktopItems) {
    for (const item of snapshot.desktop_items) {
      addPath(item.path, item.icon_data_url)
    }
  }

  return paths.slice(0, iconPathLimit)
}

function shouldIncludeDesktopItems(options: IconResolutionOptions) {
  return options.includeDesktopItems ?? true
}

function shouldIncludeLaunchers(options: IconResolutionOptions) {
  return options.includeLaunchers ?? true
}

function mergeCategoryIndices(left?: number[], right?: number[]) {
  if (!left || !right) return undefined
  return Array.from(new Set([...left, ...right].filter((index) => Number.isInteger(index) && index >= 0)))
}

function mergeIconResolutionOptions(left: IconResolutionOptions | null, right: IconResolutionOptions) {
  if (!left) {
    return {
      includeDesktopItems: shouldIncludeDesktopItems(right),
      includeLaunchers: shouldIncludeLaunchers(right),
      categoryIndices: right.categoryIndices,
    }
  }
  return {
    includeDesktopItems: shouldIncludeDesktopItems(left) || shouldIncludeDesktopItems(right),
    includeLaunchers: shouldIncludeLaunchers(left) || shouldIncludeLaunchers(right),
    categoryIndices: mergeCategoryIndices(left.categoryIndices, right.categoryIndices),
  }
}

function markIconFailures(paths: string[]) {
  let shouldRetry = false
  for (const path of paths) {
    const key = iconCacheKey(path)
    if (!key || iconCache.has(key)) continue
    if (!iconFailures.has(key) && iconFailures.size >= iconFailureEntryLimit) {
      const oldestKey = iconFailures.keys().next().value
      if (oldestKey) iconFailures.delete(oldestKey)
    }
    const attempts = (iconFailures.get(key)?.attempts ?? 0) + 1
    iconFailures.set(key, {
      attempts,
      retryAfter: Date.now() + (attempts <= iconFailureRetryLimit ? iconFailureRetryDelayMs : iconFailureCooldownMs),
    })
    if (attempts <= iconFailureRetryLimit) shouldRetry = true
  }
  return shouldRetry
}

function unresolvedIconPaths(paths: string[], icons: PathIconResult[]) {
  const resolved = new Set(icons.filter((icon) => icon.path && icon.icon_data_url).map((icon) => iconCacheKey(icon.path)))
  return paths.filter((path) => !resolved.has(iconCacheKey(path)))
}

function scheduleIconResolutionRetry(options: IconResolutionOptions, retry: (options: IconResolutionOptions) => void) {
  queuedIconRetryOptions = mergeIconResolutionOptions(queuedIconRetryOptions, options)
  if (iconRetryTimer !== null) return

  iconRetryTimer = setTimeout(() => {
    iconRetryTimer = null
    const queued = queuedIconRetryOptions
    queuedIconRetryOptions = null
    if (queued) retry(queued)
  }, iconFailureRetryDelayMs)
}

function iconOptionsForDesktopSnapshotLoad(options: DesktopSnapshotLoadOptions) {
  if (options.iconOptions === false) return null
  return options.iconOptions ?? { includeDesktopItems: false }
}

function applyResolvedIcons(snapshot: AppSnapshot, icons: PathIconResult[]) {
  const iconByPath = new Map(icons.filter((item) => item.path && item.icon_data_url).map((item) => [item.path.toLowerCase(), item.icon_data_url as string]))
  if (iconByPath.size === 0) return

  for (const category of snapshot.categories) {
    for (const item of category.item_details) {
      const icon = iconByPath.get(item.path.toLowerCase())
      if (icon) item.icon_data_url = icon
    }
  }
  for (const launcher of snapshot.launchers) {
    const icon = iconByPath.get(launcher.path.toLowerCase())
    if (icon) launcher.icon_data_url = icon
  }
  for (const item of snapshot.desktop_items) {
    const icon = iconByPath.get(item.path.toLowerCase())
    if (icon) item.icon_data_url = icon
  }
}

function snapshotPathKey(path: string) {
  return path.trim().toLowerCase()
}

function removeDesktopItemsFromSnapshot(snapshot: AppSnapshot, paths: string[]) {
  const keys = new Set(paths.map(snapshotPathKey).filter(Boolean))
  if (keys.size === 0) return
  snapshot.desktop_items = snapshot.desktop_items.filter((item) => !keys.has(snapshotPathKey(item.path)))
}

function normalizeClassifyResult(value: unknown): ClassifyResult {
  const raw = asRecord(value)
  return {
    moved: Number(raw.moved ?? raw.Moved ?? 0) || 0,
    skipped: Number(raw.skipped ?? raw.Skipped ?? 0) || 0,
    category_counts: asArray(raw.category_counts ?? raw.CategoryCounts).map((item) => {
      const rawItem = asRecord(item)
      return {
        name: asString(rawItem.name ?? rawItem.Name, "未命名分类"),
        count: Number(rawItem.count ?? rawItem.Count ?? 0) || 0,
      }
    }),
  }
}

function demoSearchOverlay(): SearchOverlayData {
  const snapshot = normalizeSnapshot(demoSnapshot)
  const launcherItems = snapshot.launchers.map((item, index): SearchItem => ({
    id: `demo-launcher-${index}`,
    name: item.name || displayPathName(item.path),
    path: item.path,
    kind: "Launcher",
    extension: extensionFromPath(item.path),
    is_dir: false,
    source: "快捷启动",
    icon_data_url: item.icon_data_url,
    open_count: index === 0 ? 8 : 3,
    last_opened_at: "",
  }))
  const fileItems = snapshot.desktop_items
    .filter((item) => !item.is_dir)
    .map((item, index): SearchItem => ({
      id: `demo-file-${index}`,
      name: item.name,
      path: item.path,
      kind: "File",
      extension: item.extension,
      is_dir: false,
      source: "最近文件",
      icon_data_url: item.icon_data_url,
      open_count: index + 1,
      last_opened_at: "",
    }))
  const directoryItems = snapshot.desktop_items
    .filter((item) => item.is_dir)
    .map((item, index): SearchItem => ({
      id: `demo-directory-${index}`,
      name: item.name,
      path: item.path,
      kind: "Directory",
      extension: "",
      is_dir: true,
      source: "最近目录",
      icon_data_url: item.icon_data_url,
      open_count: index + 2,
      last_opened_at: "",
    }))

  return {
    settings: snapshot.settings,
    paths: [snapshot.organizer_root],
    recent: [...launcherItems, ...directoryItems, ...fileItems].slice(0, 30),
    frequent: [...launcherItems, ...directoryItems, ...fileItems].sort((left, right) => right.open_count - left.open_count).slice(0, 30),
  }
}

function demoSearchItems(query: string): SearchItem[] {
  const lower = query.trim().toLowerCase()
  if (!lower) return []
  const overlay = demoSearchOverlay()
  return [...overlay.recent, ...overlay.frequent]
    .filter((item, index, items) => {
      const firstIndex = items.findIndex((candidate) => candidate.path === item.path && candidate.kind === item.kind)
      return firstIndex === index && `${item.name} ${item.path}`.toLowerCase().includes(lower)
    })
    .sort((left, right) => {
      return searchTypeRank(left) - searchTypeRank(right) || searchMatchRank(left, lower) - searchMatchRank(right, lower) || left.name.localeCompare(right.name)
    })
}

function searchTypeRank(item: SearchItem) {
  if (item.kind === "Launcher") return 0
  if (isShortcutOrApp(item)) return 1
  if (item.kind === "Directory" || item.is_dir) return 2
  return 3
}

function searchMatchRank(item: SearchItem, lower: string) {
  const name = item.name.toLowerCase()
  if (name === lower) return 0
  if (name.startsWith(lower)) return 1
  if (item.kind === "Launcher") return 2
  if (item.kind === "Directory") return 3
  return 4
}

function isShortcutOrApp(item: SearchItem) {
  return ["lnk", "exe", "appref-ms", "url", "bat", "cmd", "ps1", "msi"].includes(item.extension.trim().toLowerCase())
}

interface DustDeskState {
  page: AppPage
  selectedCategory: number
  snapshot: AppSnapshot
  desktopFrames: DesktopFrameVisibility
  loading: boolean
  error: string | null
  setPage: (page: AppPage) => void
  selectCategory: (index: number) => void
  load: (options?: SnapshotLoadOptions) => Promise<void>
  loadDesktopSnapshot: (options?: DesktopSnapshotLoadOptions) => Promise<void>
  resolveSnapshotIcons: (options?: IconResolutionOptions) => Promise<void>
  refresh: () => Promise<void>
  createCategory: (name?: string) => Promise<void>
  renameCategory: () => Promise<void>
  deleteCategory: () => Promise<void>
  toggleCategory: () => Promise<void>
  addItemToCategory: (index: number, path: string) => Promise<void>
  addItemsToCategory: (index: number, paths: string[]) => Promise<number>
  addItemsToCategoryLight: (index: number, paths: string[]) => Promise<number>
  removeItemFromCategory: (index: number, path: string) => Promise<void>
  restoreItemToDesktop: (index: number, path: string, position?: DesktopDropPosition) => Promise<string>
  restoreItemToDesktopLight: (index: number, path: string, position?: DesktopDropPosition) => Promise<string>
  restoreAllToDesktop: () => Promise<number>
  restoreAllToDesktopLight: () => Promise<number>
  startRestoreAllToDesktopTask: () => Promise<void>
  addLauncher: (path: string, name: string) => Promise<void>
  addLaunchers: (paths: string[]) => Promise<number>
  addLauncherLight: (path: string, name: string) => Promise<void>
  addLaunchersLight: (paths: string[]) => Promise<number>
  removeLauncher: (path: string) => Promise<void>
  classifyDesktopItems: () => Promise<ClassifyResult>
  classifyDesktopItemsLight: () => Promise<ClassifyResult>
  startClassifyDesktopItemsTask: () => Promise<void>
  getDesktopOperationStatus: () => Promise<DesktopOperationStatus>
  createDesktopEntries: () => Promise<string[]>
  showDesktopWidget: () => Promise<void>
  refreshDesktopFrameVisibility: () => Promise<void>
  toggleDesktopFrames: () => Promise<void>
  toggleDesktopOrganizerFrame: () => Promise<void>
  toggleDesktopLauncherFrame: () => Promise<void>
  splitDesktopWidgets: () => Promise<number[]>
  splitDesktopCategory: (index: number) => Promise<void>
  mergeDesktopCategory: (index: number) => Promise<void>
  mergeDesktopWidgets: () => Promise<void>
  saveDesktopWindowLayout: (label: string) => Promise<void>
  saveDesktopSplitIndices: (indices: number[]) => Promise<number[]>
  hideCurrentWindow: () => Promise<void>
  openSpecial: (target: "organizer" | "launchers" | "data" | "desktop") => Promise<void>
  updateRuntimeDirectory: (target: "organizer" | "launchers" | "data", path: string) => Promise<AppSnapshot>
  openPath: (path: string) => Promise<void>
  showPathInFolder: (path: string) => Promise<void>
  startAllLaunchers: () => Promise<number>
  pasteClipboardItem: (id: string) => Promise<void>
  clipboardImageBase64: (id: string) => Promise<string>
  hideClipboardOverlay: () => Promise<void>
  updateClipboardShortcut: (shortcut: string) => Promise<AppSettings>
  loadSearchOverlay: () => Promise<SearchOverlayData>
  searchItems: (query: string) => Promise<SearchItem[]>
  openSearchItem: (item: SearchItem) => Promise<void>
  hideSearchOverlay: () => Promise<void>
  updateSearchSettings: (enabled: boolean, shortcut: string, paths: string[]) => Promise<AppSettings>
  updateLaunchOnStartup: (enabled: boolean) => Promise<AppSettings>
  checkForUpdates: () => Promise<AppUpdateInfo>
  openUpdateDownload: (downloadUrl: string) => Promise<void>
}

export const useDustDeskStore = create<DustDeskState>()(
  immer((set, get) => ({
    page: "organizer",
    selectedCategory: 0,
    snapshot: emptySnapshot,
    desktopFrames: hiddenDesktopFrames,
    loading: false,
    error: null,
    setPage: (page) =>
      set((state) => {
        state.page = page
      }),
    selectCategory: (index) =>
      set((state) => {
        state.selectedCategory = index
      }),
    load: async (options = {}) => {
      if (snapshotLoadPromise) {
        if (options.force) snapshotForceReloadQueued = true
        return snapshotLoadPromise
      }
      if (!options.force && Date.now() - snapshotLoadedAt < snapshotReloadDedupeMs) return

      snapshotLoadPromise = (async () => {
        do {
          snapshotForceReloadQueued = false
          set((state) => {
            state.loading = true
            state.error = null
          })
          try {
            const snapshot = await call<AppSnapshot>("load_snapshot")
            set((state) => {
              state.snapshot = snapshot
              state.selectedCategory = Math.min(state.selectedCategory, Math.max(0, snapshot.categories.length - 1))
              state.loading = false
            })
            snapshotLoadedAt = Date.now()
            void get().resolveSnapshotIcons({ includeDesktopItems: true })
          } catch (error) {
            set((state) => {
              state.error = error instanceof Error ? error.message : String(error)
              state.loading = false
            })
          }
        } while (snapshotForceReloadQueued)
      })()

      try {
        await snapshotLoadPromise
      } finally {
        snapshotLoadPromise = null
      }
    },
    loadDesktopSnapshot: async (options = {}) => {
      if (desktopSnapshotLoadPromise) {
        if (options.force) desktopSnapshotForceReloadQueued = true
        const iconOptions = iconOptionsForDesktopSnapshotLoad(options)
        if (iconOptions) {
          void desktopSnapshotLoadPromise.then(() => get().resolveSnapshotIcons(iconOptions))
        }
        return desktopSnapshotLoadPromise
      }
      if (!options.force && Date.now() - desktopSnapshotLoadedAt < desktopSnapshotReloadDedupeMs) {
        return
      }

      desktopSnapshotLoadPromise = (async () => {
        do {
          desktopSnapshotForceReloadQueued = false
          set((state) => {
            state.loading = true
            state.error = null
          })
          try {
            let snapshot = await call<AppSnapshot>("load_desktop_snapshot")
            if (options.preserveDesktopItems && snapshot.desktop_items.length === 0) {
              snapshot = {
                ...snapshot,
                desktop_items: get().snapshot.desktop_items,
              }
            }
            set((state) => {
              state.snapshot = snapshot
              state.selectedCategory = Math.min(state.selectedCategory, Math.max(0, snapshot.categories.length - 1))
              state.loading = false
            })
            desktopSnapshotLoadedAt = Date.now()
            const iconOptions = iconOptionsForDesktopSnapshotLoad(options)
            if (iconOptions) {
              void get().resolveSnapshotIcons(iconOptions)
            }
          } catch (error) {
            set((state) => {
              state.error = error instanceof Error ? error.message : String(error)
              state.loading = false
            })
          }
        } while (desktopSnapshotForceReloadQueued)
      })()

      try {
        await desktopSnapshotLoadPromise
      } finally {
        desktopSnapshotLoadPromise = null
      }
    },
    resolveSnapshotIcons: async (options = {}) => {
      queuedIconResolutionOptions = mergeIconResolutionOptions(queuedIconResolutionOptions, options)
      iconResolutionToken += 1
      if (iconResolutionActive) return

      iconResolutionActive = true
      let retryOptions: IconResolutionOptions | null = null
      try {
        while (queuedIconResolutionOptions) {
          const currentOptions = queuedIconResolutionOptions
          queuedIconResolutionOptions = null
          const run = iconResolutionToken
          const attemptedPathKeys = new Set<string>()
          const requeueCanceledScope = () => {
            if (run === iconResolutionToken) return false
            queuedIconResolutionOptions = mergeIconResolutionOptions(queuedIconResolutionOptions, currentOptions)
            return true
          }

          while (true) {
            if (requeueCanceledScope()) break
            const paths = collectMissingIconPaths(get().snapshot, currentOptions, attemptedPathKeys)
            if (paths.length === 0) break
            for (const path of paths) attemptedPathKeys.add(iconCacheKey(path))

            let canceled = false
            for (let index = 0; index < paths.length; index += iconBatchSize) {
              if (requeueCanceledScope()) {
                canceled = true
                break
              }
              const batch = paths.slice(index, index + iconBatchSize)
              let icons: PathIconResult[]
              try {
                icons = asArray(
                  await call<PathIconResult[]>("resolve_path_icons", {
                    paths: batch,
                  }),
                ).map(normalizePathIcon)
              } catch (error) {
                if (requeueCanceledScope()) {
                  canceled = true
                  break
                }
                console.warn("Failed to resolve system icons", error)
                if (markIconFailures(batch)) {
                  retryOptions = mergeIconResolutionOptions(retryOptions, currentOptions)
                }
                continue
              }
              if (requeueCanceledScope()) {
                canceled = true
                break
              }
              rememberResolvedIcons(icons)
              set((state) => {
                applyResolvedIcons(state.snapshot, icons)
              })
              if (markIconFailures(unresolvedIconPaths(batch, icons))) {
                retryOptions = mergeIconResolutionOptions(retryOptions, currentOptions)
              }
            }
            if (canceled) break
          }
        }
      } finally {
        iconResolutionActive = false
        if (retryOptions) {
          scheduleIconResolutionRetry(retryOptions, (retry) => {
            void get().resolveSnapshotIcons(retry)
          })
        }
        if (queuedIconResolutionOptions) {
          const queued = queuedIconResolutionOptions
          queuedIconResolutionOptions = null
          void get().resolveSnapshotIcons(queued)
        }
      }
    },
    refresh: async () => get().load({ force: true }),
    createCategory: async (name) => {
      name = name?.trim() || `新分类 ${get().snapshot.categories.length + 1}`
      await call("create_category", { name })
      await get().load()
      set((state) => {
        state.selectedCategory = Math.max(0, state.snapshot.categories.length - 1)
      })
    },
    renameCategory: async () => {
      const category = get().snapshot.categories[get().selectedCategory]
      if (!category) return
      const name = window.prompt("分类名称", category.name)?.trim()
      if (!name) return
      await call("rename_category", { index: get().selectedCategory, name })
      await get().load()
    },
    deleteCategory: async () => {
      const category = get().snapshot.categories[get().selectedCategory]
      if (!category) return
      if (!window.confirm(`确认删除空分类「${category.name}」吗？`)) return
      await call("delete_category", { index: get().selectedCategory })
      await get().load()
    },
    toggleCategory: async () => {
      await call("toggle_category", { index: get().selectedCategory })
      await get().load()
    },
    addItemToCategory: async (index, path) => {
      await call("add_item_to_category", { index, path })
      await get().load()
    },
    addItemsToCategory: async (index, paths) => {
      if (paths.length === 0) return 0
      const added = Number(await call<number>("add_items_to_category", { index, paths })) || 0
      await get().load()
      return added
    },
    addItemsToCategoryLight: async (index, paths) => {
      if (paths.length === 0) return 0
      const added = Number(await call<number>("add_items_to_category", { index, paths })) || 0
      set((state) => {
        removeDesktopItemsFromSnapshot(state.snapshot, paths)
      })
      await get().loadDesktopSnapshot({ force: true, preserveDesktopItems: true })
      return added
    },
    removeItemFromCategory: async (index, path) => {
      await call("remove_item_from_category", { index, path })
      await get().load()
    },
    restoreItemToDesktop: async (index, path, position) => {
      const restored = await call<string>("restore_item_to_desktop", restoreDesktopArgs(index, path, position))
      await get().load()
      return restored
    },
    restoreItemToDesktopLight: async (index, path, position) => {
      const restored = await call<string>("restore_item_to_desktop", restoreDesktopArgs(index, path, position))
      const key = path.trim().toLowerCase()
      set((state) => {
        const category = state.snapshot.categories[index]
        if (!category) return
        category.item_paths = category.item_paths.filter((itemPath) => itemPath.trim().toLowerCase() !== key)
        category.item_details = category.item_details.filter((item) => item.path.trim().toLowerCase() !== key)
      })
      void get().loadDesktopSnapshot({ force: true })
      return restored
    },
    restoreAllToDesktop: async () => {
      const restored = Number(await call<number>("restore_all_to_desktop")) || 0
      await get().load()
      return restored
    },
    restoreAllToDesktopLight: async () => {
      set((state) => {
        state.loading = true
        state.error = null
      })
      await waitForNextPaint()
      try {
        const restored = Number(await call<number>("restore_all_to_desktop")) || 0
        await get().loadDesktopSnapshot({ force: true })
        return restored
      } catch (error) {
        set((state) => {
          state.error = error instanceof Error ? error.message : String(error)
          state.loading = false
        })
        throw error
      }
    },
    startRestoreAllToDesktopTask: async () => {
      await call("start_restore_all_to_desktop_task")
    },
    addLauncher: async (path, name) => {
      await call("add_launcher", { path, name })
      await get().load()
    },
    addLaunchers: async (paths) => {
      if (paths.length === 0) return 0
      const added = Number(await call<number>("add_launchers", { paths })) || 0
      await get().load()
      return added
    },
    addLauncherLight: async (path, name) => {
      await call("add_launcher", { path, name })
      await get().loadDesktopSnapshot({ force: true, preserveDesktopItems: true })
    },
    addLaunchersLight: async (paths) => {
      if (paths.length === 0) return 0
      const added = Number(await call<number>("add_launchers", { paths })) || 0
      await get().loadDesktopSnapshot({ force: true, preserveDesktopItems: true })
      return added
    },
    removeLauncher: async (path) => {
      await call("remove_launcher", { path })
      await get().load()
    },
    classifyDesktopItems: async () => {
      const result = await call<ClassifyResult>("classify_desktop_items")
      await get().load()
      return normalizeClassifyResult(result)
    },
    classifyDesktopItemsLight: async () => {
      set((state) => {
        state.loading = true
        state.error = null
      })
      await waitForNextPaint()
      try {
        const result = await call<ClassifyResult>("classify_desktop_items")
        await get().loadDesktopSnapshot({ force: true })
        return normalizeClassifyResult(result)
      } catch (error) {
        set((state) => {
          state.error = error instanceof Error ? error.message : String(error)
          state.loading = false
        })
        throw error
      }
    },
    startClassifyDesktopItemsTask: async () => {
      await call("start_classify_desktop_items_task")
    },
    getDesktopOperationStatus: async () => {
      return call<DesktopOperationStatus>("desktop_operation_status")
    },
    createDesktopEntries: async () => {
      return call<string[]>("create_desktop_entries")
    },
    showDesktopWidget: async () => {
      await call("show_desktop_widget")
      await get().refreshDesktopFrameVisibility()
    },
    refreshDesktopFrameVisibility: async () => {
      const visibility = await call<DesktopFrameVisibility>("desktop_frame_visibility")
      set((state) => {
        state.desktopFrames = visibility
      })
    },
    toggleDesktopFrames: async () => {
      const visibility = await call<DesktopFrameVisibility>("toggle_desktop_frames")
      set((state) => {
        state.desktopFrames = visibility
      })
    },
    toggleDesktopOrganizerFrame: async () => {
      const visibility = await call<DesktopFrameVisibility>("toggle_desktop_organizer_frame")
      set((state) => {
        state.desktopFrames = visibility
      })
    },
    toggleDesktopLauncherFrame: async () => {
      const visibility = await call<DesktopFrameVisibility>("toggle_desktop_launcher_frame")
      set((state) => {
        state.desktopFrames = visibility
      })
    },
    splitDesktopWidgets: async () => {
      const indices = await call<unknown>("split_desktop_widgets")
      await get().refreshDesktopFrameVisibility()
      return normalizeSplitIndices(indices)
    },
    splitDesktopCategory: async (index) => {
      await call("split_desktop_category", { index })
      await get().refreshDesktopFrameVisibility()
    },
    mergeDesktopCategory: async (index) => {
      await call("merge_desktop_category", { index })
      await get().refreshDesktopFrameVisibility()
    },
    mergeDesktopWidgets: async () => {
      await call("merge_desktop_widgets")
      await get().refreshDesktopFrameVisibility()
    },
    saveDesktopWindowLayout: async (label) => {
      await call("save_desktop_window_layout", { label })
    },
    saveDesktopSplitIndices: async (indices) => {
      const saved = await call<unknown>("save_desktop_split_indices", {
        indices,
      })
      return normalizeSplitIndices(saved)
    },
    hideCurrentWindow: async () => {
      await call("hide_current_window")
    },
    openSpecial: async (target) => {
      await call("open_special", { target })
    },
    updateRuntimeDirectory: async (target, path) => {
      const snapshot = await call<AppSnapshot>("update_runtime_directory", {
        target,
        path,
      })
      resetRuntimeCaches()
      set((state) => {
        state.snapshot = snapshot
        state.selectedCategory = Math.min(state.selectedCategory, Math.max(0, snapshot.categories.length - 1))
      })
      void get().resolveSnapshotIcons({ includeDesktopItems: true })
      return snapshot
    },
    openPath: async (path) => {
      await call("open_path", { path })
    },
    showPathInFolder: async (path) => {
      await call("show_path_in_folder", { path })
    },
    startAllLaunchers: async () => {
      const count = get().snapshot.launchers.length
      if (count === 0) return 0
      return call<number>("start_all_launchers")
    },
    pasteClipboardItem: async (id) => {
      await call("paste_clipboard_item", { id })
      await get().load()
    },
    clipboardImageBase64: async (id) => {
      return call<string>("clipboard_image_base64", { id })
    },
    hideClipboardOverlay: async () => {
      await call("hide_clipboard_overlay")
    },
    updateClipboardShortcut: async (shortcut) => {
      const settings = await call<AppSettings>("update_clipboard_shortcut", {
        shortcut,
      })
      set((state) => {
        state.snapshot.settings = normalizeSettings(settings)
      })
      return normalizeSettings(settings)
    },
    loadSearchOverlay: async () => {
      const overlay = await call<SearchOverlayData>("load_search_overlay")
      return normalizeSearchOverlay(overlay)
    },
    searchItems: async (query) => {
      const items = await call<SearchItem[]>("search_items", { query })
      return asArray(items).map(normalizeSearchItem)
    },
    openSearchItem: async (item) => {
      await call("open_search_item", { item })
      await get().load()
    },
    hideSearchOverlay: async () => {
      await call("hide_search_overlay")
    },
    updateSearchSettings: async (enabled, shortcut, paths) => {
      const settings = await call<AppSettings>("update_search_settings", {
        enabled,
        shortcut,
        paths,
      })
      set((state) => {
        state.snapshot.settings = normalizeSettings(settings)
      })
      return normalizeSettings(settings)
    },
    updateLaunchOnStartup: async (enabled) => {
      const settings = await call<AppSettings>("update_launch_on_startup", {
        enabled,
      })
      set((state) => {
        state.snapshot.settings = normalizeSettings(settings)
      })
      return normalizeSettings(settings)
    },
    checkForUpdates: async () => {
      const update = await call<AppUpdateInfo>("check_for_updates")
      return normalizeUpdateInfo(update)
    },
    openUpdateDownload: async (downloadUrl) => {
      await call("open_update_download", {
        downloadUrl,
      })
    },
  })),
)
