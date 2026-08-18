import { useEffect, useState } from "react"
import { ListBullets, SquaresFour } from "@phosphor-icons/react"
import { Button } from "@/components/ui/button"
import { desktopWidgetViewModesChangedEvent, readDesktopWidgetViewModes, writeDesktopWidgetViewMode, type DesktopWidgetViewMode } from "@/lib/desktop-widget-settings"

export function DesktopWidgetViewModeControl({ scope, label }: { scope: string; label: string }) {
  const [viewModes, setViewModes] = useState<Record<string, DesktopWidgetViewMode>>(readDesktopWidgetViewModes)
  const viewMode = viewModes[scope] ?? "grid"

  useEffect(() => {
    const syncViewModes = () => setViewModes(readDesktopWidgetViewModes())
    globalThis.addEventListener(desktopWidgetViewModesChangedEvent, syncViewModes)
    globalThis.addEventListener("storage", syncViewModes)
    return () => {
      globalThis.removeEventListener(desktopWidgetViewModesChangedEvent, syncViewModes)
      globalThis.removeEventListener("storage", syncViewModes)
    }
  }, [])

  const updateViewMode = (nextViewMode: DesktopWidgetViewMode) => {
    setViewModes((current) => ({ ...current, [scope]: nextViewMode }))
    writeDesktopWidgetViewMode(scope, nextViewMode)
  }

  return (
    <div className="flex items-center gap-2">
      <span className="text-xs font-medium text-muted-foreground">桌面框排版</span>
      <div className="flex rounded-lg border bg-background p-0.5" role="group" aria-label={label}>
        <Button type="button" size="sm" variant={viewMode === "grid" ? "secondary" : "ghost"} aria-pressed={viewMode === "grid"} onClick={() => updateViewMode("grid")}>
          <SquaresFour className="size-3.5" weight="duotone" />
          网格
        </Button>
        <Button type="button" size="sm" variant={viewMode === "list" ? "secondary" : "ghost"} aria-pressed={viewMode === "list"} onClick={() => updateViewMode("list")}>
          <ListBullets className="size-3.5" weight="duotone" />
          列表
        </Button>
      </div>
    </div>
  )
}
