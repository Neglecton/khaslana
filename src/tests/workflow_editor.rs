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
        description: String::new(),
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
        description: String::new(),
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
            description: String::new(),
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
        description: String::new(),
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
    // 前 COMMON_COUNT 个为常用组（菜单据此分组），其余为高级组。
    let common_count = WorkflowStepKind::COMMON_COUNT;
    assert_eq!(common_count, 6);
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

#[test]
fn content_has_comments_detection() {
    // 无注释
    assert!(!workflow_content_has_comments(
        r#"{"version":1,"name":"x","steps":[{"op":"checkout","branch":"main"}]}"#
    ));
    assert!(!workflow_content_has_comments(""));
    // 行注释
    assert!(workflow_content_has_comments(
        "{\n  // 目标分支\n  version: 1\n}"
    ));
    // 块注释
    assert!(workflow_content_has_comments(
        "{\n/* 头部说明 */\nversion: 1\n}"
    ));
    // 字符串里的 // 不是注释（URL 场景）
    assert!(!workflow_content_has_comments(
        r#"{"url": "https://example.com/repo.git"}"#
    ));
    // 字符串里的 /* 也不是注释
    assert!(!workflow_content_has_comments(r#"{"a": "x /* y"}"#));
    // 转义引号后的内容仍在字符串外判定
    assert!(!workflow_content_has_comments(
        r#"{"a": "say \"hi\" https://x"}"#
    ));
    assert!(workflow_content_has_comments(r#"{"a": "ok"} // tail"#));
}

#[test]
fn definition_to_editor_data_round_trip_all_variants() {
    // 领域定义 -> 编辑数据 -> 领域定义，结构应恒等（覆盖全部 11 变体 + description）。
    let original = WorkflowDefinition {
        version: 1,
        name: Some("往返".to_string()),
        defaults: khaslana::WorkflowDefaults {
            require_clean_worktree: false,
        },
        inputs: {
            let mut m = std::collections::BTreeMap::new();
            m.insert(
                "target".to_string(),
                WorkflowInputDefinition {
                    label: Some("目标分支".to_string()),
                    description: Some("要创建的分支名".to_string()),
                    default: Some("feature-x".to_string()),
                    required: true,
                },
            );
            m
        },
        vars: {
            let mut m = std::collections::BTreeMap::new();
            m.insert("remote".to_string(), "origin".to_string());
            m
        },
        steps: vec![
            WorkflowStep::Checkout {
                branch: "master".into(),
            },
            WorkflowStep::Fetch { remote: None },
            WorkflowStep::Pull {
                remote: Some("up".into()),
            },
            WorkflowStep::CreateBranch {
                name: "f".into(),
                from: None,
                checkout: false,
            },
            WorkflowStep::Merge {
                branch: "dev".into(),
            },
            WorkflowStep::Push {
                remote: None,
                branch: Some("f".into()),
                set_upstream: true,
            },
            WorkflowStep::GuardRemoteBranch {
                remote: Some("origin".into()),
                branch: "r".into(),
                fetch: false,
                on_exists: RemoteBranchGuardAction::Continue,
                on_missing: RemoteBranchGuardAction::Fail,
            },
            WorkflowStep::EnsureClean,
            WorkflowStep::AssertBranch { branch: "f".into() },
            WorkflowStep::FilterBranches {
                output: "stale".into(),
                pattern: r"u_(?<date>\d{8})".into(),
                date_format: "%Y%m%d".into(),
                date_group: "date".into(),
                older_than_months: "2".into(),
                skip_current: false,
            },
            WorkflowStep::DeleteBranches {
                branches: "${out.stale}".into(),
                dry_run: false,
                skip_current: true,
            },
        ],
    };

    let data = workflow_editor_data_from_definition(&original, "round-trip");
    assert_eq!(data.file_name, "round-trip");
    assert_eq!(data.inputs.len(), 1);
    assert_eq!(data.inputs[0].description, "要创建的分支名");
    let rebuilt = build_workflow_definition(&data).expect("编辑数据应能重建定义");
    assert_eq!(rebuilt, original, "反映射往返应保持定义恒等");
}

#[test]
fn editor_data_from_definition_sets_file_name_only() {
    // 反映射不设置 editing_path（由 open_workflow_editor_for_path 按模式补）。
    let definition = minimal_data();
    let built = build_workflow_definition(&definition).unwrap();
    let data = workflow_editor_data_from_definition(&built, "some-name");
    assert!(data.editing_path.is_none());
    assert_eq!(data.name, "测试模板");
}

#[test]
fn editor_data_from_definition_empty_name_round_trips() {
    // 名称为空白的定义 -> 反映射后编辑器名留空，重建仍为 None
    let mut definition = minimal_data();
    definition.name = "   ".to_string();
    let built = build_workflow_definition(&definition).unwrap();
    assert_eq!(built.name, None);
    let data = workflow_editor_data_from_definition(&built, "n");
    assert_eq!(data.name, "");
}

#[test]
fn file_name_validation_excludes_self_on_edit() {
    // 编辑模式下重名校验需排除自身文件名（保存逻辑的过滤行为验证）。
    let existing = vec!["a.json5".to_string(), "My-Flow.json5".to_string()];
    let self_name = "my-flow.json5".to_string();
    let filtered: Vec<String> = existing
        .iter()
        .filter(|name| Some(name.to_uppercase()) != Some(self_name.to_uppercase()))
        .cloned()
        .collect();
    // 自身被排除后不再误报重名
    assert_eq!(
        workflow_editor_file_name("my-flow", &filtered).unwrap(),
        "My-Flow.json5".to_lowercase().replace(".json5", "") + ".json5"
    );
    // 其它模板仍会拦截
    let err = workflow_editor_file_name("a", &existing).unwrap_err();
    assert!(err.contains("同名"));
}

#[test]
fn save_target_overwrites_same_stem_with_any_extension() {
    // .jsonc 原件：主干相同 -> 原地覆盖（保留原扩展名），不产生新文件
    let (target, stale) =
        workflow_editor_save_target(Some(Path::new("D:/t/my-flow.jsonc")), "my-flow.json5");
    assert_eq!(target, Path::new("D:/t/my-flow.jsonc"));
    assert!(stale.is_none());

    // .json5 原件同理
    let (target, stale) = workflow_editor_save_target(Some(Path::new("D:/t/a.json5")), "a.json5");
    assert_eq!(target, Path::new("D:/t/a.json5"));
    assert!(stale.is_none());

    // 大小写不敏感的主干比较
    let (target, stale) =
        workflow_editor_save_target(Some(Path::new("D:/t/My-Flow.JSON5")), "my-flow.json5");
    assert_eq!(target, Path::new("D:/t/My-Flow.JSON5"));
    assert!(stale.is_none());
}

#[test]
fn save_target_renames_and_marks_stale_for_cleanup() {
    // 改名 -> 写同目录新名 + 返回旧路径供删除（重命名语义，不残留旧文件）
    let (target, stale) =
        workflow_editor_save_target(Some(Path::new("D:/t/old-name.json5")), "new-name.json5");
    assert_eq!(target, Path::new("D:/t/new-name.json5"));
    assert_eq!(stale.as_deref(), Some(Path::new("D:/t/old-name.json5")));

    // .jsonc 改名同理
    let (target, stale) =
        workflow_editor_save_target(Some(Path::new("D:/t/old.jsonc")), "new.json5");
    assert_eq!(target, Path::new("D:/t/new.json5"));
    assert_eq!(stale.as_deref(), Some(Path::new("D:/t/old.jsonc")));
}

#[test]
fn save_target_new_mode_writes_relative_name() {
    // 新建/副本（无 editing_path）：返回相对文件名（由调用方拼模板目录），无旧文件
    let (target, stale) = workflow_editor_save_target(None, "brand-new.json5");
    assert_eq!(target, Path::new("brand-new.json5"));
    assert!(stale.is_none());
}

#[test]
fn ai_generated_json5_fills_editor_data() {
    let mut data = WorkflowEditorData::default();
    data.editing_path = Some(PathBuf::from("D:/t/existing.json5"));
    data.file_name = "existing".to_string();
    let json5 = r#"{
        version: 1,
        name: "AI 模板",
        steps: [
            { op: "checkout", branch: "master" },
            { op: "push" },
        ],
    }"#;
    apply_ai_generated_to_editor_data(&mut data, json5).unwrap();
    // 表单内容被替换
    assert_eq!(data.steps.len(), 2);
    assert_eq!(data.name, "AI 模板");
    // 语义字段保留：编辑目标不动、文件名不被 AI 名称覆盖
    assert_eq!(
        data.editing_path,
        Some(PathBuf::from("D:/t/existing.json5"))
    );
    assert_eq!(data.file_name, "existing");
    // 无 inputs/vars：高级区不自动展开
    assert!(!data.advanced_expanded);
}

#[test]
fn ai_generated_strips_code_fence_and_expands_advanced() {
    let mut data = WorkflowEditorData::default();
    let json5 = "```json5\n\
        { version: 1, name: \"带围栏\", inputs: { target: { label: \"目标\" } }, steps: [{ op: \"ensureClean\" }] }\n\
        ```";
    apply_ai_generated_to_editor_data(&mut data, json5).unwrap();
    assert_eq!(data.steps.len(), 1);
    assert_eq!(data.inputs.len(), 1);
    // 有 inputs：高级区自动展开方便检查
    assert!(data.advanced_expanded);
    // file_name 原为空：按 AI 显示名推导主干
    assert_eq!(data.file_name, "带围栏");
}

#[test]
fn ai_generated_invalid_json5_returns_chinese_error() {
    let mut data = WorkflowEditorData::default();
    let err = apply_ai_generated_to_editor_data(&mut data, "这不是 JSON").unwrap_err();
    assert!(err.contains("不是有效的工作流模板"), "实际：{err}");
    assert!(err.contains("重试"), "应含重试引导：{err}");
    // 原数据不被半途污染
    assert!(data.steps.is_empty());
}

#[test]
fn ai_generated_invalid_op_rejected_by_domain_validation() {
    let mut data = WorkflowEditorData::default();
    let json5 = r#"{ version: 1, steps: [{ op: "notARealOp", branch: "x" }] }"#;
    assert!(apply_ai_generated_to_editor_data(&mut data, json5).is_err());
}
