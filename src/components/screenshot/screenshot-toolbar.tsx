import { useLayoutEffect, useRef, useState } from "react"
import type { CSSProperties, ReactNode } from "react"
import type { Icon } from "@phosphor-icons/react"
import {
  ArrowArcLeft,
  ArrowUpRight,
  Check,
  CircleNotch,
  Circle,
  DownloadSimple,
  GridFour,
  PencilSimple,
  PushPin,
  Square,
  TextAa,
  TextT,
  X,
} from "@phosphor-icons/react"
import type { DrawTool } from "@/lib/screenshot/annotations"
import type { Rect } from "@/lib/screenshot/geometry"
import { cn } from "@/lib/utils"

export const SCREENSHOT_COLORS = ["#f5222d", "#fadb14", "#52c41a", "#1677ff", "#722ed1", "#8c8c8c", "#ffffff", "#000000"]
export const STROKE_LEVELS = [2, 4, 7]
export const TEXT_LEVELS = [16, 22, 30]
export const MOSAIC_LEVELS = [14, 26, 42]

export const TOOL_SHORTCUTS: Record<DrawTool, string> = {
  rect: "R",
  ellipse: "E",
  arrow: "A",
  pen: "P",
  mosaic: "M",
  text: "T",
}

const TOOLS: { tool: DrawTool; label: string; icon: Icon }[] = [
  { tool: "rect", label: "矩形", icon: Square },
  { tool: "ellipse", label: "椭圆", icon: Circle },
  { tool: "arrow", label: "箭头", icon: ArrowUpRight },
  { tool: "pen", label: "画笔", icon: PencilSimple },
  { tool: "mosaic", label: "马赛克", icon: GridFour },
  { tool: "text", label: "文字", icon: TextT },
]

const GAP = 8
const EDGE = 4

export interface ToolbarStyleState {
  color: string
  strokeLevel: number
  textLevel: number
  mosaicLevel: number
}

interface ScreenshotToolbarProps {
  /** Selection rect in CSS pixels. */
  anchor: Rect
  viewportWidth: number
  viewportHeight: number
  tool: DrawTool | null
  style: ToolbarStyleState
  canUndo: boolean
  busy: string | null
  onToolChange: (tool: DrawTool | null) => void
  onStyleChange: (style: Partial<ToolbarStyleState>) => void
  onUndo: () => void
  onOcr: () => void
  onPin: () => void
  onSave: () => void
  onCancel: () => void
  onDone: () => void
}

export function ScreenshotToolbar({
  anchor,
  viewportWidth,
  viewportHeight,
  tool,
  style,
  canUndo,
  busy,
  onToolChange,
  onStyleChange,
  onUndo,
  onOcr,
  onPin,
  onSave,
  onCancel,
  onDone,
}: ScreenshotToolbarProps) {
  const rootRef = useRef<HTMLDivElement | null>(null)
  const [layout, setLayout] = useState<{ left: number; top: number; above: boolean } | null>(null)

  useLayoutEffect(() => {
    const node = rootRef.current
    if (!node) return
    const width = node.offsetWidth
    const height = node.offsetHeight
    const bottom = anchor.y + anchor.height
    let top: number
    let above = false
    if (bottom + GAP + height <= viewportHeight - EDGE) {
      top = bottom + GAP
    } else if (anchor.y - GAP - height >= EDGE) {
      top = anchor.y - GAP - height
      above = true
    } else {
      top = Math.max(EDGE, bottom - GAP - height)
      above = true
    }
    const left = Math.min(Math.max(EDGE, anchor.x + anchor.width - width), Math.max(EDGE, viewportWidth - width - EDGE))
    setLayout((current) =>
      current && current.left === left && current.top === top && current.above === above ? current : { left, top, above },
    )
  }, [anchor.height, anchor.width, anchor.x, anchor.y, tool, viewportHeight, viewportWidth])

  const position: CSSProperties = layout
    ? { left: layout.left, top: layout.top, flexDirection: layout.above ? "column-reverse" : "column" }
    : { left: -9999, top: -9999 }

  return (
    <div
      ref={rootRef}
      className="absolute z-30 flex items-end gap-1.5 select-none"
      style={position}
      onPointerDown={(event) => event.stopPropagation()}
      onDoubleClick={(event) => event.stopPropagation()}
      onContextMenu={(event) => {
        event.preventDefault()
        event.stopPropagation()
      }}
    >
      <div className="flex items-center gap-0.5 rounded-lg border border-white/10 bg-neutral-900/95 p-1 text-neutral-100 shadow-xl">
        {TOOLS.map((item) => (
          <ToolButton
            key={item.tool}
            label={item.label}
            shortcut={TOOL_SHORTCUTS[item.tool]}
            active={tool === item.tool}
            disabled={Boolean(busy)}
            onClick={() => onToolChange(tool === item.tool ? null : item.tool)}
          >
            <item.icon className="size-[18px]" weight={tool === item.tool ? "bold" : "regular"} />
          </ToolButton>
        ))}
        <Divider />
        <ToolButton label="撤销" shortcut="Ctrl+Z" disabled={!canUndo || Boolean(busy)} onClick={onUndo}>
          <ArrowArcLeft className="size-[18px]" />
        </ToolButton>
        <Divider />
        <ToolButton label="识别文字" shortcut="" disabled={Boolean(busy)} onClick={onOcr}>
          <TextAa className="size-[18px]" />
        </ToolButton>
        <ToolButton label="钉在屏幕" shortcut="" disabled={Boolean(busy)} onClick={onPin}>
          {busy === "pin" ? <CircleNotch className="size-[18px] animate-spin" /> : <PushPin className="size-[18px]" />}
        </ToolButton>
        <ToolButton label="保存" shortcut="Ctrl+S" disabled={Boolean(busy)} onClick={onSave}>
          {busy === "save" ? <CircleNotch className="size-[18px] animate-spin" /> : <DownloadSimple className="size-[18px]" />}
        </ToolButton>
        <Divider />
        <ToolButton label="退出截图" shortcut="Esc" disabled={Boolean(busy)} onClick={onCancel}>
          <X className="size-[18px] text-red-400" weight="bold" />
        </ToolButton>
        <ToolButton label="完成" shortcut="Enter / 双击" disabled={Boolean(busy)} onClick={onDone}>
          {busy === "copy" ? (
            <CircleNotch className="size-[18px] animate-spin text-emerald-400" weight="bold" />
          ) : (
            <Check className="size-[18px] text-emerald-400" weight="bold" />
          )}
        </ToolButton>
      </div>
      {tool ? <StylePanel tool={tool} style={style} onStyleChange={onStyleChange} /> : null}
    </div>
  )
}

function StylePanel({
  tool,
  style,
  onStyleChange,
}: {
  tool: DrawTool
  style: ToolbarStyleState
  onStyleChange: (style: Partial<ToolbarStyleState>) => void
}) {
  const isText = tool === "text"
  const isMosaic = tool === "mosaic"
  const level = isText ? style.textLevel : isMosaic ? style.mosaicLevel : style.strokeLevel
  const setLevel = (next: number) => {
    if (isText) onStyleChange({ textLevel: next })
    else if (isMosaic) onStyleChange({ mosaicLevel: next })
    else onStyleChange({ strokeLevel: next })
  }

  return (
    <div className="flex items-center gap-2 rounded-lg border border-white/10 bg-neutral-900/95 px-2 py-1.5 text-neutral-100 shadow-xl">
      <div className="flex items-center gap-1">
        {[0, 1, 2].map((index) => (
          <button
            key={index}
            type="button"
            title={isText ? ["小号字", "中号字", "大号字"][index] : ["细", "中", "粗"][index]}
            className={cn(
              "grid size-7 place-items-center rounded-md transition-colors hover:bg-white/10",
              level === index && "bg-white/15",
            )}
            onClick={() => setLevel(index)}
          >
            {isText ? (
              <span className="font-semibold leading-none" style={{ fontSize: 11 + index * 3 }}>
                A
              </span>
            ) : (
              <span
                className={cn("rounded-full", level === index ? "bg-sky-400" : "bg-neutral-300")}
                style={{ width: 4 + index * 4, height: 4 + index * 4 }}
              />
            )}
          </button>
        ))}
      </div>
      {isMosaic ? null : (
        <>
          <span className="h-5 w-px bg-white/15" />
          <div className="flex items-center gap-1">
            {SCREENSHOT_COLORS.map((color) => (
              <button
                key={color}
                type="button"
                aria-label={`颜色 ${color}`}
                className={cn(
                  "size-5 rounded-sm border border-white/25 transition-transform hover:scale-110",
                  style.color === color && "ring-2 ring-sky-400 ring-offset-1 ring-offset-neutral-900",
                )}
                style={{ background: color }}
                onClick={() => onStyleChange({ color })}
              />
            ))}
          </div>
        </>
      )}
    </div>
  )
}

function Divider() {
  return <span className="mx-1 h-5 w-px bg-white/15" />
}

function ToolButton({
  label,
  shortcut,
  active,
  disabled,
  onClick,
  children,
}: {
  label: string
  shortcut: string
  active?: boolean
  disabled?: boolean
  onClick: () => void
  children: ReactNode
}) {
  return (
    <span className="group relative">
      <button
        type="button"
        aria-label={label}
        disabled={disabled}
        className={cn(
          "grid size-8 place-items-center rounded-md transition-colors hover:bg-white/10 disabled:pointer-events-none disabled:opacity-40",
          active && "bg-white/15 text-sky-300",
        )}
        onClick={onClick}
      >
        {children}
      </button>
      <span className="pointer-events-none absolute bottom-full left-1/2 mb-1.5 hidden -translate-x-1/2 whitespace-nowrap rounded bg-black/85 px-2 py-1 text-xs text-white shadow group-hover:block">
        {label}
        {shortcut ? <span className="ml-1.5 text-white/60">{shortcut}</span> : null}
      </span>
    </span>
  )
}
