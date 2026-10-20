import { ClipboardText, Copy, FolderOpen, Info, PencilSimple, RocketLaunch, Scissors, ShieldCheck, Trash, UploadSimple } from "@phosphor-icons/react"
import { useLayoutEffect, useRef, useState } from "react"
import { createPortal } from "react-dom"
import { desktopPopupAttribute } from "@/hooks/use-desktop-item-selection"
import { cn } from "@/lib/utils"

export interface ItemContextMenuAction {
  label: string
  tone?: "default" | "danger"
  icon?: "open" | "admin" | "folder" | "restore" | "remove" | "copy" | "path" | "cut" | "rename" | "properties"
  shortcut?: string
  onSelect: () => void | Promise<void>
}

interface ItemContextMenuProps {
  x: number
  y: number
  actions: ItemContextMenuAction[]
  header?: string
  onClose: () => void
}

const actionIcons = {
  open: RocketLaunch,
  admin: ShieldCheck,
  folder: FolderOpen,
  restore: UploadSimple,
  remove: Trash,
  copy: Copy,
  path: ClipboardText,
  cut: Scissors,
  rename: PencilSimple,
  properties: Info,
}

export function ItemContextMenu({ x, y, actions, header, onClose }: ItemContextMenuProps) {
  const menuRef = useRef<HTMLDivElement>(null)
  const [position, setPosition] = useState({ left: x, top: y })

  useLayoutEffect(() => {
    const menu = menuRef.current
    if (!menu) return

    const viewportWidth = document.documentElement.clientWidth
    const viewportHeight = document.documentElement.clientHeight
    const menuRect = menu.getBoundingClientRect()
    const viewportMargin = 8
    const maxLeft = Math.max(viewportMargin, viewportWidth - menuRect.width - viewportMargin)
    const maxTop = Math.max(viewportMargin, viewportHeight - menuRect.height - viewportMargin)

    setPosition({
      left: Math.min(Math.max(viewportMargin, x), maxLeft),
      top: Math.min(Math.max(viewportMargin, y), maxTop),
    })
  }, [actions.length, header, x, y])

  if (actions.length === 0) return null

  return createPortal(
    <>
      <button
        type="button"
        className="fixed inset-0 z-[80] cursor-default bg-transparent"
        aria-label="关闭菜单"
        onClick={onClose}
        onContextMenu={(event) => {
          event.preventDefault()
          onClose()
        }}
      />
      <div
        ref={menuRef}
        {...{ [desktopPopupAttribute]: "" }}
        data-edge-hide-hold=""
        className="fixed z-[90] w-max min-w-44 max-w-[calc(100vw-1rem)] overflow-x-auto overflow-y-auto rounded-xl border bg-popover p-1 text-popover-foreground shadow-xl ring-1 ring-foreground/10"
        style={{ left: position.left, top: position.top, maxHeight: "calc(100vh - 1rem)" }}
        role="menu"
      >
        {header ? <div className="px-2.5 pb-1 pt-1.5 text-xs font-semibold text-muted-foreground">{header}</div> : null}
        {actions.map((action) => {
          const Icon = action.icon ? actionIcons[action.icon] : undefined
          return (
            <button
              key={action.label}
              type="button"
              role="menuitem"
              className={cn(
                "flex w-full min-w-max items-center gap-2 whitespace-nowrap rounded-lg px-2.5 py-2 text-left text-sm font-medium transition hover:bg-muted",
                action.tone === "danger" && "text-destructive hover:bg-destructive/10",
              )}
              onClick={() => {
                onClose()
                void action.onSelect()
              }}
            >
              {Icon ? <Icon className="size-4 shrink-0" weight="duotone" /> : null}
              <span>{action.label}</span>
              {action.shortcut ? <span className="ml-auto pl-6 text-xs font-normal text-muted-foreground">{action.shortcut}</span> : null}
            </button>
          )
        })}
      </div>
    </>,
    document.body,
  )
}
