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
  Desktop,
  Eye,
  EyeSlash,
  FileText,
  FolderOpen,
  GameController,
  GearSix,
  GlobeHemisphereWest,
  HardDrives,
  PencilSimple,
  Plus,
  RocketLaunch,
  Trash,
  UsersThree,
  Wrench,
  X,
  type Icon,
} from "@phosphor-icons/react"
import { FileIcon } from "@/components/dustdesk/file-icon"
import { ItemContextMenu, type ItemContextMenuAction } from "@/components/dustdesk/item-context-menu"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { usePersistCurrentWindowLayout } from "@/hooks/use-persist-current-window-layout"
import { useTheme } from "@/hooks/use-theme"
import {
  allowPathLikeDrag,
  desktopDropPositionFromDragEnd,
  didDragEndOutsideWindow,
  readDustDeskPathDrag,
  type DesktopDropPosition,
  writeDustDeskPathDrag,
} from "@/lib/dustdesk-dnd"
import { repaintCurrentWindow, safeCurrentWebviewDragDropEvent, safeListen, startCurrentWindowDragging, startCurrentWindowResizeDragging } from "@/lib/tauri-window"
import { cn, displayPathName, extensionFromPath } from "@/lib/utils"
import { useDustDeskStore } from "@/stores/dustdesk-store"
import type { DesktopItem, DesktopOperationEvent } from "@/types"

const activeCategoryKey = "dustdesk-desktop-widget-active-category"
const settingsStorageKey = "dustdesk-desktop-widget-settings"
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

interface WidgetSettings {
  opacity: number
  iconSize: number
  showNames: boolean
}

interface CardLayout {
  x: number
  y: number
  width: number
  height: number
}

const defaultSettings: WidgetSettings = {
  opacity: 0.5,
  iconSize: 44,
  showNames: true,
}

const launcherVisual = {
  icon: RocketLaunch,
  color: "#fb923c",
  glow: "rgba(251, 146, 60, 0.28)",
}

export function DesktopWidgetPage() {
  useTheme()
  usePersistCurrentWindowLayout("desktop-widget")
  const snapshot = useDustDeskStore((state) => state.snapshot)
  const loadDesktopSnapshot = useDustDeskStore((state) => state.loadDesktopSnapshot)
  const createCategory = useDustDeskStore((state) => state.createCategory)
  const renameCategory = useDustDeskStore((state) => state.renameCategory)
  const deleteCategory = useDustDeskStore((state) => state.deleteCategory)
  const selectCategory = useDustDeskStore((state) => state.selectCategory)
  const openPath = useDustDeskStore((state) => state.openPath)
  const addItemsToCategoryLight = useDustDeskStore((state) => state.addItemsToCategoryLight)
  const addLaunchersLight = useDustDeskStore((state) => state.addLaunchersLight)
  const removeLauncher = useDustDeskStore((state) => state.removeLauncher)
  const restoreItemToDesktopLight = useDustDeskStore((state) => state.restoreItemToDesktopLight)
  const startRestoreAllToDesktopTask = useDustDeskStore((state) => state.startRestoreAllToDesktopTask)
  const showPathInFolder = useDustDeskStore((state) => state.showPathInFolder)
  const startClassifyDesktopItemsTask = useDustDeskStore((state) => state.startClassifyDesktopItemsTask)
  const startAllLaunchers = useDustDeskStore((state) => state.startAllLaunchers)
  const splitDesktopWidgets = useDustDeskStore((state) => state.splitDesktopWidgets)
  const splitDesktopCategory = useDustDeskStore((state) => state.splitDesktopCategory)
  const mergeDesktopWidgets = useDustDeskStore((state) => state.mergeDesktopWidgets)
  const saveDesktopSplitIndices = useDustDeskStore((state) => state.saveDesktopSplitIndices)
  const hideCurrentWindow = useDustDeskStore((state) => state.hideCurrentWindow)
  const [activeCategoryId, setActiveCategoryId] = useState(() => globalThis.localStorage.getItem(activeCategoryKey) || "category:0")
  const [splitCategoryIndices, setSplitCategoryIndices] = useState<number[]>([])
  const [settings, setSettings] = useState<WidgetSettings>(readSettings)
  const [layouts, setLayouts] = useState<Record<string, CardLayout>>(readLayouts)
  const [openSettingsId, setOpenSettingsId] = useState("")
  const [hoverZone, setHoverZone] = useState("")
  const [notice, setNotice] = useState("")
  const [isClassifyingDesktop, setIsClassifyingDesktop] = useState(false)
  const [isRestoringDesktop, setIsRestoringDesktop] = useState(false)
  const [isMergingCategories, setIsMergingCategories] = useState(false)
  const desktopOperationLabel = isClassifyingDesktop
    ? notice || "正在智能收纳桌面..."
    : isRestoringDesktop
      ? notice || "正在还原桌面..."
      : isMergingCategories
        ? "正在合并分类..."
        : ""
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
    void safeListen<DesktopOperationEvent>("dustdesk://desktop-operation", (event) => {
      const payload = event.payload
      if (payload.status === "started") {
        setNotice(payload.message)
        if (payload.kind === "classify") {
          setIsClassifyingDesktop(true)
        } else if (payload.kind === "restore") {
          setIsRestoringDesktop(true)
        }
        return
      }
      if (payload.status === "progress") {
        setNotice(payload.message)
        if (payload.kind === "restore") {
          setIsRestoringDesktop(true)
        }
        return
      }
      if (payload.kind === "classify") {
        void finishClassifyOperation(payload)
      } else if (payload.kind === "restore") {
        void finishRestoreOperation(payload)
      }
    }).then((value) => {
      unlisten = value
    })
    return () => unlisten?.()
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
    globalThis.localStorage.setItem(settingsStorageKey, JSON.stringify(settings))
  }, [settings])

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
        const added = await addLaunchersLight(paths)
        setNotice(countNotice("已加入快捷启动", added, paths.length, "没有新增启动项"))
      } else {
        const added = await addItemsToCategoryLight(target.index, paths)
        setActiveCategoryId(`category:${target.index}`)
        setNotice(countNotice(`已收纳到「${snapshot.categories[target.index]?.name ?? "分类"}」`, added, paths.length, "没有新增收纳项目"))
      }
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error))
    }
  }

  async function handleRestoreDragOut(index: number, path: string, position: DesktopDropPosition) {
    try {
      const restored = await restoreItemToDesktopLight(index, path, position)
      setNotice(`已移回桌面：${displayPathName(restored)}`)
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error))
    }
  }

  async function handleStartAll() {
    const count = await startAllLaunchers()
    setNotice(count > 0 ? `已启动 ${count} 项` : "快捷启动框还是空的")
  }

  async function handleClassifyDesktopItems() {
    if (isClassifyingDesktop) return
    pendingClassifyActionRef.current = null
    setIsClassifyingDesktop(true)
    setNotice("正在智能收纳桌面...")
    try {
      setOpenSettingsId("")
      await startClassifyDesktopItemsTask()
    } catch (error) {
      pendingClassifyActionRef.current = null
      setNotice(error instanceof Error ? error.message : String(error))
      setIsClassifyingDesktop(false)
    }
  }

  async function handleOrganizeAndSplitAll() {
    if (isClassifyingDesktop) return
    const previous = splitCategoryIndices
    pendingClassifyActionRef.current = "split-all"
    previousSplitCategoryIndicesRef.current = previous
    setIsClassifyingDesktop(true)
    setNotice("正在智能收纳并拆分...")
    try {
      setOpenSettingsId("")
      await startClassifyDesktopItemsTask()
    } catch (error) {
      pendingClassifyActionRef.current = null
      writeSplitCategoryIndices(previous)
      setSplitCategoryIndices(previous)
      setNotice(error instanceof Error ? error.message : String(error))
      setIsClassifyingDesktop(false)
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

    setIsRestoringDesktop(true)
    setNotice("正在还原桌面...")
    try {
      setOpenSettingsId("")
      await startRestoreAllToDesktopTask()
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error))
      setIsRestoringDesktop(false)
    }
  }

  async function finishClassifyOperation(payload: DesktopOperationEvent) {
    if (payload.status === "failed") {
      pendingClassifyActionRef.current = null
      setNotice(payload.message || "智能收纳失败")
      setIsClassifyingDesktop(false)
      return
    }

    await loadDesktopSnapshot({ force: true })
    const result = classifyResultFromOperation(payload)
    if (pendingClassifyActionRef.current === "split-all") {
      pendingClassifyActionRef.current = null
      try {
        const next = await splitDesktopWidgets()
        writeSplitCategoryIndices(next)
        setSplitCategoryIndices(next)
        setNotice(next.length > 0 ? `${classifyResultNotice(result)}，已拆出 ${next.length} 个分类` : `${classifyResultNotice(result)}，没有可拆出的分类内容`)
      } catch (error) {
        const previous = previousSplitCategoryIndicesRef.current
        writeSplitCategoryIndices(previous)
        setSplitCategoryIndices(previous)
        setNotice(error instanceof Error ? error.message : String(error))
      } finally {
        setIsClassifyingDesktop(false)
      }
      return
    }

    setNotice(classifyResultNotice(result))
    setIsClassifyingDesktop(false)
  }

  async function finishRestoreOperation(payload: DesktopOperationEvent) {
    if (payload.status === "failed") {
      setNotice(payload.message || "还原桌面失败")
      setIsRestoringDesktop(false)
      return
    }

    await loadDesktopSnapshot({ force: true })
    writeSplitCategoryIndices([])
    await saveDesktopSplitIndices([])
    setSplitCategoryIndices([])
    setActiveCategoryId("category:0")
    setNotice(payload.message || (payload.restored > 0 ? `已还原 ${payload.restored} 项到桌面` : "没有需要还原到桌面的收纳项目"))
    setIsRestoringDesktop(false)
  }

  function updateSettings(next: Partial<WidgetSettings>) {
    setSettings((value) => ({ ...value, ...next }))
  }

  function startDrag(id: string, event: ReactPointerEvent<HTMLElement>) {
    if ((event.target as HTMLElement).closest("button,input")) return
    dragRef.current = {
      id,
      x: event.clientX,
      y: event.clientY,
      rect: layoutFor(id, frameIndex(id, categories), layouts),
    }
  }

  function startResize(id: string, event: ReactPointerEvent<HTMLButtonElement>) {
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
    createCategory: handleCreateCategory,
    renameCategory: handleRenameActiveCategory,
    deleteCategory: handleDeleteActiveCategory,
    updateSettings,
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
    backgroundColor: `rgb(15 23 42 / ${settings.opacity})`,
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
          className="cursor-move"
          onPointerDown={(event) => {
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
          />
        </div>
        {activeCategory ? (
          <CategoryItems
            category={snapshot.categories[activeCategory.index]}
            categoryIndex={activeCategory.index}
            settings={settings}
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
        <div className="pointer-events-none absolute bottom-3 left-1/2 -translate-x-1/2 rounded-full bg-slate-950/70 px-3 py-1 text-xs text-white/80 ring-1 ring-white/10">
          {notice}
        </div>
      ) : null}
      <button
        type="button"
        className="no-drag absolute bottom-0 right-0 size-6 cursor-nwse-resize rounded-br-2xl border-b-2 border-r-2 border-white/35"
        aria-label="调整桌面框窗口大小"
        onPointerDown={(event) => {
          event.preventDefault()
          void startCurrentWindowResizeDragging("SouthEast")
        }}
      />
    </div>
  )
}

function WidgetOperationOverlay({ label }: { label: string }) {
  return (
    <div className="no-drag absolute inset-0 z-40 grid place-items-center bg-slate-950/60 backdrop-blur-md">
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
    backgroundColor: `rgb(15 23 42 / ${settings.opacity})`,
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
  createCategory: () => Promise<void>
  renameCategory: () => Promise<void>
  deleteCategory: () => Promise<void>
  updateSettings: (settings: Partial<WidgetSettings>) => void
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
}: {
  categories: CategoryTab[]
  activeId: string
  hoverZone: string
  menuProps: SettingsMenuProps
  openSettings: boolean
  onActiveChange: (id: string) => void
  onOpenSettings: () => void
  onSplitCategory: (index: number) => Promise<void>
}) {
  return (
    <div className="no-drag flex h-12 shrink-0 items-center gap-2 border-b border-white/10 px-2">
      <CategoryScroller categories={categories} activeId={activeId} hoverZone={hoverZone} onActiveChange={onActiveChange} onSplitCategory={onSplitCategory} />
      <FrameActions menuProps={menuProps} openSettings={openSettings} onOpenSettings={onOpenSettings} />
    </div>
  )
}

function CategoryFrameTop({
  category,
  menuProps,
  openSettings,
  onOpenSettings,
}: {
  category: CategoryTab
  menuProps: SettingsMenuProps
  openSettings: boolean
  onOpenSettings: () => void
}) {
  const Icon = category.icon
  return (
    <div className="no-drag flex h-12 shrink-0 items-center justify-between gap-2 border-b border-white/10 px-3" data-drop-zone={category.id}>
      <button type="button" className="flex min-w-0 items-center gap-2">
        <Icon className="size-5 shrink-0" weight="duotone" style={{ color: category.color }} />
        <span className="truncate text-sm font-semibold">{category.label}</span>
        <Badge className="bg-white/10 text-white hover:bg-white/10">{category.count}</Badge>
      </button>
      <FrameActions menuProps={menuProps} openSettings={openSettings} onOpenSettings={onOpenSettings} />
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
      <FrameActions menuProps={menuProps} openSettings={openSettings} onOpenSettings={onOpenSettings} />
    </div>
  )
}

function FrameActions({ menuProps, openSettings, onOpenSettings }: { menuProps: SettingsMenuProps; openSettings: boolean; onOpenSettings: () => void }) {
  return (
    <div className="relative shrink-0">
      <Button size="icon-sm" variant="secondary" onClick={onOpenSettings}>
        <GearSix className="size-4" weight="duotone" />
      </Button>
      {openSettings ? <SettingsMenu {...menuProps} /> : null}
    </div>
  )
}

function SettingsMenu({
  settings,
  createCategory,
  renameCategory,
  deleteCategory,
  updateSettings,
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
}: SettingsMenuProps) {
  return (
    <div className="desktop-widget-scroll absolute right-0 top-8 z-50 max-h-[min(64vh,260px)] w-48 overflow-y-auto rounded-xl border border-white/15 bg-slate-950/85 p-1 text-white shadow-2xl shadow-black/30 backdrop-blur-2xl">
      <MenuButton icon={Columns} label={isMergingCategories ? "合并中" : "一键合并分类"} disabled={isMergingCategories} onClick={() => void onMergeAllCategories()} />
      <MenuButton icon={Columns} label="拆出当前分类" onClick={() => void onSplitCategory()} />
      <MenuButton icon={Columns} label="拆分全部分类" onClick={() => void onSplitAllCategories()} />
      <MenuButton icon={Columns} label={isClassifyingDesktop ? "智能收纳中" : "智能收纳并拆分全部"} disabled={isClassifyingDesktop} onClick={() => void onOrganizeAndSplitAll()} />
      <MenuButton icon={Plus} label="新增分类" onClick={() => void createCategory()} />
      <MenuButton icon={PencilSimple} label="重命名当前分类" onClick={() => void renameCategory()} />
      <MenuButton icon={Trash} label="删除当前分类" onClick={() => void deleteCategory()} />
      <MenuButton icon={Archive} label={isClassifyingDesktop ? "智能收纳中" : "智能收纳桌面"} disabled={isClassifyingDesktop} onClick={() => void onClassifyDesktop()} />
      <MenuButton icon={Desktop} label={isRestoringDesktop ? "还原中" : "一键还原桌面"} disabled={isRestoringDesktop} onClick={() => void onRestoreAllToDesktop()} />
      <MenuButton icon={ArrowsClockwise} label="刷新" onClick={() => void onRefresh()} />
      <div className="my-1 h-px bg-white/10" />
      <RangeRow label="卡片透明度" min={0.25} max={0.85} step={0.05} value={settings.opacity} onChange={(opacity) => updateSettings({ opacity })} />
      <RangeRow label="项目大小" min={34} max={74} step={4} value={settings.iconSize} onChange={(iconSize) => updateSettings({ iconSize })} />
      <MenuButton
        icon={settings.showNames ? Eye : EyeSlash}
        label={settings.showNames ? "隐藏名称" : "显示名称"}
        onClick={() => updateSettings({ showNames: !settings.showNames })}
      />
      <div className="my-1 h-px bg-white/10" />
      <MenuButton icon={X} label="隐藏桌面框" onClick={() => void onHide()} />
    </div>
  )
}

function MenuButton({ icon: Icon, label, disabled, onClick }: { icon: Icon; label: string; disabled?: boolean; onClick: () => void }) {
  return (
    <button
      type="button"
      disabled={disabled}
      className="flex w-full items-center gap-1.5 rounded-lg px-2 py-1 text-left text-[11px] font-semibold text-white/80 transition hover:bg-white/10 hover:text-white disabled:cursor-wait disabled:opacity-55"
      onClick={onClick}
    >
      <Icon className="size-3.5" weight="duotone" />
      <span>{label}</span>
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
}: {
  categories: CategoryTab[]
  activeId: string
  hoverZone: string
  onActiveChange: (id: string) => void
  onSplitCategory: (index: number) => Promise<void>
}) {
  function handleWheel(event: ReactWheelEvent<HTMLDivElement>) {
    if (Math.abs(event.deltaY) <= Math.abs(event.deltaX)) return
    event.currentTarget.scrollLeft += event.deltaY
    event.preventDefault()
  }

  return (
    <div className="desktop-widget-tabs no-drag flex min-w-0 flex-1 gap-2 overflow-x-auto py-1" onWheel={handleWheel}>
      {categories.map((category) => {
        const Icon = category.icon
        return (
          <div
            key={category.id}
            data-drop-zone={category.id}
            className={cn(
              "inline-flex shrink-0 items-center rounded-xl text-sm font-semibold text-white/75 transition",
              activeId === category.id && "bg-white/15 text-white ring-1 ring-white/20",
              hoverZone === category.id && "bg-emerald-300/25 text-white ring-1 ring-emerald-200/50",
            )}
          >
            <button type="button" className="inline-flex min-w-0 items-center gap-1.5 rounded-l-xl px-2.5 py-1.5" title="点击切换分类" onClick={() => onActiveChange(category.id)}>
              <Icon className="size-4 shrink-0" weight="duotone" style={{ color: category.color }} />
              <span className="max-w-24 truncate">{category.label}</span>
              <span className="rounded-full bg-white/10 px-1.5 text-[10px] text-white/60">{category.count}</span>
            </button>
            <button
              type="button"
              className="grid size-8 place-items-center rounded-r-xl text-white/45 transition hover:bg-white/10 hover:text-white"
              title={`拆出 ${category.label}`}
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
  settings: WidgetSettings
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
  settings: WidgetSettings
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

function WidgetGrid({ settings, children }: { settings: WidgetSettings; children: ReactNode }) {
  const style = {
    "--widget-item-size": `${settings.iconSize}px`,
    gridTemplateColumns: `repeat(auto-fill, minmax(${Math.max(76, settings.iconSize + 48)}px, 1fr))`,
  } as CSSProperties

  return (
    <div className="grid gap-2 p-3 pb-6" style={style}>
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
  settings: WidgetSettings
  onOpen: (path: string) => Promise<void>
  onDragEndOutside?: (position: DesktopDropPosition) => unknown
  actions?: ItemContextMenuAction[]
}) {
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null)

  return (
    <button
      type="button"
      title={path}
      draggable={Boolean(dragPath)}
      className="flex min-w-0 flex-col items-center gap-2 rounded-xl border border-white/10 bg-white/10 p-2 text-center text-white/90 transition hover:border-white/25 hover:bg-white/15"
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
      <FileIcon name={name} extension={extension} isDir={isDir} iconDataUrl={iconDataUrl} className="widget-item-icon bg-white/10 text-white/70" />
      {settings.showNames ? <span className="w-full truncate text-xs font-semibold">{name}</span> : null}
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

function countNotice(action: string, count: number, total: number, empty: string) {
  if (count <= 0) return empty
  const skipped = Math.max(0, total - count)
  return `${action} ${count} 项${skipped ? `，跳过 ${skipped} 项` : ""}`
}

function readSettings(): WidgetSettings {
  try {
    const parsed = JSON.parse(globalThis.localStorage.getItem(settingsStorageKey) || "{}") as Partial<WidgetSettings>
    return {
      opacity: clamp(Number(parsed.opacity ?? defaultSettings.opacity), 0.25, 0.85),
      iconSize: clamp(Number(parsed.iconSize ?? defaultSettings.iconSize), 34, 74),
      showNames: typeof parsed.showNames === "boolean" ? parsed.showNames : defaultSettings.showNames,
    }
  } catch {
    return defaultSettings
  }
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
