import { useEffect, useMemo, useRef, useState, type CSSProperties, type DragEvent as ReactDragEvent, type ReactNode } from "react"
import { useParams } from "react-router"
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
import { allowPathLikeDrag, didDragEndOutsideWindow, hasDustDeskPathDrag, readDustDeskPathDrag, writeDustDeskPathDrag } from "@/lib/dustdesk-dnd"
import { repaintCurrentWindow, safeCurrentWebviewDragDropEvent, safeListen, startCurrentWindowDragging, startCurrentWindowResizeDragging } from "@/lib/tauri-window"
import { displayPathName, extensionFromPath } from "@/lib/utils"
import { useDustDeskStore } from "@/stores/dustdesk-store"
import type { DesktopItem, DesktopOperationEvent } from "@/types"

const settingsStorageKey = "dustdesk-desktop-widget-settings"
const splitCategoriesStorageKey = "dustdesk-desktop-widget-split-categories"

interface WidgetSettings {
  opacity: number
  iconSize: number
  showNames: boolean
}

const defaultSettings: WidgetSettings = {
  opacity: 0.5,
  iconSize: 44,
  showNames: true,
}

interface DesktopCardWindowPageProps {
  routeKind?: string
  routeIndex?: string
}

export function DesktopCardWindowPage({ routeKind, routeIndex }: DesktopCardWindowPageProps = {}) {
  useTheme()
  const params = useParams()
  const snapshot = useDustDeskStore((state) => state.snapshot)
  const loadDesktopSnapshot = useDustDeskStore((state) => state.loadDesktopSnapshot)
  const createCategory = useDustDeskStore((state) => state.createCategory)
  const renameCategory = useDustDeskStore((state) => state.renameCategory)
  const deleteCategory = useDustDeskStore((state) => state.deleteCategory)
  const selectCategory = useDustDeskStore((state) => state.selectCategory)
  const addItemsToCategoryLight = useDustDeskStore((state) => state.addItemsToCategoryLight)
  const addLaunchersLight = useDustDeskStore((state) => state.addLaunchersLight)
  const removeLauncher = useDustDeskStore((state) => state.removeLauncher)
  const restoreItemToDesktop = useDustDeskStore((state) => state.restoreItemToDesktop)
  const startRestoreAllToDesktopTask = useDustDeskStore((state) => state.startRestoreAllToDesktopTask)
  const showPathInFolder = useDustDeskStore((state) => state.showPathInFolder)
  const startClassifyDesktopItemsTask = useDustDeskStore((state) => state.startClassifyDesktopItemsTask)
  const openPath = useDustDeskStore((state) => state.openPath)
  const startAllLaunchers = useDustDeskStore((state) => state.startAllLaunchers)
  const splitDesktopWidgets = useDustDeskStore((state) => state.splitDesktopWidgets)
  const mergeDesktopCategory = useDustDeskStore((state) => state.mergeDesktopCategory)
  const mergeDesktopWidgets = useDustDeskStore((state) => state.mergeDesktopWidgets)
  const saveDesktopSplitIndices = useDustDeskStore((state) => state.saveDesktopSplitIndices)
  const hideCurrentWindow = useDustDeskStore((state) => state.hideCurrentWindow)
  const [settings, setSettings] = useState<WidgetSettings>(readSettings)
  const [menuOpen, setMenuOpen] = useState(false)
  const [notice, setNotice] = useState("")
  const [isClassifyingDesktop, setIsClassifyingDesktop] = useState(false)
  const [isRestoringDesktop, setIsRestoringDesktop] = useState(false)
  const [isMergingCategories, setIsMergingCategories] = useState(false)
  const desktopOperationLabel = isClassifyingDesktop ? "正在智能收纳桌面..." : isRestoringDesktop ? "正在还原桌面..." : isMergingCategories ? "正在合并分类..." : ""
  const pendingClassifyActionRef = useRef<"split-all" | null>(null)
  const previousSplitCategoryIndicesRef = useRef<number[]>([])
  const kind = (routeKind ?? params.kind) === "launcher" ? "launcher" : "category"
  const index = Number(routeIndex ?? params.index ?? 0)
  const windowLabel = kind === "launcher" ? "desktop-launcher" : `desktop-category-${index}`
  const category = Number.isFinite(index) ? snapshot.categories[index] : undefined
  const visual = useMemo(() => {
    if (kind === "launcher") {
      return { icon: RocketLaunch, color: "#fb923c", glow: "rgba(251, 146, 60, 0.28)" }
    }
    return categoryVisual(category?.name ?? "分类", index)
  }, [category?.name, index, kind])
  usePersistCurrentWindowLayout(windowLabel)

  useEffect(() => {
    if (!notice) return
    const timer = window.setTimeout(() => setNotice(""), 2400)
    return () => window.clearTimeout(timer)
  }, [notice])

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
      if (payload.status === "started") return
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
    globalThis.localStorage.setItem(settingsStorageKey, JSON.stringify(settings))
  }, [settings])

  useEffect(() => {
    if (!snapshot.data_dir) return
    writeSplitCategoryIndices(snapshot.desktop_layout.split_category_indices)
  }, [snapshot.data_dir, snapshot.desktop_layout.split_category_indices])

  useEffect(() => {
    const onDragOver = (event: DragEvent) => {
      allowPathLikeDrag(event)
    }
    const onDrop = (event: DragEvent) => {
      allowPathLikeDrag(event)
    }

    globalThis.addEventListener("dragover", onDragOver)
    globalThis.addEventListener("drop", onDrop)
    return () => {
      globalThis.removeEventListener("dragover", onDragOver)
      globalThis.removeEventListener("drop", onDrop)
    }
  }, [])

  useEffect(() => {
    let unlisten: (() => void) | undefined
    void safeCurrentWebviewDragDropEvent((event) => {
        const payload = event.payload
        if (payload.type !== "drop") return
        void handleDropped(payload.paths)
      })
      .then((value) => {
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
        const added = await addLaunchersLight(paths)
        setNotice(countNotice("已加入快捷启动", added, paths.length, "没有新增启动项"))
      } else {
        const added = await addItemsToCategoryLight(index, paths)
        setNotice(countNotice("已收纳", added, paths.length, "没有新增收纳项目"))
      }
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error))
    }
  }

  async function handleRestoreDragOut(path: string) {
    try {
      const restored = await restoreItemToDesktop(index, path)
      setNotice(`已移回桌面：${displayPathName(restored)}`)
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error))
    }
  }

  function handleLauncherDragOver(event: ReactDragEvent<HTMLElement>) {
    if (kind !== "launcher" || !hasDustDeskPathDrag(event.dataTransfer)) return
    event.preventDefault()
    event.dataTransfer.dropEffect = "copy"
  }

  function handleLauncherDrop(event: ReactDragEvent<HTMLElement>) {
    if (kind !== "launcher" || !hasDustDeskPathDrag(event.dataTransfer)) return
    event.preventDefault()
    event.stopPropagation()
    void handleDropped(readDustDeskPathDrag(event.dataTransfer))
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
      setMenuOpen(false)
      await startClassifyDesktopItemsTask()
    } catch (error) {
      pendingClassifyActionRef.current = null
      setNotice(error instanceof Error ? error.message : String(error))
      setIsClassifyingDesktop(false)
    }
  }

  async function handleOrganizeAndSplitAll() {
    if (isClassifyingDesktop) return
    const previous = readSplitCategoryIndices()
    pendingClassifyActionRef.current = "split-all"
    previousSplitCategoryIndicesRef.current = previous
    setIsClassifyingDesktop(true)
    setNotice("正在智能收纳并拆分...")
    try {
      setMenuOpen(false)
      await startClassifyDesktopItemsTask()
    } catch (error) {
      pendingClassifyActionRef.current = null
      writeSplitCategoryIndices(previous)
      setNotice(error instanceof Error ? error.message : String(error))
      setIsClassifyingDesktop(false)
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
      await loadDesktopSnapshot()
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
      await loadDesktopSnapshot()
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
      await loadDesktopSnapshot()
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
      await loadDesktopSnapshot({ force: true })
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

    setIsRestoringDesktop(true)
    setNotice("正在还原桌面...")
    try {
      setMenuOpen(false)
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
        setNotice(next.length > 0 ? `${classifyResultNotice(result)}，已拆出 ${next.length} 个分类` : `${classifyResultNotice(result)}，没有可拆出的分类内容`)
      } catch (error) {
        writeSplitCategoryIndices(previousSplitCategoryIndicesRef.current)
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
    setNotice(payload.message || (payload.restored > 0 ? `已还原 ${payload.restored} 项到桌面` : "没有需要还原到桌面的收纳项目"))
    setIsRestoringDesktop(false)
  }

  function updateSettings(next: Partial<WidgetSettings>) {
    setSettings((value) => ({ ...value, ...next }))
  }

  const Icon = visual.icon
  const items = category?.item_details ?? []
  const frameStyle = {
    backgroundColor: `rgb(15 23 42 / ${settings.opacity})`,
    boxShadow: `0 24px 90px rgba(2, 6, 23, 0.34), 0 0 0 1px ${visual.glow}`,
  }

  return (
    <div className="desktop-widget-page h-screen w-screen bg-transparent p-2 text-white">
      <section
        className="relative flex h-full w-full min-w-0 flex-col overflow-hidden rounded-2xl border border-white/15 backdrop-blur-2xl"
        style={frameStyle}
        onDragOver={handleLauncherDragOver}
        onDrop={handleLauncherDrop}
      >
        <header
          className="no-drag flex h-12 shrink-0 cursor-move items-center justify-between gap-2 border-b border-white/10 px-3"
          onPointerDown={(event) => {
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
          {kind === "launcher" ? (
            <Button size="xs" onClick={() => void handleStartAll()}>
              <RocketLaunch className="size-3.5" weight="duotone" />
              启动
            </Button>
          ) : null}
          <div className="relative">
            <Button size="icon-sm" variant="secondary" onClick={() => setMenuOpen((value) => !value)}>
              <GearSix className="size-4" weight="duotone" />
            </Button>
            {menuOpen ? (
              <SettingsMenu
                kind={kind}
                settings={settings}
                createCategory={handleCreateCategory}
                renameCategory={handleRenameCategory}
                deleteCategory={handleDeleteCategory}
                updateSettings={updateSettings}
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
        </header>

        {kind === "launcher" ? (
          <LauncherItems launchers={snapshot.launchers} settings={settings} onOpen={openPath} onShowInFolder={showPathInFolder} onRemoveLauncher={removeLauncher} />
        ) : (
          <CategoryItems
            items={items}
            categoryIndex={index}
            settings={settings}
            onOpen={openPath}
            onShowInFolder={showPathInFolder}
            onRestoreToDesktop={restoreItemToDesktop}
            onRestoreDragOut={handleRestoreDragOut}
          />
        )}
        {desktopOperationLabel ? <WidgetOperationOverlay label={desktopOperationLabel} /> : null}
      </section>
      {notice ? <div className="pointer-events-none absolute bottom-3 left-1/2 -translate-x-1/2 rounded-full bg-slate-950/70 px-3 py-1 text-xs text-white/80 ring-1 ring-white/10">{notice}</div> : null}
      <button
        type="button"
        className="no-drag absolute bottom-0 right-0 size-7 cursor-nwse-resize rounded-br-2xl border-b-2 border-r-2 border-white/45"
        aria-label="调整窗口大小"
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

function SettingsMenu({
  kind,
  settings,
  createCategory,
  renameCategory,
  deleteCategory,
  updateSettings,
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
  createCategory: () => Promise<void>
  renameCategory: () => Promise<void>
  deleteCategory: () => Promise<void>
  updateSettings: (settings: Partial<WidgetSettings>) => void
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
    <div className="desktop-widget-scroll absolute right-0 top-8 z-50 max-h-[min(64vh,260px)] w-48 overflow-y-auto rounded-xl border border-white/15 bg-slate-950/85 p-1 text-white shadow-2xl shadow-black/30 backdrop-blur-2xl">
      {kind === "category" ? (
        <>
          <MenuButton icon={Columns} label="合并当前分类" onClick={() => void onMerge()} />
          <MenuButton icon={Columns} label={isMergingCategories ? "合并中" : "一键合并分类"} disabled={isMergingCategories} onClick={() => void onMergeAllCategories()} />
          <MenuButton icon={Plus} label="新增分类" onClick={() => void createCategory()} />
          <MenuButton icon={PencilSimple} label="重命名当前分类" onClick={() => void renameCategory()} />
          <MenuButton icon={Trash} label="删除当前分类" onClick={() => void deleteCategory()} />
          <MenuButton icon={Columns} label="拆分全部分类" onClick={() => void onSplitAllCategories()} />
          <MenuButton icon={Columns} label={isClassifyingDesktop ? "智能收纳中" : "智能收纳并拆分全部"} disabled={isClassifyingDesktop} onClick={() => void onOrganizeAndSplitAll()} />
          <MenuButton icon={Archive} label={isClassifyingDesktop ? "智能收纳中" : "智能收纳桌面"} disabled={isClassifyingDesktop} onClick={() => void onClassifyDesktop()} />
          <MenuButton icon={Desktop} label={isRestoringDesktop ? "还原中" : "一键还原桌面"} disabled={isRestoringDesktop} onClick={() => void onRestoreAllToDesktop()} />
        </>
      ) : null}
      {kind === "launcher" ? <MenuButton icon={Columns} label={isMergingCategories ? "合并中" : "一键合并分类"} disabled={isMergingCategories} onClick={() => void onMergeAllCategories()} /> : null}
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
      <MenuButton icon={X} label="隐藏当前框" onClick={() => void onHide()} />
    </div>
  )
}

function MenuButton({ icon: Icon, label, disabled, onClick }: { icon: Icon; label: string; disabled?: boolean; onClick: () => void }) {
  return (
    <button type="button" disabled={disabled} className="flex w-full items-center gap-1.5 rounded-lg px-2 py-1 text-left text-[11px] font-semibold text-white/80 transition hover:bg-white/10 hover:text-white disabled:cursor-wait disabled:opacity-55" onClick={onClick}>
      <Icon className="size-3.5" weight="duotone" />
      <span>{label}</span>
    </button>
  )
}

function RangeRow({
  label,
  min,
  max,
  step,
  value,
  onChange,
}: {
  label: string
  min: number
  max: number
  step: number
  value: number
  onChange: (value: number) => void
}) {
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
  settings: WidgetSettings
  onOpen: (path: string) => Promise<void>
  onShowInFolder: (path: string) => Promise<void>
  onRestoreToDesktop: (index: number, path: string) => Promise<string>
  onRestoreDragOut: (path: string) => Promise<void>
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
            onDragEndOutside={() => onRestoreDragOut(item.path)}
            actions={[
              { label: "打开", icon: "open", onSelect: () => onOpen(item.path) },
              { label: "在资源管理器中显示", icon: "folder", onSelect: () => onShowInFolder(item.path) },
              { label: "移回桌面", icon: "restore", onSelect: async () => { await onRestoreToDesktop(categoryIndex, item.path) } },
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
  onShowInFolder,
  onRemoveLauncher,
}: {
  launchers: { name: string; path: string; icon_data_url?: string }[]
  settings: WidgetSettings
  onOpen: (path: string) => Promise<void>
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
              { label: "启动", icon: "open", onSelect: () => onOpen(item.path) },
              { label: "在资源管理器中显示", icon: "folder", onSelect: () => onShowInFolder(item.path) },
              { label: "从快捷启动移除", icon: "remove", tone: "danger", onSelect: () => onRemoveLauncher(item.path) },
            ]}
          />
        ))}
      </WidgetGrid>
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
  onDragEndOutside?: () => unknown
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
        void Promise.resolve(onDragEndOutside()).catch(() => undefined)
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
