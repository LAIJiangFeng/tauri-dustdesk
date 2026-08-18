import { useEffect, useRef, useState, type ReactNode } from "react"
import { Archive, CaretDown, CursorClick, Desktop, Eye, EyeSlash, LockSimple, LockSimpleOpen, RocketLaunch, SlidersHorizontal } from "@phosphor-icons/react"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { useDesktopWindowState } from "@/hooks/use-desktop-window-state"
import { cn } from "@/lib/utils"
import { useDustDeskStore } from "@/stores/dustdesk-store"

type VisibilityAction = "all" | "organizer" | "launcher"

export function DesktopFrameOperationMenu() {
  const rootRef = useRef<HTMLDivElement | null>(null)
  const [open, setOpen] = useState(false)
  const [visibilityPending, setVisibilityPending] = useState<VisibilityAction | "">("")
  const [error, setError] = useState("")
  const desktopFrames = useDustDeskStore((state) => state.desktopFrames)
  const refreshDesktopFrameVisibility = useDustDeskStore((state) => state.refreshDesktopFrameVisibility)
  const toggleDesktopFrames = useDustDeskStore((state) => state.toggleDesktopFrames)
  const toggleDesktopOrganizerFrame = useDustDeskStore((state) => state.toggleDesktopOrganizerFrame)
  const toggleDesktopLauncherFrame = useDustDeskStore((state) => state.toggleDesktopLauncherFrame)
  const { state: desktopWindowState, pending: windowStatePending, setLocked, setClickThrough } = useDesktopWindowState()

  useEffect(() => {
    if (!open) return

    const handlePointerDown = (event: PointerEvent) => {
      if (rootRef.current?.contains(event.target as Node)) return
      setOpen(false)
    }
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpen(false)
    }

    document.addEventListener("pointerdown", handlePointerDown)
    document.addEventListener("keydown", handleKeyDown)
    return () => {
      document.removeEventListener("pointerdown", handlePointerDown)
      document.removeEventListener("keydown", handleKeyDown)
    }
  }, [open])

  useEffect(() => {
    if (!open) return
    void refreshDesktopFrameVisibility().catch((reason) => {
      setError(reason instanceof Error ? reason.message : String(reason))
    })
  }, [open, refreshDesktopFrameVisibility])

  const runVisibilityAction = async (action: VisibilityAction, task: () => Promise<void>) => {
    setVisibilityPending(action)
    setError("")
    try {
      await task()
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason))
    } finally {
      setVisibilityPending("")
    }
  }

  const toggleLocked = async () => {
    setError("")
    try {
      await setLocked(!desktopWindowState.locked)
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason))
    }
  }

  const toggleClickThrough = async () => {
    setError("")
    try {
      await setClickThrough(!desktopWindowState.click_through)
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason))
    }
  }

  return (
    <div ref={rootRef} className="relative">
      <Button variant="secondary" size="lg" aria-haspopup="menu" aria-expanded={open} onClick={() => setOpen((value) => !value)}>
        <SlidersHorizontal className="size-4" weight="duotone" />
        操作
        <CaretDown className={cn("size-3.5 transition-transform", open && "rotate-180")} weight="bold" />
      </Button>

      {open ? (
        <div
          className="absolute right-0 top-[calc(100%+0.5rem)] z-[80] w-80 rounded-xl border bg-popover p-2 text-popover-foreground shadow-xl ring-1 ring-foreground/10"
          role="menu"
          aria-label="桌面框操作"
        >
          <div className="flex items-center justify-between gap-3 px-2 py-1.5">
            <span className="text-sm font-semibold">桌面框操作</span>
            <Badge variant={desktopFrames.any ? "default" : "outline"}>{desktopFrames.any ? "正在显示" : "全部隐藏"}</Badge>
          </div>

          <OperationSection title="显示范围">
            <OperationItem
              icon={desktopFrames.any ? <EyeSlash className="size-4" weight="duotone" /> : <Eye className="size-4" weight="duotone" />}
              label={desktopFrames.any ? "隐藏全部桌面框" : "显示全部桌面框"}
              status={desktopFrames.any ? "显示" : "隐藏"}
              active={desktopFrames.any}
              disabled={Boolean(visibilityPending)}
              pending={visibilityPending === "all"}
              onClick={() => void runVisibilityAction("all", toggleDesktopFrames)}
            />
            <OperationItem
              icon={<Archive className="size-4" weight="duotone" />}
              label={desktopFrames.organizer ? "隐藏收纳桌面框" : "显示收纳桌面框"}
              status={desktopFrames.organizer ? "显示" : "隐藏"}
              active={desktopFrames.organizer}
              disabled={Boolean(visibilityPending)}
              pending={visibilityPending === "organizer"}
              onClick={() => void runVisibilityAction("organizer", toggleDesktopOrganizerFrame)}
            />
            <OperationItem
              icon={<RocketLaunch className="size-4" weight="duotone" />}
              label={desktopFrames.launcher ? "隐藏快捷启动框" : "显示快捷启动框"}
              status={desktopFrames.launcher ? "显示" : "隐藏"}
              active={desktopFrames.launcher}
              disabled={Boolean(visibilityPending)}
              pending={visibilityPending === "launcher"}
              onClick={() => void runVisibilityAction("launcher", toggleDesktopLauncherFrame)}
            />
          </OperationSection>

          <OperationSection title="窗口行为">
            <OperationItem
              icon={desktopWindowState.locked ? <LockSimpleOpen className="size-4" weight="duotone" /> : <LockSimple className="size-4" weight="duotone" />}
              label={desktopWindowState.locked ? "解锁全部桌面框" : "锁定全部桌面框"}
              status={desktopWindowState.locked ? "已锁定" : "可调整"}
              active={desktopWindowState.locked}
              disabled={windowStatePending}
              onClick={() => void toggleLocked()}
            />
            <OperationItem
              icon={<CursorClick className="size-4" weight="duotone" />}
              label={desktopWindowState.click_through ? "关闭鼠标穿透" : "开启鼠标穿透"}
              status={desktopWindowState.click_through ? "已穿透" : "可操作"}
              active={desktopWindowState.click_through}
              disabled={windowStatePending}
              onClick={() => void toggleClickThrough()}
            />
          </OperationSection>

          {error ? <p className="px-2 py-1.5 text-xs leading-5 text-destructive">{error}</p> : null}
        </div>
      ) : null}
    </div>
  )
}

function OperationSection({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="mt-1 border-t pt-1">
      <p className="px-2 py-1 text-[11px] font-semibold tracking-wide text-muted-foreground">{title}</p>
      <div className="grid gap-1">{children}</div>
    </section>
  )
}

function OperationItem({
  icon,
  label,
  status,
  active,
  disabled,
  pending = false,
  onClick,
}: {
  icon: ReactNode
  label: string
  status: string
  active: boolean
  disabled: boolean
  pending?: boolean
  onClick: () => void
}) {
  return (
    <button
      type="button"
      role="menuitem"
      disabled={disabled}
      className="flex w-full items-center gap-2 rounded-lg px-2 py-2 text-left transition hover:bg-muted disabled:cursor-wait disabled:opacity-55"
      onClick={onClick}
    >
      <span className={cn("flex size-8 shrink-0 items-center justify-center rounded-lg bg-muted text-muted-foreground", active && "bg-primary/10 text-primary")}>{icon}</span>
      <span className="min-w-0 flex-1 truncate text-sm font-medium">{pending ? "处理中..." : label}</span>
      <Badge variant={active ? "default" : "outline"}>{status}</Badge>
    </button>
  )
}
