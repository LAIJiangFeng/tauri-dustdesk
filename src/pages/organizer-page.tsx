import { useDeferredValue, useEffect, useState, type DragEvent as ReactDragEvent, type ReactNode } from "react"
import { Archive, CaretRight, CheckCircle, Desktop, FolderOpen, MagnifyingGlass, PencilSimple, Plus, RocketLaunch, Trash, X } from "@phosphor-icons/react"
import { EmptyState } from "@/components/dustdesk/empty-state"
import { FileIcon } from "@/components/dustdesk/file-icon"
import { ItemContextMenu, type ItemContextMenuAction } from "@/components/dustdesk/item-context-menu"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card"
import { Input } from "@/components/ui/input"
import { ScrollArea } from "@/components/ui/scroll-area"
import { didDragEndOutsideWindow, hasDustDeskPathDrag, hasPathLikeDrag, readDustDeskPathDrag, writeDustDeskPathDrag } from "@/lib/dustdesk-dnd"
import { safeCurrentWebviewDragDropEvent } from "@/lib/tauri-window"
import { cn } from "@/lib/utils"
import { useDustDeskStore } from "@/stores/dustdesk-store"
import type { DesktopItem } from "@/types"

const organizerCategoryDropZone = "organizer-category"

export function OrganizerPage() {
  const snapshot = useDustDeskStore((state) => state.snapshot)
  const selectedCategory = useDustDeskStore((state) => state.selectedCategory)
  const selectCategory = useDustDeskStore((state) => state.selectCategory)
  const createCategory = useDustDeskStore((state) => state.createCategory)
  const renameCategory = useDustDeskStore((state) => state.renameCategory)
  const deleteCategory = useDustDeskStore((state) => state.deleteCategory)
  const openSpecial = useDustDeskStore((state) => state.openSpecial)
  const classifyDesktopItems = useDustDeskStore((state) => state.classifyDesktopItems)
  const addItemsToCategory = useDustDeskStore((state) => state.addItemsToCategory)
  const desktopFrames = useDustDeskStore((state) => state.desktopFrames)
  const refreshDesktopFrameVisibility = useDustDeskStore((state) => state.refreshDesktopFrameVisibility)
  const toggleDesktopOrganizerFrame = useDustDeskStore((state) => state.toggleDesktopOrganizerFrame)
  const [query, setQuery] = useState("")
  const [notice, setNotice] = useState<string | null>(null)
  const deferredQuery = useDeferredValue(query.trim().toLowerCase())
  const category = snapshot.categories[selectedCategory]
  const desktopItems = deferredQuery
    ? snapshot.desktop_items.filter((item) => `${item.name} ${item.path} ${item.extension}`.toLowerCase().includes(deferredQuery))
    : snapshot.desktop_items
  const handleClassifyDesktopItems = async () => {
    try {
      const result = await classifyDesktopItems()
      setNotice(classifyResultNotice(result))
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error))
    }
  }

  useEffect(() => {
    void refreshDesktopFrameVisibility()
  }, [refreshDesktopFrameVisibility])

  useEffect(() => {
    let unlisten: (() => void) | undefined
    void safeCurrentWebviewDragDropEvent((event) => {
        const payload = event.payload
        if (payload.type !== "drop") return
        if (dropZoneFromPoint(payload.position.x, payload.position.y) !== organizerCategoryDropZone) return
        void addPathsToSelectedCategory(payload.paths)
      })
      .then((value) => {
        unlisten = value
      })
    return () => unlisten?.()
  }, [addItemsToCategory, selectedCategory])

  async function addPathsToSelectedCategory(paths: string[]) {
    if (paths.length === 0) return
    try {
      const added = await addItemsToCategory(selectedCategory, paths)
      setNotice(countNotice("已收纳", added, paths.length, "没有新增收纳项目"))
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error))
    }
  }

  function handleCategoryDragOver(event: ReactDragEvent<HTMLElement>) {
    if (!hasPathLikeDrag(event.dataTransfer)) return
    event.preventDefault()
    event.dataTransfer.dropEffect = "copy"
  }

  function handleCategoryDrop(event: ReactDragEvent<HTMLElement>) {
    if (!hasPathLikeDrag(event.dataTransfer)) return
    event.preventDefault()
    if (hasDustDeskPathDrag(event.dataTransfer)) {
      void addPathsToSelectedCategory(readDustDeskPathDrag(event.dataTransfer))
    }
  }

  return (
    <div className="grid h-full min-h-0 gap-4 xl:grid-cols-[290px_minmax(0,1fr)_390px]">
      <Card
        className="min-h-0"
        data-path-drop-zone={organizerCategoryDropZone}
        onDragOver={handleCategoryDragOver}
        onDrop={handleCategoryDrop}
      >
        <CardHeader>
          <div>
            <CardTitle>分类</CardTitle>
          </div>
          <Badge variant="outline">{snapshot.categories.length}</Badge>
        </CardHeader>
        <CardContent className="flex min-h-0 flex-col gap-3">
          <div className="grid grid-cols-3 gap-2">
            <Button className="w-full" size="sm" variant="secondary" onClick={() => void createCategory()}>
              <Plus className="size-3.5" weight="bold" />
              新建
            </Button>
            <Button className="w-full" size="sm" variant="secondary" onClick={() => void renameCategory()}>
              <PencilSimple className="size-3.5" weight="duotone" />
              重命名
            </Button>
            <Button className="w-full" size="sm" variant="destructive" onClick={() => void deleteCategory()}>
              <Trash className="size-3.5" weight="duotone" />
              删除
            </Button>
          </div>

          <ScrollArea className="min-h-0 flex-1 pr-2">
            <div className="grid gap-2">
              {snapshot.categories.map((item, index) => (
                <Button
                  key={`${item.name}-${index}`}
                  variant={selectedCategory === index ? "default" : "outline"}
                  className="h-auto justify-start gap-3 p-3 text-left"
                  onClick={() => selectCategory(index)}
                >
                    <span className="min-w-0 flex-1">
                    <span className="block truncate font-medium">{item.name}</span>
                    <span className={cn("block text-xs", selectedCategory === index ? "text-primary-foreground/70" : "text-muted-foreground")}>
                      {item.item_paths.length} 个项目
                    </span>
                  </span>
                  <CaretRight className="size-4 shrink-0" weight="bold" />
                </Button>
              ))}
            </div>
          </ScrollArea>
        </CardContent>
      </Card>

      <Card className="min-h-0">
        <CardHeader>
          <div>
            <CardTitle>桌面项目</CardTitle>
          </div>
          <div className="flex items-center gap-2">
            <Button size="sm" variant="secondary" onClick={() => void openSpecial("desktop")}>
              <Desktop className="size-3.5" weight="duotone" />
              桌面
            </Button>
            <Badge variant="outline">{desktopItems.length} 项</Badge>
          </div>
        </CardHeader>
        <CardContent className="flex min-h-0 flex-col gap-4">
          <div className="grid grid-cols-3 gap-2">
            <Button variant="secondary" size="sm" onClick={() => void openSpecial("organizer")}>
              打开收纳箱
            </Button>
            <Button variant="secondary" size="sm" onClick={() => void toggleDesktopOrganizerFrame()}>
              <Desktop className="size-3.5" weight="duotone" />
              {desktopFrames.organizer ? "隐藏收纳桌面框" : "显示收纳桌面框"}
            </Button>
            <Button size="sm" onClick={() => void handleClassifyDesktopItems()}>
              <Archive className="size-3.5" weight="duotone" />
              智能收纳桌面
            </Button>
          </div>
          {notice ? (
            <div className="rounded-lg border bg-muted/45 px-3 py-2 text-xs text-muted-foreground">
              {notice}
            </div>
          ) : null}
          <div className="relative">
            <MagnifyingGlass className="pointer-events-none absolute left-2.5 top-1/2 size-4 -translate-y-1/2 text-muted-foreground" weight="duotone" />
            <Input value={query} onChange={(event) => setQuery(event.target.value)} className="pl-8" placeholder="搜索桌面文件、快捷方式、扩展名" />
          </div>

          <ScrollArea className="min-h-0 flex-1 pr-2">
            {desktopItems.length > 0 ? (
              <div className="grid grid-cols-[repeat(auto-fill,minmax(126px,1fr))] gap-3">
                {desktopItems.map((item) => (
                  <DesktopTile key={item.path} item={item} />
                ))}
              </div>
            ) : (
              <EmptyState icon={Archive} title="没有找到桌面项目" />
            )}
          </ScrollArea>
        </CardContent>
      </Card>

      <Card className="min-h-0">
        <CardHeader>
          <div>
            <CardTitle>{category?.name ?? "分类内容"}</CardTitle>
          </div>
          <Badge variant="secondary">{category?.item_paths.length ?? 0} 项</Badge>
        </CardHeader>
        <CardContent className="flex min-h-0 flex-col gap-4">
          <ScrollArea className="min-h-0 flex-1 pr-2">
            {category && category.item_details.length > 0 ? (
              <div className="grid grid-cols-[repeat(auto-fill,minmax(126px,1fr))] gap-3">
                {category.item_details.map((item) => (
                  <CategoryItemTile key={item.path} item={item} categoryIndex={selectedCategory} />
                ))}
              </div>
            ) : (
              <EmptyState icon={FolderOpen} title="这个分类还是空的" />
            )}
          </ScrollArea>
        </CardContent>
      </Card>
    </div>
  )
}

function DesktopTile({ item }: { item: DesktopItem }) {
  const openPath = useDustDeskStore((state) => state.openPath)
  const selectedCategory = useDustDeskStore((state) => state.selectedCategory)
  const snapshot = useDustDeskStore((state) => state.snapshot)
  const addItemToCategory = useDustDeskStore((state) => state.addItemToCategory)
  const addLauncher = useDustDeskStore((state) => state.addLauncher)
  const showPathInFolder = useDustDeskStore((state) => state.showPathInFolder)
  const category = snapshot.categories[selectedCategory]
  const isInCurrentCategory = Boolean(category?.item_paths.some((path) => path.toLowerCase() === item.path.toLowerCase()))
  const isLauncherAdded = snapshot.launchers.some((launcher) => launcher.path.toLowerCase() === item.path.toLowerCase())
  const canLaunch = isLaunchable(item)

  return (
    <DesktopItemShell
      title={item.path}
      dragPath={item.path}
      onOpen={() => void openPath(item.path)}
      actions={[
        { label: "打开", icon: "open", onSelect: () => openPath(item.path) },
        { label: "在资源管理器中显示", icon: "folder", onSelect: () => showPathInFolder(item.path) },
        ...(isInCurrentCategory ? [] : [{ label: `收纳到${category ? `「${category.name}」` : "当前分类"}`, icon: "restore" as const, onSelect: () => addItemToCategory(selectedCategory, item.path) }]),
        ...(canLaunch && !isLauncherAdded ? [{ label: "加入快捷启动", icon: "open" as const, onSelect: () => addLauncher(item.path, item.name) }] : []),
      ]}
    >
      <CardContent className="flex h-full flex-col items-center justify-between gap-2 p-3 text-center">
        <div className="flex min-h-0 w-full flex-1 flex-col items-center justify-center gap-2">
          <FileIcon name={item.name} extension={item.extension} isDir={item.is_dir} iconDataUrl={item.icon_data_url} className="size-12" />
          <span className="w-full truncate text-sm font-medium">{item.name}</span>
        </div>
        <div className="grid w-full grid-cols-2 gap-1.5">
          <Button
            size="xs"
            variant={isInCurrentCategory ? "secondary" : "default"}
            disabled={isInCurrentCategory}
            onClick={(event) => {
              event.stopPropagation()
              void addItemToCategory(selectedCategory, item.path)
            }}
          >
            {isInCurrentCategory ? <CheckCircle className="size-3" weight="fill" /> : <Archive className="size-3" weight="duotone" />}
            {isInCurrentCategory ? "已收纳" : "收纳"}
          </Button>
          <Button
            size="xs"
            variant="secondary"
            onClick={(event) => {
              event.stopPropagation()
              void openPath(item.path)
            }}
            title="打开此项目"
          >
            <RocketLaunch className="size-3" weight="duotone" />
            打开
          </Button>
        </div>
      </CardContent>
    </DesktopItemShell>
  )
}

function CategoryItemTile({ item, categoryIndex }: { item: DesktopItem; categoryIndex: number }) {
  const openPath = useDustDeskStore((state) => state.openPath)
  const restoreItemToDesktop = useDustDeskStore((state) => state.restoreItemToDesktop)
  const showPathInFolder = useDustDeskStore((state) => state.showPathInFolder)

  return (
    <DesktopItemShell
      title={item.path}
      dragPath={item.path}
      dragEffectAllowed="copyMove"
      onDragEndOutside={() => restoreItemToDesktop(categoryIndex, item.path)}
      onOpen={() => void openPath(item.path)}
      actions={[
        { label: "打开", icon: "open", onSelect: () => openPath(item.path) },
        { label: "在资源管理器中显示", icon: "folder", onSelect: () => showPathInFolder(item.path) },
        { label: "移回桌面", icon: "restore", onSelect: async () => { await restoreItemToDesktop(categoryIndex, item.path) } },
      ]}
    >
      <CardContent className="flex h-full flex-col items-center justify-between gap-2 p-3 text-center">
        <div className="flex min-h-0 w-full flex-1 flex-col items-center justify-center gap-2">
          <FileIcon name={item.name} extension={item.extension} isDir={item.is_dir} iconDataUrl={item.icon_data_url} className="size-12" />
          <span className="w-full truncate text-sm font-medium">{item.name}</span>
        </div>
        <div className="grid w-full grid-cols-2 gap-1.5">
          <Button size="xs" variant="secondary" onClick={(event) => {
            event.stopPropagation()
            void openPath(item.path)
          }}>
            <FolderOpen className="size-3" weight="duotone" />
            打开
          </Button>
          <Button size="xs" variant="outline" onClick={(event) => {
            event.stopPropagation()
            void restoreItemToDesktop(categoryIndex, item.path)
          }}>
            <X className="size-3" weight="bold" />
            移回
          </Button>
        </div>
      </CardContent>
    </DesktopItemShell>
  )
}

function DesktopItemShell({
  title,
  actions,
  dragPath,
  dragEffectAllowed,
  onDragEndOutside,
  onOpen,
  children,
}: {
  title: string
  actions: ItemContextMenuAction[]
  dragPath?: string
  dragEffectAllowed?: DataTransfer["effectAllowed"]
  onDragEndOutside?: () => unknown
  onOpen: () => void
  children: ReactNode
}) {
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null)

  return (
    <Card
      className="group h-44 overflow-hidden transition-colors hover:border-primary/45"
      title={title}
      draggable={Boolean(dragPath)}
      onDragStart={(event) => {
        if (isInteractiveTarget(event.target)) {
          event.preventDefault()
          return
        }
        if (!dragPath) return
        writeDustDeskPathDrag(event.dataTransfer, dragPath, dragEffectAllowed)
      }}
      onDragEnd={(event: ReactDragEvent<HTMLDivElement>) => {
        if (!dragPath || !onDragEndOutside || !didDragEndOutsideWindow(event)) return
        void Promise.resolve(onDragEndOutside()).catch(() => undefined)
      }}
      onDoubleClick={(event) => {
        if (isInteractiveTarget(event.target)) return
        onOpen()
      }}
      onContextMenu={(event) => {
        event.preventDefault()
        setMenu({ x: event.clientX, y: event.clientY })
      }}
    >
      {children}
      {menu ? <ItemContextMenu x={menu.x} y={menu.y} actions={actions} onClose={() => setMenu(null)} /> : null}
    </Card>
  )
}

function isInteractiveTarget(target: EventTarget | null) {
  return target instanceof HTMLElement && Boolean(target.closest("button,input,a,[role='menuitem']"))
}

function dropZoneFromPoint(physicalX: number, physicalY: number) {
  const ratio = globalThis.devicePixelRatio || 1
  const element = document.elementFromPoint(physicalX / ratio, physicalY / ratio)
  return element?.closest<HTMLElement>("[data-path-drop-zone]")?.dataset.pathDropZone ?? ""
}

function isLaunchable(item: DesktopItem) {
  if (item.is_dir) return false
  return ["LNK", "EXE", "APPREF-MS", "URL", "BAT", "CMD", "PS1", "MSI"].includes(item.extension.toUpperCase())
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

function countNotice(action: string, count: number, total: number, empty: string) {
  if (count <= 0) return empty
  const skipped = Math.max(0, total - count)
  return `${action} ${count} 项${skipped ? `，跳过 ${skipped} 项` : ""}`
}
