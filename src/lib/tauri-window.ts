import { invoke } from "@tauri-apps/api/core"
import { PhysicalSize } from "@tauri-apps/api/dpi"
import { listen, type EventCallback, type EventName, type UnlistenFn } from "@tauri-apps/api/event"
import { getCurrentWebview, type DragDropEvent } from "@tauri-apps/api/webview"
import { getCurrentWindow } from "@tauri-apps/api/window"

type ResizeDirection = "East" | "North" | "NorthEast" | "NorthWest" | "South" | "SouthEast" | "SouthWest" | "West"
export const desktopWindowLayoutUserChangeEvent = "dustdesk:desktop-window-layout-user-change"

export interface CurrentWindowLayout {
  x: number
  y: number
  width: number
  height: number
}

function hasTauriInternals() {
  return "__TAURI_INTERNALS__" in window
}

export function safeCurrentWindow() {
  try {
    if (!hasTauriInternals()) return null
    return getCurrentWindow()
  } catch {
    return null
  }
}

export async function safeListen<T>(event: EventName, handler: EventCallback<T>): Promise<UnlistenFn | undefined> {
  try {
    if (!hasTauriInternals()) return undefined
    return await listen<T>(event, handler)
  } catch {
    return undefined
  }
}

export async function safeCurrentWebviewDragDropEvent(handler: EventCallback<DragDropEvent>): Promise<UnlistenFn | undefined> {
  const unlisteners: UnlistenFn[] = []
  const recent = new Map<string, number>()

  const dedupedHandler: EventCallback<DragDropEvent> = (event) => {
    const key = dragDropPayloadKey(event.payload)
    const now = Date.now()
    const lastSeen = recent.get(key)
    if (lastSeen && now - lastSeen < 80) return
    recent.set(key, now)
    handler(event)
  }

  try {
    const currentWindow = safeCurrentWindow()
    if (currentWindow) {
      unlisteners.push(await currentWindow.onDragDropEvent(dedupedHandler))
    }
  } catch {
    // Some Tauri surfaces only expose webview-level drag events.
  }

  try {
    if (hasTauriInternals()) {
      unlisteners.push(await getCurrentWebview().onDragDropEvent(dedupedHandler))
    }
  } catch {
    // Window-level listener above is enough for regular WebviewWindow surfaces.
  }

  if (unlisteners.length === 0) return undefined
  return () => {
    unlisteners.forEach((unlisten) => unlisten())
  }
}

function dragDropPayloadKey(payload: DragDropEvent) {
  if (payload.type === "leave") return "leave"
  const position = "position" in payload ? `${payload.position.x},${payload.position.y}` : ""
  const paths = "paths" in payload ? payload.paths.join("\n") : ""
  return `${payload.type}:${position}:${paths}`
}

async function runCurrentWindowAction(action: (currentWindow: NonNullable<ReturnType<typeof safeCurrentWindow>>) => Promise<void>) {
  const currentWindow = safeCurrentWindow()
  if (!currentWindow) return

  try {
    await action(currentWindow)
  } catch {
    // Window controls are optional in browser previews and during early Tauri init.
  }
}

export async function startCurrentWindowDragging() {
  notifyDesktopWindowLayoutUserChange()
  await runCurrentWindowAction((currentWindow) => currentWindow.startDragging())
}

export async function startCurrentWindowResizeDragging(direction: ResizeDirection) {
  notifyDesktopWindowLayoutUserChange()
  await runCurrentWindowAction((currentWindow) => currentWindow.startResizeDragging(direction))
}

function notifyDesktopWindowLayoutUserChange() {
  try {
    window.dispatchEvent(new CustomEvent(desktopWindowLayoutUserChangeEvent))
  } catch {
    // Window layout persistence is best-effort outside a live Tauri webview.
  }
}

export async function getCurrentWindowInnerSize(): Promise<{ width: number; height: number } | null> {
  const currentWindow = safeCurrentWindow()
  if (!currentWindow) return null

  try {
    const size = await currentWindow.innerSize()
    return { width: size.width, height: size.height }
  } catch {
    return null
  }
}

export async function getCurrentWindowLayout(): Promise<CurrentWindowLayout | null> {
  const currentWindow = safeCurrentWindow()
  if (!currentWindow) return null

  try {
    const [position, size] = await Promise.all([currentWindow.outerPosition(), currentWindow.innerSize()])
    return {
      x: position.x,
      y: position.y,
      width: size.width,
      height: size.height,
    }
  } catch {
    return null
  }
}

export async function listenCurrentWindowMove(handler: (position: { x: number; y: number }) => void): Promise<UnlistenFn | undefined> {
  const currentWindow = safeCurrentWindow()
  if (!currentWindow) return undefined

  try {
    return await currentWindow.onMoved((event) => {
      handler({ x: event.payload.x, y: event.payload.y })
    })
  } catch {
    return undefined
  }
}

export async function listenCurrentWindowResize(handler: (size: { width: number; height: number }) => void): Promise<UnlistenFn | undefined> {
  const currentWindow = safeCurrentWindow()
  if (!currentWindow) return undefined

  try {
    return await currentWindow.onResized((event) => {
      handler({ width: event.payload.width, height: event.payload.height })
    })
  } catch {
    return undefined
  }
}

export async function setCurrentWindowSize(width: number, height: number, minWidth = 240, minHeight = 160) {
  await runCurrentWindowAction(async (currentWindow) => {
    await currentWindow.setMinSize(new PhysicalSize(minWidth, minHeight))
    await currentWindow.setSize(new PhysicalSize(width, height))
  })
}

export async function minimizeCurrentWindow() {
  await runCurrentWindowAction((currentWindow) => currentWindow.minimize())
}

export async function toggleCurrentWindowMaximize() {
  await runCurrentWindowAction((currentWindow) => currentWindow.toggleMaximize())
}

export async function closeCurrentWindow() {
  await runCurrentWindowAction((currentWindow) => currentWindow.close())
}

export async function hideMainWindowToTray() {
  try {
    if (hasTauriInternals()) {
      await invoke("hide_main_window_to_tray")
      return
    }
  } catch {
    // Fall back to the native close request; the backend close hook will hide instead of exiting.
  }
  await closeCurrentWindow()
}

export async function repaintCurrentWindow() {
  try {
    if (!hasTauriInternals()) return
    await invoke("repaint_current_window")
  } catch {
    // Transparent WebView repaint nudging is best-effort.
  }
}
