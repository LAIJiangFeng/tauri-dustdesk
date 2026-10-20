import type { CSSProperties, ReactNode } from "react"
import type { DesktopEdgeHideController } from "@/hooks/use-desktop-edge-hide"
import { desktopEdgeHideSlideOutTransform, desktopEdgeHideTiming, isDesktopEdgeHorizontal } from "@/lib/desktop-edge-hide"
import { cn } from "@/lib/utils"
import type { DesktopEdge } from "@/types"

interface DesktopEdgeHideShellProps {
  controller: DesktopEdgeHideController
  /** 把手背景，使用卡片背景色，不透明度不低于 0.7。 */
  handleBackground: string
  /** 把手中间抓手的颜色。 */
  accentColor: string
  children: ReactNode
}

const edgeAnchors: Record<DesktopEdge, CSSProperties> = {
  top: { top: 0, left: 0 },
  bottom: { bottom: 0, left: 0 },
  left: { top: 0, left: 0 },
  right: { top: 0, right: 0 },
}

const edgeStripThickness = 6

const edgeStripAnchors: Record<DesktopEdge, CSSProperties> = {
  top: { top: 0, left: 0, right: 0, height: edgeStripThickness },
  bottom: { bottom: 0, left: 0, right: 0, height: edgeStripThickness },
  left: { left: 0, top: 0, bottom: 0, width: edgeStripThickness },
  right: { right: 0, top: 0, bottom: 0, width: edgeStripThickness },
}

export function DesktopEdgeHideShell({ controller, handleBackground, accentColor, children }: DesktopEdgeHideShellProps) {
  const { state, phase, slidOut, reducedMotion, handleProps } = controller
  const active = Boolean(state?.enabled) && phase !== "disabled"
  const edge: DesktopEdge = state?.edge ?? "top"
  // 未开启或展开态：内容随窗口铺满，与普通框一致。保持同一套 DOM 结构，避免切换阶段时重挂载子树丢失滚动位置。
  const fluid = !active || phase === "expanded"
  const collapsed = active && phase === "collapsed"

  let contentStyle: CSSProperties = { width: "100%", height: "100%" }
  if (!fluid && state) {
    const scale = state.scale_factor > 0 ? state.scale_factor : 1
    const duration = phase === "collapsing" ? desktopEdgeHideTiming.collapseMs : desktopEdgeHideTiming.expandMs
    const easing = phase === "collapsing" ? "ease-in" : "ease-out"
    contentStyle = {
      position: "absolute",
      ...edgeAnchors[edge],
      width: state.expanded.width / scale,
      height: state.expanded.height / scale,
      transform: slidOut ? desktopEdgeHideSlideOutTransform(edge) : "none",
      transition: reducedMotion || phase === "collapsed" ? "none" : `transform ${duration}ms ${easing}`,
      visibility: collapsed ? "hidden" : "visible",
      willChange: "transform",
    }
  }

  return (
    <div className={cn(fluid ? "h-screen w-screen" : "fixed inset-0", active && "overflow-hidden")}>
      <div style={contentStyle} aria-hidden={collapsed || undefined}>
        {children}
      </div>
      {collapsed ? <EdgeHideHandle edge={edge} background={handleBackground} accentColor={accentColor} handleProps={handleProps} /> : null}
    </div>
  )
}

function EdgeHideHandle({
  edge,
  background,
  accentColor,
  handleProps,
}: {
  edge: DesktopEdge
  background: string
  accentColor: string
  handleProps: DesktopEdgeHideController["handleProps"]
}) {
  const horizontal = isDesktopEdgeHorizontal(edge)
  // 命中区和把手都锚定在停靠边、固定 6px 厚度：展开时窗口可能先于重新渲染被放大，不能把把手拉成整块。
  const hitStyle = edgeStripAnchors[edge]
  // 两端内缩 8px，与卡片可见边框（页面 p-2）对齐。
  const barStyle: CSSProperties = horizontal ? { ...edgeStripAnchors[edge], left: 8, right: 8 } : { ...edgeStripAnchors[edge], top: 8, bottom: 8 }
  const gripStyle: CSSProperties = horizontal ? { width: 28, height: 2 } : { width: 2, height: 28 }

  return (
    <div className="absolute cursor-pointer" style={hitStyle} aria-label="展开桌面框" role="button" tabIndex={-1} {...handleProps}>
      <div className="absolute grid place-items-center rounded-[3px] ring-1 ring-inset ring-white/15" style={{ ...barStyle, backgroundColor: background }}>
        <span className="rounded-full opacity-90" style={{ ...gripStyle, backgroundColor: accentColor }} />
      </div>
    </div>
  )
}
