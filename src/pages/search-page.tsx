import { useEffect, useMemo, useState } from "react"
import { ChartBar, MagnifyingGlass, Sparkle, Warning, type Icon } from "@phosphor-icons/react"
import { FileIcon } from "@/components/dustdesk/file-icon"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card"
import { Input } from "@/components/ui/input"
import { ScrollArea } from "@/components/ui/scroll-area"
import { formatCount, truncate } from "@/lib/utils"
import { useDustDeskStore } from "@/stores/dustdesk-store"
import type { SearchItem, SearchOverlayData } from "@/types"

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

export function SearchPage() {
  const loadSearchOverlay = useDustDeskStore((state) => state.loadSearchOverlay)
  const openSearchItem = useDustDeskStore((state) => state.openSearchItem)
  const [overlay, setOverlay] = useState<SearchOverlayData>(emptyOverlay)
  const [filter, setFilter] = useState("")
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState("")

  useEffect(() => {
    let cancelled = false
    setLoading(true)
    setError("")

    void loadSearchOverlay()
      .then((data) => {
        if (!cancelled) setOverlay(data)
      })
      .catch((reason) => {
        if (!cancelled) setError(reason instanceof Error ? reason.message : String(reason))
      })
      .finally(() => {
        if (!cancelled) setLoading(false)
      })

    return () => {
      cancelled = true
    }
  }, [loadSearchOverlay])

  const historyItems = useMemo(() => mergeHistoryItems(overlay), [overlay])
  const visibleHistory = useMemo(() => filterItems(historyItems, filter), [filter, historyItems])
  const visibleRecent = useMemo(() => filterItems(overlay.recent, filter), [filter, overlay.recent])
  const visibleFrequent = useMemo(() => filterItems(overlay.frequent, filter), [filter, overlay.frequent])
  const totalCount = historyItems.length
  const openCount = historyItems.reduce((sum, item) => sum + item.open_count, 0)
  const shortcutCount = historyItems.filter((item) => item.kind === "Launcher" || isShortcutOrApp(item)).length

  const openItem = async (item: SearchItem) => {
    setError("")
    try {
      await openSearchItem(item)
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason))
    }
  }

  return (
    <div className="grid h-full min-h-0 gap-4">
      <Card className="min-h-0 bg-card/95">
        <CardHeader>
          <div>
            <CardTitle>搜索历史</CardTitle>
          </div>
          <Badge variant={overlay.settings.search_enabled ? "outline" : "destructive"}>
            {overlay.settings.search_enabled ? "已启用" : "未启用"}
          </Badge>
        </CardHeader>
        <CardContent className="grid min-h-0 gap-4">
          <div className="grid gap-3 xl:grid-cols-[minmax(0,1fr)_minmax(0,1fr)_minmax(0,1fr)]">
            <MetricCard icon={MagnifyingGlass} label="历史项目" value={totalCount} hint="最近与常用去重后统计" />
            <MetricCard icon={ChartBar} label="累计打开" value={openCount} hint="来自搜索打开记录" />
            <MetricCard icon={Sparkle} label="快捷入口" value={shortcutCount} hint="快捷启动与快捷方式优先" />
          </div>

          <div className="flex flex-col gap-3 rounded-xl border bg-background/60 p-3 md:flex-row md:items-center">
            <div className="flex size-10 shrink-0 items-center justify-center rounded-xl bg-muted text-muted-foreground">
              <MagnifyingGlass className="size-5" weight="duotone" />
            </div>
            <Input
              value={filter}
              placeholder="筛选历史、最近打开、最常打开..."
              className="h-11 rounded-xl bg-card px-4 text-base font-medium md:text-sm"
              onChange={(event) => setFilter(event.target.value)}
            />
            <Badge variant="outline" className="h-7 shrink-0 px-3">
              {loading ? "读取中" : `${formatCount(totalCount)} 项`}
            </Badge>
          </div>

          {error ? (
            <div className="flex items-center gap-2 rounded-xl border border-destructive/30 bg-destructive/10 px-3 py-2 text-sm text-destructive">
              <Warning className="size-4 shrink-0" weight="fill" />
              <span className="min-w-0 truncate">{error}</span>
            </div>
          ) : null}

          <div className="grid min-h-0 gap-4 xl:grid-cols-[1.1fr_0.95fr_0.95fr]">
            <HistoryPanel title="搜索历史" badge={`${visibleHistory.length} 项`} items={visibleHistory} empty="还没有搜索打开记录" onOpen={openItem} />
            <HistoryPanel title="最近打开" badge={`${visibleRecent.length} 项`} items={visibleRecent} empty="还没有最近打开记录" onOpen={openItem} />
            <HistoryPanel title="最常打开" badge={`${visibleFrequent.length} 项`} items={visibleFrequent} empty="还没有常用记录" onOpen={openItem} />
          </div>
        </CardContent>
      </Card>
    </div>
  )
}

function MetricCard({
  icon: Icon,
  label,
  value,
  hint,
}: {
  icon: Icon
  label: string
  value: number
  hint: string
}) {
  return (
    <Card className="border-0 bg-muted/25 ring-1 ring-foreground/10">
      <CardContent className="flex items-center gap-4 p-4">
        <div className="flex size-11 shrink-0 items-center justify-center rounded-xl bg-primary/10 text-primary">
          <Icon className="size-5" weight="duotone" />
        </div>
        <div className="min-w-0">
          <div className="font-heading text-3xl font-semibold tabular-nums">{formatCount(value)}</div>
          <div className="text-sm font-medium">{label}</div>
          <div className="truncate text-xs text-muted-foreground">{hint}</div>
        </div>
      </CardContent>
    </Card>
  )
}

function HistoryPanel({
  title,
  badge,
  items,
  empty,
  onOpen,
}: {
  title: string
  badge: string
  items: SearchItem[]
  empty: string
  onOpen: (item: SearchItem) => void | Promise<void>
}) {
  return (
    <Card className="min-h-[320px] border bg-background/50">
      <CardHeader>
        <div>
          <CardTitle>{title}</CardTitle>
        </div>
        <Badge variant="outline">{badge}</Badge>
      </CardHeader>
      <CardContent className="min-h-0">
        {items.length > 0 ? (
          <ScrollArea className="h-[42vh] min-h-64 pr-2">
            <div className="grid gap-2">
              {items.map((item, index) => (
                <SearchHistoryRow key={`${item.kind}-${item.path}-${index}`} item={item} index={index} onOpen={onOpen} />
              ))}
            </div>
          </ScrollArea>
        ) : (
          <div className="flex h-64 flex-col items-center justify-center gap-2 rounded-xl border border-dashed bg-muted/25 text-center text-sm text-muted-foreground">
            <MagnifyingGlass className="size-8" weight="duotone" />
            <span>{empty}</span>
          </div>
        )}
      </CardContent>
    </Card>
  )
}

function SearchHistoryRow({
  item,
  index,
  onOpen,
}: {
  item: SearchItem
  index: number
  onOpen: (item: SearchItem) => void | Promise<void>
}) {
  return (
    <Button
      type="button"
      variant="ghost"
      className="h-auto min-w-0 justify-start rounded-xl border border-transparent bg-card/60 px-3 py-3 text-left transition-colors hover:border-border hover:bg-muted/50"
      onClick={() => void onOpen(item)}
    >
      <FileIcon
        name={item.name}
        extension={item.extension}
        isDir={item.is_dir}
        iconDataUrl={item.icon_data_url}
        className="size-10 rounded-xl bg-muted text-muted-foreground"
      />
      <span className="min-w-0 flex-1">
        <span className="flex min-w-0 items-center gap-2">
          <span className="truncate font-medium">{item.name}</span>
          <Badge variant="outline" className="h-5 shrink-0 text-[10px]">
            {kindLabel(item)}
          </Badge>
        </span>
        <span className="mt-1 block truncate text-xs text-muted-foreground">{truncate(item.path, 90)}</span>
        <span className="mt-2 flex items-center gap-2 text-[11px] text-muted-foreground">
          <span>打开 {formatCount(item.open_count)} 次</span>
          {item.last_opened_at ? <span className="truncate">最近 {item.last_opened_at}</span> : null}
        </span>
      </span>
      <span className="shrink-0 text-xs tabular-nums text-muted-foreground">{String(index + 1).padStart(2, "0")}</span>
    </Button>
  )
}

function mergeHistoryItems(overlay: SearchOverlayData) {
  const map = new Map<string, SearchItem>()
  for (const item of [...overlay.recent, ...overlay.frequent]) {
    const key = `${item.kind}:${item.path.toLowerCase()}`
    const existing = map.get(key)
    if (!existing || item.open_count > existing.open_count || item.last_opened_at > existing.last_opened_at) {
      map.set(key, item)
    }
  }
  return [...map.values()].sort((left, right) => right.last_opened_at.localeCompare(left.last_opened_at) || right.open_count - left.open_count)
}

function filterItems(items: SearchItem[], filter: string) {
  const value = filter.trim().toLowerCase()
  if (!value) return items
  return items.filter((item) => `${item.name} ${item.path} ${item.extension}`.toLowerCase().includes(value))
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
