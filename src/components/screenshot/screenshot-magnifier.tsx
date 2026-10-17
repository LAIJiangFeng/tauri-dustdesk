import { useImperativeHandle, useRef } from "react"
import type { Ref } from "react"
import type { Point } from "@/lib/screenshot/geometry"
import type { PixelReader } from "@/lib/screenshot/api"

const ZOOM_CELLS = 15
const CELL_CSS = 8
const LENS_CSS = ZOOM_CELLS * CELL_CSS
const OFFSET = 18

export interface MagnifierSource {
  image: CanvasImageSource
  pixels: PixelReader | null
  width: number
  height: number
  scale: number
}

export interface MagnifierHandle {
  /** `point` is physical pixels relative to the monitor; pass null to hide. */
  update: (point: Point | null, selectionSize?: { width: number; height: number } | null) => void
  colorAt: (point: Point) => string
  setHexMode: (hex: boolean) => void
}

function hex(value: number) {
  return value.toString(16).padStart(2, "0").toUpperCase()
}

export function ScreenshotMagnifier({ handleRef, source }: { handleRef: Ref<MagnifierHandle>; source: MagnifierSource | null }) {
  const rootRef = useRef<HTMLDivElement | null>(null)
  const canvasRef = useRef<HTMLCanvasElement | null>(null)
  const positionRef = useRef<HTMLSpanElement | null>(null)
  const colorRef = useRef<HTMLSpanElement | null>(null)
  const swatchRef = useRef<HTMLSpanElement | null>(null)
  const hexModeRef = useRef(false)
  const lastPointRef = useRef<Point | null>(null)

  const rgbAt = (point: Point): [number, number, number] | null => {
    if (!source?.pixels) return null
    const x = Math.min(Math.max(Math.floor(point.x), 0), source.width - 1)
    const y = Math.min(Math.max(Math.floor(point.y), 0), source.height - 1)
    return source.pixels(x, y)
  }

  const formatColor = (rgb: [number, number, number] | null, hexMode: boolean) => {
    if (!rgb) return "--"
    return hexMode ? `#${hex(rgb[0])}${hex(rgb[1])}${hex(rgb[2])}` : `RGB(${rgb[0]}, ${rgb[1]}, ${rgb[2]})`
  }

  const render = (point: Point | null, selectionSize?: { width: number; height: number } | null) => {
    const root = rootRef.current
    const canvas = canvasRef.current
    lastPointRef.current = point
    if (!root || !canvas || !source || !point) {
      if (root) root.style.display = "none"
      return
    }
    root.style.display = "block"

    const cssX = point.x / source.scale
    const cssY = point.y / source.scale
    const boxWidth = LENS_CSS
    const boxHeight = root.offsetHeight || LENS_CSS + 56
    const viewportWidth = window.innerWidth
    const viewportHeight = window.innerHeight
    let left = cssX + OFFSET
    let top = cssY + OFFSET
    if (left + boxWidth > viewportWidth - 2) left = cssX - OFFSET - boxWidth
    if (top + boxHeight > viewportHeight - 2) top = cssY - OFFSET - boxHeight
    root.style.transform = `translate(${Math.max(2, left)}px, ${Math.max(2, top)}px)`

    const dpr = window.devicePixelRatio || 1
    const pixelSize = Math.round(LENS_CSS * dpr)
    if (canvas.width !== pixelSize) {
      canvas.width = pixelSize
      canvas.height = pixelSize
    }
    const context = canvas.getContext("2d")
    if (context) {
      const half = Math.floor(ZOOM_CELLS / 2)
      const sx = Math.floor(point.x) - half
      const sy = Math.floor(point.y) - half
      const cell = pixelSize / ZOOM_CELLS
      context.imageSmoothingEnabled = false
      context.fillStyle = "#000"
      context.fillRect(0, 0, pixelSize, pixelSize)
      context.drawImage(source.image, sx, sy, ZOOM_CELLS, ZOOM_CELLS, 0, 0, pixelSize, pixelSize)

      context.strokeStyle = "rgba(255,255,255,0.12)"
      context.lineWidth = 1
      context.beginPath()
      for (let index = 1; index < ZOOM_CELLS; index += 1) {
        const offset = Math.round(index * cell) + 0.5
        context.moveTo(offset, 0)
        context.lineTo(offset, pixelSize)
        context.moveTo(0, offset)
        context.lineTo(pixelSize, offset)
      }
      context.stroke()

      context.fillStyle = "rgba(56, 189, 248, 0.45)"
      context.fillRect(0, half * cell + cell / 2 - dpr, pixelSize, dpr * 2)
      context.fillRect(half * cell + cell / 2 - dpr, 0, dpr * 2, pixelSize)
      context.strokeStyle = "#fff"
      context.lineWidth = Math.max(1, dpr)
      context.strokeRect(half * cell, half * cell, cell, cell)
    }

    const rgb = rgbAt(point)
    if (positionRef.current) {
      const size = selectionSize ? `  ${Math.round(selectionSize.width)} × ${Math.round(selectionSize.height)}` : ""
      positionRef.current.textContent = `(${Math.floor(point.x)}, ${Math.floor(point.y)})${size}`
    }
    if (colorRef.current) colorRef.current.textContent = formatColor(rgb, hexModeRef.current)
    if (swatchRef.current) swatchRef.current.style.background = rgb ? `rgb(${rgb[0]}, ${rgb[1]}, ${rgb[2]})` : "transparent"
  }

  useImperativeHandle(handleRef, () => ({
    update: render,
    colorAt: (point) => formatColor(rgbAt(point), true),
    setHexMode: (hexMode) => {
      hexModeRef.current = hexMode
      if (colorRef.current && lastPointRef.current) {
        colorRef.current.textContent = formatColor(rgbAt(lastPointRef.current), hexMode)
      }
    },
  }))

  return (
    <div
      ref={rootRef}
      className="pointer-events-none absolute left-0 top-0 z-40 hidden overflow-hidden rounded-md border border-white/30 bg-neutral-900/95 text-[11px] leading-4 text-neutral-100 shadow-2xl"
      style={{ width: LENS_CSS }}
    >
      <canvas ref={canvasRef} className="block" style={{ width: LENS_CSS, height: LENS_CSS }} />
      <div className="grid gap-0.5 px-2 py-1.5 font-mono">
        <span ref={positionRef} className="truncate" />
        <span className="flex items-center gap-1.5">
          <span ref={swatchRef} className="size-2.5 shrink-0 rounded-sm border border-white/40" />
          <span ref={colorRef} className="truncate" />
        </span>
        <span className="truncate font-sans text-white/55">C 复制颜色 · Shift 切换</span>
      </div>
    </div>
  )
}
