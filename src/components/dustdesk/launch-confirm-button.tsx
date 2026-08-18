import { useState } from "react"
import { Play, RocketLaunch, ShieldCheck } from "@phosphor-icons/react"
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogMedia,
  AlertDialogTitle,
  AlertDialogTrigger,
} from "@/components/ui/alert-dialog"
import { Button } from "@/components/ui/button"
import { useDustDeskStore } from "@/stores/dustdesk-store"

interface LaunchConfirmButtonProps {
  count: number
  size?: "default" | "sm" | "lg"
  className?: string
}

export function LaunchConfirmButton({ count, size = "default", className }: LaunchConfirmButtonProps) {
  const startAllLaunchers = useDustDeskStore((state) => state.startAllLaunchers)
  const [open, setOpen] = useState(false)
  const [starting, setStarting] = useState(false)
  const [failure, setFailure] = useState("")

  async function confirmStart(asAdministrator: boolean) {
    setStarting(true)
    setFailure("")
    try {
      await startAllLaunchers(asAdministrator)
      setOpen(false)
    } catch (error) {
      setFailure(error instanceof Error ? error.message : String(error))
    } finally {
      setStarting(false)
    }
  }

  return (
    <AlertDialog
      open={open}
      onOpenChange={(value) => {
        if (starting) return
        setOpen(value)
        if (value) setFailure("")
      }}
    >
      <AlertDialogTrigger asChild>
        <Button size={size} className={className} disabled={count === 0}>
          <Play className="size-4" weight="fill" />
          启动全部
        </Button>
      </AlertDialogTrigger>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogMedia>
            <RocketLaunch className="size-6" weight="duotone" />
          </AlertDialogMedia>
          <AlertDialogTitle>启动 {count} 项？</AlertDialogTitle>
          <AlertDialogDescription>普通启动直接打开全部项目；管理员启动只请求一次 Windows UAC 授权。</AlertDialogDescription>
          {failure ? <AlertDialogDescription className="text-destructive">{failure}</AlertDialogDescription> : null}
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel disabled={starting}>取消</AlertDialogCancel>
          <AlertDialogAction
            variant="secondary"
            onClick={(event) => {
              event.preventDefault()
              void confirmStart(false)
            }}
            disabled={starting}
          >
            <RocketLaunch className="size-4" weight="duotone" />
            {starting ? "启动中" : "普通启动"}
          </AlertDialogAction>
          <AlertDialogAction
            onClick={(event) => {
              event.preventDefault()
              void confirmStart(true)
            }}
            disabled={starting}
          >
            <ShieldCheck className="size-4" weight="duotone" />
            {starting ? "启动中" : "管理员启动"}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  )
}
