import { FolderOpen, RocketLaunch, Trash, UploadSimple } from "@phosphor-icons/react"
import { cn } from "@/lib/utils"

export interface ItemContextMenuAction {
  label: string
  tone?: "default" | "danger"
  icon?: "open" | "folder" | "restore" | "remove"
  onSelect: () => void | Promise<void>
}

interface ItemContextMenuProps {
  x: number
  y: number
  actions: ItemContextMenuAction[]
  onClose: () => void
}

const actionIcons = {
  open: RocketLaunch,
  folder: FolderOpen,
  restore: UploadSimple,
  remove: Trash,
}

export function ItemContextMenu({ x, y, actions, onClose }: ItemContextMenuProps) {
  if (actions.length === 0) return null

  return (
    <>
      <button type="button" className="fixed inset-0 z-[80] cursor-default bg-transparent" aria-label="关闭菜单" onClick={onClose} />
      <div
        className="fixed z-[90] min-w-44 overflow-hidden rounded-xl border bg-popover p-1 text-popover-foreground shadow-xl ring-1 ring-foreground/10"
        style={{ left: x, top: y }}
        role="menu"
      >
        {actions.map((action) => {
          const Icon = action.icon ? actionIcons[action.icon] : undefined
          return (
            <button
              key={action.label}
              type="button"
              role="menuitem"
              className={cn(
                "flex w-full items-center gap-2 rounded-lg px-2.5 py-2 text-left text-sm font-medium transition hover:bg-muted",
                action.tone === "danger" && "text-destructive hover:bg-destructive/10",
              )}
              onClick={() => {
                onClose()
                void action.onSelect()
              }}
            >
              {Icon ? <Icon className="size-4" weight="duotone" /> : null}
              <span>{action.label}</span>
            </button>
          )
        })}
      </div>
    </>
  )
}
