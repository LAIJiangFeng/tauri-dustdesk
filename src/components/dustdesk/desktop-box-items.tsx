import { useCallback, useEffect, useRef, useState, type CSSProperties, type DragEvent as ReactDragEvent, type MouseEvent as ReactMouseEvent, type ReactNode } from "react"
import { FileIcon } from "@/components/dustdesk/file-icon"
import { ItemContextMenu, type ItemContextMenuAction } from "@/components/dustdesk/item-context-menu"
import { desktopItemAttribute, desktopItemListAttribute, useDesktopItemSelection } from "@/hooks/use-desktop-item-selection"
import type { DesktopWidgetSettings, DesktopWidgetViewMode } from "@/lib/desktop-widget-settings"
import { desktopDropPositionFromDragEnd, didDragEndOutsideWindow, type DesktopDropPosition, waitForDustDeskPathDropAcceptance, writeDustDeskPathDrag } from "@/lib/dustdesk-dnd"
import { cn, displayPathName, extensionFromPath } from "@/lib/utils"
import { useDustDeskStore } from "@/stores/dustdesk-store"
import type { DesktopItem, RecycleCategoryItemsResult, RestoreItemsResult } from "@/types"

export type DesktopBoxItemSettings = DesktopWidgetSettings & { viewMode: DesktopWidgetViewMode }

export interface DesktopBoxEntry {
  key: string
  path: string
  name: string
  extension: string
  isDir?: boolean
  iconDataUrl?: string
}

export interface DesktopBoxCommonActions {
  open: ItemContextMenuAction
  copy: ItemContextMenuAction
  copyPath: ItemContextMenuAction
  remove: ItemContextMenuAction
}

export interface DesktopBoxMenuContext {
  /** The item that was right-clicked. */
  path: string
  /** Every item the menu acts on: the whole selection when the clicked item was selected. */
  paths: string[]
  common: DesktopBoxCommonActions
  /** Runs a box action and shows its result (or error) as the box notice. */
  run: (task: () => Promise<string | void>) => Promise<void>
}

interface DesktopBoxItemListProps {
  entries: DesktopBoxEntry[]
  settings: DesktopBoxItemSettings
  className?: string
  dragEffectAllowed?: DataTransfer["effectAllowed"]
  openLabel: string
  removeLabel: string
  confirmRemove: (count: number) => string
  onOpen: (path: string) => Promise<void>
  onRemove: (paths: string[]) => Promise<string>
  onDragEndOutside?: (path: string, position: DesktopDropPosition) => unknown
  menuActions: (context: DesktopBoxMenuContext) => ItemContextMenuAction[]
  children?: ReactNode
}

const openConfirmThreshold = 5
const noticeDurationMs = 2400

export function categoryBoxEntry(item: DesktopItem): DesktopBoxEntry {
  return {
    key: item.path,
    path: item.path,
    name: item.name || displayPathName(item.path),
    extension: item.extension || extensionFromPath(item.path),
    isDir: item.is_dir,
    iconDataUrl: item.icon_data_url,
  }
}

export function recycleConfirmMessage(count: number) {
  return `将 ${count} 个项目移到回收站？`
}

export function recycleResultNotice(result: RecycleCategoryItemsResult) {
  const kept = result.skipped > 0 ? `${result.skipped} 项未删除已保留${result.errors[0] ? `（${result.errors[0]}）` : ""}` : ""
  if (result.recycled === 0) return kept || "没有可删除的项目"
  return `已将 ${result.recycled} 个项目移到回收站${kept ? `，${kept}` : ""}`
}

export function restoreResultNotice(result: RestoreItemsResult) {
  const failed = result.errors.length > 0 ? `${result.errors.length} 项未能移回：${result.errors[0]}` : ""
  if (result.restored === 0) return failed || "没有移回桌面的项目"
  return `已移回桌面 ${result.restored} 项${failed ? `，${failed}` : ""}`
}

/**
 * Item list of a desktop box (split category card, merged box, launcher box) with
 * Windows-desktop-like selection, copy, copy path, open and delete.
 */
export function DesktopBoxItemList({
  entries,
  settings,
  className,
  dragEffectAllowed,
  openLabel,
  removeLabel,
  confirmRemove,
  onOpen,
  onRemove,
  onDragEndOutside,
  menuActions,
  children,
}: DesktopBoxItemListProps) {
  const copyPathsToClipboard = useDustDeskStore((state) => state.copyPathsToClipboard)
  const copyTextToClipboard = useDustDeskStore((state) => state.copyTextToClipboard)
  const [menu, setMenu] = useState<{ x: number; y: number; path: string; paths: string[] } | null>(null)
  const [notice, setNotice] = useState("")
  const busyRef = useRef(false)

  useEffect(() => {
    if (!notice) return
    const timer = window.setTimeout(() => setNotice(""), noticeDurationMs)
    return () => window.clearTimeout(timer)
  }, [notice])

  const run = useCallback(async (task: () => Promise<string | void>) => {
    if (busyRef.current) return
    busyRef.current = true
    try {
      const message = await task()
      if (message) setNotice(message)
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error))
    } finally {
      busyRef.current = false
    }
  }, [])

  const copyFiles = useCallback(
    (paths: string[]) =>
      run(async () => {
        const copied = await copyPathsToClipboard(paths)
        const missing = paths.length - copied
        return `已复制 ${copied} 个项目${missing > 0 ? `，${missing} 项不存在已跳过` : ""}`
      }),
    [copyPathsToClipboard, run],
  )

  const copyPaths = useCallback(
    (paths: string[]) =>
      run(async () => {
        await copyTextToClipboard(paths.join("\r\n"))
        return paths.length > 1 ? `已复制 ${paths.length} 个路径` : "已复制路径"
      }),
    [copyTextToClipboard, run],
  )

  const openMany = useCallback(
    (paths: string[]) =>
      run(async () => {
        if (paths.length > openConfirmThreshold && !window.confirm(`将${openLabel} ${paths.length} 个项目，继续吗？`)) return
        const failed: string[] = []
        for (const path of paths) {
          try {
            await onOpen(path)
          } catch (error) {
            failed.push(`${displayPathName(path)}：${error instanceof Error ? error.message : String(error)}`)
          }
        }
        if (failed.length > 0) return `${failed.length} 项未能${openLabel}：${failed[0]}`
      }),
    [onOpen, openLabel, run],
  )

  const removeMany = useCallback(
    (paths: string[]) =>
      run(async () => {
        if (!window.confirm(confirmRemove(paths.length))) return
        return onRemove(paths)
      }),
    [confirmRemove, onRemove, run],
  )

  const selection = useDesktopItemSelection(
    entries.map((entry) => entry.path),
    { onCopy: copyFiles, onCopyPath: copyPaths, onOpen: openMany, onDelete: removeMany },
  )

  function menuContext(path: string, paths: string[]): DesktopBoxMenuContext {
    return {
      path,
      paths,
      run,
      common: {
        open: { label: openLabel, icon: "open", shortcut: "Enter", onSelect: () => openMany(paths) },
        copy: { label: "复制", icon: "copy", shortcut: "Ctrl+C", onSelect: () => copyFiles(paths) },
        copyPath: { label: "复制路径", icon: "path", shortcut: "Ctrl+Shift+C", onSelect: () => copyPaths(paths) },
        remove: { label: removeLabel, icon: "remove", tone: "danger", shortcut: "Delete", onSelect: () => removeMany(paths) },
      },
    }
  }

  return (
    <>
      <div
        className={className}
        {...{ [desktopItemListAttribute]: "" }}
        onClick={(event) => {
          // Clicks inside the menu portal bubble here through React; only real blank-area clicks clear.
          const target = event.target as HTMLElement
          if (!event.currentTarget.contains(target) || target.closest(`[${desktopItemAttribute}]`)) return
          selection.clearSelection()
        }}
      >
        <DesktopBoxGrid settings={settings}>
          {entries.map((entry) => (
            <DesktopBoxItem
              key={entry.key}
              entry={entry}
              settings={settings}
              selected={selection.isSelected(entry.path)}
              dragEffectAllowed={dragEffectAllowed}
              onClick={(event) => selection.handleItemClick(entry.path, event)}
              onOpen={() => void openMany([entry.path])}
              onContextMenu={(event) => {
                event.preventDefault()
                const paths = selection.handleItemContextMenu(entry.path)
                setMenu({ x: event.clientX, y: event.clientY, path: entry.path, paths })
              }}
              onDragEndOutside={onDragEndOutside ? (position) => onDragEndOutside(entry.path, position) : undefined}
            />
          ))}
        </DesktopBoxGrid>
        {children}
      </div>
      {menu ? (
        <ItemContextMenu
          x={menu.x}
          y={menu.y}
          header={menu.paths.length > 1 ? `已选 ${menu.paths.length} 项` : undefined}
          actions={menuActions(menuContext(menu.path, menu.paths))}
          onClose={() => setMenu(null)}
        />
      ) : null}
      {notice ? (
        <div className="pointer-events-none absolute bottom-3 left-1/2 z-30 max-w-[calc(100%-1.5rem)] -translate-x-1/2 truncate rounded-full bg-slate-950/70 px-3 py-1 text-xs text-white/80 ring-1 ring-white/10">
          {notice}
        </div>
      ) : null}
    </>
  )
}

function DesktopBoxGrid({ settings, children }: { settings: DesktopBoxItemSettings; children: ReactNode }) {
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

function DesktopBoxItem({
  entry,
  settings,
  selected,
  dragEffectAllowed,
  onClick,
  onOpen,
  onContextMenu,
  onDragEndOutside,
}: {
  entry: DesktopBoxEntry
  settings: DesktopBoxItemSettings
  selected: boolean
  dragEffectAllowed?: DataTransfer["effectAllowed"]
  onClick: (event: ReactMouseEvent<HTMLButtonElement>) => void
  onOpen: () => void
  onContextMenu: (event: ReactMouseEvent<HTMLButtonElement>) => void
  onDragEndOutside?: (position: DesktopDropPosition) => unknown
}) {
  const isList = settings.viewMode === "list"
  // Only category items are drag sources; launcher entries must never be dragged back to the desktop.
  const dragPath = dragEffectAllowed ? entry.path : undefined

  return (
    <button
      type="button"
      title={entry.path}
      draggable={Boolean(dragPath)}
      {...{ [desktopItemAttribute]: "" }}
      aria-pressed={selected}
      className={cn(
        "flex min-w-0 items-center text-white/90 transition",
        isList
          ? cn(
              "min-h-11 flex-row gap-2 rounded-lg border px-2 py-1.5 text-left",
              selected ? "border-white/35 bg-white/20 hover:bg-white/25" : "border-transparent bg-transparent hover:border-white/10 hover:bg-white/10",
            )
          : cn(
              "flex-col gap-2 rounded-xl border p-2 text-center",
              selected ? "border-white/55 bg-white/25 ring-1 ring-white/35 hover:bg-white/30" : "border-white/10 bg-white/10 hover:border-white/25 hover:bg-white/15",
            ),
      )}
      onClick={onClick}
      onDragStart={(event) => {
        if (!dragPath) {
          event.preventDefault()
          return
        }
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
      onDoubleClick={onOpen}
      onContextMenu={onContextMenu}
    >
      <FileIcon
        name={entry.name}
        extension={entry.extension}
        isDir={entry.isDir}
        iconDataUrl={entry.iconDataUrl}
        className={cn("widget-item-icon bg-white/10 text-white/70", isList && "rounded-md")}
      />
      {isList || settings.showNames ? (
        <span className={cn("truncate text-xs font-semibold", isList ? "min-w-0 flex-1 text-left leading-5" : "w-full")}>{entry.name}</span>
      ) : null}
    </button>
  )
}
