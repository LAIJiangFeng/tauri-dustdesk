import { useEffect } from "react"
import { HashRouter } from "react-router"
import { AppRoutes } from "@/app/router"
import { DesktopCardWindowPage } from "@/pages/desktop-card-window-page"
import { DesktopWidgetPage } from "@/pages/desktop-widget-page"
import { safeCurrentWindow } from "@/lib/tauri-window"
import { useDustDeskStore } from "@/stores/dustdesk-store"

function App() {
  const load = useDustDeskStore((state) => state.load)
  const loadDesktopSnapshot = useDustDeskStore((state) => state.loadDesktopSnapshot)
  const routeParts = currentRouteParts()
  const directDesktopRoute = parseDirectDesktopRoute(routeParts)
  const isDesktopRoute = Boolean(directDesktopRoute)
  const isOverlayRoute = routeParts[0] === "clipboard-overlay" || routeParts[0] === "search-overlay"
  const desktopPage = directDesktopRoute?.page
  const desktopKind = directDesktopRoute?.kind
  const desktopIndex = directDesktopRoute?.index

  useEffect(() => {
    if (isDesktopRoute) {
      void loadDesktopSnapshot({ iconOptions: desktopRouteIconOptions(directDesktopRoute) })
      return
    }

    if (isOverlayRoute) return

    let disposed = false
    void (async () => {
      const currentWindow = safeCurrentWindow()
      if (!currentWindow) {
        if (!disposed) void load()
        return
      }

      let visible = true
      try {
        visible = await currentWindow.isVisible()
      } catch {
        // Loading is safer than leaving a visible main window without data.
      }
      if (!disposed && visible) void load()
    })()

    return () => {
      disposed = true
    }
  }, [desktopIndex, desktopKind, desktopPage, isDesktopRoute, isOverlayRoute, load, loadDesktopSnapshot])

  if (directDesktopRoute?.page === "desktop-widget") {
    return <DesktopWidgetPage />
  }

  if (directDesktopRoute?.page === "desktop-card") {
    return (
      <HashRouter>
        <DesktopCardWindowPage routeKind={directDesktopRoute.kind} routeIndex={directDesktopRoute.index} />
      </HashRouter>
    )
  }

  return (
    <HashRouter>
      <AppRoutes />
    </HashRouter>
  )
}

function currentRouteParts() {
  const routeFromSearch = new URLSearchParams(window.location.search).get("dustdeskRoute")?.trim()
  const route = routeFromSearch || window.location.hash.replace(/^#/, "").trim()
  const path = route.split(/[?#]/, 1)[0]
  return path.replace(/^\/+/, "").split("/").filter(Boolean)
}

function parseDirectDesktopRoute(parts: string[]) {
  if (parts[0] === "desktop-widget") {
    return { page: "desktop-widget" as const }
  }

  if (parts[0] === "desktop-card") {
    return {
      page: "desktop-card" as const,
      kind: parts[1],
      index: parts[2],
    }
  }

  return null
}

function desktopRouteIconOptions(route: ReturnType<typeof parseDirectDesktopRoute>) {
  if (route?.page !== "desktop-card") return { includeDesktopItems: false }
  if (route.kind === "launcher") {
    return { includeDesktopItems: false, includeLaunchers: true, categoryIndices: [] }
  }
  const index = Number(route.index)
  return {
    includeDesktopItems: false,
    includeLaunchers: false,
    categoryIndices: Number.isFinite(index) ? [index] : [],
  }
}

export default App
