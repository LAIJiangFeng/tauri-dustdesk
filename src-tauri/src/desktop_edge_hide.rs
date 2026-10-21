//! 桌面收纳框贴边自动隐藏。
//!
//! 后端是几何与状态的唯一权威：持久化配置在 `DesktopLayout.EdgeHide`，
//! 运行时只记录每个 label 是否收起以及展开矩形。窄条几何永远不写进布局。
//!
//! 同一个框在各显示器上的副本（`-monitor-<id>` 后缀）共用开关与停靠模式：没有自己记录的副本
//! 沿用主屏或其它副本已开启的设置，停靠边由各副本按自己的显示器重新解析。
//!
//! 锁顺序固定为 `LAZY_WINDOW_OPERATION_LOCK` → `EDGE_HIDE_LOCK` → `CONFIG_MUTATION_LOCK`。
//! 运行时表 `EDGE_HIDE_RUNTIME` 是叶子锁，只做短暂读写。

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
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
    models::{DesktopEdge, DesktopEdgeHideConfig, DesktopEdgeHideMode, DesktopWindowLayout},
    primary_desktop_window_label_for, remap_desktop_category_window_label,
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

/// 解析自动停靠边所需的显示器几何，全部为物理像素。
#[derive(Debug, Clone, PartialEq)]
pub struct MonitorGeometry {
    /// 本副本所在显示器的完整矩形。
    pub monitor: EdgeRect,
    /// 本显示器的工作区（扣除任务栏）。
    pub work_area: EdgeRect,
    /// 其它显示器的完整矩形。
    pub others: Vec<EdgeRect>,
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
/// 运行时自动模式走 [`auto_edge`]，这里保留四边规则作为单屏行为的基准。
#[cfg(test)]
pub fn nearest_edge(
    rect: EdgeRect,
    work_area: EdgeRect,
    previous_edge: Option<DesktopEdge>,
) -> DesktopEdge {
    nearest_edge_among(rect, work_area, previous_edge, &ALL_EDGES)
}

const ALL_EDGES: [DesktopEdge; 4] = [
    DesktopEdge::Top,
    DesktopEdge::Left,
    DesktopEdge::Right,
    DesktopEdge::Bottom,
];

/// 只在 `candidates` 里选最近的边，并列规则同 [`nearest_edge`]。`candidates` 为空时退回四边。
fn nearest_edge_among(
    rect: EdgeRect,
    work_area: EdgeRect,
    previous_edge: Option<DesktopEdge>,
    candidates: &[DesktopEdge],
) -> DesktopEdge {
    let candidates = if candidates.is_empty() {
        &ALL_EDGES[..]
    } else {
        candidates
    };
    let all_distances = [
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
    let distances = all_distances
        .into_iter()
        .filter(|(edge, _)| candidates.contains(edge))
        .collect::<Vec<_>>();
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

fn spans_overlap(start: i64, end: i64, other_start: i64, other_end: i64) -> bool {
    start < other_end && other_start < end
}

/// 鼠标推得到的「自然停靠边」：该边的工作区边缘与显示器边缘重合（任务栏所在边不算），
/// 且沿该边、在框贴边后所占的那段范围内紧挨着没有其它显示器。
///
/// 相接部分只覆盖框的一部分时也不算自然边：窄条与框等长，鼠标从相接的那一段靠近会直接
/// 越到另一块屏幕，所以只要沿边范围有任何重叠就排除；只有角点相接（重叠长度为 0）不影响。
pub fn natural_edges(rect: EdgeRect, geometry: &MonitorGeometry) -> Vec<DesktopEdge> {
    let monitor = geometry.monitor;
    let work_area = geometry.work_area;
    ALL_EDGES
        .into_iter()
        .filter(|edge| {
            let flush = match edge {
                DesktopEdge::Top => work_area.y == monitor.y,
                DesktopEdge::Bottom => work_area.bottom() == monitor.bottom(),
                DesktopEdge::Left => work_area.x == monitor.x,
                DesktopEdge::Right => work_area.right() == monitor.right(),
            };
            if !flush {
                return false;
            }
            let span = snap(rect, *edge, work_area);
            !geometry.others.iter().any(|other| {
                let (beyond, overlap) = match edge {
                    DesktopEdge::Top => (
                        other.y < monitor.y && other.bottom() >= i64::from(monitor.y),
                        spans_overlap(span.x.into(), span.right(), other.x.into(), other.right()),
                    ),
                    DesktopEdge::Bottom => (
                        other.bottom() > monitor.bottom() && i64::from(other.y) <= monitor.bottom(),
                        spans_overlap(span.x.into(), span.right(), other.x.into(), other.right()),
                    ),
                    DesktopEdge::Left => (
                        other.x < monitor.x && other.right() >= i64::from(monitor.x),
                        spans_overlap(span.y.into(), span.bottom(), other.y.into(), other.bottom()),
                    ),
                    DesktopEdge::Right => (
                        other.right() > monitor.right() && i64::from(other.x) <= monitor.right(),
                        spans_overlap(span.y.into(), span.bottom(), other.y.into(), other.bottom()),
                    ),
                };
                beyond && overlap
            })
        })
        .collect()
}

/// 自动模式：优先在自然停靠边里选最近的；一条都没有时退回四边最近。
pub fn auto_edge(
    rect: EdgeRect,
    geometry: &MonitorGeometry,
    previous_edge: Option<DesktopEdge>,
) -> DesktopEdge {
    nearest_edge_among(
        rect,
        geometry.work_area,
        previous_edge,
        &natural_edges(rect, geometry),
    )
}

/// 固定边模式完全按用户选择；自动模式见 [`auto_edge`]。
fn resolve_edge(
    config: &DesktopEdgeHideConfig,
    rect: EdgeRect,
    geometry: &MonitorGeometry,
) -> DesktopEdge {
    config
        .mode
        .fixed_edge()
        .unwrap_or_else(|| auto_edge(rect, geometry, config.edge))
}

// ---------- 跨显示器副本 ----------

/// 同一个框在各显示器上的副本共用的键：主屏 label。
fn edge_hide_group_key(label: &str) -> String {
    primary_desktop_window_label_for(label).unwrap_or_else(|| label.to_owned())
}

/// 有效配置：自己有记录用自己的；否则先看主屏 label、再按 label 顺序看其它副本，
/// 沿用第一个已开启的 `enabled` 与 `mode`，停靠边清空由本副本重新解析（固定边模式直接用该边）；
/// 都没有则默认关闭。
pub fn effective_edge_hide_config(
    records: &BTreeMap<String, DesktopEdgeHideConfig>,
    label: &str,
) -> DesktopEdgeHideConfig {
    if let Some(own) = records.get(label) {
        return own.clone();
    }
    let key = edge_hide_group_key(label);
    let primary = records.get(&key).filter(|_| key != label);
    let others = records
        .iter()
        .filter(|(other, _)| {
            other.as_str() != label && **other != key && edge_hide_group_key(other) == key
        })
        .map(|(_, config)| config);
    primary
        .into_iter()
        .chain(others)
        .find(|config| config.enabled)
        .map(|config| DesktopEdgeHideConfig {
            enabled: true,
            mode: config.mode,
            edge: config.mode.fixed_edge(),
        })
        .unwrap_or_default()
}

/// 同一个框的其它副本 label（不含自己），去重并按 label 排序。
fn edge_hide_copy_labels<'a>(
    label: &str,
    candidates: impl IntoIterator<Item = &'a str>,
) -> Vec<String> {
    let key = edge_hide_group_key(label);
    candidates
        .into_iter()
        .filter(|candidate| *candidate != label && edge_hide_group_key(candidate) == key)
        .map(str::to_owned)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// 用户在别的副本上改了开关或模式后本副本的新配置：自动模式保留自己的停靠边作为并列偏好，
/// 固定边模式直接用该边。
fn copy_edge_hide_config(
    records: &BTreeMap<String, DesktopEdgeHideConfig>,
    label: &str,
    enabled: bool,
    mode: DesktopEdgeHideMode,
) -> DesktopEdgeHideConfig {
    let own_edge = records.get(label).and_then(|config| config.edge);
    DesktopEdgeHideConfig {
        enabled,
        mode,
        edge: mode.fixed_edge().or(own_edge),
    }
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

fn load_edge_hide_records() -> BTreeMap<String, DesktopEdgeHideConfig> {
    AppStore::open()
        .ok()
        .map(|store| store.load_config().desktop_layout.edge_hide)
        .unwrap_or_default()
}

/// 有效配置，见 [`effective_edge_hide_config`]。只读，不会为副本写入记录。
fn load_edge_hide_config(label: &str) -> DesktopEdgeHideConfig {
    effective_edge_hide_config(&load_edge_hide_records(), label)
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

fn monitor_rect(monitor: &tauri::Monitor) -> EdgeRect {
    EdgeRect {
        x: monitor.position().x,
        y: monitor.position().y,
        width: monitor.size().width,
        height: monitor.size().height,
    }
}

fn work_area_rect(work_area: &DesktopMonitorWorkArea) -> EdgeRect {
    EdgeRect {
        x: work_area.x,
        y: work_area.y,
        width: work_area.width,
        height: work_area.height,
    }
}

/// 窗口所属显示器的几何与缩放比。副屏副本按 label 里的显示器 id 取目标显示器，
/// 拿不到（显示器刚断开等）时退回窗口当前所在显示器。
fn window_geometry(window: &WebviewWindow) -> Option<(MonitorGeometry, f64)> {
    let monitors = window.available_monitors().unwrap_or_default();
    let (work_area, monitor) =
        match desktop_window_target_work_area(window.app_handle(), window.label()) {
            Some(work_area) => {
                let monitor = monitors
                    .iter()
                    .find(|monitor| DesktopMonitorWorkArea::from_monitor(monitor) == work_area)
                    .map(monitor_rect)
                    .unwrap_or_else(|| work_area_rect(&work_area));
                (work_area, monitor)
            }
            None => {
                let current = window.current_monitor().ok().flatten()?;
                (
                    DesktopMonitorWorkArea::from_monitor(&current),
                    monitor_rect(&current),
                )
            }
        };
    let others = monitors
        .iter()
        .map(monitor_rect)
        .filter(|other| *other != monitor)
        .collect();
    Some((
        MonitorGeometry {
            monitor,
            work_area: work_area_rect(&work_area),
            others,
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
    // 跨缩放比不同的显示器移动时 Windows 会按新 DPI 重新缩放窗口，位置落定后再校正一次尺寸。
    if window
        .outer_size()
        .is_ok_and(|size| size.width != strip_rect.width || size.height != strip_rect.height)
    {
        window
            .set_size(Size::Physical(PhysicalSize::new(
                strip_rect.width,
                strip_rect.height,
            )))
            .map_err(to_message)?;
    }
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
    let scale_factor = window_geometry(window)
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
    let layout = capture_desktop_window_layout(window).map_err(to_message)?;
    let layout = is_valid_desktop_window_layout(&layout).then_some(layout);
    persist_edge_hide_entries(vec![(window.label().to_owned(), layout, config.clone())])
}

/// 在一次配置写入里保存多条贴边记录及可选布局。布局只能来自展开态窗口。
fn persist_edge_hide_entries(
    entries: Vec<(String, Option<DesktopWindowLayout>, DesktopEdgeHideConfig)>,
) -> Result<(), String> {
    if entries.is_empty() {
        return Ok(());
    }
    with_config_mutation(|| {
        let store = AppStore::open().map_err(to_message)?;
        let mut app_config = store.load_config_strict().map_err(to_message)?;
        for (label, layout, config) in entries.iter().cloned() {
            if let Some(layout) = layout {
                app_config
                    .desktop_layout
                    .windows
                    .insert(label.clone(), layout);
            }
            app_config.desktop_layout.edge_hide.insert(label, config);
        }
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
    let Some((geometry, _)) = window_geometry(window) else {
        return Ok(None);
    };
    let edge = resolve_edge(config, rect, &geometry);
    let snapped = snap(rect, edge, geometry.work_area);
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
    let Some((geometry, scale)) = window_geometry(window) else {
        return Ok(());
    };
    // 自动模式以上次的边为并列偏好重新解析：已贴好的框到原边距离为 0，保持不变；
    // 原来停在两屏相接处的框换到推得到的边；继承来的副本按自己的显示器解析。
    let edge = resolve_edge(&config, rect, &geometry);
    let work_area = geometry.work_area;
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
    let records = load_edge_hide_records();
    let mut config = effective_edge_hide_config(&records, label);
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
        let (geometry, _) =
            window_geometry(&window).ok_or_else(|| "无法确定桌面框所在的工作区".to_owned())?;
        let edge = resolve_edge(&config, rect, &geometry);
        let snapped = snap(rect, edge, geometry.work_area);
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
        persist_edge_hide_entries(vec![(label.to_owned(), None, config.clone())])?;
    }

    let state = build_state(&window, &config)?;
    emit_state(&window, &state);
    sync_copies(app, label, &records, enabled, mode, locked);
    Ok(state)
}

/// 把开关与模式同步给同一个框在其它显示器上的副本：运行中的窗口和配置里已有记录的副本 label。
/// 单个副本失败只记诊断，不影响用户正在操作的窗口。调用方必须持有贴边锁。
fn sync_copies(
    app: &tauri::AppHandle,
    label: &str,
    records: &BTreeMap<String, DesktopEdgeHideConfig>,
    enabled: bool,
    mode: DesktopEdgeHideMode,
    locked: bool,
) {
    let windows = app.webview_windows();
    let copies = edge_hide_copy_labels(
        label,
        windows
            .keys()
            .map(String::as_str)
            .filter(|candidate| is_desktop_card_window_label(candidate))
            .chain(records.keys().map(String::as_str)),
    );
    let mut record_only = Vec::new();
    for copy in copies {
        let mut config = copy_edge_hide_config(records, &copy, enabled, mode);
        let Some(window) = windows.get(&copy) else {
            // 没在运行的副本只更新它已有的记录，下次放置时按自己的显示器解析停靠边。
            record_only.push((copy, None, config));
            continue;
        };
        let result = if enabled {
            enable_copy(window, &mut config, locked)
        } else {
            disable_copy(window, locked)
        };
        if let Err(error) = result {
            append_desktop_diagnostic(
                "edge-hide-sync-failed",
                &format!("source={label} copy={copy} error={error}"),
            );
        }
        // 开启时 `enable_copy` 已随布局写入记录；关闭时只更新已有记录，继承来的副本无需写。
        if !enabled && records.contains_key(&copy) {
            record_only.push((copy.clone(), None, config.clone()));
        }
        if let Ok(state) = build_state(window, &config) {
            emit_state(window, &state);
        }
    }
    if let Err(error) = persist_edge_hide_entries(record_only) {
        append_desktop_diagnostic(
            "edge-hide-sync-failed",
            &format!("source={label} records error={error}"),
        );
    }
}

/// 在运行的副本上开启：按自己显示器的工作区贴边，先在展开态提交并保存布局，再直接切到收起态。
fn enable_copy(
    window: &WebviewWindow,
    config: &mut DesktopEdgeHideConfig,
    locked: bool,
) -> Result<(), String> {
    let label = window.label().to_owned();
    let runtime = runtime_get(&label);
    let current = window_rect(window)?;
    let rect = match runtime {
        Some(runtime) if runtime.collapsed => runtime.expanded,
        _ if is_expanded_size(current) => current,
        Some(runtime) => runtime.expanded,
        // 没有可信的展开矩形，交给下次放置处理，避免把异常小的几何当成展开矩形保存。
        None => return Ok(()),
    };
    let (geometry, scale) =
        window_geometry(window).ok_or_else(|| "无法确定桌面框所在的工作区".to_owned())?;
    let edge = resolve_edge(config, rect, &geometry);
    let snapped = snap(rect, edge, geometry.work_area);
    config.edge = Some(edge);
    if runtime.is_some_and(|runtime| runtime.collapsed && runtime.expanded == snapped) {
        // 展开矩形没变，布局早已保存，只更新配置，不必展开再收起。
        persist_edge_hide_entries(vec![(label.clone(), None, config.clone())])?;
    } else {
        apply_expanded_geometry(window, snapped, locked)?;
        runtime_set(
            &label,
            EdgeHideRuntime {
                collapsed: false,
                expanded: snapped,
            },
        );
        persist_layout_and_config(window, config)?;
    }
    apply_collapsed_geometry(window, strip(snapped, edge, geometry.work_area, scale))?;
    runtime_set(
        &label,
        EdgeHideRuntime {
            collapsed: true,
            expanded: snapped,
        },
    );
    Ok(())
}

/// 在运行的副本上关闭：收起中的先展开到展开矩形，再恢复最小尺寸与可缩放性，清除运行时记录。
fn disable_copy(window: &WebviewWindow, locked: bool) -> Result<(), String> {
    let label = window.label();
    match runtime_get(label) {
        Some(runtime) if runtime.collapsed => {
            apply_expanded_geometry(window, runtime.expanded, locked)?;
        }
        Some(_) => {
            restore_min_size(window)?;
            window.set_resizable(!locked).map_err(to_message)?;
        }
        None => {}
    }
    runtime_remove(label);
    Ok(())
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
    let Some((geometry, scale)) = window_geometry(&window) else {
        return build_state(&window, &config);
    };
    let edge = config
        .edge
        .unwrap_or_else(|| auto_edge(snapped, &geometry, None));
    apply_collapsed_geometry(&window, strip(snapped, edge, geometry.work_area, scale))?;
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

    fn config(
        enabled: bool,
        mode: DesktopEdgeHideMode,
        edge: Option<DesktopEdge>,
    ) -> DesktopEdgeHideConfig {
        DesktopEdgeHideConfig {
            enabled,
            mode,
            edge,
        }
    }

    fn records(
        entries: &[(&str, DesktopEdgeHideConfig)],
    ) -> BTreeMap<String, DesktopEdgeHideConfig> {
        entries
            .iter()
            .map(|(label, config)| ((*label).to_owned(), config.clone()))
            .collect()
    }

    #[test]
    fn effective_config_prefers_own_record() {
        let records = records(&[
            (
                "desktop-category-3",
                config(true, DesktopEdgeHideMode::Auto, Some(DesktopEdge::Left)),
            ),
            (
                "desktop-category-3-monitor-b",
                config(false, DesktopEdgeHideMode::Top, None),
            ),
        ]);
        assert_eq!(
            effective_edge_hide_config(&records, "desktop-category-3-monitor-b"),
            config(false, DesktopEdgeHideMode::Top, None)
        );
        assert_eq!(
            effective_edge_hide_config(&records, "desktop-category-3"),
            config(true, DesktopEdgeHideMode::Auto, Some(DesktopEdge::Left))
        );
    }

    #[test]
    fn effective_config_inherits_from_primary_and_clears_auto_edge() {
        let records = records(&[(
            "desktop-category-3",
            config(true, DesktopEdgeHideMode::Auto, Some(DesktopEdge::Left)),
        )]);
        assert_eq!(
            effective_edge_hide_config(&records, "desktop-category-3-monitor-b"),
            config(true, DesktopEdgeHideMode::Auto, None)
        );
        let records = records_fixed();
        assert_eq!(
            effective_edge_hide_config(&records, "desktop-widget-monitor-z"),
            config(true, DesktopEdgeHideMode::Right, Some(DesktopEdge::Right))
        );
    }

    fn records_fixed() -> BTreeMap<String, DesktopEdgeHideConfig> {
        records(&[(
            "desktop-widget",
            config(true, DesktopEdgeHideMode::Right, Some(DesktopEdge::Right)),
        )])
    }

    #[test]
    fn effective_config_falls_back_to_other_copies_in_label_order() {
        let records = records(&[
            (
                "desktop-launcher",
                config(false, DesktopEdgeHideMode::Auto, None),
            ),
            (
                "desktop-launcher-monitor-c",
                config(true, DesktopEdgeHideMode::Bottom, None),
            ),
            (
                "desktop-launcher-monitor-b",
                config(true, DesktopEdgeHideMode::Left, Some(DesktopEdge::Left)),
            ),
        ]);
        // 主屏关闭不挡住副本：按 label 顺序取第一个已开启的 monitor-b。
        assert_eq!(
            effective_edge_hide_config(&records, "desktop-launcher-monitor-new"),
            config(true, DesktopEdgeHideMode::Left, Some(DesktopEdge::Left))
        );
        // 主屏 label 自己有记录时用自己的。
        assert!(!effective_edge_hide_config(&records, "desktop-launcher").enabled);
        // 主屏没有记录时沿用副屏。
        let secondary_only = records_from_secondary();
        assert_eq!(
            effective_edge_hide_config(&secondary_only, "desktop-category-0"),
            config(true, DesktopEdgeHideMode::Auto, None)
        );
    }

    fn records_from_secondary() -> BTreeMap<String, DesktopEdgeHideConfig> {
        records(&[(
            "desktop-category-0-monitor-a",
            config(true, DesktopEdgeHideMode::Auto, Some(DesktopEdge::Top)),
        )])
    }

    #[test]
    fn effective_config_ignores_other_boxes_and_defaults_off() {
        let records = records(&[
            (
                "desktop-category-1",
                config(true, DesktopEdgeHideMode::Auto, None),
            ),
            (
                "desktop-category-10-monitor-a",
                config(true, DesktopEdgeHideMode::Top, None),
            ),
            (
                "desktop-widget",
                config(true, DesktopEdgeHideMode::Top, None),
            ),
        ]);
        assert_eq!(
            effective_edge_hide_config(&records, "desktop-category-10"),
            config(true, DesktopEdgeHideMode::Top, Some(DesktopEdge::Top))
        );
        assert_eq!(
            effective_edge_hide_config(&records, "desktop-category-2-monitor-a"),
            DesktopEdgeHideConfig::default()
        );
        assert_eq!(
            effective_edge_hide_config(&records, "desktop-launcher-monitor-a"),
            DesktopEdgeHideConfig::default()
        );
    }

    #[test]
    fn copy_labels_cover_same_box_only() {
        let copies = edge_hide_copy_labels(
            "desktop-category-3-monitor-b",
            [
                "desktop-category-3",
                "desktop-category-3-monitor-b",
                "desktop-category-3-monitor-a",
                "desktop-category-3-monitor-a",
                "desktop-category-30-monitor-a",
                "desktop-widget",
            ],
        );
        assert_eq!(
            copies,
            vec!["desktop-category-3", "desktop-category-3-monitor-a"]
        );
        let config = copy_edge_hide_config(
            &records(&[(
                "desktop-category-3-monitor-a",
                config(true, DesktopEdgeHideMode::Auto, Some(DesktopEdge::Right)),
            )]),
            "desktop-category-3-monitor-a",
            true,
            DesktopEdgeHideMode::Auto,
        );
        assert_eq!(config.edge, Some(DesktopEdge::Right));
    }

    fn geometry(monitor: EdgeRect, work_area: EdgeRect, others: &[EdgeRect]) -> MonitorGeometry {
        MonitorGeometry {
            monitor,
            work_area,
            others: others.to_vec(),
        }
    }

    #[test]
    fn auto_edge_avoids_shared_edge_side_by_side() {
        let left = rect(0, 0, 1920, 1080);
        let right = rect(1920, 0, 1920, 1080);
        // 右屏，任务栏在底部；框贴近右屏左侧。
        let on_right = geometry(right, rect(1920, 0, 1920, 1040), &[left]);
        let box_rect = rect(1930, 400, 360, 260);
        assert_eq!(
            nearest_edge(box_rect, on_right.work_area, None),
            DesktopEdge::Left
        );
        assert_eq!(
            natural_edges(box_rect, &on_right),
            vec![DesktopEdge::Top, DesktopEdge::Right]
        );
        assert_eq!(auto_edge(box_rect, &on_right, None), DesktopEdge::Top);
        // 上次停在相接边也不保留。
        assert_eq!(
            auto_edge(box_rect, &on_right, Some(DesktopEdge::Left)),
            DesktopEdge::Top
        );
        // 左屏贴近右侧同理：右边与右屏相接，底边是任务栏，只剩上、左，上更近。
        let on_left = geometry(left, rect(0, 0, 1920, 1040), &[right]);
        assert_eq!(
            auto_edge(rect(1550, 500, 360, 260), &on_left, None),
            DesktopEdge::Top
        );
    }

    #[test]
    fn auto_edge_avoids_shared_edge_when_stacked() {
        let upper = rect(0, -1080, 1920, 1080);
        let lower = rect(0, 0, 1920, 1080);
        let on_lower = geometry(lower, rect(0, 0, 1920, 1040), &[upper]);
        // 下屏顶部与上屏相接，底边是任务栏：在左右里选最近。
        assert_eq!(
            auto_edge(rect(800, 10, 360, 260), &on_lower, None),
            DesktopEdge::Right
        );
        let on_upper = geometry(upper, upper, &[lower]);
        assert_eq!(
            natural_edges(rect(100, -300, 360, 260), &on_upper),
            vec![DesktopEdge::Top, DesktopEdge::Left, DesktopEdge::Right]
        );
        assert_eq!(
            auto_edge(rect(800, -270, 360, 260), &on_upper, None),
            DesktopEdge::Right
        );
    }

    #[test]
    fn auto_edge_partial_neighbour_disqualifies_edge() {
        // 左侧小屏垂直偏移，只与主屏左边的 200..920 相接。
        let small = rect(-1280, 200, 1280, 720);
        let main = rect(0, 0, 2560, 1440);
        let on_main = geometry(main, rect(0, 0, 2560, 1400), &[small]);
        // 框沿边范围 100..360 与 200..920 部分重叠：左边不算自然边。
        assert!(!natural_edges(rect(5, 100, 360, 260), &on_main).contains(&DesktopEdge::Left));
        assert_eq!(
            auto_edge(rect(5, 100, 360, 260), &on_main, None),
            DesktopEdge::Top
        );
        // 框在相接段以下（1000..1260）：左边推得到。
        assert_eq!(
            auto_edge(rect(5, 1000, 360, 260), &on_main, None),
            DesktopEdge::Left
        );
        // 只有角点相接（框底正好在 200）不影响。
        assert!(natural_edges(rect(5, 40, 360, 160), &on_main).contains(&DesktopEdge::Left));
    }

    #[test]
    fn auto_edge_excludes_taskbar_edge() {
        let monitor = rect(0, 0, 1920, 1080);
        let single = geometry(monitor, rect(0, 0, 1920, 1040), &[]);
        let near_bottom = rect(800, 770, 360, 260);
        assert!(!natural_edges(near_bottom, &single).contains(&DesktopEdge::Bottom));
        assert_ne!(auto_edge(near_bottom, &single, None), DesktopEdge::Bottom);
        // 任务栏在左侧时左边不算。
        let left_taskbar = geometry(monitor, rect(48, 0, 1872, 1080), &[]);
        assert_eq!(
            auto_edge(rect(60, 400, 360, 260), &left_taskbar, None),
            DesktopEdge::Top
        );
    }

    #[test]
    fn auto_edge_single_monitor_matches_nearest_edge() {
        let monitor = rect(0, 0, 1920, 1040);
        let single = geometry(monitor, monitor, &[]);
        for (candidate, previous) in [
            (rect(20, 400, 360, 260), None),
            (rect(1550, 400, 360, 260), None),
            (rect(800, 770, 360, 260), None),
            (rect(800, 5, 360, 260), None),
            (rect(0, 0, 360, 260), Some(DesktopEdge::Left)),
            (rect(1560, 780, 360, 260), Some(DesktopEdge::Bottom)),
        ] {
            assert_eq!(
                auto_edge(candidate, &single, previous),
                nearest_edge(candidate, monitor, previous)
            );
        }
        // 没有任何自然边时退回四边最近。
        let boxed_in = geometry(
            rect(0, 0, 1920, 1080),
            rect(0, 0, 1920, 1040),
            &[
                rect(-1920, 0, 1920, 1080),
                rect(1920, 0, 1920, 1080),
                rect(0, -1080, 1920, 1080),
            ],
        );
        assert!(natural_edges(rect(20, 400, 360, 260), &boxed_in).is_empty());
        assert_eq!(
            auto_edge(rect(20, 400, 360, 260), &boxed_in, None),
            DesktopEdge::Left
        );
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
