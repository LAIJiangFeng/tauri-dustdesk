import { useEffect } from "react"
import { desktopWindowLayoutUserChangeEvent, listenCurrentWindowMove, listenCurrentWindowResize } from "@/lib/tauri-window"
import { useDustDeskStore } from "@/stores/dustdesk-store"

export function usePersistCurrentWindowLayout(label: string) {
  const saveDesktopWindowLayout = useDustDeskStore((state) => state.saveDesktopWindowLayout)

  useEffect(() => {
    if (!label) return

    let saveTimer: number | undefined
    let captureTimer: number | undefined
    let captureUntil = 0
    let unlistenMove: (() => void) | undefined
    let unlistenResize: (() => void) | undefined

    const scheduleSave = () => {
      if (Date.now() > captureUntil) return
      if (saveTimer) {
        window.clearTimeout(saveTimer)
      }
      saveTimer = window.setTimeout(() => {
        void saveDesktopWindowLayout(label)
      }, 220)
    }

    const beginUserLayoutChange = () => {
      captureUntil = Date.now() + 10_000
      if (captureTimer) {
        window.clearTimeout(captureTimer)
      }
      captureTimer = window.setTimeout(() => {
        captureUntil = 0
      }, 10_000)
      scheduleSave()
    }

    window.addEventListener(desktopWindowLayoutUserChangeEvent, beginUserLayoutChange)

    void listenCurrentWindowMove(scheduleSave).then((value) => {
      unlistenMove = value
    })
    void listenCurrentWindowResize(scheduleSave).then((value) => {
      unlistenResize = value
    })

    return () => {
      window.removeEventListener(desktopWindowLayoutUserChangeEvent, beginUserLayoutChange)
      if (captureTimer) {
        window.clearTimeout(captureTimer)
      }
      if (saveTimer) {
        window.clearTimeout(saveTimer)
      }
      unlistenMove?.()
      unlistenResize?.()
    }
  }, [label, saveDesktopWindowLayout])
}
