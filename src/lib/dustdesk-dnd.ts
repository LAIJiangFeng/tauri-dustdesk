export const dustdeskPathDragType = "application/x-dustdesk-path"
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

export interface DesktopDropPosition {
  screenX: number
  screenY: number
  scaleFactor: number
}

export function writeDustDeskPathDrag(dataTransfer: DataTransfer, path: string, effectAllowed: DataTransfer["effectAllowed"] = "copy") {
  dataTransfer.effectAllowed = effectAllowed
  dataTransfer.setData(dustdeskPathDragType, path)
  dataTransfer.setData("text/plain", path)
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
  if (event.dataTransfer.dropEffect === "copy") return false

  if (event.clientX < 0 || event.clientY < 0 || event.clientX > globalThis.innerWidth || event.clientY > globalThis.innerHeight) {
    return true
  }

  const windowLeft = globalThis.screenX
  const windowTop = globalThis.screenY
  return event.screenX < windowLeft || event.screenY < windowTop || event.screenX > windowLeft + globalThis.outerWidth || event.screenY > windowTop + globalThis.outerHeight
}

export function desktopDropPositionFromDragEnd(event: DragEndPoint): DesktopDropPosition {
  return {
    screenX: finiteNumber(event.screenX),
    screenY: finiteNumber(event.screenY),
    scaleFactor: Math.max(0.25, finiteNumber(globalThis.devicePixelRatio || 1, 1)),
  }
}

function finiteNumber(value: number, fallback = 0) {
  return Number.isFinite(value) ? value : fallback
}
