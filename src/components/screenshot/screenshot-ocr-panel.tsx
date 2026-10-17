import { useEffect, useLayoutEffect, useRef, useState } from "react"
import { Check, CircleNotch, Copy, X } from "@phosphor-icons/react"
import type { Rect } from "@/lib/screenshot/geometry"

export type OcrPanelState =
  | { status: "loading" }
  | { status: "done"; text: string }
  | { status: "error"; message: string }

const PANEL_WIDTH = 340
const PANEL_HEIGHT = 380
const GAP = 10
const EDGE = 6

export function ScreenshotOcrPanel({
  anchor,
  viewportWidth,
  viewportHeight,
  state,
  onClose,
}: {
  /** Selection rect in CSS pixels. */
  anchor: Rect
  viewportWidth: number
  viewportHeight: number
  state: OcrPanelState
  onClose: () => void
}) {
  const [draft, setDraft] = useState("")
  const [copied, setCopied] = useState(false)
  const [copyError, setCopyError] = useState("")
  const textareaRef = useRef<HTMLTextAreaElement | null>(null)
  const [position, setPosition] = useState({ left: EDGE, top: EDGE })

  useEffect(() => {
    if (state.status === "done") {
      setDraft(state.text)
      setCopied(false)
      setCopyError("")
    }
  }, [state])

  useLayoutEffect(() => {
    const right = anchor.x + anchor.width
    let left: number
    if (right + GAP + PANEL_WIDTH <= viewportWidth - EDGE) left = right + GAP
    else if (anchor.x - GAP - PANEL_WIDTH >= EDGE) left = anchor.x - GAP - PANEL_WIDTH
    else left = Math.max(EDGE, viewportWidth - PANEL_WIDTH - EDGE)
    const top = Math.min(Math.max(EDGE, anchor.y), Math.max(EDGE, viewportHeight - PANEL_HEIGHT - EDGE))
    setPosition({ left, top })
  }, [anchor.height, anchor.width, anchor.x, anchor.y, viewportHeight, viewportWidth])

  const copyAll = async () => {
    setCopyError("")
    try {
      await navigator.clipboard.writeText(draft)
      setCopied(true)
      window.setTimeout(() => setCopied(false), 1500)
    } catch (reason) {
      setCopyError(reason instanceof Error ? reason.message : String(reason))
    }
  }

  return (
    <div
      className="absolute z-30 flex flex-col overflow-hidden rounded-lg border border-white/10 bg-neutral-900/95 text-neutral-100 shadow-2xl"
      style={{ left: position.left, top: position.top, width: PANEL_WIDTH, height: PANEL_HEIGHT }}
      onPointerDown={(event) => event.stopPropagation()}
      onDoubleClick={(event) => event.stopPropagation()}
      onContextMenu={(event) => event.stopPropagation()}
      onKeyDown={(event) => {
        if (event.key !== "Escape") event.stopPropagation()
      }}
    >
      <div className="flex items-center justify-between border-b border-white/10 px-3 py-2">
        <span className="text-sm font-medium">识别文字</span>
        <button type="button" aria-label="关闭" className="grid size-7 place-items-center rounded-md hover:bg-white/10" onClick={onClose}>
          <X className="size-4" />
        </button>
      </div>
      {state.status === "loading" ? (
        <div className="flex flex-1 flex-col items-center justify-center gap-2 text-sm text-white/70">
          <CircleNotch className="size-6 animate-spin text-sky-400" weight="bold" />
          正在识别文字…
        </div>
      ) : state.status === "error" ? (
        <div className="flex flex-1 items-center justify-center px-5 text-center text-sm leading-6 text-red-300">{state.message}</div>
      ) : (
        <>
          <textarea
            ref={textareaRef}
            value={draft}
            spellCheck={false}
            placeholder="未识别到文字"
            className="min-h-0 flex-1 resize-none bg-transparent px-3 py-2 text-sm leading-6 text-neutral-100 outline-none placeholder:text-white/40"
            onChange={(event) => setDraft(event.target.value)}
          />
          <div className="flex items-center justify-between gap-2 border-t border-white/10 px-3 py-2">
            <span className="truncate text-xs text-red-300">{copyError}</span>
            <button
              type="button"
              disabled={!draft}
              className="flex h-8 shrink-0 items-center gap-1.5 rounded-md bg-sky-500 px-3 text-sm font-medium text-white hover:bg-sky-400 disabled:opacity-40"
              onClick={() => void copyAll()}
            >
              {copied ? <Check className="size-4" weight="bold" /> : <Copy className="size-4" />}
              {copied ? "已复制" : "复制全部"}
            </button>
          </div>
        </>
      )}
    </div>
  )
}
