import { useCallback, useEffect, useRef, useState } from "react"
import { ArrowsClockwise } from "@phosphor-icons/react"
import { safeListen } from "@/lib/tauri-window"
import { cn } from "@/lib/utils"
import { normalizeDesktopTransferProgress } from "@/stores/dustdesk-store"
import type { DesktopTransferOperation, DesktopTransferProgress as DesktopTransferProgressPayload } from "@/types"

export const desktopTransferProgressEvent = "dustdesk://desktop-transfer-progress"

/** Operations that already publish their own `dustdesk://desktop-operation` messages. */
const backgroundOperations = new Set<DesktopTransferOperation>(["classify", "restore-all", "exit-restore", "restart-recollect"])

export interface TransferProgressView {
  label: string
  detail?: string
  /** 0..100, or null when only the label is shown. */
  percent: number | null
}

/**
 * Item position supplied by the page when it runs several single-item commands in a row (multi
 * restore), where each backend call only knows about itself (1/1).
 */
export interface TransferItemPosition {
  index: number
  total: number
  name: string
}

/**
 * Transfer progress for one WebView. `ownTransfer` only follows drops started by this window
 * while `trackTransfer` runs; events arriving after the command settled are ignored.
 * `backgroundTransfer` follows classify / restore-all / exit-restore / restart-recollect.
 */
export function useDesktopTransferProgress() {
  const [ownTransfer, setOwnTransfer] = useState<DesktopTransferProgressPayload | null>(null)
  const [backgroundTransfer, setBackgroundTransfer] = useState<DesktopTransferProgressPayload | null>(null)
  const [ownPosition, setOwnPositionState] = useState<TransferItemPosition | null>(null)
  const ownTokenRef = useRef(0)
  const nextTokenRef = useRef(0)

  useEffect(() => {
    let disposed = false
    let unlisten: (() => void) | undefined
    void safeListen<unknown>(desktopTransferProgressEvent, (event) => {
      if (disposed) return
      const progress = normalizeDesktopTransferProgress(event.payload)
      if (!progress) return
      if (backgroundOperations.has(progress.operation)) {
        setBackgroundTransfer(progress.status === "running" ? progress : null)
        return
      }
      if (!ownTokenRef.current) return
      setOwnTransfer(progress.status === "running" ? progress : null)
    }).then((dispose) => {
      if (disposed) dispose?.()
      else unlisten = dispose
    })
    return () => {
      disposed = true
      unlisten?.()
    }
  }, [])

  const trackTransfer = useCallback(async <T,>(run: () => Promise<T>): Promise<T> => {
    nextTokenRef.current += 1
    const token = nextTokenRef.current
    ownTokenRef.current = token
    setOwnTransfer(null)
    setOwnPositionState(null)
    try {
      return await run()
    } finally {
      if (ownTokenRef.current === token) {
        ownTokenRef.current = 0
        setOwnTransfer(null)
        setOwnPositionState(null)
      }
    }
  }, [])

  /** Only takes effect while a tracked transfer runs; cleared when it settles. */
  const setOwnPosition = useCallback((position: TransferItemPosition) => {
    if (ownTokenRef.current) setOwnPositionState(position)
  }, [])

  return { ownTransfer, ownPosition, backgroundTransfer, trackTransfer, setOwnPosition }
}

export function formatTransferBytes(bytes: number) {
  if (bytes < 1024) return `${bytes} B`
  const units = ["KB", "MB", "GB", "TB"]
  let value = bytes / 1024
  let unit = 0
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024
    unit += 1
  }
  return `${value.toFixed(1)} ${units[unit]}`
}

/** `已复制 1.2 GB / 3.4 GB`, only while an item is copied across volumes. */
export function transferBytesDetail(progress: DesktopTransferProgressPayload | null): string | undefined {
  if (!progress?.copying || progress.bytes_total <= 0) return undefined
  let text = `已复制 ${formatTransferBytes(progress.bytes_done)} / ${formatTransferBytes(progress.bytes_total)}`
  if (progress.verified_bytes > 0 && progress.verified_bytes < progress.bytes_total) {
    text += `，已校验 ${formatTransferBytes(progress.verified_bytes)}`
  }
  return text
}

/** Byte ratio of the copy in flight; otherwise the item ratio when there are several items. */
export function transferPercent(progress: DesktopTransferProgressPayload | null): number | null {
  if (!progress) return null
  if (progress.copying && progress.bytes_total > 0) return Math.min(100, (progress.bytes_done / progress.bytes_total) * 100)
  if (progress.items_total > 1) return Math.min(100, (Math.max(0, progress.item_index - 1) / progress.items_total) * 100)
  return null
}

/** Label for a drop started by this window, e.g. `正在收纳 2/5 项到「文档」：泊锐科技`. */
export function describeOwnTransfer(
  progress: DesktopTransferProgressPayload | null,
  fallbackLabel: string,
  itemPosition?: TransferItemPosition | null,
): TransferProgressView {
  if (itemPosition) return describePositionedRestore(progress, itemPosition)
  if (!progress || !progress.current) return { label: fallbackLabel, percent: null }
  const position = progress.items_total > 0 ? ` ${Math.max(1, progress.item_index)}/${progress.items_total} 项` : ""
  const target = progress.target || "分类"
  let label: string
  switch (progress.operation) {
    case "archive":
      label = `正在收纳${position}到「${target}」：${progress.current}`
      break
    case "move":
      label = `正在移动${position}到「${target}」：${progress.current}`
      break
    case "restore":
      label = `正在移回桌面：${progress.current}`
      break
    default:
      label = fallbackLabel
  }
  return { label, detail: transferBytesDetail(progress), percent: transferPercent(progress) }
}

/**
 * `正在移回桌面 2/5 项：泊锐科技` for a multi restore. The page's position wins over the backend's
 * per-call 1/1, and stays in place between items when the per-item finished event clears progress.
 */
function describePositionedRestore(progress: DesktopTransferProgressPayload | null, position: TransferItemPosition): TransferProgressView {
  const running = progress?.operation === "restore" && progress.status === "running" ? progress : null
  const name = running?.copying && running.current ? running.current : position.name
  const label = position.total > 1 ? `正在移回桌面 ${position.index}/${position.total} 项：${name}` : `正在移回桌面：${name}`
  const detail = transferBytesDetail(running)
  let percent: number | null = null
  if (running?.copying && running.bytes_total > 0) percent = Math.min(100, (running.bytes_done / running.bytes_total) * 100)
  else if (position.total > 1) percent = Math.min(100, (Math.max(0, position.index - 1) / position.total) * 100)
  return { label, detail, percent }
}

/** Background/lifecycle operations already carry byte detail in their message. */
export function describeBackgroundTransfer(progress: DesktopTransferProgressPayload | null, message: string): TransferProgressView {
  return { label: message, percent: transferPercent(progress) }
}

export function TransferProgress({ view, tone = "dark", className }: { view: TransferProgressView; tone?: "dark" | "card"; className?: string }) {
  const dark = tone === "dark"
  return (
    <div className={cn("flex min-w-0 max-w-[min(28rem,90%)] flex-col gap-2", className)}>
      <div className="flex min-w-0 items-center gap-2">
        <ArrowsClockwise className={cn("shrink-0 animate-spin", dark ? "size-4 text-emerald-300" : "size-5 text-primary")} weight="duotone" />
        <span className="min-w-0 break-words">{view.label}</span>
      </div>
      {view.percent !== null ? (
        <div
          className={cn("h-1.5 w-full overflow-hidden rounded-full", dark ? "bg-white/15" : "bg-muted")}
          role="progressbar"
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={Math.round(view.percent)}
        >
          <div className={cn("h-full rounded-full transition-[width] duration-200", dark ? "bg-emerald-400" : "bg-primary")} style={{ width: `${view.percent}%` }} />
        </div>
      ) : null}
      {view.detail ? <div className={cn("text-[11px] font-normal tabular-nums", dark ? "text-white/70" : "text-muted-foreground")}>{view.detail}</div> : null}
    </div>
  )
}
