use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use chrono::{DateTime, Datelike, Local, NaiveDate};
use git2::{BranchType, Repository};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{
    BranchInfo, BranchKind, BranchName, CheckoutAttempt, GitError, GitService, RemoteName,
    RepositorySnapshot, Result, WorktreeChange,
};

mod expressions;
pub mod browser_runtime;
pub mod extensions;
mod mcp_process;
pub mod remote_templates;
mod remote_branch_guard;
mod v2;

use expressions::WorkflowExpressionValue;
use expressions::evaluate_workflow_expression;
pub use remote_branch_guard::RemoteBranchGuardAction;
pub use v2::{WorkflowAction, WorkflowActionPreview, WorkflowActionRegistry, WorkflowActionResult};
use v2::{builtin_step, empty_json_object};
use remote_branch_guard::{
    default_guard_fetch, default_on_exists, default_on_missing, guard_remote_branch, guard_summary,
    validate_remote_branch_name,
};

/// 工作流模板定义。同时支持反序列化（加载 .json5 模板）与序列化
/// （可视化模板编辑器保存），序列化省略值为 None 的可选字段保持文件简洁。
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowDefinition {
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default)]
    pub defaults: WorkflowDefaults,
    #[serde(default)]
    pub inputs: BTreeMap<String, WorkflowInputDefinition>,
    #[serde(default)]
    pub vars: BTreeMap<String, String>,
    pub steps: Vec<WorkflowStep>,
}

impl WorkflowDefinition {
    pub fn display_name(&self) -> String {
        self.name
            .clone()
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| "未命名工作流".to_string())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowInputDefinition {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    #[serde(default = "default_workflow_input_required")]
    pub required: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowDefaults {
    #[serde(default = "default_require_clean_worktree")]
    pub require_clean_worktree: bool,
}

impl Default for WorkflowDefaults {
    fn default() -> Self {
        Self {
            require_clean_worktree: true,
        }
    }
}

/// 工作流步骤。`op` 作为 serde tag；各变体的 Option 字段序列化时省略 None，
/// 布尔字段保持与解析侧一致的默认值语义（`default = ...`）。
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum WorkflowStep {
    /// v2 的可扩展动作。内置 git.* 动作仍复用同一套 Git 业务守卫。
    Invoke {
        id: String,
        uses: String,
        #[serde(default = "empty_json_object", rename = "with")]
        arguments: Value,
        #[serde(default, skip_serializing_if = "Option::is_none", rename = "saveAs")]
        save_as: Option<String>,
    },
    Checkout {
        branch: String,
    },
    Fetch {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        remote: Option<String>,
    },
    Pull {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        remote: Option<String>,
    },
    CreateBranch {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from: Option<String>,
        #[serde(default = "default_create_branch_checkout")]
        checkout: bool,
    },
    Merge {
        branch: String,
    },
    Push {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        remote: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        branch: Option<String>,
        #[serde(default = "default_set_upstream")]
        set_upstream: bool,
    },
    GuardRemoteBranch {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        remote: Option<String>,
        branch: String,
        #[serde(default = "default_guard_fetch")]
        fetch: bool,
        #[serde(default = "default_on_exists", rename = "onExists")]
        on_exists: RemoteBranchGuardAction,
        #[serde(default = "default_on_missing", rename = "onMissing")]
        on_missing: RemoteBranchGuardAction,
    },
    EnsureClean,
    AssertBranch {
        branch: String,
    },
    /// 筛选符合条件的本地分支，把结果作为数组变量写入步骤输出。
    /// 只读，不改动仓库；预览阶段即可看到命中清单。
    FilterBranches {
        /// 输出变量名，筛选结果数组写入此处（供后续步骤通过 `${...}` 消费）。
        output: String,
        /// 正则表达式，需包含一个日期捕获组（命名组优先，否则回退到组 1）。
        pattern: String,
        /// chrono 日期格式，用于解析捕获组里的日期，默认 `%Y%m%d`。
        #[serde(default = "default_filter_date_format", rename = "dateFormat")]
        date_format: String,
        /// 日期命名捕获组名，默认 `date`。
        #[serde(default = "default_filter_date_group", rename = "dateGroup")]
        date_group: String,
        /// 距今月数阈值，插值后解析为整数；分支日期距今月数大于该值才入选。
        #[serde(rename = "olderThanMonths")]
        older_than_months: String,
        /// 是否把当前分支排除在结果之外，默认 true。
        #[serde(default = "default_filter_skip_current", rename = "skipCurrent")]
        skip_current: bool,
    },
    /// 删除一批本地分支（仅本地，不涉及远端）。
    /// 通常配合 `filterBranches` 使用：把上一步的数组输出传给 `branches`。
    DeleteBranches {
        /// 分支列表，可以是数组变量（如 `${out.stale}`），也可以是换行分隔的字符串。
        branches: String,
        /// 试运行模式：只列出将要删除的分支，不真正删除，默认 true。
        #[serde(default = "default_delete_dry_run", rename = "dryRun")]
        dry_run: bool,
        /// 是否自动跳过当前分支（即使它出现在列表里也不删除），默认 true。
        #[serde(default = "default_delete_skip_current", rename = "skipCurrent")]
        skip_current: bool,
    },
}

#[derive(Clone, Debug)]
pub struct WorkflowRunOptions {
    pub default_remote: String,
    pub input_vars: BTreeMap<String, String>,
    /// 从分支菜单进入时固定的来源引用；普通入口仍以启动时 HEAD 为来源。
    pub source_branch: Option<WorkflowSourceBranch>,
}

#[derive(Clone, Debug)]
pub struct WorkflowSourceBranch {
    pub name: String,
    pub kind: BranchKind,
}

impl Default for WorkflowRunOptions {
    fn default() -> Self {
        Self {
            default_remote: "origin".to_string(),
            input_vars: BTreeMap::new(),
            source_branch: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct WorkflowRunResult {
    pub name: String,
    pub steps_run: usize,
    pub snapshot: RepositorySnapshot,
}

#[derive(Clone, Debug)]
pub struct WorkflowPreview {
    pub name: String,
    pub steps: Vec<WorkflowPreviewStep>,
}

#[derive(Clone, Debug)]
pub struct WorkflowPreviewStep {
    pub index: usize,
    pub op: &'static str,
    pub summary: String,
    /// 步骤明细行，用于在预览/日志中展示更丰富的逐条信息（如筛选命中的分支清单）。
    pub details: Vec<String>,
}

#[derive(Clone, Debug)]
pub enum WorkflowProgressEvent {
    Started {
        name: String,
        total: usize,
    },
    StepStarted {
        index: usize,
        total: usize,
        label: String,
        details: Vec<String>,
    },
    StepFinished {
        index: usize,
        total: usize,
        label: String,
        details: Vec<String>,
    },
    StepDetail {
        index: usize,
        total: usize,
        label: String,
        detail: String,
    },
    Finished {
        name: String,
        total: usize,
    },
}

pub struct WorkflowExecutor<'a> {
    service: &'a GitService,
    actions: Option<&'a WorkflowActionRegistry>,
}

/// 每次运行独立的取消令牌。取消在步骤边界生效；外部动作可在执行期间主动检查。
#[derive(Clone, Debug)]
pub struct WorkflowRunControl {
    id: u64,
    cancelled: Arc<AtomicBool>,
}

impl Default for WorkflowRunControl {
    fn default() -> Self {
        Self::new()
    }
}

impl WorkflowRunControl {
    pub fn new() -> Self {
        static NEXT_RUN_ID: AtomicU64 = AtomicU64::new(1);
        Self {
            id: NEXT_RUN_ID.fetch_add(1, Ordering::Relaxed),
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    pub fn check_cancelled(&self) -> Result<()> {
        if self.is_cancelled() {
            Err(GitError::WorkflowCancelled)
        } else {
            Ok(())
        }
    }
}

impl<'a> WorkflowExecutor<'a> {
    pub fn new(service: &'a GitService) -> Self {
        Self {
            service,
            actions: None,
        }
    }

    pub fn with_actions(service: &'a GitService, actions: &'a WorkflowActionRegistry) -> Self {
        Self {
            service,
            actions: Some(actions),
        }
    }

    pub fn preview(
        &self,
        repo: &Repository,
        definition: &WorkflowDefinition,
        options: &WorkflowRunOptions,
    ) -> Result<WorkflowPreview> {
        validate_definition(definition)?;
        self.preflight_actions(definition)?;
        validate_input_values(definition, options)?;
        let context = WorkflowEvalContext::new(self.service, repo, options)?;
        let mut resolver = WorkflowResolver::new(self.service, repo, definition, options, &context);
        let mut preview_state = WorkflowPreviewState::default();
        let steps = definition
            .steps
            .iter()
            .enumerate()
            .map(|(index, step)| {
                let resolved_step = step.resolve(&mut resolver)?;
                let mut summary = resolved_step.summary();
                // 对只读步骤（如 FilterBranches）在预览阶段即可计算明细并写入步骤输出，
                // 让后续步骤在预览时也能引用筛选结果；明细与输出一次计算共用。
                let details = if let ResolvedWorkflowStep::Invoke {
                    uses,
                    arguments,
                    save_as,
                } = &resolved_step
                {
                    let action = self
                        .actions
                        .ok_or_else(|| GitError::Message(format!("未注册工作流动作：{uses}")))?
                        .get(uses)?;
                    let preview = action.preview(self.service, repo, arguments)?;
                    summary = preview.summary;
                    if let Some(name) = save_as {
                        if let Some(value) = preview.output {
                            context.record_output(name.clone(), WorkflowExpressionValue::from_json(value));
                        } else {
                            context.defer_output(name.clone());
                        }
                    }
                    preview.details
                } else {
                    record_preview_output(&resolved_step, self.service, repo, &context)?
                };
                preview_state.apply(&resolved_step);
                resolver.set_preview_current_branch(preview_state.current_branch.clone());
                Ok(WorkflowPreviewStep {
                    index,
                    op: step.op_name(),
                    summary,
                    details,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(WorkflowPreview {
            name: definition.display_name(),
            steps,
        })
    }

    pub fn resolve_template(
        &self,
        repo: &Repository,
        definition: &WorkflowDefinition,
        options: &WorkflowRunOptions,
        template: &str,
    ) -> Result<String> {
        validate_definition(definition)?;
        let context = WorkflowEvalContext::new(self.service, repo, options)?;
        let mut resolver = WorkflowResolver::new(self.service, repo, definition, options, &context);
        resolver.interpolate(template)
    }

    pub fn run<F>(
        &self,
        repo: &mut Repository,
        definition: &WorkflowDefinition,
        options: WorkflowRunOptions,
        progress: F,
    ) -> Result<WorkflowRunResult>
    where
        F: FnMut(WorkflowProgressEvent),
    {
        self.run_with_control(repo, definition, options, &WorkflowRunControl::new(), progress)
    }

    pub fn run_with_control<F>(
        &self,
        repo: &mut Repository,
        definition: &WorkflowDefinition,
        options: WorkflowRunOptions,
        control: &WorkflowRunControl,
        mut progress: F,
    ) -> Result<WorkflowRunResult>
    where
        F: FnMut(WorkflowProgressEvent),
    {
        control.check_cancelled()?;
        validate_definition(definition)?;
        self.preflight_actions(definition)?;
        validate_input_values(definition, &options)?;
        let context = WorkflowEvalContext::new(self.service, repo, &options)?;
        if definition.defaults.require_clean_worktree {
            ensure_clean_worktree(self.service, repo)?;
        }
        control.check_cancelled()?;

        let name = definition.display_name();
        let total = definition.steps.len();
        progress(WorkflowProgressEvent::Started {
            name: name.clone(),
            total,
        });

        let mut steps_run = 0;
        let mut last_snapshot = None;

        for (index, step) in definition.steps.iter().enumerate() {
            control.check_cancelled()?;
            let resolved_step = {
                let mut resolver =
                    WorkflowResolver::new(self.service, repo, definition, &options, &context);
                step.resolve(&mut resolver)?
            };
            control.check_cancelled()?;
            let label = resolved_step.summary();
            progress(WorkflowProgressEvent::StepStarted {
                index,
                total,
                label: label.clone(),
                details: Vec::new(),
            });

            let outcome = resolved_step.execute(self.service, repo, self.actions, control,
                &mut |detail| progress(WorkflowProgressEvent::StepDetail {
                    index, total, label: label.clone(), detail,
                }))?;
            // 把步骤输出（如 FilterBranches 的命中分支）写入 context，供后续步骤消费。
            if let Some((name, value)) = outcome.output {
                context.record_output(name, value);
            }
            last_snapshot = Some(outcome.snapshot);
            steps_run += 1;

            progress(WorkflowProgressEvent::StepFinished {
                index,
                total,
                label,
                details: outcome.details,
            });
        }

        let snapshot = match last_snapshot {
            Some(snapshot) => snapshot,
            None => self.service.snapshot_after_operation(repo)?,
        };
        progress(WorkflowProgressEvent::Finished {
            name: name.clone(),
            total,
        });
        Ok(WorkflowRunResult {
            name,
            steps_run,
            snapshot,
        })
    }

    fn preflight_actions(&self, definition: &WorkflowDefinition) -> Result<()> {
        for step in &definition.steps {
            if let WorkflowStep::Invoke { uses, arguments, save_as, .. } = step {
                if builtin_step(uses, arguments)?.is_some() {
                    if save_as.is_some() {
                        return Err(GitError::Message(format!(
                            "内置动作 {uses} 不支持 saveAs，请使用动作自身的输出参数"
                        )));
                    }
                } else {
                    self.actions
                        .ok_or_else(|| GitError::Message(format!("未注册工作流动作：{uses}")))?
                        .get(uses)?;
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
enum ResolvedWorkflowStep {
    Invoke {
        uses: String,
        arguments: Value,
        save_as: Option<String>,
    },
    Checkout {
        branch: String,
    },
    Fetch {
        remote: String,
    },
    Pull {
        remote: String,
    },
    CreateBranch {
        name: String,
        from: Option<String>,
        checkout: bool,
    },
    Merge {
        branch: String,
    },
    Push {
        remote: String,
        branch: String,
        set_upstream: bool,
    },
    GuardRemoteBranch {
        remote: String,
        branch: String,
        fetch: bool,
        on_exists: RemoteBranchGuardAction,
        on_missing: RemoteBranchGuardAction,
    },
    EnsureClean,
    AssertBranch {
        branch: String,
    },
    /// 已解析的分支筛选步骤。pattern 已编译为正则，older_than_months 已转为整数。
    FilterBranches {
        output: String,
        pattern: Regex,
        date_format: String,
        date_group: String,
        older_than_months: i64,
        skip_current: bool,
    },
    /// 已解析的删除分支步骤。branches 为展开后的分支名列表。
    DeleteBranches {
        branches: Vec<String>,
        dry_run: bool,
        skip_current: bool,
    },
}

#[derive(Default)]
struct WorkflowPreviewState {
    current_branch: Option<String>,
}

impl WorkflowPreviewState {
    fn apply(&mut self, step: &ResolvedWorkflowStep) {
        match step {
            ResolvedWorkflowStep::Checkout { branch } => {
                self.current_branch = Some(branch.clone());
            }
            ResolvedWorkflowStep::CreateBranch { name, checkout, .. } if *checkout => {
                self.current_branch = Some(name.clone());
            }
            _ => {}
        }
    }
}

/// 单个步骤执行后的产出：刷新快照、明细行（用于日志），以及可选的步骤输出变量。
struct StepOutcome {
    snapshot: RepositorySnapshot,
    details: Vec<String>,
    /// 步骤输出可保存数组或 JSON 值，供后续步骤消费。
    output: Option<(String, WorkflowExpressionValue)>,
}

impl StepOutcome {
    fn snapshot(snapshot: RepositorySnapshot) -> Self {
        Self {
            snapshot,
            details: Vec::new(),
            output: None,
        }
    }

    fn snapshot_with_details(snapshot: RepositorySnapshot, details: Vec<String>) -> Self {
        Self {
            snapshot,
            details,
            output: None,
        }
    }
}

impl WorkflowStep {
    pub fn op_name(&self) -> &'static str {
        match self {
            WorkflowStep::Invoke { .. } => "invoke",
            WorkflowStep::Checkout { .. } => "checkout",
            WorkflowStep::Fetch { .. } => "fetch",
            WorkflowStep::Pull { .. } => "pull",
            WorkflowStep::CreateBranch { .. } => "createBranch",
            WorkflowStep::Merge { .. } => "merge",
            WorkflowStep::Push { .. } => "push",
            WorkflowStep::GuardRemoteBranch { .. } => "guardRemoteBranch",
            WorkflowStep::EnsureClean => "ensureClean",
            WorkflowStep::AssertBranch { .. } => "assertBranch",
            WorkflowStep::FilterBranches { .. } => "filterBranches",
            WorkflowStep::DeleteBranches { .. } => "deleteBranches",
        }
    }

    fn resolve(&self, resolver: &mut WorkflowResolver<'_, '_>) -> Result<ResolvedWorkflowStep> {
        match self {
            WorkflowStep::Invoke { uses, arguments, save_as, .. } => {
                // JS 源码中的 `${...}` 属于 JavaScript 模板字符串，不按工作流表达式展开。
                let mut arguments = arguments.clone();
                let script = if uses == "js.run" {
                    arguments.as_object_mut().and_then(|object| object.remove("script"))
                } else {
                    None
                };
                let mut arguments = resolver.interpolate_json(&arguments)?;
                if let Some(script) = script {
                    arguments.as_object_mut().expect("invoke 参数已验证为对象")
                        .insert("script".into(), script);
                }
                if let Some(step) = builtin_step(uses, &arguments)? {
                    return step.resolve(resolver);
                }
                Ok(ResolvedWorkflowStep::Invoke {
                    uses: uses.clone(),
                    arguments,
                    save_as: save_as.clone(),
                })
            }
            WorkflowStep::Checkout { branch } => Ok(ResolvedWorkflowStep::Checkout {
                branch: resolver.interpolate(branch)?,
            }),
            WorkflowStep::Fetch { remote } => Ok(ResolvedWorkflowStep::Fetch {
                remote: resolver.remote_name(remote)?,
            }),
            WorkflowStep::Pull { remote } => Ok(ResolvedWorkflowStep::Pull {
                remote: resolver.remote_name(remote)?,
            }),
            WorkflowStep::CreateBranch {
                name,
                from,
                checkout,
            } => {
                let from = from
                    .as_ref()
                    .map(|from| resolver.interpolate(from))
                    .transpose()?;
                Ok(ResolvedWorkflowStep::CreateBranch {
                    name: resolver.interpolate(name)?,
                    from,
                    checkout: *checkout,
                })
            }
            WorkflowStep::Merge { branch } => Ok(ResolvedWorkflowStep::Merge {
                branch: resolver.interpolate(branch)?,
            }),
            WorkflowStep::Push {
                remote,
                branch,
                set_upstream,
            } => Ok(ResolvedWorkflowStep::Push {
                remote: resolver.remote_name(remote)?,
                branch: resolver.branch_or_current(branch)?,
                set_upstream: *set_upstream,
            }),
            WorkflowStep::GuardRemoteBranch {
                remote,
                branch,
                fetch,
                on_exists,
                on_missing,
            } => {
                let remote = resolver.remote_name(remote)?;
                let branch = resolver.interpolate(branch)?;
                validate_remote_branch_name(&remote, &branch)?;
                Ok(ResolvedWorkflowStep::GuardRemoteBranch {
                    remote,
                    branch,
                    fetch: *fetch,
                    on_exists: *on_exists,
                    on_missing: *on_missing,
                })
            }
            WorkflowStep::EnsureClean => Ok(ResolvedWorkflowStep::EnsureClean),
            WorkflowStep::AssertBranch { branch } => Ok(ResolvedWorkflowStep::AssertBranch {
                branch: resolver.interpolate(branch)?,
            }),
            WorkflowStep::FilterBranches {
                output,
                pattern,
                date_format,
                date_group,
                older_than_months,
                skip_current,
            } => {
                let output = resolver.interpolate(output)?;
                let pattern_str = resolver.interpolate(pattern)?;
                let pattern = Regex::new(&pattern_str).map_err(|err| {
                    GitError::Message(format!("filterBranches 的正则表达式无效：{err}"))
                })?;
                // 校验正则至少存在一个捕获组：命名组优先，否则回退到组 1。
                let has_named = pattern
                    .capture_names()
                    .any(|name| name == Some(&**date_group));
                let has_group1 = pattern.captures_len() >= 2; // 组 0 是整体匹配，组 1 是第一个捕获组
                if pattern.capture_names().count() == 1 || (!has_named && !has_group1) {
                    return Err(GitError::Message(format!(
                        "filterBranches 的正则表达式必须包含至少一个捕获组：{pattern_str}"
                    )));
                }
                let older_than_months_str = resolver.interpolate(older_than_months)?;
                let older_than_months =
                    older_than_months_str.trim().parse::<i64>().map_err(|_| {
                        GitError::Message(format!(
                            "filterBranches 的 olderThanMonths 必须是整数：{older_than_months_str}"
                        ))
                    })?;
                if older_than_months < 0 {
                    return Err(GitError::Message(format!(
                        "filterBranches 的 olderThanMonths 不能为负数：{older_than_months}"
                    )));
                }
                Ok(ResolvedWorkflowStep::FilterBranches {
                    output,
                    pattern,
                    date_format: date_format.clone(),
                    date_group: date_group.clone(),
                    older_than_months,
                    skip_current: *skip_current,
                })
            }
            WorkflowStep::DeleteBranches {
                branches,
                dry_run,
                skip_current,
            } => {
                let branches = resolver.interpolate_array(branches)?;
                Ok(ResolvedWorkflowStep::DeleteBranches {
                    branches,
                    dry_run: *dry_run,
                    skip_current: *skip_current,
                })
            }
        }
    }
}

impl ResolvedWorkflowStep {
    fn summary(&self) -> String {
        match self {
            ResolvedWorkflowStep::Invoke { uses, .. } => format!("调用工作流动作 {uses}"),
            ResolvedWorkflowStep::Checkout { branch } => format!("切换到分支 {branch}"),
            ResolvedWorkflowStep::Fetch { remote } => format!("获取远端 {remote}"),
            ResolvedWorkflowStep::Pull { remote } => format!("拉取远端 {remote}"),
            ResolvedWorkflowStep::CreateBranch {
                name,
                from,
                checkout,
            } => {
                let from = from.clone().unwrap_or_else(|| "当前 HEAD".to_string());
                let suffix = if *checkout { "并切换" } else { "" };
                format!("基于 {from} 创建分支 {name}{suffix}")
            }
            ResolvedWorkflowStep::Merge { branch } => format!("合并分支 {branch}"),
            ResolvedWorkflowStep::Push { remote, branch, .. } => {
                format!("推送分支 {branch} 到 {remote}")
            }
            ResolvedWorkflowStep::GuardRemoteBranch {
                remote,
                branch,
                fetch,
                on_exists,
                on_missing,
            } => guard_summary(remote, branch, *fetch, *on_exists, *on_missing),
            ResolvedWorkflowStep::EnsureClean => "检查工作区干净".to_string(),
            ResolvedWorkflowStep::AssertBranch { branch } => {
                format!("确认当前分支是 {branch}")
            }
            ResolvedWorkflowStep::FilterBranches {
                output,
                older_than_months,
                ..
            } => {
                let _ = output;
                format!("筛选超过 {older_than_months} 个月的命名分支")
            }
            ResolvedWorkflowStep::DeleteBranches { dry_run, .. } => {
                if *dry_run {
                    "删除分支（试运行）".to_string()
                } else {
                    "删除本地分支".to_string()
                }
            }
        }
    }

    fn execute(
        &self,
        service: &GitService,
        repo: &mut Repository,
        actions: Option<&WorkflowActionRegistry>,
        control: &WorkflowRunControl,
        progress: &mut dyn FnMut(String),
    ) -> Result<StepOutcome> {
        match self {
            ResolvedWorkflowStep::Invoke { uses, arguments, save_as } => {
                let action = actions
                    .ok_or_else(|| GitError::Message(format!("未注册工作流动作：{uses}")))?
                    .get(uses)?;
                let result = action.execute_with_control_and_progress(
                    service, repo, arguments, control, progress)?;
                let output = match (save_as, result.output) {
                    (Some(name), Some(value)) => {
                        Some((name.clone(), WorkflowExpressionValue::from_json(value)))
                    }
                    _ => None,
                };
                Ok(StepOutcome {
                    snapshot: service.snapshot_after_operation(repo)?,
                    details: result.details,
                    output,
                })
            }
            ResolvedWorkflowStep::Checkout { branch } => {
                let branch = BranchName::new(branch.clone());
                // 工作流无人值守，不能弹窗确认：被未提交修改阻止时直接按
                // 「贮藏后重试」执行，恢复冲突的说明写进步骤明细。
                match service.checkout_branch_checked(repo, &branch)? {
                    CheckoutAttempt::Switched(snapshot) => Ok(StepOutcome::snapshot(snapshot)),
                    CheckoutAttempt::BlockedByLocalChanges => {
                        // 工作流无人值守，没有勾选开关：只贮藏并切换，不自动恢复。
                        let outcome = service
                            .checkout_branch_carrying_changes(repo, &branch, false)?;
                        let mut details = Vec::new();
                        if let Some(notice) = outcome.notice {
                            details.push(notice);
                        }
                        Ok(StepOutcome::snapshot_with_details(
                            outcome.snapshot,
                            details,
                        ))
                    }
                }
            }
            ResolvedWorkflowStep::Fetch { remote } => Ok(StepOutcome::snapshot(
                service.fetch(repo, &RemoteName::new(remote.clone()))?,
            )),
            ResolvedWorkflowStep::Pull { remote } => Ok(StepOutcome::snapshot(
                service.pull(repo, &RemoteName::new(remote.clone()))?,
            )),
            ResolvedWorkflowStep::CreateBranch {
                name,
                from,
                checkout,
            } => Ok(StepOutcome::snapshot(
                service.create_branch_from(
                    repo,
                    &BranchName::new(name.clone()),
                    from.as_ref()
                        .map(|from| BranchName::new(from.clone()))
                        .as_ref(),
                    *checkout,
                )?,
            )),
            ResolvedWorkflowStep::Merge { branch } => Ok(StepOutcome::snapshot(
                service.merge_branch(repo, &BranchName::new(branch.clone()))?,
            )),
            ResolvedWorkflowStep::Push {
                remote,
                branch,
                set_upstream,
            } => Ok(StepOutcome::snapshot(service.push_branch(
                repo,
                &RemoteName::new(remote.clone()),
                &BranchName::new(branch.clone()),
                *set_upstream,
            )?)),
            ResolvedWorkflowStep::GuardRemoteBranch {
                remote,
                branch,
                fetch,
                on_exists,
                on_missing,
            } => {
                if *fetch {
                    service.fetch(repo, &RemoteName::new(remote.clone()))?;
                }
                guard_remote_branch(repo, remote, branch, *on_exists, *on_missing)?;
                Ok(StepOutcome::snapshot(
                    service.snapshot_after_operation(repo)?,
                ))
            }
            ResolvedWorkflowStep::EnsureClean => {
                ensure_clean_worktree(service, repo)?;
                Ok(StepOutcome::snapshot(
                    service.snapshot_after_operation(repo)?,
                ))
            }
            ResolvedWorkflowStep::AssertBranch { branch } => {
                let actual = service.current_branch(repo).ok_or_else(|| {
                    GitError::Message("当前 HEAD 未指向本地分支，无法确认分支".into())
                })?;
                if actual != *branch {
                    return Err(GitError::Message(format!(
                        "当前分支是 {actual}，不是工作流要求的 {branch}"
                    )));
                }
                Ok(StepOutcome::snapshot(
                    service.snapshot_after_operation(repo)?,
                ))
            }
            ResolvedWorkflowStep::FilterBranches {
                output,
                pattern,
                date_format,
                date_group,
                older_than_months,
                skip_current,
            } => {
                let plan = select_branches_for_deletion(
                    service,
                    repo,
                    pattern,
                    date_format,
                    date_group,
                    *older_than_months,
                    *skip_current,
                )?;
                let details = deletion_plan_details(&plan, "命中");
                Ok(StepOutcome {
                    snapshot: service.snapshot_after_operation(repo)?,
                    details,
                    // 把命中分支作为数组变量输出，供后续步骤通过 ${output} 消费。
                    output: Some((output.clone(), WorkflowExpressionValue::Array(plan.matched))),
                })
            }
            ResolvedWorkflowStep::DeleteBranches {
                branches,
                dry_run,
                skip_current,
            } => {
                let current = service.current_branch(repo);
                let mut to_delete = Vec::new();
                let mut skipped_current = Vec::new();
                for name in branches {
                    if *skip_current && current.as_deref() == Some(name.as_str()) {
                        skipped_current.push(name.clone());
                    } else {
                        to_delete.push(name.clone());
                    }
                }
                let mut details = Vec::new();
                if *dry_run {
                    for name in &to_delete {
                        details.push(format!("将删除：{name}"));
                    }
                    for name in &skipped_current {
                        details.push(format!("跳过（当前分支）：{name}"));
                    }
                    if details.is_empty() {
                        details.push("没有需要删除的分支".to_string());
                    }
                    return Ok(StepOutcome::snapshot_with_details(
                        service.snapshot_after_operation(repo)?,
                        details,
                    ));
                }
                // 正式删除：批量执行，仅刷新一次快照。
                service.delete_local_branches(
                    repo,
                    &to_delete
                        .iter()
                        .map(|n| BranchName::new(n.clone()))
                        .collect::<Vec<_>>(),
                )?;
                for name in &to_delete {
                    details.push(format!("已删除：{name}"));
                }
                for name in &skipped_current {
                    details.push(format!("跳过（当前分支）：{name}"));
                }
                if details.is_empty() {
                    details.push("没有需要删除的分支".to_string());
                }
                Ok(StepOutcome::snapshot_with_details(
                    service.snapshot_after_operation(repo)?,
                    details,
                ))
            }
        }
    }
}

pub fn parse_workflow_json5(content: &str) -> Result<WorkflowDefinition> {
    let definition = json5::from_str::<WorkflowDefinition>(content)
        .map_err(|err| GitError::Message(format!("工作流 JSON5 解析失败：{err}")))?;
    validate_definition(&definition)?;
    Ok(definition)
}

fn validate_definition(definition: &WorkflowDefinition) -> Result<()> {
    if definition.version != 1 && definition.version != 2 {
        return Err(GitError::Message(format!(
            "不支持的工作流版本：{}",
            definition.version
        )));
    }
    if definition.steps.is_empty() {
        return Err(GitError::Message("工作流至少需要一个步骤".into()));
    }
    for key in definition.inputs.keys() {
        validate_input_name(key)?;
    }
    let mut ids = BTreeSet::new();
    for step in &definition.steps {
        if let WorkflowStep::Invoke { id, uses, arguments, save_as } = step {
            if definition.version == 1 {
                return Err(GitError::Message("v1 工作流不能使用 invoke 步骤".into()));
            }
            if id.trim().is_empty() || !ids.insert(id) {
                return Err(GitError::Message(format!("工作流步骤 id 为空或重复：{id}")));
            }
            if uses.trim().is_empty() || !arguments.is_object() {
                return Err(GitError::Message(format!("工作流步骤 {id} 的 uses 或 with 无效")));
            }
            if let Some(name) = save_as {
                validate_input_name(name)?;
            }
        }
    }
    Ok(())
}

fn validate_input_name(name: &str) -> Result<()> {
    let name = name.trim();
    if name.is_empty() {
        return Err(GitError::Message("工作流输入变量名不能为空".into()));
    }
    if name == "run.id"
        || name == "run.sourceBranch"
        || name == "run.sourceKind"
        || name == "git.initialBranch"
        || name == "git.currentBranch"
        || name == "git.head"
        || name == "git.repoName"
        || name.starts_with("date:")
        || name.starts_with("run.startedAt:")
        || name.starts_with("git.")
        || name.starts_with("run.")
    {
        return Err(GitError::Message(format!(
            "工作流输入变量不能使用内置变量名：{name}"
        )));
    }
    Ok(())
}

fn validate_input_values(
    definition: &WorkflowDefinition,
    options: &WorkflowRunOptions,
) -> Result<()> {
    for (name, input) in &definition.inputs {
        if input.required
            && options
                .input_vars
                .get(name)
                .is_none_or(|value| value.trim().is_empty())
        {
            let label = input
                .label
                .as_deref()
                .filter(|label| !label.trim().is_empty())
                .unwrap_or(name);
            return Err(GitError::Message(format!("请填写工作流变量：{label}")));
        }
    }
    Ok(())
}

fn ensure_clean_worktree(service: &GitService, repo: &Repository) -> Result<()> {
    let changes = service.status_full(repo)?;
    if changes.is_empty() {
        return Ok(());
    }
    Err(GitError::Message(format!(
        "工作区存在未提交更改，不能运行该工作流：{}",
        changes_preview(&changes)
    )))
}

fn changes_preview(changes: &[WorktreeChange]) -> String {
    let mut preview = changes
        .iter()
        .take(5)
        .map(|change| change.path.clone())
        .collect::<Vec<_>>()
        .join(", ");
    if changes.len() > 5 {
        preview.push_str(&format!(" 等 {} 个文件", changes.len()));
    }
    preview
}

struct WorkflowEvalContext {
    started_at: DateTime<Local>,
    run_id: String,
    initial_branch: Option<String>,
    source_kind: BranchKind,
    repo_name: String,
    /// 步骤间传递的输出变量：变量名 → 值。
    /// 用 RefCell 是因为 resolver 以 `&context` 共享借用 context，需内部可变性。
    step_outputs: RefCell<BTreeMap<String, WorkflowExpressionValue>>,
    deferred_outputs: RefCell<BTreeSet<String>>,
}

impl WorkflowEvalContext {
    fn new(service: &GitService, repo: &Repository, options: &WorkflowRunOptions) -> Result<Self> {
        let started_at = Local::now();
        let initial_branch = match &options.source_branch {
            Some(source) => {
                let kind = match &source.kind {
                    BranchKind::Local => BranchType::Local,
                    BranchKind::Remote => BranchType::Remote,
                };
                repo.find_branch(&source.name, kind).map_err(|_| {
                    GitError::Message(format!("工作流来源分支已不存在：{}", source.name))
                })?;
                Some(source.name.clone())
            }
            None => service.current_branch(repo),
        };
        Ok(Self {
            run_id: format!("{}", started_at.timestamp_millis()),
            initial_branch,
            source_kind: options.source_branch.as_ref().map_or(BranchKind::Local, |source| source.kind.clone()),
            repo_name: repo_display_name(repo),
            started_at,
            step_outputs: RefCell::new(BTreeMap::new()),
            deferred_outputs: RefCell::new(BTreeSet::new()),
        })
    }

    /// 把一个步骤输出写回 context（供后续步骤通过 ${output} 消费）。
    fn record_output(&self, name: String, value: WorkflowExpressionValue) {
        self.step_outputs.borrow_mut().insert(name, value);
    }

    fn defer_output(&self, name: String) {
        self.deferred_outputs.borrow_mut().insert(name);
    }
}

struct WorkflowResolver<'a, 'repo> {
    service: &'a GitService,
    repo: &'repo Repository,
    definition: &'a WorkflowDefinition,
    options: &'a WorkflowRunOptions,
    context: &'a WorkflowEvalContext,
    preview_current_branch: Option<String>,
}

impl<'a, 'repo> WorkflowResolver<'a, 'repo> {
    fn new(
        service: &'a GitService,
        repo: &'repo Repository,
        definition: &'a WorkflowDefinition,
        options: &'a WorkflowRunOptions,
        context: &'a WorkflowEvalContext,
    ) -> Self {
        Self {
            service,
            repo,
            definition,
            options,
            context,
            preview_current_branch: None,
        }
    }

    fn set_preview_current_branch(&mut self, branch: Option<String>) {
        self.preview_current_branch = branch;
    }

    fn current_branch(&self) -> Result<String> {
        if let Some(branch) = self.preview_current_branch.as_ref() {
            return Ok(branch.clone());
        }
        self.service.current_branch(self.repo).ok_or_else(|| {
            GitError::Message("当前 HEAD 未指向本地分支，无法解析 git.currentBranch".into())
        })
    }

    fn head_oid(&self) -> Result<String> {
        let head = self.repo.head()?;
        let target = head
            .target()
            .ok_or_else(|| GitError::Message("当前 HEAD 没有目标提交".into()))?;
        Ok(target.to_string())
    }

    fn remote_name(&mut self, remote: &Option<String>) -> Result<String> {
        match remote {
            Some(remote) => self.interpolate(remote),
            None => Ok(self.options.default_remote.clone()),
        }
    }

    fn branch_or_current(&mut self, branch: &Option<String>) -> Result<String> {
        match branch {
            Some(branch) => self.interpolate(branch),
            None => self.current_branch(),
        }
    }

    fn interpolate(&mut self, template: &str) -> Result<String> {
        self.interpolate_with_stack(template, &mut BTreeSet::new())
    }

    fn interpolate_with_stack(
        &mut self,
        template: &str,
        stack: &mut BTreeSet<String>,
    ) -> Result<String> {
        let mut output = String::new();
        let mut rest = template;

        while let Some(start) = rest.find("${") {
            output.push_str(&rest[..start]);
            let expression_start = start + 2;
            let Some(end) = rest[expression_start..].find('}') else {
                return Err(GitError::Message(format!(
                    "变量表达式缺少结束符：{template}"
                )));
            };
            let expression_end = expression_start + end;
            let expression = rest[expression_start..expression_end].trim();
            // 单个 ${expr} 占位整段时，表达式可能求值为数组（如步骤输出），此时仍要求转为字符串。
            let value = self.resolve_expression(expression, stack)?;
            output.push_str(&value.into_string(expression)?);
            rest = &rest[expression_end + 1..];
        }

        output.push_str(rest);
        Ok(output)
    }

    /// 把模板求值为一个表达式值，用于需要拿到数组语义的场合（如 deleteBranches 的 branches）。
    /// 支持两种形式：整段是 `${expr}` 占位时提取内部表达式求值；否则按字符串处理。
    fn interpolate_value(&mut self, template: &str) -> Result<WorkflowExpressionValue> {
        let trimmed = template.trim();
        // 整段就是单个 ${...} 占位时，提取内部表达式直接求值，以保留数组语义。
        if let Some(inner) = trimmed
            .strip_prefix("${")
            .and_then(|rest| rest.strip_suffix('}'))
        {
            return self.resolve_expression(inner.trim(), &mut BTreeSet::new());
        }
        // 否则按普通模板插值（结果必为字符串）
        self.interpolate_with_stack(template, &mut BTreeSet::new())
            .map(WorkflowExpressionValue::String)
    }

    fn interpolate_json(&mut self, value: &Value) -> Result<Value> {
        match value {
            Value::String(text) => match self.interpolate_value(text)? {
                WorkflowExpressionValue::String(value) => Ok(Value::String(value)),
                WorkflowExpressionValue::Array(items) => Ok(Value::Array(
                    items.into_iter().map(Value::String).collect(),
                )),
                WorkflowExpressionValue::Json(value) => Ok(value),
            },
            Value::Array(items) => items
                .iter()
                .map(|item| self.interpolate_json(item))
                .collect::<Result<Vec<_>>>()
                .map(Value::Array),
            Value::Object(fields) => fields
                .iter()
                .map(|(key, value)| Ok((key.clone(), self.interpolate_json(value)?)))
                .collect::<Result<Map<String, Value>>>()
                .map(Value::Object),
            value => Ok(value.clone()),
        }
    }

    /// 把模板求值为字符串数组：数组值原样返回，字符串值按换行切分并清理空项。
    fn interpolate_array(&mut self, template: &str) -> Result<Vec<String>> {
        let value = self.interpolate_value(template)?;
        match value {
            WorkflowExpressionValue::Array(items) => Ok(items),
            WorkflowExpressionValue::String(text) => Ok(text
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_string)
                .collect()),
            WorkflowExpressionValue::Json(_) => Err(GitError::Message(
                "工作流参数需要字符串或字符串数组".into(),
            )),
        }
    }

    fn resolve_expression(
        &mut self,
        expression: &str,
        stack: &mut BTreeSet<String>,
    ) -> Result<WorkflowExpressionValue> {
        // 预览无法执行 MCP/JS 来取得 saveAs 值。保留依赖该输出的表达式，
        // 让后续步骤仍可展示预览；实际运行时会正常解析或明确报错。
        let primary = expression.split('|').next().unwrap_or(expression).trim();
        let root = primary.split('.').next().unwrap_or(primary);
        if self.context.deferred_outputs.borrow().contains(root) {
            return Ok(WorkflowExpressionValue::String(format!("${{{expression}}}")));
        }
        evaluate_workflow_expression(expression, |primary| {
            self.resolve_primary_expression(primary, stack)
        })
    }

    fn resolve_primary_expression(
        &mut self,
        expression: &str,
        stack: &mut BTreeSet<String>,
    ) -> Result<WorkflowExpressionValue> {
        if let Some(format) = expression.strip_prefix("date:") {
            return Ok(WorkflowExpressionValue::String(
                self.context.started_at.format(format).to_string(),
            ));
        }
        if let Some(format) = expression.strip_prefix("run.startedAt:") {
            return Ok(WorkflowExpressionValue::String(
                self.context.started_at.format(format).to_string(),
            ));
        }

        match expression {
            "run.id" => return Ok(WorkflowExpressionValue::String(self.context.run_id.clone())),
            "run.sourceBranch" => {
                return Ok(WorkflowExpressionValue::String(
                    self.context.initial_branch.clone().ok_or_else(|| {
                        GitError::Message("工作流没有可用的来源分支".into())
                    })?,
                ));
            }
            "run.sourceKind" => {
                let kind = match self.context.source_kind {
                    BranchKind::Local => "local",
                    BranchKind::Remote => "remote",
                };
                return Ok(WorkflowExpressionValue::String(kind.into()));
            }
            "git.initialBranch" => {
                return Ok(WorkflowExpressionValue::String(
                    self.context.initial_branch.clone().ok_or_else(|| {
                        GitError::Message(
                            "当前 HEAD 未指向本地分支，无法解析 git.initialBranch".into(),
                        )
                    })?,
                ));
            }
            "git.currentBranch" => {
                return Ok(WorkflowExpressionValue::String(self.current_branch()?));
            }
            "git.head" => return Ok(WorkflowExpressionValue::String(self.head_oid()?)),
            "git.repoName" => {
                return Ok(WorkflowExpressionValue::String(
                    self.context.repo_name.clone(),
                ));
            }
            _ => {}
        }

        // 步骤输出（如 filterBranches 的命中分支）优先于 vars/inputs。
        if let Some(value) = self.context.step_outputs.borrow().get(expression).cloned() {
            return Ok(value);
        }

        if let Some((root, path)) = expression.split_once('.') {
            let value = self.context.step_outputs.borrow().get(root).cloned();
            if let Some(WorkflowExpressionValue::Json(mut value)) = value {
                for key in path.split('.') {
                    value = value.get(key).cloned().ok_or_else(|| {
                        GitError::Message(format!("工作流输出字段不存在：{expression}"))
                    })?;
                }
                return Ok(WorkflowExpressionValue::from_json(value));
            }
        }

        if let Some(value) = self.definition.vars.get(expression) {
            if !stack.insert(expression.to_string()) {
                return Err(GitError::Message(format!(
                    "工作流变量存在循环引用：{expression}"
                )));
            }
            if let Some(input) = self.options.input_vars.get(expression) {
                stack.remove(expression);
                return Ok(WorkflowExpressionValue::String(input.clone()));
            }
            let resolved = self
                .interpolate_with_stack(value, stack)
                .map(WorkflowExpressionValue::String);
            stack.remove(expression);
            return resolved;
        }
        if let Some(value) = self.options.input_vars.get(expression) {
            return Ok(WorkflowExpressionValue::String(value.clone()));
        }

        Err(GitError::Message(format!("未知工作流变量：{expression}")))
    }
}

fn repo_display_name(repo: &Repository) -> String {
    repo.workdir()
        .or_else(|| repo.path().parent())
        .and_then(Path::file_name)
        .map(|name| name.to_string_lossy().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "repository".to_string())
}

fn default_require_clean_worktree() -> bool {
    true
}

fn default_workflow_input_required() -> bool {
    true
}

fn default_create_branch_checkout() -> bool {
    true
}

fn default_set_upstream() -> bool {
    true
}

fn default_filter_date_format() -> String {
    "%Y%m%d".to_string()
}

fn default_filter_date_group() -> String {
    "date".to_string()
}

fn default_filter_skip_current() -> bool {
    true
}

fn default_delete_dry_run() -> bool {
    true
}

fn default_delete_skip_current() -> bool {
    true
}

/// 分支筛选结果分类：命中、跳过（当前分支）、跳过（日期无法解析）、跳过（未超期）。
struct DeletionPlan {
    matched: Vec<String>,
    skipped_current: Vec<String>,
    skipped_nodate: Vec<String>,
    skipped_within: Vec<String>,
}

/// 把筛选结果分类渲染成 details 明细行。
fn deletion_plan_details(plan: &DeletionPlan, matched_label: &str) -> Vec<String> {
    let mut lines = Vec::new();
    for name in &plan.matched {
        lines.push(format!("{matched_label}：{name}"));
    }
    for name in &plan.skipped_current {
        lines.push(format!("跳过（当前分支）：{name}"));
    }
    for name in &plan.skipped_nodate {
        lines.push(format!("跳过（日期无法解析）：{name}"));
    }
    for name in &plan.skipped_within {
        lines.push(format!("跳过（未超过阈值）：{name}"));
    }
    if lines.is_empty() {
        lines.push("没有匹配的分支".to_string());
    }
    lines
}

/// 计算两个日期之间的日历月数差（today 相对 branch）。
/// 采用 (today.year - branch.year) * 12 + (today.month - branch.month)，
/// 不依赖较新 chrono 的 Months API；branch 在未来时返回负数。
fn months_between(branch: NaiveDate, today: NaiveDate) -> i64 {
    (today.year() as i64 - branch.year() as i64) * 12
        + (today.month() as i64 - branch.month() as i64)
}

/// 纯函数：按正则 + 日期 + 月数阈值筛选本地分支。
/// 供 preview（只读）和 execute 共用，保证两边筛选口径一致。
fn select_branches_for_deletion(
    service: &GitService,
    repo: &Repository,
    pattern: &Regex,
    date_format: &str,
    date_group: &str,
    older_than_months: i64,
    skip_current: bool,
) -> Result<DeletionPlan> {
    let branches: Vec<BranchInfo> = service
        .local_branches(repo)?
        .into_iter()
        .filter(|b| b.kind == BranchKind::Local)
        .collect();
    let current = service.current_branch(repo);
    let today = Local::now().date_naive();

    let mut plan = DeletionPlan {
        matched: Vec::new(),
        skipped_current: Vec::new(),
        skipped_nodate: Vec::new(),
        skipped_within: Vec::new(),
    };

    for branch in branches {
        // 当前分支按配置决定是否跳过
        if skip_current && current.as_deref() == Some(branch.name.as_str()) {
            plan.skipped_current.push(branch.name);
            continue;
        }
        let Some(caps) = pattern.captures(&branch.name) else {
            continue;
        };
        // 优先取命名组，否则回退到组 1
        let date_str = caps
            .name(date_group)
            .map(|m| m.as_str())
            .or_else(|| caps.get(1).map(|m| m.as_str()))
            .unwrap_or("");
        let parsed = NaiveDate::parse_from_str(date_str, date_format);
        match parsed {
            Ok(date) => {
                if months_between(date, today) > older_than_months {
                    plan.matched.push(branch.name);
                } else {
                    plan.skipped_within.push(branch.name);
                }
            }
            Err(_) => {
                plan.skipped_nodate.push(branch.name);
            }
        }
    }

    plan.matched.sort();
    plan.skipped_current.sort();
    plan.skipped_nodate.sort();
    plan.skipped_within.sort();
    Ok(plan)
}

/// 预览阶段为已解析步骤生成明细行。
/// 仅 FilterBranches 需要计算实际命中清单（只读），其它步骤返回空。
/// 预览阶段把只读步骤的输出写入 context，让后续步骤在预览时也能引用。
/// 目前仅 FilterBranches 会产生输出（命中分支数组）。
///
/// 同时返回该步骤的明细行：FilterBranches 的明细与输出同源于分支筛选，
/// 一次计算两处共用，避免每个步骤把全量分支扫描 + 逐分支正则匹配跑两遍。
fn record_preview_output(
    step: &ResolvedWorkflowStep,
    service: &GitService,
    repo: &Repository,
    context: &WorkflowEvalContext,
) -> Result<Vec<String>> {
    if let ResolvedWorkflowStep::FilterBranches {
        output,
        pattern,
        date_format,
        date_group,
        older_than_months,
        skip_current,
    } = step
    {
        let plan = select_branches_for_deletion(
            service,
            repo,
            pattern,
            date_format,
            date_group,
            *older_than_months,
            *skip_current,
        )?;
        let details = deletion_plan_details(&plan, "命中");
        context.record_output(output.clone(), WorkflowExpressionValue::Array(plan.matched));
        return Ok(details);
    }
    Ok(Vec::new())
}

#[cfg(test)]
#[path = "tests/workflow.rs"]
mod tests;

#[cfg(test)]
#[path = "tests/workflow_extensions.rs"]
mod extension_tests;
