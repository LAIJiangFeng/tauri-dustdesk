# 技术验证：隐藏 / 恢复系统桌面图标的可靠性与崩溃兜底

- 日期：2026-10-31
- 探测工具：`src-tauri/examples/desktop_icons_probe.rs`（独立 example，不改业务代码）
- 数据来源：三次完整的 `experiment-all`（各约 108 s），外加两次通过 `cargo run` 启动 `guard-demo` 的补充试验。第三次是代码整理后的复跑。三次结果一致，表中数值是三次的范围。

## 结论速览

1. **推荐 M1：只隐藏 `SysListView32`。** 同时配套四样东西：看门狗、启动时兜底恢复、列表窗口被重新显示时再隐藏、资源管理器重启后再隐藏。
   - M1 不改任何系统设置。最坏情况是 DustDesk 和看门狗同时被杀，此时资源管理器重启、注销或重启电脑后，图标预计会自然回来（待手动确认）。
   - M2 会在切换的同一次调用里把 `HideIcons=1` 写进注册表。崩溃且看门狗失效时，重启电脑也恢复不了；卸载 DustDesk 后同样恢复不了。
2. **所有自动触发条件下 M1 都保持隐藏。** 测过的条件：桌面文件新建、改名、删除；`SHCNE_ASSOCCHANGED`；`SHCNE_UPDATEDIR`；4 种 `WM_SETTINGCHANGE`；静置 30 秒。只有一种情况会撤销 M1：M2 从“隐藏”切回“显示”。这时 DefView 会在 12.4–15.7 ms 内同步把列表窗口重新显示出来。
3. **M2 唯一有效的切换方式是向 `SHELLDLL_DefView` 发 `WM_COMMAND 0x7402`。**
   - `SendMessageTimeout` 是同步的，约 12–31 ms 返回，返回时已全部生效。
   - `PostMessage` 是异步的，约 1.4–5.6 ms 后生效。
   - 发给 DefView 的父窗口 `Progman` 没有任何效果。
   - 切换后，注册表 `HideIcons` 和 `SHGetSetSettings(SSF_HIDEICONS)` 立即一致，说明设置被持久化了。
4. **看门狗兜底实测**（父进程被 `Stop-Process -Force` 结束，M1、M2 各 9 次）：
   - M1：平均 0.4 ms 恢复，最大 0.7 ms。
   - M2：平均 2.7 ms 恢复，最大 8.7 ms。
   - 下面两种情况图标都保持隐藏，只能靠启动兜底：看门狗先被结束；父进程所在的 kill-on-close job 被关闭、且看门狗没能脱离该 job。
5. **对现有代码的影响**：M1 下，`windows_show_desktop.rs` 的 Win+D 检测和 `LVM_SETITEMPOSITION` 恢复图标位置的代码都不受影响，不需要改。M2 下，隐藏期间读到的图标位置不可靠。

## 1. 环境

| 项 | 值 |
|---|---|
| Windows | Windows 11 25H2，build 26200.9711，家庭中文版（`EditionID=CoreCountrySpecific`）。注册表 `ProductName` 仍写作 “Windows 10 Home China”，判断系统版本要看 build 号。 |
| 显示器 | 1 台，2560×1600，缩放 150%（144 DPI），工作区 2560×1528；虚拟屏幕 (0,0) 2560×1600。探测进程按 per-monitor v2 DPI 感知运行，坐标都是物理像素。 |
| 桌面目录 | `FOLDERID_Desktop` = `C:\Users\15407\Desktop` |
| 桌面窗口层级 | 见下方说明。 |
| 桌面列表 | 2 个图标；`LVS_AUTOARRANGE=0`，`LVS_EX_SNAPTOGRID=1`，图标间距 115×147；`SysListView32` 的矩形覆盖整个虚拟屏幕。 |
| 桌面宿主 | `GetAncestor(listview, GA_ROOT)` = `Progman`，可见。 |
| HideIcons | 注册表值存在且为 0；`SHGetSetSettings` 中 `fHideIcons=0`。位域自检通过：`fShowExtensions=1`，与注册表 `HideFileExt=0` 相反。 |
| 进程环境 | 探测进程处在一个允许 breakaway 的 job 里（`in_job=yes`，`CREATE_BREAKAWAY_FROM_JOB` 被接受）。用 `cargo run` 启动时，cargo 的 job 拒绝 breakaway。 |
| Explorer | pid 7440。实验全程 HWND 不变；WinEvent 监视没有看到新建桌面 DefView 或 ListView。 |

**桌面窗口层级**是 24H2 起的新布局：

```text
Progman 0x1019C（可见，"Program Manager"）
├─ SHELLDLL_DefView 0x101A0（可见）
│  └─ SysListView32 "FolderView" 0x101A2（可见）
│     └─ SysHeader32（隐藏）
├─ mpv（动态壁纸软件嵌入的子窗口）
└─ WorkerW 0x302E8（可见，壁纸层）
另有 14 个顶层 WorkerW：全部隐藏，都不含 DefView。
```

因此 `main.rs` 的 `desktop_listview_window()` 走第一条分支（Progman）就能找到列表窗口。

## 2. 探测工具与实验方法

### 2.1 命令

先构建一次：

```powershell
cargo build --manifest-path src-tauri/Cargo.toml --example desktop_icons_probe
```

之后可以用 `cargo run --manifest-path src-tauri/Cargo.toml --example desktop_icons_probe -- <命令>`，也可以直接运行 `src-tauri\target\debug\examples\desktop_icons_probe.exe <命令>`。

| 命令 | 作用 |
|---|---|
| `status` | 打印 Windows 版本 / build、Progman / WorkerW / DefView / ListView 的 HWND、可见性和矩形、显示器、HideIcons（注册表 + API）、列表与宿主是否可见。如果找到上次 `experiment-all` 记录的初始状态，会输出 `CURRENT_EQUALS_BASELINE`。 |
| `hide-window` / `show-window [showna\|show\|async]` | M1 隐藏 / 恢复。恢复默认用 `SW_SHOWNA`。如果 `HideIcons=1`，`show-window` 拒绝动手，避免出现“设置说隐藏、窗口却可见”的错位状态。 |
| `set-hide-icons on\|off [send\|post\|parent]` | M2 切换。先读状态，只在需要时发一次 `0x7402`，不会自动重发。 |
| `restore-all` | 一键恢复：先 `HideIcons → 0`，再显示列表窗口。 |
| `guard-demo m1\|m2 [--max-secs N] [--log PATH]` | 先武装看门狗，再隐藏图标，然后等着被结束。默认 300 s 后自行恢复；`--max-secs 0` 表示不自行恢复。 |
| `watchdog …` | 内部命令，由 `guard-demo` 和 `experiment-all` 启动。 |
| `experiment-all` | 跑完整实验矩阵，打印 Markdown 结果表，结束时恢复初始状态。 |

### 2.2 安全措施

- **每个实验前后**：实验前记录初始状态。实验后用 `restore_to` 恢复并核对三项：列表窗口可见性、注册表 `HideIcons` 的精确值（初始不存在就删除）、`SHGetSetSettings`。400 ms 后再核对一次，捕捉迟到的变化。只要恢复失败，就终止全部实验。
- **M2 的切换语义**：先读状态，只在不一致时发一次。最多等 3 s 确认；只有“完全没有变化”时才允许再试一次，绝不盲目重发。
- **兜底层层叠加**：
  - `experiment-all` 自己也挂一个安全看门狗。它与 `guard-demo` 用同一种看门狗：同一个 exe，`DETACHED_PROCESS`，独立进程组，尽量 `CREATE_BREAKAWAY_FROM_JOB`。
  - 控制台 Ctrl+C / 关窗口时，`SetConsoleCtrlHandler` 会先拿到变更锁，恢复图标、删除测试文件，然后退出。
  - 发生 panic 时，`catch_unwind` 捕获后仍执行最后一次显式恢复。
- **桌面测试文件**：只使用 `dustdesk-probe-<pid>-<序号>.txt`。创建用 `create_new`，改名、删除前校验前缀和目录。结束时按登记表和目录前缀两种方式清理。
- **观测手段**：
  - 用 20 ms 间隔轮询 `IsWindowVisible`；静置实验用 100 ms。
  - 同时用 `SetWinEventHook(EVENT_OBJECT_CREATE..EVENT_OBJECT_HIDE, WINEVENT_OUTOFCONTEXT)` 只监听 Explorer 进程，记录列表窗口的 SHOW / HIDE 和新建的桌面 DefView / ListView。这样轮询漏掉的瞬时变化也能看到。没有注入，也没有键盘 / 鼠标钩子。
- **崩溃兜底的测量方法**：
  - 实验 runner 启动 `guard-demo`，等它输出 `@@ready`，即看门狗已武装、图标已隐藏。
  - 然后用 `powershell Stop-Process -Id <pid> -Force` 结束它。
  - 起点：runner 观察到 guard-demo 进程句柄 signaled 的时刻。终点：runner 忙等直到图标恢复。
  - 另外从看门狗日志读取“醒来 → 恢复并复核完成”的耗时。
- **遵守的禁止项**：没有重启或结束 explorer.exe，没有模拟输入或 Win+D，没有广播消息（`WM_SETTINGCHANGE` 只发给 `Progman`），没有改壁纸或显示设置，没有碰 `%APPDATA%\DustDesk`。

## 3. 结果

### 3.1 基本操作与 M2 的调用方式

| 操作 | 结果（三次的范围） |
|---|---|
| M1 隐藏：`ShowWindow(SysListView32, SW_HIDE)` | 调用耗时 0.6–2.5 ms，返回时已隐藏。列表 `IsWindowVisible=0`、`WS_VISIBLE=0`；DefView 和 Progman 仍可见；HWND 不变；`HideIcons` 不变；`LVM_GETITEMCOUNT` 照常可用；前台窗口不变。 |
| M1 恢复：`SW_SHOWNA` / `SW_SHOW` / `ShowWindowAsync(SW_SHOWNA)` | 三种方式都在 1.0–1.8 ms 内恢复可见，都不改变前台窗口。`ShowWindowAsync` 对其它进程的子窗口同样有效，而且调用本身不等待 Explorer。 |
| M2 关：`SendMessageTimeout(DefView, WM_COMMAND, 0x7402)` | 调用在 14.2–17.8 ms 后返回。返回时三项已经同时生效：列表隐藏、注册表 `HideIcons=1`、`SHGetSetSettings` 为 1。窗口状态与 M1 完全相同：DefView 和 Progman 可见，HWND 不变，前台窗口不变。 |
| M2 开（同一方式） | 13.8–17.1 ms 后返回，返回时列表可见、`HideIcons=0`。 |
| M2：`PostMessage(DefView, …)` | 也有效，但是异步的。关：1.4–3.5 ms 后列表隐藏，1.4–5.6 ms 后注册表变为 1。开：1.5–2.1 ms 后列表可见，1.5–5.0 ms 后注册表变为 0。 |
| M2：发给 DefView 的父窗口 `Progman` | **无效**。3 s 内设置和列表都没变，也没有任何 WinEvent。 |
| 读取方式比较 | 注册表 `HideIcons`、本进程的 `SHGetSetSettings(SSF_HIDEICONS)`、`IsWindowVisible(listview)`、`WS_VISIBLE`，在每次切换后的同一轮询周期内同时反映新值，`SHGetSetSettings` 没有观察到缓存滞后。 |

**未测试 `SHGetSetSettings(bSet=TRUE)`。** 写入共享设置后会不会通知、以及怎样通知正在运行的桌面 DefView，没有验证；它也可能引发全局设置变更通知，超出本次“不广播”的约束。因此 M2 只考虑 `0x7402` 这一条路径。

### 3.2 M1 隐藏后的各种触发条件

| 实验 | 触发条件 | 结果 | 补充观察 |
|---|---|---|---|
| 1a | 在桌面新建 `dustdesk-probe-*` 文件 | 保持隐藏（观察 2.5 s） | 隐藏中的列表照样收到新项：1148.8–1167.5 ms |
| 1a | 改名 | 保持隐藏 | 新名字在 1146.8–1169.4 ms 后出现 |
| 1a | 删除 | 保持隐藏 | 1155.3–1156.9 ms 后从列表中移除 |
| 1b | `SHChangeNotify(SHCNE_ASSOCCHANGED)` | 保持隐藏（观察 3 s） | — |
| 1c | `SHChangeNotify(SHCNE_UPDATEDIR, 桌面路径)` | 保持隐藏（观察 3 s） | — |
| 1d | 静置 30 s | 保持隐藏（30 s，100 ms 轮询） | WinEvent 只记录到最初那次 HIDE |
| 1e | M1 隐藏期间，M2 先关再切回原值 | **M1 被撤销**：M2 切回“显示”时，Explorer 在 12.4–15.7 ms 内重新显示了列表窗口 | 见下方说明 |
| 1f（追加） | `WM_SETTINGCHANGE`，`lParam` 分别为 `NULL`、`"Policy"`、`"ImmersiveColorSet"`、`"ShellState"`，只发给 Progman | 4 种都保持隐藏（各观察 1.5 s） | 模拟常见的系统设置变化通知，但不是广播 |

关于 1e：在 M1 隐藏状态下切 M2，`HideIcons` 照常变成 1。这说明 DefView 按自己记录的状态做切换，不看列表窗口是否可见。切回“显示”时，DefView 会主动 `ShowWindow` 列表窗口，所以 M1 的隐藏被覆盖。

### 3.3 M2 关闭桌面图标后的各种触发条件

| 实验 | 触发条件 | 结果 | 补充观察 |
|---|---|---|---|
| 2a | 新建 / 改名 / 删除 `dustdesk-probe-*` 文件 | 都保持隐藏；`HideIcons` 保持 1；`SHGetSetSettings=1` | 隐藏中的列表在 151.5–184.8 ms 内反映变化 |
| 2b | `SHCNE_ASSOCCHANGED` | 保持隐藏 | — |
| 2c | `SHCNE_UPDATEDIR(桌面)` | 保持隐藏 | — |
| 2f（追加） | `WM_SETTINGCHANGE(lParam=NULL)` 发给 Progman | 保持隐藏 | — |

每次切回“显示”后，列表在 12.0–30.9 ms 内可见。

M1 隐藏时，列表大约 1.0–1.2 s 后才反映文件变化，与可见时一样（可见状态下删除测试文件约 1.0–1.1 s 后反映）；M2 隐藏时只需约 0.17 s。这个差异不影响结论。

### 3.4 崩溃兜底实测

**说明**：

- **恢复耗时**指父进程死亡到图标恢复的时间，起止点见 2.2 节。M1 的“恢复”指列表窗口可见；M2 还要求注册表 `HideIcons` 已回到 0。
- **看门狗内部耗时**来自看门狗日志，指它从醒来到恢复并复核完成的时间。
- `Stop-Process` 本身的耗时（从启动 Windows PowerShell 到目标进程退出，约 240–650 ms）**不计入**上述两个耗时。

| 场景 | 次数 | 结果 | 恢复耗时 | 看门狗内部耗时 |
|---|---|---|---|---|
| M1：`Stop-Process -Force` 结束 guard-demo，看门狗存活 | 9 | 全部自动恢复 | 0.2–0.7 ms，平均 0.4 ms | 1.6–3.4 ms，平均 2.3 ms |
| M2：同上 | 9 | 全部自动恢复 | 1.3–8.7 ms，平均 2.7 ms | 8.7–17.5 ms，平均 13.8 ms |
| M1：先结束看门狗，再结束 guard-demo | 3 | **不恢复**，3 s 后仍隐藏，最后由实验 runner 恢复 | — | — |
| M2：同上 | 3 | **不恢复**，且 `HideIcons=1` 已写入注册表；按注册表持久化推断，这种状态会跨资源管理器重启和电脑重启保留 | — | — |
| 补充：`cargo run … -- guard-demo m1` 后结束 guard-demo | 1 | 自动恢复 | — | 2.7 ms |
| 补充：同上，但结束的是 `cargo` 进程本身 | 1 | **不恢复**，guard-demo 与看门狗同时被杀；手动 `restore-all` 后恢复 | — | — |

补充说明：

- **M2 的看门狗内部耗时更长**，主要是 `SendMessageTimeout` 在等 DefView 处理完消息。列表可见、注册表归 0 发生在处理过程的前段，所以从外部看几毫秒内就已恢复。
- **先结束看门狗的试验**：guard-demo 在自身轮询周期（100 ms）内就察觉到看门狗退出，并输出 `@@watchdog_exited`。这说明主进程可以及时发现看门狗消失，并重新拉起一个。
- **结束 guard-demo 的那次**：cargo 的 job 拒绝 breakaway，看门狗只能留在 cargo 的 job 里。但 cargo 是正常退出的；按 cargo 的实现，正常退出前会撤销该 job 的“关闭即杀进程”设置，所以看门狗存活并完成了恢复。
- **结束 cargo 本身的那次**：job 被关闭，触发“关闭即杀进程”，guard-demo 和看门狗同时被杀，看门狗日志里只有 `armed` 一行。

### 3.5 隐藏状态下的 LVM_*（对应 E4）

做法：在可见状态下先创建一个测试文件，让它出现在桌面列表里；再进入隐藏状态，读写它的位置；然后恢复显示，最后删除文件。

| 检查项 | M1 | M2 |
|---|---|---|
| `LVM_GETITEMCOUNT` / `LVM_GETITEMTEXTW` | 正常，共 3 项（含测试文件） | 正常，共 3 项 |
| SET 之前的 `LVM_GETITEMPOSITION` | 与可见时一致，为 (21,2) | 与可见时一致，为 (21,2) |
| `LVM_SETITEMPOSITION32` 到 (136,2) 后，隐藏期间回读 | +0 / +300 / +1000 ms 都是 (136,2) | +0 ms 为 (136,2)；从 +300 ms 起变成 (136,1325)，即同一列最底部的网格行（2+9×147） |
| 恢复显示后读取 | (136,2) | (136,2) |

第 1 次实验只在 SET 后 200 ms 回读一次：M1 为 (136,2)，M2 为 (136,1325)。第 2、3 次按表中三个时间点回读，结果相同。

结论：

- M1 下，隐藏期间读写图标位置完全可靠。
- M2 下，SET 最终是生效的，但隐藏期间 Explorer 会把位置临时重排，所以这段时间读到的位置不可信。

### 3.6 资源管理器重启和电脑重启后的持久性

- **M2**：在 `SendMessageTimeout` 返回时，注册表 `HideIcons=1` 就已经写好，也就是同步持久化。3.4 节“先结束看门狗”的 M2 试验证实：没有看门狗时，图标保持隐藏，注册表也保持为 1。由此推断，资源管理器重启后桌面直接以隐藏状态启动，重启电脑同样如此（推断依据是注册表持久化，本次没有实际重启）。这正是崩溃后“连重启电脑都恢复不了”的风险。卸载 DustDesk 后，这个设置也会一直留着。
- **M1**：全程不改注册表（`HideIcons` 始终为 0），只改变当前 Explorer 实例里一个子窗口的可见性。资源管理器重启会重建 Progman / DefView / ListView，新窗口默认可见，预期图标会回来；重启电脑同理。**这一点需要用户手动确认**，因为本次禁止重启资源管理器，测法见第 7 节。

## 4. 对现有代码的影响

### 4.1 `windows_show_desktop.rs`（Win+D 检测）

这个文件中与桌面窗口有关的逻辑如下：

- `desktop_icons_host_window()`（第 293 行）通过 `crate::desktop_listview_window()` 找到列表窗口，再取 `GA_ROOT` 得到宿主。
- `is_sentinel_below_desktop_host()`（第 303 行）先要求 `IsWindowVisible(desktop_host)` 为真，再沿 z 序往下找哨兵窗口。
- `configure_desktop_card_window()`（第 134 行）、`is_external_app_foreground()`（第 272 行）以及 z 序调整，都只使用宿主窗口。

在两种隐藏方式下实测：

- `FindWindowExW` 不过滤可见性，隐藏期间找到的仍是同一个列表 HWND（0x101A2），宿主仍是 `Progman`（0x1019C）。
- 宿主的 `host_visible` 始终为 1。M1 与 M2 都只隐藏 `SysListView32` 这个子窗口，DefView 和 Progman 都保持可见。
- 隐藏子窗口不会改变任何顶层窗口的 z 序，所以哨兵相对宿主的位置判断不受影响。
- 所有 M1 / M2 操作都没有改变前台窗口（`foreground unchanged`），`is_external_app_foreground` 不会被误触发。

**结论：Win+D 检测不会被破坏，`windows_show_desktop.rs` 不需要修改。** 资源管理器重启会更换 HWND，现有代码在 `desktop_host_changed` 时已经会刷新层级，与隐藏方式无关。Win+D 期间图标是否仍保持隐藏，属于 Explorer 自身的行为，放进手动测试清单第 3 项确认。

### 4.2 `LVM_SETITEMPOSITION` 相关代码（`main.rs`）

涉及的函数：

| 函数 | 位置 |
|---|---|
| `capture_desktop_icon_positions` | 第 4740 行 |
| `position_desktop_icons` | 第 4787 行 |
| `position_visible_desktop_icons_once` | 第 4833 行 |
| `desktop_listview_item_texts` | 第 4995 行 |
| `desktop_listview_get_item_position` | 第 5123 行 |
| `desktop_listview_set_item_position` | 第 5233 行 |

这些函数的共同点：都通过 `desktop_listview_window()` 找窗口；都用跨进程的 `VirtualAllocEx`、`ReadProcessMemory` / `WriteProcessMemory` 和 `SendMessageW` 发 `LVM_*`；坐标用 `ClientToScreen` / `ScreenToClient` 换算。这些调用都不依赖窗口是否可见：隐藏窗口的矩形不变，坐标换算照常。

**M1 下逐项实测（3.5 节）：**

- 枚举项目文本可用。
- 读取位置可用。
- 设置位置立即生效，且保持稳定。
- 重新显示后位置保持不变。

**另外两点：**

- 隐藏中的列表约 1.2 s 内就会反映桌面上的文件变化，在 `position_desktop_icons` 的重试窗口（100 次 × 100 ms）之内，所以“等新图标出现再定位”的流程照常工作。
- 结论：**M1 下现有代码可以原样工作。**

**M2 下：**

- 隐藏期间设置位置是有效的，最终位置正确。
- 但 Explorer 会临时重排，导致隐藏期间读到的位置不可信。如果在 M2 隐藏期间调用 `capture_desktop_icon_positions`（例如收纳时记录原位置），可能记录到错误坐标。这是 M2 的又一个缺点。

“文件不动”方案下，项目不再在桌面和收纳目录之间来回移动，这组位置读写代码的使用频率会明显下降。不过“移回桌面”和“一键还原”仍然需要它们。

## 5. 推荐方案

### 5.1 选择 M1

| 对比项 | M1 只隐藏列表窗口 | M2 系统“显示桌面图标”设置 |
|---|---|---|
| 是否改用户设置 | 否 | 是，同步写入 `HideIcons`，右键菜单里的勾选状态也随之改变 |
| 崩溃且看门狗也失效 | 资源管理器重启、注销或重启电脑后自然恢复（待手动确认） | 一直隐藏，跨重启电脑保留；卸载 DustDesk 后也不会自动恢复 |
| 自动触发条件下的稳定性 | 全部保持隐藏；只会被“M2 切回显示”撤销 | 全部保持隐藏 |
| 隐藏期间读写图标位置 | 完全可靠 | 读到的位置会被临时重排 |
| 实现复杂度 | 一个 `ShowWindow` 调用，外加“被重新显示时再隐藏”的监视 | 切换语义，必须先读后切，并且要处理和用户手动切换之间的竞争 |
| 兜底恢复耗时 | 0.2–0.7 ms | 1.3–8.7 ms |

### 5.2 M1 的落地要点

1. **隐藏**：只对桌面 `SysListView32` 调用 `ShowWindow(SW_HIDE)`。
2. **恢复**：调用 `ShowWindow(SW_SHOWNA)`。在 UI 线程等不能被阻塞的路径上，改用 `ShowWindowAsync(SW_SHOWNA)`，这样 Explorer 挂起时也不会卡住调用方；两种方式都实测有效，也都不抢焦点。
3. **每次操作前重新查找窗口**：沿用 `Progman → SHELLDLL_DefView → SysListView32` 的查找逻辑，不要缓存 HWND。
4. **被重新显示时再隐藏**：用 `SetWinEventHook(EVENT_OBJECT_SHOW, …, explorer_pid, 0, WINEVENT_OUTOFCONTEXT)` 监听列表窗口。收到 SHOW 后，投递给工作线程去重新隐藏，并做短暂去抖。按 `AGENTS.md` 的约定，回调里不直接做窗口操作。本次已实测：Explorer 进程里列表窗口的 SHOW / HIDE 事件，在进程外钩子中都能收到。

   需要再隐藏的场景包括：用户在右键菜单里把“显示桌面图标”取消后又勾上（实验 1e），以及待手动确认的 F5、显示设置变化等。

5. **资源管理器重启**：用一个隐藏的顶层窗口监听 `RegisterWindowMessageW("TaskbarCreated")`。收到后，重新查找列表窗口并隐藏，同时按新的 Explorer pid 重新注册 WinEvent 钩子。
6. **右键菜单状态**：M1 隐藏期间，右键菜单里“显示桌面图标”仍是勾选状态。如果用户点它，就变成 M2 隐藏；再点一次，则会撤销 M1。产品上要决定：把这个动作当作“用户想看系统图标”，还是由第 4 点的监视立即再隐藏。

### 5.3 看门狗设计（已在 probe 中实现并实测）

1. **启动方式**：
   - 用同一个 exe 加内部参数启动看门狗，创建标志为 `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB`。
   - 如果因 `ERROR_ACCESS_DENIED` 失败，就去掉 breakaway 重试。
   - 标准输入输出全部为空。
   - 脱离控制台后，Ctrl+C 或关闭控制台窗口不会连带结束看门狗；breakaway 可以避免它随父进程所在的 job 一起被杀。
2. **先武装再隐藏**：父进程创建一个命名事件，看门狗打开父进程的 `SYNCHRONIZE` 句柄后再置位这个事件；父进程等到事件置位才隐藏图标。实测武装耗时 11–18 ms。正式实现还应防 PID 复用：要么同时传父进程创建时间并校验，要么改用可继承的进程句柄。
3. **等待**：用 `WaitForSingleObject(parent, INFINITE)` 等父进程退出，不轮询、不占 CPU。在等待之前先预热一次（查找窗口、加载 shell32），可以把醒来后的恢复压到 2–3 ms。
4. **恢复要幂等**：
   - 每次重新读取当前状态，只把仍需恢复的部分改回原值。
   - M1：显示当前找到的列表窗口。
   - M2（如果仍要支持）：先读 `HideIcons`，只在仍为隐藏时切换一次，并复核结果；绝不盲目重发。
   - 父进程正常退出时，应先自己恢复图标，再让看门狗退出。看门狗如果醒来发现无需动作，直接退出即可；probe 的安全看门狗就是这样做的。
5. **互相监护**：主进程同时等待看门狗的句柄，一旦看门狗退出就重新拉起。实测 guard-demo 在 100 ms 轮询周期内就能察觉看门狗退出。
6. **启动兜底**：
   - 隐藏之前，在 DustDesk 自己的数据目录里原子写入一个“图标已被 DustDesk 隐藏”的标记，记录隐藏方式和原始 `HideIcons` 值。
   - 下次启动时，如果标记存在，先恢复图标，再按需要重新隐藏。
   - 这个标记覆盖的是“DustDesk 和看门狗同时死亡、而 Explorer 仍在运行”的情况。对 M1 而言，资源管理器重启或电脑重启之后，标记只起记录作用；对 M2 而言，这是唯一的自愈手段。
7. **卸载**：如果将来支持 M2，卸载程序必须把 `HideIcons` 改回原值。M1 不需要处理。

## 6. 剩余风险

| 风险 | 影响与对策 |
|---|---|
| 父进程所在的 job 设置了“关闭即杀进程”，且不允许 breakaway | 看门狗会和 DustDesk 一起被杀。本次已用 cargo 的 job 复现：结束 cargo 后图标保持隐藏。对策：能 breakaway 就 breakaway；不行就依靠启动兜底，M1 还可以靠资源管理器重启或电脑重启恢复。 |
| 任务管理器“结束进程树”、断电、蓝屏 | 看门狗会一起消失。M1 在资源管理器重启或电脑重启后恢复；M2 会持久保持隐藏。如需更强保护，可以让看门狗由一个随即退出的中间进程拉起，不再挂在 DustDesk 的进程树下（未测）。 |
| Explorer 挂起 | 跨进程的 `ShowWindow` 会一直等到 Explorer 处理完。不能被阻塞的路径改用 `ShowWindowAsync`。 |
| M1 被“显示桌面图标”的再次应用撤销（实验 1e） | 依靠 5.2 节第 4 点的 WinEvent 监视再隐藏。此外右键菜单的勾选状态与实际不一致，需要产品层面决定怎么处理。 |
| 仅能手动验证的场景 | F5、Win+D、资源管理器重启、睡眠唤醒、锁屏解锁、缩放 / 分辨率变化、双显示器、右键菜单、往隐藏的桌面拖放文件、动态壁纸显示，见第 7 节。 |
| 双显示器 | 本机只有 1 台显示器。列表窗口覆盖整个虚拟屏幕，按理一个窗口会同时隐藏所有屏幕上的图标，需要手动确认。 |
| M2 隐藏期间读取图标位置 | 读到的位置会被临时重排（3.5 节）。如果使用 M2，不要在隐藏期间读取位置。 |
| probe 里看门狗的 PID 复用 | 理论风险。正式实现按 5.3 节第 2 点加上进程创建时间校验，或改用句柄继承。 |

## 7. 给用户的手动测试清单

这些项目自动执行会打扰正在使用电脑的用户，所以留给你手动测。

### 7.1 准备

在仓库根目录打开 PowerShell：

```powershell
cargo build --manifest-path src-tauri/Cargo.toml --example desktop_icons_probe
$probe = "src-tauri\target\debug\examples\desktop_icons_probe.exe"
& $probe status        # 确认 ICONS_VISIBLE=yes，记下 STATE 行
```

### 7.2 一键全部恢复（以防万一）

```powershell
& $probe restore-all
# 等价于（顺序不能反）：
& $probe set-hide-icons off
& $probe show-window
# 还没构建时：
cargo run --manifest-path src-tauri/Cargo.toml --example desktop_icons_probe -- restore-all
```

如果工具也用不了：

- M2：右键桌面 → 查看 → 勾选“显示桌面图标”。
- M1：任务管理器 → “Windows 资源管理器” → 重新启动。

### 7.3 测试流程

每一项都对 M1、M2 各做一遍：

1. 隐藏：
   - M1：`& $probe hide-window`
   - M2：`& $probe set-hide-icons on`
2. 执行下表中的操作。
3. 运行 `& $probe status`，记录 `STATE listview_visible=… hide_icons_reg=…`，并记录肉眼看到的现象。
4. 恢复：
   - M1：`& $probe show-window`
   - M2：`& $probe set-hide-icons off`

如果某一步之后图标自己回来了，这本身就是要记录的结果。

### 7.4 测试项目

| # | 操作 | 观察 / 记录 | 预期（待确认） |
|---|---|---|---|
| 1 | 右键桌面空白处 | 是否还能弹出桌面右键菜单（查看 / 排序方式 / 刷新 / 新建 / 显示设置 / 个性化） | 预计两种方式相同（隐藏后点击都落在 DefView 上） |
| 2 | 先在资源管理器的任意文件夹里新建一个 `dustdesk-manual-test.txt`，再把它拖到桌面空白处 | 光标是否显示“禁止”；松手后在资源管理器里检查 `C:\Users\15407\Desktop` 是否多了这个文件。**测完把它删掉** | 未知 |
| 3 | 按 Win+D 显示桌面，再按 Win+D 恢复 | 两次操作后各运行一次 `status`；图标是否一直保持隐藏 | 保持隐藏 |
| 4 | 接上第二台显示器后，先运行 `status`，确认 `Monitors: 2`，并且 ListView 的矩形等于虚拟屏幕；再隐藏 | 两块屏幕上的图标是否都隐藏 | 都隐藏 |
| 5 | 任务管理器 → “Windows 资源管理器” → 重新启动 | 图标是否回来；`status` 中列表窗口的 HWND 应该已变化 | M1：回来；M2：仍隐藏，需要 `set-hide-icons off` |
| 6a | 睡眠后唤醒 | 唤醒后运行 `status` | 未知 |
| 6b | Win+L 锁屏后解锁 | 解锁后运行 `status` | 未知 |
| 6c | 设置 → 显示 → 修改缩放或分辨率，再改回原值 | 每次修改后运行 `status` | 未知 |
| 7 | 先单击桌面空白处，再按 F5 | 运行 `status` | 未知（M1 最可能被撤销的场景之一） |
| 8（可选） | 右键 → 查看 → 更换图标大小 / 切换“自动排列图标”，然后改回原样；切换一次深色 / 浅色模式；观察动态壁纸 | 运行 `status`；动态壁纸是否仍正常显示 | 未知 |
| 9（可选，仅 M2） | M2 隐藏后重启电脑 | 登录后运行 `status`；**测完立即 `restore-all`** | 仍隐藏（注册表持久化） |

### 7.5 崩溃兜底手动演示

```powershell
& $probe guard-demo m1                 # 输出里有 pid 和看门狗日志路径；此时图标已隐藏
# 在另一个 PowerShell 窗口中：
Stop-Process -Id <guard-demo 的 pid> -Force   # 图标应几乎立刻恢复；日志里有 RESTORED_MS
```

把 `m1` 换成 `m2`，即可测试 M2。

请直接运行 exe，不要用 `cargo run` 跑 `guard-demo`。cargo 的 job 拒绝 breakaway，一旦 cargo 被结束（3.4 节已复现）或它所在的终端窗口被关闭（推断），看门狗会被一起杀掉。

## 8. 最终状态核对

三次 `experiment-all` 结束时都输出了：

```text
INITIAL listview_visible=1 hide_icons_reg=0 hide_icons_api=0
FINAL   listview_visible=1 hide_icons_reg=0 hide_icons_api=0
FINAL_EQUALS_INITIAL=yes
```

收尾时检查过：桌面上没有残留的 `dustdesk-probe-*` 文件，也没有残留的 probe 或看门狗进程。

全部实验结束后，`status` 的输出摘录如下：

```text
HideIcons: registry HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced\HideIcons = 0 | SHGetSetSettings(SSF_HIDEICONS).fHideIcons = 0
List view IsWindowVisible=1 WS_VISIBLE=1 | desktop host IsWindowVisible=1
STATE listview_visible=1 hide_icons_reg=0 hide_icons_api=0
ICONS_VISIBLE=yes
Baseline recorded at the start of the last experiment-all: listview_visible=1 hide_icons_reg=0 hide_icons_api=0
CURRENT_EQUALS_BASELINE=yes
```

结论：最终状态等于初始状态，即图标可见，`HideIcons` 保持原值 0。
