use std::fs;

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
