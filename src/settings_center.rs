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
//! - 开关走 `SettingItem::render` 通道内嵌 Kit `Switch`（Kit 的
//!   `SettingField::switch` 标题字号不可压），回调写回受控值；
//! - 文本输入不引入 Kit `StringField` 的第二套状态，统一经
//!   `SettingItem::render(..)` 通道复用
//!   `src/ui/fields.rs` 的 `TextFieldState` 宿主；
//! - 分段、色板、卡片、按钮组等同理走 `render` 通道复用既有组件。
//!
//! 行距分两档：常规页用逐条 `SettingItem`（Kit `GroupBox` 条目间距 16px）；
//! 表单型页面（AI、合并工具）行多而密，用 [`settings_compact_item`] 把整卡
//! 合并成单条目自排 8px 行距，避免 Kit 硬编码的 16px 把表单撑散。

use std::rc::Rc;

use gpui::{
    App, Context, Div, FontWeight, SharedString, StyleRefinement, Window, div, prelude::*, px,
};
use gpui_kit::assets::IconName;
use gpui_kit::component::group_box::GroupBoxVariant;
use gpui_kit::component::setting::{
    RenderOptions, SelectIndex, SettingGroup, SettingItem, SettingPage, Settings,
};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{Disableable, Sizable, h_flex, v_flex};

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
/// 排版：标题 `TYPE_BODY` SemiBold、描述 `TYPE_META` Secondary、间距 2px。
/// 标题曾取 13（介于 `TYPE_BODY` 12 与 `TYPE_TITLE` 14 之间），后统一到
/// 正文档；页头标题则取 `TYPE_TITLE`，见 [`settings_page_header_style`]。
/// `keywords` 带标题，保证按分组标题搜索时该组仍可命中。
pub(crate) fn settings_group_heading(
    title: &'static str,
    description: Option<SharedString>,
) -> SettingItem {
    SettingItem::render(move |_options, _window, _cx| {
        settings_compact_heading(title, description.clone())
    })
    .keywords([title])
}

/// 分组标题行内容（Div 级）：排版同 [`settings_group_heading`]，供其与
/// [`settings_compact_item`]（整卡单条目）共用。
pub(crate) fn settings_compact_heading(
    title: &'static str,
    description: Option<SharedString>,
) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(2.0))
        .child(
            div()
                .text_size(px(ui_theme::TYPE_BODY))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                .child(title),
        )
        .when_some(description, |this, description| {
            this.child(
                div()
                    .text_size(px(ui_theme::TYPE_META))
                    .line_height(px(16.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child(description),
            )
        })
}

/// 设置项行：标题 + 可选描述 + 右侧控件。
///
/// 刻意不走 `SettingItem::new`：Kit 在 `setting/item.rs` 内部给标题挂死
/// `text_sm()`（0.875rem × `KIT_REM_BASE` 16 = 14px），比项目正文大一号，
/// 而 `SettingItem` 未实现 `Styled`，业务侧没有任何入口把字号压回来
/// （与 `src/ui/fields.rs` 里 Kit `Input` 默认字号的情形同源，那处是直接
/// 在元素自身 `.text_size()` 才生效）。这里改走 `SettingItem::render`
/// 通道自己排版：标题 `TYPE_BODY`(12)、描述 `TYPE_META`(11)，与业务正文
/// 一致；凭据管理页整页自绘正是这个模式。
///
/// 布局复刻 Kit 横向条目：标题列 `flex_1`、控件右对齐，避免长标题把输入框
/// 挤出可见区。控件沿用调用方原有的 `SettingField::render` 闭包，输入框仍
/// 由 `src/ui/fields.rs` 的 Kit 宿主渲染（其字号已压到 `TYPE_BODY`）。
pub(crate) fn settings_item_row<F, E>(
    title: &'static str,
    description: Option<&'static str>,
    field: F,
) -> SettingItem
where
    F: Fn(&RenderOptions, &mut Window, &mut App) -> E + 'static,
    E: IntoElement,
{
    SettingItem::render(move |options, window, cx| {
        settings_item_body(title, description, field(options, window, cx))
    })
    .keywords([title])
}

/// 设置项行内容（Div 级）：排版同 [`settings_item_row`]，`field` 直接收
/// 渲染好的元素，供其与 [`settings_compact_item`] 共用。
pub(crate) fn settings_item_body(
    title: &'static str,
    description: Option<&'static str>,
    field: impl IntoElement,
) -> Div {
    h_flex()
        .w_full()
        .items_center()
        .justify_between()
        .gap_3()
        .child(
            v_flex()
                .flex_1()
                .gap(px(2.0))
                .child(
                    div()
                        .text_size(px(ui_theme::TYPE_BODY))
                        .text_color(rgb(ui_theme::CONTENT_PRIMARY))
                        .child(title),
                )
                .when_some(description, |this, text| {
                    this.child(
                        div()
                            .text_size(px(ui_theme::TYPE_META))
                            .line_height(px(16.0))
                            .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                            .child(text),
                    )
                }),
        )
        .child(field)
}

/// 紧凑分组：整卡内容合并进单个 Kit 条目渲染，行距由业务自控。
///
/// 背景：Kit `SettingGroup` 经 `GroupBox` 渲染，条目间距硬编码 `gap_4`
/// （16px）、内容区内边距 `p_4`，而 `SettingGroup` 的 `Styled` 只 refine
/// 到外层 v_flex（Outline 变体下外层仅 content 一个子元素），业务侧没有
/// 入口压回条目间距。表单型页面（AI、合并工具）迁移自旧弹窗——旧弹窗行距
/// 是 `gap_2`（8px）——迁移后行距翻倍，观感松散。这里走 `SettingItem::render`
/// 通道整卡自排：行容器用 [`settings_compact_list`]，标题用
/// [`settings_compact_heading`]，普通行用 [`settings_item_body`]。
///
/// `keywords` 手动给全组的关键词：合并后整组是一个条目，Kit 按条目过滤
/// 搜索命中，命中即整卡显示。
pub(crate) fn settings_compact_item<E, F>(
    keywords: &'static [&'static str],
    body: F,
) -> SettingItem
where
    F: Fn(&RenderOptions, &mut Window, &mut App) -> E + 'static,
    E: IntoElement,
{
    SettingItem::render(body).keywords(keywords.iter().copied())
}

/// 紧凑分组的行容器：行距 `SPACE_2`（8px），对齐旧弹窗 `gap_2` 的节奏。
pub(crate) fn settings_compact_list() -> Div {
    v_flex().w_full().gap(px(ui_theme::SPACE_2))
}

/// 开关类设置项：与 [`settings_item_row`] 同排版，控件为 Kit `Switch`。
///
/// 同样绕开 `SettingItem::new(.., SettingField::switch(..))`——那条路径的
/// 标题字号不可压。`disabled` / `with_size` 跟随 Kit 传入的 `RenderOptions`，
/// 与其它 Kit 控件的行为保持一致；条目 id 由 Kit 的 `item-N` 外层 div 限定
/// 作用域，多个开关共用 `"check"` 不会冲突。
pub(crate) fn settings_switch_row<V, S>(
    title: &'static str,
    description: Option<&'static str>,
    value: V,
    set_value: S,
) -> SettingItem
where
    V: Fn(&App) -> bool + 'static,
    S: Fn(bool, &mut App) + 'static,
{
    // 条目闭包是 `Fn`（Kit 每次渲染都会调用），`on_click` 却要 move 走写回
    // 闭包；用 `Rc` 共享一份，与 `repository_ui.rs` 的 `toggle_row` 同法。
    let set_value = Rc::new(set_value);
    settings_item_row(title, description, move |options, _window, cx| {
        let set_value = set_value.clone();
        Switch::new("check")
            .checked(value(cx))
            .disabled(options.is_disabled())
            .with_size(options.size())
            .on_click(move |checked: &bool, _, cx: &mut App| set_value(*checked, cx))
    })
}

/// 页头样式：内距与标题/描述间距对齐设计稿（[12,16,8,16]、间距 2px），
/// 覆盖 Kit 默认的 `p_4` + `gap_3`。
///
/// 标题字号必须显式给：Kit 的 `SettingPage` 渲染标题时**不设字号**，标题走
/// 继承，拿到 `TextStyle::default()` 的 `rems(1.)`（× `KIT_REM_BASE` 16 =
/// 16px）。标题取 `TYPE_TITLE`(14) SemiBold：页头要压过正文与设置项标题
/// （`TYPE_BODY` 12），与旁边 `TYPE_META`(11) 的介绍拉开明显一档；不回到
/// Kit 默认 16，避免页头在紧凑设置窗里过重。
fn settings_page_header_style() -> StyleRefinement {
    let mut style = StyleRefinement::default()
        .pt(px(12.0))
        .pr(px(16.0))
        .pb(px(8.0))
        .pl(px(16.0))
        .gap(px(2.0));
    style.text.font_size = Some(px(ui_theme::TYPE_TITLE).into());
    style.text.font_weight = Some(FontWeight::SEMIBOLD);
    style
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
            // 描述不走 `SettingPage::description`：Kit 给它挂死 `text_sm()`
            // （14px），而页头容器样式压不到元素自身。改由 `title_suffix`
            // 自绘，字号取 `TYPE_META`，与分组卡描述一致。
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
                // 标题行是 `h_flex`（默认 items_center）：描述占满剩余宽度，
                // 长文案自然换行，不会把标题挤出可见区。
                div()
                    .flex_1()
                    .text_size(px(ui_theme::TYPE_META))
                    .line_height(px(16.0))
                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                    .child(meta.description)
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
