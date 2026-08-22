use super::*;

/// 构造最小可用编辑数据：一个 checkout 步骤 + 名称。
fn minimal_data() -> WorkflowEditorData {
    let mut data = WorkflowEditorData::default();
    data.name = "测试模板".to_string();
    data.file_name = "test-template".to_string();
    let mut step = WorkflowEditorStepData::new(WorkflowStepKind::Checkout);
    step.branch = "master".to_string();
    data.steps = vec![step];
    data
}

#[test]
fn build_workflow_definition_rejects_empty_steps() {
    let data = WorkflowEditorData::default();
    let err = build_workflow_definition(&data).unwrap_err();
    assert_eq!(err, "至少需要一个步骤");
}

#[test]
fn build_workflow_definition_rejects_missing_required_slot() {
    let mut data = minimal_data();
    data.steps[0].branch = "  ".to_string();
    let err = build_workflow_definition(&data).unwrap_err();
    assert!(err.contains("第 1 个步骤"), "错误应指明步骤序号：{err}");
    assert!(err.contains("切换分支"), "错误应含步骤类型名：{err}");
    assert!(err.contains("分支"), "错误应含缺失槽名：{err}");
}

#[test]
fn build_workflow_definition_builds_checkout_step() {
    let data = minimal_data();
    let definition = build_workflow_definition(&data).unwrap();
    assert_eq!(definition.version, 1);
    assert_eq!(definition.name.as_deref(), Some("测试模板"));
    assert!(definition.defaults.require_clean_worktree);
    assert_eq!(
        definition.steps,
        vec![khaslana::WorkflowStep::Checkout {
            branch: "master".to_string()
        }]
    );
}

#[test]
fn build_workflow_definition_omits_empty_optional_fields() {
    let mut data = minimal_data();
    data.name = "   ".to_string();
    // push 分支留空 -> 序列化为省略（None）
    data.steps
        .push(WorkflowEditorStepData::new(WorkflowStepKind::Push));
    let definition = build_workflow_definition(&data).unwrap();
    assert_eq!(definition.name, None);
    match &definition.steps[1] {
        khaslana::WorkflowStep::Push {
            remote,
            branch,
            set_upstream,
        } => {
            assert_eq!(remote, &None);
            assert_eq!(branch, &None);
            assert!(*set_upstream, "push 默认建立上游跟踪");
        }
        other => panic!("第二个步骤应为 Push：{other:?}"),
    }
}

#[test]
fn build_workflow_definition_validates_inputs() {
    // 变量名为空
    let mut data = minimal_data();
    data.inputs.push(WorkflowEditorInputRowData {
        key: " ".to_string(),
        label: String::new(),
        default_value: String::new(),
        required: true,
    });
    let err = build_workflow_definition(&data).unwrap_err();
    assert!(err.contains("变量名为空"), "实际：{err}");

    // 保留前缀
    let mut data = minimal_data();
    data.inputs.push(WorkflowEditorInputRowData {
        key: "git.branch".to_string(),
        label: String::new(),
        default_value: String::new(),
        required: true,
    });
    let err = build_workflow_definition(&data).unwrap_err();
    assert!(err.contains("保留前缀"), "实际：{err}");

    // 重复
    let mut data = minimal_data();
    for _ in 0..2 {
        data.inputs.push(WorkflowEditorInputRowData {
            key: "target".to_string(),
            label: String::new(),
            default_value: String::new(),
            required: true,
        });
    }
    let err = build_workflow_definition(&data).unwrap_err();
    assert!(err.contains("重复"), "实际：{err}");

    // 正常：空 label/default 转 None，非空保留
    let mut data = minimal_data();
    data.inputs.push(WorkflowEditorInputRowData {
        key: "target".to_string(),
        label: " 目标分支 ".to_string(),
        default_value: String::new(),
        required: false,
    });
    let definition = build_workflow_definition(&data).unwrap();
    let input = &definition.inputs["target"];
    assert_eq!(input.label.as_deref(), Some("目标分支"));
    assert_eq!(input.default, None);
    assert!(!input.required);
}

#[test]
fn build_workflow_definition_validates_vars() {
    let mut data = minimal_data();
    data.vars.push(WorkflowEditorVarRowData {
        key: "run.id".to_string(),
        value: "x".to_string(),
    });
    let err = build_workflow_definition(&data).unwrap_err();
    assert!(err.contains("保留前缀"), "实际：{err}");

    let mut data = minimal_data();
    data.vars.push(WorkflowEditorVarRowData {
        key: "remote".to_string(),
        value: " origin ".to_string(),
    });
    let definition = build_workflow_definition(&data).unwrap();
    assert_eq!(definition.vars["remote"], "origin", "值应去首尾空白");
}

#[test]
fn file_name_validation() {
    // 空名拒绝
    assert!(workflow_editor_file_name("  ", &[]).is_err());
    // 非法字符拒绝
    for bad in [
        "a/b", "a\\b", "a:b", "a*b", "a?b", "a\"b", "a<b", "a>b", "a|b",
    ] {
        let err = workflow_editor_file_name(bad, &[]).unwrap_err();
        assert!(err.contains("非法字符"), "「{bad}」应报非法字符：{err}");
    }
    // 已存在的 .json5 / .jsonc 后缀剥掉再加 .json5
    assert_eq!(
        workflow_editor_file_name("my-flow.json5", &[]).unwrap(),
        "my-flow.json5"
    );
    assert_eq!(
        workflow_editor_file_name(" my-flow.jsonc ", &[]).unwrap(),
        "my-flow.json5"
    );
    // 重名拒绝（大小写不敏感）
    let existing = vec!["My-Flow.json5".to_string()];
    let err = workflow_editor_file_name("my-flow", &existing).unwrap_err();
    assert!(err.contains("同名"), "实际：{err}");
    // 合法名
    assert_eq!(
        workflow_editor_file_name("feature-2026", &[]).unwrap(),
        "feature-2026.json5"
    );
}

#[test]
fn presets_build_valid_definitions() {
    for preset in [
        WorkflowEditorPreset::SyncCurrentBranch,
        WorkflowEditorPreset::FeatureBranch,
        WorkflowEditorPreset::MergeAndPush,
    ] {
        let data = preset.build_data();
        let definition = build_workflow_definition(&data)
            .unwrap_or_else(|err| panic!("预设 {:?} 应生成合法定义：{err}", preset));
        // 序列化 -> 回读 round-trip（与保存路径完全一致）
        let serialized = json5::to_string(&definition).unwrap();
        let reparsed = parse_workflow_json5(&serialized).unwrap();
        assert_eq!(reparsed, definition, "预设 {:?} 往返不一致", preset);
        // 预设文件名合法且不与自身重名
        workflow_editor_file_name(&data.file_name, &[]).unwrap_or_else(|err| {
            panic!("预设 {:?} 文件名应合法：{err}", preset);
        });
    }
}

#[test]
fn preset_feature_branch_references_input_var() {
    let data = WorkflowEditorPreset::FeatureBranch.build_data();
    // target 输入变量被 createBranch 与 push 引用
    let definition = build_workflow_definition(&data).unwrap();
    assert!(definition.inputs.contains_key("target"));
    match &definition.steps[2] {
        khaslana::WorkflowStep::CreateBranch { name, .. } => {
            assert_eq!(name, "${target}");
        }
        other => panic!("第三个步骤应为 CreateBranch：{other:?}"),
    }
}

#[test]
fn step_draft_summaries_show_pending_placeholder() {
    let empty = WorkflowEditorStepData::new(WorkflowStepKind::Checkout);
    assert_eq!(
        workflow_step_draft_summary(WorkflowStepKind::Checkout, &empty),
        "切换到分支 （未填写）"
    );
    let mut push = WorkflowEditorStepData::new(WorkflowStepKind::Push);
    push.branch = "dev".to_string();
    assert_eq!(
        workflow_step_draft_summary(WorkflowStepKind::Push, &push),
        "推送分支 dev"
    );
    let pull = WorkflowEditorStepData::new(WorkflowStepKind::Pull);
    assert_eq!(
        workflow_step_draft_summary(WorkflowStepKind::Pull, &pull),
        "拉取（默认远端）"
    );
    let clean = WorkflowEditorStepData::new(WorkflowStepKind::EnsureClean);
    assert_eq!(
        workflow_step_draft_summary(WorkflowStepKind::EnsureClean, &clean),
        "检查工作区干净"
    );
}

#[test]
fn slot_kind_metadata_consistency() {
    // all() 覆盖 11 种且常用 6 种在前
    let all = WorkflowStepKind::all();
    assert_eq!(all.len(), 11);
    assert!(all.iter().take(6).all(|kind| kind.is_common()));
    assert!(all.iter().skip(6).all(|kind| !kind.is_common()));
    // op_name 与 from_op_name 互逆
    for kind in all {
        assert_eq!(WorkflowStepKind::from_op_name(kind.op_name()), Some(kind));
    }
    assert_eq!(WorkflowStepKind::from_op_name("nonsense"), None);
    // ensureClean 无参数
    assert!(WorkflowStepKind::EnsureClean.slots().is_empty());
}

#[test]
fn step_slot_values_survive_kind_switch() {
    // 切换步骤类型时同槽位值保留：checkout -> merge 分支名不丢
    let mut step = WorkflowEditorStepData::new(WorkflowStepKind::Checkout);
    step.branch = "release".to_string();
    step.set_slot_value(WorkflowStepSlot::Remote, "upstream".to_string());
    // 模拟 workflow_editor_set_step_kind 的数据层效果（只改 kind）
    let data_step = WorkflowEditorStepData {
        kind: WorkflowStepKind::Merge,
        ..step
    };
    assert_eq!(data_step.branch, "release");
    assert_eq!(data_step.remote, "upstream");
}

#[test]
fn guard_step_defaults_follow_domain_convention() {
    // 领域层约定：默认 onExists=Fail、onMissing=Continue、fetch=true
    let guard = WorkflowEditorStepData::new(WorkflowStepKind::GuardRemoteBranch);
    assert_eq!(guard.on_exists, RemoteBranchGuardAction::Fail);
    assert_eq!(guard.on_missing, RemoteBranchGuardAction::Continue);
    assert!(guard.guard_fetch);
}

#[test]
fn delete_branches_defaults_to_dry_run() {
    // 领域层约定：默认 dryRun=true（安全第一）、skipCurrent=true
    let delete = WorkflowEditorStepData::new(WorkflowStepKind::DeleteBranches);
    assert!(delete.delete_dry_run);
    assert!(delete.delete_skip_current);
    let mut data = minimal_data();
    data.steps[0] = {
        let mut step = WorkflowEditorStepData::new(WorkflowStepKind::DeleteBranches);
        step.branches = "${out.stale}".to_string();
        step
    };
    let definition = build_workflow_definition(&data).unwrap();
    match &definition.steps[0] {
        khaslana::WorkflowStep::DeleteBranches {
            branches,
            dry_run,
            skip_current,
        } => {
            assert_eq!(branches, "${out.stale}");
            assert!(*dry_run);
            assert!(*skip_current);
        }
        other => panic!("应为 DeleteBranches：{other:?}"),
    }
}
