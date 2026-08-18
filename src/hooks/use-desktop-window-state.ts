import { useEffect, useRef, useState } from "react"
import { invoke } from "@tauri-apps/api/core"
import { safeListen } from "@/lib/tauri-window"
import type { DesktopWindowState } from "@/types"

const defaultDesktopWindowState: DesktopWindowState = {
  locked: false,
  click_through: false,
}

export function useDesktopWindowState() {
  const [state, setState] = useState(defaultDesktopWindowState)
  const [pending, setPending] = useState(false)
  const eventRevisionRef = useRef(0)

  useEffect(() => {
    let cancelled = false
    let unlisten: (() => void) | undefined

    async function initialize() {
      const listener = await safeListen<DesktopWindowState>("dustdesk://desktop-window-state-changed", (event) => {
        if (cancelled) return
        eventRevisionRef.current += 1
        setState(event.payload)
      })
      if (cancelled) {
        listener?.()
        return
      }
      unlisten = listener

      const revisionAtRequest = eventRevisionRef.current
      try {
        const value = await invoke<DesktopWindowState>("desktop_window_state")
        if (!cancelled && eventRevisionRef.current === revisionAtRequest) {
          setState(value)
        }
      } catch {
        return
      }
    }

    void initialize()

    return () => {
      cancelled = true
      unlisten?.()
    }
  }, [])

  async function setLocked(locked: boolean) {
    setPending(true)
    const revisionAtRequest = eventRevisionRef.current
    try {
      const value = await invoke<DesktopWindowState>("set_desktop_windows_locked", { locked })
      if (eventRevisionRef.current === revisionAtRequest) {
        setState(value)
      }
      return value
    } finally {
      setPending(false)
    }
  }

  async function setClickThrough(clickThrough: boolean) {
    setPending(true)
    const revisionAtRequest = eventRevisionRef.current
    try {
      const value = await invoke<DesktopWindowState>("set_desktop_windows_click_through", { clickThrough })
      if (eventRevisionRef.current === revisionAtRequest) {
        setState(value)
      }
      return value
    } finally {
      setPending(false)
    }
  }

  return {
    state,
    pending,
    setLocked,
    setClickThrough,
  }
}
