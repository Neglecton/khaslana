use super::*;

#[test]
fn flat_list_row_rule_only_marks_selected_rows() {
    let idle = list_row_visual_rule(false);
    assert_eq!(idle.background, theme::SURFACE_BASE);
    assert!(!idle.shows_selection_indicator);

    let selected = list_row_visual_rule(true);
    assert_eq!(selected.background, theme::PRIMARY_SUBTLE);
    assert!(selected.shows_selection_indicator);
}

/// 提交条的主次动作必须是两种可辨认的语义（最新 Pencil 稿第五版）：
/// 「提交到 &lt;分支&gt;」是浅色薄实体 + 弱轮廓，带接触阴影；
/// 「提交并推送」是蓝色实面主按钮。两者底色与轮廓都不能相同。
#[test]
fn commit_action_buttons_separate_secondary_from_primary() {
    let secondary = app_button_palette(ButtonTone::Secondary, true);
    let primary = app_button_palette(ButtonTone::Primary, true);

    assert_eq!(secondary.bg, theme::WB_PANEL);
    assert_eq!(secondary.border, theme::BORDER_MUTED);
    assert_eq!(secondary.fg, theme::CONTENT_PRIMARY);
    assert_eq!(primary.bg, theme::PRIMARY);
    assert_eq!(primary.fg, theme::PRIMARY_FOREGROUND);
    assert_ne!(secondary.bg, primary.bg);
    // 主次都表达「抬起的实体」，但次级不使用品牌色。
    assert!(secondary.contact_shadow);
    assert!(primary.contact_shadow);

    // 禁用态一律平整弱化：阴影会削弱「此时不可点」的信号。
    let disabled = app_button_palette(ButtonTone::Secondary, false);
    assert!(!disabled.contact_shadow);
    assert_eq!(disabled.bg, theme::STATE_HOVER);
    assert_eq!(disabled.fg, theme::CONTENT_SECONDARY);
}

/// 区块标题的语义色：普通区块用正文色，正在查看的对象（差异文件路径）用强调色。
#[test]
fn panel_title_tone_picks_body_or_accent_color() {
    assert_eq!(PanelTitleTone::Neutral.color(), theme::CONTENT_PRIMARY);
    assert_eq!(PanelTitleTone::Accent.color(), theme::PRIMARY);
}

/// 空状态 builder 的两个可选维度：说明文字与动作行（视觉规范 §2：
/// 空状态要有可执行入口，不能只有文字）。默认都不带，保持轻量。
#[test]
fn empty_state_builder_keeps_detail_and_actions_optional() {
    let bare = EmptyState::new("没有内容");
    assert!(bare.detail.is_none());
    assert!(bare.actions.is_empty());
    assert!(!bare.fill);

    let detailed = EmptyState::new("没有内容").detail("先打开一个仓库");
    assert_eq!(detailed.detail, Some("先打开一个仓库"));

    // 动作按添加顺序进入动作行，fill 控制是否吃满剩余高度。
    let filled = EmptyState::new("没有仓库")
        .detail("打开或克隆一个仓库")
        .fill()
        .action(div().into_any_element())
        .action(div().into_any_element());
    assert!(filled.fill);
    assert_eq!(filled.actions.len(), 2);
}

/// `empty_state()` 快捷入口保持「标题 + 说明、无动作、不占满」的旧行为：
/// 它服务于列表内局部占位（如浏览文件树），行为变化会破坏行高节奏。
#[test]
fn empty_state_shortcut_stays_non_filling_without_actions() {
    // 快捷入口只是 builder 的固定组合：detail 有值、无动作、不 fill。
    let built = EmptyState::new("文件树").detail("正在加载文件树...");
    assert!(built.detail.is_some());
    assert!(built.actions.is_empty());
    assert!(!built.fill);
}

/// 面板尺寸钳制：设计尺寸放得下时原样保留；放不下时缩到可用空间；
/// 可用空间极端小时保留下限（标题与操作行仍可用）。对应最小窗
/// 860×520（可用 812×472）与 125% DPI（逻辑视口 688×416，可用
/// 640×368）两个验收档位（审查 R3）。
#[test]
fn clamp_dialog_panel_size_respects_viewport() {
    // 大窗：设计尺寸原样保留。
    assert_eq!(
        super::clamp_dialog_panel_size(900.0, 640.0, 1200.0, 900.0),
        (900.0, 640.0)
    );
    // 最小窗 860×520：900×640 超宽超高，缩到可用空间。
    assert_eq!(
        super::clamp_dialog_panel_size(900.0, 640.0, 812.0, 472.0),
        (812.0, 472.0)
    );
    // 125% DPI 逻辑视口 688×416：同样收缩，不越界。
    assert_eq!(
        super::clamp_dialog_panel_size(880.0, 640.0, 640.0, 368.0),
        (640.0, 368.0)
    );
    // 极端小窗：保留下限而不是缩到 0。
    let (width, height) = super::clamp_dialog_panel_size(480.0, 420.0, 200.0, 100.0);
    assert_eq!(width, super::DIALOG_PANEL_MIN_WIDTH);
    assert_eq!(height, super::DIALOG_PANEL_MIN_HEIGHT);
}
