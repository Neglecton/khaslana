//! 设置中心的 Kit Settings 页面模型。
//!
//! 界面样式由 gpui-kit 的 `Settings` 组件族承载：`Settings`（可拖拽侧栏 +
//! 搜索 + 右侧 `SettingPage`）→ `SettingPage`（页头 + 虚拟列表）→
//! `SettingGroup`（`GroupBox` 分组）→ `SettingItem`（标题 / 描述 / 控件）。
//! 每个分类一页，页面内容由各 view 的 `settings_*_groups` 按分组模型提供，
//! 侧栏与内容区滚动由 Kit 内部状态驱动，业务侧只保留
//! `settings_center: Option<SettingsCategory>` 作为「是否打开、定位到哪一页」
//! 的真值。
//!
//! 侧栏为单层：Kit 在「某页可见分组数 > 1 且分组有标题」时会把分组渲染成
//! 侧栏手风琴子项（两级导航）。设计稿（2026-09-23 二次调整）要求单层，
//! 因此分组一律不设 Kit 标题，改由 [`settings_group_heading`] 在分组卡内
//! 渲染标题与描述。
//!
//! 控件选型（AGENTS.md 防回归约束）：
//! - 开关走 `SettingField::switch`，内部即 Kit `Switch`，回调写回受控值；
//! - 文本输入不引入 Kit `StringField` 的第二套状态，统一经
//!   `SettingItem::new(.., SettingField::render(..))` 通道复用
//!   `src/ui/fields.rs` 的 `TextFieldState` 宿主；
//! - 分段、色板、卡片、按钮组等同理走 `render` 通道复用既有组件。

use gpui::{Context, FontWeight, SharedString, StyleRefinement, Window, div, prelude::*, px};
use gpui_kit::assets::IconName;
use gpui_kit::component::group_box::GroupBoxVariant;
use gpui_kit::component::setting::{SelectIndex, SettingGroup, SettingItem, SettingPage, Settings};

use crate::{RepositoryView, SettingsCategory, ui::theme::{self as ui_theme, rgb}};

/// 设置中心分类的固定顺序：侧栏与 `Settings` 页码共用这一份，
/// 增删分类必须同步 `page_index` 的映射语义（按位置查找）。
pub(crate) const SETTINGS_CATEGORY_ORDER: [SettingsCategory; 9] = [
    SettingsCategory::Credentials,
    SettingsCategory::Proxy,
    SettingsCategory::Ai,
    SettingsCategory::ExternalMerge,
    SettingsCategory::CodeIndex,
    SettingsCategory::Theme,
    SettingsCategory::Update,
    SettingsCategory::Shortcuts,
    SettingsCategory::About,
];

/// 分类对应的侧栏图标（Kit 内置 Lucide 目录）。
fn category_icon(category: SettingsCategory) -> IconName {
    match category {
        SettingsCategory::Credentials => IconName::KeyRound,
        SettingsCategory::Proxy => IconName::Network,
        SettingsCategory::Ai => IconName::Sparkles,
        SettingsCategory::ExternalMerge => IconName::GitMerge,
        SettingsCategory::CodeIndex => IconName::SearchCode,
        SettingsCategory::Theme => IconName::Palette,
        SettingsCategory::Update => IconName::RefreshCw,
        SettingsCategory::Shortcuts => IconName::Keyboard,
        SettingsCategory::About => IconName::Info,
    }
}

/// 分类在 `Settings` 页码中的位置。
pub(crate) fn settings_page_index(category: SettingsCategory) -> usize {
    SETTINGS_CATEGORY_ORDER
        .iter()
        .position(|candidate| *candidate == category)
        .unwrap_or(0)
}

/// 一个分类的页头信息。
pub(crate) struct SettingsPaneMeta {
    pub(crate) title: &'static str,
    pub(crate) description: &'static str,
}

/// 分组标题元素：在分组卡内渲染「标题 + 描述」，替代 Kit 的
/// `SettingGroup::title`——后者会同时在侧栏生成手风琴子项（两级导航），
/// 与设计稿的单层侧栏冲突。
///
/// 排版对齐设计稿：标题 13 SemiBold（介于 `TYPE_BODY` 12 与 `TYPE_TITLE` 14
/// 之间，设计稿取值）、描述 `TYPE_META` Secondary、两者间距 2px。
/// `keywords` 带标题，保证按分组标题搜索时该组仍可命中。
pub(crate) fn settings_group_heading(
    title: &'static str,
    description: Option<SharedString>,
) -> SettingItem {
    SettingItem::render(move |_options, _window, _cx| {
        div()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .child(
                div()
                    .text_size(px(13.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                    .child(title),
            )
            .when_some(description.clone(), |this, description| {
                this.child(
                    div()
                        .text_size(px(ui_theme::TYPE_META))
                        .line_height(px(16.0))
                        .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                        .child(description),
                )
            })
    })
    .keywords([title])
}

/// 页头样式：内距与标题/描述间距对齐设计稿（[12,16,8,16]、间距 2px），
/// 覆盖 Kit 默认的 `p_4` + `gap_3`。
fn settings_page_header_style() -> StyleRefinement {
    StyleRefinement::default()
        .pt(px(12.0))
        .pr(px(16.0))
        .pb(px(8.0))
        .pl(px(16.0))
        .gap(px(2.0))
}

/// 分类的页头标题与描述（Kit `SettingPage` 的 title / description）。
pub(crate) fn settings_pane_meta(category: SettingsCategory) -> SettingsPaneMeta {
    match category {
        SettingsCategory::Credentials => SettingsPaneMeta {
            title: "凭据管理",
            description: "各远端的用户名与访问令牌；密文保存在系统密钥库。",
        },
        SettingsCategory::Proxy => SettingsPaneMeta {
            title: "网络代理",
            description: "Git 网络操作走哪条代理链路。配置错误会直接报错，不会静默改走直连。",
        },
        SettingsCategory::Ai => SettingsPaneMeta {
            title: "AI 设置",
            description: "配置 AI 服务连接，用于提交信息生成、冲突解决建议与代码评审。",
        },
        SettingsCategory::ExternalMerge => SettingsPaneMeta {
            title: "合并工具",
            description: "把冲突文件交给外部合并工具处理，并选择需要处理的文件范围。",
        },
        SettingsCategory::CodeIndex => SettingsPaneMeta {
            title: "代码索引",
            description: "为每个仓库建立本机代码知识图谱，供全局符号搜索与 MCP 工具使用。",
        },
        SettingsCategory::Theme => SettingsPaneMeta {
            title: "外观",
            description: "主题与界面显示偏好。改动即时生效。",
        },
        SettingsCategory::Update => SettingsPaneMeta {
            title: "更新设置",
            description: "检查新版本与发布渠道；迁移、移动应用数据目录也在这里。",
        },
        SettingsCategory::Shortcuts => SettingsPaneMeta {
            title: "快捷键",
            description: "点击「重新绑定」后按下组合键录入；按 Esc 取消录制。",
        },
        SettingsCategory::About => SettingsPaneMeta {
            title: "关于",
            description: "版本号、发布渠道与版本说明。",
        },
    }
}

impl RepositoryView {
    /// 组装设置中心的 Kit `Settings`。
    ///
    /// Kit 管理侧栏选页；活动页渲染时把页码同步到业务状态。
    pub(crate) fn render_settings_kit(
        &self,
        category: SettingsCategory,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Settings {
        let pages = self.settings_pane_pages(window, cx);
        Settings::new(SharedString::from(format!(
            "settings-center-{}",
            self.settings_center_generation
        )))
        .default_selected_index(SelectIndex {
            page_ix: settings_page_index(category),
            group_ix: None,
        })
        // 侧栏默认 250px、可拖 160–360；Kit 内部换页保留拖拽宽度。
        .sidebar_width(px(250.0))
        .sidebar_size_range(px(160.0)..px(360.0))
        // 分组用 Outline 变体：BORDER_MUTED 描边 + 主题 radius，与面板语言一致；
        // 不用 Fill——Kit group_box token 未经桥接配置。
        .with_group_variant(GroupBoxVariant::Outline)
        .pages(pages)
    }

    /// 全部九页，顺序与 [`SETTINGS_CATEGORY_ORDER`] 一致。
    fn settings_pane_pages(&self, window: &Window, cx: &mut Context<Self>) -> Vec<SettingPage> {
        SETTINGS_CATEGORY_ORDER
            .iter()
            .map(|category| self.settings_pane_page(*category, window, cx))
            .collect()
    }

    /// 单页：页头（标题 + 描述 + 图标）+ 分组列表。
    fn settings_pane_page(
        &self,
        category: SettingsCategory,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> SettingPage {
        let meta = settings_pane_meta(category);
        let view = cx.entity();
        let generation = self.settings_center_generation;
        SettingPage::new(meta.title)
            .icon(category_icon(category))
            .description(meta.description)
            .header_style(&settings_page_header_style())
            .title_suffix(move |_window, cx| {
                if view.read(cx).settings_center != Some(category) {
                    let view = view.clone();
                    cx.defer(move |cx| {
                        view.update(cx, |this, cx| {
                            if this.settings_center.is_some()
                                && this.settings_center_generation == generation
                                && this.settings_center != Some(category)
                            {
                                this.apply_settings_category(category);
                                cx.notify();
                            }
                        });
                    });
                }
                div()
            })
            .groups(self.settings_pane_groups(category, window, cx))
    }

    /// 当前分类的内容区分组。
    ///
    /// 全部页面都按分组模型返回多个分组；分组不设 Kit 标题（否则侧栏出现
    /// 两级手风琴子导航），标题由 [`settings_group_heading`] 在卡内渲染。
    pub(crate) fn settings_pane_groups(
        &self,
        category: SettingsCategory,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Vec<SettingGroup> {
        match category {
            SettingsCategory::Credentials => self.settings_credentials_groups(cx),
            SettingsCategory::Proxy => self.settings_proxy_groups(window, cx),
            SettingsCategory::Ai => self.settings_ai_groups(window, cx),
            SettingsCategory::ExternalMerge => self.settings_external_merge_groups(window, cx),
            SettingsCategory::CodeIndex => self.settings_code_index_groups(window, cx),
            SettingsCategory::Theme => self.settings_theme_groups(window, cx),
            SettingsCategory::Update => self.settings_update_groups(cx),
            SettingsCategory::Shortcuts => self.settings_shortcuts_groups(cx),
            SettingsCategory::About => self.settings_about_groups(cx),
        }
    }
}
