use std::{borrow::Cow, collections::BTreeSet};

use gpui::{AssetSource, SharedString};
use rust_embed::Embed;
use gpui_kit::assets::AllAssets;

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

/// 合并项目自绘图标与 Kit 内置资源，避免本地图标依赖运行目录。
///
/// 用 `AllAssets` 而不是默认的 `Assets`：后者只带 101 个组件图标，
/// `IconName` 里的导航/文件类图标（Git 分支、云、标签等）不在其中，
/// 缺失时图标会静默不渲染。体积代价约 +1 MB，图标来源优化留到 M7。
pub(crate) struct AppAssets {
    app: KhaslanaAsset,
    ui: AllAssets,
}

impl AppAssets {
    pub(crate) fn new() -> Self {
        Self {
            app: KhaslanaAsset,
            ui: AllAssets,
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
