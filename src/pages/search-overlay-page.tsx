import { useCallback, useDeferredValue, useEffect, useRef, useState } from "react"
import type { KeyboardEvent as ReactKeyboardEvent } from "react"
import { ChartBar, ClockCounterClockwise, MagnifyingGlass, Warning } from "@phosphor-icons/react"
import { FileIcon } from "@/components/dustdesk/file-icon"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Card, CardContent } from "@/components/ui/card"
import { Input } from "@/components/ui/input"
import { ScrollArea } from "@/components/ui/scroll-area"
import { useTheme } from "@/hooks/use-theme"
import { safeListen } from "@/lib/tauri-window"
import { cn, truncate } from "@/lib/utils"
import { useDustDeskStore } from "@/stores/dustdesk-store"
import type { SearchItem, SearchOverlayData } from "@/types"

type SearchTab = "recent" | "frequent"

const SHORTCUT_EVENT_GUARD_MS = 180

const emptyOverlay: SearchOverlayData = {
  settings: {
    clipboard_shortcut: "Ctrl+Tab",
    search_enabled: true,
    search_shortcut: "Ctrl+Space",
    search_paths: [],
  },
  paths: [],
  recent: [],
  frequent: [],
}

export function SearchOverlayPage() {
  useTheme()
  const loadSearchOverlay = useDustDeskStore((state) => state.loadSearchOverlay)
  const searchItems = useDustDeskStore((state) => state.searchItems)
  const openSearchItem = useDustDeskStore((state) => state.openSearchItem)
  const hideSearchOverlay = useDustDeskStore((state) => state.hideSearchOverlay)
  const inputRef = useRef<HTMLInputElement | null>(null)
  const itemRefs = useRef<Array<HTMLButtonElement | null>>([])
  const itemsRef = useRef<SearchItem[]>([])
  const activeIndexRef = useRef(0)
  const queryRef = useRef("")
  const openingRef = useRef(false)
  const searchSeqRef = useRef(0)
  const lastShortcutEventAtRef = useRef(Number.NEGATIVE_INFINITY)
  const [overlay, setOverlay] = useState<SearchOverlayData>(emptyOverlay)
  const [query, setQuery] = useState("")
  const deferredQuery = useDeferredValue(query)
  const [activeTab, setActiveTab] = useState<SearchTab>("recent")
  const [results, setResults] = useState<SearchItem[]>([])
  const [activeIndex, setActiveIndex] = useState(0)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState("")
  const isSearching = query.trim().length > 0
  const items = isSearching ? results : activeTab === "recent" ? overlay.recent : overlay.frequent

  useEffect(() => {
    itemsRef.current = items
  }, [items])

  useEffect(() => {
    activeIndexRef.current = activeIndex
  }, [activeIndex])

  useEffect(() => {
    queryRef.current = query
  }, [query])

  const focusInput = useCallback(() => {
    window.requestAnimationFrame(() => {
      inputRef.current?.focus()
      inputRef.current?.select()
    })
  }, [])

  const setActive = useCallback((index: number) => {
    setActiveIndex(() => {
      const count = itemsRef.current.length
      const nextIndex = count === 0 ? 0 : Math.min(Math.max(index, 0), count - 1)
      activeIndexRef.current = nextIndex
      return nextIndex
    })
  }, [])

  const refreshOverlay = useCallback(async () => {
    setError("")
    setLoading(true)
    try {
      const data = await loadSearchOverlay()
      setOverlay(data)
      setQuery("")
      setResults([])
      setActiveTab("recent")
      setActive(0)
      focusInput()
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason))
    } finally {
      setLoading(false)
    }
  }, [focusInput, loadSearchOverlay, setActive])

  const close = useCallback(async () => {
    await hideSearchOverlay()
  }, [hideSearchOverlay])

  const choose = useCallback(
    async (item: SearchItem | undefined) => {
      if (!item || openingRef.current) return
      openingRef.current = true
      setError("")
      try {
        await openSearchItem(item)
      } catch (reason) {
        setError(reason instanceof Error ? reason.message : String(reason))
      } finally {
        openingRef.current = false
      }
    },
    [openSearchItem],
  )

  const moveActive = useCallback(
    (delta: number) => {
      const count = itemsRef.current.length
      if (count === 0) {
        setActive(0)
        return
      }
      const nextIndex = (activeIndexRef.current + delta + count) % count
      setActive(nextIndex)
    },
    [setActive],
  )

  const switchTab = useCallback(() => {
    setActiveTab((tab) => (tab === "recent" ? "frequent" : "recent"))
    setActive(0)
    focusInput()
  }, [focusInput, setActive])

  useEffect(() => {
    void refreshOverlay()
  }, [refreshOverlay])

  useEffect(() => {
    let unlisten: (() => void) | undefined
    let disposed = false

    void safeListen("dustdesk://search-shortcut", () => {
      const now = performance.now()
      if (now - lastShortcutEventAtRef.current < SHORTCUT_EVENT_GUARD_MS) return
      lastShortcutEventAtRef.current = now
      void refreshOverlay()
    }).then((dispose) => {
      if (disposed) {
        dispose?.()
        return
      }

      unlisten = dispose
    })

    return () => {
      disposed = true
      unlisten?.()
    }
  }, [refreshOverlay])

  useEffect(() => {
    const term = deferredQuery.trim()
    const seq = ++searchSeqRef.current

    if (!term) {
      setResults([])
      setLoading(false)
      setActive(0)
      return
    }

    setLoading(true)
    const timer = window.setTimeout(() => {
      void searchItems(term)
        .then((nextItems) => {
          if (seq !== searchSeqRef.current) return
          setResults(nextItems)
          setActive(0)
        })
        .catch((reason) => {
          if (seq !== searchSeqRef.current) return
          setError(reason instanceof Error ? reason.message : String(reason))
        })
        .finally(() => {
          if (seq === searchSeqRef.current) {
            setLoading(false)
          }
        })
    }, 110)

    return () => window.clearTimeout(timer)
  }, [deferredQuery, searchItems, setActive])

  useEffect(() => {
    if (activeIndex >= items.length) {
      setActive(Math.max(0, items.length - 1))
    }
  }, [activeIndex, items.length, setActive])

  useEffect(() => {
    itemRefs.current[activeIndex]?.scrollIntoView({
      behavior: "smooth",
      block: "nearest",
    })
  }, [activeIndex])

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
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return
      event.preventDefault()
      event.stopPropagation()
      void close()
    }

    window.addEventListener("keydown", onKeyDown, true)
    return () => window.removeEventListener("keydown", onKeyDown, true)
  }, [close])

  const handleKeyDown = (event: ReactKeyboardEvent<HTMLInputElement>) => {
    if (event.key === "Escape") {
      event.preventDefault()
      void close()
      return
    }

    if (event.key === "Tab") {
      event.preventDefault()
      if (!queryRef.current.trim()) {
        switchTab()
      }
      return
    }

    if (event.key === "ArrowDown") {
      event.preventDefault()
      moveActive(1)
      return
    }

    if (event.key === "ArrowUp") {
      event.preventDefault()
      moveActive(-1)
      return
    }

    if (event.key === "Enter") {
      event.preventDefault()
      void choose(itemsRef.current[activeIndexRef.current])
    }
  }

  const currentLabel = isSearching ? "搜索结果" : activeTab === "recent" ? "最近打开" : "最常打开"

  return (
    <main className="relative h-screen w-screen overflow-hidden border bg-background p-3 text-foreground shadow-2xl">
      <Card className="flex h-full min-h-0 flex-col overflow-hidden border bg-card/95 py-0 shadow-xl">
        <CardContent className="flex h-full min-h-0 flex-col gap-3 p-4">
          <section className="grid gap-3">
            <div className="flex items-center gap-3">
              <div className="flex size-10 shrink-0 items-center justify-center rounded-xl border bg-muted text-muted-foreground">
                <MagnifyingGlass className="size-5" weight="duotone" />
              </div>
              <Input
                ref={inputRef}
                value={query}
                autoFocus
                placeholder="搜索文件、目录、程序、快捷启动..."
                className="h-12 rounded-xl bg-background px-4 text-base font-medium shadow-none placeholder:text-muted-foreground"
                onChange={(event) => setQuery(event.target.value)}
                onKeyDown={handleKeyDown}
              />
            </div>

            <div className="flex flex-wrap items-center justify-between gap-2">
              <div className="flex items-center gap-2">
                {isSearching ? (
                  <Badge className="h-7 px-3">搜索结果</Badge>
                ) : (
                  <>
                    <Button
                      type="button"
                      variant={activeTab === "recent" ? "default" : "outline"}
                      size="sm"
                      className="rounded-full"
                      onClick={() => {
                        setActiveTab("recent")
                        setActive(0)
                        focusInput()
                      }}
                    >
                      <ClockCounterClockwise className="size-3.5" weight="duotone" />
                      最近打开
                    </Button>
                    <Button
                      type="button"
                      variant={activeTab === "frequent" ? "default" : "outline"}
                      size="sm"
                      className="rounded-full"
                      onClick={() => {
                        setActiveTab("frequent")
                        setActive(0)
                        focusInput()
                      }}
                    >
                      <ChartBar className="size-3.5" weight="duotone" />
                      最常打开
                    </Button>
                  </>
                )}
                <Badge variant="outline" className="h-7 px-3 text-muted-foreground">
                  Tab 切换
                </Badge>
              </div>
              <div className="flex items-center gap-2 text-xs text-muted-foreground">
                <span>{currentLabel}</span>
                <Badge variant="secondary">{loading ? "检索中" : `${items.length} 项`}</Badge>
              </div>
            </div>
          </section>

          <div className="min-h-0 flex-1 rounded-xl border bg-background/60 p-2">
            {!overlay.settings.search_enabled ? (
              <EmptyPanel title="搜索功能已禁用" description="到设置中心启用搜索后，Ctrl+Space 会重新唤起这个搜索框。" />
            ) : items.length > 0 ? (
              <ScrollArea className="h-full pr-2">
                <div className="grid gap-1.5">
                  {items.map((item, index) => (
                    <Button
                      key={`${item.kind}-${item.path}-${index}`}
                      ref={(node) => {
                        itemRefs.current[index] = node
                      }}
                      type="button"
                      variant="ghost"
                      className={cn(
                        "h-auto justify-start rounded-xl border border-transparent px-3 py-2.5 text-left transition-colors",
                        "hover:border-border hover:bg-muted hover:text-foreground",
                        activeIndex === index && "border-ring/50 bg-accent text-accent-foreground ring-1 ring-ring/30",
                      )}
                      onMouseEnter={() => setActive(index)}
                      onClick={() => void choose(item)}
                    >
                      <FileIcon
                        name={item.name}
                        extension={item.extension}
                        isDir={item.is_dir}
                        iconDataUrl={item.icon_data_url}
                        className="size-9 rounded-lg bg-muted text-muted-foreground"
                      />
                      <span className="min-w-0 flex-1">
                        <span className="flex min-w-0 items-center gap-2">
                          <span className="truncate text-sm font-medium">{item.name}</span>
                          <Badge className="h-5 shrink-0 text-[10px]" variant="outline">
                            {kindLabel(item)}
                          </Badge>
                        </span>
                        <span className="mt-1 block truncate text-xs text-muted-foreground">{truncate(item.path, 112)}</span>
                      </span>
                      <span className="shrink-0 text-xs tabular-nums text-muted-foreground">{String(index + 1).padStart(2, "0")}</span>
                    </Button>
                  ))}
                </div>
              </ScrollArea>
            ) : (
              <EmptyPanel
                title={isSearching ? "没有找到匹配项" : "还没有打开记录"}
                description={isSearching ? "换一个关键词，或在设置中心添加更多搜索路径。" : "打开文件或快捷启动后，这里会自动生成最近和常用列表。"}
              />
            )}
          </div>

          <footer className="flex items-center justify-between gap-3 text-xs text-muted-foreground">
            <span className="shrink-0">↑↓ 选择 · Enter 打开 · Esc 关闭</span>
            <span className="min-w-0 truncate text-right" title={overlay.paths.join("\n")}>
              路径：{overlay.paths.length > 0 ? truncate(overlay.paths.join("；"), 90) : "默认收纳目录"}
            </span>
          </footer>
        </CardContent>
      </Card>
      {error ? (
        <div className="absolute inset-x-5 bottom-5 flex items-center gap-2 rounded-xl border bg-destructive px-3 py-2 text-xs text-destructive-foreground shadow-lg">
          <Warning className="size-4 shrink-0" weight="fill" />
          <span className="truncate">{error}</span>
        </div>
      ) : null}
    </main>
  )
}

function EmptyPanel({ title, description }: { title: string; description: string }) {
  return (
    <div className="flex h-full min-h-52 flex-col items-center justify-center gap-2 rounded-xl border border-dashed bg-muted/30 p-8 text-center">
      <p className="font-heading text-base font-medium text-foreground">{title}</p>
      <p className="max-w-md text-sm leading-6 text-muted-foreground">{description}</p>
    </div>
  )
}

function kindLabel(item: SearchItem) {
  if (item.kind === "Launcher") return "快捷启动"
  if (item.kind === "Directory") return "目录"
  if (isShortcutOrApp(item)) return "快捷方式"
  return item.extension || "文件"
}

function isShortcutOrApp(item: SearchItem) {
  return ["lnk", "exe", "appref-ms", "url", "bat", "cmd", "ps1", "msi"].includes(item.extension.trim().toLowerCase())
}
