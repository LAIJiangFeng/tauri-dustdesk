import { Navigate, Route, Routes } from "react-router"
import { AppShell } from "@/layouts/app-shell"
import { ClipboardOverlayPage } from "@/pages/clipboard-overlay-page"
import { ClipboardPage } from "@/pages/clipboard-page"
import { DesktopCardWindowPage } from "@/pages/desktop-card-window-page"
import { DesktopWidgetPage } from "@/pages/desktop-widget-page"
import { HomePage } from "@/pages/home-page"
import { LauncherPage } from "@/pages/launcher-page"
import { OrganizerPage } from "@/pages/organizer-page"
import { SearchOverlayPage } from "@/pages/search-overlay-page"
import { SearchPage } from "@/pages/search-page"
import { SettingsPage } from "@/pages/settings-page"

export function AppRoutes() {
  return (
    <Routes>
      <Route path="clipboard-overlay" element={<ClipboardOverlayPage />} />
      <Route path="search-overlay" element={<SearchOverlayPage />} />
      <Route path="desktop-widget" element={<DesktopWidgetPage />} />
      <Route path="desktop-card/:kind" element={<DesktopCardWindowPage />} />
      <Route path="desktop-card/:kind/:index" element={<DesktopCardWindowPage />} />
      <Route element={<AppShell />}>
        <Route index element={<HomePage />} />
        <Route path="organizer" element={<OrganizerPage />} />
        <Route path="launcher" element={<LauncherPage />} />
        <Route path="clipboard" element={<ClipboardPage />} />
        <Route path="search" element={<SearchPage />} />
        <Route path="settings" element={<SettingsPage />} />
        <Route path="*" element={<Navigate to="/" replace />} />
      </Route>
    </Routes>
  )
}
