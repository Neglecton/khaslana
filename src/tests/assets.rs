use gpui::AssetSource;

use super::AppAssets;

#[test]
fn app_assets_load_toolbar_icons() {
    let assets = AppAssets::new();
    let open_icon = assets.load("icons/open.svg").unwrap();
    assert!(open_icon.is_some());
}
