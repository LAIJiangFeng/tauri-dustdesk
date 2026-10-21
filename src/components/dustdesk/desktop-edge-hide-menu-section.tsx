import { ArrowLineUp, ArrowsInLineHorizontal } from "@phosphor-icons/react"
import { SettingsMenuSection } from "@/components/dustdesk/settings-menu-section"
import { desktopEdgeHideModeOptions } from "@/lib/desktop-edge-hide"
import { cn } from "@/lib/utils"
import type { DesktopEdgeHideMode, DesktopEdgeHideState } from "@/types"

interface DesktopEdgeHideMenuSectionProps {
  state: DesktopEdgeHideState | null
  pending: boolean
  onChange: (enabled: boolean, mode: DesktopEdgeHideMode) => void
}

export function DesktopEdgeHideMenuSection({ state, pending, onChange }: DesktopEdgeHideMenuSectionProps) {
  const enabled = state?.enabled ?? false
  const mode = state?.mode ?? "auto"
  const Icon = enabled ? ArrowsInLineHorizontal : ArrowLineUp

  return (
    <SettingsMenuSection title="贴边隐藏">
      <button
        type="button"
        disabled={pending || !state}
        aria-pressed={enabled}
        className="flex w-full items-center gap-1.5 whitespace-nowrap rounded-lg px-2 py-1.5 text-left text-[11px] font-semibold text-white/80 transition hover:bg-white/10 hover:text-white active:translate-y-px disabled:cursor-wait disabled:opacity-55"
        onClick={() => onChange(!enabled, mode)}
      >
        <Icon className="size-3.5 shrink-0" weight="duotone" />
        <span className="truncate">{enabled ? "关闭贴边隐藏" : "开启贴边隐藏"}</span>
      </button>
      {enabled ? (
        <div className="flex items-center justify-between gap-3 rounded-lg px-2 py-1 text-[11px] font-semibold text-white/80">
          <span className="shrink-0">停靠边</span>
          <div className="flex rounded-lg border border-white/10 bg-white/5 p-0.5" role="group" aria-label="停靠边">
            {desktopEdgeHideModeOptions.map((option) => (
              <button
                key={option.mode}
                type="button"
                disabled={pending}
                className={cn(
                  "rounded-md px-1.5 py-1 transition disabled:cursor-wait",
                  mode === option.mode ? "bg-white/15 text-white" : "text-white/45 hover:text-white/80",
                )}
                aria-pressed={mode === option.mode}
                onClick={() => {
                  if (option.mode !== mode) onChange(true, option.mode)
                }}
              >
                {option.label}
              </button>
            ))}
          </div>
        </div>
      ) : null}
      <p className="px-2 pb-1 text-[10px] leading-4 text-white/45">鼠标移到屏幕边缘的窄条即可展开。设置会同步到所有显示器上的这个框。</p>
    </SettingsMenuSection>
  )
}
