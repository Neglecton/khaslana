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
    assert_eq!(ToolbarIcon::Globe.path(), "icons/globe.svg");
    assert_eq!(ToolbarIcon::ChevronRight.path(), "icons/chevron-right.svg");
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
        ToolbarIcon::Globe,
        ToolbarIcon::ChevronRight,
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
