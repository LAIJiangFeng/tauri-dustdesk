import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import {
  collapseDesktopEdgeHide,
  desktopEdgeHideChangedEvent,
  desktopEdgeHidePointer,
  desktopEdgeHideTiming,
  expandDesktopEdgeHide,
  getDesktopEdgeHide,
  hasDesktopEdgeHideDomHold,
  prefersReducedMotion,
  setDesktopEdgeHide,
} from "@/lib/desktop-edge-hide"
import { safeCurrentWebviewDragDropEvent, safeCurrentWindow, safeListen } from "@/lib/tauri-window"
import type { DesktopEdgeHideMode, DesktopEdgeHideState } from "@/types"

export type DesktopEdgeHidePhase = "disabled" | "collapsed" | "expanding" | "expanded" | "collapsing"

interface UseDesktopEdgeHideOptions {
  fallbackLabel: string
  /** 页面级占用：菜单打开、收纳/还原/合并等操作进行中。 */
  busy: boolean
  clickThrough: boolean
}

type DragHoldSource = "html5" | "dom" | "external"

function delay(ms: number) {
  return new Promise<void>((resolve) => {
    window.setTimeout(resolve, ms)
  })
}

function nextFrames(count: number) {
  return new Promise<void>((resolve) => {
    const step = (remaining: number) => {
      if (remaining <= 0) {
        resolve()
        return
      }
      window.requestAnimationFrame(() => step(remaining - 1))
    }
    step(count)
  })
}

export function useDesktopEdgeHide({ fallbackLabel, busy, clickThrough }: UseDesktopEdgeHideOptions) {
  const label = useMemo(() => safeCurrentWindow()?.label || fallbackLabel, [fallbackLabel])
  const [state, setState] = useState<DesktopEdgeHideState | null>(null)
  const [phase, setPhase] = useState<DesktopEdgeHidePhase>("disabled")
  const [slidOut, setSlidOut] = useState(false)
  const [settingPending, setSettingPending] = useState(false)
  const [reducedMotion, setReducedMotion] = useState(prefersReducedMotion)

  const phaseRef = useRef<DesktopEdgeHidePhase>("disabled")
  const busyRef = useRef(busy)
  const clickThroughRef = useRef(clickThrough)
  const operationRef = useRef(false)
  const staleRef = useRef(false)
  const cancelCollapseRef = useRef(false)
  const dwellTimerRef = useRef<number | undefined>(undefined)
  const dragHoldsRef = useRef(new Set<DragHoldSource>())

  busyRef.current = busy
  clickThroughRef.current = clickThrough

  const commitPhase = useCallback((next: DesktopEdgeHidePhase) => {
    phaseRef.current = next
    setPhase(next)
  }, [])

  const applyStable = useCallback(
    (next: DesktopEdgeHideState) => {
      setState(next)
      if (!next.enabled) {
        commitPhase("disabled")
        setSlidOut(false)
      } else if (next.collapsed) {
        commitPhase("collapsed")
        setSlidOut(true)
      } else {
        commitPhase("expanded")
        setSlidOut(false)
      }
    },
    [commitPhase],
  )

  const refresh = useCallback(async () => {
    try {
      const next = await getDesktopEdgeHide(label)
      if (operationRef.current) {
        staleRef.current = true
        return
      }
      applyStable(next)
    } catch {
      // 窗口尚未就绪或在浏览器预览中，保持当前状态。
    }
  }, [applyStable, label])

  // 同一窗口同时只允许一个几何命令；命令期间到达的事件在结束后补一次刷新。
  const runOperation = useCallback(
    async (operation: () => Promise<void>) => {
      if (operationRef.current) return false
      operationRef.current = true
      try {
        await operation()
      } finally {
        operationRef.current = false
        if (staleRef.current) {
          staleRef.current = false
          void refresh()
        }
      }
      return true
    },
    [refresh],
  )

  const isHeld = useCallback(() => busyRef.current || dragHoldsRef.current.size > 0 || hasDesktopEdgeHideDomHold(), [])

  const slideBack = useCallback(
    async (animate: boolean) => {
      commitPhase("expanding")
      setSlidOut(false)
      if (animate) await delay(desktopEdgeHideTiming.expandMs)
      commitPhase("expanded")
    },
    [commitPhase],
  )

  const expand = useCallback(async () => {
    if (phaseRef.current === "collapsing") {
      cancelCollapseRef.current = true
      return
    }
    if (phaseRef.current !== "collapsed" || clickThroughRef.current) return
    await runOperation(async () => {
      let next: DesktopEdgeHideState
      try {
        next = await expandDesktopEdgeHide(label)
      } catch {
        commitPhase("collapsed")
        setSlidOut(true)
        return
      }
      if (!next.enabled || next.collapsed) {
        applyStable(next)
        return
      }
      setState(next)
      if (prefersReducedMotion()) {
        commitPhase("expanded")
        setSlidOut(false)
        return
      }
      setSlidOut(true)
      commitPhase("expanding")
      await nextFrames(2)
      setSlidOut(false)
      await delay(desktopEdgeHideTiming.expandMs)
      commitPhase("expanded")
    })
  }, [applyStable, commitPhase, label, runOperation])

  const collapse = useCallback(
    async (immediate = false) => {
      if (phaseRef.current !== "expanded") return
      await runOperation(async () => {
        cancelCollapseRef.current = false
        const animate = !immediate && !prefersReducedMotion()
        commitPhase("collapsing")
        if (animate) {
          setSlidOut(false)
          await nextFrames(2)
          setSlidOut(true)
          await delay(desktopEdgeHideTiming.collapseMs)
          if (!cancelCollapseRef.current) {
            const pointer = await desktopEdgeHidePointer(label).catch(() => null)
            if (!pointer || pointer.inside || pointer.primary_button_down || isHeld()) {
              cancelCollapseRef.current = true
            }
          }
          if (cancelCollapseRef.current) {
            await slideBack(true)
            return
          }
        } else {
          setSlidOut(true)
        }

        let next: DesktopEdgeHideState
        try {
          next = await collapseDesktopEdgeHide(label)
        } catch {
          await slideBack(animate)
          return
        }
        if (!next.enabled || next.collapsed) {
          applyStable(next)
          return
        }
        // 后端拒绝收起（例如主键仍按下），滑回展开态。
        setState(next)
        await slideBack(animate)
      })
    },
    [applyStable, commitPhase, isHeld, label, runOperation, slideBack],
  )

  const setEdgeHide = useCallback(
    async (enabled: boolean, mode: DesktopEdgeHideMode) => {
      setSettingPending(true)
      try {
        await runOperation(async () => {
          const next = await setDesktopEdgeHide(label, enabled, mode)
          applyStable(next)
        })
      } finally {
        setSettingPending(false)
      }
    },
    [applyStable, label, runOperation],
  )

  // 初始状态与后端广播。
  useEffect(() => {
    let cancelled = false
    let unlisten: (() => void) | undefined

    void safeListen<DesktopEdgeHideState>(desktopEdgeHideChangedEvent, (event) => {
      if (cancelled || event.payload.label !== label) return
      if (operationRef.current) {
        staleRef.current = true
        return
      }
      applyStable(event.payload)
    }).then((value) => {
      if (cancelled) {
        value?.()
        return
      }
      unlisten = value
      void refresh()
    })

    return () => {
      cancelled = true
      unlisten?.()
    }
  }, [applyStable, label, refresh])

  useEffect(() => {
    let media: MediaQueryList | undefined
    try {
      media = globalThis.matchMedia?.("(prefers-reduced-motion: reduce)")
    } catch {
      return
    }
    if (!media) return
    const sync = () => setReducedMotion(media.matches)
    media.addEventListener("change", sync)
    return () => media.removeEventListener("change", sync)
  }, [])

  const enabled = state?.enabled ?? false

  // 拖拽类占用；拖入窄条时立即展开。
  useEffect(() => {
    if (!enabled) {
      dragHoldsRef.current.clear()
      return
    }
    const holds = dragHoldsRef.current
    const onDragStart = () => holds.add("html5")
    const onDragEnd = () => {
      holds.delete("html5")
      holds.delete("dom")
    }
    const onDragEnter = () => {
      holds.add("dom")
      void expand()
    }
    const onDragLeave = (event: DragEvent) => {
      if (event.relatedTarget === null) holds.delete("dom")
    }
    const onDrop = () => holds.delete("dom")

    document.addEventListener("dragstart", onDragStart)
    document.addEventListener("dragend", onDragEnd)
    document.addEventListener("dragenter", onDragEnter)
    document.addEventListener("dragleave", onDragLeave)
    document.addEventListener("drop", onDrop)

    let cancelled = false
    let unlistenExternal: (() => void) | undefined
    void safeCurrentWebviewDragDropEvent((event) => {
      const type = event.payload.type
      if (type === "enter" || type === "over") {
        holds.add("external")
        void expand()
      } else {
        holds.delete("external")
      }
    }).then((value) => {
      if (cancelled) {
        value?.()
        return
      }
      unlistenExternal = value
    })

    return () => {
      cancelled = true
      document.removeEventListener("dragstart", onDragStart)
      document.removeEventListener("dragend", onDragEnd)
      document.removeEventListener("dragenter", onDragEnter)
      document.removeEventListener("dragleave", onDragLeave)
      document.removeEventListener("drop", onDrop)
      unlistenExternal?.()
      holds.clear()
    }
  }, [enabled, expand])

  // 收起动画中鼠标回到框内：取消收起并滑回。
  useEffect(() => {
    if (phase !== "collapsing") return
    const cancel = () => {
      cancelCollapseRef.current = true
    }
    document.addEventListener("pointermove", cancel)
    document.addEventListener("pointerdown", cancel)
    return () => {
      document.removeEventListener("pointermove", cancel)
      document.removeEventListener("pointerdown", cancel)
    }
  }, [phase])

  // 离开检测：只在展开态用光标轮询，不依赖 DOM mouseleave（无边框窗口边缘的缩放热区会误报离开）。
  useEffect(() => {
    if (phase !== "expanded" || clickThrough) return
    let cancelled = false
    let timer: number | undefined
    let outsideSince: number | null = null

    const tick = async () => {
      const pointer = await desktopEdgeHidePointer(label).catch(() => null)
      if (cancelled) return
      if (pointer && !pointer.primary_button_down) {
        // 主键已松开时拖拽一定已经结束，清掉可能遗漏 leave 事件的拖拽占用。
        dragHoldsRef.current.clear()
      }
      const held = !pointer || pointer.inside || pointer.primary_button_down || operationRef.current || isHeld()
      if (held) {
        outsideSince = null
      } else {
        const now = Date.now()
        outsideSince ??= now
        if (now - outsideSince >= desktopEdgeHideTiming.leaveMs) {
          void collapse()
          return
        }
      }
      timer = window.setTimeout(() => void tick(), desktopEdgeHideTiming.pollMs)
    }

    timer = window.setTimeout(() => void tick(), desktopEdgeHideTiming.pollMs)
    return () => {
      cancelled = true
      window.clearTimeout(timer)
    }
  }, [clickThrough, collapse, isHeld, label, phase])

  // 鼠标穿透时贴边隐藏的框保持收起。
  useEffect(() => {
    if (clickThrough && phase === "expanded") {
      void collapse(true)
    }
  }, [clickThrough, collapse, phase])

  const clearDwell = useCallback(() => {
    if (dwellTimerRef.current !== undefined) {
      window.clearTimeout(dwellTimerRef.current)
      dwellTimerRef.current = undefined
    }
  }, [])

  useEffect(() => clearDwell, [clearDwell])

  const handleProps = useMemo(
    () => ({
      onPointerEnter: () => {
        if (phaseRef.current !== "collapsed" || dwellTimerRef.current !== undefined) return
        dwellTimerRef.current = window.setTimeout(() => {
          dwellTimerRef.current = undefined
          void expand()
        }, desktopEdgeHideTiming.dwellMs)
      },
      onPointerMove: () => {
        if (phaseRef.current !== "collapsed" || dwellTimerRef.current !== undefined) return
        dwellTimerRef.current = window.setTimeout(() => {
          dwellTimerRef.current = undefined
          void expand()
        }, desktopEdgeHideTiming.dwellMs)
      },
      onPointerLeave: clearDwell,
      onPointerDown: () => {
        clearDwell()
        void expand()
      },
    }),
    [clearDwell, expand],
  )

  return {
    label,
    state,
    phase,
    slidOut,
    reducedMotion,
    settingPending,
    setEdgeHide,
    handleProps,
  }
}

export type DesktopEdgeHideController = ReturnType<typeof useDesktopEdgeHide>
