import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type DragEvent as ReactDragEvent,
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
  type WheelEvent as ReactWheelEvent,
} from "react"
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
import { allowPathLikeDrag, desktopDropPositionFromDragEnd, didDragEndOutsideWindow, readDustDeskPathDrag, type DesktopDropPosition, writeDustDeskPathDrag } from "@/lib/dustdesk-dnd"
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
import { cn, displayPathName, extensionFromPath, remapIndexAfterMove } from "@/lib/utils"
import { useDustDeskStore } from "@/stores/dustdesk-store"
import type { CategoryOrderChangedEvent, DesktopItem, DesktopOperationEvent, DesktopWindowState } from "@/types"

const activeCategoryKey = "dustdesk-desktop-widget-active-category"
const layoutsStorageKey = "dustdesk-desktop-widget-layouts"
const legacyCollapsedStorageKey = "dustdesk-desktop-widget-collapsed"
const splitCategoriesStorageKey = "dustdesk-desktop-widget-split-categories"

type DropTarget = { type: "category"; index: number } | { type: "launcher" }

interface CategoryTab {
  id: string
  label: string
  count: number
  index: number
  icon: Icon
  color: string
  glow: string
}

type WidgetSettings = DesktopWidgetSettings
type WidgetItemSettings = WidgetSettings & { viewMode: DesktopWidgetViewMode }

interface CardLayout {
  x: number
  y: number
  width: number
  height: number
}

const launcherVisual = {
  icon: RocketLaunch,
  color: "#fb923c",
  glow: "rgba(251, 146, 60, 0.28)",
}

export function DesktopWidgetPage() {
  useTheme()
  const { state: desktopWindowState, pending: desktopWindowStatePending, setLocked, setClickThrough } = useDesktopWindowState()
  usePersistCurrentWindowLayout("desktop-widget")
  const snapshot = useDustDeskStore((state) => state.snapshot)
  const loadDesktopSnapshot = useDustDeskStore((state) => state.loadDesktopSnapshot)
  const createCategory = useDustDeskStore((state) => state.createCategory)
  const renameCategory = useDustDeskStore((state) => state.renameCategory)
  const deleteCategory = useDustDeskStore((state) => state.deleteCategory)
  const reorderCategoryLight = useDustDeskStore((state) => state.reorderCategoryLight)
  const selectCategory = useDustDeskStore((state) => state.selectCategory)
  const openPath = useDustDeskStore((state) => state.openPath)
  const addItemsToCategoryLight = useDustDeskStore((state) => state.addItemsToCategoryLight)
  const addLaunchersLight = useDustDeskStore((state) => state.addLaunchersLight)
  const removeLauncher = useDustDeskStore((state) => state.removeLauncher)
  const restoreItemToDesktopLight = useDustDeskStore((state) => state.restoreItemToDesktopLight)
  const startRestoreAllToDesktopTask = useDustDeskStore((state) => state.startRestoreAllToDesktopTask)
  const showPathInFolder = useDustDeskStore((state) => state.showPathInFolder)
  const startClassifyDesktopItemsTask = useDustDeskStore((state) => state.startClassifyDesktopItemsTask)
  const getDesktopOperationStatus = useDustDeskStore((state) => state.getDesktopOperationStatus)
  const startAllLaunchers = useDustDeskStore((state) => state.startAllLaunchers)
  const splitDesktopWidgets = useDustDeskStore((state) => state.splitDesktopWidgets)
  const splitDesktopCategory = useDustDeskStore((state) => state.splitDesktopCategory)
  const mergeDesktopWidgets = useDustDeskStore((state) => state.mergeDesktopWidgets)
  const saveDesktopSplitIndices = useDustDeskStore((state) => state.saveDesktopSplitIndices)
  const hideCurrentWindow = useDustDeskStore((state) => state.hideCurrentWindow)
  const [activeCategoryId, setActiveCategoryId] = useState(() => globalThis.localStorage.getItem(activeCategoryKey) || "category:0")
  const [splitCategoryIndices, setSplitCategoryIndices] = useState<number[]>([])
  const [settings, setSettings] = useState<WidgetSettings>(readDesktopWidgetSettings)
  const [viewModes, setViewModes] = useState(readDesktopWidgetViewModes)
  const [layouts, setLayouts] = useState<Record<string, CardLayout>>(readLayouts)
  const [openSettingsId, setOpenSettingsId] = useState("")
  const [hoverZone, setHoverZone] = useState("")
  const [notice, setNotice] = useState("")
  const [isClassifyingDesktop, setIsClassifyingDesktop] = useState(false)
  const [isRestoringDesktop, setIsRestoringDesktop] = useState(false)
  const [isMergingCategories, setIsMergingCategories] = useState(false)
  const [dropOperationLabel, setDropOperationLabel] = useState("")
  const desktopOperationLabel = isClassifyingDesktop ? notice || "正在智能收纳桌面..." : isRestoringDesktop ? notice || "正在还原桌面..." : isMergingCategories ? "正在合并分类..." : dropOperationLabel
  const hasSnapshot = Boolean(snapshot.data_dir)
  const dragRef = useRef<{
    id: string
    x: number
    y: number
    rect: CardLayout
  } | null>(null)
  const resizeRef = useRef<{
    id: string
    x: number
    y: number
    rect: CardLayout
  } | null>(null)
  const pendingClassifyActionRef = useRef<"split-all" | null>(null)
  const previousSplitCategoryIndicesRef = useRef<number[]>([])
  const desktopOperationRef = useRef<{
    kind: DesktopOperationEvent["kind"] | null
    scope: DesktopOperationEvent["scope"] | null
    done: boolean
  }>({ kind: null, scope: null, done: true })
  const desktopOperationEventRevisionRef = useRef(0)
  const desktopOperationTimeoutRef = useRef<number | null>(null)

  useEffect(() => {
    if (!notice) return
    if (desktopOperationLabel) return
    const timer = window.setTimeout(() => setNotice(""), 2400)
    return () => window.clearTimeout(timer)
  }, [desktopOperationLabel, notice])

  const categories = useMemo<CategoryTab[]>(() => {
    return snapshot.categories.map((category, index) => ({
      id: `category:${index}`,
      label: category.name,
      count: category.item_paths.length,
      index,
      ...categoryVisual(category.name, index),
    }))
  }, [snapshot.categories])

  const splitCategorySet = useMemo(() => new Set(splitCategoryIndices), [splitCategoryIndices])
  const groupedCategories = useMemo(() => categories.filter((category) => !splitCategorySet.has(category.index)), [categories, splitCategorySet])
  const activeCategory = groupedCategories.find((category) => category.id === activeCategoryId) ?? groupedCategories[0]
  const viewScope = desktopWidgetCategoryViewScope(activeCategory?.label ?? "", activeCategory?.index ?? 0)
  const viewMode = viewModes[viewScope] ?? "grid"
  const itemSettings: WidgetItemSettings = { ...settings, viewMode }

  useEffect(() => {
    document.documentElement.classList.add("desktop-widget-root")
    const repaintTimers = [window.setTimeout(() => void repaintCurrentWindow(), 50), window.setTimeout(() => void repaintCurrentWindow(), 240)]
    const hadLegacyCollapsedState = Boolean(globalThis.localStorage.getItem(legacyCollapsedStorageKey))
    if (hadLegacyCollapsedState) {
      globalThis.localStorage.removeItem(legacyCollapsedStorageKey)
    }
    return () => {
      document.documentElement.classList.remove("desktop-widget-root")
      repaintTimers.forEach((timer) => window.clearTimeout(timer))
    }
  }, [])

  useEffect(() => {
    let unlisten: (() => void) | undefined
    void safeListen("dustdesk://desktop-cards-changed", () => {
      void loadDesktopSnapshot({ force: true })
    }).then((value) => {
      unlisten = value
    })
    return () => unlisten?.()
  }, [loadDesktopSnapshot])

  useEffect(() => {
    let unlisten: (() => void) | undefined
    void safeListen<CategoryOrderChangedEvent>("dustdesk://category-order-changed", (event) => {
      const { from_index: fromIndex, to_index: toIndex } = event.payload
      setActiveCategoryId((current) => remapCategoryIdAfterMove(current, fromIndex, toIndex))
      setSplitCategoryIndices((current) => {
        const next = [...new Set(current.map((index) => remapIndexAfterMove(index, fromIndex, toIndex)))].sort((left, right) => left - right)
        writeSplitCategoryIndices(next)
        return next
      })
    }).then((value) => {
      unlisten = value
    })
    return () => unlisten?.()
  }, [])

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
  }, [loadDesktopSnapshot, saveDesktopSplitIndices, splitDesktopWidgets])

  useEffect(() => {
    const valid = splitCategoryIndices.filter((index) => index >= 0 && index < snapshot.categories.length)
    if (valid.length === splitCategoryIndices.length) return
    writeSplitCategoryIndices(valid)
    setSplitCategoryIndices(valid)
  }, [snapshot.categories.length, splitCategoryIndices])

  useEffect(() => {
    if (!hasSnapshot) return
    const persisted = normalizeSplitCategoryIndices(snapshot.desktop_layout.split_category_indices, snapshot.categories.length)
    setSplitCategoryIndices((current) => {
      if (sameNumberList(current, persisted)) return current
      writeSplitCategoryIndices(persisted)
      return persisted
    })
  }, [hasSnapshot, snapshot.categories.length, snapshot.desktop_layout.split_category_indices])

  useEffect(() => {
    if (activeCategory || groupedCategories.length === 0) return
    setActiveCategoryId(groupedCategories[0].id)
  }, [activeCategory, groupedCategories])

  useEffect(() => {
    globalThis.localStorage.setItem(activeCategoryKey, activeCategory?.id ?? "category:0")
  }, [activeCategory?.id])

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
    globalThis.localStorage.setItem(layoutsStorageKey, JSON.stringify(layouts))
  }, [layouts])

  useEffect(() => {
    const onDragOver = (event: DragEvent) => {
      if (!allowPathLikeDrag(event)) return
      setHoverZone(dropZoneFromClientPoint(event.clientX, event.clientY))
    }
    const onDrop = (event: DragEvent) => {
      if (!allowPathLikeDrag(event)) return
      setHoverZone("")
      const dataTransfer = event.dataTransfer
      if (!dataTransfer) return
      const paths = readDustDeskPathDrag(dataTransfer)
      if (paths.length === 0) return
      const target = parseDropTarget(dropZoneFromClientPoint(event.clientX, event.clientY) || activeCategory?.id || "category:0")
      if (!target) return
      void handleDroppedPaths(target, paths)
    }

    globalThis.addEventListener("dragover", onDragOver)
    globalThis.addEventListener("drop", onDrop)
    return () => {
      globalThis.removeEventListener("dragover", onDragOver)
      globalThis.removeEventListener("drop", onDrop)
    }
  }, [activeCategory?.id, snapshot.categories])

  useEffect(() => {
    let unlisten: (() => void) | undefined

    void safeCurrentWebviewDragDropEvent((event) => {
      const payload = event.payload
      if (payload.type === "leave") {
        setHoverZone("")
        return
      }

      const zone = "position" in payload ? dropZoneFromPoint(payload.position.x, payload.position.y) : ""
      setHoverZone(zone)

      if (payload.type !== "drop") return
      setHoverZone("")
      const target = parseDropTarget(zone || activeCategory?.id || "category:0")
      if (!target) return
      void handleDroppedPaths(target, payload.paths)
    }).then((value) => {
      unlisten = value
    })

    return () => {
      unlisten?.()
    }
  }, [activeCategory?.id, snapshot.categories])

  useEffect(() => {
    const onPointerMove = (event: PointerEvent) => {
      if (dragRef.current) {
        const { id, x, y, rect } = dragRef.current
        setLayouts((current) => ({
          ...current,
          [id]: {
            ...rect,
            x: Math.max(0, rect.x + event.clientX - x),
            y: Math.max(0, rect.y + event.clientY - y),
          },
        }))
      }

      if (resizeRef.current) {
        const { id, x, y, rect } = resizeRef.current
        setLayouts((current) => ({
          ...current,
          [id]: {
            ...rect,
            width: Math.max(230, rect.width + event.clientX - x),
            height: Math.max(150, rect.height + event.clientY - y),
          },
        }))
      }
    }

    const onPointerUp = () => {
      dragRef.current = null
      resizeRef.current = null
    }

    globalThis.addEventListener("pointermove", onPointerMove)
    globalThis.addEventListener("pointerup", onPointerUp)
    return () => {
      globalThis.removeEventListener("pointermove", onPointerMove)
      globalThis.removeEventListener("pointerup", onPointerUp)
    }
  }, [])

  async function handleDroppedPaths(target: DropTarget, paths: string[]) {
    if (paths.length === 0) {
      setNotice("这个桌面图标不是普通文件路径，Windows 不允许直接移动到收纳箱")
      return
    }
    try {
      if (target.type === "launcher") {
        setDropOperationLabel(`正在加入快捷启动 ${paths.length} 项...`)
        const added = await addLaunchersLight(paths)
        setNotice(countNotice("已加入快捷启动", added, paths.length, "没有新增启动项"))
      } else {
        setDropOperationLabel(`正在收纳 ${paths.length} 项到「${snapshot.categories[target.index]?.name ?? "分类"}」...`)
        const added = await addItemsToCategoryLight(target.index, paths)
        setActiveCategoryId(`category:${target.index}`)
        setNotice(countNotice(`已收纳到「${snapshot.categories[target.index]?.name ?? "分类"}」`, added, paths.length, "没有新增收纳项目"))
      }
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error))
    } finally {
      setDropOperationLabel("")
    }
  }

  async function handleRestoreDragOut(index: number, path: string, position: DesktopDropPosition) {
    try {
      setDropOperationLabel(`正在移回桌面：${displayPathName(path)}`)
      const restored = await restoreItemToDesktopLight(index, path, position)
      setNotice(`已移回桌面：${displayPathName(restored)}`)
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error))
    } finally {
      setDropOperationLabel("")
    }
  }

  async function handleStartAll() {
    const count = await startAllLaunchers()
    setNotice(count > 0 ? `已启动 ${count} 项` : "快捷启动框还是空的")
  }

  async function handleClassifyDesktopItems() {
    if (isClassifyingDesktop) return
    pendingClassifyActionRef.current = null
    beginDesktopOperation("classify", "manual")
    setNotice("正在智能收纳桌面...")
    try {
      setOpenSettingsId("")
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
    const previous = splitCategoryIndices
    pendingClassifyActionRef.current = "split-all"
    previousSplitCategoryIndicesRef.current = previous
    beginDesktopOperation("classify", "manual")
    setNotice("正在智能收纳并拆分...")
    try {
      setOpenSettingsId("")
      await startClassifyDesktopItemsTask()
    } catch (error) {
      pendingClassifyActionRef.current = null
      writeSplitCategoryIndices(previous)
      setSplitCategoryIndices(previous)
      setNotice(error instanceof Error ? error.message : String(error))
      completeDesktopOperation("classify", "manual")
      clearDesktopOperationFlags()
    }
  }

  async function handleSplitAllCategories() {
    const previous = splitCategoryIndices

    try {
      const next = await splitDesktopWidgets()
      setOpenSettingsId("")
      writeSplitCategoryIndices(next)
      setSplitCategoryIndices(next)
      setNotice(next.length > 0 ? `已拆出 ${next.length} 个分类` : "没有可拆出的分类内容")
    } catch (error) {
      writeSplitCategoryIndices(previous)
      setSplitCategoryIndices(previous)
      setNotice(error instanceof Error ? error.message : String(error))
    }
  }

  async function handleMergeAllCategories() {
    if (isMergingCategories) return

    setIsMergingCategories(true)
    setNotice("正在合并分类...")
    try {
      setOpenSettingsId("")
      await mergeDesktopWidgets()
      await loadDesktopSnapshot({ force: true })
      writeSplitCategoryIndices([])
      setSplitCategoryIndices([])
      setActiveCategoryId("category:0")
      setNotice(splitCategoryIndices.length > 0 ? `已合并 ${splitCategoryIndices.length} 个分类` : "分类已处于合并状态")
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error))
    } finally {
      setIsMergingCategories(false)
    }
  }

  async function handleSplitCategory(index: number) {
    if (!snapshot.categories[index]) return
    const next = normalizeSplitCategoryIndices([...splitCategoryIndices, index], snapshot.categories.length)

    try {
      setOpenSettingsId("")
      await splitDesktopCategory(index)
      writeSplitCategoryIndices(next)
      setSplitCategoryIndices(next)
      const nextActive = groupedCategories.find((category) => category.index !== index)
      if (nextActive) {
        setActiveCategoryId(nextActive.id)
      }
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error))
    }
  }

  async function handleCreateCategory() {
    try {
      setOpenSettingsId("")
      const name = window.prompt("分类名称", `新分类 ${snapshot.categories.length + 1}`)?.trim()
      if (!name) return
      await createCategory(name)
      await loadDesktopSnapshot()
      setNotice("已新增分类")
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error))
    }
  }

  async function handleRenameActiveCategory() {
    if (!activeCategory) return
    try {
      setOpenSettingsId("")
      selectCategory(activeCategory.index)
      await renameCategory()
      await loadDesktopSnapshot()
      setNotice("已重命名分类")
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error))
    }
  }

  async function handleDeleteActiveCategory() {
    if (!activeCategory) return
    try {
      setOpenSettingsId("")
      selectCategory(activeCategory.index)
      await deleteCategory()
      await loadDesktopSnapshot()
      setActiveCategoryId("category:0")
      setNotice("已删除分类")
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error))
    }
  }

  async function handleRefresh() {
    try {
      setOpenSettingsId("")
      await loadDesktopSnapshot()
      setNotice("已刷新")
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error))
    }
  }

  async function handleRestoreAllToDesktop() {
    if (isRestoringDesktop) return
    if (!window.confirm("确认把所有收纳箱项目移回桌面吗？这会清空对应的收纳记录。")) return

    beginDesktopOperation("restore", "manual")
    setNotice("正在还原桌面...")
    try {
      setOpenSettingsId("")
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
        setSplitCategoryIndices(next)
        setNotice(next.length > 0 ? `${resultNotice}，已拆出 ${next.length} 个分类` : `${resultNotice}，没有可拆出的分类内容`)
      } catch (error) {
        const previous = previousSplitCategoryIndicesRef.current
        writeSplitCategoryIndices(previous)
        setSplitCategoryIndices(previous)
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
      setSplitCategoryIndices([])
      setActiveCategoryId("category:0")
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

  async function handleReorderCategory(fromIndex: number, toIndex: number) {
    if (fromIndex === toIndex) return
    const categoryName = snapshot.categories[fromIndex]?.name ?? "分类"
    try {
      await reorderCategoryLight(fromIndex, toIndex)
      setNotice(`已调整「${categoryName}」的分类顺序`)
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error))
      throw error
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

  function startDrag(id: string, event: ReactPointerEvent<HTMLElement>) {
    if (desktopWindowState.locked) return
    if ((event.target as HTMLElement).closest("button,input")) return
    dragRef.current = {
      id,
      x: event.clientX,
      y: event.clientY,
      rect: layoutFor(id, frameIndex(id, categories), layouts),
    }
  }

  function startResize(id: string, event: ReactPointerEvent<HTMLButtonElement>) {
    if (desktopWindowState.locked) return
    event.preventDefault()
    event.stopPropagation()
    resizeRef.current = {
      id,
      x: event.clientX,
      y: event.clientY,
      rect: layoutFor(id, frameIndex(id, categories), layouts),
    }
  }

  const menuProps = {
    settings,
    viewMode,
    desktopWindowState,
    desktopWindowStatePending,
    createCategory: handleCreateCategory,
    renameCategory: handleRenameActiveCategory,
    deleteCategory: handleDeleteActiveCategory,
    updateSettings,
    updateViewMode,
    onToggleDesktopWindowLocked: handleToggleDesktopWindowLocked,
    onToggleDesktopWindowClickThrough: handleToggleDesktopWindowClickThrough,
    onSplitCategory: () => (activeCategory ? handleSplitCategory(activeCategory.index) : Promise.resolve()),
    onSplitAllCategories: handleSplitAllCategories,
    onMergeAllCategories: handleMergeAllCategories,
    onClassifyDesktop: handleClassifyDesktopItems,
    onOrganizeAndSplitAll: handleOrganizeAndSplitAll,
    onRestoreAllToDesktop: handleRestoreAllToDesktop,
    isClassifyingDesktop,
    isRestoringDesktop,
    isMergingCategories,
    onRefresh: handleRefresh,
    onHide: hideCurrentWindow,
  }

  const frameStyle = {
    backgroundColor: desktopWidgetBackgroundColor(settings),
    boxShadow: `0 24px 90px rgba(2, 6, 23, 0.34), 0 0 0 1px ${activeCategory?.glow ?? "rgba(255,255,255,0.12)"}`,
  }

  return (
    <div className="desktop-widget-page h-screen w-screen overflow-hidden bg-transparent p-2 text-white">
      <section
        data-frame-id="organizer"
        data-drop-zone={activeCategory?.id ?? "category:0"}
        style={frameStyle}
        className={cn(
          "relative flex h-full w-full min-w-0 flex-col overflow-hidden rounded-2xl border border-white/15 backdrop-blur-2xl transition-colors",
          activeCategory && (hoverZone === "organizer" || hoverZone === activeCategory.id) && "border-emerald-200/80",
        )}
      >
        <div
          className={desktopWindowState.locked ? "cursor-default" : "cursor-move"}
          onPointerDown={(event) => {
            if (desktopWindowState.locked) return
            if ((event.target as HTMLElement).closest("button,input")) return
            void startCurrentWindowDragging()
          }}
        >
          <OrganizerTop
            categories={groupedCategories}
            activeId={activeCategory?.id ?? "category:0"}
            hoverZone={hoverZone}
            menuProps={menuProps}
            openSettings={openSettingsId === "organizer"}
            onActiveChange={setActiveCategoryId}
            onOpenSettings={() => setOpenSettingsId((id) => (id === "organizer" ? "" : "organizer"))}
            onSplitCategory={handleSplitCategory}
            onReorderCategory={handleReorderCategory}
          />
        </div>
        {activeCategory ? (
          <CategoryItems
            category={snapshot.categories[activeCategory.index]}
            categoryIndex={activeCategory.index}
            settings={itemSettings}
            onOpen={openPath}
            onShowInFolder={showPathInFolder}
            onRestoreToDesktop={restoreItemToDesktopLight}
            onRestoreDragOut={handleRestoreDragOut}
          />
        ) : (
          <EmptyDropHint title="分类都已拆出" detail="在设置里点一键合并分类，就会回到这个分类组。" />
        )}
        {desktopOperationLabel ? <WidgetOperationOverlay label={desktopOperationLabel} /> : null}
      </section>
      {notice ? (
        <div className="pointer-events-none absolute bottom-3 left-1/2 -translate-x-1/2 rounded-full bg-slate-950/70 px-3 py-1 text-xs text-white/80 ring-1 ring-white/10">{notice}</div>
      ) : null}
      {!desktopWindowState.locked ? (
        <button
          type="button"
          className="no-drag absolute bottom-0 right-0 size-6 cursor-nwse-resize rounded-br-2xl border-b-2 border-r-2 border-white/35"
          aria-label="调整桌面框窗口大小"
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

function WidgetFrame({
  frameId,
  dropZone,
  layout,
  hoverZone,
  settings,
  glow,
  onDragStart,
  onResizeStart,
  children,
}: {
  frameId: string
  dropZone: string
  layout: CardLayout
  hoverZone: string
  settings: WidgetSettings
  glow?: string
  openSettings: boolean
  onOpenSettings: () => void
  onDragStart: (event: ReactPointerEvent<HTMLElement>) => void
  onResizeStart: (event: ReactPointerEvent<HTMLButtonElement>) => void
  children: ReactNode
}) {
  const style = {
    left: layout.x,
    top: layout.y,
    width: layout.width,
    height: layout.height,
    backgroundColor: desktopWidgetBackgroundColor(settings),
    boxShadow: `0 24px 90px rgba(2, 6, 23, 0.34), 0 0 0 1px ${glow ?? "rgba(255,255,255,0.12)"}`,
  }

  return (
    <section
      data-frame-id={frameId}
      data-drop-zone={dropZone}
      style={style}
      className={cn(
        "no-drag absolute flex min-w-0 flex-col overflow-hidden rounded-2xl border border-white/15 backdrop-blur-2xl transition-colors",
        (hoverZone === frameId || hoverZone === dropZone) && "border-emerald-200/80",
      )}
    >
      <div className="cursor-move" onPointerDown={onDragStart}>
        {children}
      </div>
      <button
        type="button"
        className="no-drag absolute bottom-1 right-1 size-6 cursor-nwse-resize rounded-br-xl border-b-2 border-r-2 border-white/40 opacity-80"
        aria-label="调整卡片框大小"
        onPointerDown={onResizeStart}
      />
    </section>
  )
}

interface SettingsMenuProps {
  settings: WidgetSettings
  viewMode: DesktopWidgetViewMode
  desktopWindowState: DesktopWindowState
  desktopWindowStatePending: boolean
  createCategory: () => Promise<void>
  renameCategory: () => Promise<void>
  deleteCategory: () => Promise<void>
  updateSettings: (settings: Partial<WidgetSettings>) => void
  updateViewMode: (viewMode: DesktopWidgetViewMode) => void
  onToggleDesktopWindowLocked: () => Promise<void>
  onToggleDesktopWindowClickThrough: () => Promise<void>
  onSplitCategory: () => Promise<void>
  onSplitAllCategories: () => Promise<void>
  onMergeAllCategories: () => Promise<void>
  onClassifyDesktop: () => Promise<void>
  onOrganizeAndSplitAll: () => Promise<void>
  onRestoreAllToDesktop: () => Promise<void>
  isClassifyingDesktop: boolean
  isRestoringDesktop: boolean
  isMergingCategories: boolean
  onRefresh: () => Promise<void>
  onHide: () => Promise<void>
}

function OrganizerTop({
  categories,
  activeId,
  hoverZone,
  menuProps,
  openSettings,
  onActiveChange,
  onOpenSettings,
  onSplitCategory,
  onReorderCategory,
}: {
  categories: CategoryTab[]
  activeId: string
  hoverZone: string
  menuProps: SettingsMenuProps
  openSettings: boolean
  onActiveChange: (id: string) => void
  onOpenSettings: () => void
  onSplitCategory: (index: number) => Promise<void>
  onReorderCategory: (fromIndex: number, toIndex: number) => Promise<void>
}) {
  return (
    <div className="no-drag flex h-12 shrink-0 items-center gap-2 border-b border-white/10 px-2">
      <CategoryScroller categories={categories} activeId={activeId} hoverZone={hoverZone} onActiveChange={onActiveChange} onSplitCategory={onSplitCategory} onReorderCategory={onReorderCategory} />
      <FrameActions kind="organizer" menuProps={menuProps} openSettings={openSettings} onOpenSettings={onOpenSettings} />
    </div>
  )
}

function CategoryFrameTop({ category, menuProps, openSettings, onOpenSettings }: { category: CategoryTab; menuProps: SettingsMenuProps; openSettings: boolean; onOpenSettings: () => void }) {
  const Icon = category.icon
  return (
    <div className="no-drag flex h-12 shrink-0 items-center justify-between gap-2 border-b border-white/10 px-3" data-drop-zone={category.id}>
      <button type="button" className="flex min-w-0 items-center gap-2">
        <Icon className="size-5 shrink-0" weight="duotone" style={{ color: category.color }} />
        <span className="truncate text-sm font-semibold">{category.label}</span>
        <Badge className="bg-white/10 text-white hover:bg-white/10">{category.count}</Badge>
      </button>
      <FrameActions kind="category" menuProps={menuProps} openSettings={openSettings} onOpenSettings={onOpenSettings} />
    </div>
  )
}

function LauncherTop({
  count,
  menuProps,
  openSettings,
  onStartAll,
  onOpenSettings,
}: {
  count: number
  menuProps: SettingsMenuProps
  openSettings: boolean
  onStartAll: () => Promise<void>
  onOpenSettings: () => void
}) {
  return (
    <div className="no-drag flex h-12 shrink-0 items-center justify-end gap-2 border-b border-white/10 px-3" data-drop-zone="launcher">
      <Badge className="mr-auto bg-white/10 text-white hover:bg-white/10">{count}</Badge>
      <Button size="xs" onClick={() => void onStartAll()}>
        <RocketLaunch className="size-3.5" weight="duotone" />
        启动
      </Button>
      <FrameActions kind="launcher" menuProps={menuProps} openSettings={openSettings} onOpenSettings={onOpenSettings} />
    </div>
  )
}

function FrameActions({
  kind,
  menuProps,
  openSettings,
  onOpenSettings,
}: {
  kind: "organizer" | "category" | "launcher"
  menuProps: SettingsMenuProps
  openSettings: boolean
  onOpenSettings: () => void
}) {
  return (
    <div className="relative flex shrink-0 items-center gap-1">
      <Button size="icon-sm" variant="secondary" title="设置" aria-label="设置" onClick={onOpenSettings}>
        <GearSix className="size-4" weight="duotone" />
      </Button>
      {openSettings ? <SettingsMenu kind={kind} {...menuProps} /> : null}
    </div>
  )
}

function SettingsMenu({
  kind,
  settings,
  viewMode,
  desktopWindowState,
  desktopWindowStatePending,
  createCategory,
  renameCategory,
  deleteCategory,
  updateSettings,
  updateViewMode,
  onToggleDesktopWindowLocked,
  onToggleDesktopWindowClickThrough,
  onSplitCategory,
  onSplitAllCategories,
  onMergeAllCategories,
  onClassifyDesktop,
  onOrganizeAndSplitAll,
  onRestoreAllToDesktop,
  isClassifyingDesktop,
  isRestoringDesktop,
  isMergingCategories,
  onRefresh,
  onHide,
}: SettingsMenuProps & { kind: "organizer" | "category" | "launcher" }) {
  return (
    <div
      className="desktop-widget-scroll absolute right-0 top-8 z-50 max-h-[min(440px,calc(100vh-4rem))] w-64 max-w-[calc(100vw-1rem)] space-y-1.5 overflow-x-hidden overflow-y-auto overscroll-contain rounded-2xl border border-white/15 bg-slate-950/90 p-1.5 text-white shadow-2xl shadow-black/35 backdrop-blur-2xl"
      aria-label="桌面框设置"
    >
      {kind !== "launcher" ? (
        <>
          <SettingsMenuSection title="分类管理">
            <MenuButton icon={Plus} label="新增分类" onClick={() => void createCategory()} />
            <MenuButton icon={PencilSimple} label="重命名当前分类" onClick={() => void renameCategory()} />
            <MenuButton icon={Trash} label="删除当前分类" onClick={() => void deleteCategory()} />
          </SettingsMenuSection>
          <SettingsMenuSection title="桌面框布局">
            <MenuButton icon={Columns} label="拆出当前分类" onClick={() => void onSplitCategory()} />
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
        <MenuButton icon={X} label="隐藏桌面框" onClick={() => void onHide()} />
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

function CategoryScroller({
  categories,
  activeId,
  hoverZone,
  onActiveChange,
  onSplitCategory,
  onReorderCategory,
}: {
  categories: CategoryTab[]
  activeId: string
  hoverZone: string
  onActiveChange: (id: string) => void
  onSplitCategory: (index: number) => Promise<void>
  onReorderCategory: (fromIndex: number, toIndex: number) => Promise<void>
}) {
  const scrollerRef = useRef<HTMLDivElement>(null)
  const pressRef = useRef<{
    pointerId: number
    sourceIndex: number
    startX: number
    startY: number
    active: boolean
  } | null>(null)
  const dropTargetRef = useRef<number | null>(null)
  const lastPointerXRef = useRef(0)
  const autoScrollFrameRef = useRef<number | null>(null)
  const suppressClickUntilRef = useRef(0)
  const [draggingIndex, setDraggingIndex] = useState<number | null>(null)
  const [dropTargetIndex, setDropTargetIndex] = useState<number | null>(null)
  const [isSavingOrder, setIsSavingOrder] = useState(false)

  useEffect(() => {
    return () => {
      if (autoScrollFrameRef.current !== null) window.cancelAnimationFrame(autoScrollFrameRef.current)
    }
  }, [])

  function handleWheel(event: ReactWheelEvent<HTMLDivElement>) {
    if (Math.abs(event.deltaY) <= Math.abs(event.deltaX)) return
    event.currentTarget.scrollLeft += event.deltaY
    event.preventDefault()
  }

  function handleCategoryPointerDown(event: ReactPointerEvent<HTMLButtonElement>, sourceIndex: number) {
    event.stopPropagation()
    if (event.button !== 0 || !event.isPrimary || categories.length < 2 || isSavingOrder) return

    cancelCategoryPress()
    pressRef.current = {
      pointerId: event.pointerId,
      sourceIndex,
      startX: event.clientX,
      startY: event.clientY,
      active: false,
    }
    lastPointerXRef.current = event.clientX
    event.currentTarget.setPointerCapture(event.pointerId)
  }

  function handleCategoryPointerMove(event: ReactPointerEvent<HTMLButtonElement>) {
    const press = pressRef.current
    if (!press || press.pointerId !== event.pointerId) return
    event.stopPropagation()
    lastPointerXRef.current = event.clientX

    if (!press.active && Math.abs(event.clientX - press.startX) >= 6 && Math.abs(event.clientX - press.startX) > Math.abs(event.clientY - press.startY)) {
      press.active = true
      dropTargetRef.current = press.sourceIndex
      setDraggingIndex(press.sourceIndex)
      setDropTargetIndex(press.sourceIndex)
      suppressClickUntilRef.current = Date.now() + 600
      scheduleCategoryAutoScroll()
    }
    if (!press.active) {
      return
    }

    event.preventDefault()
    updateCategoryDropTarget(event.clientX)
    scheduleCategoryAutoScroll()
  }

  function handleCategoryPointerUp(event: ReactPointerEvent<HTMLButtonElement>) {
    const press = pressRef.current
    if (!press || press.pointerId !== event.pointerId) return
    event.stopPropagation()
    if (press.active) event.preventDefault()

    const sourceIndex = press.sourceIndex
    const targetIndex = dropTargetRef.current
    const wasActive = press.active
    releaseCategoryPointer(event.currentTarget, event.pointerId)
    cancelCategoryPress()
    if (!wasActive || targetIndex === null || sourceIndex === targetIndex) return

    suppressClickUntilRef.current = Date.now() + 600
    setIsSavingOrder(true)
    void onReorderCategory(sourceIndex, targetIndex).finally(() => setIsSavingOrder(false))
  }

  function handleCategoryPointerCancel(event: ReactPointerEvent<HTMLButtonElement>) {
    const press = pressRef.current
    if (!press || press.pointerId !== event.pointerId) return
    event.stopPropagation()
    releaseCategoryPointer(event.currentTarget, event.pointerId)
    cancelCategoryPress()
  }

  function updateCategoryDropTarget(clientX: number) {
    const scroller = scrollerRef.current
    if (!scroller) return
    const targetIndex = categoryIndexFromHorizontalPoint(scroller, clientX)
    if (targetIndex === null || targetIndex === dropTargetRef.current) return
    dropTargetRef.current = targetIndex
    setDropTargetIndex(targetIndex)
  }

  function scheduleCategoryAutoScroll() {
    if (autoScrollFrameRef.current !== null) return
    const tick = () => {
      autoScrollFrameRef.current = null
      const press = pressRef.current
      const scroller = scrollerRef.current
      if (!press?.active || !scroller) return

      const bounds = scroller.getBoundingClientRect()
      const edgeSize = Math.min(64, bounds.width * 0.18)
      const pointerX = lastPointerXRef.current
      let delta = 0
      if (pointerX < bounds.left + edgeSize) {
        delta = -Math.ceil(3 + ((bounds.left + edgeSize - pointerX) / edgeSize) * 9)
      } else if (pointerX > bounds.right - edgeSize) {
        delta = Math.ceil(3 + ((pointerX - (bounds.right - edgeSize)) / edgeSize) * 9)
      }

      if (delta !== 0) {
        const previousScrollLeft = scroller.scrollLeft
        scroller.scrollLeft += delta
        updateCategoryDropTarget(pointerX)
        if (scroller.scrollLeft !== previousScrollLeft) {
          autoScrollFrameRef.current = window.requestAnimationFrame(tick)
        }
      }
    }
    autoScrollFrameRef.current = window.requestAnimationFrame(tick)
  }

  function cancelCategoryPress() {
    pressRef.current = null
    dropTargetRef.current = null
    setDraggingIndex(null)
    setDropTargetIndex(null)
    if (autoScrollFrameRef.current !== null) {
      window.cancelAnimationFrame(autoScrollFrameRef.current)
      autoScrollFrameRef.current = null
    }
  }

  return (
    <div
      ref={scrollerRef}
      className={cn("desktop-widget-tabs no-drag flex min-w-0 flex-1 gap-2 overflow-x-auto py-1 select-none", draggingIndex !== null && "cursor-grabbing")}
      onPointerDown={(event) => event.stopPropagation()}
      onWheel={handleWheel}
    >
      {categories.map((category) => {
        const Icon = category.icon
        const isDragging = draggingIndex === category.index
        const isDropTarget = dropTargetIndex === category.index && draggingIndex !== null && !isDragging
        return (
          <div
            key={category.id}
            data-drop-zone={category.id}
            data-category-order-index={category.index}
            className={cn(
              "relative inline-flex shrink-0 touch-none items-center rounded-xl text-sm font-semibold text-white/75 transition-[opacity,transform,background-color,box-shadow]",
              activeId === category.id && "bg-white/15 text-white ring-1 ring-white/20",
              hoverZone === category.id && "bg-emerald-300/25 text-white ring-1 ring-emerald-200/50",
              isDragging && "scale-[0.97] cursor-grabbing bg-white/20 opacity-45 ring-1 ring-emerald-200/60",
            )}
          >
            {isDropTarget ? (
              <span
                className={cn(
                  "pointer-events-none absolute inset-y-1 z-20 w-0.5 rounded-full bg-emerald-200 shadow-[0_0_10px_rgba(110,231,183,0.9)]",
                  draggingIndex < category.index ? "-right-1.5" : "-left-1.5",
                )}
              />
            ) : null}
            <button
              type="button"
              className={cn("inline-flex min-w-0 items-center gap-1.5 rounded-l-xl px-2.5 py-1.5", isDragging ? "cursor-grabbing" : "cursor-default")}
              title="点击切换分类，按住后左右拖动调整顺序"
              onPointerDown={(event) => handleCategoryPointerDown(event, category.index)}
              onPointerMove={handleCategoryPointerMove}
              onPointerUp={handleCategoryPointerUp}
              onPointerCancel={handleCategoryPointerCancel}
              onClick={(event) => {
                if (Date.now() < suppressClickUntilRef.current) {
                  event.preventDefault()
                  event.stopPropagation()
                  return
                }
                onActiveChange(category.id)
              }}
            >
              <Icon className="size-4 shrink-0" weight="duotone" style={{ color: category.color }} />
              <span className="max-w-24 truncate">{category.label}</span>
              <span className="rounded-full bg-white/10 px-1.5 text-[10px] text-white/60">{category.count}</span>
            </button>
            <button
              type="button"
              className="grid size-8 place-items-center rounded-r-xl text-white/45 transition hover:bg-white/10 hover:text-white"
              title={`拆出 ${category.label}`}
              onPointerDown={(event) => event.stopPropagation()}
              onClick={() => void onSplitCategory(category.index)}
            >
              <Columns className="size-3.5" weight="duotone" />
            </button>
          </div>
        )
      })}
    </div>
  )
}

function LayoutModeRow({ viewMode, updateViewMode }: Pick<SettingsMenuProps, "viewMode" | "updateViewMode">) {
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

function categoryIndexFromHorizontalPoint(container: HTMLElement, clientX: number) {
  const categories = Array.from(container.querySelectorAll<HTMLElement>("[data-category-order-index]"))
  if (categories.length === 0) return null

  let closestIndex: number | null = null
  let closestDistance = Number.POSITIVE_INFINITY
  for (const category of categories) {
    const index = Number(category.dataset.categoryOrderIndex)
    if (!Number.isInteger(index)) continue
    const bounds = category.getBoundingClientRect()
    const distance = Math.abs(clientX - (bounds.left + bounds.right) / 2)
    if (distance < closestDistance) {
      closestIndex = index
      closestDistance = distance
    }
  }
  return closestIndex
}

function releaseCategoryPointer(element: HTMLElement, pointerId: number) {
  if (!element.hasPointerCapture(pointerId)) return
  element.releasePointerCapture(pointerId)
}

function CategoryItems({
  category,
  categoryIndex,
  settings,
  onOpen,
  onShowInFolder,
  onRestoreToDesktop,
  onRestoreDragOut,
}: {
  category?: { name: string; item_details: DesktopItem[] }
  categoryIndex: number
  settings: WidgetItemSettings
  onOpen: (path: string) => Promise<void>
  onShowInFolder: (path: string) => Promise<void>
  onRestoreToDesktop: (index: number, path: string) => Promise<string>
  onRestoreDragOut: (index: number, path: string, position: DesktopDropPosition) => Promise<void>
}) {
  const items = category?.item_details ?? []
  if (items.length === 0) {
    return <EmptyDropHint title="暂无项目" detail="把桌面文件拖到这里，会自动收纳进这个分类。" />
  }

  return (
    <div className="desktop-widget-scroll h-full overflow-auto">
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
            onDragEndOutside={(position) => onRestoreDragOut(categoryIndex, item.path, position)}
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
  onStartAll,
  onShowInFolder,
  onRemoveLauncher,
}: {
  launchers: { name: string; path: string; icon_data_url?: string }[]
  settings: WidgetItemSettings
  onOpen: (path: string) => Promise<void>
  onStartAll: () => Promise<void>
  onShowInFolder: (path: string) => Promise<void>
  onRemoveLauncher: (path: string) => Promise<void>
}) {
  if (launchers.length === 0) {
    return <EmptyDropHint title="暂无启动项" detail="把快捷方式、程序或常用文件拖到这里。" />
  }

  return (
    <div className="flex h-full min-h-0 flex-col">
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
      <button className="sr-only" type="button" onClick={() => void onStartAll()}>
        启动全部
      </button>
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
        void Promise.resolve(onDragEndOutside(desktopDropPositionFromDragEnd(event))).catch(() => undefined)
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
    <div className="grid h-full min-h-[140px] place-items-center px-6 text-center">
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

function layoutFor(id: string, index: number, layouts: Record<string, CardLayout>) {
  const existing = layouts[id]
  if (existing) return existing
  if (id === "organizer") return { x: 8, y: 8, width: 560, height: 330 }
  if (id === "launcher") return { x: 584, y: 8, width: 300, height: 330 }
  const safeIndex = Math.max(0, index)
  return {
    x: 8 + (safeIndex % 2) * 304,
    y: 8 + Math.floor(safeIndex / 2) * 226,
    width: 288,
    height: 210,
  }
}

function frameIndex(id: string, categories: CategoryTab[]) {
  if (id === "organizer") return 0
  if (id === "launcher") return 1
  return Math.max(
    0,
    categories.findIndex((category) => category.id === id),
  )
}

function dropZoneFromPoint(physicalX: number, physicalY: number) {
  const ratio = globalThis.devicePixelRatio || 1
  return dropZoneFromClientPoint(physicalX / ratio, physicalY / ratio)
}

function dropZoneFromClientPoint(clientX: number, clientY: number) {
  const element = document.elementFromPoint(clientX, clientY)
  return element?.closest<HTMLElement>("[data-drop-zone]")?.dataset.dropZone ?? ""
}

function parseDropTarget(value: string): DropTarget | null {
  if (value === "launcher") return { type: "launcher" }
  if (!value.startsWith("category:")) return null
  const index = Number(value.slice("category:".length))
  if (!Number.isFinite(index) || index < 0) return null
  return { type: "category", index }
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

function remapCategoryIdAfterMove(id: string, fromIndex: number, toIndex: number) {
  const target = parseDropTarget(id)
  if (!target || target.type !== "category") return id
  return `category:${remapIndexAfterMove(target.index, fromIndex, toIndex)}`
}

function countNotice(action: string, count: number, total: number, empty: string) {
  if (count <= 0) return empty
  const skipped = Math.max(0, total - count)
  return `${action} ${count} 项${skipped ? `，跳过 ${skipped} 项` : ""}`
}

function readLayouts(): Record<string, CardLayout> {
  try {
    const parsed = JSON.parse(globalThis.localStorage.getItem(layoutsStorageKey) || "{}") as Record<string, CardLayout>
    return parsed && typeof parsed === "object" ? parsed : {}
  } catch {
    return {}
  }
}

function writeSplitCategoryIndices(indices: number[]) {
  globalThis.localStorage.setItem(splitCategoriesStorageKey, JSON.stringify(normalizeSplitCategoryIndices(indices, Number.MAX_SAFE_INTEGER)))
}

function normalizeSplitCategoryIndices(value: unknown, maxLength: number) {
  if (!Array.isArray(value)) return []
  return [...new Set(value.map(Number).filter((index) => Number.isInteger(index) && index >= 0 && index < maxLength))].sort((left, right) => left - right)
}

function sameNumberList(left: number[], right: number[]) {
  return left.length === right.length && left.every((value, index) => value === right[index])
}

function clamp(value: number, min: number, max: number) {
  if (!Number.isFinite(value)) return min
  return Math.min(max, Math.max(min, value))
}

function categoryVisual(name: string, index: number): Pick<CategoryTab, "icon" | "color" | "glow"> {
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
