//! 桌面收纳框贴边自动隐藏。
//!
//! 后端是几何与状态的唯一权威：持久化配置在 `DesktopLayout.EdgeHide`，
//! 运行时只记录每个 label 是否收起以及展开矩形。窄条几何永远不写进布局。
//!
//! 锁顺序固定为 `LAZY_WINDOW_OPERATION_LOCK` → `EDGE_HIDE_LOCK` → `CONFIG_MUTATION_LOCK`。
//! 运行时表 `EDGE_HIDE_RUNTIME` 是叶子锁，只做短暂读写。

use std::{
    collections::HashMap,
    sync::{Mutex, MutexGuard, OnceLock},
    time::{Duration, Instant},
};

use serde::Serialize;
use tauri::{
    Emitter, LogicalSize, Manager, PhysicalPosition, PhysicalSize, Position, Size, WebviewWindow,
};

use crate::{
    append_desktop_diagnostic, capture_desktop_window_layout, desktop_window_min_logical_size,
    desktop_window_target_work_area, is_desktop_card_window_label, is_valid_desktop_window_layout,
    keep_desktop_window_behind_apps, load_desktop_window_state,
    models::{DesktopEdge, DesktopEdgeHideConfig, DesktopEdgeHideMode},
    remap_desktop_category_window_label,
    store::AppStore,
    to_message, valid_scale_factor, with_config_mutation, DesktopMonitorWorkArea,
};

pub const DESKTOP_EDGE_HIDE_CHANGED_EVENT: &str = "dustdesk://desktop-edge-hide-changed";
const STRIP_LOGICAL_THICKNESS: f64 = 6.0;
const POINTER_SLOP: i64 = 2;
const PRIMARY_RELEASE_TIMEOUT: Duration = Duration::from_secs(15);
const PRIMARY_RELEASE_POLL: Duration = Duration::from_millis(30);

static EDGE_HIDE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static EDGE_HIDE_RUNTIME: OnceLock<Mutex<HashMap<String, EdgeHideRuntime>>> = OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct EdgeRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl EdgeRect {
    fn right(&self) -> i64 {
        i64::from(self.x) + i64::from(self.width)
    }

    fn bottom(&self) -> i64 {
        i64::from(self.y) + i64::from(self.height)
    }
}

#[derive(Debug, Clone, Copy)]
struct EdgeHideRuntime {
    collapsed: bool,
    expanded: EdgeRect,
}

#[derive(Debug, Clone, Serialize)]
pub struct DesktopEdgeHideState {
    label: String,
    enabled: bool,
    mode: DesktopEdgeHideMode,
    edge: Option<DesktopEdge>,
    collapsed: bool,
    expanded: EdgeRect,
    scale_factor: f64,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct DesktopEdgeHidePointer {
    inside: bool,
    primary_button_down: bool,
}

// ---------- 纯几何 ----------

fn clamp_axis(value: i64, start: i64, end: i64, length: u32) -> i32 {
    let max = (end - i64::from(length)).max(start);
    value.clamp(start, max) as i32
}

/// 把展开矩形贴到指定边：垂直于该边的坐标吸附，沿边坐标夹在工作区内。
pub fn snap(rect: EdgeRect, edge: DesktopEdge, work_area: EdgeRect) -> EdgeRect {
    let width = rect.width.min(work_area.width).max(1);
    let height = rect.height.min(work_area.height).max(1);
    let wa_x = i64::from(work_area.x);
    let wa_y = i64::from(work_area.y);
    let x = clamp_axis(i64::from(rect.x), wa_x, work_area.right(), width);
    let y = clamp_axis(i64::from(rect.y), wa_y, work_area.bottom(), height);
    let (x, y) = match edge {
        DesktopEdge::Top => (x, work_area.y),
        DesktopEdge::Bottom => (x, (work_area.bottom() - i64::from(height)) as i32),
        DesktopEdge::Left => (work_area.x, y),
        DesktopEdge::Right => ((work_area.right() - i64::from(width)) as i32, y),
    };
    EdgeRect {
        x,
        y,
        width,
        height,
    }
}

/// 收起态窄条：沿停靠边，厚度 `round(6 * scale)` 物理像素，长度等于框在该边方向的长度。
pub fn strip(snapped: EdgeRect, edge: DesktopEdge, work_area: EdgeRect, scale: f64) -> EdgeRect {
    let thickness = (STRIP_LOGICAL_THICKNESS * valid_scale_factor(scale))
        .round()
        .max(1.0) as u32;
    match edge {
        DesktopEdge::Top => EdgeRect {
            x: snapped.x,
            y: work_area.y,
            width: snapped.width,
            height: thickness,
        },
        DesktopEdge::Bottom => EdgeRect {
            x: snapped.x,
            y: (work_area.bottom() - i64::from(thickness)) as i32,
            width: snapped.width,
            height: thickness,
        },
        DesktopEdge::Left => EdgeRect {
            x: work_area.x,
            y: snapped.y,
            width: thickness,
            height: snapped.height,
        },
        DesktopEdge::Right => EdgeRect {
            x: (work_area.right() - i64::from(thickness)) as i32,
            y: snapped.y,
            width: thickness,
            height: snapped.height,
        },
    }
}

/// 离矩形最近的工作区边缘；并列时优先保留上一次的边，否则按 上 > 左 > 右 > 下。
pub fn nearest_edge(
    rect: EdgeRect,
    work_area: EdgeRect,
    previous_edge: Option<DesktopEdge>,
) -> DesktopEdge {
    let distances = [
        (
            DesktopEdge::Top,
            (i64::from(rect.y) - i64::from(work_area.y)).abs(),
        ),
        (
            DesktopEdge::Left,
            (i64::from(rect.x) - i64::from(work_area.x)).abs(),
        ),
        (DesktopEdge::Right, (work_area.right() - rect.right()).abs()),
        (
            DesktopEdge::Bottom,
            (work_area.bottom() - rect.bottom()).abs(),
        ),
    ];
    let minimum = distances
        .iter()
        .map(|(_, distance)| *distance)
        .min()
        .unwrap_or(0);
    if let Some(previous) = previous_edge {
        if distances
            .iter()
            .any(|(edge, distance)| *edge == previous && *distance == minimum)
        {
            return previous;
        }
    }
    distances
        .iter()
        .find(|(_, distance)| *distance == minimum)
        .map(|(edge, _)| *edge)
        .unwrap_or(DesktopEdge::Top)
}

fn rect_contains_point(rect: EdgeRect, x: i32, y: i32, slop: i64) -> bool {
    let x = i64::from(x);
    let y = i64::from(y);
    x >= i64::from(rect.x) - slop
        && x < rect.right() + slop
        && y >= i64::from(rect.y) - slop
        && y < rect.bottom() + slop
}

fn resolve_edge(
    config: &DesktopEdgeHideConfig,
    rect: EdgeRect,
    work_area: EdgeRect,
) -> DesktopEdge {
    config
        .mode
        .fixed_edge()
        .unwrap_or_else(|| nearest_edge(rect, work_area, config.edge))
}

// ---------- 锁与运行时 ----------

fn edge_hide_lock() -> Result<MutexGuard<'static, ()>, String> {
    EDGE_HIDE_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| "贴边隐藏锁已损坏".to_owned())
}

fn runtime_map() -> MutexGuard<'static, HashMap<String, EdgeHideRuntime>> {
    let map = EDGE_HIDE_RUNTIME.get_or_init(|| Mutex::new(HashMap::new()));
    match map.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn runtime_get(label: &str) -> Option<EdgeHideRuntime> {
    runtime_map().get(label).copied()
}

fn runtime_set(label: &str, runtime: EdgeHideRuntime) {
    runtime_map().insert(label.to_owned(), runtime);
}

fn runtime_remove(label: &str) {
    runtime_map().remove(label);
}

/// 收起中的窗口必须保持不可缩放，`apply_desktop_window_state` 用它避免把窄条改回可缩放。
pub fn is_collapsed(label: &str) -> bool {
    runtime_get(label).is_some_and(|runtime| runtime.collapsed)
}

/// 分类重排后让运行时状态跟随分类移动。
pub fn remap_runtime_after_category_reorder(
    from_index: usize,
    to_index: usize,
) -> Result<(), String> {
    let _guard = edge_hide_lock()?;
    let mut map = runtime_map();
    let previous = std::mem::take(&mut *map);
    *map = previous
        .into_iter()
        .map(|(label, runtime)| {
            let mapped =
                remap_desktop_category_window_label(&label, from_index, to_index).unwrap_or(label);
            (mapped, runtime)
        })
        .collect();
    Ok(())
}

fn load_edge_hide_config(label: &str) -> DesktopEdgeHideConfig {
    AppStore::open()
        .ok()
        .and_then(|store| {
            store
                .load_config()
                .desktop_layout
                .edge_hide
                .get(label)
                .cloned()
        })
        .unwrap_or_default()
}

// ---------- 窗口几何 ----------

fn window_rect(window: &WebviewWindow) -> Result<EdgeRect, String> {
    let position = window.outer_position().map_err(to_message)?;
    let size = window.inner_size().map_err(to_message)?;
    Ok(EdgeRect {
        x: position.x,
        y: position.y,
        width: size.width,
        height: size.height,
    })
}

fn window_work_area(window: &WebviewWindow) -> Option<(EdgeRect, f64)> {
    let work_area =
        desktop_window_target_work_area(window.app_handle(), window.label()).or_else(|| {
            window
                .current_monitor()
                .ok()
                .flatten()
                .as_ref()
                .map(DesktopMonitorWorkArea::from_monitor)
        })?;
    Some((
        EdgeRect {
            x: work_area.x,
            y: work_area.y,
            width: work_area.width,
            height: work_area.height,
        },
        valid_scale_factor(work_area.scale_factor),
    ))
}

fn is_expanded_size(rect: EdgeRect) -> bool {
    is_valid_desktop_window_layout(&crate::models::DesktopWindowLayout {
        width: rect.width,
        height: rect.height,
        ..Default::default()
    })
}

fn set_window_rect(window: &WebviewWindow, rect: EdgeRect) -> Result<(), String> {
    window
        .set_position(Position::Physical(PhysicalPosition::new(rect.x, rect.y)))
        .map_err(to_message)?;
    window
        .set_size(Size::Physical(PhysicalSize::new(rect.width, rect.height)))
        .map_err(to_message)
}

fn restore_min_size(window: &WebviewWindow) -> Result<(), String> {
    let (min_width, min_height) = desktop_window_min_logical_size(window.label());
    window
        .set_min_size(Some(Size::Logical(LogicalSize::new(
            f64::from(min_width),
            f64::from(min_height),
        ))))
        .map_err(to_message)
}

fn apply_collapsed_geometry(window: &WebviewWindow, strip_rect: EdgeRect) -> Result<(), String> {
    // 去掉无边框窗口边缘的缩放热区，否则整条窄条都是缩放边，WebView 收不到鼠标。
    window.set_resizable(false).map_err(to_message)?;
    // Windows 会按最小跟踪尺寸把窄条撑大。
    window
        .set_min_size(Some(Size::Physical(PhysicalSize::new(1, 1))))
        .map_err(to_message)?;
    window
        .set_size(Size::Physical(PhysicalSize::new(
            strip_rect.width,
            strip_rect.height,
        )))
        .map_err(to_message)?;
    window
        .set_position(Position::Physical(PhysicalPosition::new(
            strip_rect.x,
            strip_rect.y,
        )))
        .map_err(to_message)?;
    keep_desktop_window_behind_apps(window);

    if let (Ok(position), Ok(size)) = (window.outer_position(), window.outer_size()) {
        if size.width != strip_rect.width
            || size.height != strip_rect.height
            || position.x != strip_rect.x
            || position.y != strip_rect.y
        {
            append_desktop_diagnostic(
                "edge-hide-collapse-mismatch",
                &format!(
                    "label={} target={},{} {}x{} actual={},{} {}x{}",
                    window.label(),
                    strip_rect.x,
                    strip_rect.y,
                    strip_rect.width,
                    strip_rect.height,
                    position.x,
                    position.y,
                    size.width,
                    size.height
                ),
            );
        }
    }
    Ok(())
}

fn apply_expanded_geometry(
    window: &WebviewWindow,
    rect: EdgeRect,
    locked: bool,
) -> Result<(), String> {
    set_window_rect(window, rect)?;
    restore_min_size(window)?;
    window.set_resizable(!locked).map_err(to_message)?;
    keep_desktop_window_behind_apps(window);
    Ok(())
}

fn desktop_locked() -> bool {
    load_desktop_window_state()
        .map(|state| state.locked)
        .unwrap_or(false)
}

fn desktop_click_through() -> bool {
    load_desktop_window_state()
        .map(|state| state.click_through)
        .unwrap_or(false)
}

fn build_state(
    window: &WebviewWindow,
    config: &DesktopEdgeHideConfig,
) -> Result<DesktopEdgeHideState, String> {
    let runtime = runtime_get(window.label());
    let expanded = match runtime {
        Some(runtime) => runtime.expanded,
        None => window_rect(window)?,
    };
    let scale_factor = window_work_area(window)
        .map(|(_, scale)| scale)
        .unwrap_or_else(|| valid_scale_factor(window.scale_factor().unwrap_or(1.0)));
    Ok(DesktopEdgeHideState {
        label: window.label().to_owned(),
        enabled: config.enabled,
        mode: config.mode,
        edge: config.edge,
        collapsed: config.enabled && runtime.is_some_and(|runtime| runtime.collapsed),
        expanded,
        scale_factor,
    })
}

fn emit_state(window: &WebviewWindow, state: &DesktopEdgeHideState) {
    let _ = window
        .app_handle()
        .emit(DESKTOP_EDGE_HIDE_CHANGED_EVENT, state.clone());
}

/// 把贴边后的展开矩形和停靠边一起写进配置。调用方必须已持有贴边锁，且窗口已处于该矩形。
fn persist_layout_and_config(
    window: &WebviewWindow,
    config: &DesktopEdgeHideConfig,
) -> Result<(), String> {
    let label = window.label().to_owned();
    let layout = capture_desktop_window_layout(window).map_err(to_message)?;
    let layout = is_valid_desktop_window_layout(&layout).then_some(layout);
    let config = config.clone();
    with_config_mutation(|| {
        let store = AppStore::open().map_err(to_message)?;
        let mut app_config = store.load_config_strict().map_err(to_message)?;
        if let Some(layout) = layout {
            app_config
                .desktop_layout
                .windows
                .insert(label.clone(), layout);
        }
        app_config.desktop_layout.edge_hide.insert(label, config);
        store.save_config(&app_config).map_err(to_message)
    })
}

/// 读取当前展开矩形、解析停靠边并贴边。返回贴边后的矩形以及是否有变化。
/// 调用方必须持有贴边锁，窗口必须处于展开态。
fn commit_expanded_rect(
    window: &WebviewWindow,
    config: &mut DesktopEdgeHideConfig,
    fallback: Option<EdgeRect>,
) -> Result<Option<(EdgeRect, bool)>, String> {
    let current = window_rect(window)?;
    let rect = if is_expanded_size(current) {
        current
    } else if let Some(fallback) = fallback {
        fallback
    } else {
        return Ok(None);
    };
    let Some((work_area, _)) = window_work_area(window) else {
        return Ok(None);
    };
    let edge = resolve_edge(config, rect, work_area);
    let snapped = snap(rect, edge, work_area);
    if snapped != current {
        set_window_rect(window, snapped)?;
    }
    let previous = runtime_get(window.label()).map(|runtime| runtime.expanded);
    let changed = snapped != current || config.edge != Some(edge) || previous != Some(snapped);
    config.edge = Some(edge);
    if changed {
        persist_layout_and_config(window, config)?;
    }
    runtime_set(
        window.label(),
        EdgeHideRuntime {
            collapsed: false,
            expanded: snapped,
        },
    );
    Ok(Some((snapped, changed)))
}

// ---------- 主流程挂钩 ----------

/// 放置路径（`show_desktop_background_window`）在 `apply_desktop_window_state` 之后、`show()` 之前调用。
/// 以刚放好的矩形为展开矩形重新贴边；运行时为收起或没有记录时应用窄条几何。
pub fn reapply_after_placement(window: &WebviewWindow) -> Result<(), String> {
    let label = window.label().to_owned();
    if !is_desktop_card_window_label(&label) {
        return Ok(());
    }
    let _guard = edge_hide_lock()?;
    let config = load_edge_hide_config(&label);
    let runtime = runtime_get(&label);

    if !config.enabled {
        if runtime.is_some() {
            runtime_remove(&label);
            restore_min_size(window)?;
            window
                .set_resizable(!desktop_locked())
                .map_err(to_message)?;
        }
        return Ok(());
    }

    let current = window_rect(window)?;
    let rect = if is_expanded_size(current) {
        current
    } else if let Some(runtime) = runtime {
        runtime.expanded
    } else {
        return Ok(());
    };
    let Some((work_area, scale)) = window_work_area(window) else {
        return Ok(());
    };
    let edge = config
        .mode
        .fixed_edge()
        .or(config.edge)
        .unwrap_or_else(|| nearest_edge(rect, work_area, None));
    let snapped = snap(rect, edge, work_area);
    let collapsed = runtime.map_or(true, |runtime| runtime.collapsed) || desktop_click_through();
    if collapsed {
        apply_collapsed_geometry(window, strip(snapped, edge, work_area, scale))?;
    } else {
        apply_expanded_geometry(window, snapped, desktop_locked())?;
    }
    runtime_set(
        &label,
        EdgeHideRuntime {
            collapsed,
            expanded: snapped,
        },
    );
    let mut config = config;
    config.edge = Some(edge);
    let state = build_state(window, &config)?;
    emit_state(window, &state);
    Ok(())
}

/// `save_desktop_window_layout` 对已开启贴边的 label 走这里：等鼠标松开后重新贴边并保存。
/// 返回 `true` 表示已处理（调用方不要再按普通流程保存）。
pub fn commit_user_layout_change(app: &tauri::AppHandle, label: &str) -> Result<bool, String> {
    if !load_edge_hide_config(label).enabled {
        return Ok(false);
    }
    if is_collapsed(label) {
        return Ok(true);
    }

    // 等待主键松开时不得持锁。
    let deadline = Instant::now() + PRIMARY_RELEASE_TIMEOUT;
    while primary_button_down() && Instant::now() < deadline {
        std::thread::sleep(PRIMARY_RELEASE_POLL);
    }

    let _guard = edge_hide_lock()?;
    let mut config = load_edge_hide_config(label);
    if !config.enabled {
        return Ok(false);
    }
    if is_collapsed(label) {
        return Ok(true);
    }
    let Some(window) = app.get_webview_window(label) else {
        return Ok(true);
    };
    if window.is_maximized().unwrap_or(false) {
        return Ok(true);
    }
    if let Some((_, true)) = commit_expanded_rect(&window, &mut config, None)? {
        let state = build_state(&window, &config)?;
        emit_state(&window, &state);
    }
    Ok(true)
}

// ---------- 命令实现 ----------

fn desktop_window(app: &tauri::AppHandle, label: &str) -> Result<WebviewWindow, String> {
    if !is_desktop_card_window_label(label) {
        return Err("只有桌面框支持贴边隐藏".to_owned());
    }
    app.get_webview_window(label)
        .ok_or_else(|| "桌面框窗口不存在".to_owned())
}

fn get_impl(app: &tauri::AppHandle, label: &str) -> Result<DesktopEdgeHideState, String> {
    let window = desktop_window(app, label)?;
    let _guard = edge_hide_lock()?;
    build_state(&window, &load_edge_hide_config(label))
}

fn set_impl(
    app: &tauri::AppHandle,
    label: &str,
    enabled: bool,
    mode: DesktopEdgeHideMode,
) -> Result<DesktopEdgeHideState, String> {
    let window = desktop_window(app, label)?;
    let _guard = edge_hide_lock()?;
    let mut config = load_edge_hide_config(label);
    config.enabled = enabled;
    config.mode = mode;
    let runtime = runtime_get(label);
    let locked = desktop_locked();

    if enabled {
        let collapsed_expanded = runtime
            .filter(|runtime| runtime.collapsed)
            .map(|runtime| runtime.expanded);
        if let Some(expanded) = collapsed_expanded {
            // 切换停靠边时先展开，再按新边贴边。
            apply_expanded_geometry(&window, expanded, locked)?;
        }
        let current = window_rect(&window)?;
        let rect = if is_expanded_size(current) {
            current
        } else {
            collapsed_expanded.unwrap_or(current)
        };
        let (work_area, _) =
            window_work_area(&window).ok_or_else(|| "无法确定桌面框所在的工作区".to_owned())?;
        let edge = resolve_edge(&config, rect, work_area);
        let snapped = snap(rect, edge, work_area);
        apply_expanded_geometry(&window, snapped, locked)?;
        config.edge = Some(edge);
        runtime_set(
            label,
            EdgeHideRuntime {
                collapsed: false,
                expanded: snapped,
            },
        );
        persist_layout_and_config(&window, &config)?;
    } else {
        match runtime {
            Some(runtime) if runtime.collapsed => {
                apply_expanded_geometry(&window, runtime.expanded, locked)?;
            }
            _ => {
                restore_min_size(&window)?;
                window.set_resizable(!locked).map_err(to_message)?;
            }
        }
        runtime_remove(label);
        let config = config.clone();
        let label = label.to_owned();
        with_config_mutation(|| {
            let store = AppStore::open().map_err(to_message)?;
            let mut app_config = store.load_config_strict().map_err(to_message)?;
            app_config.desktop_layout.edge_hide.insert(label, config);
            store.save_config(&app_config).map_err(to_message)
        })?;
    }

    let state = build_state(&window, &config)?;
    emit_state(&window, &state);
    Ok(state)
}

fn expand_impl(app: &tauri::AppHandle, label: &str) -> Result<DesktopEdgeHideState, String> {
    let window = desktop_window(app, label)?;
    let _guard = edge_hide_lock()?;
    let config = load_edge_hide_config(label);
    if !config.enabled || desktop_click_through() {
        return build_state(&window, &config);
    }
    match runtime_get(label) {
        Some(runtime) if runtime.collapsed => {
            apply_expanded_geometry(&window, runtime.expanded, desktop_locked())?;
            runtime_set(
                label,
                EdgeHideRuntime {
                    collapsed: false,
                    expanded: runtime.expanded,
                },
            );
        }
        Some(_) => {}
        None => {
            let current = window_rect(&window)?;
            if is_expanded_size(current) {
                runtime_set(
                    label,
                    EdgeHideRuntime {
                        collapsed: false,
                        expanded: current,
                    },
                );
            }
        }
    }
    build_state(&window, &config)
}

fn collapse_impl(app: &tauri::AppHandle, label: &str) -> Result<DesktopEdgeHideState, String> {
    let window = desktop_window(app, label)?;
    let _guard = edge_hide_lock()?;
    let mut config = load_edge_hide_config(label);
    if !config.enabled || is_collapsed(label) {
        return build_state(&window, &config);
    }
    // 用户可能正在拖动或缩放本窗口。
    if primary_button_down() {
        return build_state(&window, &config);
    }
    let fallback = runtime_get(label).map(|runtime| runtime.expanded);
    // 先提交当前展开矩形，避免拖完立刻离开导致保存被跳过。
    let Some((snapped, _)) = commit_expanded_rect(&window, &mut config, fallback)? else {
        return build_state(&window, &config);
    };
    let Some((work_area, scale)) = window_work_area(&window) else {
        return build_state(&window, &config);
    };
    let edge = config
        .edge
        .unwrap_or_else(|| nearest_edge(snapped, work_area, None));
    apply_collapsed_geometry(&window, strip(snapped, edge, work_area, scale))?;
    runtime_set(
        label,
        EdgeHideRuntime {
            collapsed: true,
            expanded: snapped,
        },
    );
    build_state(&window, &config)
}

fn pointer_impl(app: &tauri::AppHandle, label: &str) -> Result<DesktopEdgeHidePointer, String> {
    let window = desktop_window(app, label)?;
    let position = window.outer_position().map_err(to_message)?;
    let size = window.outer_size().map_err(to_message)?;
    let rect = EdgeRect {
        x: position.x,
        y: position.y,
        width: size.width,
        height: size.height,
    };
    let inside = cursor_position()
        .map(|(x, y)| rect_contains_point(rect, x, y, POINTER_SLOP))
        .unwrap_or(false);
    Ok(DesktopEdgeHidePointer {
        inside,
        primary_button_down: primary_button_down(),
    })
}

// ---------- Win32 ----------

#[cfg(windows)]
fn cursor_position() -> Option<(i32, i32)> {
    use windows_sys::Win32::{Foundation::POINT, UI::WindowsAndMessaging::GetCursorPos};

    let mut point = POINT { x: 0, y: 0 };
    (unsafe { GetCursorPos(&mut point) } != 0).then_some((point.x, point.y))
}

#[cfg(not(windows))]
fn cursor_position() -> Option<(i32, i32)> {
    None
}

#[cfg(windows)]
fn primary_button_down() -> bool {
    use windows_sys::Win32::UI::{
        Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_RBUTTON},
        WindowsAndMessaging::{GetSystemMetrics, SM_SWAPBUTTON},
    };

    let swapped = unsafe { GetSystemMetrics(SM_SWAPBUTTON) } != 0;
    let key = if swapped { VK_RBUTTON } else { VK_LBUTTON };
    (unsafe { GetAsyncKeyState(i32::from(key)) } as u16 & 0x8000) != 0
}

#[cfg(not(windows))]
fn primary_button_down() -> bool {
    false
}

// ---------- 命令 ----------

#[tauri::command]
pub async fn get_desktop_edge_hide(
    app: tauri::AppHandle,
    label: String,
) -> Result<DesktopEdgeHideState, String> {
    tauri::async_runtime::spawn_blocking(move || get_impl(&app, &label))
        .await
        .map_err(to_message)?
}

#[tauri::command]
pub async fn set_desktop_edge_hide(
    app: tauri::AppHandle,
    label: String,
    enabled: bool,
    mode: DesktopEdgeHideMode,
) -> Result<DesktopEdgeHideState, String> {
    tauri::async_runtime::spawn_blocking(move || set_impl(&app, &label, enabled, mode))
        .await
        .map_err(to_message)?
}

#[tauri::command]
pub async fn expand_desktop_edge_hide(
    app: tauri::AppHandle,
    label: String,
) -> Result<DesktopEdgeHideState, String> {
    tauri::async_runtime::spawn_blocking(move || expand_impl(&app, &label))
        .await
        .map_err(to_message)?
}

#[tauri::command]
pub async fn collapse_desktop_edge_hide(
    app: tauri::AppHandle,
    label: String,
) -> Result<DesktopEdgeHideState, String> {
    tauri::async_runtime::spawn_blocking(move || collapse_impl(&app, &label))
        .await
        .map_err(to_message)?
}

#[tauri::command]
pub async fn desktop_edge_hide_pointer(
    app: tauri::AppHandle,
    label: String,
) -> Result<DesktopEdgeHidePointer, String> {
    tauri::async_runtime::spawn_blocking(move || pointer_impl(&app, &label))
        .await
        .map_err(to_message)?
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORK_AREA: EdgeRect = EdgeRect {
        x: 0,
        y: 0,
        width: 1920,
        height: 1040,
    };

    fn rect(x: i32, y: i32, width: u32, height: u32) -> EdgeRect {
        EdgeRect {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn snap_moves_perpendicular_axis_and_keeps_along_axis() {
        let source = rect(300, 200, 360, 260);
        assert_eq!(
            snap(source, DesktopEdge::Top, WORK_AREA),
            rect(300, 0, 360, 260)
        );
        assert_eq!(
            snap(source, DesktopEdge::Bottom, WORK_AREA),
            rect(300, 780, 360, 260)
        );
        assert_eq!(
            snap(source, DesktopEdge::Left, WORK_AREA),
            rect(0, 200, 360, 260)
        );
        assert_eq!(
            snap(source, DesktopEdge::Right, WORK_AREA),
            rect(1560, 200, 360, 260)
        );
    }

    #[test]
    fn snap_clamps_along_axis_and_size_into_work_area() {
        assert_eq!(
            snap(rect(1800, -50, 360, 260), DesktopEdge::Top, WORK_AREA),
            rect(1560, 0, 360, 260)
        );
        assert_eq!(
            snap(rect(-80, 900, 360, 260), DesktopEdge::Left, WORK_AREA),
            rect(0, 780, 360, 260)
        );
        assert_eq!(
            snap(rect(10, 10, 4000, 2000), DesktopEdge::Bottom, WORK_AREA),
            rect(0, 0, 1920, 1040)
        );
    }

    #[test]
    fn snap_respects_offset_work_area() {
        let work_area = rect(-1280, 40, 1280, 984);
        assert_eq!(
            snap(rect(-900, 300, 320, 260), DesktopEdge::Right, work_area),
            rect(-320, 300, 320, 260)
        );
        assert_eq!(
            snap(rect(-900, 300, 320, 260), DesktopEdge::Top, work_area),
            rect(-900, 40, 320, 260)
        );
    }

    #[test]
    fn strip_uses_scaled_thickness_along_edge() {
        let snapped = rect(300, 0, 360, 260);
        assert_eq!(
            strip(snapped, DesktopEdge::Top, WORK_AREA, 1.0),
            rect(300, 0, 360, 6)
        );
        assert_eq!(
            strip(
                rect(300, 780, 360, 260),
                DesktopEdge::Bottom,
                WORK_AREA,
                1.5
            ),
            rect(300, 1031, 360, 9)
        );
        assert_eq!(
            strip(rect(0, 200, 360, 260), DesktopEdge::Left, WORK_AREA, 1.25),
            rect(0, 200, 8, 260)
        );
        assert_eq!(
            strip(
                rect(1560, 200, 360, 260),
                DesktopEdge::Right,
                WORK_AREA,
                2.0
            ),
            rect(1908, 200, 12, 260)
        );
    }

    #[test]
    fn nearest_edge_picks_minimum_distance() {
        assert_eq!(
            nearest_edge(rect(20, 400, 360, 260), WORK_AREA, None),
            DesktopEdge::Left
        );
        assert_eq!(
            nearest_edge(rect(1550, 400, 360, 260), WORK_AREA, None),
            DesktopEdge::Right
        );
        assert_eq!(
            nearest_edge(rect(800, 770, 360, 260), WORK_AREA, None),
            DesktopEdge::Bottom
        );
        assert_eq!(
            nearest_edge(rect(800, 5, 360, 260), WORK_AREA, None),
            DesktopEdge::Top
        );
    }

    #[test]
    fn nearest_edge_breaks_ties_with_previous_then_fixed_order() {
        // 左上角：上、左距离都为 0。
        let corner = rect(0, 0, 360, 260);
        assert_eq!(nearest_edge(corner, WORK_AREA, None), DesktopEdge::Top);
        assert_eq!(
            nearest_edge(corner, WORK_AREA, Some(DesktopEdge::Left)),
            DesktopEdge::Left
        );
        assert_eq!(
            nearest_edge(corner, WORK_AREA, Some(DesktopEdge::Bottom)),
            DesktopEdge::Top
        );
        // 右下角：右、下并列时按 右 > 下。
        let bottom_right = rect(1560, 780, 360, 260);
        assert_eq!(
            nearest_edge(bottom_right, WORK_AREA, None),
            DesktopEdge::Right
        );
        assert_eq!(
            nearest_edge(bottom_right, WORK_AREA, Some(DesktopEdge::Bottom)),
            DesktopEdge::Bottom
        );
    }

    #[test]
    fn pointer_hit_test_uses_slop() {
        let window = rect(100, 0, 360, 6);
        assert!(rect_contains_point(window, 100, 0, POINTER_SLOP));
        assert!(rect_contains_point(window, 98, 7, POINTER_SLOP));
        assert!(!rect_contains_point(window, 97, 3, POINTER_SLOP));
        assert!(!rect_contains_point(window, 200, 8, POINTER_SLOP));
    }

    #[test]
    fn edge_hide_config_serializes_lowercase() {
        let config = DesktopEdgeHideConfig {
            enabled: true,
            mode: DesktopEdgeHideMode::Auto,
            edge: Some(DesktopEdge::Right),
        };
        let json = serde_json::to_string(&config).expect("序列化应成功");
        assert_eq!(json, r#"{"enabled":true,"mode":"auto","edge":"right"}"#);
        let parsed: DesktopEdgeHideConfig =
            serde_json::from_str(r#"{"enabled":true,"mode":"left"}"#).expect("反序列化应成功");
        assert_eq!(parsed.mode, DesktopEdgeHideMode::Left);
        assert_eq!(parsed.edge, None);
    }
}
