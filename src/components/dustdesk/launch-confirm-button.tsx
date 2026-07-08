import { useState } from "react"
import { Play, RocketLaunch } from "@phosphor-icons/react"
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
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

  async function confirmStart() {
    setStarting(true)
    try {
      await startAllLaunchers()
      setOpen(false)
    } finally {
      setStarting(false)
    }
  }

  return (
    <AlertDialog open={open} onOpenChange={setOpen}>
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
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel disabled={starting}>取消</AlertDialogCancel>
          <AlertDialogAction onClick={() => void confirmStart()} disabled={starting}>
            {starting ? "启动中" : "确认启动"}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  )
}
