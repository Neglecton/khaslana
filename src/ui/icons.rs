use gpui::{IntoElement, ParentElement, Radians, Styled, Transformation, div, px, svg};

use super::theme::{ThemeVariant, active_variant, rgb};

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
    ChevronLeft,
    Update,
    Settings,
    Keyboard,
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
            Self::ChevronLeft => "icons/chevron-left.svg",
            Self::Update => "icons/update.svg",
            Self::Settings => "icons/settings.svg",
            Self::Keyboard => "icons/keyboard.svg",
        }
    }
}

pub(crate) fn toolbar_icon(icon: ToolbarIcon, color: u32) -> impl IntoElement {
    toolbar_icon_with_size(icon, color, 15.0, 16.0)
}

/// OAuth 快速登录的品牌图标（带文字 logo 的 lockup）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OauthBrand {
    Github,
    Gitee,
}

impl OauthBrand {
    pub(crate) fn id_str(self) -> &'static str {
        match self {
            OauthBrand::Github => "oauth-login-github",
            OauthBrand::Gitee => "oauth-login-gitee",
        }
    }

    /// 按指定主题返回带文字 logo 的 SVG 路径（品牌图标用固定 fill，靠浅/深变体适配）。
    pub(crate) fn lockup_path_for(self, variant: ThemeVariant) -> &'static str {
        match (self, variant) {
            (OauthBrand::Github, ThemeVariant::Light) => "icons/github_lockup_light.svg",
            (OauthBrand::Github, ThemeVariant::Dark) => "icons/github_lockup_dark.svg",
            (OauthBrand::Gitee, ThemeVariant::Light) => "icons/gitee_lockup_light.svg",
            (OauthBrand::Gitee, ThemeVariant::Dark) => "icons/gitee_lockup_dark.svg",
        }
    }

    /// 当前主题下的路径。
    pub(crate) fn lockup_path(self) -> &'static str {
        self.lockup_path_for(active_variant())
    }

    /// 原始宽高比（宽/高），用于按固定高度等比缩放 logo。
    pub(crate) fn aspect(self) -> f32 {
        match self {
            OauthBrand::Github => 416.0 / 95.0,
            OauthBrand::Gitee => 178.0 / 56.0,
        }
    }
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
