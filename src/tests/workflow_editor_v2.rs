use super::*;

fn roundtrip(text: &str) -> WorkflowDefinition {
    let original = parse_workflow_json5(text).unwrap();
    let editor = workflow_editor_data_from_definition(&original, "demo").unwrap();
    assert_eq!(editor.version, original.version);
    let rebuilt = build_workflow_definition(&editor).unwrap();
    assert_eq!(rebuilt, original);
    rebuilt
}

#[test]
fn v2_roundtrips_mixed_git_mcp_js_skill_and_guards() {
    roundtrip(r#"{version:2, name:'混合工作流', inputs:{target:{default:'feature/demo'}}, vars:{url:'https://example.com'}, steps:[
      {op:'createBranch',name:'${target}',checkout:true},
      {op:'invoke',id:'mcp',uses:'mcp.call',with:{server:'browser.edge',tool:'fill',arguments:{value:'${target}',nested:{enabled:true}},browserGuard:{url:'${url}'}},saveAs:'browser'},
      {op:'invoke',id:'js',uses:'js.run',with:{script:'return input;',input:{branch:'${target}',values:[1,true,null]},tools:[]},saveAs:'data'},
      {op:'invoke',id:'skill',uses:'skill.run',with:{skill:'demo',task:'填写 ${target}',tools:[{server:'browser.edge',tool:'browser_type'}],custom:{enabled:false}}}
    ]}"#);
}

#[test]
fn v2_unknown_and_git_invokes_preserve_complete_parameters() {
    roundtrip(r#"{version:2,steps:[
      {op:'invoke',id:'git',uses:'git.createBranch',with:{name:'${target}',checkout:false}},
      {op:'invoke',id:'custom',uses:'extension.custom',with:{array:[null,1,{key:true}],unknown:'原值'},saveAs:'output'}
    ]}"#);
}

#[test]
fn v1_stays_v1_after_editing_and_v2_legacy_steps_stay_v2() {
    roundtrip("{version:1,steps:[{op:'ensureClean'}]}");
    roundtrip("{version:2,steps:[{op:'ensureClean'}]}");
}

#[test]
fn structured_parameters_override_extras_without_losing_guard() {
    let definition = parse_workflow_json5(r#"{version:2,steps:[{op:'invoke',id:'a',uses:'skill.run',with:{skill:'demo',task:'旧任务',browserGuard:{url:'https://example.com',target:'input',contains:['Text input']}}}]}"#).unwrap();
    let mut editor = workflow_editor_data_from_definition(&definition, "demo").unwrap();
    editor.steps[0].set_slot_value(WorkflowStepSlot::Task, "新任务 ${target}".into());
    let built = build_workflow_definition(&editor).unwrap();
    let WorkflowStep::Invoke { arguments, .. } = &built.steps[0] else { panic!("应为 invoke"); };
    assert_eq!(arguments["task"], "新任务 ${target}");
    assert_eq!(arguments["browserGuard"]["url"], "https://example.com");
}

#[test]
fn structured_json_remains_typed_and_accepts_json5() {
    let mut action = InvokeEditorData::new_action("js.run", "js");
    action.set(WorkflowStepSlot::Script, "return input;".into());
    action.set(WorkflowStepSlot::JsInput, "{enabled:true, values:[1,null,],}".into());
    let WorkflowStep::Invoke { arguments, .. } = action.build().unwrap() else { panic!("应为 invoke"); };
    assert_eq!(arguments["input"]["enabled"], true);
    assert_eq!(arguments["input"]["values"][0], 1);
    assert!(arguments["input"]["values"][1].is_null());
}

#[test]
fn malformed_and_non_object_parameters_are_rejected() {
    let mut action = InvokeEditorData::new_action("custom.run", "a");
    for invalid in ["[1]", "null", "{broken"] {
        action.set(WorkflowStepSlot::Arguments, invalid.into());
        assert!(action.build().is_err());
    }
    action = InvokeEditorData::new_action("js.run", "a");
    action.set(WorkflowStepSlot::JsInput, "{broken".into());
    assert!(action.build().unwrap_err().contains("脚本输入"));
}

#[test]
fn v1_cannot_silently_receive_ai_v2_actions() {
    let mut editor = WorkflowEditorData::default();
    let original = editor.clone();
    assert!(apply_ai_generated_to_editor_data(&mut editor, "{version:2,steps:[{op:'invoke',id:'a',uses:'js.run',with:{script:'return 1;'}}]}").is_err());
    assert_eq!(editor.version, original.version);
    assert_eq!(editor.steps.len(), original.steps.len());
}

#[test]
fn ai_legacy_response_preserves_current_v2_editor_version() {
    let mut editor = WorkflowEditorData { version:2, ..Default::default() };
    apply_ai_generated_to_editor_data(&mut editor, "{version:1,steps:[{op:'ensureClean'}]}").unwrap();
    assert_eq!(build_workflow_definition(&editor).unwrap().version, 2);
}

#[test]
fn generated_step_id_avoids_existing_ids_after_reorder() {
    let mut first = WorkflowEditorStepData::new(WorkflowStepKind::Invoke);
    first.invoke = InvokeEditorData::new_action("js.run", "step-2");
    let mut second = first.clone();
    second.invoke.set(WorkflowStepSlot::StepId, "step-1".into());
    let mut steps = vec![first, second];
    assert_eq!(super::super::document::next_step_id(&steps), "step-3");
    steps.swap(0,1);
    assert_eq!(super::super::document::next_step_id(&steps), "step-3");
}

#[test]
fn variable_rail_uses_logical_width_threshold() {
    assert!(!super::super::document::variable_rail_visible(860.0));
    assert!(!super::super::document::variable_rail_visible(1099.0));
    assert!(super::super::document::variable_rail_visible(1100.0));
}

#[test]
fn installed_edge_examples_roundtrip_without_losing_guards_or_permissions() {
    roundtrip(include_str!("../../docs/examples/workflow-edge-demo/edge-web-form.json5"));
    roundtrip(include_str!("../../docs/examples/workflow-edge-demo/edge-web-form-skill.json5"));
}

#[test]
fn step_identity_and_reserved_output_fail_before_saving() {
    let definition = parse_workflow_json5("{version:2,steps:[{op:'invoke',id:'a',uses:'js.run',with:{script:'return 1;'}}]}").unwrap();
    let mut editor = workflow_editor_data_from_definition(&definition, "demo").unwrap();
    editor.steps.push(editor.steps[0].clone());
    assert!(build_workflow_definition(&editor).unwrap_err().contains("重复"));
    editor.steps.pop();
    editor.steps[0].invoke.set(WorkflowStepSlot::SaveAs, "git.head".into());
    assert!(build_workflow_definition(&editor).unwrap_err().contains("内置变量"));
}

#[test]
fn tool_selection_preserves_other_entries_and_handles_unchecking() {
    let first = super::super::document::edit_tool_selection("", "browser.edge", "browser_type", true).unwrap();
    let second = super::super::document::edit_tool_selection(&first, "custom", "read", true).unwrap();
    let third = super::super::document::edit_tool_selection(&second, "browser.edge", "browser_type", false).unwrap();
    let entries: serde_json::Value = serde_json::from_str(&third).unwrap();
    assert_eq!(entries, serde_json::json!([{ "server":"custom", "tool":"read" }]));
    assert!(super::super::document::edit_tool_selection("{broken", "custom", "read", true).is_err());
    assert!(super::super::document::edit_tool_selection("{}", "custom", "read", true).is_err());
}

#[test]
fn v2_output_binding_only_resolves_earlier_steps() {
    let definition = parse_workflow_json5("{version:2,steps:[{op:'invoke',id:'a',uses:'js.run',with:{script:'return input;',input:{}},saveAs:'data'},{op:'invoke',id:'b',uses:'skill.run',with:{skill:'demo',task:'读取 ${out.data.branch}'}}]}").unwrap();
    let editor = workflow_editor_data_from_definition(&definition, "demo").unwrap();
    assert!(workflow_step_binding(1, &editor.steps, &[], &[]).unknown.is_empty());
    let mut reversed = editor.steps.clone();
    reversed.swap(0,1);
    assert_eq!(workflow_step_binding(0, &reversed, &[], &[]).unknown, vec!["out.data.branch"]);
}

#[test]
fn v1_upgrade_is_a_copy_and_keeps_original_target_and_inputs() {
    let definition = parse_workflow_json5("{version:1,inputs:{target:{default:'feature/demo'}},steps:[{op:'createBranch',name:'${target}'}]}").unwrap();
    let mut original = workflow_editor_data_from_definition(&definition, "existing").unwrap();
    original.editing_path = Some(PathBuf::from("existing.json5"));
    let copy = super::super::document::v2_copy_data(&original);
    assert_eq!(original.version, 1);
    assert!(original.editing_path.is_some());
    assert_eq!(copy.version, 2);
    assert_eq!(copy.file_name, "existing-v2");
    assert!(copy.editing_path.is_none());
    assert_eq!(build_workflow_definition(&copy).unwrap().inputs, definition.inputs);
    assert_eq!(build_workflow_definition(&copy).unwrap().steps, definition.steps);
}

#[test]
fn duplication_preserves_parameters_but_allocates_new_identity_and_output() {
    let definition = parse_workflow_json5("{version:2,steps:[{op:'invoke',id:'step-1',uses:'js.run',with:{script:'return input;',input:{x:1}},saveAs:'original'}]}").unwrap();
    let editor = workflow_editor_data_from_definition(&definition, "demo").unwrap();
    let copy = super::super::document::duplicate_step_data(&editor.steps, 0).unwrap();
    let WorkflowStep::Invoke { id, arguments, save_as, .. } = copy.invoke.build().unwrap() else { panic!("应为 invoke"); };
    assert_eq!(id, "step-2");
    assert_eq!(save_as, None);
    assert_eq!(arguments["input"]["x"], 1);
    assert_eq!(editor.steps[0].invoke.value(WorkflowStepSlot::SaveAs), "original");
    assert!(super::super::document::duplicate_step_data(&editor.steps, 10).is_none());
}
