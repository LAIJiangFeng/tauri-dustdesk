import type { Icon } from "@phosphor-icons/react"
import { ChartDonut, ClipboardText, Database, Gauge, GridFour, Path, RocketLaunch } from "@phosphor-icons/react"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card"
import { cn, formatCount, truncate } from "@/lib/utils"
import { useDustDeskStore } from "@/stores/dustdesk-store"

interface StatItem {
  name: string
  value: number
  detail: string
  icon: Icon
  tone: string
}

export function HomePage() {
  const snapshot = useDustDeskStore((state) => state.snapshot)
  const openSpecial = useDustDeskStore((state) => state.openSpecial)
  const openPath = useDustDeskStore((state) => state.openPath)
  const stats: StatItem[] = [
    { name: "分类数量", value: snapshot.categories.length, detail: "DesktopCategories", icon: GridFour, tone: "bg-chart-1/15 text-chart-1" },
    { name: "桌面项目", value: snapshot.desktop_items.length, detail: "Desktop roots", icon: Database, tone: "bg-chart-2/15 text-chart-2" },
    { name: "快捷启动", value: snapshot.launchers.length, detail: "Launchers", icon: RocketLaunch, tone: "bg-chart-3/15 text-chart-3" },
    { name: "剪贴板历史", value: snapshot.clipboard.length, detail: "Clipboard", icon: ClipboardText, tone: "bg-chart-4/15 text-chart-4" },
  ]
  const total = stats.reduce((sum, item) => sum + item.value, 0)
  const maxValue = Math.max(...stats.map((item) => item.value), 1)
  const dominant = stats.reduce((current, item) => (item.value > current.value ? item : current), stats[0])
  const activeCategories = snapshot.categories.filter((item) => item.item_paths.length > 0).length
  const desktopFileCount = snapshot.desktop_items.filter((item) => !item.is_dir).length
  const desktopDirCount = snapshot.desktop_items.length - desktopFileCount
  const pathRows = [
    { name: "收纳目录", path: snapshot.organizer_root, target: "organizer" as const },
    { name: "快捷启动目录", path: snapshot.launchers_root, target: "launchers" as const },
    { name: "本地数据目录", path: snapshot.data_dir, target: "data" as const },
  ]

  return (
    <div className="h-full min-h-0 overflow-y-auto pr-1 [scrollbar-color:var(--border)_transparent] [scrollbar-width:thin]">
      <div className="grid min-w-0 gap-4">
      <Card className="min-h-0 bg-card/95">
        <CardHeader>
          <div>
            <CardTitle>统计概览</CardTitle>
          </div>
          <Badge variant="outline">{stats.length} 组</Badge>
        </CardHeader>
        <CardContent className="grid min-h-0 gap-4">
          <div className="grid gap-4 2xl:grid-cols-[0.95fr_1.05fr]">
            <Card className="relative min-h-64 overflow-hidden border-0 bg-muted/35 ring-1 ring-foreground/10">
              <CardContent className="relative z-10 grid h-full content-between gap-6 p-6">
                <div className="flex items-start justify-between gap-3">
                  <div>
                    <Badge variant="secondary" className="mb-4 gap-1">
                      <Gauge className="size-3" weight="duotone" />
                      工作台总览
                    </Badge>
                    <strong className="block font-heading text-6xl font-semibold tracking-tight tabular-nums">{formatCount(total)}</strong>
                    <p className="mt-2 max-w-56 text-sm leading-6 text-muted-foreground">
                      当前桌面、收纳、启动与剪贴板历史的聚合规模。
                    </p>
                  </div>
                  <div className={cn("flex size-12 items-center justify-center rounded-2xl", dominant.tone)}>
                    <dominant.icon className="size-6" weight="duotone" />
                  </div>
                </div>
                <div className="grid grid-cols-3 gap-2">
                  <MiniMetric label="已归档分类" value={formatCount(activeCategories)} />
                  <MiniMetric label="文件" value={formatCount(desktopFileCount)} />
                  <MiniMetric label="目录" value={formatCount(desktopDirCount)} />
                </div>
              </CardContent>
              <ChartDonut className="absolute -bottom-10 -right-8 size-48 text-foreground/[0.04]" weight="duotone" />
            </Card>

            <Card className="min-h-64 border-0 bg-muted/25 ring-1 ring-foreground/10">
              <CardContent className="grid h-full gap-5 p-6">
                <div className="flex items-center justify-between">
                  <div>
                    <h3 className="font-heading text-lg font-semibold">模块分布</h3>
                    <p className="text-sm text-muted-foreground">按当前快照实时计算</p>
                  </div>
                  <Badge variant="outline">Live</Badge>
                </div>
                <div className="grid gap-4">
                  {stats.map((item) => (
                    <DistributionRow key={item.name} item={item} maxValue={maxValue} total={total} />
                  ))}
                </div>
              </CardContent>
            </Card>
          </div>

          <div className="grid gap-3 md:grid-cols-2 2xl:grid-cols-4">
            {stats.map((item) => (
              <StatCard key={item.name} item={item} maxValue={maxValue} />
            ))}
          </div>
        </CardContent>
      </Card>

      <Card className="min-h-0 bg-card/95">
        <CardHeader>
          <div>
            <CardTitle>本地路径</CardTitle>
          </div>
          <Badge variant="outline">{pathRows.length} 个</Badge>
        </CardHeader>
        <CardContent className="grid gap-3 md:grid-cols-3">
          {pathRows.map((row) => (
            <Button
              key={row.name}
              variant="outline"
              className="h-auto min-w-0 justify-start gap-3 rounded-xl bg-muted/25 p-4 text-left hover:bg-muted/45"
              title={row.path}
              onClick={() => void openSpecial(row.target)}
              onDoubleClick={() => void openPath(row.path)}
            >
              <span className="grid size-10 shrink-0 place-items-center rounded-lg bg-background/70 ring-1 ring-foreground/10">
                <Path className="size-5 text-muted-foreground" weight="duotone" />
              </span>
              <span className="min-w-0 flex-1">
                <span className="block font-medium">{row.name}</span>
                <span className="mt-1 block truncate font-mono text-[11px] text-muted-foreground">{truncate(row.path || "等待读取", 58)}</span>
              </span>
            </Button>
          ))}
        </CardContent>
      </Card>
      </div>
    </div>
  )
}

function StatCard({ item, maxValue }: { item: StatItem; maxValue: number }) {
  const Icon = item.icon
  const height = Math.max(18, Math.round((item.value / maxValue) * 54))

  return (
    <Card className="border-0 bg-muted/20 ring-1 ring-foreground/10 transition-colors hover:bg-muted/35">
      <CardContent className="grid gap-4 p-4">
        <div className="flex items-start justify-between gap-3">
          <div className={cn("flex size-10 items-center justify-center rounded-xl", item.tone)}>
            <Icon className="size-5" weight="duotone" />
          </div>
          <Badge variant="outline" className="max-w-32 truncate">
            {item.detail}
          </Badge>
        </div>
        <div>
          <strong className="font-heading text-4xl font-semibold tracking-tight tabular-nums">{formatCount(item.value)}</strong>
          <p className="mt-1 text-sm text-muted-foreground">{item.name}</p>
        </div>
        <div className="flex h-14 items-end gap-1.5" aria-hidden="true">
          {[0.44, 0.68, 0.52, 0.82, 0.6, 1].map((scale, index) => (
            <span
              key={index}
              className={cn("w-full rounded-t-md bg-foreground/10", index === 5 && "bg-primary")}
              style={{ height: `${Math.max(8, Math.round(height * scale))}px` }}
            />
          ))}
        </div>
      </CardContent>
    </Card>
  )
}

function DistributionRow({ item, maxValue, total }: { item: StatItem; maxValue: number; total: number }) {
  const Icon = item.icon
  const width = Math.max(4, Math.round((item.value / maxValue) * 100))
  const percent = total > 0 ? Math.round((item.value / total) * 100) : 0

  return (
    <div className="grid gap-2">
      <div className="flex items-center justify-between gap-3 text-sm">
        <div className="flex min-w-0 items-center gap-2">
          <span className={cn("flex size-7 shrink-0 items-center justify-center rounded-lg", item.tone)}>
            <Icon className="size-4" weight="duotone" />
          </span>
          <span className="truncate font-medium">{item.name}</span>
        </div>
        <span className="shrink-0 font-mono text-xs text-muted-foreground">{percent}%</span>
      </div>
      <div className="h-2 overflow-hidden rounded-full bg-muted">
        <div className="h-full rounded-full bg-primary transition-[width] duration-300" style={{ width: `${width}%` }} />
      </div>
    </div>
  )
}

function MiniMetric({ label, value }: { label: string; value: string }) {
  return (
    <div className="rounded-xl bg-background/60 p-3 ring-1 ring-foreground/10">
      <p className="text-xs text-muted-foreground">{label}</p>
      <p className="mt-1 truncate font-heading text-sm font-semibold">{value}</p>
    </div>
  )
}
