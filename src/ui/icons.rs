use gpui::{IntoElement, ParentElement, Radians, Styled, Transformation, div, px, svg};

use super::theme::rgb;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ToolbarIcon {
    Open,
    Clone,
    Refresh,
    Fetch,
    Pull,
    Push,
    Credentials,
    Proxy,
    Workflow,
    Worktree,
    History,
    Stash,
    Submodule,
    Ai,
    Search,
    Close,
    Maximize,
    Restore,
    Plus,
    Minus,
    Trash,
    Globe,
    ChevronRight,
    Update,
    Settings,
}

impl ToolbarIcon {
    pub(crate) fn path(self) -> &'static str {
        match self {
            Self::Open => "icons/open.svg",
            Self::Clone => "icons/clone.svg",
            Self::Refresh => "icons/refresh.svg",
            Self::Fetch => "icons/fetch.svg",
            Self::Pull => "icons/pull.svg",
            Self::Push => "icons/push.svg",
            Self::Credentials => "icons/credentials.svg",
            Self::Proxy => "icons/proxy.svg",
            Self::Workflow => "icons/workflow.svg",
            Self::Worktree => "icons/worktree.svg",
            Self::History => "icons/history.svg",
            Self::Stash => "icons/stash.svg",
            Self::Submodule => "icons/submodule.svg",
            Self::Ai => "icons/ai.svg",
            Self::Search => "icons/search.svg",
            Self::Close => "icons/close.svg",
            Self::Maximize => "icons/maximize.svg",
            Self::Restore => "icons/restore.svg",
            Self::Plus => "icons/plus.svg",
            Self::Minus => "icons/minus.svg",
            Self::Trash => "icons/trash.svg",
            Self::Globe => "icons/globe.svg",
            Self::ChevronRight => "icons/chevron-right.svg",
            Self::Update => "icons/update.svg",
            Self::Settings => "icons/settings.svg",
        }
    }
}

pub(crate) fn toolbar_icon(icon: ToolbarIcon, color: u32) -> impl IntoElement {
    toolbar_icon_with_size(icon, color, 15.0, 16.0)
}

/// 可指定图标和槽位大小的版本，用于行内按钮等小尺寸场景
pub(crate) fn toolbar_icon_with_size(
    icon: ToolbarIcon,
    color: u32,
    icon_size: f32,
    slot_size: f32,
) -> impl IntoElement {
    div()
        .flex_none()
        .size(px(slot_size))
        .flex()
        .items_center()
        .justify_center()
        .child(
            svg()
                .path(icon.path())
                .size(px(icon_size))
                .text_color(rgb(color))
                .flex_none(),
        )
}

/// 带旋转角度的图标渲染，用于展开/收起 chevron 等场景
pub(crate) fn toolbar_icon_rotated(
    icon: ToolbarIcon,
    color: u32,
    rotation_degrees: f32,
) -> impl IntoElement {
    div()
        .flex_none()
        .size(px(16.0))
        .flex()
        .items_center()
        .justify_center()
        .child(
            svg()
                .path(icon.path())
                .size(px(15.0))
                .text_color(rgb(color))
                .flex_none()
                .with_transformation(Transformation::rotate(Radians(
                    rotation_degrees * std::f32::consts::PI / 180.0,
                ))),
        )
}

#[cfg(test)]
#[path = "../tests/ui/icons.rs"]
mod tests;
