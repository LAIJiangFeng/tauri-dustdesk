import { invoke } from "@tauri-apps/api/core"

export const dustdeskPathDragType = "application/x-dustdesk-path"
const dustdeskDragSessionType = "application/x-dustdesk-drag-session"
const dustdeskActiveDragStorageKey = "dustdesk-active-path-drag"
const dustdeskAcceptedDragStorageKey = "dustdesk-accepted-path-drag"
const dragSessionMaxAgeMs = 15_000
const nativeFileDragType = "Files"
const plainTextDragType = "text/plain"
const uriListDragType = "text/uri-list"

interface DragEndPoint {
  clientX: number
  clientY: number
  screenX: number
  screenY: number
  dataTransfer: DataTransfer
}

interface DustDeskDragSession {
  id: string
  path: string
  createdAt: number
}

export interface DesktopDropPosition {
  screenX: number
  screenY: number
  scaleFactor: number
  dragSessionId?: string
}

export interface InternalPathDragOutcome {
  action: "moved_to_category" | "added_to_launcher" | "accepted_by_box" | "restored_to_desktop"
  target_category_index?: number | null
  affected: number
  restored_path?: string | null
}

export function writeDustDeskPathDrag(dataTransfer: DataTransfer, path: string, effectAllowed: DataTransfer["effectAllowed"] = "copy") {
  const session: DustDeskDragSession = {
    id: `${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`,
    path,
    createdAt: Date.now(),
  }
  dataTransfer.effectAllowed = effectAllowed
  dataTransfer.setData(dustdeskPathDragType, path)
  dataTransfer.setData(dustdeskDragSessionType, session.id)
  dataTransfer.setData("text/plain", path)
  writeStoredDragSession(dustdeskActiveDragStorageKey, session)
  return session.id
}

export function hasDustDeskPathDrag(dataTransfer: DataTransfer) {
  return Array.from(dataTransfer.types).includes(dustdeskPathDragType)
}

export function hasPathLikeDrag(dataTransfer: DataTransfer | null) {
  if (!dataTransfer) return false
  const types = Array.from(dataTransfer.types)
  return types.includes(dustdeskPathDragType) || types.includes(nativeFileDragType) || types.includes(plainTextDragType) || types.includes(uriListDragType)
}

export function allowPathLikeDrag(event: DragEvent) {
  if (!hasPathLikeDrag(event.dataTransfer)) return false
  event.preventDefault()
  if (event.dataTransfer) {
    event.dataTransfer.dropEffect = "copy"
  }
  return true
}

export function readDustDeskPathDrag(dataTransfer: DataTransfer) {
  const paths = [
    ...pathsFromText(dataTransfer.getData(dustdeskPathDragType)),
    ...pathsFromText(dataTransfer.getData(plainTextDragType)),
    ...pathsFromUriList(dataTransfer.getData(uriListDragType)),
  ]
  return Array.from(new Set(paths))
}

function pathsFromText(value: string) {
  return value
    .split(/\r?\n/)
    .map((path) => path.trim().replace(/^"|"$/g, ""))
    .filter(isLocalPathText)
}

function pathsFromUriList(value: string) {
  return value
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter((line) => line && !line.startsWith("#"))
    .map(fileUriToWindowsPath)
    .filter(isLocalPathText)
}

function fileUriToWindowsPath(value: string) {
  if (!value.toLowerCase().startsWith("file:")) return value
  try {
    const url = new URL(value)
    let path = decodeURIComponent(url.pathname)
    if (/^\/[a-zA-Z]:\//.test(path)) path = path.slice(1)
    return path.replace(/\//g, "\\")
  } catch {
    return value
  }
}

function isLocalPathText(value: string) {
  return /^[a-zA-Z]:[\\/]/.test(value) || value.startsWith("\\\\")
}

export function didDragEndOutsideWindow(event: DragEndPoint) {
  // A move outside may be Windows Explorer receiving the item, so only skip the legacy copy case.
  // Internal move targets use the drag-session acceptance handshake before any desktop restore.
  if (event.dataTransfer.dropEffect === "copy") return false

  if (event.clientX < 0 || event.clientY < 0 || event.clientX > globalThis.innerWidth || event.clientY > globalThis.innerHeight) {
    return true
  }

  const windowLeft = globalThis.screenX
  const windowTop = globalThis.screenY
  return event.screenX < windowLeft || event.screenY < windowTop || event.screenX > windowLeft + globalThis.outerWidth || event.screenY > windowTop + globalThis.outerHeight
}

export function markDustDeskPathDropAccepted(dataTransfer: DataTransfer | null, paths: string[]) {
  const sessionId = dataTransfer ? safeGetData(dataTransfer, dustdeskDragSessionType) : ""
  const active = readStoredDragSession(dustdeskActiveDragStorageKey)
  const matchedActive = active && paths.some((path) => sameDragPath(path, active.path)) ? active : null
  const id = sessionId || matchedActive?.id || ""
  notifyBackendPathDropAccepted(paths, id)
  if (!id) return
  writeStoredDragSession(dustdeskAcceptedDragStorageKey, {
    id,
    path: matchedActive?.path ?? paths[0] ?? "",
    createdAt: Date.now(),
  })
}

export function waitForDustDeskPathDropAcceptance(dataTransfer: DataTransfer, path: string, delayMs = 180) {
  const sessionId = safeGetData(dataTransfer, dustdeskDragSessionType)
  const active = readStoredDragSession(dustdeskActiveDragStorageKey)
  const expectedId = sessionId || (active && sameDragPath(active.path, path) ? active.id : "")

  return new Promise<boolean>((resolve) => {
    globalThis.setTimeout(() => {
      const accepted = readStoredDragSession(dustdeskAcceptedDragStorageKey)
      const wasAccepted = Boolean(expectedId && accepted?.id === expectedId)
      clearStoredDragSession(dustdeskActiveDragStorageKey, expectedId)
      clearStoredDragSession(dustdeskAcceptedDragStorageKey, expectedId)
      resolve(wasAccepted)
    }, delayMs)
  })
}

export function desktopDropPositionFromDragEnd(event: DragEndPoint): DesktopDropPosition {
  const active = readStoredDragSession(dustdeskActiveDragStorageKey)
  const dragSessionId = safeGetData(event.dataTransfer, dustdeskDragSessionType) || active?.id || ""
  return {
    screenX: finiteNumber(event.screenX),
    screenY: finiteNumber(event.screenY),
    scaleFactor: Math.max(0.25, finiteNumber(globalThis.devicePixelRatio || 1, 1)),
    ...(dragSessionId ? { dragSessionId } : {}),
  }
}

export function registerDustDeskDropCategory(index: number | null) {
  if (!isTauriRuntime()) return
  void invoke("register_internal_path_drop_category", { index }).catch(() => undefined)
}

function notifyBackendPathDropAccepted(paths: string[], sessionId: string) {
  if (!isTauriRuntime() || paths.length === 0) return
  void invoke("mark_internal_path_drop_accepted", {
    paths,
    sessionId: sessionId || null,
  }).catch(() => undefined)
}

function isTauriRuntime() {
  return "__TAURI_INTERNALS__" in globalThis
}

function finiteNumber(value: number, fallback = 0) {
  return Number.isFinite(value) ? value : fallback
}

function safeGetData(dataTransfer: DataTransfer, type: string) {
  try {
    return dataTransfer.getData(type).trim()
  } catch {
    return ""
  }
}

function sameDragPath(left: string, right: string) {
  const normalize = (value: string) => value.trim().replace(/\//g, "\\").replace(/\\+$/, "").toLowerCase()
  return normalize(left) === normalize(right)
}

function writeStoredDragSession(key: string, session: DustDeskDragSession) {
  try {
    globalThis.localStorage?.setItem(key, JSON.stringify(session))
  } catch {
    // Dragging must continue even when storage is unavailable.
  }
}

function readStoredDragSession(key: string) {
  try {
    const value = globalThis.localStorage?.getItem(key)
    if (!value) return null
    const session = JSON.parse(value) as Partial<DustDeskDragSession>
    if (typeof session.id !== "string" || typeof session.path !== "string" || typeof session.createdAt !== "number") return null
    if (Date.now() - session.createdAt > dragSessionMaxAgeMs) {
      globalThis.localStorage?.removeItem(key)
      return null
    }
    return session as DustDeskDragSession
  } catch {
    return null
  }
}

function clearStoredDragSession(key: string, expectedId: string) {
  if (!expectedId) return
  const session = readStoredDragSession(key)
  if (session?.id !== expectedId) return
  try {
    globalThis.localStorage?.removeItem(key)
  } catch {
    // Ignore storage cleanup failures.
  }
}
