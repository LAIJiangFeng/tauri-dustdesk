import { startTransition, useEffect } from "react"
import { NavLink, Outlet, useLocation } from "react-router"
import { ArrowsClockwise, Crosshair, Desktop, Minus, MoonStars, Square, SunDim, Warning, X } from "@phosphor-icons/react"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { ScrollArea } from "@/components/ui/scroll-area"
import { Separator } from "@/components/ui/separator"
import { navigationItems, pageFromPath, pageMeta } from "@/config/navigation"
import { useTheme } from "@/hooks/use-theme"
import { hideMainWindowToTray, minimizeCurrentWindow, toggleCurrentWindowMaximize } from "@/lib/tauri-window"
import { truncate } from "@/lib/utils"
import { useDustDeskStore } from "@/stores/dustdesk-store"

export function AppShell() {
  const location = useLocation()
  const activePage = pageFromPath(location.pathname)
  const setPage = useDustDeskStore((state) => state.setPage)
  const { theme, toggleTheme } = useTheme()

  useEffect(() => {
    startTransition(() => setPage(activePage))
  }, [activePage, setPage])

  return (
    <div className="flex h-screen w-screen flex-col overflow-hidden bg-background text-foreground">
      <WindowTitleBar />
      <main className="flex min-h-0 flex-1 overflow-hidden">
        <DesktopRail activePath={location.pathname} />
        <section className="flex min-w-0 flex-1 flex-col">
          <TopBar activePage={activePage} theme={theme} onToggleTheme={toggleTheme} />
          <Separator />
          <div className="min-h-0 min-w-0 flex-1 overflow-hidden p-4 md:p-6">
            <Outlet />
          </div>
          <MobileDock activePath={location.pathname} />
        </section>
      </main>
    </div>
  )
}

function WindowTitleBar() {
  return (
    <header className="window-drag flex h-8 shrink-0 items-center justify-between border-b bg-background px-2 text-foreground">
      <div className="flex min-w-0 items-center gap-2">
        <img src="/desknest-logo.svg" alt="DeskNest" className="size-4 rounded-sm" />
        <span className="truncate text-xs font-medium">DeskNest</span>
      </div>
      <div className="no-drag flex h-full items-center">
        <button
          type="button"
          className="flex h-full w-11 items-center justify-center text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
          aria-label="最小化"
          onClick={() => void minimizeCurrentWindow()}
        >
          <Minus className="size-4" weight="bold" />
        </button>
        <button
          type="button"
          className="flex h-full w-11 items-center justify-center text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
          aria-label="最大化"
          onClick={() => void toggleCurrentWindowMaximize()}
        >
          <Square className="size-3.5" weight="bold" />
        </button>
        <button
          type="button"
          className="flex h-full w-11 items-center justify-center text-muted-foreground transition-colors hover:bg-destructive hover:text-white"
          aria-label="关闭"
          onClick={() => void hideMainWindowToTray()}
        >
          <X className="size-4" weight="bold" />
        </button>
      </div>
    </header>
  )
}

function DesktopRail({ activePath }: { activePath: string }) {
  return (
    <aside className="window-drag hidden w-28 shrink-0 border-r bg-sidebar p-3 md:flex md:flex-col">
      <div className="no-drag mb-3 flex h-14 items-center justify-center rounded-xl bg-primary text-primary-foreground">
        <img src="/desknest-logo.svg" alt="DeskNest" className="size-9" />
      </div>
      <ScrollArea className="no-drag min-h-0 flex-1">
        <nav className="flex flex-col gap-2" aria-label="DeskNest 主导航">
          {navigationItems.map((item) => {
            const Icon = item.icon
            return (
              <Button
                key={item.page}
                asChild
                variant={item.path === activePath ? "default" : "ghost"}
                size="lg"
                className="h-auto w-full flex-col gap-1 py-3"
              >
                <NavLink to={item.path} end={item.path === "/"} title={`${item.label} - ${item.hint}`}>
                  <Icon className="size-5" weight="duotone" />
                  <span className="text-xs">{item.label}</span>
                </NavLink>
              </Button>
            )
          })}
        </nav>
      </ScrollArea>
      <Badge variant="outline" className="no-drag mt-3 justify-center">
        v2
      </Badge>
    </aside>
  )
}

function MobileDock({ activePath }: { activePath: string }) {
  return (
    <nav className="no-drag fixed inset-x-3 bottom-3 z-30 grid grid-cols-5 gap-2 rounded-xl border bg-background p-2 shadow-sm md:hidden" aria-label="DeskNest 移动导航">
      {navigationItems.slice(0, 5).map((item) => {
        const Icon = item.icon
        return (
          <Button key={item.page} asChild variant={item.path === activePath ? "default" : "ghost"} size="sm" className="h-auto flex-col gap-1 py-2">
            <NavLink to={item.path} end={item.path === "/"}>
              <Icon className="size-4" weight="duotone" />
              <span className="text-[11px]">{item.label}</span>
            </NavLink>
          </Button>
        )
      })}
    </nav>
  )
}

function TopBar({
  activePage,
  theme,
  onToggleTheme,
}: {
  activePage: ReturnType<typeof pageFromPath>
  theme: "dark" | "light"
  onToggleTheme: () => void
}) {
  const loading = useDustDeskStore((state) => state.loading)
  const error = useDustDeskStore((state) => state.error)
  const refresh = useDustDeskStore((state) => state.refresh)
  const openSpecial = useDustDeskStore((state) => state.openSpecial)
  const desktopFrames = useDustDeskStore((state) => state.desktopFrames)
  const refreshDesktopFrameVisibility = useDustDeskStore((state) => state.refreshDesktopFrameVisibility)
  const toggleDesktopFrames = useDustDeskStore((state) => state.toggleDesktopFrames)
  const meta = pageMeta[activePage]

  useEffect(() => {
    void refreshDesktopFrameVisibility()
  }, [refreshDesktopFrameVisibility])

  return (
    <header className="window-drag flex min-h-20 shrink-0 items-center justify-between gap-4 px-4 py-3 md:px-6">
      <div className="min-w-0">
        <div className="mb-2 flex items-center gap-2">
          {error ? (
            <Badge variant="destructive" className="gap-1">
              <Warning className="size-3" weight="fill" />
              {truncate(error, 24)}
            </Badge>
          ) : (
            <Badge variant="outline">{loading ? "同步中" : "运行中"}</Badge>
          )}
        </div>
        <h1 className="font-heading text-3xl font-semibold tracking-tight md:text-4xl">{meta.title}</h1>
      </div>

      <div className="no-drag hidden shrink-0 items-center gap-2 xl:flex">
        <Button variant="secondary" size="lg" onClick={onToggleTheme}>
          {theme === "dark" ? <SunDim className="size-4" weight="duotone" /> : <MoonStars className="size-4" weight="duotone" />}
          {theme === "dark" ? "白色" : "黑色"}
        </Button>
        <Button variant="secondary" size="lg" onClick={() => void openSpecial("desktop")}>
          <Crosshair className="size-4" weight="duotone" />
          桌面
        </Button>
        <Button variant="secondary" size="lg" onClick={() => void toggleDesktopFrames()}>
          <Desktop className="size-4" weight="duotone" />
          {desktopFrames.any ? "隐藏桌面框" : "显示桌面框"}
        </Button>
        <Button variant="secondary" size="lg" onClick={() => void refresh()}>
          <ArrowsClockwise className="size-4" weight="duotone" />
          刷新
        </Button>
      </div>
    </header>
  )
}
