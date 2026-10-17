import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react"
import type { MouseEvent as ReactMouseEvent, WheelEvent as ReactWheelEvent } from "react"
import { ArrowsIn, Copy, DownloadSimple, X } from "@phosphor-icons/react"
import type { Icon } from "@phosphor-icons/react"
import { PhysicalPosition, PhysicalSize } from "@tauri-apps/api/dpi"
import {
  applyTransparentDocument,
  errorText,
  getScreenshotPin,
  screenshotPinClose,
  screenshotPinCopy,
  screenshotPinSave,
} from "@/lib/screenshot/api"
import { clamp } from "@/lib/screenshot/geometry"
import { safeCurrentWindow } from "@/lib/tauri-window"
import type { ScreenshotPinInfo } from "@/types"

const MIN_ZOOM = 0.1
const MAX_ZOOM = 5
const ZOOM_STEP = 1.1
const ZOOM_BADGE_MS = 900

interface ContextMenuState {
  x: number
  y: number
}

function pinIdFromLocation() {
  const hash = window.location.hash
  const query = hash.includes("?") ? hash.slice(hash.indexOf("?") + 1) : window.location.search.slice(1)
  return new URLSearchParams(query).get("id") ?? ""
}

export function ScreenshotPinPage() {
  const pinId = useMemo(pinIdFromLocation, [])
  const [pin, setPin] = useState<ScreenshotPinInfo | null>(null)
  const [zoom, setZoom] = useState(1)
  const [zoomBadgeVisible, setZoomBadgeVisible] = useState(false)
  const [menu, setMenu] = useState<ContextMenuState | null>(null)
  const [notice, setNotice] = useState<{ text: string; error: boolean } | null>(null)
  const zoomRef = useRef(1)
  const resizingRef = useRef(false)
  const pendingZoomRef = useRef<{ zoom: number; clientX: number; clientY: number } | null>(null)
  const badgeTimerRef = useRef(0)
  const noticeTimerRef = useRef(0)
  const closingRef = useRef(false)

  useLayoutEffect(() => applyTransparentDocument(), [])

  useEffect(() => {
    if (!pinId) return
    let disposed = false
    void (async () => {
      try {
        const info = await getScreenshotPin(pinId)
        if (!disposed && info) setPin(info)
      } catch (reason) {
        if (!disposed) setNotice({ text: errorText(reason), error: true })
      }
    })()
    return () => {
      disposed = true
    }
  }, [pinId])

  const showNotice = useCallback((text: string, error = false) => {
    window.clearTimeout(noticeTimerRef.current)
    setNotice({ text, error })
    noticeTimerRef.current = window.setTimeout(() => setNotice(null), error ? 3500 : 1400)
  }, [])

  const close = useCallback(async () => {
    if (closingRef.current) return
    closingRef.current = true
    try {
      await screenshotPinClose(pinId)
    } catch {
      closingRef.current = false
      await safeCurrentWindow()?.close()
    }
  }, [pinId])

  const applyZoom = useCallback(
    async (nextZoom: number, clientX: number, clientY: number) => {
      const currentWindow = safeCurrentWindow()
      if (!pin) return
      const target = clamp(nextZoom, MIN_ZOOM, MAX_ZOOM)
      if (Math.abs(target - zoomRef.current) < 0.0001) return

      setZoom(target)
      setZoomBadgeVisible(true)
      window.clearTimeout(badgeTimerRef.current)
      badgeTimerRef.current = window.setTimeout(() => setZoomBadgeVisible(false), ZOOM_BADGE_MS)

      if (!currentWindow) {
        zoomRef.current = target
        return
      }
      if (resizingRef.current) {
        pendingZoomRef.current = { zoom: target, clientX, clientY }
        return
      }

      resizingRef.current = true
      try {
        const [position, size] = await Promise.all([currentWindow.outerPosition(), currentWindow.innerSize()])
        const nextWidth = Math.max(1, Math.round(pin.width * target))
        const nextHeight = Math.max(1, Math.round(pin.height * target))
        const ratio = window.devicePixelRatio || 1
        const cursorX = clamp(clientX * ratio, 0, size.width)
        const cursorY = clamp(clientY * ratio, 0, size.height)
        const factorX = size.width > 0 ? nextWidth / size.width : 1
        const factorY = size.height > 0 ? nextHeight / size.height : 1
        const nextX = Math.round(position.x + cursorX - cursorX * factorX)
        const nextY = Math.round(position.y + cursorY - cursorY * factorY)
        await currentWindow.setSize(new PhysicalSize(nextWidth, nextHeight))
        await currentWindow.setPosition(new PhysicalPosition(nextX, nextY))
        zoomRef.current = target
      } catch (reason) {
        showNotice(`缩放失败：${errorText(reason)}`, true)
      } finally {
        resizingRef.current = false
        const pending = pendingZoomRef.current
        pendingZoomRef.current = null
        if (pending) void applyZoom(pending.zoom, pending.clientX, pending.clientY)
      }
    },
    [pin, showNotice],
  )

  const handleWheel = (event: ReactWheelEvent<HTMLDivElement>) => {
    if (!pin) return
    const base = pendingZoomRef.current?.zoom ?? zoom
    const next = event.deltaY < 0 ? base * ZOOM_STEP : base / ZOOM_STEP
    void applyZoom(Math.round(next * 100) / 100, event.clientX, event.clientY)
  }

  const handleMouseDown = (event: ReactMouseEvent<HTMLDivElement>) => {
    if (event.button !== 0) return
    if (menu) {
      setMenu(null)
      return
    }
    if (event.detail >= 2) {
      void close()
      return
    }
    void safeCurrentWindow()
      ?.startDragging()
      .catch(() => undefined)
  }

  const copy = async () => {
    setMenu(null)
    try {
      await screenshotPinCopy(pinId)
      showNotice("已复制到剪贴板")
    } catch (reason) {
      showNotice(errorText(reason), true)
    }
  }

  const save = async () => {
    setMenu(null)
    try {
      const path = await screenshotPinSave(pinId)
      if (path) showNotice("已保存")
    } catch (reason) {
      showNotice(errorText(reason), true)
    }
  }

  const resetSize = () => {
    setMenu(null)
    void applyZoom(1, window.innerWidth / 2, window.innerHeight / 2)
  }

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault()
        if (menu) setMenu(null)
        else void close()
        return
      }
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "c") {
        event.preventDefault()
        void copy()
      }
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "s") {
        event.preventDefault()
        void save()
      }
    }
    window.addEventListener("keydown", onKeyDown, true)
    return () => window.removeEventListener("keydown", onKeyDown, true)
  })

  useEffect(() => {
    if (!menu) return
    const onBlur = () => setMenu(null)
    window.addEventListener("blur", onBlur)
    return () => window.removeEventListener("blur", onBlur)
  }, [menu])

  const menuItems: { label: string; icon: Icon; shortcut: string; action: () => void }[] = [
    { label: "复制", icon: Copy, shortcut: "Ctrl+C", action: () => void copy() },
    { label: "保存", icon: DownloadSimple, shortcut: "Ctrl+S", action: () => void save() },
    { label: "原始大小", icon: ArrowsIn, shortcut: "", action: resetSize },
    { label: "关闭", icon: X, shortcut: "Esc", action: () => void close() },
  ]

  return (
    <div
      className="fixed inset-0 select-none overflow-hidden"
      onMouseDown={handleMouseDown}
      onDoubleClick={() => void close()}
      onWheel={handleWheel}
      onContextMenu={(event) => {
        event.preventDefault()
        const width = 168
        const height = menuItems.length * 34 + 8
        setMenu({
          x: Math.max(0, Math.min(event.clientX, window.innerWidth - width)),
          y: Math.max(0, Math.min(event.clientY, window.innerHeight - height)),
        })
      }}
    >
      <div className="absolute inset-0 overflow-hidden border border-sky-400/70 bg-neutral-900 shadow-[0_0_0_1px_rgba(0,0,0,0.25),0_4px_16px_rgba(0,0,0,0.35)]">
        {pin ? (
          <img src={pin.image_url} alt="钉图" draggable={false} className="pointer-events-none block h-full w-full object-fill" />
        ) : null}
      </div>

      {zoomBadgeVisible ? (
        <div className="pointer-events-none absolute left-1/2 top-1/2 -translate-x-1/2 -translate-y-1/2 rounded-md bg-black/70 px-3 py-1.5 font-mono text-sm text-white">
          {Math.round(zoom * 100)}%
        </div>
      ) : null}

      {menu ? (
        <div
          className="absolute z-10 w-[168px] overflow-hidden rounded-md border border-white/10 bg-neutral-900/95 py-1 text-sm text-neutral-100 shadow-xl"
          style={{ left: menu.x, top: menu.y }}
          onMouseDown={(event) => event.stopPropagation()}
        >
          {menuItems.map((item) => (
            <button
              key={item.label}
              type="button"
              className="flex h-[34px] w-full items-center gap-2 px-3 text-left hover:bg-white/10"
              onClick={item.action}
            >
              <item.icon className="size-4" />
              <span className="flex-1">{item.label}</span>
              {item.shortcut ? <span className="text-xs text-white/45">{item.shortcut}</span> : null}
            </button>
          ))}
        </div>
      ) : null}

      {notice ? (
        <div
          className={
            "pointer-events-none absolute bottom-2 left-1/2 max-w-[90%] -translate-x-1/2 truncate rounded px-2 py-1 text-xs shadow " +
            (notice.error ? "bg-red-600/95 text-white" : "bg-black/75 text-white")
          }
        >
          {notice.text}
        </div>
      ) : null}
    </div>
  )
}
