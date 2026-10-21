import { useCallback, useEffect, useRef, useState } from "react"
import type { KeyboardEvent as ReactKeyboardEvent } from "react"
import { ArrowBendUpLeft, Copy, Crosshair, FolderOpen, MagnifyingGlass, Warning } from "@phosphor-icons/react"
import { FileIcon } from "@/components/dustdesk/file-icon"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { useTheme } from "@/hooks/use-theme"
import {
  executeFileDialogEntry,
  fileDialogAssistChangedEvent,
  focusFileDialog,
  formatRelativeTime,
  listFileDialogEntries,
  loadFileDialogAssistState,
  searchFileDialogEntries,
  type FileDialogActionMode,
  type FileDialogAssistState,
  type FileDialogEntry,
  type FileDialogKind,
} from "@/lib/file-dialog-assist"
import { safeListen } from "@/lib/tauri-window"
import { cn } from "@/lib/utils"
import { useDustDeskStore } from "@/stores/dustdesk-store"

const SEARCH_DEBOUNCE_MS = 150

const emptyState: FileDialogAssistState = { dialog: null, organizer_root: "", shortcuts: [] }

type Notice = { kind: "elevated"; path: string } | { kind: "error"; message: string } | { kind: "copied" }

const kindLabels: Record<FileDialogKind, string> = {
  open: "打开文件",
  save: "保存文件",
  folder: "选择文件夹",
}

function actionHint(entry: FileDialogEntry, kind: FileDialogKind) {
  if (entry.is_dir) return "Enter 跳转"
  return kind === "open" ? "Enter 选中" : "Enter 跳到所在文件夹"
}

export function FileDialogAssistPage() {
  useTheme()
  const copyTextToClipboard = useDustDeskStore((state) => state.copyTextToClipboard)
  const [assistState, setAssistState] = useState<FileDialogAssistState>(emptyState)
  const [query, setQuery] = useState("")
  const [browseStack, setBrowseStack] = useState<string[]>([])
  const [entries, setEntries] = useState<FileDialogEntry[]>([])
  const [activeIndex, setActiveIndex] = useState(0)
  const [loading, setLoading] = useState(false)
  const [busy, setBusy] = useState(false)
  const [notice, setNotice] = useState<Notice | null>(null)
  const inputRef = useRef<HTMLInputElement | null>(null)
  const rowRefs = useRef<Array<HTMLDivElement | null>>([])
  const requestRef = useRef(0)
  const browsePath = browseStack.length > 0 ? browseStack[browseStack.length - 1] : null
  const dialogKind = assistState.dialog?.kind ?? "open"
  const dialogId = assistState.dialog?.id ?? ""
  const trimmedQuery = query.trim()

  const reset = useCallback(async () => {
    setQuery("")
    setBrowseStack([])
    setNotice(null)
    try {
      setAssistState(await loadFileDialogAssistState())
    } catch (reason) {
      setNotice({ kind: "error", message: errorMessage(reason) })
    }
  }, [])

  useEffect(() => {
    let disposed = false
    let unlisten: (() => void) | undefined
    // The panel can be created after the first dialog was announced, so read the state first.
    void reset()
    void safeListen<unknown>(fileDialogAssistChangedEvent, () => {
      void reset()
    }).then((dispose) => {
      if (disposed) dispose?.()
      else unlisten = dispose
    })
    return () => {
      disposed = true
      unlisten?.()
    }
  }, [reset])

  useEffect(() => {
    const requestId = ++requestRef.current
    setLoading(true)
    const timer = window.setTimeout(
      () => {
        const request = trimmedQuery ? searchFileDialogEntries(trimmedQuery, browsePath) : listFileDialogEntries(browsePath)
        void request
          .then((items) => {
            if (requestRef.current !== requestId) return
            setEntries(items)
            setActiveIndex(0)
          })
          .catch((reason: unknown) => {
            if (requestRef.current !== requestId) return
            setEntries([])
            setNotice({ kind: "error", message: errorMessage(reason) })
          })
          .finally(() => {
            if (requestRef.current === requestId) setLoading(false)
          })
      },
      trimmedQuery ? SEARCH_DEBOUNCE_MS : 0,
    )
    return () => window.clearTimeout(timer)
  }, [browsePath, dialogId, trimmedQuery])

  useEffect(() => {
    rowRefs.current[activeIndex]?.scrollIntoView({ block: "nearest" })
  }, [activeIndex])

  useEffect(() => {
    // The panel is shown without activation; once the user clicks into it, typing goes to search.
    const handleFocus = () => {
      if (document.activeElement === document.body) inputRef.current?.focus()
    }
    window.addEventListener("focus", handleFocus)
    return () => window.removeEventListener("focus", handleFocus)
  }, [])

  const focusInput = () => inputRef.current?.focus()

  const run = useCallback(
    async (path: string, mode: FileDialogActionMode) => {
      if (busy) return
      setBusy(true)
      setNotice(null)
      try {
        const outcome = await executeFileDialogEntry(path, mode)
        if (outcome === "elevated") setNotice({ kind: "elevated", path })
      } catch (reason) {
        setNotice({ kind: "error", message: errorMessage(reason) })
      } finally {
        setBusy(false)
      }
    },
    [busy],
  )

  const copyPath = async (path: string) => {
    try {
      await copyTextToClipboard(path)
      setNotice({ kind: "copied" })
    } catch (reason) {
      setNotice({ kind: "error", message: errorMessage(reason) })
    }
  }

  const openFolder = (path: string) => {
    setQuery("")
    setBrowseStack((stack) => [...stack, path])
    focusInput()
  }

  const handleKeyDown = (event: ReactKeyboardEvent<HTMLInputElement>) => {
    const entry = entries[activeIndex]
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault()
      if (entries.length === 0) return
      const step = event.key === "ArrowDown" ? 1 : -1
      setActiveIndex((index) => (index + step + entries.length) % entries.length)
      return
    }
    if (event.key === "Enter") {
      event.preventDefault()
      if (entry) void run(entry.path, event.shiftKey ? "locate" : "auto")
      return
    }
    if (event.key === "Tab") {
      event.preventDefault()
      if (entry?.is_dir) openFolder(entry.path)
      return
    }
    if (event.key === "Backspace" && query === "" && browseStack.length > 0) {
      event.preventDefault()
      setBrowseStack((stack) => stack.slice(0, -1))
      return
    }
    if (event.key === "Escape") {
      event.preventDefault()
      if (query) setQuery("")
      else void focusFileDialog().catch(() => undefined)
    }
  }

  const listLabel = trimmedQuery ? "搜索结果" : browsePath ? "浏览" : "收纳目录"

  return (
    <main className="flex h-screen w-screen select-none flex-col gap-2 overflow-hidden border border-border bg-background p-2.5 text-foreground">
      <div className="flex items-center gap-2">
        <div className="flex size-8 shrink-0 items-center justify-center rounded-lg bg-primary/10 text-primary">
          <MagnifyingGlass className="size-4" weight="bold" />
        </div>
        <Input
          ref={inputRef}
          value={query}
          placeholder="搜索收纳目录，支持拼音首字母，例如 ndbg"
          className="h-8 flex-1 rounded-lg bg-background text-sm shadow-none"
          onChange={(event) => setQuery(event.target.value)}
          onKeyDown={handleKeyDown}
        />
        <Badge variant="outline" className="h-6 shrink-0 text-muted-foreground">
          {kindLabels[dialogKind]}
        </Badge>
      </div>

      {assistState.shortcuts.length > 0 ? (
        <div className="flex shrink-0 gap-1.5 overflow-x-auto pb-0.5 [scrollbar-width:none]">
          {assistState.shortcuts.map((shortcut, index) => (
            <Button
              key={shortcut.path}
              type="button"
              variant={index === 0 ? "secondary" : "outline"}
              size="xs"
              className="shrink-0 rounded-full"
              title={shortcut.path}
              disabled={busy}
              onClick={() => void run(shortcut.path, "auto")}
            >
              <FolderOpen weight="duotone" />
              {shortcut.name}
            </Button>
          ))}
        </div>
      ) : null}

      <div className="flex min-w-0 shrink-0 items-center gap-2 text-xs text-muted-foreground">
        {browsePath ? (
          <Button
            type="button"
            variant="ghost"
            size="xs"
            className="shrink-0"
            onClick={() => {
              setBrowseStack((stack) => stack.slice(0, -1))
              focusInput()
            }}
          >
            <ArrowBendUpLeft weight="bold" />
            返回
          </Button>
        ) : null}
        <span className="shrink-0 font-medium text-foreground">{listLabel}</span>
        <span className="min-w-0 flex-1 truncate" title={browsePath ?? assistState.organizer_root}>
          {browsePath ?? assistState.organizer_root}
        </span>
        <span className="shrink-0 tabular-nums">{loading ? "检索中" : `${entries.length} 项`}</span>
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto rounded-lg border bg-muted/20 p-1">
        {entries.length > 0 ? (
          entries.map((entry, index) => (
            <div
              key={entry.path}
              ref={(node) => {
                rowRefs.current[index] = node
              }}
              role="button"
              tabIndex={-1}
              className={cn(
                "group flex cursor-default items-center gap-2.5 rounded-md px-2 py-1.5",
                activeIndex === index ? "bg-accent text-accent-foreground" : "hover:bg-muted",
              )}
              onMouseEnter={() => setActiveIndex(index)}
              onMouseDown={(event) => event.preventDefault()}
              onClick={() => void run(entry.path, "auto")}
            >
              <FileIcon name={entry.name} extension={entry.extension} isDir={entry.is_dir} className="size-7 rounded-md" />
              <div className="min-w-0 flex-1">
                <HighlightedName name={entry.name} highlight={entry.highlight} />
                <p className="truncate text-[11px] leading-4 text-muted-foreground" title={entry.parent}>
                  {entry.parent}
                </p>
              </div>
              <div className="flex shrink-0 items-center gap-2 text-[11px] text-muted-foreground">
                {activeIndex === index ? <span>{actionHint(entry, dialogKind)}</span> : null}
                <span className="w-20 text-right tabular-nums">{formatRelativeTime(entry.modified_ms)}</span>
                {entry.is_dir ? (
                  <Button
                    type="button"
                    variant="ghost"
                    size="xs"
                    title="在面板里展开（Tab）"
                    onClick={(event) => {
                      event.stopPropagation()
                      openFolder(entry.path)
                    }}
                  >
                    展开
                  </Button>
                ) : (
                  <Button
                    type="button"
                    variant="ghost"
                    size="xs"
                    title="跳到所在文件夹并填入文件名，不确认（Shift+Enter）"
                    disabled={busy}
                    onClick={(event) => {
                      event.stopPropagation()
                      void run(entry.path, "locate")
                    }}
                  >
                    <Crosshair weight="bold" />
                    定位
                  </Button>
                )}
              </div>
            </div>
          ))
        ) : (
          <div className="flex h-full items-center justify-center text-xs text-muted-foreground">
            {loading ? "正在读取..." : trimmedQuery ? "没有找到匹配项" : "这个文件夹是空的"}
          </div>
        )}
      </div>

      {notice ? <NoticeBar notice={notice} onCopy={(path) => void copyPath(path)} /> : null}

      <footer className="flex shrink-0 items-center justify-between gap-2 text-[11px] text-muted-foreground">
        <span className="truncate">↑↓ 选择 · Enter 执行 · Shift+Enter 定位 · Tab 展开 · Backspace 返回 · Esc 回到对话框</span>
        <span className="shrink-0">DustDesk</span>
      </footer>
    </main>
  )
}

function NoticeBar({ notice, onCopy }: { notice: Notice; onCopy: (path: string) => void }) {
  if (notice.kind === "copied") {
    return <p className="shrink-0 text-xs text-emerald-600 dark:text-emerald-400">已复制路径，可在对话框的文件名框里粘贴</p>
  }
  return (
    <div className="flex shrink-0 items-center gap-2 rounded-md border border-destructive/30 bg-destructive/10 px-2 py-1 text-xs text-destructive">
      <Warning className="size-4 shrink-0" weight="fill" />
      <span className="min-w-0 flex-1 truncate">
        {notice.kind === "elevated" ? "该对话框所属程序以管理员身份运行，无法自动填入" : notice.message}
      </span>
      {notice.kind === "elevated" ? (
        <Button type="button" variant="outline" size="xs" className="shrink-0" onClick={() => onCopy(notice.path)}>
          <Copy weight="bold" />
          复制路径
        </Button>
      ) : null}
    </div>
  )
}

function HighlightedName({ name, highlight }: { name: string; highlight: number[] }) {
  const marked = new Set(highlight)
  const segments: Array<{ text: string; marked: boolean }> = []
  Array.from(name).forEach((char, index) => {
    const isMarked = marked.has(index)
    const last = segments[segments.length - 1]
    if (last && last.marked === isMarked) last.text += char
    else segments.push({ text: char, marked: isMarked })
  })
  return (
    <p className="truncate text-sm leading-5" title={name}>
      {segments.map((segment, index) =>
        segment.marked ? (
          <mark key={index} className="rounded-sm bg-primary/20 px-px text-foreground">
            {segment.text}
          </mark>
        ) : (
          <span key={index}>{segment.text}</span>
        ),
      )}
    </p>
  )
}

function errorMessage(reason: unknown) {
  return reason instanceof Error ? reason.message : String(reason)
}
