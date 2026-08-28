import { useCallback, useEffect, useMemo, useRef, useState, type CSSProperties, type DragEvent as ReactDragEvent, type ReactNode } from "react"
import { useParams } from "react-router"
import {
  Archive,
  ArrowsClockwise,
  Briefcase,
  Code,
  Columns,
  CursorClick,
  Desktop,
  Eye,
  EyeSlash,
  FileText,
  FolderOpen,
  GameController,
  GearSix,
  GlobeHemisphereWest,
  HardDrives,
  ListBullets,
  LockSimple,
  LockSimpleOpen,
  PencilSimple,
  Plus,
  RocketLaunch,
  ShieldCheck,
  SortAscending,
  SquaresFour,
  Trash,
  UsersThree,
  Wrench,
  X,
  type Icon,
} from "@phosphor-icons/react"
import { FileIcon } from "@/components/dustdesk/file-icon"
import { ItemContextMenu, type ItemContextMenuAction } from "@/components/dustdesk/item-context-menu"
import { SettingsMenuSection } from "@/components/dustdesk/settings-menu-section"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { usePersistCurrentWindowLayout } from "@/hooks/use-persist-current-window-layout"
import { useDesktopWindowState } from "@/hooks/use-desktop-window-state"
import { useTheme } from "@/hooks/use-theme"
import { allowPathLikeDrag, desktopDropPositionFromDragEnd, didDragEndOutsideWindow, hasDustDeskPathDrag, markDustDeskPathDropAccepted, readDustDeskPathDrag, registerDustDeskDropCategory, type DesktopDropPosition, waitForDustDeskPathDropAcceptance, writeDustDeskPathDrag } from "@/lib/dustdesk-dnd"
import {
  desktopWidgetBackgroundColor,
  desktopWidgetCategoryViewScope,
  desktopWidgetSettingsChangedEvent,
  desktopWidgetViewModesChangedEvent,
  normalizeDesktopWidgetColor,
  readDesktopWidgetSettings,
  readDesktopWidgetViewModes,
  writeDesktopWidgetSettings,
  writeDesktopWidgetViewMode,
  type DesktopWidgetSettings,
  type DesktopWidgetViewMode,
} from "@/lib/desktop-widget-settings"
import { repaintCurrentWindow, safeCurrentWebviewDragDropEvent, safeListen, startCurrentWindowDragging, startCurrentWindowResizeDragging } from "@/lib/tauri-window"
import { cn, displayPathName, extensionFromPath } from "@/lib/utils"
import { useDustDeskStore } from "@/stores/dustdesk-store"
import type { DesktopItem, DesktopOperationEvent, DesktopWindowState } from "@/types"

const splitCategoriesStorageKey = "dustdesk-desktop-widget-split-categories"

type WidgetSettings = DesktopWidgetSettings
type WidgetItemSettings = WidgetSettings & { viewMode: DesktopWidgetViewMode }

interface DesktopCardWindowPageProps {
  routeKind?: string
  routeIndex?: string
}

export function DesktopCardWindowPage({ routeKind, routeIndex }: DesktopCardWindowPageProps = {}) {
  useTheme()
  const { state: desktopWindowState, pending: desktopWindowStatePending, setLocked, setClickThrough } = useDesktopWindowState()
  const params = useParams()
  const snapshot = useDustDeskStore((state) => state.snapshot)
  const loadDesktopSnapshot = useDustDeskStore((state) => state.loadDesktopSnapshot)
  const createCategory = useDustDeskStore((state) => state.createCategory)
  const renameCategory = useDustDeskStore((state) => state.renameCategory)
  const deleteCategory = useDustDeskStore((state) => state.deleteCategory)
  const setCategorySortByName = useDustDeskStore((state) => state.setCategorySortByName)
  const selectCategory = useDustDeskStore((state) => state.selectCategory)
  const addItemsToCategoryLight = useDustDeskStore((state) => state.addItemsToCategoryLight)
  const addLaunchersLight = useDustDeskStore((state) => state.addLaunchersLight)
  const removeLauncher = useDustDeskStore((state) => state.removeLauncher)
  const restoreItemToDesktopLight = useDustDeskStore((state) => state.restoreItemToDesktopLight)
  const completeInternalPathDragLight = useDustDeskStore((state) => state.completeInternalPathDragLight)
  const startRestoreAllToDesktopTask = useDustDeskStore((state) => state.startRestoreAllToDesktopTask)
  const showPathInFolder = useDustDeskStore((state) => state.showPathInFolder)
  const startClassifyDesktopItemsTask = useDustDeskStore((state) => state.startClassifyDesktopItemsTask)
  const getDesktopOperationStatus = useDustDeskStore((state) => state.getDesktopOperationStatus)
  const openPath = useDustDeskStore((state) => state.openPath)
  const openPathAsAdministrator = useDustDeskStore((state) => state.openPathAsAdministrator)
  const startAllLaunchers = useDustDeskStore((state) => state.startAllLaunchers)
  const splitDesktopWidgets = useDustDeskStore((state) => state.splitDesktopWidgets)
  const mergeDesktopCategory = useDustDeskStore((state) => state.mergeDesktopCategory)
  const mergeDesktopWidgets = useDustDeskStore((state) => state.mergeDesktopWidgets)
  const saveDesktopSplitIndices = useDustDeskStore((state) => state.saveDesktopSplitIndices)
  const hideCurrentWindow = useDustDeskStore((state) => state.hideCurrentWindow)
  const [settings, setSettings] = useState<WidgetSettings>(readDesktopWidgetSettings)
  const [viewModes, setViewModes] = useState(readDesktopWidgetViewModes)
  const [menuOpen, setMenuOpen] = useState(false)
  const [notice, setNotice] = useState("")
  const [isClassifyingDesktop, setIsClassifyingDesktop] = useState(false)
  const [isRestoringDesktop, setIsRestoringDesktop] = useState(false)
  const [isMergingCategories, setIsMergingCategories] = useState(false)
  const [dropOperationLabel, setDropOperationLabel] = useState("")
  const desktopOperationLabel = isClassifyingDesktop ? notice || "正在智能收纳桌面..." : isRestoringDesktop ? notice || "正在还原桌面..." : isMergingCategories ? "正在合并分类..." : dropOperationLabel
  const pendingClassifyActionRef = useRef<"split-all" | null>(null)
  const previousSplitCategoryIndicesRef = useRef<number[]>([])
  const desktopOperationRef = useRef<{
    kind: DesktopOperationEvent["kind"] | null
    scope: DesktopOperationEvent["scope"] | null
    done: boolean
  }>({ kind: null, scope: null, done: true })
  const desktopOperationEventRevisionRef = useRef(0)
  const desktopOperationTimeoutRef = useRef<number | null>(null)
  const kind = (routeKind ?? params.kind) === "launcher" ? "launcher" : "category"
  const index = Number(routeIndex ?? params.index ?? 0)
  const windowLabel = kind === "launcher" ? "desktop-launcher" : `desktop-category-${index}`
  const desktopCardIconOptions = useMemo(() => scopedDesktopCardIconOptions(kind, index), [index, kind])
  const loadDesktopCardSnapshot = useCallback(
    (options?: Parameters<typeof loadDesktopSnapshot>[0]) => loadDesktopSnapshot({ ...options, iconOptions: desktopCardIconOptions }),
    [desktopCardIconOptions, loadDesktopSnapshot],
  )
  const category = Number.isFinite(index) ? snapshot.categories[index] : undefined
  const viewScope = kind === "launcher" ? "launcher" : desktopWidgetCategoryViewScope(category?.name ?? "", index)
  const viewMode = viewModes[viewScope] ?? "grid"
  const itemSettings: WidgetItemSettings = { ...settings, viewMode }
  const visual = useMemo(() => {
    if (kind === "launcher") {
      return {
        icon: RocketLaunch,
        color: "#fb923c",
        glow: "rgba(251, 146, 60, 0.28)",
      }
    }
    return categoryVisual(category?.name ?? "分类", index)
  }, [category?.name, index, kind])
  usePersistCurrentWindowLayout(windowLabel)

  useEffect(() => {
    registerDustDeskDropCategory(kind === "category" && Number.isFinite(index) ? index : null)
  }, [index, kind])

  useEffect(() => {
    if (!notice) return
    if (desktopOperationLabel) return
    const timer = window.setTimeout(() => setNotice(""), 2400)
    return () => window.clearTimeout(timer)
  }, [desktopOperationLabel, notice])

  useEffect(() => {
    document.documentElement.classList.add("desktop-widget-root")
    const repaintTimers = [window.setTimeout(() => void repaintCurrentWindow(), 50), window.setTimeout(() => void repaintCurrentWindow(), 240)]
    return () => {
      document.documentElement.classList.remove("desktop-widget-root")
      repaintTimers.forEach((timer) => window.clearTimeout(timer))
    }
  }, [])

  useEffect(() => {
    let unlisten: (() => void) | undefined
    void safeListen("dustdesk://desktop-cards-changed", () => {
      void loadDesktopCardSnapshot({ force: true })
    }).then((value) => {
      unlisten = value
    })
    return () => unlisten?.()
  }, [loadDesktopCardSnapshot])

  useEffect(() => {
    let unlisten: (() => void) | undefined
    void safeListen<DesktopOperationEvent>("dustdesk://desktop-operation", (event) => {
      desktopOperationEventRevisionRef.current += 1
      const payload = event.payload
      if (payload.status === "started") {
        beginDesktopOperation(payload.kind, payload.scope)
        setNotice(payload.message)
        return
      }
      if (payload.status === "progress") {
        beginDesktopOperation(payload.kind, payload.scope)
        setNotice(payload.message)
        return
      }
      if (payload.kind === "classify") {
        void finishClassifyOperation(payload)
      } else if (payload.kind === "restore") {
        void finishRestoreOperation(payload)
      }
    }).then((value) => {
      unlisten = value
      void syncDesktopOperationStatus()
    })
    return () => {
      unlisten?.()
      clearDesktopOperationTimeout()
    }
  }, [loadDesktopCardSnapshot, saveDesktopSplitIndices, splitDesktopWidgets])

  useEffect(() => {
    writeDesktopWidgetSettings(settings)
  }, [settings])

  useEffect(() => {
    const syncSettings = () => setSettings(readDesktopWidgetSettings())
    globalThis.addEventListener(desktopWidgetSettingsChangedEvent, syncSettings)
    globalThis.addEventListener("storage", syncSettings)
    return () => {
      globalThis.removeEventListener(desktopWidgetSettingsChangedEvent, syncSettings)
      globalThis.removeEventListener("storage", syncSettings)
    }
  }, [])

  useEffect(() => {
    const syncViewModes = () => setViewModes(readDesktopWidgetViewModes())
    globalThis.addEventListener(desktopWidgetViewModesChangedEvent, syncViewModes)
    globalThis.addEventListener("storage", syncViewModes)
    return () => {
      globalThis.removeEventListener(desktopWidgetViewModesChangedEvent, syncViewModes)
      globalThis.removeEventListener("storage", syncViewModes)
    }
  }, [])

  useEffect(() => {
    if (!snapshot.data_dir) return
    writeSplitCategoryIndices(snapshot.desktop_layout.split_category_indices)
  }, [snapshot.data_dir, snapshot.desktop_layout.split_category_indices])

  useEffect(() => {
    const onDragOver = (event: DragEvent) => {
      if (!allowPathLikeDrag(event)) return
      if (kind === "category" && event.dataTransfer && hasDustDeskPathDrag(event.dataTransfer)) {
        event.dataTransfer.dropEffect = "move"
      }
    }
    const onDrop = (event: DragEvent) => {
      if (!allowPathLikeDrag(event)) return
      const dataTransfer = event.dataTransfer
      if (!dataTransfer) return
      const paths = readDustDeskPathDrag(dataTransfer)
      if (paths.length === 0) return
      markDustDeskPathDropAccepted(dataTransfer, paths)
      void handleDropped(paths)
    }

    globalThis.addEventListener("dragover", onDragOver)
    globalThis.addEventListener("drop", onDrop)
    return () => {
      globalThis.removeEventListener("dragover", onDragOver)
      globalThis.removeEventListener("drop", onDrop)
    }
  }, [kind, index, snapshot.categories])

  useEffect(() => {
    let unlisten: (() => void) | undefined
    void safeCurrentWebviewDragDropEvent((event) => {
      const payload = event.payload
      if (payload.type !== "drop") return
      markDustDeskPathDropAccepted(null, payload.paths)
      void handleDropped(payload.paths)
    }).then((value) => {
      unlisten = value
    })
    return () => unlisten?.()
  }, [kind, index, category?.name])

  async function handleDropped(paths: string[]) {
    if (paths.length === 0) {
      setNotice("这个桌面图标不是普通文件路径，Windows 不允许直接移动到收纳箱")
      return
    }
    try {
      if (kind === "launcher") {
        setDropOperationLabel(`正在加入快捷启动 ${paths.length} 项...`)
        const added = await addLaunchersLight(paths)
        setNotice(countNotice("已加入快捷启动", added, paths.length, "没有新增启动项"))
      } else {
        const movedBetweenCategories = paths.some((path) =>
          snapshot.categories.some(
            (candidate, candidateIndex) =>
              candidateIndex !== index && candidate.item_paths.some((itemPath) => sameDragPath(itemPath, path)),
          ),
        )
        setDropOperationLabel(`正在${movedBetweenCategories ? "移动" : "收纳"} ${paths.length} 项到「${category?.name ?? "分类"}」...`)
        const added = await addItemsToCategoryLight(index, paths)
        setNotice(countNotice(movedBetweenCategories ? "已移动" : "已收纳", added, paths.length, "没有新增收纳项目"))
      }
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error))
    } finally {
      setDropOperationLabel("")
    }
  }

  async function handleRestoreDragOut(path: string, position: DesktopDropPosition) {
    try {
      setDropOperationLabel(`正在识别拖拽目标：${displayPathName(path)}`)
      const outcome = await completeInternalPathDragLight(index, path, position)
      if (outcome.action === "moved_to_category") {
        const targetName = snapshot.categories[outcome.target_category_index ?? -1]?.name ?? "目标分类"
        setNotice(`已移动到「${targetName}」：${displayPathName(path)}`)
      } else if (outcome.action === "added_to_launcher") {
        setNotice(`已加入快捷启动：${displayPathName(path)}`)
      } else if (outcome.action === "restored_to_desktop") {
        setNotice(`已移回桌面：${displayPathName(outcome.restored_path ?? path)}`)
      } else {
        setNotice(`目标 Box 已接收：${displayPathName(path)}`)
      }
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error))
    } finally {
      setDropOperationLabel("")
    }
  }

  async function handleStartAll(asAdministrator = false) {
    try {
      const count = await startAllLaunchers(asAdministrator)
      setNotice(count > 0 ? `${asAdministrator ? "已请求管理员启动" : "已启动"} ${count} 项` : "快捷启动框还是空的")
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error))
    }
  }

  async function handleClassifyDesktopItems() {
    if (isClassifyingDesktop) return
    pendingClassifyActionRef.current = null
    beginDesktopOperation("classify", "manual")
    setNotice("正在智能收纳桌面...")
    try {
      setMenuOpen(false)
      await startClassifyDesktopItemsTask()
    } catch (error) {
      pendingClassifyActionRef.current = null
      setNotice(error instanceof Error ? error.message : String(error))
      completeDesktopOperation("classify", "manual")
      clearDesktopOperationFlags()
    }
  }

  async function handleOrganizeAndSplitAll() {
    if (isClassifyingDesktop) return
    const previous = readSplitCategoryIndices()
    pendingClassifyActionRef.current = "split-all"
    previousSplitCategoryIndicesRef.current = previous
    beginDesktopOperation("classify", "manual")
    setNotice("正在智能收纳并拆分...")
    try {
      setMenuOpen(false)
      await startClassifyDesktopItemsTask()
    } catch (error) {
      pendingClassifyActionRef.current = null
      writeSplitCategoryIndices(previous)
      setNotice(error instanceof Error ? error.message : String(error))
      completeDesktopOperation("classify", "manual")
      clearDesktopOperationFlags()
    }
  }

  async function handleSplitAllCategories() {
    const previous = readSplitCategoryIndices()

    try {
      setMenuOpen(false)
      const next = await splitDesktopWidgets()
      writeSplitCategoryIndices(next)
      setNotice(next.length > 0 ? `已拆出 ${next.length} 个分类` : "没有可拆出的分类内容")
    } catch (error) {
      writeSplitCategoryIndices(previous)
      setNotice(error instanceof Error ? error.message : String(error))
    }
  }

  async function handleCreateCategory() {
    try {
      setMenuOpen(false)
      const name = window.prompt("分类名称", `新分类 ${snapshot.categories.length + 1}`)?.trim()
      if (!name) return
      await createCategory(name)
      await loadDesktopCardSnapshot()
      setNotice("已新增分类")
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error))
    }
  }

  async function handleRenameCategory() {
    if (kind !== "category" || !category) return
    try {
      setMenuOpen(false)
      selectCategory(index)
      await renameCategory()
      await loadDesktopCardSnapshot()
      setNotice("已重命名分类")
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error))
    }
  }

  async function handleDeleteCategory() {
    if (kind !== "category" || !category) return
    try {
      setMenuOpen(false)
      selectCategory(index)
      await deleteCategory()
      const next = readSplitCategoryIndices().filter((item) => item !== index)
      writeSplitCategoryIndices(next)
      await mergeDesktopCategory(index)
      setNotice("已删除分类")
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error))
    }
  }

  async function handleRefresh() {
    try {
      setMenuOpen(false)
      await loadDesktopCardSnapshot()
      setNotice("已刷新")
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error))
    }
  }

  async function handleMergeCategory() {
    if (kind !== "category") return
    const next = readSplitCategoryIndices().filter((item) => item !== index)
    try {
      setMenuOpen(false)
      await mergeDesktopCategory(index)
      writeSplitCategoryIndices(next)
      setNotice("已合并当前分类")
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error))
    }
  }

  async function handleMergeAllCategories() {
    if (isMergingCategories) return

    const previous = readSplitCategoryIndices()
    setIsMergingCategories(true)
    setNotice("正在合并分类...")
    try {
      setMenuOpen(false)
      await mergeDesktopWidgets()
      await loadDesktopCardSnapshot({ force: true })
      writeSplitCategoryIndices([])
      setNotice(previous.length > 0 ? `已合并 ${previous.length} 个分类` : "分类已处于合并状态")
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error))
    } finally {
      setIsMergingCategories(false)
    }
  }

  async function handleRestoreAllToDesktop() {
    if (isRestoringDesktop) return
    if (!window.confirm("确认把所有收纳箱项目移回桌面吗？这会清空对应的收纳记录。")) return

    beginDesktopOperation("restore", "manual")
    setNotice("正在还原桌面...")
    try {
      setMenuOpen(false)
      await startRestoreAllToDesktopTask()
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error))
      completeDesktopOperation("restore", "manual")
      clearDesktopOperationFlags()
    }
  }

  async function finishClassifyOperation(payload: DesktopOperationEvent) {
    if (!completeDesktopOperation(payload.kind, payload.scope)) return
    clearDesktopOperationFlags()
    if (payload.status === "failed") {
      pendingClassifyActionRef.current = null
      setNotice(payload.message || "智能收纳失败")
      return
    }

    const result = classifyResultFromOperation(payload)
    const resultNotice = payload.message || classifyResultNotice(result)
    if (pendingClassifyActionRef.current === "split-all") {
      pendingClassifyActionRef.current = null
      try {
        const next = await splitDesktopWidgets()
        writeSplitCategoryIndices(next)
        setNotice(next.length > 0 ? `${resultNotice}，已拆出 ${next.length} 个分类` : `${resultNotice}，没有可拆出的分类内容`)
      } catch (error) {
        writeSplitCategoryIndices(previousSplitCategoryIndicesRef.current)
        setNotice(error instanceof Error ? error.message : String(error))
      }
      return
    }

    setNotice(resultNotice)
  }

  async function finishRestoreOperation(payload: DesktopOperationEvent) {
    if (!completeDesktopOperation(payload.kind, payload.scope)) return
    clearDesktopOperationFlags()
    if (payload.status === "failed") {
      setNotice(payload.message || "还原桌面失败")
      return
    }

    if (payload.scope === "manual") {
      writeSplitCategoryIndices([])
      await saveDesktopSplitIndices([])
    }
    setNotice(payload.message || (payload.restored > 0 ? `已还原 ${payload.restored} 项到桌面` : "没有需要还原到桌面的收纳项目"))
  }

  function beginDesktopOperation(kind: DesktopOperationEvent["kind"], scope: DesktopOperationEvent["scope"]) {
    clearDesktopOperationTimeout()
    desktopOperationRef.current = { kind, scope, done: false }
    setIsClassifyingDesktop(kind === "classify")
    setIsRestoringDesktop(kind === "restore")
    desktopOperationTimeoutRef.current = window.setTimeout(() => {
      const current = desktopOperationRef.current
      if (current.kind !== kind || current.scope !== scope || current.done) return
      desktopOperationTimeoutRef.current = null
      void syncDesktopOperationStatus()
    }, 10_000)
  }

  function completeDesktopOperation(kind: DesktopOperationEvent["kind"], scope: DesktopOperationEvent["scope"]) {
    const current = desktopOperationRef.current
    if (current.kind !== kind || current.scope !== scope || current.done) return false
    clearDesktopOperationTimeout()
    desktopOperationRef.current = { kind, scope, done: true }
    return true
  }

  function clearDesktopOperationFlags() {
    setIsClassifyingDesktop(false)
    setIsRestoringDesktop(false)
  }

  function clearDesktopOperationTimeout() {
    if (desktopOperationTimeoutRef.current === null) return
    window.clearTimeout(desktopOperationTimeoutRef.current)
    desktopOperationTimeoutRef.current = null
  }

  async function syncDesktopOperationStatus() {
    const eventRevision = desktopOperationEventRevisionRef.current
    try {
      const status = await getDesktopOperationStatus()
      if (eventRevision !== desktopOperationEventRevisionRef.current) return
      const payload = status.last
      if (!status.running) {
        if (payload?.status === "finished" || payload?.status === "failed") {
          if (payload.kind === "classify") {
            await finishClassifyOperation(payload)
          } else if (payload.kind === "restore") {
            await finishRestoreOperation(payload)
          }
        } else if (!payload) {
          clearDesktopOperationTimeout()
          desktopOperationRef.current = { kind: null, scope: null, done: true }
          clearDesktopOperationFlags()
        }
        return
      }
      if (payload?.kind === "classify") {
        beginDesktopOperation("classify", payload.scope)
        setNotice(payload.message || "正在智能收纳桌面...")
      } else if (payload?.kind === "restore") {
        beginDesktopOperation("restore", payload.scope)
        setNotice(payload.message || "正在还原桌面...")
      }
    } catch {
      if (eventRevision !== desktopOperationEventRevisionRef.current) return
      clearDesktopOperationTimeout()
      desktopOperationRef.current = { kind: null, scope: null, done: true }
      clearDesktopOperationFlags()
    }
  }

  function updateSettings(next: Partial<WidgetSettings>) {
    setSettings((value) => ({ ...value, ...next }))
  }

  function updateViewMode(next: DesktopWidgetViewMode) {
    writeDesktopWidgetViewMode(viewScope, next)
  }

  async function handleToggleDesktopWindowLocked() {
    try {
      const next = await setLocked(!desktopWindowState.locked)
      setNotice(next.locked ? "已锁定全部桌面框" : "已解锁全部桌面框")
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error))
    }
  }

  async function handleToggleDesktopWindowClickThrough() {
    try {
      const next = await setClickThrough(!desktopWindowState.click_through)
      setNotice(next.click_through ? "已开启鼠标穿透，可从系统托盘关闭" : "已关闭鼠标穿透")
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error))
    }
  }

  const Icon = visual.icon
  const items = category?.item_details ?? []
  const frameStyle = {
    backgroundColor: desktopWidgetBackgroundColor(settings),
    boxShadow: `0 24px 90px rgba(2, 6, 23, 0.34), 0 0 0 1px ${visual.glow}`,
  }

  return (
    <div className="desktop-widget-page h-screen w-screen bg-transparent p-2 text-white">
      <section
        className="relative flex h-full w-full min-w-0 flex-col overflow-hidden rounded-2xl border border-white/15 backdrop-blur-2xl"
        style={frameStyle}
      >
        <header
          className={cn("no-drag flex h-12 shrink-0 items-center justify-between gap-2 border-b border-white/10 px-3", desktopWindowState.locked ? "cursor-default" : "cursor-move")}
          onPointerDown={(event) => {
            if (desktopWindowState.locked) return
            if ((event.target as HTMLElement).closest("button,input")) return
            void startCurrentWindowDragging()
          }}
        >
          {kind === "launcher" ? (
            <Badge className="mr-auto bg-white/10 text-white hover:bg-white/10">{snapshot.launchers.length}</Badge>
          ) : (
            <button type="button" className="flex min-w-0 items-center gap-2">
              <Icon className="size-5 shrink-0" weight="duotone" style={{ color: visual.color }} />
              <span className="truncate text-sm font-semibold">{category?.name ?? "分类"}</span>
              <Badge className="bg-white/10 text-white hover:bg-white/10">{items.length}</Badge>
            </button>
          )}
          <div className="ml-auto flex shrink-0 items-center gap-1">
            {kind === "launcher" ? (
              <>
                <Button size="xs" onClick={() => void handleStartAll()}>
                  <RocketLaunch className="size-3.5" weight="duotone" />
                  启动
                </Button>
                <Button size="icon-sm" variant="secondary" title="以管理员身份启动全部" aria-label="以管理员身份启动全部" onClick={() => void handleStartAll(true)}>
                  <ShieldCheck className="size-4" weight="duotone" />
                </Button>
              </>
            ) : null}
            <div className="relative">
              <Button size="icon-sm" variant="secondary" title="设置" aria-label="设置" onClick={() => setMenuOpen((value) => !value)}>
                <GearSix className="size-4" weight="duotone" />
              </Button>
              {menuOpen ? (
                <SettingsMenu
                  kind={kind}
                  settings={settings}
                  viewMode={viewMode}
                  createCategory={handleCreateCategory}
                  renameCategory={handleRenameCategory}
                  deleteCategory={handleDeleteCategory}
                  categorySortByName={category?.sort_by_name ?? false}
                  onToggleCategorySort={async () => {
                    if (!category) return
                    const enabled = !category.sort_by_name
                    try {
                      await setCategorySortByName(index, enabled)
                      setNotice(enabled ? `「${category.name}」已按名称排序` : `「${category.name}」已恢复原收纳顺序`)
                    } catch (error) {
                      setNotice(error instanceof Error ? error.message : String(error))
                    }
                  }}
                  updateSettings={updateSettings}
                  updateViewMode={updateViewMode}
                  desktopWindowState={desktopWindowState}
                  desktopWindowStatePending={desktopWindowStatePending}
                  onToggleDesktopWindowLocked={handleToggleDesktopWindowLocked}
                  onToggleDesktopWindowClickThrough={handleToggleDesktopWindowClickThrough}
                  onRefresh={handleRefresh}
                  onSplitAllCategories={handleSplitAllCategories}
                  onClassifyDesktop={handleClassifyDesktopItems}
                  onOrganizeAndSplitAll={handleOrganizeAndSplitAll}
                  onRestoreAllToDesktop={handleRestoreAllToDesktop}
                  isClassifyingDesktop={isClassifyingDesktop}
                  isRestoringDesktop={isRestoringDesktop}
                  isMergingCategories={isMergingCategories}
                  onMerge={handleMergeCategory}
                  onMergeAllCategories={handleMergeAllCategories}
                  onHide={hideCurrentWindow}
                />
              ) : null}
            </div>
          </div>
        </header>

        {kind === "launcher" ? (
          <LauncherItems
            launchers={snapshot.launchers}
            settings={itemSettings}
            onOpen={openPath}
            onOpenAsAdministrator={openPathAsAdministrator}
            onShowInFolder={showPathInFolder}
            onRemoveLauncher={removeLauncher}
          />
        ) : (
          <CategoryItems
            items={items}
            categoryIndex={index}
            settings={itemSettings}
            onOpen={openPath}
            onShowInFolder={showPathInFolder}
            onRestoreToDesktop={restoreItemToDesktopLight}
            onRestoreDragOut={handleRestoreDragOut}
          />
        )}
        {desktopOperationLabel ? <WidgetOperationOverlay label={desktopOperationLabel} /> : null}
      </section>
      {notice ? (
        <div className="pointer-events-none absolute bottom-3 left-1/2 -translate-x-1/2 rounded-full bg-slate-950/70 px-3 py-1 text-xs text-white/80 ring-1 ring-white/10">{notice}</div>
      ) : null}
      {!desktopWindowState.locked ? (
        <button
          type="button"
          className="no-drag absolute bottom-0 right-0 size-7 cursor-nwse-resize rounded-br-2xl border-b-2 border-r-2 border-white/45"
          aria-label="调整窗口大小"
          onPointerDown={(event) => {
            event.preventDefault()
            void startCurrentWindowResizeDragging("SouthEast")
          }}
        />
      ) : null}
    </div>
  )
}

function WidgetOperationOverlay({ label }: { label: string }) {
  return (
    <div className="no-drag pointer-events-none absolute inset-0 z-40 grid place-items-center bg-slate-950/60 backdrop-blur-md">
      <div className="flex items-center gap-2 rounded-xl border border-white/15 bg-slate-950/90 px-3 py-2 text-xs font-semibold text-white shadow-2xl shadow-black/30">
        <ArrowsClockwise className="size-4 animate-spin text-emerald-300" weight="duotone" />
        <span>{label}</span>
      </div>
    </div>
  )
}

function SettingsMenu({
  kind,
  settings,
  viewMode,
  createCategory,
  renameCategory,
  deleteCategory,
  categorySortByName,
  onToggleCategorySort,
  updateSettings,
  updateViewMode,
  desktopWindowState,
  desktopWindowStatePending,
  onToggleDesktopWindowLocked,
  onToggleDesktopWindowClickThrough,
  onRefresh,
  onSplitAllCategories,
  onClassifyDesktop,
  onOrganizeAndSplitAll,
  onRestoreAllToDesktop,
  isClassifyingDesktop,
  isRestoringDesktop,
  isMergingCategories,
  onMerge,
  onMergeAllCategories,
  onHide,
}: {
  kind: "category" | "launcher"
  settings: WidgetSettings
  viewMode: DesktopWidgetViewMode
  createCategory: () => Promise<void>
  renameCategory: () => Promise<void>
  deleteCategory: () => Promise<void>
  categorySortByName: boolean
  onToggleCategorySort: () => Promise<void>
  updateSettings: (settings: Partial<WidgetSettings>) => void
  updateViewMode: (viewMode: DesktopWidgetViewMode) => void
  desktopWindowState: DesktopWindowState
  desktopWindowStatePending: boolean
  onToggleDesktopWindowLocked: () => Promise<void>
  onToggleDesktopWindowClickThrough: () => Promise<void>
  onRefresh: () => Promise<void>
  onSplitAllCategories: () => Promise<void>
  onClassifyDesktop: () => Promise<void>
  onOrganizeAndSplitAll: () => Promise<void>
  onRestoreAllToDesktop: () => Promise<void>
  isClassifyingDesktop: boolean
  isRestoringDesktop: boolean
  isMergingCategories: boolean
  onMerge: () => Promise<void>
  onMergeAllCategories: () => Promise<void>
  onHide: () => Promise<void>
}) {
  return (
    <div
      className="desktop-widget-scroll absolute right-0 top-8 z-50 max-h-[min(440px,calc(100vh-4rem))] w-64 max-w-[calc(100vw-1rem)] space-y-1.5 overflow-x-hidden overflow-y-auto overscroll-contain rounded-2xl border border-white/15 bg-slate-950/90 p-1.5 text-white shadow-2xl shadow-black/35 backdrop-blur-2xl"
      aria-label="桌面框设置"
    >
      {kind === "category" ? (
        <>
          <SettingsMenuSection title="分类管理">
            <MenuButton icon={Plus} label="新增分类" onClick={() => void createCategory()} />
            <MenuButton icon={PencilSimple} label="重命名当前分类" onClick={() => void renameCategory()} />
            <MenuButton icon={SortAscending} label={categorySortByName ? "关闭名称排序" : "按名称排序"} onClick={() => void onToggleCategorySort()} />
            <MenuButton icon={Trash} label="删除当前分类" onClick={() => void deleteCategory()} />
          </SettingsMenuSection>
          <SettingsMenuSection title="桌面框布局">
            <MenuButton icon={Columns} label="合并当前分类" onClick={() => void onMerge()} />
            <MenuButton icon={Columns} label="拆分全部分类" onClick={() => void onSplitAllCategories()} />
            <MenuButton icon={Columns} label={isMergingCategories ? "合并中" : "一键合并分类"} disabled={isMergingCategories} onClick={() => void onMergeAllCategories()} />
          </SettingsMenuSection>
          <SettingsMenuSection title="桌面整理">
            <MenuButton icon={Columns} label={isClassifyingDesktop ? "智能收纳中" : "智能收纳并拆分全部"} disabled={isClassifyingDesktop} onClick={() => void onOrganizeAndSplitAll()} />
            <MenuButton icon={Archive} label={isClassifyingDesktop ? "智能收纳中" : "智能收纳桌面"} disabled={isClassifyingDesktop} onClick={() => void onClassifyDesktop()} />
            <MenuButton icon={Desktop} label={isRestoringDesktop ? "还原中" : "一键还原桌面"} disabled={isRestoringDesktop} onClick={() => void onRestoreAllToDesktop()} />
          </SettingsMenuSection>
        </>
      ) : null}
      <SettingsMenuSection title="窗口控制">
        <MenuButton icon={ArrowsClockwise} label="刷新内容" onClick={() => void onRefresh()} />
        <MenuButton
          icon={desktopWindowState.locked ? LockSimpleOpen : LockSimple}
          label={desktopWindowState.locked ? "解锁全部桌面框" : "锁定全部桌面框"}
          disabled={desktopWindowStatePending}
          onClick={() => void onToggleDesktopWindowLocked()}
        />
        <MenuButton
          icon={CursorClick}
          label={desktopWindowState.click_through ? "关闭鼠标穿透" : "开启鼠标穿透"}
          disabled={desktopWindowStatePending}
          onClick={() => void onToggleDesktopWindowClickThrough()}
        />
        <MenuButton icon={X} label="隐藏当前框" onClick={() => void onHide()} />
      </SettingsMenuSection>
      <SettingsMenuSection title="外观与排版">
        <RangeRow label="卡片透明度" min={0.25} max={0.85} step={0.05} value={settings.opacity} onChange={(opacity) => updateSettings({ opacity })} />
        <ColorRow value={settings.backgroundColor} onChange={(backgroundColor) => updateSettings({ backgroundColor })} />
        <LayoutModeRow viewMode={viewMode} updateViewMode={updateViewMode} />
        <RangeRow label="项目大小" min={34} max={74} step={4} value={settings.iconSize} onChange={(iconSize) => updateSettings({ iconSize })} />
        {viewMode === "grid" ? (
          <MenuButton icon={settings.showNames ? Eye : EyeSlash} label={settings.showNames ? "隐藏名称" : "显示名称"} onClick={() => updateSettings({ showNames: !settings.showNames })} />
        ) : null}
      </SettingsMenuSection>
    </div>
  )
}

function LayoutModeRow({ viewMode, updateViewMode }: { viewMode: DesktopWidgetViewMode; updateViewMode: (viewMode: DesktopWidgetViewMode) => void }) {
  return (
    <div className="flex items-center justify-between gap-3 rounded-lg px-2 py-1 text-[11px] font-semibold text-white/80">
      <span>项目排版</span>
      <div className="flex rounded-lg border border-white/10 bg-white/5 p-0.5" role="group" aria-label="项目排版">
        <button
          type="button"
          className={cn("flex items-center gap-1 rounded-md px-1.5 py-1 transition", viewMode === "grid" ? "bg-white/15 text-white" : "text-white/45 hover:text-white/80")}
          aria-pressed={viewMode === "grid"}
          onClick={() => updateViewMode("grid")}
        >
          <SquaresFour className="size-3.5" weight="duotone" />
          <span>网格</span>
        </button>
        <button
          type="button"
          className={cn("flex items-center gap-1 rounded-md px-1.5 py-1 transition", viewMode === "list" ? "bg-white/15 text-white" : "text-white/45 hover:text-white/80")}
          aria-pressed={viewMode === "list"}
          onClick={() => updateViewMode("list")}
        >
          <ListBullets className="size-3.5" weight="duotone" />
          <span>列表</span>
        </button>
      </div>
    </div>
  )
}

function MenuButton({ icon: Icon, label, disabled, onClick }: { icon: Icon; label: string; disabled?: boolean; onClick: () => void }) {
  return (
    <button
      type="button"
      disabled={disabled}
      className="flex w-full items-center gap-1.5 whitespace-nowrap rounded-lg px-2 py-1.5 text-left text-[11px] font-semibold text-white/80 transition hover:bg-white/10 hover:text-white active:translate-y-px disabled:cursor-wait disabled:opacity-55"
      onClick={onClick}
    >
      <Icon className="size-3.5 shrink-0" weight="duotone" />
      <span className="truncate">{label}</span>
    </button>
  )
}

function RangeRow({ label, min, max, step, value, onChange }: { label: string; min: number; max: number; step: number; value: number; onChange: (value: number) => void }) {
  return (
    <label className="block rounded-lg px-2 py-1 text-[11px] font-semibold text-white/80">
      <span className="mb-1 flex justify-between">
        <span>{label}</span>
        <span className="text-white/45">{Math.round(value * (max <= 1 ? 100 : 1))}</span>
      </span>
      <input className="w-full accent-emerald-300" type="range" min={min} max={max} step={step} value={value} onChange={(event) => onChange(Number(event.target.value))} />
    </label>
  )
}

function CategoryItems({
  items,
  categoryIndex,
  settings,
  onOpen,
  onShowInFolder,
  onRestoreToDesktop,
  onRestoreDragOut,
}: {
  items: DesktopItem[]
  categoryIndex: number
  settings: WidgetItemSettings
  onOpen: (path: string) => Promise<void>
  onShowInFolder: (path: string) => Promise<void>
  onRestoreToDesktop: (index: number, path: string) => Promise<string>
  onRestoreDragOut: (path: string, position: DesktopDropPosition) => Promise<void>
}) {
  if (items.length === 0) {
    return <EmptyDropHint title="暂无项目" detail="把桌面文件拖到这里，会自动收纳进这个分类。" />
  }

  return (
    <div className="desktop-widget-scroll min-h-0 flex-1 overflow-auto">
      <WidgetGrid settings={settings}>
        {items.map((item) => (
          <WidgetItem
            key={item.path}
            name={item.name || displayPathName(item.path)}
            path={item.path}
            extension={item.extension || extensionFromPath(item.path)}
            isDir={item.is_dir}
            iconDataUrl={item.icon_data_url}
            dragPath={item.path}
            dragEffectAllowed="copyMove"
            settings={settings}
            onOpen={onOpen}
            onDragEndOutside={(position) => onRestoreDragOut(item.path, position)}
            actions={[
              {
                label: "打开",
                icon: "open",
                onSelect: () => onOpen(item.path),
              },
              {
                label: "在资源管理器中显示",
                icon: "folder",
                onSelect: () => onShowInFolder(item.path),
              },
              {
                label: "移回桌面",
                icon: "restore",
                onSelect: async () => {
                  await onRestoreToDesktop(categoryIndex, item.path)
                },
              },
            ]}
          />
        ))}
      </WidgetGrid>
    </div>
  )
}

function LauncherItems({
  launchers,
  settings,
  onOpen,
  onOpenAsAdministrator,
  onShowInFolder,
  onRemoveLauncher,
}: {
  launchers: { name: string; path: string; icon_data_url?: string }[]
  settings: WidgetItemSettings
  onOpen: (path: string) => Promise<void>
  onOpenAsAdministrator: (path: string) => Promise<void>
  onShowInFolder: (path: string) => Promise<void>
  onRemoveLauncher: (path: string) => Promise<void>
}) {
  if (launchers.length === 0) {
    return <EmptyDropHint title="暂无启动项" detail="把快捷方式、程序或常用文件拖到这里。" />
  }

  return (
    <div className="desktop-widget-scroll min-h-0 flex-1 overflow-auto">
      <WidgetGrid settings={settings}>
        {launchers.map((item, index) => (
          <WidgetItem
            key={`${item.path}-${index}`}
            name={item.name || displayPathName(item.path)}
            path={item.path}
            extension={extensionFromPath(item.path)}
            isDir={false}
            iconDataUrl={item.icon_data_url}
            settings={settings}
            onOpen={onOpen}
            actions={[
              {
                label: "启动",
                icon: "open",
                onSelect: () => onOpen(item.path),
              },
              {
                label: "以管理员身份启动",
                icon: "admin",
                onSelect: () => onOpenAsAdministrator(item.path),
              },
              {
                label: "在资源管理器中显示",
                icon: "folder",
                onSelect: () => onShowInFolder(item.path),
              },
              {
                label: "从快捷启动移除",
                icon: "remove",
                tone: "danger",
                onSelect: () => onRemoveLauncher(item.path),
              },
            ]}
          />
        ))}
      </WidgetGrid>
    </div>
  )
}

function WidgetGrid({ settings, children }: { settings: WidgetItemSettings; children: ReactNode }) {
  const isList = settings.viewMode === "list"
  const itemSize = isList ? Math.round(Math.max(28, Math.min(38, settings.iconSize * 0.68))) : settings.iconSize
  const style = {
    "--widget-item-size": `${itemSize}px`,
    ...(isList
      ? {}
      : {
          gridTemplateColumns: `repeat(auto-fill, minmax(${Math.max(76, settings.iconSize + 48)}px, 1fr))`,
        }),
  } as CSSProperties

  return (
    <div className={cn(isList ? "flex flex-col gap-0.5 p-2 pb-6" : "grid gap-2 p-3 pb-6")} style={style} data-view-mode={settings.viewMode}>
      {children}
    </div>
  )
}

function WidgetItem({
  name,
  path,
  extension,
  iconDataUrl,
  dragPath,
  dragEffectAllowed,
  isDir,
  settings,
  onOpen,
  onDragEndOutside,
  actions,
}: {
  name: string
  path: string
  extension: string
  isDir?: boolean
  iconDataUrl?: string
  dragPath?: string
  dragEffectAllowed?: DataTransfer["effectAllowed"]
  settings: WidgetItemSettings
  onOpen: (path: string) => Promise<void>
  onDragEndOutside?: (position: DesktopDropPosition) => unknown
  actions?: ItemContextMenuAction[]
}) {
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null)
  const isList = settings.viewMode === "list"

  return (
    <button
      type="button"
      title={path}
      draggable={Boolean(dragPath)}
      className={cn(
        "flex min-w-0 items-center text-white/90 transition",
        isList
          ? "min-h-11 flex-row gap-2 rounded-lg border border-transparent bg-transparent px-2 py-1.5 text-left hover:border-white/10 hover:bg-white/10"
          : "flex-col gap-2 rounded-xl border border-white/10 bg-white/10 p-2 text-center hover:border-white/25 hover:bg-white/15",
      )}
      onDragStart={(event) => {
        if (!dragPath) return
        writeDustDeskPathDrag(event.dataTransfer, dragPath, dragEffectAllowed)
      }}
      onDragEnd={(event: ReactDragEvent<HTMLButtonElement>) => {
        if (!dragPath || !onDragEndOutside || !didDragEndOutsideWindow(event)) return
        const position = desktopDropPositionFromDragEnd(event)
        void waitForDustDeskPathDropAcceptance(event.dataTransfer, dragPath).then((accepted) => {
          if (accepted) return
          return Promise.resolve(onDragEndOutside(position)).catch(() => undefined)
        })
      }}
      onDoubleClick={() => void onOpen(path)}
      onContextMenu={(event) => {
        event.preventDefault()
        setMenu({ x: event.clientX, y: event.clientY })
      }}
    >
      <FileIcon name={name} extension={extension} isDir={isDir} iconDataUrl={iconDataUrl} className={cn("widget-item-icon bg-white/10 text-white/70", isList && "rounded-md")} />
      {isList || settings.showNames ? <span className={cn("truncate text-xs font-semibold", isList ? "min-w-0 flex-1 text-left leading-5" : "w-full")}>{name}</span> : null}
      {menu && actions ? <ItemContextMenu x={menu.x} y={menu.y} actions={actions} onClose={() => setMenu(null)} /> : null}
    </button>
  )
}

function EmptyDropHint({ title, detail }: { title: string; detail: string }) {
  return (
    <div className="grid h-full min-h-[140px] flex-1 place-items-center px-6 text-center">
      <div>
        <div className="mx-auto mb-3 grid size-12 place-items-center rounded-2xl bg-white/10 ring-1 ring-white/10">
          <FolderOpen className="size-6 text-white/60" weight="duotone" />
        </div>
        <div className="text-sm font-semibold text-white/90">{title}</div>
        <div className="mt-1 max-w-56 text-xs leading-5 text-white/50">{detail}</div>
      </div>
    </div>
  )
}

function classifyResultNotice(result: { moved: number; skipped: number; category_counts: { name: string; count: number }[] }) {
  if (result.moved === 0) {
    return result.skipped > 0 ? `没有新的桌面项目可收纳，已跳过 ${result.skipped} 项` : "没有新的桌面项目可收纳"
  }

  const detail = result.category_counts
    .slice(0, 4)
    .map((item) => `${item.name} ${item.count}`)
    .join("、")
  return `已智能收纳 ${result.moved} 项${detail ? `：${detail}` : ""}${result.skipped ? `，跳过 ${result.skipped} 项` : ""}`
}

function classifyResultFromOperation(payload: DesktopOperationEvent) {
  return {
    moved: payload.moved,
    skipped: payload.skipped,
    category_counts: payload.category_counts ?? [],
  }
}

function ColorRow({ value, onChange }: { value: string; onChange: (value: string) => void }) {
  return (
    <label className="flex cursor-pointer items-center justify-between gap-3 rounded-lg px-2 py-1 text-[11px] font-semibold text-white/80 transition hover:bg-white/10 hover:text-white">
      <span>收纳框颜色</span>
      <span className="flex items-center gap-2">
        <span className="font-mono text-[10px] uppercase text-white/50">{value}</span>
        <input
          className="h-6 w-9 cursor-pointer rounded border border-white/20 bg-transparent p-0.5"
          type="color"
          value={value}
          aria-label="选择收纳框颜色"
          onInput={(event) => onChange(normalizeDesktopWidgetColor(event.currentTarget.value))}
        />
      </span>
    </label>
  )
}

function countNotice(action: string, count: number, total: number, empty: string) {
  if (count <= 0) return empty
  const skipped = Math.max(0, total - count)
  return `${action} ${count} 项${skipped ? `，跳过 ${skipped} 项` : ""}`
}

function sameDragPath(left: string, right: string) {
  const normalize = (value: string) => value.trim().replace(/\//g, "\\").replace(/\\+$/, "").toLowerCase()
  return normalize(left) === normalize(right)
}

function readSplitCategoryIndices(): number[] {
  try {
    const parsed = JSON.parse(globalThis.localStorage.getItem(splitCategoriesStorageKey) || "[]")
    if (!Array.isArray(parsed)) return []
    return [...new Set(parsed.map(Number).filter((index) => Number.isInteger(index) && index >= 0))].sort((left, right) => left - right)
  } catch {
    return []
  }
}

function writeSplitCategoryIndices(indices: number[]) {
  globalThis.localStorage.setItem(splitCategoriesStorageKey, JSON.stringify([...new Set(indices)].sort((left, right) => left - right)))
}

function scopedDesktopCardIconOptions(kind: string, index: number) {
  if (kind === "launcher") {
    return {
      includeDesktopItems: false,
      includeLaunchers: true,
      categoryIndices: [],
    }
  }
  return {
    includeDesktopItems: false,
    includeLaunchers: false,
    categoryIndices: Number.isFinite(index) ? [index] : [],
  }
}

function clamp(value: number, min: number, max: number) {
  if (!Number.isFinite(value)) return min
  return Math.min(max, Math.max(min, value))
}

function categoryVisual(name: string, index: number): { icon: Icon; color: string; glow: string } {
  const lower = name.toLowerCase()
  const presets: Array<[boolean, Icon, string, string]> = [
    [name.includes("开发") || lower.includes("dev"), Code, "#6ee7b7", "rgba(52, 211, 153, 0.22)"],
    [name.includes("工具") || lower.includes("tool"), Wrench, "#93c5fd", "rgba(96, 165, 250, 0.22)"],
    [name.includes("文档") || lower.includes("doc"), FileText, "#f87171", "rgba(248, 113, 113, 0.22)"],
    [name.includes("社交") || lower.includes("chat"), UsersThree, "#fbbf24", "rgba(251, 191, 36, 0.22)"],
    [name.includes("游戏") || lower.includes("game"), GameController, "#60a5fa", "rgba(96, 165, 250, 0.22)"],
    [name.includes("办公") || lower.includes("office"), Briefcase, "#c084fc", "rgba(192, 132, 252, 0.22)"],
    [name.includes("浏览") || lower.includes("browser"), GlobeHemisphereWest, "#38bdf8", "rgba(56, 189, 248, 0.22)"],
    [name.includes("本机") || lower.includes("local"), HardDrives, "#a3e635", "rgba(163, 230, 53, 0.22)"],
  ]
  const matched = presets.find(([matches]) => matches)
  if (matched) {
    return { icon: matched[1], color: matched[2], glow: matched[3] }
  }

  const fallback = [
    ["#facc15", "rgba(250, 204, 21, 0.2)"],
    ["#22d3ee", "rgba(34, 211, 238, 0.2)"],
    ["#fb7185", "rgba(251, 113, 133, 0.2)"],
    ["#a78bfa", "rgba(167, 139, 250, 0.2)"],
  ][index % 4]
  return { icon: FolderOpen, color: fallback[0], glow: fallback[1] }
}
