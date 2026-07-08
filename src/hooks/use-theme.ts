import { useEffect, useState } from "react"
import { setTheme as setTauriTheme } from "@tauri-apps/api/app"

export type ThemeMode = "dark" | "light"

const storageKey = "dustdesk-theme"

function storedTheme(): ThemeMode {
  if (typeof window === "undefined") return "dark"
  return window.localStorage.getItem(storageKey) === "light" ? "light" : "dark"
}

function applyTheme(theme: ThemeMode) {
  const root = document.documentElement
  root.dataset.theme = theme
  root.classList.toggle("dark", theme === "dark")
  document.documentElement.style.colorScheme = theme
  document.body.style.colorScheme = theme
  if ("__TAURI_INTERNALS__" in window) {
    void setTauriTheme(theme).catch(() => undefined)
  }
}

export function useTheme() {
  const [theme, setTheme] = useState<ThemeMode>(storedTheme)

  useEffect(() => {
    applyTheme(theme)
    window.localStorage.setItem(storageKey, theme)
  }, [theme])

  useEffect(() => {
    const syncTheme = () => setTheme(storedTheme())
    const onStorage = (event: StorageEvent) => {
      if (event.key === storageKey) syncTheme()
    }
    const onVisibilityChange = () => {
      if (!document.hidden) syncTheme()
    }

    applyTheme(storedTheme())
    window.addEventListener("storage", onStorage)
    window.addEventListener("focus", syncTheme)
    document.addEventListener("visibilitychange", onVisibilityChange)
    return () => {
      window.removeEventListener("storage", onStorage)
      window.removeEventListener("focus", syncTheme)
      document.removeEventListener("visibilitychange", onVisibilityChange)
    }
  }, [])

  return {
    theme,
    setTheme,
    toggleTheme: () => setTheme((value) => (value === "dark" ? "light" : "dark")),
  }
}
