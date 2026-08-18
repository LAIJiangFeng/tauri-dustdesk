import { useEffect, useState } from "react"
import { ArrowCounterClockwise, Eye, EyeSlash } from "@phosphor-icons/react"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Card, CardAction, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card"
import {
  defaultDesktopWidgetSettings,
  desktopWidgetSettingsChangedEvent,
  normalizeDesktopWidgetColor,
  readDesktopWidgetSettings,
  writeDesktopWidgetSettings,
  type DesktopWidgetSettings,
} from "@/lib/desktop-widget-settings"

export function DesktopFrameControlPanel() {
  const [settings, setSettings] = useState<DesktopWidgetSettings>(readDesktopWidgetSettings)

  useEffect(() => {
    const syncSettings = () => setSettings(readDesktopWidgetSettings())
    globalThis.addEventListener(desktopWidgetSettingsChangedEvent, syncSettings)
    globalThis.addEventListener("storage", syncSettings)
    return () => {
      globalThis.removeEventListener(desktopWidgetSettingsChangedEvent, syncSettings)
      globalThis.removeEventListener("storage", syncSettings)
    }
  }, [])

  const updateSettings = (patch: Partial<DesktopWidgetSettings>) => {
    const next = { ...settings, ...patch }
    setSettings(next)
    writeDesktopWidgetSettings(next)
  }

  const resetSettings = () => {
    const next = { ...defaultDesktopWidgetSettings }
    setSettings(next)
    writeDesktopWidgetSettings(next)
  }

  return (
    <Card className="mb-3">
      <CardHeader className="border-b">
        <CardTitle>桌面框外观</CardTitle>
        <CardDescription>颜色、透明度与项目尺寸会同步应用到全部收纳桌面框和快捷启动框。</CardDescription>
        <CardAction>
          <Badge variant="outline">全局样式</Badge>
        </CardAction>
      </CardHeader>
      <CardContent className="grid gap-3">
        <div className="grid gap-3 md:grid-cols-2 xl:grid-cols-4">
          <label className="grid gap-2 rounded-lg border bg-muted/25 p-3 text-sm">
            <span className="flex items-center justify-between gap-3 font-medium">
              背景颜色
              <span className="font-mono text-xs text-muted-foreground">{settings.backgroundColor.toUpperCase()}</span>
            </span>
            <input
              type="color"
              className="h-9 w-full cursor-pointer rounded-md border bg-transparent p-1"
              value={settings.backgroundColor}
              aria-label="选择桌面框背景颜色"
              onInput={(event) =>
                updateSettings({
                  backgroundColor: normalizeDesktopWidgetColor(event.currentTarget.value),
                })
              }
            />
          </label>
          <RangeControl
            label="卡片透明度"
            valueLabel={`${Math.round(settings.opacity * 100)}%`}
            min={0.25}
            max={0.85}
            step={0.05}
            value={settings.opacity}
            onChange={(opacity) => updateSettings({ opacity })}
          />
          <RangeControl label="项目大小" valueLabel={`${settings.iconSize}px`} min={34} max={74} step={4} value={settings.iconSize} onChange={(iconSize) => updateSettings({ iconSize })} />
          <div className="grid content-between gap-2 rounded-lg border bg-muted/25 p-3">
            <div>
              <p className="text-sm font-medium">网格项目名称</p>
              <p className="mt-1 text-xs text-muted-foreground">列表排版始终显示名称。</p>
            </div>
            <Button type="button" size="sm" variant="outline" onClick={() => updateSettings({ showNames: !settings.showNames })}>
              {settings.showNames ? <EyeSlash className="size-3.5" weight="duotone" /> : <Eye className="size-3.5" weight="duotone" />}
              {settings.showNames ? "隐藏名称" : "显示名称"}
            </Button>
          </div>
        </div>
        <Button type="button" size="sm" variant="ghost" className="w-fit" onClick={resetSettings}>
          <ArrowCounterClockwise className="size-3.5" weight="duotone" />
          恢复默认外观
        </Button>
      </CardContent>
    </Card>
  )
}

function RangeControl({
  label,
  valueLabel,
  min,
  max,
  step,
  value,
  onChange,
}: {
  label: string
  valueLabel: string
  min: number
  max: number
  step: number
  value: number
  onChange: (value: number) => void
}) {
  return (
    <label className="grid gap-2 rounded-lg border bg-muted/25 p-3 text-sm">
      <span className="flex items-center justify-between gap-3 font-medium">
        {label}
        <span className="text-xs text-muted-foreground">{valueLabel}</span>
      </span>
      <input className="w-full accent-primary" type="range" min={min} max={max} step={step} value={value} onChange={(event) => onChange(Number(event.target.value))} />
    </label>
  )
}
