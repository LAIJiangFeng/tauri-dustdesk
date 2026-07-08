export const dustdeskPathDragType = "application/x-dustdesk-path"
const nativeFileDragType = "Files"

interface DragEndPoint {
  clientX: number
  clientY: number
  screenX: number
  screenY: number
  dataTransfer: DataTransfer
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
  return types.includes(dustdeskPathDragType) || types.includes(nativeFileDragType)
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
  const path = dataTransfer.getData(dustdeskPathDragType).trim()
  return path ? [path] : []
}

export function didDragEndOutsideWindow(event: DragEndPoint) {
  if (event.dataTransfer.dropEffect === "copy") return false

  if (
    event.clientX < 0 ||
    event.clientY < 0 ||
    event.clientX > globalThis.innerWidth ||
    event.clientY > globalThis.innerHeight
  ) {
    return true
  }

  const windowLeft = globalThis.screenX
  const windowTop = globalThis.screenY
  return (
    event.screenX < windowLeft ||
    event.screenY < windowTop ||
    event.screenX > windowLeft + globalThis.outerWidth ||
    event.screenY > windowTop + globalThis.outerHeight
  )
}
