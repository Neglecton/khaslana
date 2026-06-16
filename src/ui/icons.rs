use gpui::{IntoElement, ParentElement, Styled, div, px, rgb, svg};

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
    More,
    Ai,
    Search,
    Close,
    Plus,
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
            Self::More => "icons/more.svg",
            Self::Ai => "icons/ai.svg",
            Self::Search => "icons/search.svg",
            Self::Close => "icons/close.svg",
            Self::Plus => "icons/plus.svg",
        }
    }
}

pub(crate) fn toolbar_icon(icon: ToolbarIcon, color: u32) -> impl IntoElement {
    // GPUI 会把 SVG 渲染为 alpha mask 后统一着色，外层固定槽位避免按钮布局压缩图标。
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
                .flex_none(),
        )
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::ToolbarIcon;

    #[test]
    fn toolbar_icon_paths_match_embedded_asset_root() {
        assert_eq!(ToolbarIcon::Open.path(), "icons/open.svg");
        assert_eq!(ToolbarIcon::Worktree.path(), "icons/worktree.svg");
        assert_eq!(ToolbarIcon::History.path(), "icons/history.svg");
        assert_eq!(ToolbarIcon::Stash.path(), "icons/stash.svg");
        assert_eq!(ToolbarIcon::More.path(), "icons/more.svg");
        assert_eq!(ToolbarIcon::Ai.path(), "icons/ai.svg");
        assert_eq!(ToolbarIcon::Search.path(), "icons/search.svg");
        assert_eq!(ToolbarIcon::Close.path(), "icons/close.svg");
        assert_eq!(ToolbarIcon::Plus.path(), "icons/plus.svg");
    }

    #[test]
    fn toolbar_svgs_use_monochrome_mask_shapes() {
        for icon in [
            ToolbarIcon::Open,
            ToolbarIcon::Clone,
            ToolbarIcon::Refresh,
            ToolbarIcon::Fetch,
            ToolbarIcon::Pull,
            ToolbarIcon::Push,
            ToolbarIcon::Credentials,
            ToolbarIcon::Proxy,
            ToolbarIcon::Workflow,
            ToolbarIcon::Worktree,
            ToolbarIcon::History,
            ToolbarIcon::Stash,
            ToolbarIcon::Submodule,
            ToolbarIcon::More,
            ToolbarIcon::Ai,
            ToolbarIcon::Search,
            ToolbarIcon::Close,
            ToolbarIcon::Plus,
        ] {
            let asset_path = format!("assets/{}", icon.path());
            let svg = fs::read_to_string(&asset_path).unwrap_or_else(|err| {
                panic!("failed to read {asset_path}: {err}");
            });

            assert!(
                !svg.contains("currentColor"),
                "{asset_path} should not depend on currentColor; GPUI tints SVG alpha masks"
            );
            assert!(
                svg.contains("#000000"),
                "{asset_path} should provide an opaque monochrome mask"
            );
        }
    }
}
