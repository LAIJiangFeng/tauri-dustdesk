import { invoke } from "@tauri-apps/api/core"
import { create } from "zustand"
import { immer } from "zustand/middleware/immer"
import { displayPathName, extensionFromPath } from "@/lib/utils"
import type {
  AppPage,
  AppSettings,
  AppSnapshot,
  ClassifyResult,
  ClipboardHistoryItem,
  DeskCategory,
  DesktopFrameVisibility,
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
}

const emptySnapshot: AppSnapshot = {
  data_dir: "",
  organizer_root: "",
  launchers_root: "",
  settings: defaultSettings,
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
}

interface DesktopSnapshotLoadOptions {
  force?: boolean
}

const iconBatchSize = 24
const iconPathLimit = 512
const desktopSnapshotReloadDedupeMs = 120
let iconResolutionToken = 0
let iconResolutionActive = false
let queuedIconResolutionOptions: IconResolutionOptions | null = null
const iconCache = new Map<string, string | null>()
let desktopSnapshotLoadPromise: Promise<void> | null = null
let desktopSnapshotLoadedAt = 0

const demoSnapshot: AppSnapshot = {
  data_dir: "%APPDATA%\\DustDesk\\Data",
  organizer_root: "%APPDATA%\\DustDesk\\Data\\DesktopOrganizer",
  launchers_root: "%APPDATA%\\DustDesk\\Data\\Launchers",
  settings: defaultSettings,
  categories: [
    { name: "开发", is_collapsed: false, item_paths: [], item_details: [] },
    { name: "工具", is_collapsed: false, item_paths: [], item_details: [] },
    { name: "文档", is_collapsed: false, item_paths: [], item_details: [] },
    { name: "社交", is_collapsed: false, item_paths: [], item_details: [] },
    { name: "游戏", is_collapsed: false, item_paths: [], item_details: [] },
  ],
  desktop_items: [
    { name: "Apifox", path: "C:\\Users\\Desktop\\Apifox.lnk", extension: "LNK", is_dir: false },
    { name: "Cursor", path: "C:\\Users\\Desktop\\Cursor.lnk", extension: "LNK", is_dir: false },
    { name: "Project Assets", path: "C:\\Users\\Desktop\\Project Assets", extension: "DIR", is_dir: true },
    { name: "design-reference", path: "C:\\Users\\Desktop\\design-reference.png", extension: "PNG", is_dir: false },
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
    if (command === "load_snapshot" || command === "load_desktop_snapshot") {
      return normalizeSnapshot(demoSnapshot) as T
    }
    if (command === "resolve_path_icons") {
      return asArray(args?.paths).map((path) => ({ path: asString(path) })) as T
    }
    if (command === "load_search_overlay") {
      return demoSearchOverlay() as T
    }
    if (command === "search_items") {
      return demoSearchItems(asString(args?.query)) as T
    }
    if (command === "update_clipboard_shortcut") {
      return normalizeSettings({ ...defaultSettings, clipboard_shortcut: asString(args?.shortcut, defaultSettings.clipboard_shortcut) }) as T
    }
    if (command === "update_search_settings") {
      return normalizeSettings({
        ...defaultSettings,
        search_enabled: asBoolean(args?.enabled, defaultSettings.search_enabled),
        search_shortcut: asString(args?.shortcut, defaultSettings.search_shortcut),
        search_paths: asArray(args?.paths).map((item) => asString(item)).filter(Boolean),
      }) as T
    }
    if (command === "classify_desktop_items") {
      return {
        moved: demoSnapshot.desktop_items.length,
        skipped: 0,
        category_counts: [{ name: "演示分类", count: demoSnapshot.desktop_items.length }],
      } as T
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
  if (command === "load_snapshot" || command === "load_desktop_snapshot") {
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

function asBoolean(value: unknown, fallback = false) {
  return typeof value === "boolean" ? value : fallback
}

function asArray(value: unknown): unknown[] {
  return Array.isArray(value) ? value : []
}

function normalizeSplitIndices(value: unknown): number[] {
  return [...new Set(asArray(value).map(Number).filter((index) => Number.isInteger(index) && index >= 0))].sort((left, right) => left - right)
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
    data_dir: asString(raw.data_dir ?? raw.DataDir),
    organizer_root: asString(raw.organizer_root ?? raw.OrganizerRoot),
    launchers_root: asString(raw.launchers_root ?? raw.LaunchersRoot),
    settings: normalizeSettings(raw.settings ?? raw.Settings),
    categories: asArray(raw.categories ?? raw.DesktopCategories).map(normalizeCategory),
    desktop_items: asArray(raw.desktop_items ?? raw.DesktopItems).map((item) => {
      const rawItem = asRecord(item)
      return {
        name: asString(rawItem.name ?? rawItem.Name, "未命名"),
        path: asString(rawItem.path ?? rawItem.Path),
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
    search_paths: asArray(raw.search_paths ?? raw.SearchPaths).map((item) => asString(item)).filter(Boolean),
  }
}

function normalizeCategory(value: unknown): DeskCategory {
  const raw = asRecord(value)
  const itemPaths = asArray(raw.item_paths ?? raw.ItemPaths).map((item) => asString(item)).filter(Boolean)
  const itemDetails = asArray(raw.item_details ?? raw.ItemDetails).map(normalizeDesktopItem).filter((item) => item.path)
  return {
    name: asString(raw.name ?? raw.Name, "未命名分类"),
    is_collapsed: asBoolean(raw.is_collapsed ?? raw.IsCollapsed),
    item_paths: itemPaths,
    item_details: itemDetails.length > 0 ? itemDetails : itemPaths.map(pathToDesktopItem),
  }
}

function normalizeDesktopItem(value: unknown) {
  const raw = asRecord(value)
  const path = asString(raw.path ?? raw.Path)
  return {
    name: asString(raw.name ?? raw.Name) || displayPathName(path),
    path,
    extension: asString(raw.extension ?? raw.Extension) || extensionFromPath(path),
    is_dir: asBoolean(raw.is_dir ?? raw.IsDir),
    icon_data_url: asString(raw.icon_data_url ?? raw.IconDataUrl) || undefined,
  }
}

function pathToDesktopItem(path: string) {
  return {
    name: displayPathName(path),
    path,
    extension: extensionFromPath(path),
    is_dir: false,
  }
}

function normalizeLauncher(value: unknown): LaunchItem {
  const raw = asRecord(value)
  return {
    name: asString(raw.name ?? raw.Name),
    path: asString(raw.path ?? raw.Path),
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
    image_path: asString(raw.image_path ?? raw.ImagePath),
    image_thumb_path: asString(raw.image_thumb_path ?? raw.ImageThumbPath),
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
    paths: asArray(raw.paths ?? raw.Paths).map((item) => asString(item)).filter(Boolean),
    recent: asArray(raw.recent ?? raw.Recent).map(normalizeSearchItem),
    frequent: asArray(raw.frequent ?? raw.Frequent).map(normalizeSearchItem),
  }
}

function normalizeSearchItem(value: unknown): SearchItem {
  const raw = asRecord(value)
  return {
    id: asString(raw.id ?? raw.Id),
    name: asString(raw.name ?? raw.Name, "未命名"),
    path: asString(raw.path ?? raw.Path),
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
    path: asString(raw.path ?? raw.Path),
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
    if (!icon.path) continue
    iconCache.set(iconCacheKey(icon.path), icon.icon_data_url ?? null)
  }
}

function collectMissingIconPaths(snapshot: AppSnapshot, options: IconResolutionOptions) {
  const includeDesktopItems = options.includeDesktopItems ?? true
  const paths: string[] = []
  const seen = new Set<string>()

  const addPath = (path: string, iconDataUrl?: string) => {
    const trimmed = path.trim()
    if (!trimmed || iconDataUrl) return
    const key = trimmed.toLowerCase()
    if (iconCache.has(key)) return
    if (seen.has(key)) return
    seen.add(key)
    paths.push(trimmed)
  }

  for (const category of snapshot.categories) {
    for (const item of category.item_details) {
      addPath(item.path, item.icon_data_url)
    }
  }
  for (const launcher of snapshot.launchers) {
    addPath(launcher.path, launcher.icon_data_url)
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

function mergeIconResolutionOptions(left: IconResolutionOptions | null, right: IconResolutionOptions) {
  if (!left) {
    return { includeDesktopItems: shouldIncludeDesktopItems(right) }
  }
  return {
    includeDesktopItems: shouldIncludeDesktopItems(left) || shouldIncludeDesktopItems(right),
  }
}

function applyResolvedIcons(snapshot: AppSnapshot, icons: PathIconResult[]) {
  const iconByPath = new Map(
    icons
      .filter((item) => item.path && item.icon_data_url)
      .map((item) => [item.path.toLowerCase(), item.icon_data_url as string]),
  )
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

  return {
    settings: snapshot.settings,
    paths: [snapshot.organizer_root],
    recent: [...launcherItems, ...fileItems].slice(0, 30),
    frequent: [...launcherItems, ...fileItems].sort((left, right) => right.open_count - left.open_count).slice(0, 30),
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
  load: () => Promise<void>
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
  restoreItemToDesktop: (index: number, path: string) => Promise<string>
  addLauncher: (path: string, name: string) => Promise<void>
  addLaunchers: (paths: string[]) => Promise<number>
  addLauncherLight: (path: string, name: string) => Promise<void>
  addLaunchersLight: (paths: string[]) => Promise<number>
  removeLauncher: (path: string) => Promise<void>
  classifyDesktopItems: () => Promise<ClassifyResult>
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
    load: async () => {
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
        void get().resolveSnapshotIcons({ includeDesktopItems: true })
      } catch (error) {
        set((state) => {
          state.error = error instanceof Error ? error.message : String(error)
          state.loading = false
        })
      }
    },
    loadDesktopSnapshot: async (options = {}) => {
      if (desktopSnapshotLoadPromise) {
        return desktopSnapshotLoadPromise
      }
      if (!options.force && Date.now() - desktopSnapshotLoadedAt < desktopSnapshotReloadDedupeMs) {
        return
      }

      desktopSnapshotLoadPromise = (async () => {
        set((state) => {
          state.loading = true
          state.error = null
        })
        try {
          const snapshot = await call<AppSnapshot>("load_desktop_snapshot")
          set((state) => {
            state.snapshot = snapshot
            state.selectedCategory = Math.min(state.selectedCategory, Math.max(0, snapshot.categories.length - 1))
            state.loading = false
          })
          desktopSnapshotLoadedAt = Date.now()
          void get().resolveSnapshotIcons({ includeDesktopItems: false })
        } catch (error) {
          set((state) => {
            state.error = error instanceof Error ? error.message : String(error)
            state.loading = false
          })
        }
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
      try {
        while (queuedIconResolutionOptions) {
          const currentOptions = queuedIconResolutionOptions
          queuedIconResolutionOptions = null
          const run = iconResolutionToken
          const paths = collectMissingIconPaths(get().snapshot, currentOptions)

          for (let index = 0; index < paths.length; index += iconBatchSize) {
            if (run !== iconResolutionToken) break
            const batch = paths.slice(index, index + iconBatchSize)
            const icons = asArray(await call<PathIconResult[]>("resolve_path_icons", { paths: batch })).map(normalizePathIcon)
            if (run !== iconResolutionToken) break
            rememberResolvedIcons(icons)
            set((state) => {
              applyResolvedIcons(state.snapshot, icons)
            })
          }
        }
      } finally {
        iconResolutionActive = false
        if (queuedIconResolutionOptions) {
          const queued = queuedIconResolutionOptions
          queuedIconResolutionOptions = null
          void get().resolveSnapshotIcons(queued)
        }
      }
    },
    refresh: async () => get().load(),
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
      await get().loadDesktopSnapshot({ force: true })
      return added
    },
    removeItemFromCategory: async (index, path) => {
      await call("remove_item_from_category", { index, path })
      await get().load()
    },
    restoreItemToDesktop: async (index, path) => {
      const restored = await call<string>("restore_item_to_desktop", { index, path })
      await get().load()
      return restored
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
      await get().loadDesktopSnapshot({ force: true })
    },
    addLaunchersLight: async (paths) => {
      if (paths.length === 0) return 0
      const added = Number(await call<number>("add_launchers", { paths })) || 0
      await get().loadDesktopSnapshot({ force: true })
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
    hideCurrentWindow: async () => {
      await call("hide_current_window")
    },
    openSpecial: async (target) => {
      await call("open_special", { target })
    },
    updateRuntimeDirectory: async (target, path) => {
      const snapshot = await call<AppSnapshot>("update_runtime_directory", { target, path })
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
      const settings = await call<AppSettings>("update_clipboard_shortcut", { shortcut })
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
      const settings = await call<AppSettings>("update_search_settings", { enabled, shortcut, paths })
      set((state) => {
        state.snapshot.settings = normalizeSettings(settings)
      })
      return normalizeSettings(settings)
    },
  })),
)
