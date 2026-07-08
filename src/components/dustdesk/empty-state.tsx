import type { Icon } from "@phosphor-icons/react"
import type { ReactNode } from "react"
import { Card, CardContent } from "@/components/ui/card"

interface EmptyStateProps {
  icon: Icon
  title: string
  description?: string
  action?: ReactNode
}

export function EmptyState({ icon: Icon, title, description, action }: EmptyStateProps) {
  return (
    <Card className="h-full min-h-72 border-dashed">
      <CardContent className="flex h-full min-h-72 flex-col items-center justify-center gap-3 p-8 text-center">
        <div className="flex size-12 items-center justify-center rounded-lg bg-muted text-muted-foreground">
          <Icon className="size-6" weight="duotone" />
        </div>
        <div className="space-y-1">
          <h3 className="font-heading text-base font-medium">{title}</h3>
          {description ? <p className="max-w-sm text-sm text-muted-foreground">{description}</p> : null}
        </div>
        {action ? <div className="pt-2">{action}</div> : null}
      </CardContent>
    </Card>
  )
}
