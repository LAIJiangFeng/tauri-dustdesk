import { useCallback, useEffect, useRef, useState } from "react"
import type { WheelEvent as ReactWheelEvent } from "react"
import { CircleNotch, ClipboardText } from "@phosphor-icons/react"
import { convertFileSrc } from "@tauri-apps/api/core"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Card, CardContent } from "@/components/ui/card"
import { useTheme } from "@/hooks/use-theme"
import { safeCurrentWindow, safeListen } from "@/lib/tauri-window"
import { cn, truncate } from "@/lib/utils"
import { useDustDeskStore } from "@/stores/dustdesk-store"
import type { ClipboardHistoryItem } from "@/types"

const WHEEL_DEAD_ZONE_PX = 18
const WHEEL_SCROLL_FACTOR = 0.35
const SHORTCUT_EVENT_GUARD_MS = 180

function waitForNextPaint() {
  return new Promise<void>((resolve) => {
    requestAnimationFrame(() => resolve())
  })
}

function sleep(ms: number) {
  return new Promise<void>((resolve) => window.setTimeout(resolve, ms))
}

function wheelDeltaToPixels(delta: number, deltaMode: number, viewportSize: number) {
  if (deltaMode === WheelEvent.DOM_DELTA_LINE) return delta * 16
  if (deltaMode === WheelEvent.DOM_DELTA_PAGE) return delta * viewportSize
  return delta
}

export function ClipboardOverlayPage() {
  useTheme()
  const clipboard = useDustDeskStore((state) => state.snapshot.clipboard)
  const load = useDustDeskStore((state) => state.load)
  const pasteClipboardItem = useDustDeskStore((state) => state.pasteClipboardItem)
  const hideClipboardOverlay = useDustDeskStore((state) => state.hideClipboardOverlay)
  const [activeIndex, setActiveIndex] = useState(0)
  const [error, setError] = useState("")
  const [pastingItemId, setPastingItemId] = useState("")
  const items = clipboard
  const itemsRef = useRef(items)
  const itemRefs = useRef<Array<HTMLButtonElement | null>>([])
  const activeIndexRef = useRef(activeIndex)
  const shownRef = useRef(false)
  const openingRef = useRef(false)
  const choosingRef = useRef(false)
  const lastShortcutEventAtRef = useRef(Number.NEGATIVE_INFINITY)

  useEffect(() => {
    itemsRef.current = items
  }, [items])

  useEffect(() => {
    activeIndexRef.current = activeIndex
  }, [activeIndex])

  const refreshOverlay = useCallback(async () => {
    setError("")
    await load()
    activeIndexRef.current = 0
    setActiveIndex(0)
  }, [load])

  const close = useCallback(async () => {
    shownRef.current = false
    openingRef.current = false
    await hideClipboardOverlay()
  }, [hideClipboardOverlay])

  const choose = useCallback(
    async (item: ClipboardHistoryItem) => {
      if (choosingRef.current) return
      choosingRef.current = true
      setError("")
      setPastingItemId(item.id)
      try {
        await waitForNextPaint()
        await sleep(item.kind === "Image" ? 180 : 80)
        await pasteClipboardItem(item.id)
        shownRef.current = false
        openingRef.current = false
      } catch (reason) {
        try {
          const currentWindow = safeCurrentWindow()
          await currentWindow?.show()
          await currentWindow?.setFocus()
        } catch {
          // If the overlay cannot be restored, keep the error state for the next open.
        }
        setError(reason instanceof Error ? reason.message : String(reason))
        shownRef.current = true
      } finally {
        setPastingItemId("")
        choosingRef.current = false
      }
    },
    [close, pasteClipboardItem],
  )

  const chooseActive = useCallback(async () => {
    const item = itemsRef.current[activeIndexRef.current]
    if (!item) {
      await close()
      return
    }

    await choose(item)
  }, [choose, close])

  const advance = useCallback((direction = 1) => {
    setActiveIndex((index) => {
      const count = itemsRef.current.length
      if (count === 0) return 0
      const nextIndex = (index + direction + count) % count
      activeIndexRef.current = nextIndex
      return nextIndex
    })
  }, [])

  const jumpTo = useCallback((index: number) => {
    setActiveIndex(() => {
      const count = itemsRef.current.length
      if (count === 0) return 0
      const nextIndex = Math.min(Math.max(index, 0), count - 1)
      activeIndexRef.current = nextIndex
      return nextIndex
    })
  }, [])

  const handleWheel = useCallback((event: ReactWheelEvent<HTMLDivElement>) => {
    const viewportSize = event.currentTarget.clientWidth
    const deltaX = wheelDeltaToPixels(event.deltaX, event.deltaMode, viewportSize)
    const deltaY = wheelDeltaToPixels(event.deltaY, event.deltaMode, viewportSize)
    const primaryDelta = Math.abs(deltaX) > Math.abs(deltaY) ? deltaX : deltaY

    event.preventDefault()
    if (Math.abs(primaryDelta) < WHEEL_DEAD_ZONE_PX) return
    event.currentTarget.scrollBy({ left: primaryDelta * WHEEL_SCROLL_FACTOR, behavior: "auto" })
  }, [])

  useEffect(() => {
    let unlisten: (() => void) | undefined
    let disposed = false

    const handleShortcut = () => {
      const now = performance.now()
      if (now - lastShortcutEventAtRef.current < SHORTCUT_EVENT_GUARD_MS) return
      lastShortcutEventAtRef.current = now

      if (openingRef.current || choosingRef.current) return

      if (shownRef.current) {
        advance()
        return
      }

      shownRef.current = true
      openingRef.current = true
      void refreshOverlay().finally(() => {
        openingRef.current = false
      })
    }

    void (async () => {
      const dispose = await safeListen("dustdesk://clipboard-shortcut", handleShortcut)
      if (disposed) {
        dispose?.()
        return
      }

      unlisten = dispose
      const currentWindow = safeCurrentWindow()
      if (!currentWindow) {
        handleShortcut()
        return
      }

      try {
        if (await currentWindow.isVisible() && !disposed) handleShortcut()
      } catch {
        if (!disposed) handleShortcut()
      }
    })()

    return () => {
      disposed = true
      unlisten?.()
    }
  }, [advance, refreshOverlay])

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      const currentItems = itemsRef.current
      const currentIndex = activeIndexRef.current

      if (event.key === "Escape") {
        event.preventDefault()
        void close()
        return
      }

      if (event.key === "Tab") {
        event.preventDefault()
        return
      }

      if (event.key === "ArrowRight") {
        event.preventDefault()
        advance(1)
        return
      }

      if (event.key === "ArrowLeft") {
        event.preventDefault()
        advance(-1)
        return
      }

      if (event.key === "Home") {
        event.preventDefault()
        jumpTo(0)
        return
      }

      if (event.key === "End") {
        event.preventDefault()
        jumpTo(currentItems.length - 1)
        return
      }

      if (event.key === "Enter" && currentItems[currentIndex]) {
        event.preventDefault()
        void choose(currentItems[currentIndex])
        return
      }

      if (/^[1-9]$/.test(event.key)) {
        const index = Number(event.key) - 1
        if (currentItems[index]) {
          event.preventDefault()
          void choose(currentItems[index])
        }
      }
    }

    const onKeyUp = (event: KeyboardEvent) => {
      if ((event.key === "Control" || event.code === "ControlLeft" || event.code === "ControlRight") && shownRef.current) {
        event.preventDefault()
        void chooseActive()
      }
    }

    window.addEventListener("keydown", onKeyDown, true)
    window.addEventListener("keyup", onKeyUp, true)
    return () => {
      window.removeEventListener("keydown", onKeyDown, true)
      window.removeEventListener("keyup", onKeyUp, true)
    }
  }, [advance, choose, chooseActive, close, jumpTo])

  useEffect(() => {
    const onBlur = () => {
      window.setTimeout(() => {
        if (!document.hasFocus()) {
          void close()
        }
      }, 120)
    }

    window.addEventListener("blur", onBlur)
    return () => window.removeEventListener("blur", onBlur)
  }, [close])

  useEffect(() => {
    if (activeIndex >= items.length) {
      const nextIndex = Math.max(0, items.length - 1)
      activeIndexRef.current = nextIndex
      setActiveIndex(nextIndex)
    }
  }, [activeIndex, items.length])

  useEffect(() => {
    itemRefs.current[activeIndex]?.scrollIntoView({
      behavior: "smooth",
      block: "nearest",
      inline: "center",
    })
  }, [activeIndex])

  return (
    <main className="flex h-screen w-screen items-center overflow-hidden border bg-background px-5 py-8 text-foreground shadow-2xl">
      <div className="flex min-w-0 flex-1 items-center gap-5">
        <Badge className="hidden h-12 shrink-0 gap-2 rounded-2xl border bg-card px-5 text-sm text-foreground md:inline-flex" variant="outline">
          <ClipboardText className="size-4" weight="duotone" />
          {items.length > 0 ? `${activeIndex + 1}/${items.length}` : "Ctrl + Tab"}
        </Badge>

        <div
          className="min-w-0 flex-1 overflow-x-auto overflow-y-hidden whitespace-nowrap px-4 py-5 [scrollbar-color:var(--border)_transparent] [scrollbar-width:thin]"
          onWheel={handleWheel}
        >
          <div className="flex gap-5 py-3">
            {items.length > 0 ? (
              items.map((item, index) => (
                <ClipboardChoiceButton
                  key={item.id || `${item.created_at}-${index}`}
                  item={item}
                  index={index}
                  active={activeIndex === index}
                  pasting={pastingItemId === item.id}
                  locked={Boolean(pastingItemId)}
                  setRef={(node) => {
                    itemRefs.current[index] = node
                  }}
                  onChoose={choose}
                />
              ))
            ) : (
              <Card className="h-40 w-full border bg-card">
                <CardContent className="flex h-full items-center justify-center text-sm text-muted-foreground">暂无剪贴历史</CardContent>
              </Card>
            )}
          </div>
        </div>

        <div className="grid shrink-0 gap-2">
          <Badge className="justify-center border bg-card text-muted-foreground" variant="outline">{items.length} 条</Badge>
          <Button className="h-12 rounded-2xl border bg-card px-6 text-foreground hover:bg-muted" variant="outline" onClick={() => void close()}>
            关闭
          </Button>
        </div>
      </div>
      {pastingItemId ? (
        <div className="absolute bottom-1 left-4 right-4 flex items-center justify-center gap-2 truncate text-xs font-medium text-primary">
          <CircleNotch className="size-3.5 animate-spin" weight="bold" />
          正在粘贴，请稍候
        </div>
      ) : error ? (
        <div className="absolute bottom-1 left-4 right-4 truncate text-xs text-destructive">{error}</div>
      ) : null}
    </main>
  )
}

function ClipboardChoiceButton({
  item,
  index,
  active,
  pasting,
  locked,
  setRef,
  onChoose,
}: {
  item: ClipboardHistoryItem
  index: number
  active: boolean
  pasting: boolean
  locked: boolean
  setRef: (node: HTMLButtonElement | null) => void
  onChoose: (item: ClipboardHistoryItem) => void
}) {
  return (
    <Button
      ref={setRef}
      type="button"
      variant="ghost"
      className={cn(
        "group relative h-40 w-72 shrink-0 overflow-hidden rounded-3xl border bg-card p-0 text-left text-card-foreground shadow-sm transition-all duration-150 hover:border-ring/40 hover:bg-muted hover:text-foreground hover:shadow-lg",
        active && "scale-[1.02] border-ring bg-accent text-accent-foreground shadow-xl ring-2 ring-ring/40 ring-offset-2 ring-offset-background",
        pasting && "border-primary/60 ring-2 ring-primary/30",
      )}
      aria-busy={pasting}
      aria-disabled={locked}
      onClick={() => onChoose(item)}
    >
      <span
        className={cn(
          "absolute inset-x-5 top-0 h-1 rounded-b-full bg-muted-foreground/30 transition-colors",
          active && "bg-primary",
        )}
      />
      <Card className="h-full w-full border-0 bg-transparent py-0 shadow-none">
        <CardContent className="grid h-full min-w-0 grid-rows-[auto_minmax(0,1fr)_auto] gap-3 p-5">
          <div className="flex items-center justify-between gap-2">
            <Badge
              className={cn(
                "h-8 rounded-full px-3 text-sm tabular-nums",
                active ? "bg-primary text-primary-foreground" : "border bg-background text-muted-foreground",
              )}
            >
              {index + 1}
            </Badge>
            <span className="truncate text-sm font-medium text-muted-foreground">{item.kind}</span>
          </div>
          {item.kind === "Image" && hasImagePreview(item) ? (
            <div className="min-h-0 overflow-hidden rounded-xl border bg-muted/30">
              <img src={imageSource(item)} alt="图片剪贴内容" className="h-full w-full object-contain" />
            </div>
          ) : (
            <div className="min-h-0 min-w-0 overflow-hidden text-left text-[15px] font-semibold leading-6 text-card-foreground whitespace-pre-wrap break-words [overflow-wrap:anywhere]">
              {truncate(item.text || "图片剪贴内容", 120)}
            </div>
          )}
          <div className="flex items-center justify-between text-xs text-muted-foreground">
            <span>{active ? "松开 Ctrl 粘贴" : "Tab 切换"}</span>
            <span>{index + 1} 选择</span>
          </div>
        </CardContent>
      </Card>
      {pasting ? (
        <span className="absolute inset-0 flex flex-col items-center justify-center gap-2 bg-background/85 text-sm font-semibold text-foreground backdrop-blur-sm">
          <CircleNotch className="size-7 animate-spin text-primary" weight="bold" />
          {item.kind === "Image" ? "正在准备图片" : "正在粘贴"}
        </span>
      ) : null}
    </Button>
  )
}

function hasImagePreview(item: ClipboardHistoryItem) {
  return Boolean(item.image_thumb_path || item.image_path || item.image_png_base64)
}

function imageSource(item: ClipboardHistoryItem) {
  if (item.image_png_base64) {
    return `data:image/png;base64,${item.image_png_base64}`
  }

  const path = item.image_thumb_path || item.image_path
  if (path && "__TAURI_INTERNALS__" in window) {
    return convertFileSrc(path)
  }
  return ""
}
