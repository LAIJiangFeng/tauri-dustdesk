import { useEffect } from "react"
import { HashRouter } from "react-router"
import { AppRoutes } from "@/app/router"
import { DesktopCardWindowPage } from "@/pages/desktop-card-window-page"
import { DesktopWidgetPage } from "@/pages/desktop-widget-page"
import { useDustDeskStore } from "@/stores/dustdesk-store"

function App() {
  const load = useDustDeskStore((state) => state.load)
  const loadDesktopSnapshot = useDustDeskStore((state) => state.loadDesktopSnapshot)
  const directDesktopRoute = parseDirectDesktopRoute()
  const isDesktopRoute = Boolean(directDesktopRoute)

  useEffect(() => {
    if (isDesktopRoute) {
      void loadDesktopSnapshot({ iconOptions: desktopRouteIconOptions(directDesktopRoute) })
    } else {
      void load()
    }
  }, [isDesktopRoute, load, loadDesktopSnapshot])

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

function parseDirectDesktopRoute() {
  const route = new URLSearchParams(window.location.search).get("dustdeskRoute")?.trim()
  if (!route) return null

  const parts = route.replace(/^\/+/, "").split("/").filter(Boolean)
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
