import { StrictMode } from "react"
import { createRoot } from "react-dom/client"
import App from "./App"
import { AppErrorBoundary } from "./components/app-error-boundary"
import "./styles.css"

const routeFromSearch = new URLSearchParams(window.location.search).get("dustdeskRoute")
if (routeFromSearch && !window.location.hash) {
  window.location.hash = `#/${routeFromSearch.replace(/^\/+/, "")}`
}

if (window.location.hash.startsWith("#/desktop-widget") || window.location.hash.startsWith("#/desktop-card")) {
  document.documentElement.classList.add("desktop-widget-root")
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <AppErrorBoundary>
      <App />
    </AppErrorBoundary>
  </StrictMode>,
)
