use std::{borrow::Cow, collections::BTreeSet};

use gpui::{AssetSource, SharedString};
use rust_embed::Embed;
use yororen_ui::assets::UiAsset;

#[derive(Embed)]
#[folder = "assets/"]
#[include = "icons/**/*"]
#[exclude = "*.DS_Store"]
pub(crate) struct KhaslanaAsset;

impl AssetSource for KhaslanaAsset {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        Ok(Self::get(path).map(|file| file.data))
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        Ok(Self::iter()
            .filter_map(|asset| asset.starts_with(path).then(|| asset.into()))
            .collect())
    }
}

/// 合并项目自绘图标与 Yororen 内置资源，避免本地图标依赖运行目录。
pub(crate) struct AppAssets {
    app: KhaslanaAsset,
    ui: UiAsset,
}

impl AppAssets {
    pub(crate) fn new() -> Self {
        Self {
            app: KhaslanaAsset,
            ui: UiAsset,
        }
    }
}

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        if let Some(asset) = self.app.load(path)? {
            return Ok(Some(asset));
        }
        self.ui.load(path)
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        let mut merged = BTreeSet::<SharedString>::new();
        for asset in self.app.list(path)? {
            merged.insert(asset);
        }
        for asset in self.ui.list(path)? {
            merged.insert(asset);
        }
        Ok(merged.into_iter().collect())
    }
}

#[cfg(test)]
#[path = "tests/assets.rs"]
mod tests;
