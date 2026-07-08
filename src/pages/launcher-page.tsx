import { useEffect, useState, type DragEvent as ReactDragEvent, type ReactNode } from "react"
import { open } from "@tauri-apps/plugin-dialog"
import { Desktop, FolderOpen, Plus, RocketLaunch } from "@phosphor-icons/react"
import { EmptyState } from "@/components/dustdesk/empty-state"
import { FileIcon } from "@/components/dustdesk/file-icon"
import { ItemContextMenu, type ItemContextMenuAction } from "@/components/dustdesk/item-context-menu"
import { LaunchConfirmButton } from "@/components/dustdesk/launch-confirm-button"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card"
import { ScrollArea } from "@/components/ui/scroll-area"
import { hasDustDeskPathDrag, hasPathLikeDrag, readDustDeskPathDrag } from "@/lib/dustdesk-dnd"
import { safeCurrentWebviewDragDropEvent } from "@/lib/tauri-window"
import { displayPathName, extensionFromPath } from "@/lib/utils"
import { useDustDeskStore } from "@/stores/dustdesk-store"

export function LauncherPage() {
  const snapshot = useDustDeskStore((state) => state.snapshot)
  const openPath = useDustDeskStore((state) => state.openPath)
  const openSpecial = useDustDeskStore((state) => state.openSpecial)
  const addLaunchers = useDustDeskStore((state) => state.addLaunchers)
  const removeLauncher = useDustDeskStore((state) => state.removeLauncher)
  const showPathInFolder = useDustDeskStore((state) => state.showPathInFolder)
  const desktopFrames = useDustDeskStore((state) => state.desktopFrames)
  const refreshDesktopFrameVisibility = useDustDeskStore((state) => state.refreshDesktopFrameVisibility)
  const toggleDesktopLauncherFrame = useDustDeskStore((state) => state.toggleDesktopLauncherFrame)

  useEffect(() => {
    void refreshDesktopFrameVisibility()
  }, [refreshDesktopFrameVisibility])

  useEffect(() => {
    let unlisten: (() => void) | undefined
    void safeCurrentWebviewDragDropEvent((event) => {
        if (event.payload.type !== "drop") return
        void addLauncherPaths(event.payload.paths)
      })
      .then((value) => {
        unlisten = value
      })
    return () => unlisten?.()
  }, [addLaunchers])

  async function addLauncherPaths(paths: string[]) {
    await addLaunchers(paths)
  }

  async function handleAddLauncher() {
    const selected = await open({
      multiple: true,
      directory: false,
      filters: [{ name: "启动项", extensions: ["lnk", "url", "exe", "appref-ms", "bat", "cmd", "ps1", "*"] }],
    })
    const paths = Array.isArray(selected) ? selected : selected ? [selected] : []
    await addLaunchers(paths)
  }

  async function handleAddLauncherDirectory() {
    const selected = await open({
      multiple: true,
      directory: true,
    })
    const paths = Array.isArray(selected) ? selected : selected ? [selected] : []
    await addLaunchers(paths)
  }

  function handleDragOver(event: ReactDragEvent<HTMLElement>) {
    if (!hasPathLikeDrag(event.dataTransfer)) return
    event.preventDefault()
    event.dataTransfer.dropEffect = "copy"
  }

  function handleDrop(event: ReactDragEvent<HTMLElement>) {
    if (!hasPathLikeDrag(event.dataTransfer)) return
    event.preventDefault()
    if (hasDustDeskPathDrag(event.dataTransfer)) {
      void addLauncherPaths(readDustDeskPathDrag(event.dataTransfer))
    }
  }

  return (
    <div className="grid h-full min-h-0 gap-4">
      <Card className="min-h-0" onDragOver={handleDragOver} onDrop={handleDrop}>
        <CardHeader>
          <div>
            <CardTitle>启动项</CardTitle>
          </div>
          <div className="flex flex-wrap items-center gap-2">
            <Button size="sm" onClick={() => void handleAddLauncher()}>
              <Plus className="size-3.5" weight="bold" />
              添加
            </Button>
            <Button variant="secondary" size="sm" onClick={() => void handleAddLauncherDirectory()}>
              <FolderOpen className="size-3.5" weight="duotone" />
              添加目录
            </Button>
            <LaunchConfirmButton count={snapshot.launchers.length} size="sm" />
            <Button variant="secondary" size="sm" onClick={() => void toggleDesktopLauncherFrame()}>
              <Desktop className="size-3.5" weight="duotone" />
              {desktopFrames.launcher ? "隐藏启动桌面框" : "显示启动桌面框"}
            </Button>
            <Button size="sm" variant="secondary" onClick={() => void openSpecial("launchers")}>
              <FolderOpen className="size-3.5" weight="duotone" />
              启动目录
            </Button>
          </div>
        </CardHeader>
        <CardContent className="min-h-0">
          <ScrollArea className="h-full pr-2">
            {snapshot.launchers.length > 0 ? (
              <div className="grid grid-cols-[repeat(auto-fill,minmax(126px,1fr))] gap-3">
                {snapshot.launchers.map((item, index) => (
                  <LauncherItemShell
                    key={`${item.name}-${item.path}-${index}`}
                    title={item.path}
                    onOpen={() => void openPath(item.path)}
                    actions={[
                      { label: "启动", icon: "open", onSelect: () => openPath(item.path) },
                      { label: "在资源管理器中显示", icon: "folder", onSelect: () => showPathInFolder(item.path) },
                      { label: "从快捷启动移除", icon: "remove", tone: "danger", onSelect: () => removeLauncher(item.path) },
                    ]}
                  >
                    <CardContent className="flex h-full flex-col items-center justify-between gap-2 p-3 text-center">
                      <div className="flex min-h-0 w-full flex-1 flex-col items-center justify-center gap-2">
                        <FileIcon name={item.name || displayPathName(item.path)} extension={extensionFromPath(item.path)} iconDataUrl={item.icon_data_url} className="size-12" />
                        <span className="w-full truncate text-sm font-medium">{item.name || displayPathName(item.path)}</span>
                      </div>
                      <Button size="xs" variant="secondary" className="w-full" onClick={(event) => {
                        event.stopPropagation()
                        void openPath(item.path)
                      }}>
                        <RocketLaunch className="size-3" weight="duotone" />
                        启动
                      </Button>
                    </CardContent>
                  </LauncherItemShell>
                ))}
              </div>
            ) : (
              <EmptyState icon={RocketLaunch} title="还没有启动项" />
            )}
          </ScrollArea>
        </CardContent>
      </Card>
    </div>
  )
}

function LauncherItemShell({
  title,
  actions,
  onOpen,
  children,
}: {
  title: string
  actions: ItemContextMenuAction[]
  onOpen: () => void
  children: ReactNode
}) {
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null)

  return (
    <Card
      className="group h-44 select-none overflow-hidden transition-colors hover:border-primary/45"
      title={title}
      draggable={false}
      onDragStart={(event) => event.preventDefault()}
      onDoubleClick={onOpen}
      onContextMenu={(event) => {
        event.preventDefault()
        setMenu({ x: event.clientX, y: event.clientY })
      }}
    >
      {children}
      {menu ? <ItemContextMenu x={menu.x} y={menu.y} actions={actions} onClose={() => setMenu(null)} /> : null}
    </Card>
  )
}
