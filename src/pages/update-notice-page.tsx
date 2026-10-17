import { useCallback, useEffect, useState } from "react"
import { ArrowCircleUp, DownloadSimple, X } from "@phosphor-icons/react"
import { Button } from "@/components/ui/button"
import { useTheme } from "@/hooks/use-theme"
import { safeListen } from "@/lib/tauri-window"
import { normalizeUpdateNotice, useDustDeskStore } from "@/stores/dustdesk-store"
import type { AppUpdateInfo } from "@/types"

const UPDATE_NOTICE_CHANGED_EVENT = "dustdesk://update-notice-changed"
const RELEASE_NOTE_LINES = 2

type NoticeAction = "update" | "later" | "skip"

function releaseNoteLines(body: string) {
  return body
    .split(/\r?\n/)
    .map((line) => line.replace(/^\s*(?:#{1,6}\s+|[-*+]\s+|\d+\.\s+)/, "").trim())
    .filter(Boolean)
    .slice(0, RELEASE_NOTE_LINES)
}

export function UpdateNoticePage() {
  useTheme()
  const getUpdateNotice = useDustDeskStore((state) => state.getUpdateNotice)
  const dismissUpdateNotice = useDustDeskStore((state) => state.dismissUpdateNotice)
  const openUpdateDownload = useDustDeskStore((state) => state.openUpdateDownload)
  const updateUpdateSettings = useDustDeskStore((state) => state.updateUpdateSettings)
  const [update, setUpdate] = useState<AppUpdateInfo | null>(null)
  const [pendingAction, setPendingAction] = useState<NoticeAction | "">("")
  const [error, setError] = useState("")

  useEffect(() => {
    let disposed = false
    let unlisten: (() => void) | undefined

    // The window may be created after the backend emitted its event, so read the state first.
    void getUpdateNotice()
      .then((notice) => {
        if (!disposed) setUpdate(notice)
      })
      .catch(() => undefined)
    void safeListen<unknown>(UPDATE_NOTICE_CHANGED_EVENT, (event) => {
      setUpdate(normalizeUpdateNotice(event.payload))
    }).then((dispose) => {
      if (disposed) dispose?.()
      else unlisten = dispose
    })

    return () => {
      disposed = true
      unlisten?.()
    }
  }, [getUpdateNotice])

  const runAction = useCallback(
    async (action: NoticeAction) => {
      if (pendingAction) return
      setPendingAction(action)
      setError("")
      try {
        if (action === "update" && update) {
          await openUpdateDownload(update.download_url)
        } else if (action === "skip" && update) {
          await updateUpdateSettings({ skippedUpdateVersion: update.latest_version })
        }
        await dismissUpdateNotice()
      } catch (reason) {
        setError(reason instanceof Error ? reason.message : String(reason))
        setPendingAction("")
      }
    },
    [dismissUpdateNotice, openUpdateDownload, pendingAction, update, updateUpdateSettings],
  )

  const notes = update ? releaseNoteLines(update.body) : []

  return (
    <main className="flex h-screen w-screen select-none flex-col gap-2 overflow-hidden border border-border bg-background p-3 text-foreground">
      <div className="flex items-start gap-2.5">
        <div className="flex size-8 shrink-0 items-center justify-center rounded-lg bg-primary/10 text-primary">
          <ArrowCircleUp className="size-5" weight="duotone" />
        </div>
        <div className="min-w-0 flex-1">
          <h1 className="font-heading text-sm font-medium leading-5">DustDesk 有新版本</h1>
          <p className="truncate text-xs leading-5 text-muted-foreground">
            {update ? `当前 ${update.current_version} → 最新 ${update.latest_version}` : "正在读取更新信息..."}
          </p>
        </div>
        <Button
          type="button"
          variant="ghost"
          size="icon-xs"
          aria-label="稍后提醒"
          disabled={Boolean(pendingAction)}
          onClick={() => void runAction("later")}
        >
          <X weight="bold" />
        </Button>
      </div>

      <div className="min-h-0 flex-1 overflow-hidden pl-[42px]">
        {error ? (
          <p className="line-clamp-2 text-xs leading-5 text-destructive [overflow-wrap:anywhere]">{error}</p>
        ) : (
          notes.map((line, index) => (
            <p key={`${index}-${line}`} className="truncate text-xs leading-5 text-muted-foreground">
              {line}
            </p>
          ))
        )}
      </div>

      <div className="flex items-center justify-end gap-1.5">
        <Button type="button" variant="ghost" size="sm" disabled={!update || Boolean(pendingAction)} onClick={() => void runAction("skip")}>
          跳过此版本
        </Button>
        <Button type="button" variant="outline" size="sm" disabled={Boolean(pendingAction)} onClick={() => void runAction("later")}>
          稍后提醒
        </Button>
        <Button type="button" size="sm" disabled={!update || Boolean(pendingAction)} onClick={() => void runAction("update")}>
          <DownloadSimple weight="bold" />
          {pendingAction === "update" ? "正在打开" : "立即更新"}
        </Button>
      </div>
    </main>
  )
}
