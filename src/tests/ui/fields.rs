use super::*;

/// 迁移范围就是注册表本身减去三个例外。这条断言的意义在于「例外是唯一例外」：
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

/// 冲突草稿走 `input()` 的第一个分支（专用渲染），不能同时建 Kit 宿主：
/// 两个宿主并存时 `kit_field_focused` 会让键盘整体让位，草稿的按块接受、
/// 语法高亮与三栏联动全部失效。
#[test]
fn conflict_editor_stays_on_its_own_renderer() {
    assert!(!kit_field_migrated(FieldId::ConflictEditor));
}

/// 工作流动态字段随工作流页在 M6 迁移：它们的 `TextFieldState` 按模板动态增删，
/// 宿主生命周期与静态字段不同，混进来会在切换模板时留下悬空宿主。
#[test]
fn workflow_dynamic_fields_are_not_migrated() {
    assert!(!kit_field_migrated(FieldId::WorkflowInput(0)));
    assert!(!kit_field_migrated(FieldId::WorkflowEditor(
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
