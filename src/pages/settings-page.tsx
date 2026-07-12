import { useEffect, useRef, useState } from "react"
import type { KeyboardEvent as ReactKeyboardEvent } from "react"
import { ArrowsClockwise, Desktop, DownloadSimple, FolderOpen, GearSix, HardDrives, Keyboard, MagnifyingGlass, PlayCircle, Plus, X } from "@phosphor-icons/react"
import { open } from "@tauri-apps/plugin-dialog"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card"
import { Input } from "@/components/ui/input"
import { ScrollArea } from "@/components/ui/scroll-area"
import { useDustDeskStore } from "@/stores/dustdesk-store"
import type { AppUpdateInfo } from "@/types"

const MODIFIER_CODES = new Set(["ControlLeft", "ControlRight", "ShiftLeft", "ShiftRight", "AltLeft", "AltRight", "MetaLeft", "MetaRight"])
type RuntimeDirectoryTarget = "data" | "organizer" | "launchers"

function shortcutKeyFromEvent(event: KeyboardEvent) {
  if (MODIFIER_CODES.has(event.code)) return ""
  if (event.code.startsWith("Key")) return event.code.slice(3).toUpperCase()
  if (event.code.startsWith("Digit")) return event.code.slice(5)
  if (event.code === "Space") return "Space"
  return event.code || event.key
}

function shortcutFromEvent(event: KeyboardEvent) {
  const modifiers: string[] = []
  if (event.ctrlKey) modifiers.push("Ctrl")
  if (event.altKey) modifiers.push("Alt")
  if (event.shiftKey) modifiers.push("Shift")
  if (event.metaKey) modifiers.push("Super")

  const key = shortcutKeyFromEvent(event)
  if (!key) return modifiers.join("+")
  return [...modifiers, key].join("+")
}

export function SettingsPage() {
  const snapshot = useDustDeskStore((state) => state.snapshot)
  const openSpecial = useDustDeskStore((state) => state.openSpecial)
  const updateRuntimeDirectory = useDustDeskStore((state) => state.updateRuntimeDirectory)
  const updateClipboardShortcut = useDustDeskStore((state) => state.updateClipboardShortcut)
  const updateSearchSettings = useDustDeskStore((state) => state.updateSearchSettings)
  const updateLaunchOnStartup = useDustDeskStore((state) => state.updateLaunchOnStartup)
  const checkForUpdates = useDustDeskStore((state) => state.checkForUpdates)
  const openUpdateDownload = useDustDeskStore((state) => state.openUpdateDownload)
  const shortcutInputRef = useRef<HTMLInputElement | null>(null)
  const searchShortcutInputRef = useRef<HTMLInputElement | null>(null)
  const [shortcutDraft, setShortcutDraft] = useState(snapshot.settings.clipboard_shortcut)
  const [isRecordingShortcut, setIsRecordingShortcut] = useState(false)
  const [isSavingShortcut, setIsSavingShortcut] = useState(false)
  const [shortcutError, setShortcutError] = useState("")
  const [shortcutSuccess, setShortcutSuccess] = useState("")
  const [searchEnabledDraft, setSearchEnabledDraft] = useState(snapshot.settings.search_enabled)
  const [searchShortcutDraft, setSearchShortcutDraft] = useState(snapshot.settings.search_shortcut)
  const [searchPathsDraft, setSearchPathsDraft] = useState(snapshot.settings.search_paths)
  const [searchPathInput, setSearchPathInput] = useState("")
  const [isRecordingSearchShortcut, setIsRecordingSearchShortcut] = useState(false)
  const [isSavingSearch, setIsSavingSearch] = useState(false)
  const [savingDirectoryTarget, setSavingDirectoryTarget] = useState<RuntimeDirectoryTarget | "">("")
  const [directoryError, setDirectoryError] = useState("")
  const [directorySuccess, setDirectorySuccess] = useState("")
  const [searchError, setSearchError] = useState("")
  const [searchSuccess, setSearchSuccess] = useState("")
  const [isSavingStartup, setIsSavingStartup] = useState(false)
  const [startupError, setStartupError] = useState("")
  const [startupSuccess, setStartupSuccess] = useState("")
  const [isCheckingUpdate, setIsCheckingUpdate] = useState(false)
  const [updateInfo, setUpdateInfo] = useState<AppUpdateInfo | null>(null)
  const [updateError, setUpdateError] = useState("")
  const [updateSuccess, setUpdateSuccess] = useState("")
  const safeSearchPathsDraft = Array.isArray(searchPathsDraft) ? searchPathsDraft : []
  const effectiveSearchPaths = safeSearchPathsDraft.length > 0 ? safeSearchPathsDraft : [snapshot.organizer_root].filter(Boolean)
  const rows = [
    { name: "数据目录", value: snapshot.data_dir, target: "data" as const, icon: HardDrives },
    { name: "收纳目录", value: snapshot.organizer_root, target: "organizer" as const, icon: FolderOpen },
    { name: "快捷启动目录", value: snapshot.launchers_root, target: "launchers" as const, icon: GearSix },
    { name: "系统桌面", value: "当前用户 Desktop", target: "desktop" as const, icon: Desktop },
  ]

  useEffect(() => {
    if (!isRecordingShortcut) {
      setShortcutDraft(snapshot.settings.clipboard_shortcut)
    }
  }, [isRecordingShortcut, snapshot.settings.clipboard_shortcut])

  useEffect(() => {
    if (!isRecordingSearchShortcut) {
      setSearchShortcutDraft(snapshot.settings.search_shortcut)
    }
    setSearchEnabledDraft(snapshot.settings.search_enabled)
    setSearchPathsDraft(snapshot.settings.search_paths)
  }, [isRecordingSearchShortcut, snapshot.settings.search_enabled, snapshot.settings.search_paths, snapshot.settings.search_shortcut])

  useEffect(() => {
    if (isRecordingShortcut) {
      shortcutInputRef.current?.focus()
      shortcutInputRef.current?.select()
    }
  }, [isRecordingShortcut])

  useEffect(() => {
    if (isRecordingSearchShortcut) {
      searchShortcutInputRef.current?.focus()
      searchShortcutInputRef.current?.select()
    }
  }, [isRecordingSearchShortcut])

  const saveShortcut = async (shortcut: string) => {
    const nextShortcut = shortcut.trim()
    if (!nextShortcut) {
      setShortcutError("快捷键不能为空")
      return
    }

    setIsSavingShortcut(true)
    setShortcutError("")
    setShortcutSuccess("")
    try {
      const settings = await updateClipboardShortcut(nextShortcut)
      setShortcutDraft(settings.clipboard_shortcut)
      setShortcutSuccess("剪贴板快捷键已保存")
    } catch (reason) {
      setShortcutError(reason instanceof Error ? reason.message : String(reason))
    } finally {
      setIsSavingShortcut(false)
    }
  }

  const saveSearchSettings = async (
    enabled = searchEnabledDraft,
    shortcut = searchShortcutDraft,
    paths = searchPathsDraft,
  ) => {
    const nextShortcut = shortcut.trim()
    if (!nextShortcut) {
      setSearchError("搜索快捷键不能为空")
      return
    }

    const nextPaths = normalizePathList(paths)
    setIsSavingSearch(true)
    setSearchError("")
    setSearchSuccess("")
    try {
      const settings = await updateSearchSettings(enabled, nextShortcut, nextPaths)
      setSearchEnabledDraft(settings.search_enabled)
      setSearchShortcutDraft(settings.search_shortcut)
      setSearchPathsDraft(settings.search_paths)
      setSearchSuccess("搜索设置已保存")
    } catch (reason) {
      setSearchError(reason instanceof Error ? reason.message : String(reason))
    } finally {
      setIsSavingSearch(false)
    }
  }

  const handleShortcutKeyDown = (event: ReactKeyboardEvent<HTMLInputElement>) => {
    if (!isRecordingShortcut) return

    event.preventDefault()
    event.stopPropagation()

    const nextShortcut = shortcutFromEvent(event.nativeEvent)
    if (!nextShortcut) return

    setShortcutDraft(nextShortcut)
    if (!shortcutKeyFromEvent(event.nativeEvent)) {
      setShortcutError("继续按一个主键，例如 Tab / V / Space")
      return
    }

    if (!nextShortcut.includes("+")) {
      setShortcutError("请按住 Ctrl / Alt / Shift / Win 中至少一个修饰键")
      return
    }

    setIsRecordingShortcut(false)
    void saveShortcut(nextShortcut)
  }

  const handleSearchShortcutKeyDown = (event: ReactKeyboardEvent<HTMLInputElement>) => {
    if (!isRecordingSearchShortcut) return

    event.preventDefault()
    event.stopPropagation()

    const nextShortcut = shortcutFromEvent(event.nativeEvent)
    if (!nextShortcut) return

    setSearchShortcutDraft(nextShortcut)
    if (!shortcutKeyFromEvent(event.nativeEvent)) {
      setSearchError("继续按一个主键，例如 Space / K / F")
      return
    }

    if (!nextShortcut.includes("+")) {
      setSearchError("请按住 Ctrl / Alt / Shift / Win 中至少一个修饰键")
      return
    }

    setIsRecordingSearchShortcut(false)
    void saveSearchSettings(searchEnabledDraft, nextShortcut, searchPathsDraft)
  }

  const addSearchPath = (path = searchPathInput) => {
    const trimmed = path.trim()
    if (!trimmed) return

    setSearchPathsDraft((paths) => normalizePathList([...defaultSearchPaths(snapshot.organizer_root, paths), trimmed]))
    setSearchPathInput("")
    setSearchError("")
    setSearchSuccess("")
  }

  const chooseSearchPath = async () => {
    setSearchError("")
    try {
      const selected = await open({
        directory: true,
        multiple: false,
        title: "选择搜索目录",
      })
      if (typeof selected === "string") {
        addSearchPath(selected)
      }
    } catch (reason) {
      setSearchError(reason instanceof Error ? reason.message : String(reason))
    }
  }

  const removeSearchPath = (index: number) => {
    setSearchPathsDraft((paths) => paths.filter((_, itemIndex) => itemIndex !== index))
    setSearchError("")
    setSearchSuccess("")
  }

  const chooseRuntimeDirectory = async (target: RuntimeDirectoryTarget, currentPath: string) => {
    setDirectoryError("")
    setDirectorySuccess("")
    try {
      const selected = await open({
        directory: true,
        multiple: false,
        title: `选择${runtimeDirectoryLabel(target)}`,
        defaultPath: currentPath || undefined,
      })
      if (typeof selected !== "string") return

      setSavingDirectoryTarget(target)
      const snapshot = await updateRuntimeDirectory(target, selected)
      setSearchPathsDraft(snapshot.settings.search_paths)
      setDirectorySuccess(`${runtimeDirectoryLabel(target)}已修改`)
    } catch (reason) {
      setDirectoryError(reason instanceof Error ? reason.message : String(reason))
    } finally {
      setSavingDirectoryTarget("")
    }
  }

  const toggleLaunchOnStartup = async () => {
    const enabled = !snapshot.settings.launch_on_startup
    setIsSavingStartup(true)
    setStartupError("")
    setStartupSuccess("")
    try {
      const settings = await updateLaunchOnStartup(enabled)
      setStartupSuccess(settings.launch_on_startup ? "已设置为开机自动启动" : "已关闭开机自动启动")
    } catch (reason) {
      setStartupError(reason instanceof Error ? reason.message : String(reason))
    } finally {
      setIsSavingStartup(false)
    }
  }

  const checkUpdateNow = async () => {
    setIsCheckingUpdate(true)
    setUpdateError("")
    setUpdateSuccess("")
    try {
      const update = await checkForUpdates()
      setUpdateInfo(update)
      setUpdateSuccess(update.update_available ? `发现新版本 ${update.latest_version}` : "当前已经是最新版本")
      if (update.update_available && window.confirm(`发现 DustDesk ${update.latest_version}，是否现在下载更新？`)) {
        await openUpdateDownload(update.download_url)
        setUpdateSuccess("已打开更新下载链接，下载后运行安装包即可覆盖更新")
      }
    } catch (reason) {
      setUpdateError(reason instanceof Error ? reason.message : String(reason))
    } finally {
      setIsCheckingUpdate(false)
    }
  }

  const downloadUpdate = async () => {
    if (!updateInfo?.download_url) return
    setUpdateError("")
    try {
      await openUpdateDownload(updateInfo.download_url)
      setUpdateSuccess("已打开更新下载链接，下载后运行安装包即可覆盖更新")
    } catch (reason) {
      setUpdateError(reason instanceof Error ? reason.message : String(reason))
    }
  }

  return (
    <Card className="h-full min-h-0">
        <CardHeader>
          <div>
            <CardTitle>设置中心</CardTitle>
          </div>
          <Badge variant="outline">{rows.length + 3} 项</Badge>
      </CardHeader>
      <CardContent className="min-h-0">
        <ScrollArea className="h-full pr-2">
          <div className="mb-3 grid gap-3 xl:grid-cols-[minmax(0,1.15fr)_minmax(0,1.85fr)]">
            <Card>
              <CardContent className="flex min-h-56 flex-col gap-4 p-5">
                <div className="flex size-11 items-center justify-center rounded-lg bg-muted text-muted-foreground">
                  <Keyboard className="size-5" weight="duotone" />
                </div>
                <div>
                  <h3 className="font-heading text-base font-medium">剪贴板快捷键</h3>
                  <p className="mt-2 text-sm leading-6 text-muted-foreground">
                    按住组合键唤起剪贴板；松开 Ctrl 后会粘贴当前选中的记录。
                  </p>
                </div>
                <Badge className="mt-auto w-fit" variant="outline">
                  当前：{snapshot.settings.clipboard_shortcut}
                </Badge>
              </CardContent>
            </Card>

            <Card>
              <CardContent className="flex min-h-56 flex-col gap-4 p-5">
                <div className="grid gap-2">
                  <div className="flex items-center justify-between gap-3">
                    <h3 className="font-heading text-base font-medium">自定义快捷键</h3>
                    {isRecordingShortcut ? <Badge>录制中</Badge> : <Badge variant="outline">全局快捷键</Badge>}
                  </div>
                  <Input
                    ref={shortcutInputRef}
                    value={shortcutDraft}
                    placeholder="例如 Ctrl+Alt+V"
                    className="h-11 font-mono"
                    onChange={(event) => setShortcutDraft(event.target.value)}
                    onKeyDown={handleShortcutKeyDown}
                  />
                  <p className="text-xs leading-5 text-muted-foreground">
                    格式示例：Ctrl+Tab、Ctrl+Alt+V、Alt+Space。必须包含至少一个修饰键。
                  </p>
                  {shortcutSuccess ? <p className="text-xs leading-5 text-emerald-600 dark:text-emerald-400">{shortcutSuccess}</p> : null}
                  {shortcutError ? <p className="text-xs leading-5 text-destructive">{shortcutError}</p> : null}
                </div>
                <div className="mt-auto flex flex-wrap gap-2">
                  <Button type="button" variant={isRecordingShortcut ? "default" : "secondary"} onClick={() => setIsRecordingShortcut(true)}>
                    {isRecordingShortcut ? "请按组合键" : "录制快捷键"}
                  </Button>
                  <Button type="button" disabled={isSavingShortcut} onClick={() => void saveShortcut(shortcutDraft)}>
                    {isSavingShortcut ? "保存中" : "保存"}
                  </Button>
                  <Button
                    type="button"
                    variant="ghost"
                    onClick={() => {
                      setShortcutDraft(snapshot.settings.clipboard_shortcut)
                      setShortcutError("")
                      setShortcutSuccess("")
                      setIsRecordingShortcut(false)
                    }}
                  >
                    取消
                  </Button>
                </div>
              </CardContent>
            </Card>
          </div>

          <div className="mb-3 grid gap-3 xl:grid-cols-[minmax(0,1.05fr)_minmax(0,1.95fr)]">
            <Card>
              <CardContent className="flex min-h-72 flex-col gap-4 p-5">
                <div className="flex size-11 items-center justify-center rounded-lg bg-muted text-muted-foreground">
                  <MagnifyingGlass className="size-5" weight="duotone" />
                </div>
                <div>
                  <h3 className="font-heading text-base font-medium">全局搜索</h3>
                  <p className="mt-2 text-sm leading-6 text-muted-foreground">
                    Ctrl+Space 屏幕中间弹出搜索框，支持搜索配置路径、快捷启动、程序、文件和目录。
                  </p>
                </div>
                <div className="mt-auto flex flex-wrap gap-2">
                  <Badge className="w-fit" variant={searchEnabledDraft ? "default" : "outline"}>
                    {searchEnabledDraft ? "已启用" : "已禁用"}
                  </Badge>
                  <Badge className="w-fit" variant="outline">
                    当前：{snapshot.settings.search_shortcut}
                  </Badge>
                </div>
              </CardContent>
            </Card>

            <Card>
              <CardContent className="grid min-h-72 gap-4 p-5">
                <div className="grid gap-2">
                  <div className="flex flex-wrap items-center justify-between gap-3">
                    <h3 className="font-heading text-base font-medium">搜索快捷键与路径</h3>
                    {isRecordingSearchShortcut ? <Badge>录制中</Badge> : <Badge variant="outline">Spotlight 弹窗</Badge>}
                  </div>
                  <div className="grid gap-2 md:grid-cols-[minmax(0,1fr)_auto]">
                    <Input
                      ref={searchShortcutInputRef}
                      value={searchShortcutDraft}
                      placeholder="例如 Ctrl+Space"
                      className="h-11 font-mono"
                      onChange={(event) => setSearchShortcutDraft(event.target.value)}
                      onKeyDown={handleSearchShortcutKeyDown}
                    />
                    <Button
                      type="button"
                      variant={isRecordingSearchShortcut ? "default" : "secondary"}
                      onClick={() => setIsRecordingSearchShortcut(true)}
                    >
                      {isRecordingSearchShortcut ? "请按组合键" : "录制搜索快捷键"}
                    </Button>
                  </div>
                  <p className="text-xs leading-5 text-muted-foreground">
                    默认 Ctrl+Space。如与输入法冲突，可以改成 Ctrl+Alt+Space、Alt+Space 等。
                  </p>
                </div>

                <div className="grid gap-2">
                  <div className="flex flex-wrap items-center justify-between gap-2">
                    <span className="text-sm font-medium">搜索路径</span>
                    <span className="text-xs text-muted-foreground">为空时默认使用收纳目录</span>
                  </div>
                  <div className="grid gap-2 md:grid-cols-[minmax(0,1fr)_auto]">
                    <Input
                      value={searchPathInput}
                      placeholder="粘贴一个目录路径，例如 D:\\Work"
                      className="h-10"
                      onChange={(event) => setSearchPathInput(event.target.value)}
                      onKeyDown={(event) => {
                        if (event.key === "Enter") {
                          event.preventDefault()
                          addSearchPath()
                        }
                      }}
                    />
                    <Button type="button" variant="secondary" onClick={() => void chooseSearchPath()}>
                      <Plus className="size-4" weight="bold" />
                      添加路径
                    </Button>
                  </div>
                  <div className="flex flex-wrap gap-2">
                    <Button type="button" size="sm" variant="outline" onClick={() => addSearchPath(snapshot.organizer_root)}>
                      加入收纳目录
                    </Button>
                    <Button type="button" size="sm" variant="outline" onClick={() => addSearchPath(snapshot.launchers_root)}>
                      加入启动目录
                    </Button>
                  </div>
                  <div className="grid max-h-36 gap-2 overflow-y-auto pr-1">
                    {effectiveSearchPaths.map((path, index) => (
                      <div key={`${path}-${index}`} className="flex items-center gap-2 rounded-lg border bg-muted/30 px-3 py-2 text-sm">
                        <Badge variant={safeSearchPathsDraft.length === 0 ? "secondary" : "outline"} className="shrink-0">
                          {safeSearchPathsDraft.length === 0 ? "默认" : String(index + 1).padStart(2, "0")}
                        </Badge>
                        <span className="min-w-0 flex-1 truncate" title={path}>
                          {path}
                        </span>
                        {safeSearchPathsDraft.length > 0 ? (
                          <Button type="button" size="icon-xs" variant="ghost" onClick={() => removeSearchPath(index)}>
                            <X className="size-3" weight="bold" />
                          </Button>
                        ) : null}
                      </div>
                    ))}
                  </div>
                  {searchSuccess ? <p className="text-xs leading-5 text-emerald-600 dark:text-emerald-400">{searchSuccess}</p> : null}
                  {searchError ? <p className="text-xs leading-5 text-destructive">{searchError}</p> : null}
                </div>

                <div className="flex flex-wrap gap-2">
                  <Button
                    type="button"
                    variant={searchEnabledDraft ? "secondary" : "default"}
                    disabled={isSavingSearch}
                    onClick={() => {
                      const nextEnabled = !searchEnabledDraft
                      setSearchEnabledDraft(nextEnabled)
                      void saveSearchSettings(nextEnabled, searchShortcutDraft, searchPathsDraft)
                    }}
                  >
                    {searchEnabledDraft ? "禁用搜索" : "启用搜索"}
                  </Button>
                  <Button type="button" disabled={isSavingSearch} onClick={() => void saveSearchSettings()}>
                    {isSavingSearch ? "保存中" : "保存搜索设置"}
                  </Button>
                  <Button
                    type="button"
                    variant="ghost"
                    onClick={() => {
                      setSearchEnabledDraft(snapshot.settings.search_enabled)
                      setSearchShortcutDraft(snapshot.settings.search_shortcut)
                      setSearchPathsDraft(snapshot.settings.search_paths)
                      setSearchPathInput("")
                      setSearchError("")
                      setSearchSuccess("")
                      setIsRecordingSearchShortcut(false)
                    }}
                  >
                    取消
                  </Button>
                </div>
              </CardContent>
            </Card>
          </div>

          <div className="mb-3 grid gap-3 xl:grid-cols-[minmax(0,1.05fr)_minmax(0,1.95fr)]">
            <Card>
              <CardContent className="flex min-h-44 flex-col gap-4 p-5">
                <div className="flex size-11 items-center justify-center rounded-lg bg-muted text-muted-foreground">
                  <PlayCircle className="size-5" weight="duotone" />
                </div>
                <div>
                  <h3 className="font-heading text-base font-medium">开机自启</h3>
                  <p className="mt-2 text-sm leading-6 text-muted-foreground">
                    登录 Windows 后自动启动 DeskNest，并继续显示桌面收纳卡片。
                  </p>
                </div>
                <Badge className="mt-auto w-fit" variant={snapshot.settings.launch_on_startup ? "default" : "outline"}>
                  {snapshot.settings.launch_on_startup ? "已启用" : "已关闭"}
                </Badge>
              </CardContent>
            </Card>

            <Card>
              <CardContent className="flex min-h-44 flex-col gap-4 p-5">
                <div className="grid gap-2">
                  <div className="flex flex-wrap items-center justify-between gap-3">
                    <h3 className="font-heading text-base font-medium">启动项设置</h3>
                    <Badge variant="outline">当前用户</Badge>
                  </div>
                  <p className="text-sm leading-6 text-muted-foreground">
                    使用 Windows 当前用户的 Startup 启动目录，不需要管理员权限。
                  </p>
                  {startupSuccess ? <p className="text-xs leading-5 text-emerald-600 dark:text-emerald-400">{startupSuccess}</p> : null}
                  {startupError ? <p className="text-xs leading-5 text-destructive">{startupError}</p> : null}
                </div>
                <div className="mt-auto flex flex-wrap gap-2">
                  <Button type="button" disabled={isSavingStartup} onClick={() => void toggleLaunchOnStartup()}>
                    {isSavingStartup ? "保存中" : snapshot.settings.launch_on_startup ? "关闭开机自启" : "开启开机自启"}
                  </Button>
                </div>
              </CardContent>
            </Card>
          </div>

          <div className="mb-3 grid gap-3 xl:grid-cols-[minmax(0,1.05fr)_minmax(0,1.95fr)]">
            <Card>
              <CardContent className="flex min-h-44 flex-col gap-4 p-5">
                <div className="flex size-11 items-center justify-center rounded-lg bg-muted text-muted-foreground">
                  <ArrowsClockwise className="size-5" weight="duotone" />
                </div>
                <div>
                  <h3 className="font-heading text-base font-medium">软件更新</h3>
                  <p className="mt-2 text-sm leading-6 text-muted-foreground">
                    检查 GitHub Release 上的最新安装包，有新版本时会提示下载更新。
                  </p>
                </div>
                <Badge className="mt-auto w-fit" variant={updateInfo?.update_available ? "default" : "outline"}>
                  {updateInfo ? `当前 ${updateInfo.current_version}` : "等待检查"}
                </Badge>
              </CardContent>
            </Card>

            <Card>
              <CardContent className="flex min-h-44 flex-col gap-4 p-5">
                <div className="grid gap-2">
                  <div className="flex flex-wrap items-center justify-between gap-3">
                    <h3 className="font-heading text-base font-medium">检查更新</h3>
                    <Badge variant={updateInfo?.update_available ? "default" : "outline"}>
                      {updateInfo?.update_available ? "有新版本" : "手动检查"}
                    </Badge>
                  </div>
                  <p className="text-sm leading-6 text-muted-foreground">
                    {updateInfo
                      ? `最新版本：${updateInfo.latest_version || "未知"}${updateInfo.asset_name ? `，安装包：${updateInfo.asset_name}` : ""}`
                      : "点击检查后会联网读取最新发布版本。"}
                  </p>
                  {updateInfo?.release_name ? <p className="text-xs leading-5 text-muted-foreground">发布：{updateInfo.release_name}</p> : null}
                  {updateSuccess ? <p className="text-xs leading-5 text-emerald-600 dark:text-emerald-400">{updateSuccess}</p> : null}
                  {updateError ? <p className="text-xs leading-5 text-destructive">{updateError}</p> : null}
                </div>
                <div className="mt-auto flex flex-wrap gap-2">
                  <Button type="button" disabled={isCheckingUpdate} onClick={() => void checkUpdateNow()}>
                    <ArrowsClockwise className="size-4" weight="bold" />
                    {isCheckingUpdate ? "检查中" : "检查更新"}
                  </Button>
                  {updateInfo?.update_available ? (
                    <Button type="button" variant="secondary" onClick={() => void downloadUpdate()}>
                      <DownloadSimple className="size-4" weight="bold" />
                      下载更新
                    </Button>
                  ) : null}
                </div>
              </CardContent>
            </Card>
          </div>

          <div className="grid gap-3 xl:grid-cols-4">
            {rows.map((row) => {
              const Icon = row.icon
              const directoryTarget: RuntimeDirectoryTarget | null = row.target === "desktop" ? null : row.target
              const isSavingDirectory = Boolean(directoryTarget && savingDirectoryTarget === directoryTarget)
              return (
                <Card key={row.name} className="min-w-0">
                  <CardContent className="flex min-h-44 min-w-0 flex-col gap-4 p-5">
                    <div className="flex items-start gap-3">
                      <div className="flex size-11 shrink-0 items-center justify-center rounded-lg bg-muted text-muted-foreground">
                        <Icon className="size-5" weight="duotone" />
                      </div>
                      <div className="min-w-0 flex-1">
                        <h3 className="font-heading text-base font-medium">{row.name}</h3>
                        <p className="mt-2 min-w-0 truncate font-mono text-[13px] leading-6 text-muted-foreground" title={row.value}>
                          {row.value || "等待读取"}
                        </p>
                      </div>
                    </div>
                    <div className="mt-auto grid gap-2">
                      <Button variant="secondary" size="sm" onClick={() => void openSpecial(row.target)}>
                        打开
                      </Button>
                      {directoryTarget ? (
                        <Button
                          variant="outline"
                          size="sm"
                          disabled={isSavingDirectory}
                          onClick={() => void chooseRuntimeDirectory(directoryTarget, row.value)}
                        >
                          {isSavingDirectory ? "修改中" : "修改目录"}
                        </Button>
                      ) : null}
                    </div>
                  </CardContent>
                </Card>
              )
            })}
          </div>
          {directorySuccess ? <p className="mt-3 text-xs leading-5 text-emerald-600 dark:text-emerald-400">{directorySuccess}</p> : null}
          {directoryError ? <p className="mt-3 text-xs leading-5 text-destructive">{directoryError}</p> : null}
        </ScrollArea>
      </CardContent>
    </Card>
  )
}

function runtimeDirectoryLabel(target: RuntimeDirectoryTarget) {
  if (target === "data") return "数据目录"
  if (target === "organizer") return "收纳目录"
  return "快捷启动目录"
}

function defaultSearchPaths(organizerRoot: string, paths: string[]) {
  if (paths.length > 0) return paths
  return organizerRoot.trim() ? [organizerRoot] : []
}

function normalizePathList(paths: string[]) {
  const seen = new Set<string>()
  const output: string[] = []
  for (const path of paths) {
    const trimmed = path.trim()
    const key = trimmed.toLowerCase()
    if (!trimmed || seen.has(key)) continue
    seen.add(key)
    output.push(trimmed)
  }
  return output
}
