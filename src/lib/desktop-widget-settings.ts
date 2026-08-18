export const desktopWidgetSettingsStorageKey = "dustdesk-desktop-widget-settings"
export const desktopWidgetSettingsChangedEvent = "dustdesk:desktop-widget-settings-changed"
export const desktopWidgetViewModesStorageKey = "dustdesk-desktop-widget-view-modes"
export const desktopWidgetViewModesChangedEvent = "dustdesk:desktop-widget-view-modes-changed"

export type DesktopWidgetViewMode = "grid" | "list"

export interface DesktopWidgetSettings {
  opacity: number
  iconSize: number
  showNames: boolean
  backgroundColor: string
}

export const defaultDesktopWidgetSettings: DesktopWidgetSettings = {
  opacity: 0.5,
  iconSize: 44,
  showNames: true,
  backgroundColor: "#0f172a",
}

export function readDesktopWidgetSettings(): DesktopWidgetSettings {
  try {
    const parsed = JSON.parse(globalThis.localStorage.getItem(desktopWidgetSettingsStorageKey) || "{}") as Partial<DesktopWidgetSettings>
    return normalizeDesktopWidgetSettings(parsed)
  } catch {
    return defaultDesktopWidgetSettings
  }
}

export function writeDesktopWidgetSettings(settings: DesktopWidgetSettings) {
  const serialized = JSON.stringify(settings)
  if (globalThis.localStorage.getItem(desktopWidgetSettingsStorageKey) === serialized) return
  globalThis.localStorage.setItem(desktopWidgetSettingsStorageKey, serialized)
  globalThis.dispatchEvent(new CustomEvent(desktopWidgetSettingsChangedEvent, { detail: settings }))
}

export function readDesktopWidgetViewModes(): Record<string, DesktopWidgetViewMode> {
  try {
    const parsed = JSON.parse(globalThis.localStorage.getItem(desktopWidgetViewModesStorageKey) || "{}") as Record<string, unknown>
    return Object.fromEntries(Object.entries(parsed).filter((entry): entry is [string, DesktopWidgetViewMode] => entry[1] === "grid" || entry[1] === "list"))
  } catch {
    return {}
  }
}

export function writeDesktopWidgetViewMode(scope: string, viewMode: DesktopWidgetViewMode) {
  const current = readDesktopWidgetViewModes()
  if (current[scope] === viewMode) return
  const next = { ...current, [scope]: viewMode }
  globalThis.localStorage.setItem(desktopWidgetViewModesStorageKey, JSON.stringify(next))
  globalThis.dispatchEvent(new CustomEvent(desktopWidgetViewModesChangedEvent, { detail: next }))
}

export function desktopWidgetCategoryViewScope(categoryName: string, categoryIndex: number) {
  const normalizedName = categoryName.trim()
  return normalizedName ? `category:${normalizedName}` : `category-index:${categoryIndex}`
}

export function desktopWidgetBackgroundColor(settings: DesktopWidgetSettings) {
  const { red, green, blue } = hexToRgb(settings.backgroundColor)
  return `rgb(${red} ${green} ${blue} / ${settings.opacity})`
}

export function normalizeDesktopWidgetColor(value: unknown) {
  if (typeof value !== "string") return defaultDesktopWidgetSettings.backgroundColor
  const normalized = value.trim().toLowerCase()
  return /^#[0-9a-f]{6}$/.test(normalized) ? normalized : defaultDesktopWidgetSettings.backgroundColor
}

function normalizeDesktopWidgetSettings(settings: Partial<DesktopWidgetSettings>): DesktopWidgetSettings {
  return {
    opacity: clamp(Number(settings.opacity ?? defaultDesktopWidgetSettings.opacity), 0.25, 0.85),
    iconSize: clamp(Number(settings.iconSize ?? defaultDesktopWidgetSettings.iconSize), 34, 74),
    showNames: typeof settings.showNames === "boolean" ? settings.showNames : defaultDesktopWidgetSettings.showNames,
    backgroundColor: normalizeDesktopWidgetColor(settings.backgroundColor),
  }
}

function hexToRgb(color: string) {
  const value = Number.parseInt(normalizeDesktopWidgetColor(color).slice(1), 16)
  return {
    red: (value >> 16) & 255,
    green: (value >> 8) & 255,
    blue: value & 255,
  }
}

function clamp(value: number, min: number, max: number) {
  if (!Number.isFinite(value)) return min
  return Math.min(max, Math.max(min, value))
}
