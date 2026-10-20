import { invoke } from "@tauri-apps/api/core"
import type { DesktopEdge, DesktopEdgeHideMode, DesktopEdgeHidePointer, DesktopEdgeHideState } from "@/types"

export const desktopEdgeHideChangedEvent = "dustdesk://desktop-edge-hide-changed"
export const desktopEdgeHideHoldAttribute = "data-edge-hide-hold"

export const desktopEdgeHideTiming = {
  dwellMs: 120,
  expandMs: 200,
  collapseMs: 160,
  leaveMs: 600,
  pollMs: 120,
} as const

export const desktopEdgeHideModeOptions: { mode: DesktopEdgeHideMode; label: string }[] = [
  { mode: "auto", label: "自动" },
  { mode: "top", label: "上" },
  { mode: "left", label: "左" },
  { mode: "right", label: "右" },
  { mode: "bottom", label: "下" },
]

export function getDesktopEdgeHide(label: string) {
  return invoke<DesktopEdgeHideState>("get_desktop_edge_hide", { label })
}

export function setDesktopEdgeHide(label: string, enabled: boolean, mode: DesktopEdgeHideMode) {
  return invoke<DesktopEdgeHideState>("set_desktop_edge_hide", { label, enabled, mode })
}

export function expandDesktopEdgeHide(label: string) {
  return invoke<DesktopEdgeHideState>("expand_desktop_edge_hide", { label })
}

export function collapseDesktopEdgeHide(label: string) {
  return invoke<DesktopEdgeHideState>("collapse_desktop_edge_hide", { label })
}

export function desktopEdgeHidePointer(label: string) {
  return invoke<DesktopEdgeHidePointer>("desktop_edge_hide_pointer", { label })
}

/** 内容滑到停靠边外侧时的 transform。 */
export function desktopEdgeHideSlideOutTransform(edge: DesktopEdge) {
  switch (edge) {
    case "top":
      return "translate3d(0, -100%, 0)"
    case "bottom":
      return "translate3d(0, 100%, 0)"
    case "left":
      return "translate3d(-100%, 0, 0)"
    case "right":
      return "translate3d(100%, 0, 0)"
  }
}

export function isDesktopEdgeHorizontal(edge: DesktopEdge) {
  return edge === "top" || edge === "bottom"
}

export function prefersReducedMotion() {
  try {
    return globalThis.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false
  } catch {
    return false
  }
}

/** 页面中存在弹出层、菜单或遮罩等占用标记，或者输入元素处于焦点时，不能收起。 */
export function hasDesktopEdgeHideDomHold() {
  if (document.querySelector(`[${desktopEdgeHideHoldAttribute}]`)) return true
  const active = document.activeElement
  if (!(active instanceof HTMLElement)) return false
  if (active.isContentEditable) return true
  if (active instanceof HTMLTextAreaElement || active instanceof HTMLSelectElement) return true
  if (active instanceof HTMLInputElement) {
    return !["button", "checkbox", "radio", "range", "color", "submit", "reset"].includes(active.type)
  }
  return false
}
