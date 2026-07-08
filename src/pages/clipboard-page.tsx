import { useEffect, useState } from "react"
import { CircleNotch, ClipboardText, ImageSquare, Keyboard, Lightning, TextT, type Icon } from "@phosphor-icons/react"
import { convertFileSrc } from "@tauri-apps/api/core"
import { EmptyState } from "@/components/dustdesk/empty-state"
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog"
import { Badge } from "@/components/ui/badge"
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card"
import { ScrollArea } from "@/components/ui/scroll-area"
import { cn, truncate } from "@/lib/utils"
import { useDustDeskStore } from "@/stores/dustdesk-store"
import type { ClipboardHistoryItem } from "@/types"

export function ClipboardPage() {
  const snapshot = useDustDeskStore((state) => state.snapshot)
  const [selectedItem, setSelectedItem] = useState<ClipboardHistoryItem | null>(null)
  const imageCount = snapshot.clipboard.filter((item) => item.kind === "Image").length
  const textCount = snapshot.clipboard.length - imageCount
  const pathCount = snapshot.clipboard.filter((item) => looksLikePath(item.text)).length
  const latestItems = snapshot.clipboard.slice(0, 6)
  const typeStats: Array<{ label: string; value: number; hint: string; icon: Icon }> = [
    { label: "文本", value: textCount, hint: "普通文字记录", icon: TextT },
    { label: "图片", value: imageCount, hint: "截图与图片", icon: ImageSquare },
    { label: "路径", value: pathCount, hint: "疑似文件路径", icon: Lightning },
  ]
  const shortcuts = [
    ["Ctrl + Tab", "呼出剪贴板"],
    ["Tab / 方向键", "切换卡片"],
    ["松开 Ctrl", "粘贴选中"],
  ]

  return (
    <div className="grid h-full min-h-0 gap-4 xl:grid-cols-[minmax(0,1fr)_360px]">
      <Card className="min-h-0">
        <CardHeader>
          <div>
            <CardTitle>剪贴历史</CardTitle>
          </div>
          <Badge variant="outline">{snapshot.clipboard.length} 条</Badge>
        </CardHeader>
        <CardContent className="min-h-0">
          <ScrollArea className="h-full pr-2">
            {snapshot.clipboard.length > 0 ? (
              <div className="grid gap-3 lg:grid-cols-2 2xl:grid-cols-3">
                {snapshot.clipboard.map((item, index) => (
                  <ClipboardHistoryCard key={item.id || index} item={item} onOpen={setSelectedItem} />
                ))}
              </div>
            ) : (
              <EmptyState icon={ClipboardText} title="暂无剪贴历史" />
            )}
          </ScrollArea>
        </CardContent>
      </Card>

      <Card className="min-h-0">
        <CardHeader>
          <div>
            <CardTitle>剪贴板概览</CardTitle>
          </div>
          <Badge variant="secondary" className="gap-2">
            <Keyboard className="size-3.5" weight="duotone" />
            {snapshot.settings.clipboard_shortcut}
          </Badge>
        </CardHeader>
        <CardContent className="flex min-h-0 flex-col gap-4">
          <div className="grid gap-2">
            {typeStats.map(({ label, value, hint, icon: Icon }, index) => (
              <Card key={label} size="sm" className={cn("border bg-background/60", index === 0 && "bg-primary text-primary-foreground")}>
                <CardContent className="flex items-center gap-3 p-3">
                  <div className={cn("flex size-10 shrink-0 items-center justify-center rounded-lg bg-muted text-muted-foreground", index === 0 && "bg-primary-foreground/15 text-primary-foreground")}>
                    <Icon className="size-5" weight="duotone" />
                  </div>
                  <div className="min-w-0 flex-1">
                    <div className="text-sm font-medium">{label}</div>
                    <div className={cn("truncate text-xs text-muted-foreground", index === 0 && "text-primary-foreground/70")}>{hint}</div>
                  </div>
                  <strong className="font-heading text-2xl font-semibold tabular-nums">{value}</strong>
                </CardContent>
              </Card>
            ))}
          </div>

          <Card size="sm" className="min-h-0 flex-1 border bg-background/60">
            <CardHeader>
              <div>
                <CardTitle>最近片段</CardTitle>
              </div>
              <Badge variant="outline">{latestItems.length}</Badge>
            </CardHeader>
            <CardContent className="min-h-0">
              <ScrollArea className="h-full pr-2">
                {latestItems.length > 0 ? (
                  <div className="grid gap-2">
                    {latestItems.map((item, index) => (
                      <button
                        key={item.id || index}
                        type="button"
                        className="rounded-lg border bg-card px-3 py-2 text-left transition-colors hover:bg-muted/40 focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50"
                        onClick={() => setSelectedItem(item)}
                      >
                        <div className="mb-1 flex items-center justify-between gap-2">
                          <Badge variant={item.kind === "Image" ? "secondary" : "outline"}>{item.kind}</Badge>
                          <span className="text-xs tabular-nums text-muted-foreground">{String(index + 1).padStart(2, "0")}</span>
                        </div>
                        {item.kind === "Image" && hasImagePreview(item) ? (
                          <img src={imageSource(item)} alt="图片剪贴内容" className="h-14 w-full rounded-md border object-cover" />
                        ) : (
                          <p className="line-clamp-2 text-xs leading-5 text-muted-foreground [overflow-wrap:anywhere]">
                            {truncate(item.text || "图片剪贴内容", 86)}
                          </p>
                        )}
                      </button>
                    ))}
                  </div>
                ) : (
                  <div className="flex min-h-32 items-center justify-center rounded-lg border border-dashed text-sm text-muted-foreground">
                    暂无最近记录
                  </div>
                )}
              </ScrollArea>
            </CardContent>
          </Card>

          <Card size="sm" className="border bg-background/60">
            <CardContent className="grid gap-2 p-3">
              {shortcuts.map(([key, value]) => (
                <div key={key} className="flex items-center justify-between gap-3 text-sm">
                  <Badge variant="outline" className="font-mono">
                    {key}
                  </Badge>
                  <span className="text-muted-foreground">{value}</span>
                </div>
              ))}
            </CardContent>
          </Card>
        </CardContent>
      </Card>
      <ClipboardDetailDialog item={selectedItem} onOpenChange={(open) => !open && setSelectedItem(null)} />
    </div>
  )
}

function ClipboardHistoryCard({
  item,
  onOpen,
}: {
  item: ClipboardHistoryItem
  onOpen: (item: ClipboardHistoryItem) => void
}) {
  return (
    <button
      type="button"
      className="group min-w-0 rounded-xl text-left outline-none transition-transform hover:-translate-y-0.5 focus-visible:ring-[3px] focus-visible:ring-ring/50"
      onClick={() => onOpen(item)}
    >
      <Card className="h-full border bg-card/80 transition-colors group-hover:bg-muted/40">
        <CardContent className="grid min-h-44 grid-rows-[auto_minmax(0,1fr)] gap-4 p-4">
          <div className="flex items-center justify-between gap-3">
            <Badge variant={item.kind === "Image" ? "secondary" : "outline"}>{item.kind}</Badge>
            <span className="truncate text-xs text-muted-foreground">{item.created_at || "本地记录"}</span>
          </div>
          {item.kind === "Image" && hasImagePreview(item) ? (
            <ImagePreview item={item} />
          ) : (
            <p className="overflow-hidden text-sm leading-6 text-muted-foreground [overflow-wrap:anywhere]">
              {truncate(item.text || "图片剪贴内容", 180)}
            </p>
          )}
        </CardContent>
      </Card>
    </button>
  )
}

function ClipboardDetailDialog({
  item,
  onOpenChange,
}: {
  item: ClipboardHistoryItem | null
  onOpenChange: (open: boolean) => void
}) {
  const clipboardImageBase64 = useDustDeskStore((state) => state.clipboardImageBase64)
  const [fullImageBase64, setFullImageBase64] = useState("")
  const [imageError, setImageError] = useState("")
  const [imageLoading, setImageLoading] = useState(false)
  const isImage = item?.kind === "Image" && hasImagePreview(item)
  const imageSrc = fullImageBase64 ? `data:image/png;base64,${fullImageBase64}` : item ? imageSource(item) : ""

  useEffect(() => {
    setFullImageBase64("")
    setImageError("")
    setImageLoading(false)
    if (!item || item.kind !== "Image") return

    let cancelled = false
    setImageLoading(true)
    clipboardImageBase64(item.id)
      .then((value) => {
        if (!cancelled) setFullImageBase64(value)
      })
      .catch((reason) => {
        if (!cancelled) setImageError(reason instanceof Error ? reason.message : String(reason))
      })
      .finally(() => {
        if (!cancelled) setImageLoading(false)
      })

    return () => {
      cancelled = true
    }
  }, [clipboardImageBase64, item])

  return (
    <AlertDialog open={Boolean(item)} onOpenChange={onOpenChange}>
      <AlertDialogContent
        className={cn(
          "w-[calc(100vw-2rem)] gap-5 overflow-hidden p-5",
          isImage ? "max-w-[min(96vw,1440px)]" : "max-w-[min(92vw,820px)]"
        )}
        size="wide"
      >
        <AlertDialogHeader className="place-items-start text-left">
          <div className="flex w-full items-center justify-between gap-3">
            <div className="min-w-0">
              <AlertDialogTitle>{isImage ? "图片剪贴内容" : "文本剪贴内容"}</AlertDialogTitle>
              <AlertDialogDescription className="mt-1 text-left">
                {item?.created_at || "本地记录"}
              </AlertDialogDescription>
            </div>
            <Badge variant={isImage ? "secondary" : "outline"}>{item?.kind || "Text"}</Badge>
          </div>
        </AlertDialogHeader>

        {item ? (
          isImage ? (
            <div className="grid gap-2">
              <div className="relative flex h-[min(78vh,860px)] items-center justify-center overflow-hidden rounded-xl border bg-muted/30 p-3">
                <img src={imageSrc} alt="放大的图片剪贴内容" className={cn("max-h-full max-w-full object-contain transition-opacity", imageLoading && "opacity-55")} />
                {imageLoading ? (
                  <div className="absolute inset-0 flex flex-col items-center justify-center gap-2 bg-background/70 text-sm font-semibold text-foreground backdrop-blur-sm">
                    <CircleNotch className="size-8 animate-spin text-primary" weight="bold" />
                    正在加载原图
                  </div>
                ) : null}
              </div>
              {imageError ? <p className="text-xs text-destructive">{imageError}</p> : null}
            </div>
          ) : (
            <ScrollArea className="max-h-[65vh] min-w-0 max-w-full overflow-hidden rounded-xl border bg-muted/25">
              <pre className="m-0 block min-w-0 max-w-full whitespace-pre-wrap break-all p-4 font-mono text-sm leading-6 text-foreground [overflow-wrap:anywhere]">
                {item.text || "空文本"}
              </pre>
            </ScrollArea>
          )
        ) : null}

        <AlertDialogFooter>
          <AlertDialogCancel>关闭</AlertDialogCancel>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  )
}

function ImagePreview({ item }: { item: ClipboardHistoryItem }) {
  return (
    <div className="min-h-0 overflow-hidden rounded-lg border bg-muted/30">
      <img src={imageSource(item)} alt="图片剪贴内容" className="h-full max-h-36 w-full object-contain" />
    </div>
  )
}

function hasImagePreview(item: ClipboardHistoryItem) {
  return Boolean(item.image_thumb_path || item.image_path || item.image_png_base64)
}

function imageSource(item: ClipboardHistoryItem) {
  if (item.image_png_base64) {
    return `data:image/png;base64,${item.image_png_base64}`
  }

  const path = item.image_thumb_path || item.image_path
  if (path && "__TAURI_INTERNALS__" in window) {
    return convertFileSrc(path)
  }
  return ""
}

function looksLikePath(text: string) {
  const value = text.trim()
  return /^[a-zA-Z]:[\\/]/.test(value) || value.startsWith("\\\\") || value.startsWith("/") || /\.(lnk|exe|bat|cmd|ps1|app)$/i.test(value)
}
