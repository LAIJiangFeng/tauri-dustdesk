# DustDesk 改为"文件不动"收纳（方案二）实施方案

> 状态：草案，待审阅。本文只描述方案，不包含代码改动。
>
> 依据：分支 `fynvra/dustdesk` 提交 `c9d4474`（0.1.13，已包含"同盘移动直接改名"和"收纳目录默认放到桌面所在磁盘"），技术验证见 `docs/spikes/desktop-icon-hiding.md`（下文简称 spike）。
>
> 行号：`main.rs` 共 15,491 行，文中 "main.rs:875" 之类指该提交里函数或语句的大致起始位置。标"待确认"的内容在代码和已有文档里找不到依据，需要验证后再定。

## 0. 方案速览

**一句话**：文件永远留在桌面（用户桌面 + 公共桌面），DustDesk 只在 `config.json` 里记录"哪个文件属于哪个分类"，并用卡片显示；运行期间只隐藏系统桌面图标列表（M1），退出或关闭开关即恢复。

**改动范围**

| 层 | 文件 | 改什么 |
|---|---|---|
| 后端 | `main.rs` | 加入分类、一键整理、卡片间移动、移出分类、拖出改为只改配置；快照修复改为以桌面为准；启动和退出不再搬文件；新增"未分类"计算和外部文件导入 |
| 后端 | `desktop_item_actions.rs` | 删除、重命名、剪切同时支持未分类项；重命名不再迁移图标位置记录 |
| 后端 | `file_dialog_assist.rs` | 快捷项、默认列表、搜索根从收纳目录改为桌面和分类 |
| 后端 | `models.rs`、`store.rs` | 模式标记、文件 ID、未分类槽位；不再创建收纳目录 |
| 后端（新） | 建议新建 `desktop_watch.rs`、`in_place_migration.rs` | 桌面变化监听与改名跟踪；旧数据一次性迁移 |
| 后端（T1，进行中） | `desktop_icon_visibility.rs` | 系统图标隐藏/恢复、看门狗、兜底 |
| 前端 | `dustdesk-store.ts`、`desktop-widget-page.tsx`、`desktop-card-window-page.tsx`、`organizer-page.tsx`、`settings-page.tsx`、`file-dialog-assist-page.tsx`、`home-page.tsx`、`types.ts` | 未分类卡片、新语义和文案 |
| 文档 | `AGENTS.md`、`README.md` | 规则重写（§12） |

**主要收益与代价**

| 收益 | 代价 |
|---|---|
| 加入分类、一键整理、卡片间移动、移出分类都只写配置，毫秒级完成，不再有跨盘复制 | 依赖隐藏系统图标；M1 有残余风险（§13），需要 T1 的看门狗和兜底 |
| 退出只恢复图标，启动只校准配置，不再"退出时整体还原、启动时整体收回" | 需要新做桌面监听、改名跟踪、未分类卡片、一次性迁移 |
| 文件始终在桌面：崩溃、强杀、卸载都不会把文件困在收纳目录；在资源管理器里打开 Desktop 文件夹就能看到全部文件 | DustDesk 未运行时被外部改名、且文件 ID 也对不上的项目会失去归类（文件不丢，落入未分类） |
| 公共桌面项目也能归类（现在移动它们需要管理员权限，见 main.rs:7228 的报错文案） | 桌面目录里的文件数量不再减少；不支持降级回旧版本（§13） |
| T2–T7 期间可删约 1,900 行生产代码和约 28 个测试；迁移窗口结束后再删约 800 行（§11） | |

**分几步**（§14）

1. T1 系统图标隐藏/恢复基础模块（进行中，默认关闭，可先合入 main）
2. T2 数据模型与操作语义（后端）
3. T3 桌面变化监听与改名跟踪
4. T4 未分类卡片与界面（前后端）
5. T5 旧数据一次性迁移
6. T6 文件对话框助手与搜索适配
7. T7 退役清理，重写 AGENTS.md 和 README
8. T8 迁移窗口结束后，删除迁移专用代码（transfer journal 等）

T2–T6 在集成分支上完成后一起发版。只发布 T2 会让老用户的配置被新语义误读。

**待决问题**（详见 §15）：①未分类用卡片还是全屏自绘；②外部拖入是移动还是复制；③"一键还原"的新含义；④收纳目录是否保留；⑤用户切换"显示桌面图标"时暂停还是立即再隐藏；⑥是否保留旧的物理收纳模式；⑦迁移后是否默认隐藏系统图标；⑧公共桌面项目的删除/重命名是否接入系统提权；⑨未分类卡片能否单独隐藏；⑩拖出卡片时是否把图标摆到落点；⑪迁移代码保留多久；⑫右键"新建"后是否在卡片里自动进入改名。

## 1. 背景与目标

### 1.1 现状

| 环节 | 现在的做法 | 代码 | 问题 |
|---|---|---|---|
| 加入分类 | 把文件物理移动到 `DesktopOrganizer\<分类名>` | `add_items_to_category_impl` main.rs:875 → `archive_item_path` 6714 → `move_path` 7140 | 跨盘时要复制、校验、删源，大目录很慢 |
| 一键整理 | 扫描桌面，按规则逐项移动，最多 4 个线程 | `classify_desktop_items_with_progress` 1430 | 同上，所以需要后台任务和进度条 |
| 卡片间移动 | 在收纳目录的分类子目录之间移动 | `archive_item_path` | 文件路径随分类变化 |
| 移回桌面 | 移回用户桌面，按记录的位置摆图标 | `restore_item_to_desktop_impl` 1160、`restore_path_to_desktop_impl` 4521、`position_desktop_icon_at` 4776 | 同上 |
| 正常退出 | 先写重启收纳标记，再把所有收纳项移回桌面；失败就不退出 | `request_real_app_exit` 14876 → `restore_desktop_organization_for_exit_with_progress` 3609、`prepare_exit_restart_marker` 4013 | 退出要等整体搬家 |
| 启动 | 恢复运行目录迁移 → 切换收纳目录所在卷 → 按标记把桌面项目收回 | setup 闭包 main.rs:15197（15248、15256、15257 行） | 启动要等整体搬家 |
| 崩溃恢复 | transfer journal、隔离区、staging、robocopy、committed marker | main.rs:7378–9106，约 1,700 行 | 规则复杂（AGENTS.md"跨盘收纳迁移与崩溃恢复"一节 8 条） |
| 自愈 | 扫描 `DesktopOrganizer\<分类名>` 补回分类；桌面路径一律当作陈旧引用删除 | `repair_category_item_paths_with_desktop_roots` 6856（注释在 6890） | 只能以收纳目录为准 |
| 收纳目录位置 | 默认与桌面同卷；旧目录启动时自动切换 | store.rs:375–437、`switch_legacy_organizer_to_desktop_volume` main.rs:2347 | 为了减少跨盘又加了一层迁移 |

### 1.2 目标

- 文件不移动：分类只是配置里的成员关系，文件始终在用户桌面或公共桌面。
- 运行时桌面看起来是"已整理"的：隐藏系统桌面图标，由分类卡片和"未分类"卡片显示全部项目。
- 退出、崩溃、卸载后，桌面原样可见，不需要任何反向迁移。
- 老用户数据一次性迁移，不阻塞启动，可以中断后恢复。
- 退役物理收纳相关的大部分机制，简化 AGENTS.md。

### 1.3 非目标

- 不注入资源管理器（不做 Fences 式的注入）。
- 第一期不做铺满整个桌面的自绘图标层，没归类的项目用"未分类"卡片显示。
- 不改快捷启动：复制入口语义、以 Launchers 目录为准等规则都不变。
- 不使用 M2（系统"显示桌面图标"设置，即注册表 `HideIcons`）。
- 不改剪贴板、截图、贴边隐藏、文件对话框助手的交互规则，只改它们引用"收纳目录"的地方。

## 2. 市面做法与选型

| 做法 | 代表 | 原理 | 主要问题 | 结论 |
|---|---|---|---|---|
| 注入资源管理器 | Stardock Fences | 向 Explorer 注入代码，在桌面列表内部分组 | 违背项目"不注入"的原则（AGENTS.md 文件对话框助手一节同样禁止注入）；Explorer 升级后容易崩，也容易被安全软件拦截 | 不选 |
| 隐藏系统图标，自绘全屏图标层 | 腾讯桌面整理、360 桌面助手（按需求方描述） | 关掉系统图标，自己画铺满桌面的图标和格子 | 要重做框选、对齐、拖放、键盘等整套桌面行为，工作量大 | 第一期不做；本方案的卡片是它的子集，以后可以演进过去 |
| 给已分类文件设"隐藏"属性 | — | 只隐藏已分类项，未分类项留在系统桌面上 | 改写用户文件属性；用户开启"显示隐藏的项目"就失效；崩溃后属性残留 | 不选 |
| M2：系统"显示桌面图标"设置 | — | 向 DefView 发 `WM_COMMAND 0x7402` | 同步写入注册表 `HideIcons`：崩溃且看门狗失效时重启电脑也恢复不了，卸载后也不会恢复（spike §3.6 按注册表持久化推断）；隐藏期间读到的图标位置不可信（spike §3.5） | 不选 |
| **M1：只隐藏 `SysListView32`** | — | `ShowWindow(SW_HIDE)` | 用户在右键菜单切换"显示桌面图标"会撤销隐藏（可监听后暂停）；右键菜单的勾选状态与实际不一致 | **选用** |

选用 M1 的依据：

| 证据 | 结果 | 来源 |
|---|---|---|
| 隐藏/恢复耗时 | 0.6–2.5 ms / 1.0–1.8 ms，不改变前台窗口 | spike §3.1 |
| 自动触发条件 | 新建、改名、删除桌面文件，`SHCNE_ASSOCCHANGED`，`SHCNE_UPDATEDIR`，4 种 `WM_SETTINGCHANGE`，静置 30 s，都保持隐藏 | spike §3.2 |
| 唯一会撤销隐藏的情况 | 右键菜单里把"显示桌面图标"关掉再打开（实验 1e，12.4–15.7 ms 内重新显示） | spike §3.2 |
| 看门狗 | 父进程被 `Stop-Process -Force` 结束后 0.2–0.7 ms 恢复图标（9 次） | spike §3.4 |
| 隐藏期间读写图标位置 | M1 下完全可靠，现有 `LVM_*` 代码可以原样工作 | spike §3.5、§4.2 |
| Win+D 检测 | `windows_show_desktop.rs` 不需要改 | spike §4.1 |
| 用户手动测试 | 隐藏后右键桌面，菜单正常；从资源管理器往桌面拖文件，能放进去但看不到，再拖一次提示已存在；多次 Win+D、F5、锁屏解锁都保持隐藏；重启资源管理器后图标自动回来 | 需求方提供 |
| 还没手动测的 | 双显示器、睡眠唤醒、缩放/分辨率变化（spike §7.4 第 4、6a、6c 项），以及本方案新提出的几项（附录 A） | — |

## 3. 数据模型

### 3.1 成员关系以配置为准

- 继续使用 `DeskCategory.item_paths`（models.rs:296），语义从"收纳目录里的路径"改为"桌面上的真实路径"。前端类型 `DeskCategory`（types.ts:3）不用改字段。
- 成员关系只由 `config.json` 决定，不再从目录结构推断。一个路径最多属于一个分类，沿用现有"先从所有分类移除再加入"的写法（main.rs:931–936）。
- 原则上只接受用户桌面、公共桌面根目录的直接子项（`desktop_roots` main.rs:12545）。迁移遗留项（还在旧收纳目录里的）作为过渡状态允许存在，照常显示（§10）。

### 3.2 "未分类"

- 定义：桌面根目录的直接子项 − 所有分类的成员 − 过滤项（§3.3）。每次快照时计算，不落盘。
- 推荐表示方式：在 `DesktopCategories` 里放一个特殊槽位（`Kind = Uncategorized`，固定名"未分类"，`ItemPaths` 永远为空，快照时才填 `item_details`）。理由：拆分/合并、窗口 label `desktop-category-{index}`、贴边隐藏配置、拖放目标 `register_internal_path_drop_category`（main.rs:983）、分类排序都按 index 工作，槽位可以直接复用；把条目拖到"未分类"天然就是"移出分类"。
- 备选：`AppSnapshot` 增加独立的 `uncategorized_items` 和独立 label（如 `desktop-uncategorized`）。要改的 label、拆分、贴边隐藏代码更多，不推荐。
- 轻量快照 `load_desktop_snapshot_impl`（main.rs:584）目前 `desktop_items` 为空（608 行）。方案二必须带上未分类（只列目录，不取图标），否则桌面框没东西可显示。
- 槽位不能删除、不能重命名；一键整理和按名称找分类的 `find_category_index`（main.rs:6650）要跳过它。如果用户已经有一个叫"未分类"的普通分类，迁移时保留它，槽位换一个显示名（实现时定）。
- 普通分类的"删除"可以放开：现在只能删除空分类（main.rs:657），方案二下删除后成员回到未分类，文件不受影响。

### 3.3 过滤规则

桌面扫描、未分类、一键整理、配置修复统一使用：

| 过滤项 | 现有代码 | 说明 |
|---|---|---|
| `desktop.ini` | `is_desktop_ini_path` main.rs:6263 | 沿用 |
| `.dustdesk-*.tmp` 内部事务项 | `is_internal_transfer_path` 8227 | 沿用；迁移窗口内桌面上仍可能有遗留 |
| 隐藏、系统属性文件（如 Office 的 `~$*.docx`、`Thumbs.db`） | 无：`desktop_items`（5739）目前不过滤 | 新增：跟随资源管理器的"显示隐藏的项目/受保护的系统文件"设置（读取方式待确认，例如 `SHGetSetSettings` 的 `fShowAllObjects`、`fShowSuperHidden`） |
| 恰好位于桌面根目录的 DustDesk 数据目录、Launchers、旧收纳目录 | `validate_archive_source` 6795 有类似保护 | 新增 |
| DustDesk 自己的桌面入口 | `should_skip_desktop_classify_item` 6269、`is_app_desktop_entry` 6289 | 显示在未分类里，但一键整理跳过（沿用） |
| 只取直接子项，不递归；同名的用户桌面项和公共桌面项都显示 | `desktop_items` 按完整路径去重 | 沿用 |

### 3.4 模式与版本标记

需要，分两段：

1. `OrganizeMode`（写在 `config.json`）：缺省为 `Archive`（旧数据）。迁移步骤 A 完成时改为 `InPlace`，并且与 `ItemPaths` 的改写放在同一次原子写入（`store.save_config` → store.rs 的 `atomic_write`），不会出现"路径改了但标记没写"的中间态。
2. 迁移 journal `%APPDATA%\DustDesk\in-place-migration.json`：记录步骤 B（把遗留项放回桌面）的逐项计划和进度。全部完成后在 `config.json` 写入完成时间（例如 `InPlaceMigrationCompletedAt`）并删除 journal，以后启动不再检查旧收纳目录。

之所以不等 B 全部完成再写模式标记：B 可能因为文件被占用而跨好几次启动才做完，而新版本不再保留旧模式的代码，从第一次启动起就必须按方案二运行。所以把"按哪种语义理解配置"（`OrganizeMode`）和"迁移是否做完"（journal 与完成时间）拆成两个标记。

### 3.5 `models.rs` 改动

| 结构（位置） | 改动 |
|---|---|
| `AppConfig`（models.rs:27） | 新增 `OrganizeMode`（枚举 `Archive`/`InPlace`，加 `#[serde(other)]` 兜底，避免 `load_config_strict` 遇到未知值整体失败）；新增 `DesktopItemIds: BTreeMap<String, DesktopItemId>`（路径 → 卷序列号 + 文件 ID，为空时不序列化，写法同 `DesktopIconPositions`）；新增 `DesktopRoots: Vec<String>`（上次见到的用户/公共桌面路径，用来发现 OneDrive 等重定向变化）；`DesktopIconPositions`（37 行）迁移期间只读，迁移完成后清空；新增 `InPlaceMigrationCompletedAt`（§3.4） |
| `DeskCategory`（286） | `ItemPaths` 语义改为桌面路径；新增 `Kind`（`Normal`/`Uncategorized`，默认 `Normal`） |
| `AppSnapshot`（11） | 新增 `organize_mode`、`pending_migration_items`（旧收纳目录里还剩几项）和 T1 的图标隐藏状态；`organizer_root`、`organizer_on_desktop_volume`（23 行）在迁移窗口结束后删除 |
| `AppSettings`（165） | T1 新增 `hide_system_desktop_icons`（序列化名以 T1 为准）；`SearchPaths` 为空时的默认值改为桌面（§8） |
| `DesktopItem`（317） | 建议新增 `in_public_desktop: bool`，界面据此提示权限 |
| `ClassifyResult`（344） | `moved` 的含义改为"已归类"；字段名可以不改，前端不用动 |
| 新增 `DesktopItemId` | `{ volume_serial: u64, file_id: String }`（128 位文件 ID 的十六进制） |

### 3.6 `config.json` 兼容

- 新字段都带 `#[serde(default)]`，旧配置可以直接读。
- 同一个 `ItemPaths` 字段在两种模式下含义不同，靠 `OrganizeMode` 区分。新版本只在迁移代码里理解 `Archive`。
- 不支持降级：旧版本读到新配置时，会把桌面路径当作陈旧引用删掉（main.rs:6890 的逻辑），保存时还会丢掉不认识的字段。文件都在桌面上不会丢，但分类关系会丢。对策是迁移前备份（§10.3）。

## 4. 系统图标隐藏

### 4.1 启用条件

全部满足才隐藏，任何一条不满足就立即恢复。

| 条件 | 说明 |
|---|---|
| `OrganizeMode = InPlace` | 迁移步骤 A 完成前不隐藏，保证用户随时能看到文件 |
| 设置 `hide_system_desktop_icons` 开启 | T1 新增，默认关闭；迁移后的默认值见待决 ⑦ |
| 收纳桌面框可见 | 以 `desktop_frame_visibility_impl`（main.rs:14128）返回的 `organizer` 为准：合并框或任一分类卡片可见。托盘"显示桌面框"、`toggle_desktop_frames`（2893）、`toggle_desktop_organizer_frame`（2908）隐藏框时恢复图标。T1 已规划"桌面框隐藏时恢复系统图标" |
| 未处于"已暂停" | T1：列表窗口被重新显示（用户在右键菜单切换了"显示桌面图标"）就暂停，托盘提供"重新隐藏" |
| 不在退出流程中 | `request_real_app_exit`（14876）一开始就恢复 |

### 4.2 兜底链

| 层 | 何时生效 | 效果 | 依据 |
|---|---|---|---|
| 1 正常恢复 | 退出、关闭开关、隐藏桌面框、暂停 | 主进程 `ShowWindow(SW_SHOWNA)`；不能被阻塞的路径用 `ShowWindowAsync` | spike §5.2 |
| 2 看门狗 | 主进程崩溃或被强杀 | 同一个 exe 启动的看门狗进程 0.2–0.7 ms 内恢复 | spike §3.4、§5.3 |
| 3 启动兜底标记 | DustDesk 与看门狗同时死亡，Explorer 还在运行 | 下次启动先按 `desktop-icons-hidden.json` 恢复，再按条件重新隐藏 | T1 |
| 4 资源管理器重启、注销、重启电脑 | 以上都没生效 | M1 不改注册表，新的列表窗口默认可见 | 重启资源管理器已由用户手动确认；注销、重启电脑按同一原理推断（spike §3.6） |

### 4.3 与 T1 的关系

T1（进行中，未合并）负责：

- 新模块 `desktop_icon_visibility.rs`；
- 看门狗：同一个 exe 加内部参数启动，在 `tauri::Builder` 之前处理（`main` 在 main.rs:15124）；
- 启动兜底标记 `desktop-icons-hidden.json`；
- WinEvent 监听：列表窗口被重新显示时进入"已暂停"，托盘提供"重新隐藏"；
- `TaskbarCreated`：资源管理器重启后重新应用；
- 桌面框隐藏时恢复系统图标；
- 设置 `hide_system_desktop_icons` 和托盘开关，默认关闭。

方案二在 T1 之上追加（接口以 T1 的实际实现为准）：

- 启用条件多一条 `OrganizeMode = InPlace`。
- 启动顺序：T1 的兜底恢复放在最前面；迁移步骤 A 完成、桌面框显示之后才允许隐藏（§10.1）。
- 退出时不再还原文件，只由 T1 恢复图标；`RunEvent::Exit`（main.rs:15475）里的同步兜底也改为只恢复图标。
- 状态要推给前端（例如事件 `dustdesk://desktop-icons-state-changed`），卡片据此显示"已暂停"提示条。

## 5. 操作语义对照表

| 操作 | 现在 | 方案二 | 涉及的代码 |
|---|---|---|---|
| 从桌面拖进卡片 | 物理移到 `DesktopOrganizer\<分类>`，记录原图标位置 | 只把路径写进该分类（并从其它分类移除），记录文件 ID。隐藏系统图标时桌面上没有可拖的图标，来源改为未分类卡片或资源管理器里的 Desktop 文件夹；不隐藏时照常从桌面拖 | `add_items_to_category_impl` main.rs:875（`capture_desktop_icon_positions` 910、`archive_item_path` 929）；`handleDroppedPaths` desktop-widget-page.tsx:419、`handleDropped` desktop-card-window-page.tsx:273、`addItemsToCategoryLight` dustdesk-store.ts:1236 |
| 一键整理 | 扫描桌面 → 按规则分类 → 多线程移动，后台任务 + 进度 | 规则不变，只把未分类项写进对应分类；瞬时完成，可保留开始/结束事件；公共桌面项也能归类 | `classify_desktop_items_with_progress` 1430、`classify_desktop_item` 6469、`should_skip_desktop_classify_item` 6269、`start_classify_desktop_items_task` 1374 |
| 卡片之间移动 | 在收纳目录的分类子目录之间移动 | 只改配置 | `add_items_to_category_impl`；`complete_internal_path_drag_impl` 1051 的 `Category` 分支 |
| 移回桌面 / 移出分类 | 移回用户桌面并摆回原位置，同时清除重启收纳标记 | 菜单改叫"移出分类"：只从分类里移除，项目进入未分类；文件本来就在桌面，退出后在系统桌面原位置可见 | `restore_item_to_desktop_impl` 1160；现成但界面没用到的 `remove_item_from_category` 963；菜单 desktop-widget-page.tsx:1573、desktop-card-window-page.tsx:982、organizer-page.tsx:795 |
| 拖出卡片到桌面或别的文件夹 | 拖到窗口外、且没有内部目标接收 → 移回用户桌面，摆到落点。拖到别的文件夹结果也一样：拖拽数据只有路径文本（`writeDustDeskPathDrag` dustdesk-dnd.ts:40），资源管理器文件夹收不到文件（按代码推断） | 拖到窗口外 = 移出分类；是否把图标摆到落点见待决 ⑩。真正拖进别的文件夹或软件需要原生 OLE 文件拖拽，第一期不做 | `complete_internal_path_drag_impl` 1051；`didDragEndOutsideWindow` dustdesk-dnd.ts:114；desktop-box-items.tsx:433 |
| 从别的文件夹拖进卡片 | 移入收纳目录（跨盘是复制 + 删源） | 先放进用户桌面再加入分类：同卷直接改名移动；跨卷复制，源文件保留（待决 ②）；同名生成唯一名，不覆盖；跨卷复制走后台任务并显示进度 | 现在 `archive_item_path` 接受任意路径（测试 main.rs:10172）；复用 `move_path` 7140、`copy_path_via_staging` 7395、`unique_destination` 7102、`validate_archive_source` 6795 的保护逻辑、`start_desktop_background_operation` 13309 |
| 删除到回收站 | 只删本分类记录里的路径，失败的保留 | 不变；未分类项也可以删（只接受桌面根目录的直接子项）；桌面上的项目不再需要先恢复 transfer journal，只有仍在旧收纳目录的迁移遗留项在迁移窗口内保留这一步 | `delete_category_items_to_recycle_bin_impl` desktop_item_actions.rs:642、`recycle_path` 741 |
| 重命名 | 同目录不覆盖改名，原位替换 `item_paths`，把图标位置记录改挂到新路径 | 改名方式不变；不再维护图标位置记录（文件没离开桌面，资源管理器自己保持位置，待确认）；未分类项也可以改名 | `rename_category_item_impl` desktop_item_actions.rs:206（位置记录迁移在 236–247）、`rename_in_place` 394 |
| 复制 / 复制路径 | CF_HDROP + COPY / CF_UNICODETEXT | 不变 | `copy_paths_to_clipboard` desktop_item_actions.rs:30、`copy_text_to_clipboard` 44；clipboard_bridge.rs:159、185 |
| 剪切 | CF_HDROP + MOVE，只接受分类条目，拒绝 Launchers | 不变，未分类项也允许。在资源管理器里粘贴后文件离开桌面，监听发现后从分类移除 | `cut_paths_to_clipboard` desktop_item_actions.rs:101（`categories_contain_path` 检查）、`clipboard_cut_status` 138 |
| 桌面右键"新建" | 新项出现在系统桌面 | 新项进入隐藏的列表，监听发现后出现在未分类卡片。资源管理器在隐藏列表里的改名框怎么表现待手动验证；是否在卡片里自动进入改名见待决 ⑫ | 新增监听（§6） |
| 一键还原 | 把所有收纳项移回桌面、清空记录、清除重启标记 | 文件已在桌面，原含义不存在了。推荐改为"显示系统桌面图标"（暂停隐藏，不动分类），"清空全部分类"放到设置里并要求确认（待决 ③） | `restore_all_organized_items_to_desktop_impl` 4233、`start_restore_all_to_desktop_task` 1215；按钮 desktop-widget-page.tsx:1168、desktop-card-window-page.tsx:846、organizer-page.tsx:596 |
| 删除分类 | 只能删除空分类 | 允许删除非空分类（需确认），成员回到未分类 | `delete_category` 657 |
| 正常退出 | 写重启标记 → 全部移回桌面 → 失败就不退出 | 恢复系统图标后直接退出，没有"正在退出并还原桌面"的等待 | `request_real_app_exit` 14876、`restore_desktop_organization_for_exit_with_progress` 3609、`RunEvent::Exit` 15475 |
| 启动 | 恢复运行目录迁移 → 切换收纳目录卷 → 按标记收回桌面项目 | 恢复运行目录迁移 → T1 兜底恢复 →（仅迁移版本）迁移步骤 A → 显示桌面框 → 校准配置、启动监听 → 按条件隐藏图标；步骤 B 在后台 | setup 闭包 15197；§10.1 |
| 公共桌面项目 | 能扫描到，但普通权限下移动失败（main.rs:7228 的报错文案就是"公共桌面的项目需要以管理员身份运行 DustDesk"） | 归类、移出、卡片间移动都只改配置，不需要权限。删除和重命名需要管理员权限：现在的实现不会弹系统提权确认（重命名用 `MoveFileExW`，直接失败；删除带 `FOF_NOERRORUI`，会不会弹提权待确认）；可以改用 `IFileOperation`，让系统弹确认（待决 ⑧） | `desktop_roots` 12545、`public_desktop` 12572；`rename_in_place`、`recycle_path` |
| 快捷启动 | 复制入口语义 | 不变 | `add_launchers_with_names` 1299、`create_launcher_entry` 5536、`remove_launcher_impl` 1341 |

## 6. 桌面变化监听

### 6.1 选型

| | `ReadDirectoryChangesW` | `SHChangeNotifyRegister` | 轮询 |
|---|---|---|---|
| 来源 | 文件系统，按目录，可以不递归 | Shell 命名空间事件 | 定时 `read_dir` |
| 改名 | 成对给出旧名和新名 | `SHCNE_RENAMEITEM` | 只能靠比对 |
| 线程 | 专用线程阻塞读取即可 | 需要窗口和消息循环 | 定时器 |
| 结论 | **选用** | 不选：事件来源杂，还要消息窗口 | 只做兜底（主界面获得焦点时刷新，现有规则已要求） |

做法：

- 两个监听：用户桌面、公共桌面（`user_desktop` main.rs:12559、`public_desktop` 12572），不递归，关注文件名和目录名的变化。是否同时关注属性变化（以便跟进"隐藏"属性）待定。
- 监听线程只收集事件、投递给合并 worker，自己不做耗时工作（与 AGENTS.md 对 WinEvent 回调的要求一致）。
- worker 去抖约 200–300 ms，然后在 `with_config_mutation`（main.rs:3559）里处理改名和缺失（§6.2），有变化才保存；最后调用一次 `emit_desktop_cards_changed`（13305）。
- 前端沿用 `loadDesktopSnapshot` 的并发合并和 120 ms 去重（dustdesk-store.ts:1052）。DustDesk 自己的操作也会触发事件，重复刷新在这里被吸收，不需要额外抑制。
- 缓冲区溢出（返回 0 字节），或句柄失效（桌面目录被删除或重定向）时，做一次全量校准并重建监听。
- `ReadDirectoryChangesW` 属于 windows-sys 的 `Win32_Storage_FileSystem` 特性（Cargo.toml 已启用）。如果用重叠 IO 或 `CancelIoEx` 结束监听线程，可能还要加 `Win32_System_IO`（待确认）。

### 6.2 外部改名和"安全保存"时保持分类

| 情况 | 处理 |
|---|---|
| 分类里的路径暂时不存在 | 先在内存里标记"暂缺"，快照里不显示，不立即删除记录；宽限期（建议 3–5 s）后仍不存在才处理。Office、记事本等"安全保存"会先删除或改名原文件，再写回同名文件；成员关系按路径记录，文件重新出现后自动归位 |
| 运行中被改名 | 监听拿到旧名 → 新名；宽限期结束时旧路径仍不存在、新路径存在，才把成员关系原位转给新路径（保持顺序，写法同 `rename_category_item_impl`） |
| DustDesk 未运行时被改名，或监听溢出 | 用 `DesktopItemIds` 里的卷序列号 + 文件 ID，与当前未分类项的文件 ID 比对，匹配上就改写路径 |
| 在用户桌面和公共桌面之间移动 | 同卷时文件 ID 不变，按上一行跟随 |
| 跨卷移动、复制后删除、OneDrive 重新下载 | 文件 ID 变了，无法跟随，新文件落入未分类；旧记录在启动校准时删除 |
| 桌面路径整体变化（OneDrive 桌面备份开关、组策略重定向） | 与 `DesktopRoots` 比对发现变化后，先按"旧根 → 新根"改写前缀，再用文件 ID 校验 |

- 文件 ID 用 `GetFileInformationByHandleEx(FileIdInfo)` 取 128 位 ID。NTFS、ReFS 上稳定；FAT32、exFAT 不保证，只能落入未分类。仓库里已有类似调用：store.rs:450 的 `volume_serial_number`、main.rs:12257 的测试辅助 `test_file_identity`。
- 记录时机：加入分类时记录，启动校准时补记。只有出现缺失路径时才需要读取未分类项的文件 ID，平时没有额外开销。

## 7. 图标位置

文件从不移动，图标位置由资源管理器自己保存。

| 代码 | 现在的用途 | 方案二 |
|---|---|---|
| `AppConfig.desktop_icon_positions`（models.rs:37）、`DesktopIconPosition`（150） | 记住已收纳项的原位置 | 迁移期只读；迁移步骤 B 用完后清空，不再写入；字段保留到迁移代码退役 |
| `capture_desktop_icon_positions`（main.rs:4740），以及它在收纳和一键整理里的调用（910、1455） | 移动前读取位置 | 退役 |
| `position_desktop_icons`（4787）、`position_visible_desktop_icons_once`（4833）、`position_desktop_icon_at`（4776） | 移回后摆位置 | 迁移步骤 B 使用；如果待决 ⑩ 选"摆到落点"就继续保留 |
| `desktop_listview_item_texts`（4995）、`desktop_listview_get_item_position`（5123）、`desktop_listview_set_item_position`（5233）、`desktop_drop_position_to_icon_position`（4601） | 跨进程 `LVM_*` | 同上；spike 已验证 M1 隐藏期间可用（§3.5） |
| `desktop_listview_window`（4908） | 找到桌面列表窗口 | 保留：`windows_show_desktop.rs:294` 和 T1 都要用 |
| `retain_desktop_icon_positions_for_categories`（6695）、重命名里的位置迁移（desktop_item_actions.rs:236–247） | 维护位置记录 | 退役 |

如果待决 ⑩ 选"不摆落点"，迁移窗口结束后，上表中除 `desktop_listview_window` 以外的位置代码都可以删除（约 650 行）。

## 8. 文件对话框助手与搜索

| 位置 | 现在 | 方案二 |
|---|---|---|
| `file_dialog_assist_state`（file_dialog_assist.rs:711） | 快捷项"收纳目录"，加上每个分类的 `DesktopOrganizer\<分类>` 子目录（目录存在才显示） | 快捷项"桌面"（用户桌面；公共桌面有内容时再加"公共桌面"）。分类快捷项改为虚拟列表：点击后在面板里列出该分类的项目，不再让对话框跳转到某个目录 |
| `file_dialog_assist_list(None)`（764 起） | 列出收纳目录 | 列出桌面（用户桌面 + 公共桌面合并）；新增按分类列出的参数 |
| `plan_dialog_action`（156） | 打开对话框直接选中文件；保存、选文件夹对话框只跳转 | 不变：打开对话框里点分类项目直接选中；保存对话框跳到桌面并保留原文件名 |
| `search_roots`（373）、`effective_search_paths`（main.rs:5767） | 搜索路径为空时用收纳目录 | 为空时用用户桌面 + 公共桌面；继续用 `SEARCH_SCAN_LIMIT`（main.rs:58）限制递归扫描量 |
| `FileDialogAssistState.organizer_root`（file_dialog_assist.rs:468） | 显示在面板路径栏 | 改为默认根（桌面） |
| 设置页 settings-page.tsx:516、537、645 | "为空时默认使用收纳目录""加入收纳目录""一键跳到收纳目录或搜索收纳的文件" | "为空时默认搜索桌面（含公共桌面）""加入桌面""一键跳到桌面或按分类找文件" |
| 面板 file-dialog-assist-page.tsx:194、205、252 | "收纳目录"、"搜索收纳目录…" | "桌面"、"搜索桌面…" |
| 迁移 | — | 用户在 `SearchPaths` 里显式加过旧收纳目录的，替换为桌面路径 |

## 9. 设置与界面

### 9.1 设置项

| 设置 | 处理 |
|---|---|
| 收纳目录（settings-page.tsx:74–82，含跨盘警告） | 迁移完成且没有遗留项后隐藏；有遗留项时显示只读的"旧收纳目录（还有 N 项待放回桌面）"和"打开"按钮。`update_runtime_directory`（main.rs:1972）不再接受 `organizer` |
| 数据目录、快捷启动目录 | 不变（运行目录迁移保留） |
| 新开关"运行时隐藏系统桌面图标" | T1 的 `hide_system_desktop_icons`，设置页和托盘各一处；暂停时托盘显示"重新隐藏" |
| "收纳模式"开关 | 不新增；待决 ⑥ 推荐不保留旧模式 |

### 9.2 未分类卡片

| 场景 | 处理 |
|---|---|
| 合并态（desktop-widget-page.tsx） | "未分类"是第一个标签，显示数量；不能重命名、删除；把条目拖到这个标签 = 移出分类 |
| 拆分态 | 作为普通槽位参与拆分/合并，label 为 `desktop-category-{index}`（其它显示器为 `desktop-category-{index}-monitor-{id}`，main.rs:13838），可以贴边隐藏；`show_split_desktop_widgets`（3078）"只拆非空分类"的规则同样适用 |
| 槽位位置 | 迁移时先追加到末尾，再用现有 `reorder_category_config`（784）一次性移到第 0 位，拆分索引、窗口布局、贴边配置随之重映射。前端按 index 存的本地状态（当前标签、排版模式）会错位一次，可以接受 |
| 隐藏 | 隐藏系统图标期间，不允许单独隐藏未分类卡片（待决 ⑨） |
| 主界面收纳页"桌面项目"（organizer-page.tsx:570 起） | 改为只列未分类，否则已分类项会重复出现；删除"打开收纳箱"按钮（585） |
| 首页（home-page.tsx:23、34） | "桌面项目"改为"未分类 N / 已分类 M"；去掉"收纳目录"行 |
| 项目很多时 | 未分类列表需要虚拟滚动或分页（§13） |

### 9.3 提示文案

| 场景 | 文案（草稿） |
|---|---|
| 迁移完成后第一次启动 | "DustDesk 不再移动你的文件。原来收纳的项目都放回了桌面，并按原分类显示在卡片里；运行时会隐藏系统桌面图标，退出 DustDesk 后自动恢复。" |
| 已暂停隐藏（卡片顶部提示条） | "你在桌面右键菜单里切换了'显示桌面图标'，已暂停隐藏。[重新隐藏]" |
| 分类空状态（desktop-widget-page.tsx:1544、desktop-card-window-page.tsx:953） | "把桌面上的文件拖到这里归类，文件仍留在桌面。" |
| 未分类空状态 | "桌面上的项目都已归类。" |
| 移出分类 | "已移出「X」，可以在'未分类'里找到。" |
| 外部拖入 | 同卷："已移到桌面并加入「X」"；跨卷："已复制到桌面并加入「X」，原文件保留"；进度："正在复制到桌面 3/10：xxx" |
| 公共桌面项目 | "这是所有用户共享的公共桌面项目，删除或重命名需要管理员权限。" |
| 删除非空分类 | "删除分类「X」？其中 N 个项目会回到'未分类'，文件不受影响。" |
| 迁移进度和失败 | 见 §10.4 |
| 要改的旧文案 | "框里暂不支持粘贴，可以把文件拖进来收纳"（desktop-box-items.tsx:67）；"Windows 不允许直接移动到收纳箱"（desktop-widget-page.tsx:421、desktop-card-window-page.tsx:275）；"确认把所有收纳箱项目移回桌面吗？"（desktop-widget-page.tsx:618、desktop-card-window-page.tsx:465、organizer-page.tsx:103） |

## 10. 旧数据一次性迁移

### 10.1 启动顺序

| 步骤 | 现在（main.rs:15197 起） | 方案二（迁移版本） |
|---|---|---|
| 1 | 恢复运行目录迁移 journal（15248） | 同左 |
| 2 | 显示桌面框和进度遮罩 | T1：如果有 `desktop-icons-hidden.json`，先恢复图标 |
| 3 | `switch_legacy_organizer_to_desktop_volume`（15256） | 删除这一步：收纳目录马上停用，不必再切换卷 |
| 4 | `recollect_desktop_organization_from_restart_marker_with_progress`（15257），里面先严格恢复 transfer journal | 严格恢复 transfer journal，沿用 3827–3836 行"先全部、再与标记相关"的做法；若 `OrganizeMode != InPlace`，执行迁移步骤 A。**绝不能再把桌面项目搬回收纳目录** |
| 5 | 初始化运行服务、显示桌面框、清理 staging（15326） | 发出"启动恢复完成"、显示桌面框、广播；后台开始步骤 B |
| 6 | — | 校准（§6.2）→ 启动桌面监听 → 按条件隐藏图标 |

### 10.2 步骤

**A：标记 → 分类（不动文件，毫秒级）**

- 输入是重启收纳标记 `desktop-organization-restart.json`（`DesktopOrganizationRestartMarker` main.rs:374；每一项 `RestartItemMarker`（405）有 `source_path`、`desktop_path`、`desktop_icon_position`）。正常退出后，它记录了每个已还原项目在桌面上的实际路径。
- 逐项处理：

| 标记项的状态 | 处理 |
|---|---|
| 桌面路径存在，收纳路径不存在（常见情况） | 把桌面路径加入分类，记录文件 ID |
| 两边都存在（现有代码视为歧义、两份都保留，见 3729、3918 行注释） | 桌面那份加入分类；收纳目录那份交给步骤 B，以唯一名放回桌面，也加入同一分类，由用户自己删除 |
| 只有收纳路径存在（退出时没能移回） | 交给步骤 B |
| 两边都不存在 | 丢弃 |

- 分类定位复用 `restart_category_index`（4145）的规则：先按 index + 名称找，再按名称找，都找不到就新建。
- config 里原有、仍指向收纳目录的 `ItemPaths`（强杀或崩溃后会有）原样保留给步骤 B。
- 写入顺序：①把 `config.json` 备份为 `config.pre-in-place.json`（Data 目录）；②一次原子写 config：`OrganizeMode=InPlace`、未分类槽位、改写后的 `ItemPaths`、`DesktopItemIds`；③把标记改名为 `desktop-organization-restart.migrated.json`（旧版本只认原文件名，改名后不会再被执行）。

**B：旧收纳目录里的遗留项放回桌面（后台）**

- 来源：config 中位于旧收纳目录的路径；`organizer_contents`（5387）扫到但没有记录的项，按目录名归入分类（规则同 `category_index_for_organizer_dir` 7012、`recover_missing_categories_from_organizer` 6965）；收纳根目录下的散文件进入未分类。
- 目标是用户桌面 `user_desktop()`（12559）。同名处理沿用 `restore_path_to_desktop_impl`（4521）：内容逐字节相同才删除收纳目录里的副本（`remove_duplicate_path_if_identical` 4579），否则用 `unique_destination`（7102）生成唯一名，绝不覆盖。
- 移动用 `move_path`（7140）：同卷直接改名；跨卷走现有 staging + transfer journal。
- 成功后：把分类里的旧路径原位替换为新路径，记录文件 ID；按 `DesktopIconPositions` 摆放图标（`position_desktop_icons` 4787）。
- Launchers：`create_launcher_entry`（5536）给非快捷方式来源创建的 `.lnk` 指向旧收纳路径，要按"旧 → 新"映射，用 `system_icon::update_shortcut_target`（system_icon.rs:119）改写目标。步骤 A 的映射（`source_path` → `desktop_path`）同样适用。
- 可选：`search-history.json` 里的旧路径按映射逐项改写（现有 `rewrite_search_history_path_prefixes` 6197 只支持改写前缀）。
- 全部完成后：清空 `DesktopIconPositions`，执行 `cleanup_empty_organizer_dirs`（5415）；收纳根目录本身不删；记录完成时间，删除迁移 journal。

### 10.3 迁移 journal 与中断恢复

- 文件为 `%APPDATA%\DustDesk\in-place-migration.json`，原子写入（store.rs 的 `atomic_write`）。每项移动前先落盘计划（源路径、计划目标、分类），与现有退出流程"先持久化目标再移动"的顺序一致（main.rs:3744）。
- 移动在配置锁外执行：持锁写计划 → 释放锁后移动 → 再持锁复核路径仍在配置中并提交。跨卷复制可能很久，不能一直占着 `with_config_mutation`，否则快照加载都会卡住。

| 中断点 | 下次启动 |
|---|---|
| A 写 config 之前 | 原样重来（标记和 config 都没变） |
| A 写 config 之后、标记改名之前 | `OrganizeMode` 已是 `InPlace`，跳过 A，只补做改名 |
| B 同卷改名进行中 | 改名是原子的。看 journal：目标在、源不在 → 已完成；源在、目标不在 → 重做；两边都在 → 两份都保留，目标加入分类 |
| B 跨卷目录移动进行中 | 先由 transfer journal 恢复（未提交则还原源目录，已提交则继续清理隔离源），再按上一行判断 |
| 降级后又升级 | 旧版本可能已经清空分类并重新收纳；新版本按"未迁移"重新走一遍 A、B |

备份：`config.pre-in-place.json` 和改名后的标记都保留。需要退回旧版本时，把标记改回原名再启动旧版本，旧版本会按标记重新收纳。这个步骤写进发布说明，不做成按钮。

### 10.4 来源、失败与进度

- **公共桌面来源**：代码里没有记录。`RestartItemMarker` 只有上面三个字段；单项移回、一键还原、退出还原都固定还原到 `user_desktop()`（main.rs:1174、4241、3616）。所以无法区分来源，统一放到用户桌面，这与旧版本退出时的行为一致。另外，普通权限下旧版本本来就移不动公共桌面项目（7228）。
- **失败项**（被占用、没有权限、磁盘空间不足）：留在旧收纳目录，卡片照常显示、可以打开；写诊断日志；下次启动重试；连续失败多次后在设置页提示手动处理。
- **不阻塞启动**：A 只写配置。"启动恢复完成"信号（快照加载会等它，`wait_for_startup_recovery` main.rs:478）在 A 完成后就发出，B 不参与。B 在后台线程逐项执行，期间所有功能都可用。
- **进度**：用 `DesktopOperationPayload`（main.rs:179）发送，但界面上用不遮挡的提示条。现有的 lifecycle 进度会让卡片进入忙碌状态并显示遮罩，B 可能很久，不能复用遮罩。文案："正在把旧收纳目录里的项目放回桌面 3/20：xxx"；失败时："有 N 个项目仍在旧收纳目录（文件被占用或没有权限），卡片里照常显示，下次启动会重试。"
- **升级路径**：先正常退出再升级时，所有项目已经在桌面上，只走 A。安装器强制结束旧进程、或旧版本崩溃时，项目还在收纳目录，走 B。Tauri NSIS 安装器升级时如何结束正在运行的 DustDesk（是否按进程名结束、会不会触发正常退出）待确认。
- **全新安装**：没有标记、收纳目录为空，A、B 都是空操作，第一次启动就写入 `OrganizeMode=InPlace`。迁移时发现重启标记、收纳目录遗留项或已有分类成员的，算作老用户（供待决 ⑦ 区分默认值）。

## 11. 退役与保留

| 机制 | 代码 | 方案二 | 删除时机 |
|---|---|---|---|
| 物理收纳 | `archive_item_path` main.rs:6714、`archive_destination` 6837 | 删除；`validate_archive_source`（6795）的保护逻辑挪给外部拖入 | T7 |
| 一键整理的移动线程池 | `classify_desktop_items_with_progress` 1430–1751（约 320 行） | 改写为只改配置（约 80 行） | T2 |
| 收纳目录卷切换 | `switch_legacy_organizer_to_desktop_volume` 及相关 2317–2770、类型 282–372；store.rs:375–437 的默认目录选择；`organizer-volume-auto-switch.json` | T5 起不再调用 | T7 |
| 退出还原、按标记回收 | `restore_desktop_organization_for_exit*` 3605–3817、`recollect_*` 3818–4012、`prepare_exit_restart_marker` 4013 及 4091–4222 的辅助函数；`rewrite_restart_marker_path_prefixes` 2283 | T5 起不再调用；标记类型（374–448）和 store.rs:139–160 的读取保留给迁移 | 执行逻辑 T7；读取 T8 |
| 一键还原旧实现 | `restore_all_organized_items_to_desktop_impl` 4233、`restore_candidate_count` 4422 | 换成新语义（T4） | T7 |
| 单项还原到桌面 | `restore_path_to_desktop_impl` 4521、`remove_duplicate_path_if_identical` 4579 | 只给迁移步骤 B 用 | T8 |
| 收纳目录自愈 | `repair_category_item_paths*` 6844–6964、`recover_missing_categories_from_organizer` 6965、`category_index_for_organizer_dir` 7012、`organizer_contents` 5387、`cleanup_empty_organizer_dirs` 5415 | `repair` 改写为桌面版（§6.2）；扫描收纳目录的部分只给迁移用 | 改写 T2；扫描部分 T8 |
| transfer journal、隔离区 | `TransferQuarantineJournal`、`QuarantinedTransferSource` 等 7573–7817，恢复逻辑 7920–8104 | 迁移窗口内保留（启动恢复、步骤 B 的跨卷目录移动）；外部拖入跨卷只复制，不需要隔离区 | T8 |
| staging 复制、robocopy、校验 | 7378–7572、7818–7919、8105–9106 中的 staging 部分（8133 的定时清理顺带恢复 journal，T8 时拆开） | 保留：跨卷复制和运行目录迁移在用 | 保留 |
| `move_path` | 7140–7377 | 保留：外部拖入的同卷移动、步骤 B | 保留 |
| 运行目录迁移 | `RuntimeMigrationJournal` 261、1971–2282 | 保留（数据目录、快捷启动目录）；去掉 `organizer` 目标 | 去掉 organizer 分支 T7 |
| 每个操作前恢复 transfer journal | 加入分类 883；删除、重命名（desktop_item_actions.rs:653、211）等 | 针对桌面项目的操作去掉；启动、步骤 B，以及针对旧收纳目录遗留项的操作在迁移窗口内保留 | 桌面部分 T2；其余 T8 |
| 内部临时项过滤 | `is_internal_transfer_path` 8227 等 | 保留（桌面上可能有旧遗留） | 保留 |
| 桌面"收纳箱"入口 | `app_desktop_entries` 1772、`repair_existing_app_desktop_entries` 1779 | 去掉"收纳箱"；用户桌面上已有的快捷方式不自动删除 | T7 |
| `open_special("organizer")` | 1817 | 遗留期间保留为"打开旧收纳目录" | T8 |
| 每次调用都创建收纳目录 | `ensure_runtime_dirs` store.rs:231 | 不再创建 | T2 |
| 快照里的收纳目录字段 | `AppSnapshot.organizer_root`、`organizer_on_desktop_volume` | 连同设置页警告一起删除 | T8 |

**代码量粗估**（按函数起止行号估算）：

| 阶段 | 生产代码 | 测试 |
|---|---|---|
| T2–T7 可删 | 约 1,900 行：卷切换约 540、退出/回收约 650、物理收纳约 130、自愈改写净减约 100、一键整理净减约 240、一键还原约 230 | 约 28 个（其中 4 个改写为桌面版），约 1,300 行：main.rs:10148–11418 的收纳、标记、卷切换、启动测试，以及 12332 |
| T8 再删 | 约 800 行：隔离区与 journal 约 430、单项还原约 90、收纳目录扫描约 110、标记类型与读取约 140，另加迁移模块本身。如果待决 ⑩ 选"不摆落点"，再加约 650 行位置代码 | 约 7 个（隔离区 4 个、同名还原 3 个） |
| 新增 | 约 1,000–1,400 行：监听与校准、未分类、文件 ID、迁移、外部拖入（不含 T1）；前端约 400 行 | 迁移、校准、外部拖入的单元测试 |

main.rs 现有 81 个测试，其中移动、复制、staging 相关的约 20 个继续保留。

## 12. AGENTS.md 改动清单

"窗口"指"Windows 下的 Tauri 窗口创建"一节，"跨盘"指"跨盘收纳迁移与崩溃恢复"一节，序号按原文条目顺序。

| 位置 | 原规则要点 | 处理 | 新规则要点 |
|---|---|---|---|
| 窗口 1–12 | 窗口创建、稳定 label、不置顶、置底、拖放注册、拆分/合并顺序、批量创建 | 保留 | 补充：未分类槽位同样使用 `desktop-category-{index}` |
| 窗口 13 | "移回桌面"不能只从配置移除，必须把文件移回桌面；同名时保留桌面原有项 | 改写 | "移出分类"只改配置，项目进入未分类；文件始终在桌面 |
| 窗口 14 | 收纳箱项目要支持拖出卡片移回桌面，必须走 `restore_item_to_desktop` | 改写 | 拖到窗口外 = 移出分类（是否摆到落点按待决 ⑩），不移动文件 |
| 窗口 15 | config.json、launch.json 的读改写必须串行 | 保留并补充 | 桌面监听和迁移的配置写入也走 `with_config_mutation`；耗时的移动、复制不能持有配置锁 |
| 窗口 16 | 加载快照时扫描 `DesktopOrganizer/<分类名>` 补回分类 | 删除 | 改为：以配置为准校准桌面（宽限期 + 文件 ID 跟踪改名） |
| 窗口 17 | 正常退出先写重启收纳标记再还原，启动按标记收回 | 删除 | 改为：退出只恢复系统图标；启动绝不把桌面项目搬进任何目录；旧标记只由一次性迁移读取 |
| 窗口 18 | 显式还原前清除重启收纳标记 | 删除 | — |
| 窗口 19–23 | 快捷启动的复制语义、不能作为拖拽源、以 Launchers 为准、创建入口、图标不写进 launch.json | 保留 | — |
| 窗口 24 | 桌面项目加入收纳必须物理移动到 `DesktopOrganizer/<分类名>` | 改写 | 桌面项目加入分类只写配置；外部文件按待决 ② 移入或复制到桌面，同名不覆盖 |
| 窗口 25 | 不存在的路径在加载快照时自动清理 | 改写 | 先标记暂缺，宽限期过后仍缺失、且按文件 ID 也找不到才删除 |
| 窗口 26 | 桌面路径用 Known Folder | 保留并补充 | 用户桌面和公共桌面都要监听；桌面路径变化时重建监听、改写前缀 |
| 窗口 27 | 默认目录；收纳目录必须与桌面同卷 | 改写 | 只保留 Data、Launchers 的默认位置，不再创建收纳目录 |
| 窗口 28–29 | 旧收纳目录启动时自动切换卷；手动选择的不切换 | 删除 | 由"一次性迁移"规则取代（见末行"新增"） |
| 跨盘 1、8 | 同卷先原子改名，只有 `ERROR_NOT_SAME_DEVICE` 才走跨盘 | 改写 | 只适用于外部拖入和迁移步骤 B |
| 跨盘 2–5 | 隔离区、UTF-16 journal、robocopy 退出码、staging 校验、committed marker、后台清理 | 迁移窗口内保留，T8 删除 | robocopy 和 staging 校验移到"跨卷复制"下继续有效 |
| 跨盘 6 | 启动和每次桌面存储操作前串行恢复 transfer journal | 改写 | 只在启动和迁移步骤 B 之前恢复；T8 删除 |
| 跨盘 7 | 内部事务数据统一过滤 | 保留并补充 | 加上隐藏、系统文件的过滤（§3.3） |
| 多窗口数据刷新 1–4 | 广播、主界面自动刷新、快照合并去重、快捷启动框菜单 | 保留 | 补充：桌面监听去抖合并后只广播一次 |
| 剪贴板 1–7 | — | 保留 | — |
| 条目操作 1 | 删除走回收站；理由是"只从配置删路径无效，快照会从收纳目录补回" | 改写理由 | 删除仍走回收站；未分类项也可以删；不再提收纳目录 |
| 条目操作 2 | 删除只处理本分类 `item_paths`，开头先恢复 transfer journal | 改写 | 只处理本分类成员，或桌面根目录直接子项（未分类）；去掉 journal 要求（迁移窗口内的旧收纳目录遗留项除外） |
| 条目操作 3、4、6、8 | 复制、快捷启动删除、快捷启动重命名、剪切状态轮询 | 保留 | — |
| 条目操作 5 | 重命名要把 `desktop_icon_positions` 改挂到新路径 | 改写 | 原位替换成员路径即可，不再维护位置记录 |
| 条目操作 7 | 剪切只接受分类条目 | 改写 | 分类条目和未分类项都可以；Launchers 仍然拒绝 |
| 快照性能 1 | 桌面框用轻量快照 | 改写 | 轻量快照必须包含未分类（不含图标） |
| 快照性能 8 | 只有明确需要的页面才做完整桌面扫描和图标提取 | 改写 | 列举桌面根目录是常规操作；图标提取仍然只能异步 |
| 快照性能 2–7 | 图标异步、缓存、批量命令、light 拖拽后刷新 | 保留 | — |
| 贴边隐藏、文件对话框助手 | — | 保留（规则本身没有提收纳目录） | — |
| 相关验证 | `cargo check`、`npm run build` | 保留并补充 | 改桌面监听或迁移后要跑 `cargo test`；改图标隐藏后按 spike §7 的手动清单验证 |
| 新增 | — | 新增 | ①分类成员以 config.json 为准，加入、移出、卡片间移动都不移动文件；②未分类 = 桌面根目录直接子项 − 已分类 − 过滤项；③系统图标只用 M1，不得改 `HideIcons`，先武装看门狗、写兜底标记再隐藏，桌面框隐藏、退出、暂停时恢复（具体条文由 T1 写入）；④桌面监听回调只投递，去抖合并后再处理；⑤一次性迁移的启动顺序、journal，以及"绝不把桌面项目搬回收纳目录"；⑥公共桌面项目的权限提示 |

## 13. 风险与对策

| 风险 | 影响 | 对策 | 状态 |
|---|---|---|---|
| 用户在右键菜单切换"显示桌面图标" | M1 被撤销 | T1：监听到列表重新显示就进入"已暂停"，托盘和卡片提示"重新隐藏" | T1 进行中 |
| DustDesk 与看门狗同时被结束（结束进程树、断电、安装器按进程名结束——看门狗与主程序是同一个 exe） | 图标保持隐藏 | 启动兜底标记；资源管理器重启、注销、重启电脑后恢复；帮助里写明"任务管理器 → 重新启动 Windows 资源管理器"；安装器的行为待确认 | 部分待确认 |
| 隐藏的列表仍可能拿到键盘焦点 | 用户点桌面空白处后按 Delete、F2、Ctrl+A、Enter 或字母键，可能作用在看不见的选中项上 | 待手动验证。如果确实存在：隐藏后清空列表选择（跨进程 `LVM_SETITEMSTATE`，做法同现有 `LVM_SETITEMPOSITION32`），必要时监听列表的焦点和选择事件 | 待确认 |
| 隐藏状态下右键"新建" | 资源管理器的改名框在隐藏列表里，用户看不到 | 待手动验证；可能要发 `LVM_CANCELEDITLABEL` 结束隐藏的编辑，再在未分类卡片里改名（待决 ⑫） | 待确认 |
| 未分类项目很多（上千个） | 卡片渲染慢，而且每个显示器各有一份窗口；后端图标缓存上限 1024 项（main.rs:62） | 未分类列表虚拟滚动或分页；图标继续异步分批解析（前端已有）；快照不带图标 | 设计内 |
| 外部改名、安全保存 | 分类丢失 | 宽限期 + 改名事件 + 文件 ID（§6.2）；做不到时落入未分类，文件不丢 | 设计内 |
| 公共桌面权限 | 删除、重命名失败 | 归类不受影响；删除和重命名给出明确提示，或改用 `IFileOperation` 让系统弹提权（待决 ⑧） | 待决 |
| 用户以为文件丢了 | 投诉、误操作 | 迁移后首次提示；未分类卡片不能单独隐藏；托盘一键显示系统图标；退出即恢复；在资源管理器里打开 Desktop 文件夹能看到全部文件 | 设计内 |
| OneDrive 桌面 | 路径变化、冲突副本、占位文件 | Known Folder 已经处理（`user_desktop` main.rs:12559）；冲突副本进入未分类；记录 `DesktopRoots` 应对"桌面备份"开关；方案二不移动文件，现有移动代码拒绝 reparse 点（8272）的问题不再相关（对现状有没有影响待确认） | 部分待确认 |
| 多显示器、DPI | 隐藏能否覆盖所有显示器；卡片布局 | 一个列表窗口覆盖整个虚拟屏，预计会一起隐藏，待双屏手动确认（spike §7.4 第 4 项）；卡片沿用现有的按显示器显示（13854）和 DPI 换算（4614） | 待确认 |
| 睡眠唤醒、缩放或分辨率变化 | 隐藏可能被撤销 | 待手动验证（spike §7.4 第 6a、6c 项）；被撤销时由 T1 的监听进入暂停 | 待确认 |
| Explorer 挂起 | 跨进程 `ShowWindow` 阻塞 | 不能阻塞的路径用 `ShowWindowAsync`（spike §5.2） | T1 |
| 迁移中断或失败 | 部分项目还在旧收纳目录 | journal + 遗留项照常显示 + 启动重试（§10.3） | 设计内 |
| 降级到旧版本 | 分类关系丢失（文件不丢） | 不支持降级；迁移前备份配置，保留改名后的标记（§10.3） | 设计内 |
| 桌面目录里的文件不再减少 | 在资源管理器里打开 Desktop 文件夹时内容很多 | 这正是"文件不动"的本意，在说明里讲清楚 | 接受 |

**回滚方式**

- 产品内：关掉"隐藏系统桌面图标"，系统图标立即恢复；文件一直在桌面上，不存在反向迁移问题。卸载后 M1 不会留下任何设置。
- 代码层面：发版后如果要撤回整个方案，只能再发一个新版本处理（旧版本会清掉桌面路径的分类关系）。所以 T2–T6 必须在集成分支上完成、手动验证后再一起发版。

## 14. 实施步骤

依赖关系：

```text
T1（进行中，默认关闭，可先合入 main）
T2 数据模型与操作语义 ──┬── T3 桌面监听与改名跟踪 ──────────────┐
                         ├── T4 未分类卡片与界面（还依赖 T1）──────┤
                         ├── T5 旧数据一次性迁移 ───────────────────┼── T7 退役清理 + 文档 ── 发版 ──（若干版本后）T8
                         └── T6 文件对话框助手与搜索 ──────────────┘
```

- T2–T6 在集成分支（例如 `feat/in-place-organizing`）上逐个合并、各自验收，全部完成后再一起合入 main。
- `main.rs` 有 1.5 万行，新代码尽量放进新模块（`desktop_watch.rs`、`in_place_migration.rs` 等），减少并行开发时的冲突。T4 和 T5 都会改启动流程（main.rs:15197 起），需要约定先后。
- 验收命令中的 `cargo` 都在 `src-tauri` 目录下执行。

| 任务 | 内容 | 依赖 / 并行 | 验收命令 | 需要手动验证 | 工作量（粗估） |
|---|---|---|---|---|---|
| T1 | 见 §4.3 | 无 | `cargo check`、`cargo test`、`npm run build` | spike §7 清单；托盘开关；暂停与重新隐藏 | 进行中 |
| T2 数据模型与操作语义 | §3 全部；§5 中加入、整理、卡片间移动、移出、拖出、删除、重命名、剪切、删除分类改为只改配置；外部拖入（同卷移动、跨卷复制、进度）；轻量快照带未分类；快照修复改为桌面版；不再创建收纳目录 | 最先做 | `cargo check`、`cargo test`（新增：成员唯一、未分类计算与过滤、外部拖入同名不覆盖、跨卷复制保留源） | 拖入、拖出、卡片间移动；公共桌面项归类 | 4–6 天 |
| T3 桌面监听与改名跟踪 | §6：两个监听线程、去抖、宽限期、改名事件、文件 ID、溢出重扫、桌面路径变化 | T2 之后；可与 T4–T6 并行 | `cargo check`、`cargo test`（校准逻辑写成纯函数单测：改名、安全保存、ID 匹配、前缀改写） | 在资源管理器里新建、改名、删除；用 Word 编辑桌面文档并保存；DustDesk 未运行时改名；OneDrive 桌面 | 3–4 天 |
| T4 未分类卡片与界面 | §9：未分类槽位（前后端）、合并/拆分、主界面和首页、设置页、托盘、文案、新的"一键还原"（待决 ③）、隐藏条件接线（§4.1） | T2、T1 之后；可与 T3、T5、T6 并行 | `npm run build`、`cargo check` | 合并、拆分、多显示器、贴边隐藏下的未分类卡片；暂停提示条；项目很多时是否流畅 | 4–5 天 |
| T5 旧数据一次性迁移 | §10 全部；调整启动顺序；改写 Launchers；替换 SearchPaths；备份 | T2 之后；可与 T3、T4、T6 并行 | `cargo check`、`cargo test`（正常退出后、强杀后、两份都在、同名冲突、跨卷目录、各个中断点恢复、重复运行幂等） | 用 0.1.13 造数据后升级：正常退出、任务管理器强杀、收纳目录在另一个盘、有公共桌面项 | 4–5 天 |
| T6 文件对话框助手与搜索 | §8 | T2 之后；可并行 | `cargo check`、`cargo test`、`npm run build` | 记事本打开、另存为对话框；浏览器上传对话框；搜索默认范围 | 1–2 天 |
| T7 退役清理与文档 | §11 中标 T7 的删除；重写 AGENTS.md（§12）和 README.md 第 47–53 行 | T2–T6 之后 | `cargo check`、`cargo test`、`npm run build` | 全量回归 | 2–3 天 |
| T8 迁移窗口后清理 | 删除 transfer journal、隔离区、标记读取、迁移步骤 B、旧收纳目录相关字段 | 发版后若干版本（待决 ⑪） | 同上 | 回归 | 1–2 天 |

不含 T1 合计约 19–27 人日。发版前的总体手动验证 = 附录 A 全部事项 + 上表各任务的手动项。

## 15. 待用户决定的问题

| # | 问题 | 选项 | 推荐 | 理由 |
|---|---|---|---|---|
| ① | 没归类的项目怎么显示 | A "未分类"卡片；B 全屏自绘图标层 | A | 复用现有卡片和窗口机制；B 要重做整套桌面交互，可以作为以后的演进 |
| ② | 从别处拖进卡片的文件 | A 同卷移动、跨卷复制（与资源管理器默认一致）；B 一律复制；C 一律移动；D 只记录引用、不动文件 | A | 与资源管理器一致；跨卷不删源，隔离区和 journal 可以退役；C 要长期保留整套崩溃恢复；D 违背"分类成员都在桌面" |
| ③ | "一键还原"的新含义 | A 改为"显示系统桌面图标"（暂停隐藏，不动分类）；B 清空全部分类并显示图标；C 去掉按钮 | A，另在设置里提供"清空全部分类"（需确认） | 原含义"把文件搬回桌面"已经不存在；A 不破坏整理结果 |
| ④ | 收纳目录是否保留 | A 迁移完成后不再使用（设置项隐藏，路径文件留作迁移重试）；B 保留，只给外部拖入的文件用 | A | B 会让分类成员分散在两个位置，搜索、对话框、隐藏逻辑都要分两套 |
| ⑤ | 用户在右键菜单切换"显示桌面图标"时 | A 暂停隐藏，托盘提供"重新隐藏"；B 立即再隐藏 | A（T1 按此实现） | 用户明确想看系统图标，立即再隐藏像在跟用户抢 |
| ⑥ | 是否保留旧的物理收纳模式作为可选模式 | A 不保留；B 保留 | A | 双模式要同时维护两套语义、迁移和规则，复杂度翻倍 |
| ⑦ | 迁移后是否默认开启"隐藏系统桌面图标" | A 老用户迁移后默认开启，新用户首次启动时询问一次；B 全部默认开启；C 全部默认关闭（T1 目前的默认） | A | 老用户原来的桌面上本来就没有已收纳项，开启才能保持观感；新用户突然看不到图标容易以为文件丢了 |
| ⑧ | 公共桌面项目的删除和重命名 | A 改用 `IFileOperation`，由系统弹提权确认；B 第一期只提示"需要管理员权限，请在资源管理器中操作" | 先 B，A 视需要跟进 | A 的行为要先验证（附录 A 第 5 项），不影响第一期主体 |
| ⑨ | 隐藏系统图标期间，未分类卡片能否单独隐藏 | A 不能；B 能，隐藏时同时恢复系统图标；C 能，其它什么也不做 | A | C 会让未分类项目在哪里都看不到 |
| ⑩ | 拖出卡片到桌面时，是否把系统图标摆到落点 | A 摆（显示系统图标后，图标出现在松手的位置）；B 不摆 | B | 隐藏期间看不到落点，A 只在不隐藏图标时有用；选 B 可在迁移窗口后删掉约 650 行 `LVM_*` 位置代码 |
| ⑪ | 迁移代码（含 transfer journal 恢复）保留多久 | A 保留 3 个正式版本；B 保留半年；C 长期保留 | A、B 取较晚者 | 删除后，从更老版本直接升级的用户需要先装一个过渡版本；届时启动检测到旧数据就给出提示 |
| ⑫ | 桌面右键"新建"后，是否在未分类卡片里自动进入改名 | A 自动进入；B 只显示新项 | 先做附录 A 第 2 项验证再定 | 取决于资源管理器在隐藏列表里的改名框怎么表现 |

## 附录 A：待确认与待手动验证

| # | 事项 | 方式 |
|---|---|---|
| 1 | 隐藏后点击桌面空白处，按 Delete、F2、Ctrl+A、Enter、字母键，会不会作用于看不见的项目 | 手动，用 probe 的 `hide-window` |
| 2 | 隐藏后右键"新建"：改名框是否出现，键盘输入去了哪里 | 手动 |
| 3 | 双显示器下，两块屏幕的图标是否都隐藏 | 手动（spike §7.4 第 4 项） |
| 4 | 睡眠唤醒、修改缩放或分辨率后是否仍然隐藏 | 手动（spike §7.4 第 6a、6c 项） |
| 5 | `SHFileOperationW` 带 `FOF_NOERRORUI` 删除公共桌面项时会不会弹提权；`IFileOperation` 的提权行为 | 代码试验 |
| 6 | Tauri NSIS 安装器升级时如何结束正在运行的 DustDesk，会不会连看门狗一起结束 | 查模板并实测 |
| 7 | 在资源管理器里改名后，桌面图标位置是否保持 | 手动 |
| 8 | 桌面重定向到网络共享时，`ReadDirectoryChangesW` 是否可靠 | 实测 |
| 9 | 是否需要给 windows-sys 增加 `Win32_System_IO` 特性 | 编码时确认 |
| 10 | 资源管理器"显示隐藏的项目"设置的读取方式 | 编码时确认 |
| 11 | OneDrive 占位文件会不会被现有移动代码的 reparse 检查拒绝（只影响现状） | 实测 |
