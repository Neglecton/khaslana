use std::fs;
use std::path::Path;

use super::*;

#[test]
fn workflow_template_dir_uses_visible_home_directory() {
    let dir = workflow_templates_dir_from_home(Path::new(r"C:\Users\tester"));

    assert!(dir.ends_with(Path::new(".khaslana").join("workflows")));
}

#[test]
fn workflow_template_scan_only_includes_json5_and_jsonc_files() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(
        temp.path().join("a.json5"),
        "{ version: 1, name: \"A\", steps: [{ op: \"ensureClean\" }] }",
    )
    .unwrap();
    fs::write(
        temp.path().join("b.jsonc"),
        "{ version: 1, name: \"B\", steps: [{ op: \"ensureClean\" }] }",
    )
    .unwrap();
    fs::write(
        temp.path().join("ignored.json"),
        "{ version: 1, steps: [{ op: \"ensureClean\" }] }",
    )
    .unwrap();

    let templates = load_workflow_templates_from_dir(temp.path()).unwrap();

    assert_eq!(templates.len(), 2);
    assert_eq!(
        templates
            .iter()
            .map(|template| template.file_name.as_str())
            .collect::<Vec<_>>(),
        vec!["a.json5", "b.jsonc"]
    );
}

#[test]
fn workflow_template_scan_keeps_invalid_template_with_error() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(
        temp.path().join("valid.json5"),
        "{ version: 1, name: \"可用模板\", steps: [{ op: \"ensureClean\" }] }",
    )
    .unwrap();
    fs::write(temp.path().join("broken.json5"), "{ version: 1, steps: [").unwrap();

    let templates = load_workflow_templates_from_dir(temp.path()).unwrap();
    let valid = templates
        .iter()
        .find(|template| template.file_name == "valid.json5")
        .unwrap();
    let broken = templates
        .iter()
        .find(|template| template.file_name == "broken.json5")
        .unwrap();

    assert_eq!(valid.display_name, "可用模板");
    assert!(valid.error.is_none());
    assert_eq!(broken.display_name, "broken");
    assert!(broken.error.is_some());
}

#[test]
fn workflow_selected_template_path_clears_external_or_stale_selection() {
    let current = Path::new(r"C:\\workflows\\current.json5");
    let external = Path::new(r"C:\\other\\external.json5");
    let templates = vec![WorkflowTemplateItem {
        path: current.to_path_buf(),
        display_name: "当前".to_string(),
        file_name: "current.json5".to_string(),
        modified_label: "修改时间未知".to_string(),
        error: None,
    }];

    assert_eq!(
        workflow_selected_template_path(Some(current), &templates),
        Some(current.to_path_buf())
    );
    assert_eq!(
        workflow_selected_template_path(Some(external), &templates),
        None
    );
}

#[test]
fn workflow_template_list_model_keeps_large_lists_as_indices() {
    let templates = (0..10_000)
        .map(|index| WorkflowTemplateItem {
            path: Path::new("templates").join(format!("{index}.json5")),
            display_name: format!("模板 {index}"),
            file_name: format!("{index}.json5"),
            modified_label: "修改时间未知".to_string(),
            error: None,
        })
        .collect::<Vec<_>>();

    let model = WorkflowTemplateListModel::from_templates(&templates);

    assert_eq!(model.len(), templates.len());
    assert_eq!(model.index_at(0), Some(0));
    assert_eq!(model.index_at(9_999), Some(9_999));
    assert_eq!(model.index_at(10_000), None);
    // 模型只保存下标；模板内容和 AnyElement 都由 uniform_list 的可视回调按需读取。
    assert_eq!(
        model,
        WorkflowTemplateListModel {
            indices: (0..10_000).collect(),
        }
    );
}

// dir_has_workflow_template 仅识别 .json5/.jsonc 文件。
#[test]
fn dir_has_workflow_template_detects_template_files() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    assert!(!dir_has_workflow_template(dir));
    fs::write(dir.join("readme.txt"), "x").unwrap();
    assert!(!dir_has_workflow_template(dir));
    fs::write(dir.join("wf.json5"), "{}").unwrap();
    assert!(dir_has_workflow_template(dir));
}

// copy_workflow_templates 递归拷贝模板文件，跳过非模板，且同名文件不覆盖。
#[test]
fn copy_workflow_templates_copies_only_templates_without_overwrite() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    let dst = tmp.path().join("dst");
    fs::create_dir_all(src.join("sub")).unwrap();
    fs::write(src.join("a.json5"), "a").unwrap();
    fs::write(src.join("sub").join("b.jsonc"), "b").unwrap();
    fs::write(src.join("note.md"), "skip").unwrap();

    copy_workflow_templates(&src, &dst).unwrap();

    assert!(dst.join("a.json5").exists());
    assert!(dst.join("sub").join("b.jsonc").exists());
    assert!(!dst.join("note.md").exists());

    // 同名文件不覆盖。
    fs::write(dst.join("a.json5"), "keep").unwrap();
    copy_workflow_templates(&src, &dst).unwrap();
    assert_eq!(fs::read_to_string(dst.join("a.json5")).unwrap(), "keep");
}

#[test]
fn workflow_studio_layout_uses_preview_only_when_no_inputs() {
    assert_eq!(
        workflow_studio_layout(false, false, false, 0),
        WorkflowStudioLayout::NoDefinition
    );
    assert_eq!(
        workflow_studio_layout(true, false, false, 0),
        WorkflowStudioLayout::PreviewOnly
    );
    assert_eq!(
        workflow_studio_layout(true, true, false, 0),
        WorkflowStudioLayout::InputsAndPreview
    );
}

#[test]
fn workflow_console_expands_only_for_busy_or_existing_log() {
    assert_eq!(
        workflow_console_state(false, 0),
        WorkflowConsoleState::Collapsed
    );
    assert_eq!(
        workflow_console_state(true, 0),
        WorkflowConsoleState::Expanded
    );
    // 完成态与失败态都由已有日志驱动展开，不新增额外的状态事件字段。
    assert_eq!(
        workflow_console_state(false, 1),
        WorkflowConsoleState::Expanded
    );
    assert_eq!(
        workflow_console_state(false, 2),
        WorkflowConsoleState::Expanded
    );
}

#[test]
fn workflow_template_click_loads_on_standard_click_not_only_double_click() {
    assert!(workflow_template_click_loads(true, false));
    assert!(!workflow_template_click_loads(false, false));
    assert!(!workflow_template_click_loads(true, true));
}

#[test]
fn workflow_template_keyboard_activation_only_accepts_enter_or_space_when_idle() {
    assert!(workflow_template_key_activates("enter", false));
    assert!(workflow_template_key_activates("space", false));
    assert!(!workflow_template_key_activates("escape", false));
    assert!(!workflow_template_key_activates("enter", true));
    assert!(!workflow_template_key_activates("space", true));
}

#[test]
fn workflow_content_scroll_ids_are_distinct() {
    assert_ne!(
        WORKFLOW_INPUT_LIST_SCROLL_ID,
        WORKFLOW_PREVIEW_LIST_SCROLL_ID
    );
    assert_ne!(WORKFLOW_INPUT_LIST_SCROLL_ID, WORKFLOW_LOG_LIST_SCROLL_ID);
    assert_ne!(WORKFLOW_PREVIEW_LIST_SCROLL_ID, WORKFLOW_LOG_LIST_SCROLL_ID);
}

#[test]
fn workflow_template_selection_requires_loaded_path_match() {
    let template = Path::new(r"C:\workflows\deploy.json5");
    let other = Path::new(r"C:\workflows\other.json5");
    assert!(workflow_template_selection_matches(
        Some(template),
        Some(template),
        Some(template)
    ));
    assert!(!workflow_template_selection_matches(
        Some(template),
        Some(other),
        Some(template)
    ));
    assert!(!workflow_template_selection_matches(
        None,
        Some(template),
        Some(template)
    ));
}
