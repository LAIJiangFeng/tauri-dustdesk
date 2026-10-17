import { boundsOfPoints, distanceToSegment, rectContains, rectFromPoints, type Point, type Rect } from "@/lib/screenshot/geometry"

// All annotation coordinates and sizes are physical pixels relative to the monitor's top-left corner.

export type DrawTool = "rect" | "ellipse" | "arrow" | "pen" | "mosaic" | "text"

export interface ShapeAnnotation {
  id: string
  kind: "rect" | "ellipse" | "arrow"
  start: Point
  end: Point
  color: string
  width: number
}

export interface PenAnnotation {
  id: string
  kind: "pen"
  points: Point[]
  color: string
  width: number
}

export interface MosaicAnnotation {
  id: string
  kind: "mosaic"
  points: Point[]
  brush: number
  block: number
}

export interface TextAnnotation {
  id: string
  kind: "text"
  x: number
  y: number
  text: string
  color: string
  fontSize: number
}

export type Annotation = ShapeAnnotation | PenAnnotation | MosaicAnnotation | TextAnnotation

export const TEXT_FONT_FAMILY = '"Microsoft YaHei UI", "Microsoft YaHei", "Segoe UI", sans-serif'
export const TEXT_LINE_HEIGHT = 1.3
/** Padding (in font-size units) between a text annotation's anchor and its first glyph, mirrored by the editor. */
export const TEXT_PADDING = 0.2

let idCounter = 0
export function nextAnnotationId() {
  idCounter += 1
  return `a${Date.now().toString(36)}${idCounter}`
}

export function textFont(fontSize: number) {
  return `${fontSize}px ${TEXT_FONT_FAMILY}`
}

let measureContext: CanvasRenderingContext2D | null = null
function getMeasureContext() {
  if (!measureContext) measureContext = document.createElement("canvas").getContext("2d")
  return measureContext
}

export function textBounds(annotation: TextAnnotation): Rect {
  const lines = annotation.text.split("\n")
  const context = getMeasureContext()
  let width = 0
  if (context) {
    context.font = textFont(annotation.fontSize)
    for (const line of lines) width = Math.max(width, context.measureText(line).width)
  } else {
    width = Math.max(...lines.map((line) => line.length)) * annotation.fontSize
  }
  const padding = annotation.fontSize * TEXT_PADDING
  return {
    x: annotation.x,
    y: annotation.y,
    width: width + padding * 2,
    height: lines.length * annotation.fontSize * TEXT_LINE_HEIGHT + padding * 2,
  }
}

export function annotationBounds(annotation: Annotation): Rect {
  switch (annotation.kind) {
    case "rect":
    case "ellipse":
    case "arrow": {
      const rect = rectFromPoints(annotation.start, annotation.end)
      const pad = annotation.width * (annotation.kind === "arrow" ? 3 : 1)
      return { x: rect.x - pad, y: rect.y - pad, width: rect.width + pad * 2, height: rect.height + pad * 2 }
    }
    case "pen":
      return boundsOfPoints(annotation.points, annotation.width)
    case "mosaic":
      return boundsOfPoints(annotation.points, annotation.brush / 2)
    case "text":
      return textBounds(annotation)
  }
}

export function hitTestAnnotation(annotation: Annotation, point: Point, tolerance: number) {
  switch (annotation.kind) {
    case "rect": {
      const rect = rectFromPoints(annotation.start, annotation.end)
      const pad = tolerance + annotation.width / 2
      if (!rectContains(rect, point, pad)) return false
      const inner = { x: rect.x + pad, y: rect.y + pad, width: rect.width - pad * 2, height: rect.height - pad * 2 }
      return inner.width <= 0 || inner.height <= 0 || !rectContains(inner, point)
    }
    case "ellipse": {
      const rect = rectFromPoints(annotation.start, annotation.end)
      const rx = rect.width / 2
      const ry = rect.height / 2
      if (rx < 1 || ry < 1) return rectContains(rect, point, tolerance)
      const nx = (point.x - (rect.x + rx)) / rx
      const ny = (point.y - (rect.y + ry)) / ry
      const distance = Math.sqrt(nx * nx + ny * ny)
      const band = (tolerance + annotation.width / 2) / Math.min(rx, ry)
      return Math.abs(distance - 1) <= band
    }
    case "arrow":
      return distanceToSegment(point, annotation.start, annotation.end) <= tolerance + annotation.width * 1.5
    case "pen": {
      const limit = tolerance + annotation.width / 2
      const points = annotation.points
      if (points.length === 1) return Math.hypot(point.x - points[0].x, point.y - points[0].y) <= limit
      for (let index = 1; index < points.length; index += 1) {
        if (distanceToSegment(point, points[index - 1], points[index]) <= limit) return true
      }
      return false
    }
    case "mosaic":
      return false
    case "text":
      return rectContains(textBounds(annotation), point, tolerance)
  }
}

export function translateAnnotation(annotation: Annotation, dx: number, dy: number): Annotation {
  switch (annotation.kind) {
    case "rect":
    case "ellipse":
    case "arrow":
      return {
        ...annotation,
        start: { x: annotation.start.x + dx, y: annotation.start.y + dy },
        end: { x: annotation.end.x + dx, y: annotation.end.y + dy },
      }
    case "pen":
    case "mosaic":
      return { ...annotation, points: annotation.points.map((point) => ({ x: point.x + dx, y: point.y + dy })) }
    case "text":
      return { ...annotation, x: annotation.x + dx, y: annotation.y + dy }
  }
}

export function annotationHasArea(annotation: Annotation) {
  switch (annotation.kind) {
    case "rect":
    case "ellipse":
    case "arrow":
      return Math.hypot(annotation.end.x - annotation.start.x, annotation.end.y - annotation.start.y) >= 3
    case "pen":
    case "mosaic":
      return annotation.points.length > 0
    case "text":
      return annotation.text.trim().length > 0
  }
}

/** Source used to build mosaic blocks: always the original captured pixels. */
export interface MosaicSource {
  image: CanvasImageSource
  width: number
  height: number
}

export class MosaicRenderer {
  private readonly source: MosaicSource
  private readonly pixelated = new Map<number, HTMLCanvasElement>()
  private readonly cache = new WeakMap<MosaicAnnotation, { canvas: HTMLCanvasElement; x: number; y: number; points: number }>()

  constructor(source: MosaicSource) {
    this.source = source
  }

  private pixelatedFor(block: number) {
    const cached = this.pixelated.get(block)
    if (cached) return cached
    const canvas = document.createElement("canvas")
    canvas.width = Math.max(1, Math.ceil(this.source.width / block))
    canvas.height = Math.max(1, Math.ceil(this.source.height / block))
    const context = canvas.getContext("2d")
    if (context) {
      context.imageSmoothingEnabled = true
      context.imageSmoothingQuality = "high"
      context.drawImage(
        this.source.image,
        0,
        0,
        canvas.width * block,
        canvas.height * block,
        0,
        0,
        canvas.width,
        canvas.height,
      )
    }
    this.pixelated.set(block, canvas)
    return canvas
  }

  private render(annotation: MosaicAnnotation) {
    const bounds = boundsOfPoints(annotation.points, annotation.brush / 2 + 1)
    const x = Math.floor(bounds.x)
    const y = Math.floor(bounds.y)
    const canvas = document.createElement("canvas")
    canvas.width = Math.max(1, Math.ceil(bounds.width) + 2)
    canvas.height = Math.max(1, Math.ceil(bounds.height) + 2)
    const context = canvas.getContext("2d")
    if (!context) return { canvas, x, y, points: annotation.points.length }

    context.translate(-x, -y)
    context.lineCap = "round"
    context.lineJoin = "round"
    context.lineWidth = annotation.brush
    context.strokeStyle = "#000"
    context.fillStyle = "#000"
    const points = annotation.points
    if (points.length === 1) {
      context.beginPath()
      context.arc(points[0].x, points[0].y, annotation.brush / 2, 0, Math.PI * 2)
      context.fill()
    } else {
      context.beginPath()
      context.moveTo(points[0].x, points[0].y)
      for (let index = 1; index < points.length; index += 1) context.lineTo(points[index].x, points[index].y)
      context.stroke()
    }

    const pixelated = this.pixelatedFor(annotation.block)
    context.globalCompositeOperation = "source-in"
    context.imageSmoothingEnabled = false
    context.drawImage(pixelated, 0, 0, pixelated.width * annotation.block, pixelated.height * annotation.block)
    return { canvas, x, y, points: annotation.points.length }
  }

  draw(context: CanvasRenderingContext2D, annotation: MosaicAnnotation) {
    let entry = this.cache.get(annotation)
    // Live strokes mutate their point list, so rebuild whenever it grew.
    if (!entry || entry.points !== annotation.points.length) {
      entry = this.render(annotation)
      this.cache.set(annotation, entry)
    }
    context.drawImage(entry.canvas, entry.x, entry.y)
  }
}

function drawArrow(context: CanvasRenderingContext2D, annotation: ShapeAnnotation) {
  const { start, end, width } = annotation
  const angle = Math.atan2(end.y - start.y, end.x - start.x)
  const length = Math.hypot(end.x - start.x, end.y - start.y)
  const headLength = Math.min(Math.max(width * 4.5, 12), length * 0.6)
  const headWidth = headLength * 0.6
  const baseX = end.x - Math.cos(angle) * headLength
  const baseY = end.y - Math.sin(angle) * headLength
  const normalX = -Math.sin(angle)
  const normalY = Math.cos(angle)

  context.beginPath()
  context.moveTo(start.x, start.y)
  context.lineTo(end.x - Math.cos(angle) * headLength * 0.5, end.y - Math.sin(angle) * headLength * 0.5)
  context.lineWidth = width
  context.stroke()

  context.beginPath()
  context.moveTo(end.x, end.y)
  context.lineTo(baseX + normalX * headWidth, baseY + normalY * headWidth)
  context.lineTo(baseX + (end.x - baseX) * 0.25, baseY + (end.y - baseY) * 0.25)
  context.lineTo(baseX - normalX * headWidth, baseY - normalY * headWidth)
  context.closePath()
  context.fill()
}

function drawSmoothPath(context: CanvasRenderingContext2D, points: Point[], width: number) {
  if (points.length === 0) return
  if (points.length === 1) {
    context.beginPath()
    context.arc(points[0].x, points[0].y, width / 2, 0, Math.PI * 2)
    context.fill()
    return
  }
  context.beginPath()
  context.moveTo(points[0].x, points[0].y)
  for (let index = 1; index < points.length - 1; index += 1) {
    const midX = (points[index].x + points[index + 1].x) / 2
    const midY = (points[index].y + points[index + 1].y) / 2
    context.quadraticCurveTo(points[index].x, points[index].y, midX, midY)
  }
  const last = points[points.length - 1]
  context.lineTo(last.x, last.y)
  context.lineWidth = width
  context.stroke()
}

export function drawAnnotation(context: CanvasRenderingContext2D, annotation: Annotation, mosaic: MosaicRenderer | null) {
  context.save()
  context.lineCap = "round"
  context.lineJoin = "round"
  switch (annotation.kind) {
    case "rect": {
      const rect = rectFromPoints(annotation.start, annotation.end)
      context.strokeStyle = annotation.color
      context.lineWidth = annotation.width
      context.strokeRect(rect.x, rect.y, rect.width, rect.height)
      break
    }
    case "ellipse": {
      const rect = rectFromPoints(annotation.start, annotation.end)
      context.strokeStyle = annotation.color
      context.lineWidth = annotation.width
      context.beginPath()
      context.ellipse(rect.x + rect.width / 2, rect.y + rect.height / 2, rect.width / 2, rect.height / 2, 0, 0, Math.PI * 2)
      context.stroke()
      break
    }
    case "arrow":
      context.strokeStyle = annotation.color
      context.fillStyle = annotation.color
      drawArrow(context, annotation)
      break
    case "pen":
      context.strokeStyle = annotation.color
      context.fillStyle = annotation.color
      drawSmoothPath(context, annotation.points, annotation.width)
      break
    case "mosaic":
      mosaic?.draw(context, annotation)
      break
    case "text": {
      const padding = annotation.fontSize * TEXT_PADDING
      const lineHeight = annotation.fontSize * TEXT_LINE_HEIGHT
      context.font = textFont(annotation.fontSize)
      context.fillStyle = annotation.color
      context.textBaseline = "middle"
      annotation.text.split("\n").forEach((line, index) => {
        context.fillText(line, annotation.x + padding, annotation.y + padding + lineHeight * (index + 0.5))
      })
      break
    }
  }
  context.restore()
}

export function drawSelectionOutline(context: CanvasRenderingContext2D, annotation: Annotation, scale: number) {
  const bounds = annotationBounds(annotation)
  const pad = 3 * scale
  context.save()
  context.setLineDash([4 * scale, 3 * scale])
  context.lineWidth = Math.max(1, scale)
  context.strokeStyle = "rgba(255,255,255,0.95)"
  context.strokeRect(bounds.x - pad, bounds.y - pad, bounds.width + pad * 2, bounds.height + pad * 2)
  context.lineDashOffset = 3.5 * scale
  context.strokeStyle = "rgba(0,0,0,0.65)"
  context.strokeRect(bounds.x - pad, bounds.y - pad, bounds.width + pad * 2, bounds.height + pad * 2)
  context.restore()
}

/** Composes "original pixels inside selection + all annotations" at physical resolution. */
export async function exportSelectionPng(
  image: CanvasImageSource,
  selection: Rect,
  annotations: Annotation[],
  mosaic: MosaicRenderer | null,
): Promise<Uint8Array> {
  const canvas = document.createElement("canvas")
  canvas.width = Math.max(1, Math.round(selection.width))
  canvas.height = Math.max(1, Math.round(selection.height))
  const context = canvas.getContext("2d")
  if (!context) throw new Error("无法创建截图画布")
  context.imageSmoothingEnabled = false
  context.drawImage(image, -Math.round(selection.x), -Math.round(selection.y))
  context.translate(-Math.round(selection.x), -Math.round(selection.y))
  for (const annotation of annotations) drawAnnotation(context, annotation, mosaic)

  const blob = await new Promise<Blob | null>((resolve) => canvas.toBlob(resolve, "image/png"))
  if (!blob) throw new Error("截图导出失败")
  return new Uint8Array(await blob.arrayBuffer())
}
