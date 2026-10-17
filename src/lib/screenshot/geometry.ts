export interface Point {
  x: number
  y: number
}

export interface Rect {
  x: number
  y: number
  width: number
  height: number
}

export type HandleDirection = "n" | "s" | "e" | "w" | "ne" | "nw" | "se" | "sw"

export const HANDLE_DIRECTIONS: HandleDirection[] = ["nw", "n", "ne", "e", "se", "s", "sw", "w"]

export function clamp(value: number, min: number, max: number) {
  return Math.min(Math.max(value, min), max)
}

export function rectFromPoints(a: Point, b: Point): Rect {
  const x = Math.min(a.x, b.x)
  const y = Math.min(a.y, b.y)
  return { x, y, width: Math.abs(a.x - b.x), height: Math.abs(a.y - b.y) }
}

export function roundRect(rect: Rect): Rect {
  const x = Math.round(rect.x)
  const y = Math.round(rect.y)
  return {
    x,
    y,
    width: Math.max(0, Math.round(rect.x + rect.width) - x),
    height: Math.max(0, Math.round(rect.y + rect.height) - y),
  }
}

export function clampPoint(point: Point, bounds: Rect): Point {
  return {
    x: clamp(point.x, bounds.x, bounds.x + bounds.width),
    y: clamp(point.y, bounds.y, bounds.y + bounds.height),
  }
}

export function clampRect(rect: Rect, bounds: Rect): Rect {
  const x1 = clamp(rect.x, bounds.x, bounds.x + bounds.width)
  const y1 = clamp(rect.y, bounds.y, bounds.y + bounds.height)
  const x2 = clamp(rect.x + rect.width, bounds.x, bounds.x + bounds.width)
  const y2 = clamp(rect.y + rect.height, bounds.y, bounds.y + bounds.height)
  return { x: x1, y: y1, width: Math.max(0, x2 - x1), height: Math.max(0, y2 - y1) }
}

/** Moves a rect by delta while keeping it fully inside bounds (size is preserved). */
export function moveRectWithin(rect: Rect, dx: number, dy: number, bounds: Rect): Rect {
  return {
    ...rect,
    x: clamp(rect.x + dx, bounds.x, bounds.x + bounds.width - rect.width),
    y: clamp(rect.y + dy, bounds.y, bounds.y + bounds.height - rect.height),
  }
}

export function rectContains(rect: Rect, point: Point, padding = 0) {
  return (
    point.x >= rect.x - padding &&
    point.x <= rect.x + rect.width + padding &&
    point.y >= rect.y - padding &&
    point.y <= rect.y + rect.height + padding
  )
}

export function handlePoint(rect: Rect, direction: HandleDirection): Point {
  const cx = rect.x + rect.width / 2
  const cy = rect.y + rect.height / 2
  const right = rect.x + rect.width
  const bottom = rect.y + rect.height
  switch (direction) {
    case "n":
      return { x: cx, y: rect.y }
    case "s":
      return { x: cx, y: bottom }
    case "e":
      return { x: right, y: cy }
    case "w":
      return { x: rect.x, y: cy }
    case "ne":
      return { x: right, y: rect.y }
    case "nw":
      return { x: rect.x, y: rect.y }
    case "se":
      return { x: right, y: bottom }
    case "sw":
      return { x: rect.x, y: bottom }
  }
}

export function hitHandle(rect: Rect, point: Point, radius: number): HandleDirection | null {
  for (const direction of HANDLE_DIRECTIONS) {
    const handle = handlePoint(rect, direction)
    if (Math.abs(handle.x - point.x) <= radius && Math.abs(handle.y - point.y) <= radius) return direction
  }
  return null
}

/** Resizes `origin` by dragging the given handle to `point`; edges may cross and the rect is re-normalized. */
export function resizeRect(origin: Rect, direction: HandleDirection, point: Point, bounds: Rect): Rect {
  const clamped = clampPoint(point, bounds)
  let x1 = origin.x
  let y1 = origin.y
  let x2 = origin.x + origin.width
  let y2 = origin.y + origin.height
  if (direction.includes("w")) x1 = clamped.x
  if (direction.includes("e")) x2 = clamped.x
  if (direction.includes("n")) y1 = clamped.y
  if (direction.includes("s")) y2 = clamped.y
  return rectFromPoints({ x: x1, y: y1 }, { x: x2, y: y2 })
}

export function handleCursor(direction: HandleDirection) {
  switch (direction) {
    case "n":
    case "s":
      return "ns-resize"
    case "e":
    case "w":
      return "ew-resize"
    case "ne":
    case "sw":
      return "nesw-resize"
    case "nw":
    case "se":
      return "nwse-resize"
  }
}

export function distanceToSegment(point: Point, a: Point, b: Point) {
  const dx = b.x - a.x
  const dy = b.y - a.y
  const lengthSquared = dx * dx + dy * dy
  if (lengthSquared === 0) return Math.hypot(point.x - a.x, point.y - a.y)
  const t = clamp(((point.x - a.x) * dx + (point.y - a.y) * dy) / lengthSquared, 0, 1)
  return Math.hypot(point.x - (a.x + t * dx), point.y - (a.y + t * dy))
}

export function boundsOfPoints(points: Point[], padding = 0): Rect {
  let minX = Number.POSITIVE_INFINITY
  let minY = Number.POSITIVE_INFINITY
  let maxX = Number.NEGATIVE_INFINITY
  let maxY = Number.NEGATIVE_INFINITY
  for (const point of points) {
    minX = Math.min(minX, point.x)
    minY = Math.min(minY, point.y)
    maxX = Math.max(maxX, point.x)
    maxY = Math.max(maxY, point.y)
  }
  if (!Number.isFinite(minX)) return { x: 0, y: 0, width: 0, height: 0 }
  return { x: minX - padding, y: minY - padding, width: maxX - minX + padding * 2, height: maxY - minY + padding * 2 }
}
