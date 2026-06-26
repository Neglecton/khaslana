use gpui::{
    Bounds, Context, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    Pixels, Point, ScrollHandle, SharedString, UniformListScrollHandle, Window, canvas, div, fill,
    point, prelude::*, px, rgb, rgba,
};
use khaslana::{ChangeState, DiffLineKind, DiffScope};

use crate::{DiffHeaderTarget, RepositoryView, ui::theme as ui_theme};

pub(crate) use crate::ui::theme::ACCENT as COLOR_BLUE_SOFT;
pub(crate) use crate::ui::theme::BORDER as COLOR_BORDER;
pub(crate) use crate::ui::theme::CARD as COLOR_HEADER_BG;
pub(crate) use crate::ui::theme::CARD as COLOR_PANEL_BG;
pub(crate) use crate::ui::theme::CARD as COLOR_SURFACE;
pub(crate) use crate::ui::theme::FOREGROUND as COLOR_TEXT;
pub(crate) use crate::ui::theme::MUTED_FOREGROUND as COLOR_TEXT_FAINT;
pub(crate) use crate::ui::theme::MUTED_FOREGROUND as COLOR_TEXT_MUTED;
const SCROLLBAR_THICKNESS: f32 = 8.0;
const SCROLLBAR_MARGIN: f32 = 2.0;
const SCROLLBAR_MIN_THUMB: f32 = 28.0;
const REPO_TAB_SCROLL_ID: &str = "repo-tab-bar-scroll";
const REPO_TAB_SCROLLBAR_Y_OFFSET: f32 = 4.0;
const REPO_TAB_SCROLLBAR_THICKNESS: f32 = 5.0;
pub(crate) const DIFF_ROW_HEIGHT: f32 = 22.0;
pub(crate) const NAV_ROW_HEIGHT: f32 = 30.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ScrollbarAxis {
    Vertical,
    Horizontal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ScrollbarMode {
    Vertical,
    Horizontal,
    Both,
}

impl ScrollbarMode {
    fn has_vertical(self) -> bool {
        matches!(self, Self::Vertical | Self::Both)
    }

    fn has_horizontal(self) -> bool {
        matches!(self, Self::Horizontal | Self::Both)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ScrollbarDragState {
    pub(crate) scroll_id: SharedString,
    pub(crate) axis: ScrollbarAxis,
    pub(crate) start_position: Point<Pixels>,
    pub(crate) start_offset: Point<Pixels>,
    pub(crate) track_len: f32,
    pub(crate) thumb_len: f32,
    pub(crate) max_offset: f32,
}

#[derive(Clone, Copy, Debug)]
struct ScrollbarGeometry {
    track: Bounds<Pixels>,
    thumb: Bounds<Pixels>,
    track_len: f32,
    thumb_len: f32,
    max_offset: f32,
}

pub(crate) fn scrollable_frame_when(
    scroll_id: &'static str,
    mode: ScrollbarMode,
    content: gpui::AnyElement,
    handle: ScrollHandle,
    content_present: bool,
    _cx: &mut Context<RepositoryView>,
) -> impl IntoElement {
    scrollable_frame_base(scroll_id, mode, content, handle, true, content_present, _cx)
}

pub(crate) fn scrollable_frame_intrinsic(
    scroll_id: &'static str,
    mode: ScrollbarMode,
    content: gpui::AnyElement,
    handle: ScrollHandle,
    _cx: &mut Context<RepositoryView>,
) -> impl IntoElement {
    scrollable_frame_base(scroll_id, mode, content, handle, false, true, _cx)
}

pub(crate) fn scrollable_uniform_frame(
    scroll_id: &'static str,
    mode: ScrollbarMode,
    content: gpui::AnyElement,
    handle: UniformListScrollHandle,
    content_present: bool,
    _cx: &mut Context<RepositoryView>,
) -> impl IntoElement {
    let base_handle = handle.0.borrow().base_handle.clone();
    scrollable_frame_base(
        scroll_id,
        mode,
        content,
        base_handle,
        true,
        content_present,
        _cx,
    )
}

fn scrollable_frame_base(
    scroll_id: &'static str,
    mode: ScrollbarMode,
    content: gpui::AnyElement,
    handle: ScrollHandle,
    fill_parent: bool,
    content_present: bool,
    _cx: &mut Context<RepositoryView>,
) -> impl IntoElement {
    let entity = _cx.entity();
    let scroll_id = SharedString::from(scroll_id);
    div()
        .relative()
        .flex()
        .flex_col()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .when(fill_parent, |this| this.flex_1())
        .when(!fill_parent, |this| this.flex_none())
        .child(content)
        .child(
            canvas(
                |_, _, _| (),
                move |bounds, _, window, _cx| {
                    if !content_present {
                        handle.set_offset(Point::default());
                        let should_clear_drag = entity
                            .read(_cx)
                            .scrollbar_drag
                            .as_ref()
                            .is_some_and(|drag| drag.scroll_id == scroll_id);
                        if should_clear_drag {
                            entity.update(_cx, |this, cx| {
                                this.scrollbar_drag = None;
                                cx.notify();
                            });
                        }
                        return;
                    }

                    let active_axis = entity
                        .read(_cx)
                        .scrollbar_drag
                        .as_ref()
                        .filter(|drag| drag.scroll_id == scroll_id)
                        .map(|drag| drag.axis);
                    paint_scrollbars(&handle, &scroll_id, mode, bounds, active_axis, window);

                    register_scrollbar_mouse_down(
                        entity.clone(),
                        scroll_id.clone(),
                        handle.clone(),
                        mode,
                        bounds,
                        window,
                    );
                    register_scrollbar_mouse_move(
                        entity.clone(),
                        scroll_id.clone(),
                        handle.clone(),
                        window,
                    );
                    register_scrollbar_mouse_up(entity.clone(), window);
                },
            )
            .absolute()
            .top(px(0.0))
            .left(px(0.0))
            .right(px(0.0))
            .bottom(px(0.0)),
        )
}

fn paint_scrollbars(
    handle: &ScrollHandle,
    scroll_id: &SharedString,
    mode: ScrollbarMode,
    bounds: Bounds<Pixels>,
    active_axis: Option<ScrollbarAxis>,
    window: &mut Window,
) {
    if mode.has_vertical()
        && let Some(geometry) =
            scrollbar_geometry(handle, scroll_id, bounds, ScrollbarAxis::Vertical)
    {
        paint_scrollbar_axis(&geometry, ScrollbarAxis::Vertical, active_axis, window);
    }

    if mode.has_horizontal()
        && let Some(geometry) =
            scrollbar_geometry(handle, scroll_id, bounds, ScrollbarAxis::Horizontal)
    {
        paint_scrollbar_axis(&geometry, ScrollbarAxis::Horizontal, active_axis, window);
    }
}

fn paint_scrollbar_axis(
    geometry: &ScrollbarGeometry,
    axis: ScrollbarAxis,
    active_axis: Option<ScrollbarAxis>,
    window: &mut Window,
) {
    window.paint_quad(fill(geometry.track, rgba(ui_theme::SCROLLBAR_TRACK)).corner_radii(px(4.0)));
    let thumb_color = if active_axis == Some(axis) {
        ui_theme::SCROLLBAR_THUMB_ACTIVE
    } else {
        ui_theme::SCROLLBAR_THUMB
    };
    window.paint_quad(fill(geometry.thumb, rgba(thumb_color)).corner_radii(px(4.0)));
}

fn register_scrollbar_mouse_down(
    entity: gpui::Entity<RepositoryView>,
    scroll_id: SharedString,
    handle: ScrollHandle,
    mode: ScrollbarMode,
    bounds: Bounds<Pixels>,
    window: &mut Window,
) {
    window.on_mouse_event(move |event: &MouseDownEvent, _, _, cx| {
        if event.button != MouseButton::Left {
            return;
        }

        let axis_and_geometry = [
            mode.has_vertical().then_some(ScrollbarAxis::Vertical),
            mode.has_horizontal().then_some(ScrollbarAxis::Horizontal),
        ]
        .into_iter()
        .flatten()
        .filter_map(|axis| {
            scrollbar_geometry(&handle, &scroll_id, bounds, axis).map(|geometry| (axis, geometry))
        })
        .find(|(_, geometry)| geometry.track.contains(&event.position));

        let Some((axis, geometry)) = axis_and_geometry else {
            return;
        };

        cx.stop_propagation();
        if geometry.thumb.contains(&event.position) {
            let start_position = event.position;
            let start_offset = handle.offset();
            entity.update(cx, |this, cx| {
                this.close_popups();
                this.scrollbar_drag = Some(ScrollbarDragState {
                    scroll_id: scroll_id.clone(),
                    axis,
                    start_position,
                    start_offset,
                    track_len: geometry.track_len,
                    thumb_len: geometry.thumb_len,
                    max_offset: geometry.max_offset,
                });
                cx.notify();
            });
        } else {
            page_scroll(&handle, axis, &geometry, event.position);
            entity.update(cx, |this, cx| {
                this.close_popups();
                cx.notify();
            });
        }
    });
}

fn register_scrollbar_mouse_move(
    entity: gpui::Entity<RepositoryView>,
    scroll_id: SharedString,
    handle: ScrollHandle,
    window: &mut Window,
) {
    window.on_mouse_event(move |event: &MouseMoveEvent, _, _, cx| {
        if !event.dragging() {
            return;
        }

        let drag = entity.read(cx).scrollbar_drag.clone();
        let Some(drag) = drag else {
            return;
        };
        if drag.scroll_id != scroll_id {
            return;
        }

        cx.stop_propagation();
        apply_scrollbar_drag(&handle, &drag, event.position);
        entity.update(cx, |_this, cx| cx.notify());
    });
}

fn register_scrollbar_mouse_up(entity: gpui::Entity<RepositoryView>, window: &mut Window) {
    window.on_mouse_event(move |_: &MouseUpEvent, _, _, cx| {
        if entity.read(cx).scrollbar_drag.is_none() {
            return;
        }

        entity.update(cx, |this, cx| {
            this.scrollbar_drag = None;
            cx.notify();
        });
    });
}

fn scrollbar_geometry(
    handle: &ScrollHandle,
    scroll_id: &SharedString,
    bounds: Bounds<Pixels>,
    axis: ScrollbarAxis,
) -> Option<ScrollbarGeometry> {
    let max_offset = handle.max_offset();
    let scroll_offset = handle.offset();
    let margin = SCROLLBAR_MARGIN;
    let thickness = SCROLLBAR_THICKNESS;
    let reserve_horizontal = f32::from(max_offset.width) > 1.0;
    let reserve_vertical = f32::from(max_offset.height) > 1.0;

    let (viewport_len, max_offset, current_offset, track) = match axis {
        ScrollbarAxis::Vertical => {
            let max_offset: f32 = max_offset.height.into();
            if max_offset <= 1.0 {
                return None;
            }
            let track = Bounds::from_corners(
                point(
                    bounds.origin.x + bounds.size.width - px(thickness + margin),
                    bounds.origin.y + px(margin),
                ),
                point(
                    bounds.origin.x + bounds.size.width - px(margin),
                    bounds.origin.y + bounds.size.height
                        - px(margin
                            + if reserve_horizontal {
                                thickness + margin
                            } else {
                                0.0
                            }),
                ),
            );
            (
                f32::from(bounds.size.height),
                max_offset,
                -f32::from(scroll_offset.y),
                track,
            )
        }
        ScrollbarAxis::Horizontal => {
            let max_offset: f32 = max_offset.width.into();
            if max_offset <= 1.0 {
                return None;
            }
            let is_repo_tab_scroll = scroll_id.as_ref() == REPO_TAB_SCROLL_ID;
            let y_offset = is_repo_tab_scroll
                .then_some(REPO_TAB_SCROLLBAR_Y_OFFSET)
                .unwrap_or_default();
            let horizontal_thickness = if is_repo_tab_scroll {
                REPO_TAB_SCROLLBAR_THICKNESS
            } else {
                thickness
            };
            let bottom_margin = (margin - y_offset).max(0.0);
            let track = Bounds::from_corners(
                point(
                    bounds.origin.x + px(margin),
                    bounds.origin.y + bounds.size.height - px(horizontal_thickness + bottom_margin),
                ),
                point(
                    bounds.origin.x + bounds.size.width
                        - px(margin
                            + if reserve_vertical {
                                thickness + margin
                            } else {
                                0.0
                            }),
                    bounds.origin.y + bounds.size.height - px(bottom_margin),
                ),
            );
            (
                f32::from(bounds.size.width),
                max_offset,
                -f32::from(scroll_offset.x),
                track,
            )
        }
    };

    let track_len = match axis {
        ScrollbarAxis::Vertical => f32::from(track.size.height),
        ScrollbarAxis::Horizontal => f32::from(track.size.width),
    };
    if track_len <= SCROLLBAR_MIN_THUMB {
        return None;
    }

    let content_len = viewport_len + max_offset;
    let thumb_len = (viewport_len / content_len * track_len).clamp(SCROLLBAR_MIN_THUMB, track_len);
    let travel = (track_len - thumb_len).max(1.0);
    let thumb_pos = (current_offset / max_offset).clamp(0.0, 1.0) * travel;

    let thumb = match axis {
        ScrollbarAxis::Vertical => Bounds::from_corners(
            point(track.origin.x, track.origin.y + px(thumb_pos)),
            point(
                track.origin.x + track.size.width,
                track.origin.y + px(thumb_pos + thumb_len),
            ),
        ),
        ScrollbarAxis::Horizontal => Bounds::from_corners(
            point(track.origin.x + px(thumb_pos), track.origin.y),
            point(
                track.origin.x + px(thumb_pos + thumb_len),
                track.origin.y + track.size.height,
            ),
        ),
    };

    Some(ScrollbarGeometry {
        track,
        thumb,
        track_len,
        thumb_len,
        max_offset,
    })
}

fn page_scroll(
    handle: &ScrollHandle,
    axis: ScrollbarAxis,
    geometry: &ScrollbarGeometry,
    position: Point<Pixels>,
) {
    let offset = handle.offset();
    let page = match axis {
        ScrollbarAxis::Vertical => f32::from(handle.bounds().size.height) * 0.85,
        ScrollbarAxis::Horizontal => f32::from(handle.bounds().size.width) * 0.85,
    };

    let before_thumb = match axis {
        ScrollbarAxis::Vertical => position.y < geometry.thumb.origin.y,
        ScrollbarAxis::Horizontal => position.x < geometry.thumb.origin.x,
    };
    let delta = if before_thumb { page } else { -page };
    set_axis_offset(handle, axis, offset, delta, geometry.max_offset);
}

fn apply_scrollbar_drag(handle: &ScrollHandle, drag: &ScrollbarDragState, position: Point<Pixels>) {
    let movement = match drag.axis {
        ScrollbarAxis::Vertical => f32::from(position.y - drag.start_position.y),
        ScrollbarAxis::Horizontal => f32::from(position.x - drag.start_position.x),
    };
    let travel = (drag.track_len - drag.thumb_len).max(1.0);
    let content_delta = -(movement / travel * drag.max_offset);
    set_axis_offset(
        handle,
        drag.axis,
        drag.start_offset,
        content_delta,
        drag.max_offset,
    );
}

fn set_axis_offset(
    handle: &ScrollHandle,
    axis: ScrollbarAxis,
    start_offset: Point<Pixels>,
    delta: f32,
    max_offset: f32,
) {
    let mut offset = start_offset;
    match axis {
        ScrollbarAxis::Vertical => {
            let next = (f32::from(offset.y) + delta).clamp(-max_offset, 0.0);
            offset.y = px(next);
        }
        ScrollbarAxis::Horizontal => {
            let next = (f32::from(offset.x) + delta).clamp(-max_offset, 0.0);
            offset.x = px(next);
        }
    }
    handle.set_offset(offset);
}

pub(crate) fn section_header(label: impl Into<SharedString>) -> impl IntoElement {
    div()
        .flex_none()
        .px_3()
        .py_2()
        .border_b_1()
        .border_color(rgb(ui_theme::BORDER))
        .bg(rgb(COLOR_HEADER_BG))
        .text_size(px(12.0))
        .font_weight(gpui::FontWeight::BOLD)
        .text_color(rgb(ui_theme::FOREGROUND))
        .child(label.into())
}

pub(crate) fn section_header_action(
    label: impl Into<SharedString>,
    action: Option<gpui::AnyElement>,
) -> impl IntoElement {
    let header = div()
        .flex_none()
        .flex()
        .items_center()
        .justify_between()
        .gap_2()
        .px_3()
        .py_2()
        .border_b_1()
        .border_color(rgb(ui_theme::BORDER))
        .bg(rgb(COLOR_HEADER_BG))
        .child(
            div()
                .text_size(px(12.0))
                .font_weight(gpui::FontWeight::BOLD)
                .text_color(rgb(ui_theme::FOREGROUND))
                .child(label.into()),
        );
    if let Some(action) = action {
        header.child(action)
    } else {
        header
    }
}

pub(crate) fn history_scope_button(
    label: &'static str,
    selected: bool,
    action: impl Fn(&mut RepositoryView) + 'static,
    cx: &mut Context<RepositoryView>,
) -> impl IntoElement {
    div()
        .id(format!("history-scope-{label}"))
        .flex_none()
        .px_2()
        .py_1()
        .rounded_sm()
        .border_1()
        .border_color(if selected {
            rgb(ui_theme::ACCENT)
        } else {
            rgb(COLOR_BORDER)
        })
        .bg(if selected {
            rgb(ui_theme::ACCENT)
        } else {
            rgb(ui_theme::CARD)
        })
        .text_size(px(11.0))
        .text_color(if selected {
            rgb(ui_theme::PRIMARY)
        } else {
            rgb(ui_theme::MUTED_FOREGROUND)
        })
        .cursor_pointer()
        .hover(|this| this.bg(rgb(ui_theme::SECONDARY)))
        .on_click(cx.listener(move |this, _event, _window, cx| {
            action(this);
            cx.notify();
        }))
        .child(label)
}

pub(crate) fn nav_list(
    owner: &RepositoryView,
    id: &'static str,
    rows: Vec<gpui::AnyElement>,
    cx: &mut Context<RepositoryView>,
) -> impl IntoElement {
    let handle = owner.scroll_handle(id);
    let content_present = !rows.is_empty();
    let content = div()
        .id(id)
        .flex()
        .flex_col()
        .gap(px(2.0))
        .min_w(px(0.0))
        .min_h(px(0.0))
        .overflow_y_scroll()
        .track_scroll(&handle)
        .children(rows)
        .into_any_element();

    scrollable_frame_when(
        id,
        ScrollbarMode::Vertical,
        content,
        handle,
        content_present,
        cx,
    )
}

pub(crate) fn nav_row(
    id: impl Into<SharedString>,
    selected: bool,
    emphasized: bool,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id.into())
        .flex()
        .flex_none()
        .h(px(NAV_ROW_HEIGHT))
        .min_h(px(NAV_ROW_HEIGHT))
        .items_center()
        .justify_between()
        .gap_2()
        .px_2()
        .py_1()
        .rounded_sm()
        .cursor_pointer()
        .bg(if selected {
            rgb(ui_theme::ACCENT)
        } else if emphasized {
            rgb(ui_theme::ACCENT)
        } else {
            rgb(ui_theme::CARD)
        })
        .border_1()
        .border_color(if selected {
            rgb(ui_theme::PRIMARY)
        } else if emphasized {
            rgb(ui_theme::PRIMARY)
        } else {
            rgb(COLOR_BORDER)
        })
}

pub(crate) fn placeholder_row(text: &'static str) -> impl IntoElement {
    div()
        .flex_none()
        .min_h(px(NAV_ROW_HEIGHT))
        .mx_1()
        .my_1()
        .px_3()
        .py_2()
        .rounded_sm()
        .border_1()
        .border_color(rgb(ui_theme::BORDER))
        .bg(rgb(ui_theme::CARD))
        .text_size(px(12.0))
        .text_color(rgb(COLOR_TEXT_FAINT))
        .line_height(px(18.0))
        .child(text)
}

pub(crate) fn commit_time_label(seconds: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp(seconds, 0)
        .map(|time| {
            time.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
        })
        .unwrap_or_else(|| "时间未知".to_string())
}

pub(crate) fn author_avatar(author: &str) -> impl IntoElement {
    div()
        .flex_none()
        .size(px(20.0))
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(rgb(author_avatar_color(author)))
        .text_color(rgb(COLOR_SURFACE))
        .text_size(px(11.0))
        .font_weight(gpui::FontWeight::BOLD)
        .child(author_avatar_initial(author))
}

fn author_avatar_color(author: &str) -> u32 {
    const PALETTE: [u32; 10] = [
        0x6366f1, 0x3b82f6, 0x06b6d4, 0x14b8a6, 0x22c55e, 0x84cc16, 0xf59e0b, 0xf97316, 0xef4444,
        0xa855f7,
    ];
    let mut hash = 0u32;
    for byte in author.bytes() {
        hash = hash.wrapping_mul(31).wrapping_add(byte as u32);
    }
    PALETTE[(hash as usize) % PALETTE.len()]
}

fn author_avatar_initial(author: &str) -> String {
    author
        .trim()
        .chars()
        .find(|ch| !ch.is_whitespace())
        .map(|ch| ch.to_uppercase().to_string())
        .unwrap_or_else(|| "?".to_string())
}

pub(crate) fn diff_scope_label(scope: &DiffScope) -> &'static str {
    match scope {
        DiffScope::Staged => "已暂存",
        DiffScope::Unstaged => "未暂存",
    }
}

pub(crate) fn diff_scope_id(scope: &DiffScope) -> &'static str {
    match scope {
        DiffScope::Staged => "staged",
        DiffScope::Unstaged => "unstaged",
    }
}

pub(crate) fn change_state_color(state: &ChangeState) -> u32 {
    match state {
        ChangeState::Added => ui_theme::GIT_ADDED,
        ChangeState::Modified => ui_theme::COLOR_WARNING_FOREGROUND,
        ChangeState::Deleted | ChangeState::Conflicted => ui_theme::DESTRUCTIVE,
        ChangeState::Renamed => ui_theme::PRIMARY,
        ChangeState::Typechange => ui_theme::PRIMARY,
        ChangeState::Untracked => ui_theme::MUTED_FOREGROUND,
    }
}

/// 设计图：变更行状态字母 badge 的背景色
/// M → PRIMARY, A → GIT_ADDED, ? → GIT_UNTRACKED, D → DESTRUCTIVE
pub(crate) fn change_state_badge_bg(state: &ChangeState) -> u32 {
    match state {
        ChangeState::Modified => ui_theme::PRIMARY,
        ChangeState::Added => ui_theme::GIT_ADDED,
        ChangeState::Deleted | ChangeState::Conflicted => ui_theme::DESTRUCTIVE,
        ChangeState::Renamed => ui_theme::PRIMARY,
        ChangeState::Typechange => ui_theme::PRIMARY,
        ChangeState::Untracked => ui_theme::GIT_UNTRACKED,
    }
}

pub(crate) fn context_menu_item(
    label: &'static str,
    enabled: bool,
    on_click: impl Fn(&mut RepositoryView) + 'static,
    cx: &mut Context<RepositoryView>,
) -> impl IntoElement {
    div()
        .id(format!("context-menu-{label}"))
        .px_3()
        .py_1()
        .text_color(if enabled {
            rgb(COLOR_TEXT)
        } else {
            rgb(COLOR_TEXT_FAINT)
        })
        .bg(rgb(ui_theme::CARD))
        .cursor_pointer()
        .when(enabled, |this| {
            this.hover(|this| this.bg(rgb(ui_theme::SECONDARY)))
        })
        .on_click(cx.listener(move |this, _event, _window, cx| {
            cx.stop_propagation();
            if enabled {
                on_click(this);
                cx.notify();
            }
        }))
        .child(label)
}

pub(crate) fn context_menu_item_with_context(
    label: &'static str,
    enabled: bool,
    on_click: impl Fn(&mut RepositoryView, &mut Context<RepositoryView>) + 'static,
    cx: &mut Context<RepositoryView>,
) -> impl IntoElement {
    div()
        .id(format!("context-menu-{label}"))
        .px_3()
        .py_1()
        .text_color(if enabled {
            rgb(COLOR_TEXT)
        } else {
            rgb(COLOR_TEXT_FAINT)
        })
        .bg(rgb(ui_theme::CARD))
        .cursor_pointer()
        .when(enabled, |this| {
            this.hover(|this| this.bg(rgb(ui_theme::SECONDARY)))
        })
        .on_click(cx.listener(move |this, _event, _window, cx| {
            cx.stop_propagation();
            if enabled {
                on_click(this, cx);
                cx.notify();
            }
        }))
        .child(label)
}

pub(crate) fn menu_separator() -> impl IntoElement {
    div().h(px(1.0)).mx_1().my_1().bg(rgb(ui_theme::BORDER))
}

pub(crate) fn diff_header_toggle(
    label: &'static str,
    target: DiffHeaderTarget,
    cx: &mut Context<RepositoryView>,
) -> impl IntoElement {
    div()
        .id(match target {
            DiffHeaderTarget::Worktree => "diff-header-toggle",
            DiffHeaderTarget::History => "history-diff-header-toggle",
            DiffHeaderTarget::Stash => "stash-diff-header-toggle",
            DiffHeaderTarget::Browse => "browse-diff-header-toggle",
        })
        .flex()
        .w_full()
        .min_w(px(0.0))
        .h(px(DIFF_ROW_HEIGHT))
        .min_h(px(DIFF_ROW_HEIGHT))
        .line_height(px(DIFF_ROW_HEIGHT))
        .overflow_hidden()
        .items_center()
        .gap_2()
        .px_2()
        .bg(rgb(COLOR_HEADER_BG))
        .text_color(rgb(COLOR_TEXT_MUTED))
        .cursor_pointer()
        .hover(|this| this.bg(rgb(COLOR_BLUE_SOFT)))
        .on_click(cx.listener(move |this, _event, _window, cx| {
            match target {
                DiffHeaderTarget::Worktree => this.toggle_diff_headers(),
                DiffHeaderTarget::History => this.toggle_history_diff_headers(),
                DiffHeaderTarget::Stash => this.toggle_stash_diff_headers(),
                DiffHeaderTarget::Browse => this.toggle_browse_diff_headers(),
            }
            cx.notify();
        }))
        .child(
            div()
                .flex_none()
                .w(px(92.0))
                .text_align(gpui::TextAlign::Right)
                .text_color(rgb(COLOR_TEXT_FAINT))
                .child(""),
        )
        .child(label)
}

pub(crate) fn diff_line(
    kind: DiffLineKind,
    old_lineno: Option<u32>,
    new_lineno: Option<u32>,
    content: String,
) -> impl IntoElement {
    let (bg, fg) = match kind {
        DiffLineKind::Added => (ui_theme::DIFF_ADDED_BG, ui_theme::DIFF_ADDED_TEXT),
        DiffLineKind::Removed => (ui_theme::DIFF_REMOVED_BG, ui_theme::DIFF_REMOVED_TEXT),
        DiffLineKind::Header => (ui_theme::DIFF_HEADER_BG, ui_theme::DIFF_HEADER_TEXT),
        DiffLineKind::Context => (COLOR_PANEL_BG, COLOR_TEXT),
    };
    let old_lineno = old_lineno.map(|line| line.to_string()).unwrap_or_default();
    let new_lineno = new_lineno.map(|line| line.to_string()).unwrap_or_default();

    div()
        .flex()
        .w_full()
        .min_w(px(0.0))
        .h(px(DIFF_ROW_HEIGHT))
        .min_h(px(DIFF_ROW_HEIGHT))
        .line_height(px(DIFF_ROW_HEIGHT))
        .overflow_hidden()
        .items_center()
        .bg(rgb(bg))
        .text_color(rgb(fg))
        .child(diff_lineno(old_lineno))
        .child(diff_lineno(new_lineno))
        .child(
            div()
                .flex_none()
                .h(px(DIFF_ROW_HEIGHT))
                .line_height(px(DIFF_ROW_HEIGHT))
                .overflow_hidden()
                .px_2()
                .whitespace_nowrap()
                .child(content),
        )
}

fn diff_lineno(line: String) -> impl IntoElement {
    div()
        .flex_none()
        .w(px(46.0))
        .px_1()
        .text_align(gpui::TextAlign::Right)
        .text_color(rgb(COLOR_TEXT_FAINT))
        .bg(rgb(COLOR_HEADER_BG))
        .child(line)
}

#[cfg(test)]
#[path = "tests/ui_helpers.rs"]
mod tests;
