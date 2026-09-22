use super::*;

/// 迁移范围就是注册表本身减去唯一例外。这条断言的意义在于「例外是唯一例外」：
/// `ensure_kit_fields` 遍历的是 `DEDICATED_FIELDS`，若这里多出一个未迁移字段，
/// 说明有人给某个静态字段开了后门（或往 `kit_field_migrated` 里加了新的排除项），
/// 那个字段会悄悄退回旧自绘外观。
#[test]
fn dedicated_fields_are_migrated_except_conflict_editor() {
    for (id, _) in crate::DEDICATED_FIELDS {
        let expected = *id != FieldId::ConflictEditor;
        assert_eq!(
            kit_field_migrated(*id),
            expected,
            "FieldId {id:?} 的迁移判定与预期不符"
        );
    }
}

/// 冲突结果区自 M6 起固定为只读文档视图，`ConflictEditor` 的自绘编辑器路径
/// 已删除；字段只剩 focus 锚点职责，不建任何输入宿主（建了也没有渲染点）。
#[test]
fn conflict_editor_stays_without_input_host() {
    assert!(!kit_field_migrated(FieldId::ConflictEditor));
}

/// 工作流动态字段在 M6 迁入 Kit：它们不进 `DEDICATED_FIELDS`，由
/// `ensure_kit_fields` 末段的「动态字段多退少补」单独维护宿主生命周期。
/// 这里断言它们落在迁移判定内，防止将来有人把排除项加回去。
#[test]
fn workflow_dynamic_fields_are_migrated() {
    assert!(kit_field_migrated(FieldId::WorkflowInput(0)));
    assert!(kit_field_migrated(FieldId::WorkflowEditor(
        crate::workflow_editor::WorkflowEditorFieldId::Name
    )));
}

/// 多行字段必须建多行宿主：`Textarea` 与 `Input` 是两套状态类型，把多行字段按
/// 单行建会静默丢掉换行、自动增高与内部滚动。
#[test]
fn multiline_fields_use_textarea_state() {
    for id in [FieldId::CommitMessage, FieldId::TagMessage] {
        assert!(
            RepositoryView::is_multiline_field(id),
            "FieldId {id:?} 应走多行宿主"
        );
        assert!(kit_field_migrated(id), "FieldId {id:?} 应在迁移范围内");
    }
    // 单行字段不应被误判成多行（多行宿主会接受换行、Enter 语义也随之改变）。
    for id in [FieldId::CloneUrl, FieldId::TagName, FieldId::AiModel] {
        assert!(!RepositoryView::is_multiline_field(id));
    }
}

#[test]
fn textarea_press_enter_never_submits_after_mutating_the_value() {
    assert!(!kit_press_enter_submits(true));
    assert!(kit_press_enter_submits(false));
}

/// 宿主身份决定「同一个输入框」还是「同一个位置换了内容」。文本比较
/// 看不出来的是 uid：两份业务字段恰好同文本时，只有 uid 变化才需要
/// 完整重绑（审查 R5：切换模板后 Ctrl+Z 不得回到前一模板的编辑）。
#[test]
fn kit_field_needs_rebind_distinguishes_object_replacement() {
    let host = super::KitFieldIdentity {
        uid: 7,
        placeholder: "模板 A 的分支名".into(),
    };
    // 同一份业务真值：值稳定后每帧同步都不重绑（不清撤销历史）。
    assert!(!super::kit_field_needs_rebind(
        &host,
        7,
        &"模板 A 的分支名".into()
    ));
    // 换了业务对象（uid 变）但文本/占位符相同：必须重绑。
    assert!(super::kit_field_needs_rebind(
        &host,
        8,
        &"模板 A 的分支名".into()
    ));
    // 同一对象换了占位符来源（label 变化）：必须重绑。
    assert!(super::kit_field_needs_rebind(
        &host,
        7,
        &"模板 B 的分支名".into()
    ));
}
