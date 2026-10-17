import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react"
import type { MouseEvent as ReactMouseEvent, PointerEvent as ReactPointerEvent } from "react"
import { ScreenshotMagnifier, type MagnifierHandle, type MagnifierSource } from "@/components/screenshot/screenshot-magnifier"
import { ScreenshotOcrPanel, type OcrPanelState } from "@/components/screenshot/screenshot-ocr-panel"
import {
  MOSAIC_LEVELS,
  SCREENSHOT_COLORS,
  STROKE_LEVELS,
  TEXT_LEVELS,
  TOOL_SHORTCUTS,
  ScreenshotToolbar,
  type ToolbarStyleState,
} from "@/components/screenshot/screenshot-toolbar"
import {
  MosaicRenderer,
  TEXT_FONT_FAMILY,
  TEXT_LINE_HEIGHT,
  TEXT_PADDING,
  annotationHasArea,
  drawAnnotation,
  drawSelectionOutline,
  exportSelectionPng,
  hitTestAnnotation,
  nextAnnotationId,
  translateAnnotation,
  type Annotation,
  type DrawTool,
  type TextAnnotation,
} from "@/lib/screenshot/annotations"
import {
  SCREENSHOT_END_EVENT,
  SCREENSHOT_START_EVENT,
  applyTransparentDocument,
  errorText,
  getScreenshotSession,
  loadScreenshotImage,
  screenshotCancel,
  screenshotCopy,
  screenshotOcr,
  screenshotOverlayReady,
  screenshotPin,
  screenshotSave,
  type ScreenshotImage,
} from "@/lib/screenshot/api"
import {
  HANDLE_DIRECTIONS,
  clampPoint,
  clampRect,
  handleCursor,
  hitHandle,
  moveRectWithin,
  rectContains,
  rectFromPoints,
  resizeRect,
  roundRect,
  type HandleDirection,
  type Point,
  type Rect,
} from "@/lib/screenshot/geometry"
import { safeListen } from "@/lib/tauri-window"
import type { ScreenshotSession, ScreenshotStartEvent } from "@/types"

const HISTORY_LIMIT = 100
const DRAG_THRESHOLD_CSS = 3
const HANDLE_HIT_CSS = 7
const MIN_POINT_DISTANCE = 1.5
const DIM_COLOR = "rgba(0, 0, 0, 0.45)"
const ACCENT = "#1aad19"
/** Selection / hovered-window frame width in CSS pixels; drawn inside the rect, never exported. */
const FRAME_CSS = 2
const HANDLE_CSS = 8
const FIRST_PAINT_TIMEOUT_MS = 300

type Interaction =
  | { kind: "none" }
  | { kind: "pending-select"; start: Point }
  | { kind: "creating"; start: Point }
  | { kind: "moving"; origin: Rect; start: Point }
  | { kind: "resizing"; origin: Rect; direction: HandleDirection }
  | { kind: "drawing"; draft: Annotation }
  | { kind: "moving-annotation"; id: string; start: Point; original: Annotation; before: Annotation[]; moved: boolean }

type BusyAction = "copy" | "save" | "pin"

interface TextEditorState {
  id: string | null
  x: number
  y: number
  text: string
  color: string
  fontSize: number
}

function monitorIndexFromLocation() {
  const hash = window.location.hash
  const query = hash.includes("?") ? hash.slice(hash.indexOf("?") + 1) : window.location.search.slice(1)
  const value = Number(new URLSearchParams(query).get("monitor"))
  return Number.isFinite(value) ? value : 0
}

function waitForNextPaint() {
  return new Promise<void>((resolve) => {
    requestAnimationFrame(() => requestAnimationFrame(() => resolve()))
  })
}

/** Two animation frames, capped in case the WebView throttles animation frames. */
function waitForFirstPaint() {
  return Promise.race([waitForNextPaint(), new Promise<void>((resolve) => window.setTimeout(resolve, FIRST_PAINT_TIMEOUT_MS))])
}

/** Start-latency marks of one session, logged as a single `[screenshot]` console line once ready is sent. */
interface StartTiming {
  origin: number
  last: number
  stages: string[]
}

function createStartTiming(): StartTiming {
  const now = performance.now()
  return { origin: now, last: now, stages: [] }
}

function markStartTiming(timing: StartTiming | null, stage: string) {
  if (!timing) return
  const now = performance.now()
  timing.stages.push(`${stage} ${Math.round(now - timing.last)}ms`)
  timing.last = now
}

function isEditableTarget(target: EventTarget | null) {
  return target instanceof HTMLTextAreaElement || target instanceof HTMLInputElement
}

export function ScreenshotOverlayPage() {
  const monitorIndex = useMemo(monitorIndexFromLocation, [])
  const [session, setSession] = useState<ScreenshotSession | null>(null)
  const [magnifierSource, setMagnifierSource] = useState<MagnifierSource | null>(null)
  const [hasSelection, setHasSelection] = useState(false)
  const [dragging, setDragging] = useState(false)
  const [layoutVersion, setLayoutVersion] = useState(0)
  const [annotations, setAnnotations] = useState<Annotation[]>([])
  const [canUndo, setCanUndo] = useState(false)
  const [tool, setTool] = useState<DrawTool | null>(null)
  const [toolStyle, setToolStyle] = useState<ToolbarStyleState>({
    color: SCREENSHOT_COLORS[0],
    strokeLevel: 1,
    textLevel: 1,
    mosaicLevel: 1,
  })
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [textEditor, setTextEditor] = useState<TextEditorState | null>(null)
  const [ocr, setOcr] = useState<OcrPanelState | null>(null)
  const [busy, setBusy] = useState<BusyAction | null>(null)
  const [notice, setNotice] = useState<{ text: string; error: boolean } | null>(null)

  const rootRef = useRef<HTMLDivElement | null>(null)
  const baseCanvasRef = useRef<HTMLCanvasElement | null>(null)
  const annotationCanvasRef = useRef<HTMLCanvasElement | null>(null)
  const selectionBoxRef = useRef<HTMLDivElement | null>(null)
  const sizeLabelRef = useRef<HTMLDivElement | null>(null)
  const textAreaRef = useRef<HTMLTextAreaElement | null>(null)
  const magnifierRef = useRef<MagnifierHandle | null>(null)

  const sessionRef = useRef<ScreenshotSession | null>(null)
  const sessionIdRef = useRef<string | null>(null)
  const loadTokenRef = useRef(0)
  const bitmapRef = useRef<ImageBitmap | null>(null)
  /** True once the capture is drawn into the base canvas; the dim layer must never show before it. */
  const baseDrawnRef = useRef(false)
  const mosaicRef = useRef<MosaicRenderer | null>(null)
  const selectionRef = useRef<Rect | null>(null)
  const hoverRectRef = useRef<Rect | null>(null)
  const pointerRef = useRef<Point | null>(null)
  const interactionRef = useRef<Interaction>({ kind: "none" })
  const annotationsRef = useRef<Annotation[]>([])
  const historyRef = useRef<Annotation[][]>([])
  const toolRef = useRef<DrawTool | null>(null)
  const toolStyleRef = useRef(toolStyle)
  const selectedIdRef = useRef<string | null>(null)
  const textEditorRef = useRef<TextEditorState | null>(null)
  const ocrRef = useRef<OcrPanelState | null>(null)
  const busyRef = useRef<BusyAction | null>(null)
  const frameRef = useRef(0)
  const annotationsDirtyRef = useRef(true)
  const hexModeRef = useRef(false)
  const noticeTimerRef = useRef(0)
  const startTimingRef = useRef<StartTiming | null>(null)

  toolRef.current = tool
  toolStyleRef.current = toolStyle
  selectedIdRef.current = selectedId
  textEditorRef.current = textEditor
  ocrRef.current = ocr

  const scale = session?.monitor.scale_factor || window.devicePixelRatio || 1
  const cssWidth = session ? session.monitor.width / scale : window.innerWidth
  const cssHeight = session ? session.monitor.height / scale : window.innerHeight
  const locked = annotations.length > 0

  useLayoutEffect(() => applyTransparentDocument(), [])

  const showNotice = useCallback((text: string, error = false) => {
    window.clearTimeout(noticeTimerRef.current)
    setNotice({ text, error })
    noticeTimerRef.current = window.setTimeout(() => setNotice(null), error ? 4000 : 1600)
  }, [])

  const monitorBounds = (): Rect => {
    const current = sessionRef.current
    return current ? { x: 0, y: 0, width: current.monitor.width, height: current.monitor.height } : { x: 0, y: 0, width: 0, height: 0 }
  }

  const currentScale = () => sessionRef.current?.monitor.scale_factor || window.devicePixelRatio || 1

  // ---------- imperative frame rendering (no React re-render on pointer move) ----------

  const renderFrame = () => {
    frameRef.current = 0
    const current = sessionRef.current
    const box = selectionBoxRef.current
    const label = sizeLabelRef.current
    const frameScale = currentScale()
    const interaction = interactionRef.current
    const painted = Boolean(current) && baseDrawnRef.current

    if (box && label) {
      const rect = selectionRef.current ?? hoverRectRef.current
      if (painted && rect) {
        box.style.display = "block"
        box.style.left = `${rect.x / frameScale}px`
        box.style.top = `${rect.y / frameScale}px`
        box.style.width = `${rect.width / frameScale}px`
        box.style.height = `${rect.height / frameScale}px`
        box.style.borderColor = ACCENT
        label.style.display = "block"
        label.textContent = `${Math.round(rect.width)} × ${Math.round(rect.height)}`
        label.style.top = rect.y / frameScale >= 26 ? `${-24 - FRAME_CSS}px` : "4px"
        label.style.left = rect.y / frameScale >= 26 ? `${-FRAME_CSS}px` : "4px"
      } else {
        box.style.display = painted ? "block" : "none"
        box.style.left = "0px"
        box.style.top = "0px"
        box.style.width = "0px"
        box.style.height = "0px"
        box.style.borderColor = "transparent"
        label.style.display = "none"
      }
    }

    if (annotationsDirtyRef.current) {
      annotationsDirtyRef.current = false
      const canvas = annotationCanvasRef.current
      const context = canvas?.getContext("2d")
      if (canvas && context) {
        context.setTransform(1, 0, 0, 1, 0, 0)
        context.clearRect(0, 0, canvas.width, canvas.height)
        const selection = selectionRef.current
        if (selection) {
          context.save()
          context.beginPath()
          context.rect(selection.x, selection.y, selection.width, selection.height)
          context.clip()
          const editingId = textEditorRef.current?.id
          for (const annotation of annotationsRef.current) {
            if (annotation.id === editingId) continue
            drawAnnotation(context, annotation, mosaicRef.current)
          }
          if (interaction.kind === "drawing") drawAnnotation(context, interaction.draft, mosaicRef.current)
          const selected = annotationsRef.current.find((annotation) => annotation.id === selectedIdRef.current)
          if (selected && selected.id !== editingId) drawSelectionOutline(context, selected, frameScale)
          context.restore()
        }
      }
    }

    const showMagnifier =
      painted &&
      !busyRef.current &&
      (interaction.kind === "creating" ||
        interaction.kind === "resizing" ||
        interaction.kind === "pending-select" ||
        (interaction.kind === "none" && !selectionRef.current))
    const sizeRect = selectionRef.current ?? hoverRectRef.current
    magnifierRef.current?.update(showMagnifier ? pointerRef.current : null, sizeRect)
  }

  const scheduleRender = (annotationsChanged = false) => {
    if (annotationsChanged) annotationsDirtyRef.current = true
    if (!frameRef.current) frameRef.current = requestAnimationFrame(renderFrame)
  }

  useEffect(
    () => () => {
      // Reset the id too: StrictMode remounts reuse these refs, and a stale non-zero id would make
      // scheduleRender believe a frame is still pending forever, so nothing would ever render again.
      cancelAnimationFrame(frameRef.current)
      frameRef.current = 0
    },
    [],
  )

  // ---------- annotations & history ----------

  const commitAnnotations = (next: Annotation[], before: Annotation[] = annotationsRef.current) => {
    historyRef.current.push(before)
    if (historyRef.current.length > HISTORY_LIMIT) historyRef.current.shift()
    annotationsRef.current = next
    setAnnotations(next)
    setCanUndo(true)
    scheduleRender(true)
  }

  const undo = () => {
    const previous = historyRef.current.pop()
    if (!previous) return
    annotationsRef.current = previous
    setAnnotations(previous)
    setCanUndo(historyRef.current.length > 0)
    if (selectedIdRef.current && !previous.some((annotation) => annotation.id === selectedIdRef.current)) setSelectedId(null)
    scheduleRender(true)
  }

  const commitTextEditor = () => {
    const editor = textEditorRef.current
    if (!editor) return
    textEditorRef.current = null
    setTextEditor(null)
    const text = editor.text.replace(/\s+$/, "")
    const existing = editor.id ? annotationsRef.current.find((annotation) => annotation.id === editor.id) : undefined
    if (existing) {
      if (!text.trim()) {
        commitAnnotations(annotationsRef.current.filter((annotation) => annotation.id !== editor.id))
      } else if (existing.kind === "text" && (existing.text !== text || existing.color !== editor.color || existing.fontSize !== editor.fontSize)) {
        commitAnnotations(
          annotationsRef.current.map((annotation) =>
            annotation.id === editor.id ? { ...existing, text, color: editor.color, fontSize: editor.fontSize } : annotation,
          ),
        )
      }
    } else if (text.trim()) {
      const annotation: TextAnnotation = {
        id: nextAnnotationId(),
        kind: "text",
        x: editor.x,
        y: editor.y,
        text,
        color: editor.color,
        fontSize: editor.fontSize,
      }
      commitAnnotations([...annotationsRef.current, annotation])
    }
    scheduleRender(true)
  }

  const openTextEditor = (state: TextEditorState) => {
    textEditorRef.current = state
    setTextEditor(state)
    scheduleRender(true)
  }

  // ---------- session lifecycle ----------

  const resetEditor = useCallback(() => {
    interactionRef.current = { kind: "none" }
    selectionRef.current = null
    hoverRectRef.current = null
    annotationsRef.current = []
    historyRef.current = []
    textEditorRef.current = null
    busyRef.current = null
    setHasSelection(false)
    setDragging(false)
    setAnnotations([])
    setCanUndo(false)
    setTool(null)
    setSelectedId(null)
    setTextEditor(null)
    setOcr(null)
    setBusy(null)
    setNotice(null)
    scheduleRender(true)
  }, [])

  const clearSession = useCallback(() => {
    loadTokenRef.current += 1
    sessionIdRef.current = null
    sessionRef.current = null
    bitmapRef.current?.close()
    bitmapRef.current = null
    baseDrawnRef.current = false
    mosaicRef.current = null
    const base = baseCanvasRef.current
    if (base) {
      base.width = 0
      base.height = 0
    }
    setSession(null)
    setMagnifierSource(null)
    resetEditor()
  }, [resetEditor])

  const reportReady = useCallback(
    async (sessionId: string) => {
      await waitForFirstPaint()
      if (sessionIdRef.current !== sessionId) return
      const timing = startTimingRef.current
      startTimingRef.current = null
      markStartTiming(timing, "paint")
      try {
        await screenshotOverlayReady(sessionId, monitorIndex)
      } catch {
        // Only the backend start log waits for this; it gives up on its own after a timeout.
      }
      if (timing) {
        markStartTiming(timing, "ready")
        console.info(
          `[screenshot] session ${sessionId}: ${timing.stages.join(", ")}; total ${Math.round(performance.now() - timing.origin)}ms`,
        )
      }
    },
    [monitorIndex],
  )

  const loadSession = useCallback(
    async (expectedSessionId?: string) => {
      if (expectedSessionId && expectedSessionId === sessionIdRef.current) return
      const timing = startTimingRef.current ?? createStartTiming()
      startTimingRef.current = timing
      // Concurrent loads (mount + start event) race here: whichever resolves first bumps the token and wins.
      const requestToken = loadTokenRef.current
      let next: ScreenshotSession | null
      try {
        next = await getScreenshotSession(monitorIndex)
      } catch {
        return
      }
      if (!next || requestToken !== loadTokenRef.current) return
      if (next.session_id === sessionIdRef.current) return

      clearSession()
      startTimingRef.current = timing
      markStartTiming(timing, "session")
      const loadToken = loadTokenRef.current
      sessionIdRef.current = next.session_id
      let image: ScreenshotImage
      try {
        image = await loadScreenshotImage(next.image_url, () => markStartTiming(timing, "fetch"))
        markStartTiming(timing, "decode")
      } catch (reason) {
        if (loadToken === loadTokenRef.current) {
          showNotice(`截图加载失败：${errorText(reason)}`, true)
          // Report anyway so the backend start log is not left waiting; the error stays visible and Esc dismisses it.
          void reportReady(next.session_id)
        }
        return
      }
      const { bitmap, pixels } = image
      if (loadToken !== loadTokenRef.current) {
        bitmap.close()
        return
      }

      bitmapRef.current = bitmap
      mosaicRef.current = new MosaicRenderer({ image: bitmap, width: bitmap.width, height: bitmap.height })
      sessionRef.current = next
      setSession(next)
      setMagnifierSource({ image: bitmap, pixels, width: bitmap.width, height: bitmap.height, scale: next.monitor.scale_factor || 1 })
      scheduleRender(true)
    },
    [clearSession, monitorIndex, reportReady, showNotice],
  )

  useLayoutEffect(() => {
    const base = baseCanvasRef.current
    const overlay = annotationCanvasRef.current
    const bitmap = bitmapRef.current
    if (!session || !base || !overlay || !bitmap) return
    base.width = bitmap.width
    base.height = bitmap.height
    base.getContext("2d")?.drawImage(bitmap, 0, 0)
    overlay.width = bitmap.width
    overlay.height = bitmap.height
    // The overlay is already on screen: render the dim layer synchronously so it lands in the same
    // frame as the base image, never on top of the transparent window (a dimmed live desktop).
    baseDrawnRef.current = true
    cancelAnimationFrame(frameRef.current)
    annotationsDirtyRef.current = true
    renderFrame()
    markStartTiming(startTimingRef.current, "draw")
    void reportReady(session.session_id)
  }, [session])

  useEffect(() => {
    let disposed = false
    const unlisteners: Array<() => void> = []

    void (async () => {
      const startDispose = await safeListen<ScreenshotStartEvent>(SCREENSHOT_START_EVENT, (event) => {
        if (event.payload.monitor_index !== monitorIndex) return
        if (event.payload.session_id !== sessionIdRef.current) startTimingRef.current = createStartTiming()
        void loadSession(event.payload.session_id)
      })
      const endDispose = await safeListen(SCREENSHOT_END_EVENT, () => clearSession())
      if (disposed) {
        startDispose?.()
        endDispose?.()
        return
      }
      if (startDispose) unlisteners.push(startDispose)
      if (endDispose) unlisteners.push(endDispose)
    })()

    void loadSession()

    return () => {
      disposed = true
      unlisteners.forEach((unlisten) => unlisten())
    }
  }, [clearSession, loadSession, monitorIndex])

  // ---------- actions ----------

  const cancelScreenshot = useCallback(async () => {
    clearSession()
    try {
      await screenshotCancel()
    } catch {
      // The backend also ends sessions on its own; a failed cancel leaves nothing to clean up here.
    }
  }, [clearSession])

  const goBack = () => {
    if (interactionRef.current.kind !== "none") return
    if (textEditorRef.current) {
      commitTextEditor()
      return
    }
    if (ocrRef.current) {
      setOcr(null)
      return
    }
    if (selectionRef.current) {
      resetEditor()
      return
    }
    void cancelScreenshot()
  }

  const runExportAction = async (action: BusyAction) => {
    const selection = selectionRef.current
    const bitmap = bitmapRef.current
    const current = sessionRef.current
    if (busyRef.current || !selection || !bitmap || !current) return
    commitTextEditor()
    const rect = roundRect(selection)
    if (rect.width < 1 || rect.height < 1) return

    busyRef.current = action
    setBusy(action)
    scheduleRender()
    await waitForNextPaint()
    try {
      const png = await exportSelectionPng(bitmap, rect, annotationsRef.current, mosaicRef.current)
      if (action === "copy") {
        await screenshotCopy(png)
      } else if (action === "save") {
        const path = await screenshotSave(png)
        if (path === null) showNotice("已取消保存")
      } else {
        await screenshotPin(png, {
          x: current.monitor.x + rect.x,
          y: current.monitor.y + rect.y,
          width: rect.width,
          height: rect.height,
        })
      }
    } catch (reason) {
      showNotice(errorText(reason), true)
    } finally {
      busyRef.current = null
      setBusy(null)
    }
  }

  const runOcr = async () => {
    const selection = selectionRef.current
    const bitmap = bitmapRef.current
    if (busyRef.current || !selection || !bitmap) return
    commitTextEditor()
    const rect = roundRect(selection)
    if (rect.width < 1 || rect.height < 1) return
    const sessionId = sessionIdRef.current
    setOcr({ status: "loading" })
    try {
      const png = await exportSelectionPng(bitmap, rect, annotationsRef.current, mosaicRef.current)
      const result = await screenshotOcr(png)
      if (sessionId !== sessionIdRef.current) return
      setOcr({ status: "done", text: result?.text ?? "" })
    } catch (reason) {
      if (sessionId !== sessionIdRef.current) return
      setOcr({ status: "error", message: errorText(reason) })
    }
  }

  const deleteSelectedAnnotation = () => {
    const id = selectedIdRef.current
    if (!id) return
    setSelectedId(null)
    selectedIdRef.current = null
    commitAnnotations(annotationsRef.current.filter((annotation) => annotation.id !== id))
  }

  const copyColorAtPointer = async () => {
    const point = pointerRef.current
    const magnifier = magnifierRef.current
    if (!point || !magnifier) return
    const color = magnifier.colorAt(point)
    if (color === "--") return
    try {
      await navigator.clipboard.writeText(color)
      showNotice(`已复制颜色 ${color}`)
    } catch (reason) {
      showNotice(`复制颜色失败：${errorText(reason)}`, true)
    }
  }

  const changeTool = (next: DrawTool | null) => {
    commitTextEditor()
    setTool(next)
  }

  const changeStyle = (patch: Partial<ToolbarStyleState>) => {
    const nextStyle = { ...toolStyleRef.current, ...patch }
    setToolStyle(nextStyle)
    const id = selectedIdRef.current
    const selected = id ? annotationsRef.current.find((annotation) => annotation.id === id) : undefined
    if (!selected) return
    const currentScaleFactor = currentScale()
    let updated: Annotation | null = null
    if (selected.kind === "rect" || selected.kind === "ellipse" || selected.kind === "arrow" || selected.kind === "pen") {
      updated = { ...selected, color: nextStyle.color, width: STROKE_LEVELS[nextStyle.strokeLevel] * currentScaleFactor }
    } else if (selected.kind === "text") {
      updated = { ...selected, color: nextStyle.color, fontSize: TEXT_LEVELS[nextStyle.textLevel] * currentScaleFactor }
    }
    if (updated) {
      const replacement = updated
      commitAnnotations(annotationsRef.current.map((annotation) => (annotation.id === id ? replacement : annotation)))
    }
  }

  // ---------- pointer handling ----------

  const toPhysical = (event: { clientX: number; clientY: number }): Point => {
    const pointScale = currentScale()
    return clampPoint({ x: event.clientX * pointScale, y: event.clientY * pointScale }, monitorBounds())
  }

  const windowRectAt = (point: Point): Rect => {
    const current = sessionRef.current
    const bounds = monitorBounds()
    if (!current) return bounds
    for (const windowRect of current.windows) {
      if (windowRect.width < 4 || windowRect.height < 4) continue
      if (rectContains(windowRect, point)) {
        return clampRect({ x: windowRect.x, y: windowRect.y, width: windowRect.width, height: windowRect.height }, bounds)
      }
    }
    return bounds
  }

  const topAnnotationAt = (point: Point) => {
    const tolerance = 4 * currentScale()
    const list = annotationsRef.current
    for (let index = list.length - 1; index >= 0; index -= 1) {
      if (hitTestAnnotation(list[index], point, tolerance)) return list[index]
    }
    return null
  }

  const updateCursor = (point: Point) => {
    const root = rootRef.current
    if (!root) return
    const selection = selectionRef.current
    const activeTool = toolRef.current
    let cursor = "crosshair"
    if (selection) {
      const handle = annotationsRef.current.length === 0 ? hitHandle(selection, point, HANDLE_HIT_CSS * currentScale()) : null
      const inside = rectContains(selection, point)
      if (handle) cursor = handleCursor(handle)
      else if (inside && canPickAnnotation(activeTool) && topAnnotationAt(point)) cursor = "move"
      else if (inside && activeTool === "text") cursor = "text"
      else if (inside && !activeTool) cursor = annotationsRef.current.length === 0 ? "move" : "default"
      else if (!inside) cursor = annotationsRef.current.length === 0 && !activeTool ? "crosshair" : "default"
    }
    if (root.style.cursor !== cursor) root.style.cursor = cursor
  }

  const commitSelection = (rect: Rect) => {
    selectionRef.current = roundRect(rect)
    hoverRectRef.current = null
    setHasSelection(true)
    setLayoutVersion((version) => version + 1)
    scheduleRender(true)
  }

  const createDraft = (activeTool: DrawTool, point: Point): Annotation => {
    const style = toolStyleRef.current
    const pointScale = currentScale()
    const width = STROKE_LEVELS[style.strokeLevel] * pointScale
    switch (activeTool) {
      case "rect":
      case "ellipse":
      case "arrow":
        return { id: nextAnnotationId(), kind: activeTool, start: point, end: point, color: style.color, width }
      case "pen":
        return { id: nextAnnotationId(), kind: "pen", points: [point], color: style.color, width }
      case "mosaic": {
        const brush = MOSAIC_LEVELS[style.mosaicLevel] * pointScale
        return {
          id: nextAnnotationId(),
          kind: "mosaic",
          points: [point],
          brush,
          block: Math.max(4, Math.round(brush / 2.6)),
        }
      }
      case "text":
        throw new Error("text annotations are created by the editor")
    }
  }

  const handlePointerDown = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (event.button !== 0 || !sessionRef.current || busyRef.current) return
    if (textEditorRef.current) {
      event.preventDefault()
      commitTextEditor()
      return
    }
    const point = toPhysical(event)
    pointerRef.current = point
    const selection = selectionRef.current
    const activeTool = toolRef.current
    let next: Interaction = { kind: "none" }

    if (!selection) {
      next = { kind: "pending-select", start: point }
    } else {
      const isLocked = annotationsRef.current.length > 0
      const handle = !isLocked ? hitHandle(selection, point, HANDLE_HIT_CSS * currentScale()) : null
      const inside = rectContains(selection, point)
      const picked = inside && canPickAnnotation(activeTool) ? topAnnotationAt(point) : null

      if (handle) {
        next = { kind: "resizing", origin: selection, direction: handle }
      } else if (picked) {
        setSelectedId(picked.id)
        selectedIdRef.current = picked.id
        next = { kind: "moving-annotation", id: picked.id, start: point, original: picked, before: annotationsRef.current, moved: false }
      } else if (inside && activeTool === "text") {
        setSelectedId(null)
        const style = toolStyleRef.current
        openTextEditor({
          id: null,
          x: point.x,
          y: point.y,
          text: "",
          color: style.color,
          fontSize: TEXT_LEVELS[style.textLevel] * currentScale(),
        })
        event.preventDefault()
        return
      } else if (inside && activeTool) {
        setSelectedId(null)
        selectedIdRef.current = null
        next = { kind: "drawing", draft: createDraft(activeTool, point) }
      } else if (inside && !isLocked) {
        next = { kind: "moving", origin: selection, start: point }
      } else if (!inside && !isLocked && !activeTool) {
        selectionRef.current = null
        setHasSelection(false)
        setOcr(null)
        next = { kind: "creating", start: point }
        setDragging(true)
      } else {
        setSelectedId(null)
        selectedIdRef.current = null
      }
    }

    interactionRef.current = next
    if (next.kind !== "none") {
      event.currentTarget.setPointerCapture(event.pointerId)
      if (next.kind === "moving" || next.kind === "resizing") setDragging(true)
    }
    scheduleRender(true)
  }

  const handlePointerMove = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (!sessionRef.current) return
    const point = toPhysical(event)
    pointerRef.current = point
    const interaction = interactionRef.current
    const bounds = monitorBounds()

    switch (interaction.kind) {
      case "none":
        if (!selectionRef.current) hoverRectRef.current = windowRectAt(point)
        updateCursor(point)
        scheduleRender()
        return
      case "pending-select": {
        const threshold = DRAG_THRESHOLD_CSS * currentScale()
        if (Math.abs(point.x - interaction.start.x) < threshold && Math.abs(point.y - interaction.start.y) < threshold) {
          scheduleRender()
          return
        }
        interactionRef.current = { kind: "creating", start: interaction.start }
        hoverRectRef.current = null
        setDragging(true)
        selectionRef.current = rectFromPoints(interaction.start, point)
        scheduleRender(true)
        return
      }
      case "creating":
        selectionRef.current = clampRect(rectFromPoints(interaction.start, point), bounds)
        scheduleRender(true)
        return
      case "moving":
        selectionRef.current = moveRectWithin(interaction.origin, point.x - interaction.start.x, point.y - interaction.start.y, bounds)
        scheduleRender(true)
        return
      case "resizing":
        selectionRef.current = resizeRect(interaction.origin, interaction.direction, point, bounds)
        scheduleRender(true)
        return
      case "drawing": {
        const selection = selectionRef.current
        if (!selection) return
        const draft = interaction.draft
        if (draft.kind === "pen" || draft.kind === "mosaic") {
          const nativeEvent = event.nativeEvent
          const samples = typeof nativeEvent.getCoalescedEvents === "function" ? nativeEvent.getCoalescedEvents() : []
          const points = samples.length > 0 ? samples.map((sample) => toPhysical(sample)) : [point]
          for (const sample of points) {
            const clamped = clampPoint(sample, selection)
            const last = draft.points[draft.points.length - 1]
            if (!last || Math.hypot(clamped.x - last.x, clamped.y - last.y) >= MIN_POINT_DISTANCE) draft.points.push(clamped)
          }
        } else if (draft.kind === "rect" || draft.kind === "ellipse" || draft.kind === "arrow") {
          let end = clampPoint(point, selection)
          if (event.shiftKey && draft.kind !== "arrow") {
            const size = Math.max(Math.abs(end.x - draft.start.x), Math.abs(end.y - draft.start.y))
            end = clampPoint(
              { x: draft.start.x + Math.sign(end.x - draft.start.x || 1) * size, y: draft.start.y + Math.sign(end.y - draft.start.y || 1) * size },
              selection,
            )
          }
          draft.end = end
        }
        scheduleRender(true)
        return
      }
      case "moving-annotation": {
        const dx = point.x - interaction.start.x
        const dy = point.y - interaction.start.y
        if (!interaction.moved && Math.hypot(dx, dy) < DRAG_THRESHOLD_CSS * currentScale()) return
        interaction.moved = true
        const moved = translateAnnotation(interaction.original, dx, dy)
        annotationsRef.current = interaction.before.map((annotation) => (annotation.id === interaction.id ? moved : annotation))
        scheduleRender(true)
        return
      }
    }
  }

  const handlePointerUp = (event: ReactPointerEvent<HTMLDivElement>) => {
    const interaction = interactionRef.current
    interactionRef.current = { kind: "none" }
    if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId)
    const minimum = 2 * currentScale()

    switch (interaction.kind) {
      case "pending-select":
        commitSelection(hoverRectRef.current ?? windowRectAt(interaction.start))
        break
      case "creating": {
        const selection = selectionRef.current
        if (selection && selection.width >= minimum && selection.height >= minimum) {
          commitSelection(selection)
        } else {
          selectionRef.current = null
          setHasSelection(false)
          hoverRectRef.current = windowRectAt(toPhysical(event))
        }
        break
      }
      case "moving":
      case "resizing": {
        const selection = selectionRef.current
        if (selection && selection.width >= 1 && selection.height >= 1) commitSelection(selection)
        else commitSelection(interaction.origin)
        break
      }
      case "drawing":
        if (annotationHasArea(interaction.draft)) commitAnnotations([...annotationsRef.current, interaction.draft])
        break
      case "moving-annotation":
        if (interaction.moved) {
          const next = annotationsRef.current
          annotationsRef.current = interaction.before
          commitAnnotations(next, interaction.before)
        }
        break
      case "none":
        break
    }

    setDragging(false)
    setLayoutVersion((version) => version + 1)
    scheduleRender(true)
  }

  const handleDoubleClick = (event: ReactMouseEvent<HTMLDivElement>) => {
    if (busyRef.current || textEditorRef.current) return
    const selection = selectionRef.current
    if (!selection) return
    const point = toPhysical(event)
    if (!rectContains(selection, point)) return
    const picked = topAnnotationAt(point)
    if (picked?.kind === "text") {
      setSelectedId(null)
      openTextEditor({ id: picked.id, x: picked.x, y: picked.y, text: picked.text, color: picked.color, fontSize: picked.fontSize })
      return
    }
    if (toolRef.current === "text") return
    void runExportAction("copy")
  }

  // ---------- keyboard ----------

  const keyHandlerRef = useRef<(event: KeyboardEvent) => void>(() => undefined)
  keyHandlerRef.current = (event: KeyboardEvent) => {
    if (!sessionRef.current) {
      // A visible overlay without a usable session (e.g. image failed to load) must still be dismissible.
      if (event.key === "Escape") void cancelScreenshot()
      return
    }
    if (isEditableTarget(event.target)) {
      if (event.key === "Escape") {
        event.preventDefault()
        if (textEditorRef.current) commitTextEditor()
        else goBack()
      }
      return
    }

    const ctrl = event.ctrlKey || event.metaKey
    const key = event.key
    if (key === "Escape") {
      event.preventDefault()
      goBack()
      return
    }
    if (key === "Shift" && !event.repeat) {
      hexModeRef.current = !hexModeRef.current
      magnifierRef.current?.setHexMode(hexModeRef.current)
      return
    }
    if (busyRef.current) return

    if (ctrl && key.toLowerCase() === "z") {
      event.preventDefault()
      undo()
      return
    }
    if (ctrl && key.toLowerCase() === "c") {
      event.preventDefault()
      if (selectionRef.current) void runExportAction("copy")
      return
    }
    if (ctrl && key.toLowerCase() === "s") {
      event.preventDefault()
      if (selectionRef.current) void runExportAction("save")
      return
    }
    if (key === "Enter") {
      event.preventDefault()
      if (selectionRef.current) void runExportAction("copy")
      return
    }
    if ((key === "Delete" || key === "Backspace") && selectedIdRef.current) {
      event.preventDefault()
      deleteSelectedAnnotation()
      return
    }
    if (ctrl || event.altKey) return
    if (key.toLowerCase() === "c") {
      event.preventDefault()
      void copyColorAtPointer()
      return
    }
    if (selectionRef.current) {
      const match = (Object.entries(TOOL_SHORTCUTS) as [DrawTool, string][]).find(([, shortcut]) => shortcut === key.toUpperCase())
      if (match) {
        event.preventDefault()
        changeTool(toolRef.current === match[0] ? null : match[0])
      }
    }
  }

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => keyHandlerRef.current(event)
    window.addEventListener("keydown", onKeyDown, true)
    return () => window.removeEventListener("keydown", onKeyDown, true)
  }, [])

  useEffect(() => {
    if (!textEditor) return
    const node = textAreaRef.current
    if (!node) return
    node.focus()
    const length = node.value.length
    node.setSelectionRange(length, length)
    // Focus only when the editor opens or switches target, not on every keystroke.
  }, [textEditor?.id, textEditor?.x, textEditor?.y])

  useEffect(() => {
    scheduleRender(true)
  }, [selectedId, annotations])

  // ---------- render ----------

  const selection = selectionRef.current
  const anchor = useMemo<Rect | null>(() => {
    if (!hasSelection || !selection) return null
    return { x: selection.x / scale, y: selection.y / scale, width: selection.width / scale, height: selection.height / scale }
    // layoutVersion forces a recompute after imperative selection edits.
  }, [hasSelection, layoutVersion, scale, selection?.x, selection?.y, selection?.width, selection?.height])

  const showHandles = hasSelection && !locked && !dragging

  return (
    <div
      ref={rootRef}
      className="fixed inset-0 select-none overflow-hidden"
      style={{ cursor: "crosshair", background: session ? "transparent" : "rgba(0,0,0,0.01)" }}
      onPointerDown={handlePointerDown}
      onPointerMove={handlePointerMove}
      onPointerUp={handlePointerUp}
      onPointerCancel={handlePointerUp}
      onPointerLeave={() => {
        if (interactionRef.current.kind !== "none") return
        pointerRef.current = null
        scheduleRender()
      }}
      onDoubleClick={handleDoubleClick}
      onContextMenu={(event) => {
        event.preventDefault()
        goBack()
      }}
    >
      <canvas
        ref={baseCanvasRef}
        className="pointer-events-none absolute left-0 top-0"
        style={{ width: cssWidth, height: cssHeight, visibility: session ? "visible" : "hidden" }}
      />
      <div
        ref={selectionBoxRef}
        className="pointer-events-none absolute z-10 box-border"
        style={{ display: "none", borderWidth: FRAME_CSS, borderStyle: "solid", boxShadow: `0 0 0 99999px ${DIM_COLOR}` }}
      >
        <div
          ref={sizeLabelRef}
          className="absolute whitespace-nowrap rounded bg-black/75 px-1.5 py-0.5 font-mono text-xs leading-4 text-white"
          style={{ display: "none" }}
        />
        {showHandles
          ? HANDLE_DIRECTIONS.map((direction) => <SelectionHandle key={direction} direction={direction} />)
          : null}
      </div>
      <canvas
        ref={annotationCanvasRef}
        className="pointer-events-none absolute left-0 top-0 z-20"
        style={{ width: cssWidth, height: cssHeight, visibility: session ? "visible" : "hidden" }}
      />

      {textEditor ? (
        <textarea
          ref={textAreaRef}
          value={textEditor.text}
          rows={1}
          spellCheck={false}
          className="absolute z-30 resize-none overflow-hidden whitespace-pre border border-dashed border-white/80 bg-transparent outline-none [field-sizing:content]"
          style={{
            left: textEditor.x / scale,
            top: textEditor.y / scale,
            minWidth: (textEditor.fontSize / scale) * 2,
            maxWidth: Math.max(40, cssWidth - textEditor.x / scale - 4),
            color: textEditor.color,
            fontFamily: TEXT_FONT_FAMILY,
            fontSize: textEditor.fontSize / scale,
            lineHeight: TEXT_LINE_HEIGHT,
            padding: (textEditor.fontSize * TEXT_PADDING) / scale - 1,
            caretColor: textEditor.color,
          }}
          onPointerDown={(event) => event.stopPropagation()}
          onDoubleClick={(event) => event.stopPropagation()}
          onChange={(event) => {
            const next = { ...textEditor, text: event.target.value }
            textEditorRef.current = next
            setTextEditor(next)
          }}
          onBlur={() => commitTextEditor()}
        />
      ) : null}

      {anchor && !dragging && session ? (
        <ScreenshotToolbar
          anchor={anchor}
          viewportWidth={cssWidth}
          viewportHeight={cssHeight}
          tool={tool}
          style={toolStyle}
          canUndo={canUndo}
          busy={busy}
          onToolChange={changeTool}
          onStyleChange={changeStyle}
          onUndo={undo}
          onOcr={() => void runOcr()}
          onPin={() => void runExportAction("pin")}
          onSave={() => void runExportAction("save")}
          onCancel={() => void cancelScreenshot()}
          onDone={() => void runExportAction("copy")}
        />
      ) : null}

      {anchor && ocr && !dragging ? (
        <ScreenshotOcrPanel anchor={anchor} viewportWidth={cssWidth} viewportHeight={cssHeight} state={ocr} onClose={() => setOcr(null)} />
      ) : null}

      <ScreenshotMagnifier handleRef={magnifierRef} source={magnifierSource} />

      {notice ? (
        <div
          className={
            "pointer-events-none absolute left-1/2 top-6 z-50 -translate-x-1/2 rounded-md px-3 py-1.5 text-sm shadow-lg " +
            (notice.error ? "bg-red-600/95 text-white" : "bg-neutral-900/90 text-white")
          }
        >
          {notice.text}
        </div>
      ) : null}
    </div>
  )
}

function canPickAnnotation(tool: DrawTool | null) {
  return tool !== "pen" && tool !== "mosaic"
}

/** Handles are centred on the frame line, which lies inside the box's padding edge. */
function SelectionHandle({ direction }: { direction: HandleDirection }) {
  const edge = FRAME_CSS / 2
  const left = direction.includes("w") ? `${-edge}px` : direction.includes("e") ? `calc(100% + ${edge}px)` : "50%"
  const top = direction.includes("n") ? `${-edge}px` : direction.includes("s") ? `calc(100% + ${edge}px)` : "50%"
  return (
    <span
      className="absolute box-border -translate-x-1/2 -translate-y-1/2 rounded-full border border-white"
      style={{ left, top, width: HANDLE_CSS, height: HANDLE_CSS, background: ACCENT }}
    />
  )
}
