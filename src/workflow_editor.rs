//! 工作流模板可视化创建/编辑器。
//!
//! 架构分两层：
//! - **纯数据层**（无 GPUI 依赖，可单测）：`WorkflowEditorData` 持有字符串/布尔
//!   形态的编辑数据，`build_workflow_definition` / `workflow_editor_file_name` /
//!   `workflow_step_draft_summary`、反映射 `workflow_editor_data_from_definition`
//!   与预设构造都在这层。
//! - **UI 状态层**：`WorkflowEditorState` 包一层文本框（`TextFieldState` 需要
//!   GPUI Context 构造，无法进纯数据层），经 `sync_from_fields` 回写纯数据层。
//!
//! 小白化设计：步骤类型下拉按「常用 6 种在前、高级 5 种在后」排序；空步骤
//! 列表时展示预设模板卡片一键载入；inputs / vars 收进「高级」折叠区；每个
//! 参数字段带中文说明与 placeholder。文本槽跨步骤类型复用（如 checkout 切到
//! merge 时分支名保留），避免小白误切类型清空已填内容。
//!
//! 编辑已有模板（v2）：模板列表右键「编辑此模板 / 复制为副本」。原文件含
//! JSON5 注释时先弹确认（序列化保存会丢失注释与排版），副本不弹确认——
//! 原文件不动，强制另存新文件名。

use std::fs;
use std::path::{Path, PathBuf};

use gpui::{Context, IntoElement, Window, div, prelude::*, px};
use khaslana::{
    RemoteBranchGuardAction, WorkflowDefinition, WorkflowInputDefinition, WorkflowStep,
    parse_workflow_json5,
};

use crate::{
    FieldId, RepositoryView, TextFieldState,
    ui::components::{dialog_actions, glass_menu, section_title},
    ui::theme::{self as ui_theme, rgb},
    ui_helpers::{ScrollbarMode, menu_separator, placeholder_row, scrollable_frame_when},
    workflow_view::workflow_templates_dir,
};

// ---------------------------------------------------------------------------
// 纯数据层
// ---------------------------------------------------------------------------

/// 编辑器文本字段的可寻址标识：由 `field` / `field_mut` / `focused_field`
/// 据此路由到 `WorkflowEditorState` 内的具体输入框。动态索引字段（步骤
/// 参数/输入变量/自定义变量）不进 `DEDICATED_FIELDS`（那份表只放静态字段）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorkflowEditorFieldId {
    /// 模板显示名。
    Name,
    /// 保存文件名（不含扩展名）。
    FileName,
    /// 第 N 个步骤的某个文本参数槽。
    StepParam { step: usize, slot: WorkflowStepSlot },
    /// 第 N 个输入变量的某一部分。
    InputPart {
        index: usize,
        part: WorkflowInputPart,
    },
    /// 第 N 个自定义变量的键或值。
    VarPart { index: usize, key: bool },
    /// AI 功能需求描述输入框（多行）。
    AiDescription,
    /// 添加步骤选择器的搜索框。
    PickerSearch,
}

/// 输入变量编辑行的子字段。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorkflowInputPart {
    /// 变量名（inputs 的 key）。
    Key,
    /// 显示名（label）。
    Label,
    /// 说明文字（description，运行页输入框下方展示）。
    Description,
    /// 默认值（default）。
    Default,
}

/// 步骤文本参数槽。槽跨步骤类型复用：切换步骤类型时同槽位值保留。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum WorkflowStepSlot {
    /// checkout / merge / assertBranch / push 的目标分支。
    Branch,
    /// fetch / pull / push / guardRemoteBranch 的远端名。
    Remote,
    /// createBranch 的新分支名。
    Name,
    /// createBranch 的起点。
    From,
    /// filterBranches 的输出变量名。
    Output,
    /// filterBranches 的正则。
    Pattern,
    /// filterBranches 的日期格式。
    DateFormat,
    /// filterBranches 的日期捕获组名。
    DateGroup,
    /// filterBranches 的月数阈值（字符串形态，与领域层一致）。
    OlderThanMonths,
    /// deleteBranches 的分支列表。
    Branches,
}

impl WorkflowStepSlot {
    /// 槽的中文显示名（表单 label）。
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Branch => "分支",
            Self::Remote => "远端（留空用默认远端）",
            Self::Name => "新分支名",
            Self::From => "起点（留空用当前 HEAD）",
            Self::Output => "输出变量名",
            Self::Pattern => "分支名正则（需含日期捕获组）",
            Self::DateFormat => "日期格式",
            Self::DateGroup => "日期捕获组名",
            Self::OlderThanMonths => "距今月数阈值",
            Self::Branches => "分支列表（换行分隔或 ${数组变量}）",
        }
    }

    /// 槽的 placeholder 示例。
    pub(crate) fn placeholder(self) -> &'static str {
        match self {
            Self::Branch => "如 master",
            Self::Remote => "如 origin",
            Self::Name => "如 feature-login",
            Self::From => "如 master",
            Self::Output => "如 stale",
            Self::Pattern => "如 uat_(?<date>\\d{8})",
            Self::DateFormat => "%Y%m%d",
            Self::DateGroup => "date",
            Self::OlderThanMonths => "如 3",
            Self::Branches => "如 ${out.stale}",
        }
    }
}

/// 步骤类型元数据：`op_name` 与 `WorkflowStep` 的 serde tag 一致；
/// `all()` 的顺序即下拉展示顺序（常用 6 种在前、高级 5 种在后）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorkflowStepKind {
    Checkout,
    Fetch,
    Pull,
    CreateBranch,
    Merge,
    Push,
    GuardRemoteBranch,
    EnsureClean,
    AssertBranch,
    FilterBranches,
    DeleteBranches,
}

impl WorkflowStepKind {
    /// 步骤类型的 serde tag（与 `WorkflowStep` 的 op 值一致）。
    pub(crate) fn op_name(self) -> &'static str {
        match self {
            Self::Checkout => "checkout",
            Self::Fetch => "fetch",
            Self::Pull => "pull",
            Self::CreateBranch => "createBranch",
            Self::Merge => "merge",
            Self::Push => "push",
            Self::GuardRemoteBranch => "guardRemoteBranch",
            Self::EnsureClean => "ensureClean",
            Self::AssertBranch => "assertBranch",
            Self::FilterBranches => "filterBranches",
            Self::DeleteBranches => "deleteBranches",
        }
    }

    /// 由 op 名反查步骤类型。
    pub(crate) fn from_op_name(op: &str) -> Option<Self> {
        Self::all().into_iter().find(|kind| kind.op_name() == op)
    }

    /// 步骤类型的中文显示名。
    pub(crate) fn display_name(self) -> &'static str {
        match self {
            Self::Checkout => "切换分支",
            Self::Fetch => "获取",
            Self::Pull => "拉取",
            Self::CreateBranch => "新建分支",
            Self::Merge => "合并分支",
            Self::Push => "推送",
            Self::GuardRemoteBranch => "检查远端分支",
            Self::EnsureClean => "检查工作区干净",
            Self::AssertBranch => "断言当前分支",
            Self::FilterBranches => "筛选本地分支",
            Self::DeleteBranches => "删除本地分支",
        }
    }

    /// 简短说明（类型下拉/参数区的辅助文案）。
    pub(crate) fn description(self) -> &'static str {
        match self {
            Self::Checkout => "切换到指定本地分支",
            Self::Fetch => "从远端获取最新引用，不合并",
            Self::Pull => "拉取并合并当前分支的远端更新",
            Self::CreateBranch => "基于起点创建新分支，可同时切换",
            Self::Merge => "把指定分支合并进当前分支",
            Self::Push => "推送当前分支到远端",
            Self::GuardRemoteBranch => "按远端分支是否存在决定继续或停止",
            Self::EnsureClean => "确保工作区没有未提交改动，否则停止",
            Self::AssertBranch => "当前分支与预期不符时报错停止",
            Self::FilterBranches => "按正则筛选本地分支，输出数组变量",
            Self::DeleteBranches => "删除一批本地分支（可试运行）",
        }
    }

    /// 「常用」分组的步骤数（`all()` 前 N 个），下拉菜单据此分组展示。
    pub(crate) const COMMON_COUNT: usize = 6;

    /// 该步骤类型在表单中出现的文本槽（顺序即渲染顺序）。
    pub(crate) fn slots(self) -> &'static [WorkflowStepSlot] {
        match self {
            Self::Checkout => &[WorkflowStepSlot::Branch],
            Self::Fetch => &[WorkflowStepSlot::Remote],
            Self::Pull => &[WorkflowStepSlot::Remote],
            Self::CreateBranch => &[WorkflowStepSlot::Name, WorkflowStepSlot::From],
            Self::Merge => &[WorkflowStepSlot::Branch],
            Self::Push => &[WorkflowStepSlot::Branch, WorkflowStepSlot::Remote],
            Self::GuardRemoteBranch => &[WorkflowStepSlot::Branch, WorkflowStepSlot::Remote],
            Self::EnsureClean => &[],
            Self::AssertBranch => &[WorkflowStepSlot::Branch],
            Self::FilterBranches => &[
                WorkflowStepSlot::Output,
                WorkflowStepSlot::Pattern,
                WorkflowStepSlot::DateFormat,
                WorkflowStepSlot::DateGroup,
                WorkflowStepSlot::OlderThanMonths,
            ],
            Self::DeleteBranches => &[WorkflowStepSlot::Branches],
        }
    }

    /// 全部步骤类型，按「常用在前、高级在后」排序（下拉顺序）。
    pub(crate) fn all() -> [Self; 11] {
        [
            Self::Checkout,
            Self::Fetch,
            Self::Pull,
            Self::CreateBranch,
            Self::Merge,
            Self::Push,
            Self::GuardRemoteBranch,
            Self::EnsureClean,
            Self::AssertBranch,
            Self::FilterBranches,
            Self::DeleteBranches,
        ]
    }

    /// 某槽在该步骤类型下是否必填（保存校验与红点标记共用）。
    pub(crate) fn slot_required(self, slot: WorkflowStepSlot) -> bool {
        match self {
            Self::Checkout => slot == WorkflowStepSlot::Branch,
            Self::Fetch => false,
            Self::Pull => false,
            Self::CreateBranch => slot == WorkflowStepSlot::Name,
            Self::Merge => slot == WorkflowStepSlot::Branch,
            Self::Push => false,
            Self::GuardRemoteBranch => slot == WorkflowStepSlot::Branch,
            Self::EnsureClean => false,
            Self::AssertBranch => slot == WorkflowStepSlot::Branch,
            Self::FilterBranches => matches!(
                slot,
                WorkflowStepSlot::Output
                    | WorkflowStepSlot::Pattern
                    | WorkflowStepSlot::OlderThanMonths
            ),
            Self::DeleteBranches => slot == WorkflowStepSlot::Branches,
        }
    }
}

/// 步骤布尔参数标识（交互层用，对应 `WorkflowEditorStepData` 的开关字段）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorkflowStepFlag {
    CreateCheckout,
    PushSetUpstream,
    GuardFetch,
    FilterSkipCurrent,
    DeleteDryRun,
    DeleteSkipCurrent,
}

/// 单个步骤的编辑数据：类型 + 文本槽值 + 布尔/枚举参数。
#[derive(Clone, Debug)]
pub(crate) struct WorkflowEditorStepData {
    pub(crate) kind: WorkflowStepKind,
    pub(crate) branch: String,
    pub(crate) remote: String,
    pub(crate) name: String,
    pub(crate) from: String,
    pub(crate) output: String,
    pub(crate) pattern: String,
    pub(crate) date_format: String,
    pub(crate) date_group: String,
    pub(crate) older_than_months: String,
    pub(crate) branches: String,
    /// createBranch 的创建后切换。
    pub(crate) checkout: bool,
    /// push 的建立上游跟踪。
    pub(crate) set_upstream: bool,
    /// guardRemoteBranch 的先 fetch 刷新。
    pub(crate) guard_fetch: bool,
    /// guardRemoteBranch 的远端分支存在时策略。
    pub(crate) on_exists: RemoteBranchGuardAction,
    /// guardRemoteBranch 的远端分支缺失时策略。
    pub(crate) on_missing: RemoteBranchGuardAction,
    /// filterBranches 的排除当前分支。
    pub(crate) filter_skip_current: bool,
    /// deleteBranches 的试运行。
    pub(crate) delete_dry_run: bool,
    /// deleteBranches 的跳过当前分支。
    pub(crate) delete_skip_current: bool,
}

impl WorkflowEditorStepData {
    /// 按类型创建默认步骤，高级步骤的字段给出贴近直觉的初值。
    pub(crate) fn new(kind: WorkflowStepKind) -> Self {
        Self {
            kind,
            branch: String::new(),
            remote: String::new(),
            name: String::new(),
            from: String::new(),
            output: "stale".to_string(),
            pattern: String::new(),
            date_format: "%Y%m%d".to_string(),
            date_group: "date".to_string(),
            older_than_months: String::new(),
            branches: String::new(),
            checkout: true,
            set_upstream: true,
            guard_fetch: true,
            on_exists: RemoteBranchGuardAction::Fail,
            on_missing: RemoteBranchGuardAction::Continue,
            filter_skip_current: true,
            delete_dry_run: true,
            delete_skip_current: true,
        }
    }

    /// 读取文本槽的当前值。
    pub(crate) fn slot_value(&self, slot: WorkflowStepSlot) -> &str {
        match slot {
            WorkflowStepSlot::Branch => &self.branch,
            WorkflowStepSlot::Remote => &self.remote,
            WorkflowStepSlot::Name => &self.name,
            WorkflowStepSlot::From => &self.from,
            WorkflowStepSlot::Output => &self.output,
            WorkflowStepSlot::Pattern => &self.pattern,
            WorkflowStepSlot::DateFormat => &self.date_format,
            WorkflowStepSlot::DateGroup => &self.date_group,
            WorkflowStepSlot::OlderThanMonths => &self.older_than_months,
            WorkflowStepSlot::Branches => &self.branches,
        }
    }

    /// 写入文本槽。
    pub(crate) fn set_slot_value(&mut self, slot: WorkflowStepSlot, value: String) {
        match slot {
            WorkflowStepSlot::Branch => self.branch = value,
            WorkflowStepSlot::Remote => self.remote = value,
            WorkflowStepSlot::Name => self.name = value,
            WorkflowStepSlot::From => self.from = value,
            WorkflowStepSlot::Output => self.output = value,
            WorkflowStepSlot::Pattern => self.pattern = value,
            WorkflowStepSlot::DateFormat => self.date_format = value,
            WorkflowStepSlot::DateGroup => self.date_group = value,
            WorkflowStepSlot::OlderThanMonths => self.older_than_months = value,
            WorkflowStepSlot::Branches => self.branches = value,
        }
    }

    /// 由编辑数据构建领域步骤。调用方（`build_workflow_definition`）已先行
    /// 校验必填槽非空，此处空字符串按空串透传（可选字段会转为 None）。
    fn build_step(&self) -> WorkflowStep {
        let optional = |value: &str| {
            let trimmed = value.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        };
        match self.kind {
            WorkflowStepKind::Checkout => WorkflowStep::Checkout {
                branch: self.branch.trim().to_string(),
            },
            WorkflowStepKind::Fetch => WorkflowStep::Fetch {
                remote: optional(&self.remote),
            },
            WorkflowStepKind::Pull => WorkflowStep::Pull {
                remote: optional(&self.remote),
            },
            WorkflowStepKind::CreateBranch => WorkflowStep::CreateBranch {
                name: self.name.trim().to_string(),
                from: optional(&self.from),
                checkout: self.checkout,
            },
            WorkflowStepKind::Merge => WorkflowStep::Merge {
                branch: self.branch.trim().to_string(),
            },
            WorkflowStepKind::Push => WorkflowStep::Push {
                remote: optional(&self.remote),
                branch: optional(&self.branch),
                set_upstream: self.set_upstream,
            },
            WorkflowStepKind::GuardRemoteBranch => WorkflowStep::GuardRemoteBranch {
                remote: optional(&self.remote),
                branch: self.branch.trim().to_string(),
                fetch: self.guard_fetch,
                on_exists: self.on_exists,
                on_missing: self.on_missing,
            },
            WorkflowStepKind::EnsureClean => WorkflowStep::EnsureClean,
            WorkflowStepKind::AssertBranch => WorkflowStep::AssertBranch {
                branch: self.branch.trim().to_string(),
            },
            WorkflowStepKind::FilterBranches => WorkflowStep::FilterBranches {
                output: self.output.trim().to_string(),
                pattern: self.pattern.trim().to_string(),
                date_format: self.date_format.trim().to_string(),
                date_group: self.date_group.trim().to_string(),
                older_than_months: self.older_than_months.trim().to_string(),
                skip_current: self.filter_skip_current,
            },
            WorkflowStepKind::DeleteBranches => WorkflowStep::DeleteBranches {
                branches: self.branches.trim().to_string(),
                dry_run: self.delete_dry_run,
                skip_current: self.delete_skip_current,
            },
        }
    }
}

/// 输入变量编辑行数据。
#[derive(Clone, Debug)]
pub(crate) struct WorkflowEditorInputRowData {
    pub(crate) key: String,
    pub(crate) label: String,
    /// 运行页输入框下方的说明文字（领域层 `description` 字段）。
    pub(crate) description: String,
    pub(crate) default_value: String,
    pub(crate) required: bool,
}

/// 自定义变量编辑行数据。
#[derive(Clone, Debug)]
pub(crate) struct WorkflowEditorVarRowData {
    pub(crate) key: String,
    pub(crate) value: String,
}

/// 编辑器纯数据层（可单测）：所有编辑值以字符串/布尔形态持有。
#[derive(Clone, Debug)]
pub(crate) struct WorkflowEditorData {
    pub(crate) name: String,
    pub(crate) file_name: String,
    pub(crate) require_clean_worktree: bool,
    pub(crate) steps: Vec<WorkflowEditorStepData>,
    /// 当前选中编辑的步骤下标（UI 层同步维护）。
    pub(crate) selected_step: usize,
    pub(crate) inputs: Vec<WorkflowEditorInputRowData>,
    pub(crate) vars: Vec<WorkflowEditorVarRowData>,
    /// 编辑器内错误提示（保存校验失败时显示，弹窗不关闭）。
    pub(crate) error: Option<String>,
    /// 编辑目标文件：None = 新建语义（写模板目录 + file_name）；
    /// Some = 编辑已有模板（文件名未变时原路径覆盖，改名后写新路径）。
    /// 「复制为副本」载入内容但置 None，强制另存新文件。
    pub(crate) editing_path: Option<std::path::PathBuf>,
}

impl Default for WorkflowEditorData {
    fn default() -> Self {
        Self {
            name: String::new(),
            file_name: String::new(),
            require_clean_worktree: true,
            steps: Vec::new(),
            selected_step: 0,
            inputs: Vec::new(),
            vars: Vec::new(),
            error: None,
            editing_path: None,
        }
    }
}

/// inputs/vars 键的保留字前缀校验（与领域层 `validate_definition` 的内建命名空间一致）。
fn workflow_editor_key_reserved(key: &str) -> bool {
    key.starts_with("git.") || key.starts_with("run.") || key.starts_with("date:")
}

/// 轻量启发式扫描 JSON5 文本是否含注释（`//` 行注释或 `/* */` 块注释）。
/// 跳过字符串字面量（单引号/双引号 + 反斜杠转义），避免把 URL `https://`
/// 之类的内容误判为注释；字符串里写 `//` 造成的误报只会多弹一次
/// 「保存将丢失注释」确认，安全方向正确。纯函数，可单测。
pub(crate) fn workflow_content_has_comments(content: &str) -> bool {
    let bytes = content.as_bytes();
    let mut index = 0;
    // 字符串内状态：Some(引号字节) 表示当前在对应引号的字符串字面量里。
    let mut string_quote: Option<u8> = None;
    while index < bytes.len() {
        let byte = bytes[index];
        if let Some(quote) = string_quote {
            if byte == b'\\' {
                // 转义序列跳过下一个字符（含转义引号）
                index += 2;
                continue;
            }
            if byte == quote {
                string_quote = None;
            }
            index += 1;
            continue;
        }
        match byte {
            b'"' | b'\'' => {
                string_quote = Some(byte);
                index += 1;
            }
            b'/' if index + 1 < bytes.len() && bytes[index + 1] == b'/' => return true,
            b'/' if index + 1 < bytes.len() && bytes[index + 1] == b'*' => return true,
            _ => index += 1,
        }
    }
    false
}

/// 由已解析的领域定义反向构建编辑器数据（编辑/复制入口）。
/// 11 种步骤逐字段回填：`Option<String>` 空值映射为空串，布尔/枚举直接透传；
/// inputs 含 description 完整往返。纯函数，可单测。
pub(crate) fn workflow_editor_data_from_definition(
    definition: &WorkflowDefinition,
    file_stem: &str,
) -> WorkflowEditorData {
    let optional = |value: &Option<String>| value.clone().unwrap_or_default();
    let steps = definition
        .steps
        .iter()
        .map(|step| {
            let mut editor = WorkflowEditorStepData::new(
                WorkflowStepKind::from_op_name(step_op_name(step))
                    .unwrap_or(WorkflowStepKind::Checkout),
            );
            match step {
                WorkflowStep::Checkout { branch } => editor.branch = branch.clone(),
                WorkflowStep::Fetch { remote } => editor.remote = optional(remote),
                WorkflowStep::Pull { remote } => editor.remote = optional(remote),
                WorkflowStep::CreateBranch {
                    name,
                    from,
                    checkout,
                } => {
                    editor.name = name.clone();
                    editor.from = optional(from);
                    editor.checkout = *checkout;
                }
                WorkflowStep::Merge { branch } => editor.branch = branch.clone(),
                WorkflowStep::Push {
                    remote,
                    branch,
                    set_upstream,
                } => {
                    editor.remote = optional(remote);
                    editor.branch = optional(branch);
                    editor.set_upstream = *set_upstream;
                }
                WorkflowStep::GuardRemoteBranch {
                    remote,
                    branch,
                    fetch,
                    on_exists,
                    on_missing,
                } => {
                    editor.remote = optional(remote);
                    editor.branch = branch.clone();
                    editor.guard_fetch = *fetch;
                    editor.on_exists = *on_exists;
                    editor.on_missing = *on_missing;
                }
                WorkflowStep::EnsureClean => {}
                WorkflowStep::AssertBranch { branch } => editor.branch = branch.clone(),
                WorkflowStep::FilterBranches {
                    output,
                    pattern,
                    date_format,
                    date_group,
                    older_than_months,
                    skip_current,
                } => {
                    editor.output = output.clone();
                    editor.pattern = pattern.clone();
                    editor.date_format = date_format.clone();
                    editor.date_group = date_group.clone();
                    editor.older_than_months = older_than_months.clone();
                    editor.filter_skip_current = *skip_current;
                }
                WorkflowStep::DeleteBranches {
                    branches,
                    dry_run,
                    skip_current,
                } => {
                    editor.branches = branches.clone();
                    editor.delete_dry_run = *dry_run;
                    editor.delete_skip_current = *skip_current;
                }
            }
            editor
        })
        .collect();

    let inputs = definition
        .inputs
        .iter()
        .map(|(key, input)| WorkflowEditorInputRowData {
            key: key.clone(),
            label: optional(&input.label),
            description: optional(&input.description),
            default_value: optional(&input.default),
            required: input.required,
        })
        .collect();

    let vars = definition
        .vars
        .iter()
        .map(|(key, value)| WorkflowEditorVarRowData {
            key: key.clone(),
            value: value.clone(),
        })
        .collect();

    WorkflowEditorData {
        name: definition.name.clone().unwrap_or_default(),
        file_name: file_stem.to_string(),
        require_clean_worktree: definition.defaults.require_clean_worktree,
        steps,
        selected_step: 0,
        inputs,
        vars,
        error: None,
        editing_path: None,
    }
}

/// 把 AI 生成的 JSON5 文本解析并回填到编辑器数据（AI 创建/编辑的落地步骤）。
///
/// - 先剥 AI 可能添加的 markdown 代码块围栏，再走标准解析 + 校验；
/// - 替换 name / require_clean_worktree / steps / inputs / vars；
/// - 保留调用方语义字段：`editing_path`、`error` 不动；`file_name` 仅在
///   原为空时按 AI 给的显示名推导；有 inputs/vars 时自动展开高级区，
///   方便用户检查 AI 写的变量。
/// - 解析/校验失败返回中文错误（含原始解析信息与重试引导）。
pub(crate) fn apply_ai_generated_to_editor_data(
    data: &mut WorkflowEditorData,
    json5_text: &str,
) -> Result<(), String> {
    let cleaned = khaslana::strip_code_fence(json5_text.trim());
    let definition = parse_workflow_json5(&cleaned)
        .map_err(|err| format!("AI 生成的内容不是有效的工作流模板：{err}。请重试或调整需求描述"));

    let generated = match definition {
        Ok(definition) => {
            // file_name 已填则以它为准；为空时用 AI 给的显示名推导主干。
            let stem_hint = if data.file_name.trim().is_empty() {
                definition.name.clone().unwrap_or_default()
            } else {
                data.file_name.clone()
            };
            workflow_editor_data_from_definition(&definition, &stem_hint)
        }
        Err(err) => return Err(err),
    };

    let file_name_empty = data.file_name.trim().is_empty();
    let editing_path = data.editing_path.clone();
    let error = data.error.take();

    *data = WorkflowEditorData {
        // AI 未提供名称时保留编辑器当前文件名（新建模式为空则由用户填写）。
        file_name: if file_name_empty && !generated.name.is_empty() {
            slug_from_display_name(&generated.name)
        } else {
            data.file_name.clone()
        },
        editing_path,
        error,
        ..generated
    };
    Ok(())
}

/// 由显示名推导保守的文件名主干：仅保留字母数字/连字符/下划线/中文，
/// 其余替换为连字符；结果可能仍不合法（如全符号），保存时由
/// `workflow_editor_file_name` 兜底校验。
fn slug_from_display_name(display_name: &str) -> String {
    let mut slug = display_name
        .trim()
        .chars()
        .map(|ch| {
            if ch.is_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>();
    slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        "ai-template".to_string()
    } else {
        slug
    }
}

/// 读取领域步骤的 serde op tag（反映射分发用，与 `WorkflowStepKind::op_name` 对应）。
fn step_op_name(step: &WorkflowStep) -> &'static str {
    match step {
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

/// 由编辑数据构建工作流定义，逐项给出中文校验错误。
/// 返回 `Err(中文错误)` 时调用方把错误展示在编辑器内且不关闭弹窗。
pub(crate) fn build_workflow_definition(
    data: &WorkflowEditorData,
) -> Result<WorkflowDefinition, String> {
    if data.steps.is_empty() {
        return Err("至少需要一个步骤".to_string());
    }
    let steps = data
        .steps
        .iter()
        .enumerate()
        .map(|(index, step)| {
            for &slot in step.kind.slots() {
                if step.kind.slot_required(slot) && step.slot_value(slot).trim().is_empty() {
                    return Err(format!(
                        "第 {} 个步骤（{}）缺少「{}」",
                        index + 1,
                        step.kind.display_name(),
                        slot.label()
                    ));
                }
            }
            Ok(step.build_step())
        })
        .collect::<Result<Vec<_>, String>>()?;

    let mut inputs = std::collections::BTreeMap::new();
    for (index, input) in data.inputs.iter().enumerate() {
        let key = input.key.trim();
        if key.is_empty() {
            return Err(format!("第 {} 个输入变量的变量名为空", index + 1));
        }
        if workflow_editor_key_reserved(key) {
            return Err(format!(
                "输入变量「{key}」使用了保留前缀（git. / run. / date:），请换一个名字"
            ));
        }
        if inputs.contains_key(key) {
            return Err(format!("输入变量「{key}」重复"));
        }
        let label = input.label.trim();
        let description = input.description.trim();
        let default = input.default_value.trim();
        inputs.insert(
            key.to_string(),
            WorkflowInputDefinition {
                label: (!label.is_empty()).then(|| label.to_string()),
                description: (!description.is_empty()).then(|| description.to_string()),
                default: (!default.is_empty()).then(|| default.to_string()),
                required: input.required,
            },
        );
    }

    let mut vars = std::collections::BTreeMap::new();
    for (index, var) in data.vars.iter().enumerate() {
        let key = var.key.trim();
        if key.is_empty() {
            return Err(format!("第 {} 个自定义变量的变量名为空", index + 1));
        }
        if workflow_editor_key_reserved(key) {
            return Err(format!(
                "自定义变量「{key}」使用了保留前缀（git. / run. / date:），请换一个名字"
            ));
        }
        if vars.contains_key(key) {
            return Err(format!("自定义变量「{key}」重复"));
        }
        vars.insert(key.to_string(), var.value.trim().to_string());
    }

    let name = data.name.trim();
    Ok(WorkflowDefinition {
        version: 1,
        name: (!name.is_empty()).then(|| name.to_string()),
        defaults: khaslana::WorkflowDefaults {
            require_clean_worktree: data.require_clean_worktree,
        },
        inputs,
        vars,
        steps,
    })
}

/// 校验并规范化保存文件名：非空、无非法字符、与现有模板不重名。
/// 返回带 `.json5` 后缀的文件名。
pub(crate) fn workflow_editor_file_name(
    raw: &str,
    existing_templates: &[String],
) -> Result<String, String> {
    let stem = raw
        .trim()
        .trim_end_matches(".json5")
        .trim_end_matches(".jsonc")
        .trim();
    if stem.is_empty() {
        return Err("请填写模板文件名".to_string());
    }
    if stem.chars().count() > 80 {
        return Err("模板文件名过长（最多 80 字符）".to_string());
    }
    if let Some(bad) = stem.chars().find(|ch| {
        ch.is_control() || matches!(ch, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
    }) {
        return Err(format!("文件名含非法字符「{bad}」"));
    }
    let file_name = format!("{stem}.json5");
    if existing_templates
        .iter()
        .any(|name| name.eq_ignore_ascii_case(&file_name))
    {
        return Err(format!("已存在同名模板文件「{file_name}」，请换个文件名"));
    }
    Ok(file_name)
}

/// 编辑模式下决定写盘目标的纯函数（可单测）。
///
/// 按**文件主干**（忽略大小写）比较而非全名：原文件可能是 `.jsonc` 扩展名，
/// 而编辑器保存恒为 `.json5`，按全名比较会让 `.jsonc` 模板每次保存都被误判
/// 为「改名」从而多出一个新文件。主干相同 → 覆盖原路径（保留原扩展名）；
/// 主干不同（用户改名）→ 写新路径并返回旧路径供调用方删除（重命名语义，
/// 否则目录里会残留旧文件）。
pub(crate) fn workflow_editor_save_target(
    editing_path: Option<&Path>,
    new_file_name: &str,
) -> (PathBuf, Option<PathBuf>) {
    let Some(editing_path) = editing_path else {
        // 新建/副本：直接写模板目录 + 新名，无旧文件可删。
        return (PathBuf::from(new_file_name), None);
    };
    let old_stem = editing_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(|stem| stem.to_uppercase());
    let new_stem = PathBuf::from(new_file_name)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(|stem| stem.to_uppercase());
    if old_stem.is_some() && old_stem == new_stem {
        // 未改名（含 .jsonc 等其它受支持扩展名）：原地覆盖。
        (editing_path.to_path_buf(), None)
    } else {
        // 改名：写到同目录新名，旧文件由调用方删除。
        let dir = editing_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        (dir.join(new_file_name), Some(editing_path.to_path_buf()))
    }
}

// ---------------------------------------------------------------------------
// 预设模板
// ---------------------------------------------------------------------------

/// 预设模板标识（点击卡片应用）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorkflowEditorPreset {
    /// 同步当前分支：拉取 + 推送。
    SyncCurrentBranch,
    /// 新建功能分支并推送（含 target 输入变量示范）。
    FeatureBranch,
    /// 合并分支并推送（含 source 输入变量）。
    MergeAndPush,
}

impl WorkflowEditorPreset {
    /// 预设卡片标题。
    fn title(self) -> &'static str {
        match self {
            Self::SyncCurrentBranch => "同步当前分支",
            Self::FeatureBranch => "新建功能分支并推送",
            Self::MergeAndPush => "合并分支并推送",
        }
    }

    /// 预设卡片描述。
    fn description(self) -> &'static str {
        match self {
            Self::SyncCurrentBranch => "拉取远端更新并推送本地提交，适合日常同步",
            Self::FeatureBranch => "切到基础分支、拉取最新、创建功能分支并推送（示范输入变量）",
            Self::MergeAndPush => "获取远端、把指定分支合并进当前分支并推送",
        }
    }

    /// 生成预设的编辑数据（含默认名与步骤）。
    fn build_data(self) -> WorkflowEditorData {
        let mut data = WorkflowEditorData::default();
        match self {
            Self::SyncCurrentBranch => {
                data.name = "同步当前分支".to_string();
                data.file_name = "sync-current-branch".to_string();
                data.steps = vec![
                    WorkflowEditorStepData::new(WorkflowStepKind::Pull),
                    WorkflowEditorStepData::new(WorkflowStepKind::Push),
                ];
            }
            Self::FeatureBranch => {
                data.name = "新建功能分支并推送".to_string();
                data.file_name = "feature-branch".to_string();
                data.inputs = vec![WorkflowEditorInputRowData {
                    key: "target".to_string(),
                    label: "新分支名".to_string(),
                    description: String::new(),
                    default_value: "feature-${date:%Y%m%d}".to_string(),
                    required: true,
                }];
                let mut checkout = WorkflowEditorStepData::new(WorkflowStepKind::Checkout);
                checkout.branch = "master".to_string();
                let mut create = WorkflowEditorStepData::new(WorkflowStepKind::CreateBranch);
                create.name = "${target}".to_string();
                let mut push = WorkflowEditorStepData::new(WorkflowStepKind::Push);
                push.branch = "${target}".to_string();
                data.steps = vec![
                    checkout,
                    WorkflowEditorStepData::new(WorkflowStepKind::Pull),
                    create,
                    push,
                ];
            }
            Self::MergeAndPush => {
                data.name = "合并分支并推送".to_string();
                data.file_name = "merge-and-push".to_string();
                data.inputs = vec![WorkflowEditorInputRowData {
                    key: "source".to_string(),
                    label: "要合并的分支".to_string(),
                    description: String::new(),
                    default_value: String::new(),
                    required: true,
                }];
                let mut merge = WorkflowEditorStepData::new(WorkflowStepKind::Merge);
                merge.branch = "${source}".to_string();
                data.steps = vec![
                    WorkflowEditorStepData::new(WorkflowStepKind::Fetch),
                    merge,
                    WorkflowEditorStepData::new(WorkflowStepKind::Push),
                ];
            }
        }
        data
    }
}

const WORKFLOW_EDITOR_PRESETS: &[WorkflowEditorPreset] = &[
    WorkflowEditorPreset::SyncCurrentBranch,
    WorkflowEditorPreset::FeatureBranch,
    WorkflowEditorPreset::MergeAndPush,
];

// ---------------------------------------------------------------------------
// 分步向导（纯函数）
// ---------------------------------------------------------------------------

/// 向导各步的标签（下标即展示顺序，步骤指示器与底部计数共用）。
pub(crate) const WORKFLOW_WIZARD_STEPS: [&str; 4] = ["基本信息", "操作步骤", "变量设置", "完成"];

/// 「变量设置」步在向导中的下标（绑定条的「去定义」跳转目标）。
pub(crate) const WIZARD_STEP_VARS: usize = 2;

/// 提取参数值里引用的模板变量名：扫描 `${...}` 片段，内建命名空间
/// （git. / run. / date:）不属于用户变量，跳过。同名去重、保持首次出现顺序。
/// 纯函数，可单测。
pub(crate) fn extract_template_var_refs(value: &str) -> Vec<String> {
    let mut refs = Vec::new();
    let bytes = value.as_bytes();
    let mut index = 0;
    while index + 1 < bytes.len() {
        if bytes[index] == b'$' && bytes[index + 1] == b'{' {
            let Some(close) = value[index + 2..].find('}') else {
                break;
            };
            let name = value[index + 2..index + 2 + close].trim();
            let builtin =
                name.starts_with("git.") || name.starts_with("run.") || name.starts_with("date:");
            if !builtin && !name.is_empty() && !refs.iter().any(|existing| existing == name) {
                refs.push(name.to_string());
            }
            index = index + 2 + close + 1;
        } else {
            index += 1;
        }
    }
    refs
}

/// 单个步骤的变量引用分析结果：引用了哪些已声明变量、哪些未声明、
/// 以及本步骤（filterBranches）对外输出的变量名。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct WorkflowStepBinding {
    /// 引用的全部用户变量名（含上方步骤输出的 out.*）。
    pub(crate) used: Vec<String>,
    /// 引用了但既不在 inputs/vars、也不是任何更早步骤输出的名字。
    pub(crate) unknown: Vec<String>,
    /// filterBranches 的输出变量（`out.{output}`），供后续步骤引用。
    pub(crate) produces: Option<String>,
}

/// 分析第 `step_index` 个步骤的变量引用：声明集合为 inputs/vars 键 +
/// 更早 filterBranches 步骤累计的输出名。纯函数，可单测。
pub(crate) fn workflow_step_binding(
    step_index: usize,
    steps: &[WorkflowEditorStepData],
    declared_inputs: &[String],
    declared_vars: &[String],
) -> WorkflowStepBinding {
    let Some(step) = steps.get(step_index) else {
        return WorkflowStepBinding::default();
    };
    // 更早 filterBranches 步骤的输出集合（out.{output}）。
    let mut earlier_outputs: Vec<String> = Vec::new();
    for earlier in steps.iter().take(step_index) {
        if earlier.kind == WorkflowStepKind::FilterBranches {
            let output = earlier.output.trim();
            if !output.is_empty() {
                earlier_outputs.push(format!("out.{output}"));
            }
        }
    }

    let mut used: Vec<String> = Vec::new();
    let mut unknown: Vec<String> = Vec::new();
    for slot in step.kind.slots() {
        for name in extract_template_var_refs(step.slot_value(*slot)) {
            let declared = declared_inputs.contains(&name)
                || declared_vars.contains(&name)
                || earlier_outputs.contains(&name);
            if !used.contains(&name) {
                used.push(name.clone());
            }
            if !declared && !unknown.contains(&name) {
                unknown.push(name);
            }
        }
    }

    let produces = (step.kind == WorkflowStepKind::FilterBranches)
        .then(|| format!("out.{}", step.output.trim()))
        .filter(|out| out != "out.");

    WorkflowStepBinding {
        used,
        unknown,
        produces,
    }
}

/// 绑定条文案：使用变量 / 输出变量 / 未声明警告三段拼接。纯函数，可单测。
pub(crate) fn workflow_binding_summary(binding: &WorkflowStepBinding) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !binding.used.is_empty() {
        parts.push(format!("使用变量 {}", binding.used.join("、")));
    }
    if let Some(out) = &binding.produces {
        parts.push(format!("输出 {out} 可供后续步骤使用"));
    }
    if !binding.unknown.is_empty() {
        parts.push(format!(
            "引用了未声明变量 {} · 去「变量设置」定义",
            binding.unknown.join("、")
        ));
    }
    if parts.is_empty() {
        "未引用变量".to_string()
    } else {
        parts.join(" · ")
    }
}

/// 汇总某个输入/自定义变量被哪些步骤（1 起始序号）引用，用于变量卡片的
/// 「被步骤 N 引用」标签。返回空 vec 表示未被引用。纯函数，可单测。
pub(crate) fn workflow_var_referenced_by_steps(
    name: &str,
    steps: &[WorkflowEditorStepData],
) -> Vec<usize> {
    let mut referencing = Vec::new();
    for (index, step) in steps.iter().enumerate() {
        let references = step.kind.slots().iter().any(|slot| {
            extract_template_var_refs(step.slot_value(*slot)).contains(&name.to_string())
        });
        if references {
            referencing.push(index + 1);
        }
    }
    referencing
}

/// 按搜索词过滤选择器里的步骤类型：匹配中文名 / 操作名 / 说明，
/// 大小写不敏感；空词返回全量（常用在前、高级在后）。纯函数，可单测。
pub(crate) fn filter_workflow_step_kinds(query: &str) -> Vec<WorkflowStepKind> {
    let query = query.trim().to_lowercase();
    let all = WorkflowStepKind::all();
    if query.is_empty() {
        return all.to_vec();
    }
    all.into_iter()
        .filter(|kind| {
            kind.display_name().to_lowercase().contains(&query)
                || kind.op_name().to_lowercase().contains(&query)
                || kind.description().to_lowercase().contains(&query)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// UI 状态层（文本框包装）
// ---------------------------------------------------------------------------

/// 注释丢失确认前的暂存：待编辑的模板路径与已反映射的编辑数据。
/// 确认弹窗（`ConfirmWorkflowEditComments`）确认后据此进入编辑器。
pub(crate) struct PendingWorkflowEdit {
    pub(crate) path: std::path::PathBuf,
    pub(crate) data: WorkflowEditorData,
}

/// 单个步骤的 UI 状态：文本框 + 回写数据的引用槽。
struct WorkflowEditorStepState {
    /// 该步骤各槽的文本框，按 `WorkflowStepSlot` 寻址。
    /// 惰性创建：首次渲染某槽时才建框，未建框的槽值以纯数据层为准。
    fields: std::collections::HashMap<WorkflowStepSlot, TextFieldState>,
}

/// 编辑器整体 UI 状态：包住纯数据层并持有全部文本框。
pub(crate) struct WorkflowEditorState {
    data: WorkflowEditorData,
    name_field: TextFieldState,
    file_name_field: TextFieldState,
    /// 步骤文本框，与 `data.steps` 按下标一一对应。
    step_fields: Vec<WorkflowEditorStepState>,
    /// 输入变量文本框，与 `data.inputs` 按下标一一对应。
    input_fields: Vec<WorkflowEditorInputRowState>,
    /// 自定义变量文本框，与 `data.vars` 按下标一一对应。
    var_fields: Vec<WorkflowEditorVarRowState>,
    /// 当前展开「步骤类型」下拉菜单的步骤下标（None = 全部收起）。
    kind_menu_open: Option<usize>,
    /// 当前展开的守卫策略下拉：(步骤下标, 是否 on_exists)，None = 全部收起。
    /// 互斥展开，同一时间只开一个。
    guard_menu_open: Option<(usize, bool)>,
    /// AI 模板生成进行中（防重入 + 按钮加载态）。
    ai_loading: bool,
    /// AI 功能需求描述输入框（用户自然语言描述想要的模板）。
    ai_description_field: TextFieldState,
    /// 向导当前步下标（0 基本信息 / 1 操作步骤 / 2 变量设置 / 3 完成）。
    wizard_step: usize,
    /// 第 2 步是否处于「添加步骤选择器」视图（替代步骤卡片列表）。
    step_picker_open: bool,
    /// 选择器的搜索框（按名称 / 操作名 / 说明过滤步骤类型）。
    picker_search_field: TextFieldState,
}

struct WorkflowEditorInputRowState {
    key_field: Option<TextFieldState>,
    label_field: Option<TextFieldState>,
    description_field: Option<TextFieldState>,
    default_field: Option<TextFieldState>,
}

struct WorkflowEditorVarRowState {
    key_field: Option<TextFieldState>,
    value_field: Option<TextFieldState>,
}

impl WorkflowEditorState {
    /// 新建空白编辑器状态。
    fn new(cx: &mut Context<RepositoryView>) -> Self {
        Self::from_data(WorkflowEditorData::default(), cx)
    }

    /// 由纯数据（预设或既有草稿）构建 UI 状态，文本框按需初始化并预填。
    fn from_data(data: WorkflowEditorData, cx: &mut Context<RepositoryView>) -> Self {
        let name_field =
            TextFieldState::new(cx, "显示在模板列表中的名称").with_value(data.name.clone());
        let file_name_field =
            TextFieldState::new(cx, "如 my-workflow").with_value(data.file_name.clone());
        let mut state = Self {
            data,
            name_field,
            file_name_field,
            step_fields: Vec::new(),
            input_fields: Vec::new(),
            var_fields: Vec::new(),
            kind_menu_open: None,
            guard_menu_open: None,
            ai_loading: false,
            ai_description_field: TextFieldState::new(
                cx,
                "用一句话描述你想要的模板，如：基于 master 创建 release 分支并推送",
            ),
            wizard_step: 0,
            step_picker_open: false,
            picker_search_field: TextFieldState::new(cx, "搜索操作"),
        };
        state.ensure_field_capacity();
        state
    }

    /// 步骤/行数变化后补齐文本框容器（值在渲染时按需预填）。
    fn ensure_field_capacity(&mut self) {
        self.step_fields
            .resize_with(self.data.steps.len(), || WorkflowEditorStepState {
                fields: std::collections::HashMap::new(),
            });
        self.input_fields
            .resize_with(self.data.inputs.len(), || WorkflowEditorInputRowState {
                key_field: None,
                label_field: None,
                description_field: None,
                default_field: None,
            });
        self.var_fields
            .resize_with(self.data.vars.len(), || WorkflowEditorVarRowState {
                key_field: None,
                value_field: None,
            });
    }

    /// 读取步骤槽文本框；不存在时以数据层值创建（惰性初始化 + 预填）。
    /// 读取步骤槽文本框；不存在时以数据层值创建（惰性初始化 + 预填）。
    /// 先不可变借用 steps 读预填值（clone 出来），再可变借用 step_fields，
    /// 避免对同一 state 的交叉借用。
    fn step_slot_field<'a>(
        state: &'a mut WorkflowEditorState,
        step: usize,
        slot: WorkflowStepSlot,
        cx: &mut Context<RepositoryView>,
    ) -> Option<&'a mut TextFieldState> {
        let value = state.data.steps.get(step)?.slot_value(slot).to_string();
        let step_state = state.step_fields.get_mut(step)?;
        if !step_state.fields.contains_key(&slot) {
            step_state.fields.insert(
                slot,
                TextFieldState::new(cx, slot.placeholder()).with_value(value),
            );
        }
        step_state.fields.get_mut(&slot)
    }

    /// 同步把文本框当前值写回纯数据层（保存前与变更回调时调用）。
    fn sync_from_fields(&mut self) {
        self.data.name = self.name_field.value.clone();
        self.data.file_name = self.file_name_field.value.clone();
        for (index, step_state) in self.step_fields.iter().enumerate() {
            let Some(step) = self.data.steps.get_mut(index) else {
                continue;
            };
            for (slot, field) in step_state.fields.iter() {
                step.set_slot_value(*slot, field.value.clone());
            }
        }
        for (index, row_state) in self.input_fields.iter().enumerate() {
            let Some(row) = self.data.inputs.get_mut(index) else {
                continue;
            };
            if let Some(field) = &row_state.key_field {
                row.key = field.value.clone();
            }
            if let Some(field) = &row_state.label_field {
                row.label = field.value.clone();
            }
            if let Some(field) = &row_state.description_field {
                row.description = field.value.clone();
            }
            if let Some(field) = &row_state.default_field {
                row.default_value = field.value.clone();
            }
        }
        for (index, row_state) in self.var_fields.iter().enumerate() {
            let Some(row) = self.data.vars.get_mut(index) else {
                continue;
            };
            if let Some(field) = &row_state.key_field {
                row.key = field.value.clone();
            }
            if let Some(field) = &row_state.value_field {
                row.value = field.value.clone();
            }
        }
    }
}

/// 步骤类型下拉菜单滚动区的 'static id：按 index 惰性生成一次并缓存
/// （渲染闭包每帧取用，不能每帧泄漏新串）。
fn workflow_step_menu_scroll_id(index: usize) -> &'static str {
    static CACHE: std::sync::Mutex<Vec<&'static str>> = std::sync::Mutex::new(Vec::new());
    let mut cache = CACHE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if index >= cache.len() {
        cache.resize(index + 1, "");
    }
    if cache[index].is_empty() {
        cache[index] =
            Box::leak(format!("workflow-editor-step-kind-{index}-menu").into_boxed_str());
    }
    cache[index]
}

impl RepositoryView {
    /// 打开「新建工作流模板」编辑器。
    pub(crate) fn open_workflow_editor(&mut self, cx: &mut Context<Self>) {
        self.close_popups();
        self.workflow_editor = Some(WorkflowEditorState::new(cx));
        self.active_dialog = Some(crate::DialogState::WorkflowEditor);
        self.last_error = None;
    }

    /// 打开「编辑已有模板」：读文件 → 解析 → 反映射为编辑数据。
    ///
    /// 原文件含 JSON5 注释时先弹确认（保存会丢失注释与排版），确认后
    /// 才真正进编辑器；无注释直接进入。「复制为副本」（`as_copy`）不弹
    /// 确认——原文件不会被覆盖，载入内容并强制另存新文件名。
    pub(crate) fn open_workflow_editor_for_path(
        &mut self,
        path: std::path::PathBuf,
        as_copy: bool,
        cx: &mut Context<Self>,
    ) {
        self.close_popups();
        let content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(err) => {
                self.notify_error(format!("工作流模板读取失败：{err}"), cx);
                return;
            }
        };
        let definition = match parse_workflow_json5(&content) {
            Ok(definition) => definition,
            Err(err) => {
                self.notify_error(format!("无法编辑该模板：{err}"), cx);
                return;
            }
        };
        let file_stem = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| "template".to_string());
        let mut data = workflow_editor_data_from_definition(&definition, &file_stem);
        if as_copy {
            // 副本语义：原文件不动，强制另存新文件名。
            data.file_name = format!("{file_stem}-copy");
            data.editing_path = None;
        } else if workflow_content_has_comments(&content) {
            // 有注释先确认丢失风险；把待开数据暂存，确认后经
            // confirm_workflow_edit_comments 进入编辑器。
            self.pending_workflow_edit = Some(PendingWorkflowEdit { path, data });
            self.active_dialog = Some(crate::DialogState::ConfirmWorkflowEditComments);
            return;
        } else {
            data.editing_path = Some(path);
        }
        self.workflow_editor = Some(WorkflowEditorState::from_data(data, cx));
        self.active_dialog = Some(crate::DialogState::WorkflowEditor);
    }

    /// 注释丢失确认后真正进入编辑模式。
    pub(crate) fn confirm_workflow_edit_comments(&mut self, cx: &mut Context<Self>) {
        let Some(pending) = self.pending_workflow_edit.take() else {
            self.active_dialog = None;
            return;
        };
        let mut data = pending.data;
        data.editing_path = Some(pending.path);
        self.workflow_editor = Some(WorkflowEditorState::from_data(data, cx));
        self.active_dialog = Some(crate::DialogState::WorkflowEditor);
    }

    /// 关闭编辑器（放弃未保存内容）。
    pub(crate) fn close_workflow_editor(&mut self) {
        self.workflow_editor = None;
        self.active_dialog = None;
        self.pending_workflow_edit = None;
    }

    /// 应用预设模板（仅编辑器步骤为空时生效，防止误覆盖已编辑内容）。
    pub(crate) fn apply_workflow_editor_preset(
        &mut self,
        preset: WorkflowEditorPreset,
        cx: &mut Context<Self>,
    ) {
        if let Some(state) = self.workflow_editor.as_mut() {
            if state.data.steps.is_empty() {
                *state = WorkflowEditorState::from_data(preset.build_data(), cx);
                // 预设的主体是步骤序列，载入后直接进入「操作步骤」步。
                state.wizard_step = 1;
            }
        }
    }

    /// 按类型添加一个步骤（选择器点击选项），并选中新步骤。
    pub(crate) fn workflow_editor_add_step_of_kind(&mut self, kind: WorkflowStepKind) {
        if let Some(state) = self.workflow_editor.as_mut() {
            state.data.steps.push(WorkflowEditorStepData::new(kind));
            state.data.selected_step = state.data.steps.len() - 1;
            state.step_picker_open = false;
            state.ensure_field_capacity();
        }
    }

    /// 跳转到向导第 step 步（越界钳制），并收起弹出的菜单。
    pub(crate) fn workflow_editor_goto_step(&mut self, step: usize) {
        if let Some(state) = self.workflow_editor.as_mut() {
            state.wizard_step = step.min(WORKFLOW_WIZARD_STEPS.len() - 1);
            state.kind_menu_open = None;
            state.guard_menu_open = None;
            // 离开第 2 步时收起选择器视图。
            if state.wizard_step != 1 {
                state.step_picker_open = false;
            }
        }
    }

    /// 向导下一步。
    pub(crate) fn workflow_editor_next_step(&mut self, cx: &mut Context<Self>) {
        let current = self
            .workflow_editor
            .as_ref()
            .map(|state| state.wizard_step)
            .unwrap_or(0);
        self.workflow_editor_goto_step(current + 1);
        cx.notify();
    }

    /// 向导上一步（第 1 步时无效）。
    pub(crate) fn workflow_editor_prev_step(&mut self, cx: &mut Context<Self>) {
        let current = self
            .workflow_editor
            .as_ref()
            .map(|state| state.wizard_step)
            .unwrap_or(0);
        if current > 0 {
            self.workflow_editor_goto_step(current - 1);
        }
        cx.notify();
    }

    /// 打开「添加步骤」选择器视图，并清空上次的搜索词。
    pub(crate) fn workflow_editor_open_step_picker(&mut self) {
        if let Some(state) = self.workflow_editor.as_mut() {
            state.step_picker_open = true;
            state.picker_search_field.set_value(String::new());
        }
    }

    /// 关闭选择器视图，回到步骤卡片列表。
    pub(crate) fn workflow_editor_close_step_picker(&mut self) {
        if let Some(state) = self.workflow_editor.as_mut() {
            state.step_picker_open = false;
        }
    }

    /// 绑定条「去定义」：跳到变量设置步并展开对应分组（inputs/vars 共页）。
    pub(crate) fn workflow_editor_jump_to_vars(&mut self, cx: &mut Context<Self>) {
        self.workflow_editor_goto_step(WIZARD_STEP_VARS);
        cx.notify();
    }

    /// 删除第 index 个步骤；选中态钳制到剩余范围。
    pub(crate) fn workflow_editor_remove_step(&mut self, index: usize) {
        if let Some(state) = self.workflow_editor.as_mut() {
            if index < state.data.steps.len() {
                state.data.steps.remove(index);
                if index < state.step_fields.len() {
                    state.step_fields.remove(index);
                }
                if state.data.selected_step >= state.data.steps.len() {
                    state.data.selected_step = state.data.steps.len().saturating_sub(1);
                }
            }
        }
    }

    /// 上移/下移第 index 个步骤；越界时忽略，成功后选中目标位置。
    pub(crate) fn workflow_editor_move_step(&mut self, index: usize, up: bool) {
        if let Some(state) = self.workflow_editor.as_mut() {
            let target = if up {
                index.checked_sub(1)
            } else {
                index.checked_add(1)
            };
            let Some(target) = target else { return };
            if index < state.data.steps.len() && target < state.data.steps.len() {
                state.data.steps.swap(index, target);
                if index < state.step_fields.len() && target < state.step_fields.len() {
                    state.step_fields.swap(index, target);
                }
                state.data.selected_step = target;
            }
        }
    }

    /// 切换第 index 个步骤的类型；同槽位值保留（小白误切不丢已填输入）。
    pub(crate) fn workflow_editor_set_step_kind(&mut self, index: usize, kind: WorkflowStepKind) {
        if let Some(state) = self.workflow_editor.as_mut() {
            if let Some(step) = state.data.steps.get_mut(index) {
                step.kind = kind;
            }
        }
    }

    /// 展开/收起第 index 个步骤的类型下拉菜单（互斥：同时只开一个）。
    pub(crate) fn workflow_editor_toggle_kind_menu(&mut self, index: usize) {
        if let Some(state) = self.workflow_editor.as_mut() {
            state.kind_menu_open = if state.kind_menu_open == Some(index) {
                None
            } else {
                Some(index)
            };
        }
    }

    /// 第 index 个步骤的类型下拉菜单是否展开。
    pub(crate) fn workflow_editor_kind_menu_open(&self, index: usize) -> bool {
        self.workflow_editor
            .as_ref()
            .is_some_and(|state| state.kind_menu_open == Some(index))
    }

    /// 收起类型下拉菜单（选中项后调用）。
    pub(crate) fn workflow_editor_close_kind_menu(&mut self) {
        if let Some(state) = self.workflow_editor.as_mut() {
            state.kind_menu_open = None;
        }
    }

    /// 展开/收起守卫策略下拉（互斥：同时只开一个；与类型菜单也互斥）。
    pub(crate) fn workflow_editor_toggle_guard_menu(&mut self, index: usize, on_exists: bool) {
        if let Some(state) = self.workflow_editor.as_mut() {
            state.kind_menu_open = None;
            let key = (index, on_exists);
            state.guard_menu_open = if state.guard_menu_open == Some(key) {
                None
            } else {
                Some(key)
            };
        }
    }

    /// 守卫策略下拉是否展开。
    /// 任一编辑器下拉（类型/守卫）是否打开（main.rs 的 any_popup_menu_open
    /// 引用，弹层打开期间分割线等交互门控与其它菜单一致）。
    pub(crate) fn workflow_editor_menu_open(&self) -> bool {
        self.workflow_editor
            .as_ref()
            .is_some_and(|state| state.kind_menu_open.is_some() || state.guard_menu_open.is_some())
    }

    /// 关闭编辑器的全部下拉菜单；返回是否有菜单被关闭（决定是否 notify）。
    pub(crate) fn workflow_editor_close_menus(&mut self) -> bool {
        let Some(state) = self.workflow_editor.as_mut() else {
            return false;
        };
        let had_open = state.kind_menu_open.is_some() || state.guard_menu_open.is_some();
        state.kind_menu_open = None;
        state.guard_menu_open = None;
        had_open
    }

    pub(crate) fn workflow_editor_guard_menu_open(&self, index: usize, on_exists: bool) -> bool {
        self.workflow_editor
            .as_ref()
            .is_some_and(|state| state.guard_menu_open == Some((index, on_exists)))
    }

    /// 收起全部下拉菜单（守卫策略选中后调用）。
    pub(crate) fn workflow_editor_close_guard_menu(&mut self) {
        if let Some(state) = self.workflow_editor.as_mut() {
            state.guard_menu_open = None;
        }
    }

    /// 切换第 index 个步骤的某个布尔参数。
    pub(crate) fn workflow_editor_toggle_step_flag(
        &mut self,
        index: usize,
        flag: WorkflowStepFlag,
    ) {
        if let Some(state) = self.workflow_editor.as_mut() {
            let Some(step) = state.data.steps.get_mut(index) else {
                return;
            };
            match flag {
                WorkflowStepFlag::CreateCheckout => step.checkout = !step.checkout,
                WorkflowStepFlag::PushSetUpstream => step.set_upstream = !step.set_upstream,
                WorkflowStepFlag::GuardFetch => step.guard_fetch = !step.guard_fetch,
                WorkflowStepFlag::FilterSkipCurrent => {
                    step.filter_skip_current = !step.filter_skip_current
                }
                WorkflowStepFlag::DeleteDryRun => step.delete_dry_run = !step.delete_dry_run,
                WorkflowStepFlag::DeleteSkipCurrent => {
                    step.delete_skip_current = !step.delete_skip_current
                }
            }
        }
    }

    /// 设置第 index 个步骤的守卫策略（on_exists/on_missing 二选一传入）。
    pub(crate) fn workflow_editor_set_guard_action(
        &mut self,
        index: usize,
        on_exists: Option<RemoteBranchGuardAction>,
        on_missing: Option<RemoteBranchGuardAction>,
    ) {
        if let Some(state) = self.workflow_editor.as_mut() {
            if let Some(step) = state.data.steps.get_mut(index) {
                if let Some(action) = on_exists {
                    step.on_exists = action;
                }
                if let Some(action) = on_missing {
                    step.on_missing = action;
                }
            }
        }
    }

    /// 添加一条空输入变量行。
    pub(crate) fn workflow_editor_add_input_row(&mut self) {
        if let Some(state) = self.workflow_editor.as_mut() {
            state.data.inputs.push(WorkflowEditorInputRowData {
                key: String::new(),
                label: String::new(),
                description: String::new(),
                default_value: String::new(),
                required: true,
            });
            state.ensure_field_capacity();
        }
    }

    /// 删除第 index 条输入变量行。
    pub(crate) fn workflow_editor_remove_input_row(&mut self, index: usize) {
        if let Some(state) = self.workflow_editor.as_mut() {
            if index < state.data.inputs.len() {
                state.data.inputs.remove(index);
                if index < state.input_fields.len() {
                    state.input_fields.remove(index);
                }
            }
        }
    }

    /// 切换第 index 条输入变量的必填开关。
    pub(crate) fn workflow_editor_toggle_input_required(&mut self, index: usize) {
        if let Some(state) = self.workflow_editor.as_mut() {
            if let Some(row) = state.data.inputs.get_mut(index) {
                row.required = !row.required;
            }
        }
    }

    /// 添加一条空自定义变量行。
    pub(crate) fn workflow_editor_add_var_row(&mut self) {
        if let Some(state) = self.workflow_editor.as_mut() {
            state.data.vars.push(WorkflowEditorVarRowData {
                key: String::new(),
                value: String::new(),
            });
            state.ensure_field_capacity();
        }
    }

    /// 删除第 index 条自定义变量行。
    pub(crate) fn workflow_editor_remove_var_row(&mut self, index: usize) {
        if let Some(state) = self.workflow_editor.as_mut() {
            if index < state.data.vars.len() {
                state.data.vars.remove(index);
                if index < state.var_fields.len() {
                    state.var_fields.remove(index);
                }
            }
        }
    }

    /// 渲染前确保编辑器当前需要展示的文本框已存在（惰性创建 + 预填）。
    /// 文本框创建需要 Context（focus_handle），而 field_mut 的调用点
    /// （text_input paint 闭包）没有 cx，因此初始化统一前移到渲染期。
    /// 向导第 2 步所有步骤卡片同屏渲染，全部步骤的槽文本框都要就位。
    pub(crate) fn ensure_workflow_editor_fields_inited(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let Some(state) = self.workflow_editor.as_mut() else {
            return;
        };
        // 全部步骤的槽文本框（卡片化后参数表单内联在每张卡片里）。
        for step_index in 0..state.data.steps.len() {
            let slots = state.data.steps[step_index].kind.slots().to_vec();
            for slot in slots {
                let _ = WorkflowEditorState::step_slot_field(state, step_index, slot, cx);
            }
        }
        // 变量设置步初始化所有变量行文本框
        if state.wizard_step == WIZARD_STEP_VARS {
            let inputs = state.data.inputs.clone();
            for (index, row) in inputs.iter().enumerate() {
                let Some(row_state) = state.input_fields.get_mut(index) else {
                    continue;
                };
                if row_state.key_field.is_none() {
                    row_state.key_field =
                        Some(TextFieldState::new(cx, "变量名").with_value(&row.key));
                }
                if row_state.label_field.is_none() {
                    row_state.label_field =
                        Some(TextFieldState::new(cx, "显示名").with_value(&row.label));
                }
                if row_state.description_field.is_none() {
                    row_state.description_field =
                        Some(TextFieldState::new(cx, "说明").with_value(&row.description));
                }
                if row_state.default_field.is_none() {
                    row_state.default_field =
                        Some(TextFieldState::new(cx, "默认值").with_value(&row.default_value));
                }
            }
            let vars = state.data.vars.clone();
            for (index, row) in vars.iter().enumerate() {
                let Some(row_state) = state.var_fields.get_mut(index) else {
                    continue;
                };
                if row_state.key_field.is_none() {
                    row_state.key_field =
                        Some(TextFieldState::new(cx, "变量名").with_value(&row.key));
                }
                if row_state.value_field.is_none() {
                    row_state.value_field =
                        Some(TextFieldState::new(cx, "值").with_value(&row.value));
                }
            }
        }
        let _ = window;
    }

    /// 编辑器字段的只读寻址：只返回已存在的文本框，不创建。
    /// 供 `field()`（渲染期读 placeholder/布局）与 `field_mut`（text_input
    /// paint 路径等）使用；渲染前经 `ensure_workflow_editor_fields_inited`
    /// 保证所需字段已初始化，编辑器已关闭时返回 None 由调用方兜底。
    pub(crate) fn workflow_editor_field_ref(
        &self,
        id: WorkflowEditorFieldId,
    ) -> Option<&TextFieldState> {
        let state = self.workflow_editor.as_ref()?;
        Some(match id {
            WorkflowEditorFieldId::Name => &state.name_field,
            WorkflowEditorFieldId::FileName => &state.file_name_field,
            WorkflowEditorFieldId::StepParam { step, slot } => {
                state.step_fields.get(step)?.fields.get(&slot)?
            }
            WorkflowEditorFieldId::InputPart { index, part } => {
                let row_state = state.input_fields.get(index)?;
                match part {
                    WorkflowInputPart::Key => row_state.key_field.as_ref()?,
                    WorkflowInputPart::Label => row_state.label_field.as_ref()?,
                    WorkflowInputPart::Description => row_state.description_field.as_ref()?,
                    WorkflowInputPart::Default => row_state.default_field.as_ref()?,
                }
            }
            WorkflowEditorFieldId::VarPart { index, key } => {
                let row_state = state.var_fields.get(index)?;
                if key {
                    row_state.key_field.as_ref()?
                } else {
                    row_state.value_field.as_ref()?
                }
            }
            WorkflowEditorFieldId::AiDescription => &state.ai_description_field,
            WorkflowEditorFieldId::PickerSearch => &state.picker_search_field,
        })
    }

    /// 编辑器字段的无 Context 可变寻址：只返回已存在的文本框，不创建。
    /// 供 `field_mut`（text_input paint 路径等）使用；渲染期经
    /// `ensure_workflow_editor_fields_inited` 保证所需字段已初始化。
    pub(crate) fn workflow_editor_field_ref_mut(
        &mut self,
        id: WorkflowEditorFieldId,
    ) -> Option<&mut TextFieldState> {
        let state = self.workflow_editor.as_mut()?;
        match id {
            WorkflowEditorFieldId::Name => Some(&mut state.name_field),
            WorkflowEditorFieldId::FileName => Some(&mut state.file_name_field),
            WorkflowEditorFieldId::StepParam { step, slot } => {
                state.step_fields.get_mut(step)?.fields.get_mut(&slot)
            }
            WorkflowEditorFieldId::InputPart { index, part } => {
                let row_state = state.input_fields.get_mut(index)?;
                match part {
                    WorkflowInputPart::Key => row_state.key_field.as_mut(),
                    WorkflowInputPart::Label => row_state.label_field.as_mut(),
                    WorkflowInputPart::Description => row_state.description_field.as_mut(),
                    WorkflowInputPart::Default => row_state.default_field.as_mut(),
                }
            }
            WorkflowEditorFieldId::VarPart { index, key } => {
                let row_state = state.var_fields.get_mut(index)?;
                if key {
                    row_state.key_field.as_mut()
                } else {
                    row_state.value_field.as_mut()
                }
            }
            WorkflowEditorFieldId::AiDescription => Some(&mut state.ai_description_field),
            WorkflowEditorFieldId::PickerSearch => Some(&mut state.picker_search_field),
        }
    }

    /// 编辑器字段的只读访问（`focused_field` 焦点探测用，不创建文本框）。
    pub(crate) fn workflow_editor_focused_field(&self, window: &Window) -> Option<FieldId> {
        let state = self.workflow_editor.as_ref()?;
        if state.name_field.focus.is_focused(window) {
            return Some(FieldId::WorkflowEditor(WorkflowEditorFieldId::Name));
        }
        if state.file_name_field.focus.is_focused(window) {
            return Some(FieldId::WorkflowEditor(WorkflowEditorFieldId::FileName));
        }
        for (step, step_state) in state.step_fields.iter().enumerate() {
            for (slot, field) in step_state.fields.iter() {
                if field.focus.is_focused(window) {
                    return Some(FieldId::WorkflowEditor(WorkflowEditorFieldId::StepParam {
                        step,
                        slot: *slot,
                    }));
                }
            }
        }
        for (index, row_state) in state.input_fields.iter().enumerate() {
            for (part, field) in [
                (WorkflowInputPart::Key, &row_state.key_field),
                (WorkflowInputPart::Label, &row_state.label_field),
                (WorkflowInputPart::Description, &row_state.description_field),
                (WorkflowInputPart::Default, &row_state.default_field),
            ] {
                if let Some(field) = field
                    && field.focus.is_focused(window)
                {
                    return Some(FieldId::WorkflowEditor(WorkflowEditorFieldId::InputPart {
                        index,
                        part,
                    }));
                }
            }
        }
        for (index, row_state) in state.var_fields.iter().enumerate() {
            for (key, field) in [
                (true, &row_state.key_field),
                (false, &row_state.value_field),
            ] {
                if let Some(field) = field
                    && field.focus.is_focused(window)
                {
                    return Some(FieldId::WorkflowEditor(WorkflowEditorFieldId::VarPart {
                        index,
                        key,
                    }));
                }
            }
        }
        if state.ai_description_field.focus.is_focused(window) {
            return Some(FieldId::WorkflowEditor(
                WorkflowEditorFieldId::AiDescription,
            ));
        }
        if state.picker_search_field.focus.is_focused(window) {
            return Some(FieldId::WorkflowEditor(WorkflowEditorFieldId::PickerSearch));
        }
        None
    }

    /// 文本框值变化回调：同步回纯数据层（预览/校验都读数据层）。
    pub(crate) fn workflow_editor_field_changed(&mut self, id: WorkflowEditorFieldId) {
        if let Some(state) = self.workflow_editor.as_mut() {
            state.sync_from_fields();
        }
        let _ = id;
    }

    /// AI 生成失败事件处理（由 main.rs 的 AiRequestFailed 分支调用）：
    /// 复位编辑器 loading 并把错误写进弹窗内错误条，用户可直接重试。
    pub(crate) fn handle_ai_workflow_template_failed(&mut self, error: &str) {
        if let Some(state) = self.workflow_editor.as_mut() {
            state.ai_loading = false;
            state.data.error = Some(format!("AI 生成失败：{error}"));
        }
    }

    /// 后台任务 panic 后的兜底复位（由 BackgroundTaskPanicked 调用）：
    /// ai_loading 是私有字段且失败路径还需要写错误条，统一走编辑器模块。
    pub(crate) fn reset_ai_loading_after_panic(&mut self) {
        if let Some(state) = self.workflow_editor.as_mut() {
            state.ai_loading = false;
        }
    }

    /// AI 生成完成事件处理：解析回填编辑器表单。
    /// 编辑器已关闭（用户在生成期间关掉弹窗）则静默丢弃结果；
    /// 解析失败把错误写进编辑器内错误条，弹窗不关闭、可立即重试。
    pub(crate) fn handle_ai_workflow_template_generated(
        &mut self,
        content: String,
        cx: &mut Context<Self>,
    ) {
        // 思考弹窗随完成自动关闭（编辑器弹窗仍开着，结果直接回填表单）。
        self.ai_thinking_overlay = None;
        let Some(state) = self.workflow_editor.as_mut() else {
            return;
        };
        state.ai_loading = false;
        // 以 AI 结果为准整体替换表单；仅保留语义字段（editing_path 等），
        // 由 apply_ai_generated_to_editor_data 处理。
        state.sync_from_fields();
        let mut data = state.data.clone();
        match apply_ai_generated_to_editor_data(&mut data, &content) {
            Ok(()) => {
                let has_vars = !data.inputs.is_empty() || !data.vars.is_empty();
                *state = WorkflowEditorState::from_data(data, cx);
                // AI 生成了变量：跳到「变量设置」步方便逐项检查（对应旧版
                // 自动展开高级区的行为）。
                if has_vars {
                    state.wizard_step = WIZARD_STEP_VARS;
                }
                self.status = "AI 已生成工作流模板，请检查后保存".to_string();
                self.notify_success("AI 已生成模板，请检查无误后保存", cx);
            }
            Err(err) => {
                state.data.error = Some(err);
                self.notify_error("AI 返回的内容无法解析为工作流模板", cx);
            }
        }
        cx.notify();
    }

    /// AI 生成/修改工作流模板：把用户需求描述（编辑模式附当前模板内容）
    /// 发给大模型，返回的 JSON5 经解析校验后回填当前编辑器表单。
    ///
    /// 照提交信息生成模式：独立 `ai_loading` 防重入（不借用全局 busy），
    /// `TaskKind::Long` 后台执行，成功发 `AiWorkflowTemplateGenerated`、
    /// 失败发共用 `AiRequestFailed`。保存仍走既有「保存模板」链路。
    pub(crate) fn generate_workflow_template_with_ai(&mut self, cx: &mut Context<Self>) {
        let Some(state) = self.workflow_editor.as_mut() else {
            return;
        };
        if state.ai_loading {
            return;
        }
        if !self.ai_settings.is_usable() {
            state.data.error = Some("请先在设置中心的 AI 设置中配置并启用供应商".into());
            cx.notify();
            return;
        }
        state.sync_from_fields();
        let request = state.ai_description_field.value.trim().to_string();
        if request.is_empty() {
            state.data.error = Some("请先在输入框中描述你想要的模板功能，再点击 AI 生成".into());
            cx.notify();
            return;
        }
        // 编辑模式携带当前模板内容作为上下文；序列化失败按无上下文降级
        // （新建语义从零生成），不阻断生成。
        let current_json5 = build_workflow_definition(&state.data)
            .ok()
            .and_then(|definition| json5::to_string(&definition).ok());

        state.ai_loading = true;
        // 清掉上一次的错误提示（新一轮生成开始）。
        state.data.error = None;

        // 公共思考弹窗执行器：思维链在弹窗内实时流式展示，完成后弹窗
        // 自动关闭并把 JSON5 回填编辑器表单。
        self.start_ai_thinking_task(
            "AI 正在生成工作流模板",
            |content| crate::UiEvent::AiWorkflowTemplateGenerated { content },
            move |client, _proxy_url, _tx, on_delta| {
                let (system, user) =
                    khaslana::workflow_template_prompts(&request, current_json5.as_deref());
                let result =
                    client.request_stream(&[system, user], &mut |delta| on_delta(delta))?;
                khaslana::ai::validate_generated_content(
                    &result,
                    "AI 返回的内容为空，请重试或检查模型配置",
                    "AI 未返回正文（仅返回了思考过程），请重试或更换模型",
                )?;
                Ok(result.content)
            },
        );
        cx.notify();
    }

    /// 保存模板：校验 -> 序列化 -> 回读校验 -> 写盘 -> 刷新列表。
    /// 校验失败把错误写入编辑器内展示，弹窗不关闭。
    ///
    /// 编辑模式（`editing_path` 存在）：文件名未变时原路径覆盖；改名后
    /// 写新路径且不删旧文件（避免误删用户数据）。重名校验排除自身，
    /// 否则未改名保存必误报「已存在同名模板」。
    pub(crate) fn save_workflow_editor(&mut self, cx: &mut Context<Self>) {
        let Some(state) = self.workflow_editor.as_mut() else {
            return;
        };
        state.sync_from_fields();
        let data = state.data.clone();

        let definition = match build_workflow_definition(&data) {
            Ok(definition) => definition,
            Err(err) => {
                if let Some(state) = self.workflow_editor.as_mut() {
                    state.data.error = Some(err);
                }
                cx.notify();
                return;
            }
        };
        // 重名校验排除编辑目标自身（大小写不敏感比对发生在 file_name 校验里）。
        let self_file_name = data
            .editing_path
            .as_ref()
            .and_then(|path| path.file_name())
            .and_then(|name| name.to_str())
            .map(ToOwned::to_owned);
        let existing = self
            .workflow_templates
            .iter()
            .filter_map(|template| {
                let name = template
                    .path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(ToOwned::to_owned)?;
                // 编辑模式下跳过自己，允许原地保存。
                (Some(name.to_uppercase()) != self_file_name.as_ref().map(|n| n.to_uppercase()))
                    .then_some(name)
            })
            .collect::<Vec<_>>();
        let file_name = match workflow_editor_file_name(&data.file_name, &existing) {
            Ok(name) => name,
            Err(err) => {
                if let Some(state) = self.workflow_editor.as_mut() {
                    state.data.error = Some(err);
                }
                cx.notify();
                return;
            }
        };
        let Some(dir) = workflow_templates_dir() else {
            if let Some(state) = self.workflow_editor.as_mut() {
                state.data.error = Some("无法定位工作流模板目录".to_string());
            }
            cx.notify();
            return;
        };

        // 序列化 -> 回读校验（round-trip 守卫，保证写盘文件必能被加载端解析）
        let serialized = match json5::to_string(&definition) {
            Ok(text) => text,
            Err(err) => {
                if let Some(state) = self.workflow_editor.as_mut() {
                    state.data.error = Some(format!("模板序列化失败：{err}"));
                }
                cx.notify();
                return;
            }
        };
        if let Err(err) = parse_workflow_json5(&serialized) {
            tracing::warn!("工作流模板序列化回读校验失败：{err}");
            if let Some(state) = self.workflow_editor.as_mut() {
                state.data.error = Some("模板序列化结果无法回读，已取消保存".to_string());
            }
            cx.notify();
            return;
        }

        // 写盘目标：编辑模式按文件主干比较（.jsonc 原件也原地覆盖，保留
        // 原扩展名）；主干不同（用户改名）写新路径并删除旧文件（重命名
        // 语义），否则目录里会残留旧模板、列表出现重复条目。
        let (path, stale_path) =
            workflow_editor_save_target(data.editing_path.as_deref(), &file_name);
        let path = if path.is_absolute() {
            path
        } else {
            dir.join(path)
        };
        if let Err(err) = fs::write(&path, serialized.as_bytes()) {
            if let Some(state) = self.workflow_editor.as_mut() {
                state.data.error = Some(format!("模板写入失败：{err}"));
            }
            cx.notify();
            return;
        }
        // 新文件写成功后才删旧的（删除失败不阻断保存，仅记日志——下次保存
        // 会再试；列表以新文件为准）。
        if let Some(stale) = &stale_path {
            if let Err(err) = fs::remove_file(stale) {
                tracing::warn!(
                    "工作流模板改名后清理旧文件失败（{}）：{err}",
                    stale.display()
                );
            }
        }

        // 成功：关闭编辑器、刷新模板目录、选中并加载新模板。
        self.pending_workflow_edit = None;
        self.workflow_editor = None;
        self.active_dialog = None;
        // 重新定位模板目录（active_data_dir 解析）+ 全量重扫，列表立即反映
        // 覆盖/改名结果。
        self.workflow_template_dir = workflow_templates_dir();
        self.refresh_workflow_templates();
        self.workflow_state.selected_template_path = Some(path.clone());
        let saved_label = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| file_name.clone());
        self.status = format!("工作流模板已保存：{saved_label}");
        if self.repo_path.is_some() {
            self.load_workflow_file(path, cx);
        }
        self.notify_success(format!("工作流模板已保存：{saved_label}"), cx);
    }
}

/// `field_mut` 的编辑器字段分支：寻址失败（编辑器已关闭的兜底路径）时
/// 回落到静态文本框。独立自由函数隔离两次可变借用，match 内分支返回后
/// 借用即结束。
pub(crate) fn workflow_editor_field_or_fallback<'a>(
    view: &'a mut RepositoryView,
    id: WorkflowEditorFieldId,
) -> &'a mut TextFieldState {
    // 借用隔离技巧：workflow_editor_field_ref_mut 的返回引用与 branch_name
    // 都基于 view，两段可变借用不能同时出现在 match 的两个分支里。
    // 通过先探测（不持有引用）再分路获取，让每条路径只有一次借用。
    let resolved = view.workflow_editor_field_ref_mut(id).is_some();
    if resolved {
        // 探测通过：编辑器打开且字段存在，直接取（Some 必然成立）。
        view.workflow_editor_field_ref_mut(id)
            .expect("编辑器字段探测通过后必然可寻址")
    } else {
        &mut view.branch_name
    }
}

// ---------------------------------------------------------------------------
// 渲染层
// ---------------------------------------------------------------------------

impl RepositoryView {
    /// 渲染「编辑带注释模板」确认弹窗：告知保存将丢失注释与排版。
    pub(crate) fn render_confirm_workflow_edit_comments(
        &self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let file_label = self
            .pending_workflow_edit
            .as_ref()
            .and_then(|pending| pending.path.file_name())
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "该模板".to_string());
        self.dialog_panel("编辑工作流模板", cx)
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::FOREGROUND))
                    .child(format!("模板 {file_label} 中包含手工写的注释。")),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child("可视化编辑器保存时会重新生成文件，原文件中的注释与排版将无法保留（内容本身不受影响）。"),
            )
            .child(
                dialog_actions()
                    .child(self.button("取消", true, |this, _, _| {
                        this.pending_workflow_edit = None;
                        this.active_dialog = None;
                    }, cx))
                    .child(self.primary_button(
                        "继续编辑",
                        true,
                        |this, _, cx| this.confirm_workflow_edit_comments(cx),
                        cx,
                    )),
            )
    }

    /// 渲染「删除工作流模板」确认弹窗（危险操作，danger 按钮样式）。
    pub(crate) fn render_confirm_delete_workflow_template_dialog(
        &self,
        path: std::path::PathBuf,
        display_name: String,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.dialog_panel("删除工作流模板", cx)
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::FOREGROUND))
                    .child(format!("确认删除模板「{display_name}」？")),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(ui_theme::COLOR_ERROR_FOREGROUND))
                    .child("删除后无法恢复，且不会影响仓库或远端。"),
            )
            .child(
                dialog_actions()
                    .child(self.button("取消", true, |this, _, _| this.close_dialog(), cx))
                    .child(self.danger_button(
                        "确认删除",
                        true,
                        move |this, _, cx| {
                            this.active_dialog = None;
                            this.delete_workflow_template(path.clone(), cx);
                        },
                        cx,
                    )),
            )
    }

    /// 渲染守卫策略下拉（存在/缺失 -> 停止/继续）。自绘 glass_menu 弹出层：
    /// 与步骤类型下拉同模式——deferred 延迟绘制避免被后续输入框遮挡，
    /// 紧凑行高与编辑器整体风格一致。
    fn render_workflow_editor_guard_select(
        &self,
        index: usize,
        on_exists: bool,
        current: RemoteBranchGuardAction,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let (label, id) = if on_exists {
            (
                "远端分支已存在时".to_string(),
                format!("workflow-editor-guard-exists-{index}"),
            )
        } else {
            (
                "远端分支不存在时".to_string(),
                format!("workflow-editor-guard-missing-{index}"),
            )
        };
        let current_label = match current {
            RemoteBranchGuardAction::Fail => "停止工作流",
            RemoteBranchGuardAction::Continue => "继续执行",
        };
        let menu_open = self.workflow_editor_guard_menu_open(index, on_exists);

        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(workflow_editor_field_label(&label, false))
            // 触发按钮 + 锚定弹出层：外层 relative，菜单 absolute top_full
            .child(
                div().relative().child(
                    div()
                        .id(id)
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .w_full()
                        .h(px(34.0))
                        .px_3()
                        .border_1()
                        .border_color(rgb(if menu_open {
                            ui_theme::PRIMARY
                        } else {
                            ui_theme::BORDER
                        }))
                        .rounded(px(ui_theme::RADIUS_XS))
                        .bg(rgb(ui_theme::CARD))
                        .cursor_pointer()
                        .hover(|this| this.bg(rgb(ui_theme::SECONDARY)))
                        // 同类型触发器：阻断 mouse_down 冒泡保住 toggle 语义。
                        .on_mouse_down(gpui::MouseButton::Left, |_event, _window, cx| {
                            cx.stop_propagation();
                        })
                        .on_click(cx.listener(move |this, _event, _window, cx| {
                            this.workflow_editor_toggle_guard_menu(index, on_exists);
                            cx.notify();
                        }))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(rgb(ui_theme::FOREGROUND))
                                .child(current_label),
                        )
                        .child(
                            div()
                                .text_size(px(10.0))
                                .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                                .child("▾"),
                        ),
                ),
            )
            .when(menu_open, |this| {
                // deferred + 高 priority：菜单延迟到最后绘制，盖住表单中排在
                // 其后的输入框（步骤类型下拉同模式）。
                let menu = glass_menu()
                    .absolute()
                    .top_full()
                    .left_0()
                    .mt(px(4.0))
                    .w_full()
                    .on_mouse_down(gpui::MouseButton::Left, |_ev, _window, cx| {
                        cx.stop_propagation();
                    })
                    .on_mouse_down(gpui::MouseButton::Right, |_ev, _window, cx| {
                        cx.stop_propagation();
                    })
                    .children(
                        [
                            (RemoteBranchGuardAction::Fail, "停止工作流"),
                            (RemoteBranchGuardAction::Continue, "继续执行"),
                        ]
                        .iter()
                        .map(|(action, action_label)| {
                            let selected = *action == current;
                            let action = *action;
                            div()
                                .id(format!(
                                    "workflow-editor-guard-{index}-{on_exists}-{}",
                                    if matches!(action, RemoteBranchGuardAction::Fail) {
                                        "fail"
                                    } else {
                                        "continue"
                                    }
                                ))
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap_2()
                                .px_3()
                                .py_1()
                                .text_size(px(12.0))
                                .text_color(rgb(if selected {
                                    ui_theme::PRIMARY
                                } else {
                                    ui_theme::FOREGROUND
                                }))
                                .bg(rgb(if selected {
                                    ui_theme::PRIMARY_SUBTLE
                                } else {
                                    ui_theme::CARD
                                }))
                                .cursor_pointer()
                                .hover(|this| this.bg(rgb(ui_theme::PRIMARY_SUBTLE)))
                                .on_click(cx.listener(move |this, _event, _window, cx| {
                                    cx.stop_propagation();
                                    this.workflow_editor_set_guard_action(
                                        index,
                                        on_exists.then_some(action),
                                        (!on_exists).then_some(action),
                                    );
                                    this.workflow_editor_close_guard_menu();
                                    cx.notify();
                                }))
                                .child(*action_label)
                                .when(selected, |this| {
                                    this.child(
                                        div()
                                            .text_size(px(11.0))
                                            .text_color(rgb(ui_theme::PRIMARY))
                                            .child("✓"),
                                    )
                                })
                                .into_any_element()
                        })
                        .collect::<Vec<_>>(),
                    );
                this.child(gpui::deferred(menu).with_priority(100))
            })
            .into_any_element()
    }
}

impl RepositoryView {
    /// 渲染「新建/编辑工作流模板」分步向导弹窗（宽面板，内容区滚动）。
    ///
    /// 四步向导（对齐设计稿）：基本信息 → 操作步骤（含添加步骤选择器）
    /// → 变量设置 → 完成（摘要 + JSON5 预览）。保存链路不变。
    pub(crate) fn render_workflow_editor_dialog(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let Some(editor) = self.workflow_editor.as_ref() else {
            return div().into_any_element();
        };
        let step = editor.wizard_step;
        let scroll_handle = self.scroll_handle("workflow-editor-scroll");

        let content = match step {
            0 => self.render_wizard_step_basic(window, cx),
            1 => self.render_wizard_step_steps(window, cx),
            2 => self.render_wizard_step_vars(window, cx),
            _ => self.render_wizard_step_review(),
        };

        div()
            .id("workflow-editor-panel")
            .w(px(880.0))
            .max_h(px(640.0))
            .rounded_sm()
            .border_1()
            .border_color(rgb(ui_theme::BORDER))
            .bg(rgb(ui_theme::CARD))
            .shadow_lg()
            .flex()
            .flex_col()
            .occlude()
            // 弹窗空白处点击收起步骤下拉（类型/守卫菜单）：模态遮罩挡住根层
            // capture，这些菜单无法走全局「点外部关闭」链，由弹窗壳层兜底；
            // 菜单本体已 stop_propagation，点击菜单项不会到达这里。
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _event, _window, cx| {
                    if this.workflow_editor_close_menus() {
                        cx.notify();
                    }
                    cx.stop_propagation();
                }),
            )
            // 顶栏：标题 + 副标题 + 关闭
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_between()
                    .px_4()
                    .pt_3()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(16.0))
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .text_color(rgb(ui_theme::FOREGROUND))
                                    .child(if editor.data.editing_path.is_some() {
                                        "编辑工作流模板"
                                    } else {
                                        "新建工作流模板"
                                    }),
                            )
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                                    .child("按引导分步完成，变量随步骤绑定，不会遗漏"),
                            ),
                    )
                    .child(self.primary_button(
                        "关闭",
                        true,
                        |this, _, _| this.close_workflow_editor(),
                        cx,
                    )),
            )
            // 步骤指示器 + 分隔线
            .child(self.render_wizard_indicator(step))
            .child(
                div()
                    .flex_none()
                    .w_full()
                    .h(px(1.0))
                    .bg(rgb(ui_theme::BORDER)),
            )
            // 内容区（滚动）
            .child(scrollable_frame_when(
                "workflow-editor-scroll",
                ScrollbarMode::Vertical,
                div()
                    .id("workflow-editor-scroll")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(0.0))
                    .p_4()
                    .overflow_y_scroll()
                    .track_scroll(&scroll_handle)
                    .child(content)
                    .into_any_element(),
                scroll_handle,
                true,
                cx,
            ))
            // 错误条（固定在底部导航上方，任何一步都可见）
            .when_some(editor.data.error.clone(), |this, error| {
                this.child(
                    div()
                        .mx_4()
                        .mb_2()
                        .flex_none()
                        .px_3()
                        .py_2()
                        .border_1()
                        .border_color(rgb(ui_theme::DESTRUCTIVE))
                        .rounded(px(ui_theme::RADIUS_XS))
                        .text_size(px(12.0))
                        .text_color(rgb(ui_theme::DESTRUCTIVE))
                        .child(error),
                )
            })
            // 底部导航
            .child(self.render_wizard_footer(cx))
            .into_any_element()
    }

    /// 向导步骤指示器：四枚 chip，当前步主色高亮（设计稿步骤条）。
    fn render_wizard_indicator(&self, current: usize) -> impl IntoElement {
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .px_4()
            .py_2()
            .children(
                WORKFLOW_WIZARD_STEPS
                    .iter()
                    .enumerate()
                    .map(|(index, label)| {
                        let active = index == current;
                        div()
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .px_2()
                            .py_1()
                            .rounded(px(ui_theme::RADIUS_XS))
                            .bg(rgb(if active {
                                ui_theme::PRIMARY
                            } else {
                                ui_theme::TILE
                            }))
                            .child(
                                div()
                                    .size(px(18.0))
                                    .rounded_full()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .bg(rgb(if active {
                                        ui_theme::PRIMARY_FOREGROUND
                                    } else {
                                        ui_theme::SECONDARY
                                    }))
                                    .text_size(px(10.0))
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .text_color(rgb(if active {
                                        ui_theme::PRIMARY
                                    } else {
                                        ui_theme::MUTED_FOREGROUND
                                    }))
                                    .child(format!("{}", index + 1)),
                            )
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(rgb(if active {
                                        ui_theme::PRIMARY_FOREGROUND
                                    } else {
                                        ui_theme::MUTED_FOREGROUND
                                    }))
                                    .child((*label).to_string()),
                            )
                    })
                    .collect::<Vec<_>>(),
            )
    }

    /// 底部导航：左「← 上一步」、中「下一步 → / 保存模板」、右步骤计数。
    fn render_wizard_footer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let step = self
            .workflow_editor
            .as_ref()
            .map(|editor| editor.wizard_step)
            .unwrap_or(0);
        let last = step + 1 >= WORKFLOW_WIZARD_STEPS.len();
        let left: gpui::AnyElement = if step == 0 {
            div().into_any_element()
        } else {
            self.button(
                "← 上一步",
                true,
                |this, _, cx| this.workflow_editor_prev_step(cx),
                cx,
            )
            .into_any_element()
        };
        let center: gpui::AnyElement = if last {
            self.primary_button(
                "保存模板",
                true,
                |this, _, cx| this.save_workflow_editor(cx),
                cx,
            )
            .into_any_element()
        } else {
            self.primary_button(
                "下一步 →",
                true,
                |this, _, cx| this.workflow_editor_next_step(cx),
                cx,
            )
            .into_any_element()
        };
        div()
            .flex()
            .flex_none()
            .items_center()
            .justify_between()
            .px_4()
            .py_3()
            .border_t_1()
            .border_color(rgb(ui_theme::BORDER))
            .child(left)
            .child(center)
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child(format!(
                        "第 {} 步 / 共 {} 步",
                        step + 1,
                        WORKFLOW_WIZARD_STEPS.len()
                    )),
            )
    }

    // ------------------------------------------------------------------
    // 第 1 步：基本信息
    // ------------------------------------------------------------------

    fn render_wizard_step_basic(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let editor = self.workflow_editor.as_ref().expect("编辑器状态缺失");
        let ai_ready = self.ai_settings.is_usable();
        let ai_enabled = ai_ready && !editor.ai_loading;
        let ai_label = if editor.ai_loading {
            "AI 生成中..."
        } else if ai_ready {
            "AI 生成"
        } else {
            "AI 生成（未配置）"
        };

        div()
            .flex()
            .gap_5()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap_3()
                    // AI 助手区块（普通底色 + 边框，不跟随主题色）
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .p_3()
                            .border_1()
                            .border_color(rgb(ui_theme::BORDER))
                            .rounded(px(ui_theme::RADIUS_MD))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_size(px(13.0))
                                            .text_color(rgb(ui_theme::PRIMARY))
                                            .child("✨"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(13.0))
                                            .font_weight(gpui::FontWeight::BOLD)
                                            .text_color(rgb(ui_theme::PRIMARY))
                                            .child("AI 助手"),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                                    .child(match editor.data.editing_path.is_some() {
                                        true => "描述修改需求，AI 会基于当前模板内容调整步骤与变量。",
                                        false => "用一句话描述你想要的工作流，AI 生成步骤与变量并自动填入后续各步，你只需检查和微调。",
                                    }),
                            )
                            .child(self.input(
                                FieldId::WorkflowEditor(WorkflowEditorFieldId::AiDescription),
                                false,
                                window,
                                cx,
                            ))
                            .child(
                                div()
                                    .flex()
                                    .justify_end()
                                    .child(self.button(ai_label, ai_enabled, |this, _, cx| {
                                        this.generate_workflow_template_with_ai(cx);
                                    }, cx)),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(workflow_editor_field_label("模板名称（可选）", false))
                            .child(self.input(
                                FieldId::WorkflowEditor(WorkflowEditorFieldId::Name),
                                false,
                                window,
                                cx,
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(workflow_editor_field_label("保存文件名（.json5）", true))
                            .child(self.input(
                                FieldId::WorkflowEditor(WorkflowEditorFieldId::FileName),
                                false,
                                window,
                                cx,
                            )),
                    )
                    .child(self.toggle_row(
                        "workflow-editor-require-clean",
                        "运行前要求工作区干净（推荐）",
                        editor.data.require_clean_worktree,
                        |this, _, _| {
                            if let Some(state) = this.workflow_editor.as_mut() {
                                state.data.require_clean_worktree =
                                    !state.data.require_clean_worktree;
                            }
                        },
                        cx,
                    )),
            )
            .child(workflow_guide_card(
                "这一步要做什么",
                "填写模板的显示名称和保存文件名。名称显示在工作流列表里；文件名是保存到模板目录时的名字（不含扩展名）。",
            ))
            .into_any_element()
    }

    // ------------------------------------------------------------------
    // 第 2 步：操作步骤（卡片列表 / 添加步骤选择器两个视图）
    // ------------------------------------------------------------------

    fn render_wizard_step_steps(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let editor = self.workflow_editor.as_ref().expect("编辑器状态缺失");
        if editor.step_picker_open {
            return self.render_wizard_step_picker(window, cx);
        }
        let data = &editor.data;

        let mut list = div().flex().flex_col().flex_1().min_w_0().gap_2();
        if data.steps.is_empty() {
            list = list.child(self.render_wizard_presets(cx));
        } else {
            for index in 0..data.steps.len() {
                list = list.child(self.render_workflow_step_card(index, window, cx));
            }
        }
        list = list.child(
            div()
                .id("workflow-editor-add-step")
                .flex()
                .items_center()
                .justify_center()
                .w_full()
                .h(px(32.0))
                .border_1()
                .border_color(rgb(ui_theme::BORDER))
                .rounded(px(ui_theme::RADIUS_XS))
                .cursor_pointer()
                .text_size(px(12.0))
                .text_color(rgb(ui_theme::PRIMARY))
                .hover(|this| this.bg(rgb(ui_theme::ACCENT)))
                .on_click(cx.listener(|this, _event, _window, cx| {
                    this.workflow_editor_open_step_picker();
                    cx.notify();
                }))
                .child("+ 添加步骤"),
        );

        div()
            .flex()
            .gap_5()
            .child(list)
            .child(workflow_guide_card(
                "操作步骤",
                "每个步骤按顺序执行。参数直接填在卡片里，${变量} 会标出引用情况；用 ↑ ↓ 调整顺序，点 ✕ 删除。",
            ))
            .into_any_element()
    }

    /// 添加步骤选择器视图：搜索框 + 常用/高级两组选项卡网格。
    fn render_wizard_step_picker(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let editor = self.workflow_editor.as_ref().expect("编辑器状态缺失");
        let query = editor.picker_search_field.value.clone();
        let kinds = filter_workflow_step_kinds(&query);
        let split = WorkflowStepKind::COMMON_COUNT.min(kinds.len());

        let mut panel = div().flex().flex_col().flex_1().min_w_0().gap_3().child(
            div()
                .flex()
                .items_end()
                .justify_between()
                .gap_2()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .text_size(px(14.0))
                                .font_weight(gpui::FontWeight::BOLD)
                                .text_color(rgb(ui_theme::FOREGROUND))
                                .child("添加操作步骤"),
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                                .child("选择要加入工作流的 Git 操作，参数稍后在卡片里填写"),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(div().w(px(200.0)).child(self.input(
                            FieldId::WorkflowEditor(WorkflowEditorFieldId::PickerSearch),
                            true,
                            window,
                            cx,
                        )))
                        .child(self.button(
                            "返回列表",
                            true,
                            |this, _, _| this.workflow_editor_close_step_picker(),
                            cx,
                        )),
                ),
        );

        if kinds.is_empty() {
            panel = panel.child(placeholder_row("没有匹配的操作"));
        } else {
            let groups: [(&str, &[WorkflowStepKind]); 2] =
                [("常用", &kinds[..split]), ("高级", &kinds[split..])];
            for (label, group_kinds) in groups {
                if group_kinds.is_empty() {
                    continue;
                }
                let mut group = div().flex().flex_col().gap_1p5().child(
                    div()
                        .text_size(px(11.0))
                        .font_weight(gpui::FontWeight::BOLD)
                        .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                        .child(label.to_string()),
                );
                for pair in group_kinds.chunks(2) {
                    group = group.child(
                        div().flex().gap_2().children(
                            pair.iter()
                                .map(|kind| self.render_step_kind_option(*kind, cx))
                                .collect::<Vec<_>>(),
                        ),
                    );
                }
                panel = panel.child(group);
            }
        }
        panel = panel.child(
            div()
                .text_size(px(11.0))
                .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                .child("点击某个操作即可添加为工作流步骤，并回到步骤列表"),
        );

        div()
            .flex()
            .gap_5()
            .child(panel)
            .child(workflow_guide_card(
                "选择一个操作",
                "常用操作覆盖日常分支协作；高级操作用于守卫与批量清理。加入后会自动选中新步骤，继续点「+ 添加步骤」可追加多个。",
            ))
            .into_any_element()
    }

    /// 选择器里的单个操作选项卡。
    fn render_step_kind_option(
        &self,
        kind: WorkflowStepKind,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        div()
            .id(format!("workflow-editor-picker-{}", kind.op_name()))
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .gap_1()
            .p(px(10.0))
            .border_1()
            .border_color(rgb(ui_theme::BORDER))
            .rounded(px(ui_theme::RADIUS_MD))
            .cursor_pointer()
            .hover(|this| {
                this.border_color(rgb(ui_theme::PRIMARY))
                    .bg(rgb(ui_theme::TILE))
            })
            .on_click(cx.listener(move |this, _event, _window, cx| {
                cx.stop_propagation();
                this.workflow_editor_add_step_of_kind(kind);
                cx.notify();
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .size(px(24.0))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(ui_theme::RADIUS_XS))
                            .bg(rgb(ui_theme::PRIMARY_SUBTLE))
                            .text_size(px(13.0))
                            .text_color(rgb(ui_theme::PRIMARY))
                            .child(workflow_step_kind_glyph(kind)),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(rgb(ui_theme::FOREGROUND))
                            .child(kind.display_name().to_string()),
                    )
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                            .child(kind.op_name().to_string()),
                    ),
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child(kind.description().to_string()),
            )
            .into_any_element()
    }

    /// 第 2 步的单张步骤卡：序号徽标 + 类型下拉 + ↑↓✕ + 内联参数 + 绑定条。
    fn render_workflow_step_card(
        &self,
        index: usize,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let editor = self.workflow_editor.as_ref().expect("编辑器状态缺失");
        let Some(step) = editor.data.steps.get(index) else {
            return div().into_any_element();
        };
        let kind = step.kind;
        let menu_id = format!("workflow-editor-step-kind-{index}");
        let menu_open = self.workflow_editor_kind_menu_open(index);

        // 类型下拉：触发器与锚定弹出层同处一个 relative 包裹
        // （deferred + 高优先级绘制，避免被后续输入框遮挡）。
        let mut kind_dropdown = div().relative().child(
            div()
                .id(menu_id.clone())
                .flex()
                .items_center()
                .gap_1()
                .px_2()
                .h(px(24.0))
                .rounded(px(ui_theme::RADIUS_XS))
                .cursor_pointer()
                .hover(|this| this.bg(rgb(ui_theme::SECONDARY)))
                // 阻断 mouse_down 冒泡：否则弹窗壳层的「收起菜单」先执行，
                // 随后的 toggle 又把菜单打开，触发器永远关不掉菜单。
                .on_mouse_down(gpui::MouseButton::Left, |_event, _window, cx| {
                    cx.stop_propagation();
                })
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    this.workflow_editor_toggle_kind_menu(index);
                    cx.notify();
                }))
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(rgb(ui_theme::FOREGROUND))
                        .child(kind.display_name().to_string()),
                )
                .child(
                    div()
                        .text_size(px(9.0))
                        .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                        .child("▾"),
                ),
        );
        if menu_open {
            // 滚动 id 按 index 惰性生成一次并缓存（scrollable_frame_when 要求
            // 'static id）：这里每帧执行，直接 Box::leak 会让开着菜单的每一帧
            // 都泄漏一个新字符串、无限增长。
            let menu_scroll_id = workflow_step_menu_scroll_id(index);
            let handle = self.scroll_handle(menu_scroll_id);
            kind_dropdown = kind_dropdown.child(
                gpui::deferred(
                    glass_menu()
                        .absolute()
                        .top_full()
                        .left_0()
                        .mt(px(4.0))
                        .w(px(240.0))
                        .max_h(px(240.0))
                        .on_mouse_down(gpui::MouseButton::Left, |_ev, _window, cx| {
                            cx.stop_propagation();
                        })
                        .on_mouse_down(gpui::MouseButton::Right, |_ev, _window, cx| {
                            cx.stop_propagation();
                        })
                        .child(scrollable_frame_when(
                            menu_scroll_id,
                            ScrollbarMode::Vertical,
                            div()
                                .id(format!("{menu_id}-menu"))
                                .flex()
                                .flex_col()
                                .max_h(px(240.0))
                                .overflow_y_scroll()
                                .track_scroll(&handle)
                                .children(render_workflow_kind_menu_items(
                                    index, kind, &menu_id, cx,
                                ))
                                .into_any_element(),
                            handle,
                            true,
                            cx,
                        )),
                )
                .with_priority(100),
            );
        }

        let mut card = div()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .border_1()
            .border_color(rgb(ui_theme::BORDER))
            .rounded(px(ui_theme::RADIUS_MD))
            .bg(rgb(ui_theme::CARD))
            // 卡头：序号 + 类型下拉 + 排序/删除
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .w_full()
                    .child(
                        div()
                            .size(px(22.0))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .bg(rgb(ui_theme::PRIMARY))
                            .text_size(px(11.0))
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(rgb(ui_theme::PRIMARY_FOREGROUND))
                            .child(format!("{}", index + 1)),
                    )
                    .child(
                        // 类型下拉触发器 + 锚定弹出层（同 relative 包裹）
                        kind_dropdown,
                    )
                    .child(div().flex_1())
                    .child(workflow_editor_step_row_actions(index, cx)),
            );

        // 参数槽（内联在卡片里）
        for slot in kind.slots() {
            card = card.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(workflow_editor_field_label(
                        slot.label(),
                        kind.slot_required(*slot),
                    ))
                    .child(self.input(
                        FieldId::WorkflowEditor(WorkflowEditorFieldId::StepParam {
                            step: index,
                            slot: *slot,
                        }),
                        false,
                        window,
                        cx,
                    )),
            );
        }

        // 类型相关开关与守卫策略（沿用原表单语义）
        match kind {
            WorkflowStepKind::CreateBranch => {
                card = card.child(self.toggle_row(
                    "workflow-editor-create-checkout",
                    "创建后切换到新分支",
                    step.checkout,
                    move |this, _, _| {
                        this.workflow_editor_toggle_step_flag(
                            index,
                            WorkflowStepFlag::CreateCheckout,
                        )
                    },
                    cx,
                ));
            }
            WorkflowStepKind::Push => {
                card = card.child(self.toggle_row(
                    "workflow-editor-push-upstream",
                    "推送时建立上游跟踪",
                    step.set_upstream,
                    move |this, _, _| {
                        this.workflow_editor_toggle_step_flag(
                            index,
                            WorkflowStepFlag::PushSetUpstream,
                        )
                    },
                    cx,
                ));
            }
            WorkflowStepKind::GuardRemoteBranch => {
                card = card
                    .child(self.toggle_row(
                        "workflow-editor-guard-fetch",
                        "检查前先从远端获取（刷新引用）",
                        step.guard_fetch,
                        move |this, _, _| {
                            this.workflow_editor_toggle_step_flag(
                                index,
                                WorkflowStepFlag::GuardFetch,
                            )
                        },
                        cx,
                    ))
                    .child(self.render_workflow_editor_guard_select(
                        index,
                        true,
                        step.on_exists,
                        cx,
                    ))
                    .child(self.render_workflow_editor_guard_select(
                        index,
                        false,
                        step.on_missing,
                        cx,
                    ));
            }
            WorkflowStepKind::FilterBranches => {
                card = card.child(self.toggle_row(
                    "workflow-editor-filter-skip-current",
                    "排除当前分支",
                    step.filter_skip_current,
                    move |this, _, _| {
                        this.workflow_editor_toggle_step_flag(
                            index,
                            WorkflowStepFlag::FilterSkipCurrent,
                        )
                    },
                    cx,
                ));
            }
            WorkflowStepKind::DeleteBranches => {
                card = card
                    .child(self.toggle_row(
                        "workflow-editor-delete-dry-run",
                        "试运行（只列出将删除的分支，不真正删除）",
                        step.delete_dry_run,
                        move |this, _, _| {
                            this.workflow_editor_toggle_step_flag(
                                index,
                                WorkflowStepFlag::DeleteDryRun,
                            )
                        },
                        cx,
                    ))
                    .child(self.toggle_row(
                        "workflow-editor-delete-skip-current",
                        "自动跳过当前分支",
                        step.delete_skip_current,
                        move |this, _, _| {
                            this.workflow_editor_toggle_step_flag(
                                index,
                                WorkflowStepFlag::DeleteSkipCurrent,
                            )
                        },
                        cx,
                    ));
            }
            WorkflowStepKind::EnsureClean => {
                card = card.child(
                    div()
                        .text_size(px(11.0))
                        .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                        .child("该步骤无需参数：运行到此步时检查工作区，有未提交改动则停止。"),
                );
            }
            _ => {}
        }

        // 绑定条：变量使用情况（设计稿卡片底部 accent 条）
        let input_keys = editor
            .data
            .inputs
            .iter()
            .map(|row| row.key.trim().to_string())
            .filter(|key| !key.is_empty())
            .collect::<Vec<_>>();
        let var_keys = editor
            .data
            .vars
            .iter()
            .map(|row| row.key.trim().to_string())
            .filter(|key| !key.is_empty())
            .collect::<Vec<_>>();
        let binding = workflow_step_binding(index, &editor.data.steps, &input_keys, &var_keys);
        let summary = workflow_binding_summary(&binding);
        let has_unknown = !binding.unknown.is_empty();
        let (strip_bg, strip_fg) = if has_unknown {
            (ui_theme::COLOR_WARNING, ui_theme::COLOR_WARNING_FOREGROUND)
        } else if binding.used.is_empty() && binding.produces.is_some() {
            (ui_theme::COLOR_SUCCESS, ui_theme::COLOR_SUCCESS_FOREGROUND)
        } else if !binding.used.is_empty() {
            (ui_theme::PRIMARY_SUBTLE, ui_theme::PRIMARY)
        } else {
            (ui_theme::TILE, ui_theme::MUTED_FOREGROUND)
        };
        card = card.child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .px_2()
                .py_1()
                .rounded(px(ui_theme::RADIUS_XS))
                .bg(rgb(strip_bg))
                .child(
                    div()
                        .text_size(px(11.0))
                        .font_weight(gpui::FontWeight::BOLD)
                        .text_color(rgb(strip_fg))
                        .child("$"),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(11.0))
                        .text_color(rgb(strip_fg))
                        .child(summary),
                )
                .when(has_unknown, |this| {
                    this.child(
                        div()
                            .id(format!("workflow-editor-binding-goto-{index}"))
                            .flex_none()
                            .px_2()
                            .py(px(2.0))
                            .rounded(px(ui_theme::RADIUS_XS))
                            .cursor_pointer()
                            .text_size(px(10.0))
                            .bg(rgb(strip_fg))
                            .text_color(rgb(strip_bg))
                            .on_click(cx.listener(move |this, _event, _window, cx| {
                                cx.stop_propagation();
                                this.workflow_editor_jump_to_vars(cx);
                            }))
                            .child("去定义"),
                    )
                }),
        );

        card.into_any_element()
    }

    /// 第 2 步空列表时的预设模板卡片区。
    fn render_wizard_presets(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(section_title("从常用预设开始（可选）"))
            .child(
                div().flex().flex_wrap().gap_2().children(
                    WORKFLOW_EDITOR_PRESETS
                        .iter()
                        .map(|preset| {
                            let preset = *preset;
                            div()
                                .id(format!("workflow-editor-preset-{:?}", preset))
                                .flex()
                                .flex_col()
                                .gap_1()
                                .min_w(px(240.0))
                                .flex_1()
                                .p_3()
                                .border_1()
                                .border_color(rgb(ui_theme::BORDER))
                                .rounded_sm()
                                .cursor_pointer()
                                .hover(|this| this.bg(rgb(ui_theme::ACCENT)))
                                .on_click(cx.listener(move |this, _event, _window, cx| {
                                    this.apply_workflow_editor_preset(preset, cx);
                                    cx.notify();
                                }))
                                .child(
                                    div()
                                        .text_size(px(12.0))
                                        .font_weight(gpui::FontWeight::BOLD)
                                        .text_color(rgb(ui_theme::PRIMARY))
                                        .child(preset.title()),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.0))
                                        .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                                        .child(preset.description()),
                                )
                                .into_any_element()
                        })
                        .collect::<Vec<_>>(),
                ),
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child("也可以跳过预设，点下方「+ 添加步骤」从零开始。"),
            )
            .into_any_element()
    }

    // ------------------------------------------------------------------
    // 第 3 步：变量设置
    // ------------------------------------------------------------------

    fn render_wizard_step_vars(&self, window: &Window, cx: &mut Context<Self>) -> gpui::AnyElement {
        let editor = self.workflow_editor.as_ref().expect("编辑器状态缺失");
        let data = &editor.data;

        let mut inputs_group = div()
            .flex()
            .flex_col()
            .gap_1p5()
            .child(
                div()
                    .text_size(px(12.0))
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(rgb(ui_theme::FOREGROUND))
                    .child("运行前用户填写的变量（inputs）"),
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child("运行工作流前在页面填写"),
            );
        if data.inputs.is_empty() {
            inputs_group = inputs_group.child(placeholder_row("暂无输入变量"));
        } else {
            for index in 0..data.inputs.len() {
                inputs_group =
                    inputs_group.child(self.render_workflow_input_card(index, window, cx));
            }
        }
        inputs_group = inputs_group.child(workflow_editor_add_button(
            "workflow-editor-add-input",
            "+ 添加输入变量",
            |this| this.workflow_editor_add_input_row(),
            cx,
        ));

        let mut vars_group = div()
            .flex()
            .flex_col()
            .gap_1p5()
            .child(
                div()
                    .text_size(px(12.0))
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(rgb(ui_theme::FOREGROUND))
                    .child("自定义变量（vars）"),
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child("工作流内部使用的固定值，可写 ${...} 表达式"),
            );
        if data.vars.is_empty() {
            vars_group = vars_group.child(placeholder_row("暂无自定义变量"));
        } else {
            for index in 0..data.vars.len() {
                vars_group = vars_group.child(self.render_workflow_var_card(index, window, cx));
            }
        }
        vars_group = vars_group.child(workflow_editor_add_button(
            "workflow-editor-add-var",
            "+ 添加自定义变量",
            |this| this.workflow_editor_add_var_row(),
            cx,
        ));

        div()
            .flex()
            .gap_5()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap_4()
                    .child(inputs_group)
                    .child(vars_group),
            )
            .child(workflow_guide_card(
                "核对变量绑定",
                "这里统一管理所有 ${变量}。每个变量卡片会标出被哪些步骤引用；步骤卡片里标红的未声明变量，在这里补一个同名变量即可。",
            ))
            .into_any_element()
    }

    /// 输入变量卡片：键徽标 + 引用标签 + 四个字段 + 必填开关 + 删除。
    fn render_workflow_input_card(
        &self,
        index: usize,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let editor = self.workflow_editor.as_ref().expect("编辑器状态缺失");
        let Some(row) = editor.data.inputs.get(index) else {
            return div().into_any_element();
        };
        let key_live = self
            .workflow_editor_field_ref(WorkflowEditorFieldId::InputPart {
                index,
                part: WorkflowInputPart::Key,
            })
            .map(|field| field.value.clone())
            .unwrap_or_else(|| row.key.clone());
        let trimmed_key = key_live.trim().to_string();
        let referenced = if trimmed_key.is_empty() {
            Vec::new()
        } else {
            workflow_var_referenced_by_steps(&trimmed_key, &editor.data.steps)
        };

        div()
            .flex()
            .flex_col()
            .gap_1p5()
            .p_2p5()
            .border_1()
            .border_color(rgb(ui_theme::BORDER))
            .rounded(px(ui_theme::RADIUS_MD))
            // 卡头：键徽标 + 引用标签 + 删除
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .px_2()
                            .py(px(2.0))
                            .rounded(px(ui_theme::RADIUS_XS))
                            .bg(rgb(ui_theme::PRIMARY_SUBTLE))
                            .text_size(px(11.0))
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(rgb(ui_theme::PRIMARY))
                            .child(if trimmed_key.is_empty() {
                                "(未命名)".to_string()
                            } else {
                                trimmed_key.clone()
                            }),
                    )
                    .child(div().flex_1().min_w_0())
                    .when(!referenced.is_empty(), |this| {
                        this.child(
                            div()
                                .text_size(px(10.0))
                                .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                                .child(format!(
                                    "→ 被步骤 {} 引用",
                                    referenced
                                        .iter()
                                        .map(|step_no| step_no.to_string())
                                        .collect::<Vec<_>>()
                                        .join("、")
                                )),
                        )
                    })
                    .child(workflow_editor_card_remove_button(
                        format!("workflow-editor-remove-input-{index}"),
                        move |this, cx| {
                            this.workflow_editor_remove_input_row(index);
                            cx.notify();
                        },
                        cx,
                    )),
            )
            // 字段两列排布
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(field_column(
                        "变量名",
                        true,
                        self.input(
                            FieldId::WorkflowEditor(WorkflowEditorFieldId::InputPart {
                                index,
                                part: WorkflowInputPart::Key,
                            }),
                            false,
                            window,
                            cx,
                        ),
                    ))
                    .child(field_column(
                        "显示名（可选）",
                        false,
                        self.input(
                            FieldId::WorkflowEditor(WorkflowEditorFieldId::InputPart {
                                index,
                                part: WorkflowInputPart::Label,
                            }),
                            false,
                            window,
                            cx,
                        ),
                    )),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(field_column(
                        "说明（可选，运行时展示）",
                        false,
                        self.input(
                            FieldId::WorkflowEditor(WorkflowEditorFieldId::InputPart {
                                index,
                                part: WorkflowInputPart::Description,
                            }),
                            false,
                            window,
                            cx,
                        ),
                    ))
                    .child(field_column(
                        "默认值（可选）",
                        false,
                        self.input(
                            FieldId::WorkflowEditor(WorkflowEditorFieldId::InputPart {
                                index,
                                part: WorkflowInputPart::Default,
                            }),
                            false,
                            window,
                            cx,
                        ),
                    )),
            )
            .child(self.toggle_row(
                "workflow-editor-input-required",
                "必填",
                row.required,
                move |this, _, _| this.workflow_editor_toggle_input_required(index),
                cx,
            ))
            .into_any_element()
    }

    /// 自定义变量卡片：键徽标 + 引用标签 + 变量名/值字段 + 删除。
    fn render_workflow_var_card(
        &self,
        index: usize,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let editor = self.workflow_editor.as_ref().expect("编辑器状态缺失");
        let Some(row) = editor.data.vars.get(index) else {
            return div().into_any_element();
        };
        let key_live = self
            .workflow_editor_field_ref(WorkflowEditorFieldId::VarPart { index, key: true })
            .map(|field| field.value.clone())
            .unwrap_or_else(|| row.key.clone());
        let trimmed_key = key_live.trim().to_string();
        let referenced = if trimmed_key.is_empty() {
            Vec::new()
        } else {
            workflow_var_referenced_by_steps(&trimmed_key, &editor.data.steps)
        };

        div()
            .flex()
            .flex_col()
            .gap_1p5()
            .p_2p5()
            .border_1()
            .border_color(rgb(ui_theme::BORDER))
            .rounded(px(ui_theme::RADIUS_MD))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .px_2()
                            .py(px(2.0))
                            .rounded(px(ui_theme::RADIUS_XS))
                            .bg(rgb(ui_theme::PRIMARY_SUBTLE))
                            .text_size(px(11.0))
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(rgb(ui_theme::PRIMARY))
                            .child(if trimmed_key.is_empty() {
                                "(未命名)".to_string()
                            } else {
                                trimmed_key.clone()
                            }),
                    )
                    .child(div().flex_1().min_w_0())
                    .when(!referenced.is_empty(), |this| {
                        this.child(
                            div()
                                .text_size(px(10.0))
                                .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                                .child(format!(
                                    "→ 被步骤 {} 引用",
                                    referenced
                                        .iter()
                                        .map(|step_no| step_no.to_string())
                                        .collect::<Vec<_>>()
                                        .join("、")
                                )),
                        )
                    })
                    .child(workflow_editor_card_remove_button(
                        format!("workflow-editor-remove-var-{index}"),
                        move |this, cx| {
                            this.workflow_editor_remove_var_row(index);
                            cx.notify();
                        },
                        cx,
                    )),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(field_column(
                        "变量名",
                        true,
                        self.input(
                            FieldId::WorkflowEditor(WorkflowEditorFieldId::VarPart {
                                index,
                                key: true,
                            }),
                            false,
                            window,
                            cx,
                        ),
                    ))
                    .child(field_column(
                        "值",
                        false,
                        self.input(
                            FieldId::WorkflowEditor(WorkflowEditorFieldId::VarPart {
                                index,
                                key: false,
                            }),
                            false,
                            window,
                            cx,
                        ),
                    )),
            )
            .into_any_element()
    }

    // ------------------------------------------------------------------
    // 第 4 步：完成（摘要 + JSON5 预览）
    // ------------------------------------------------------------------

    fn render_wizard_step_review(&self) -> gpui::AnyElement {
        let editor = self.workflow_editor.as_ref().expect("编辑器状态缺失");
        let data = &editor.data;

        let name = data.name.trim();
        let op_names = data
            .steps
            .iter()
            .map(|step| step.kind.display_name().to_string())
            .collect::<Vec<_>>();
        let steps_value = if data.steps.is_empty() {
            "尚未添加步骤".to_string()
        } else {
            format!("共 {} 步（{}）", data.steps.len(), op_names.join("、"))
        };
        let input_names = data
            .inputs
            .iter()
            .map(|row| row.key.trim().to_string())
            .filter(|key| !key.is_empty())
            .collect::<Vec<_>>();
        let var_names = data
            .vars
            .iter()
            .map(|row| row.key.trim().to_string())
            .filter(|key| !key.is_empty())
            .collect::<Vec<_>>();
        let inputs_value = if input_names.is_empty() {
            "无".to_string()
        } else {
            format!("{} 个（{}）", input_names.len(), input_names.join("、"))
        };
        let vars_value = if var_names.is_empty() {
            "无".to_string()
        } else {
            format!("{} 个（{}）", var_names.len(), var_names.join("、"))
        };
        let save_label = data
            .editing_path
            .as_ref()
            .and_then(|path| path.file_name())
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| format!("{}.json5", data.file_name.trim()));

        // JSON5 预览：合法时输出 pretty JSON（同为合法 JSON5）；否则给占位说明。
        let preview = build_workflow_definition(data)
            .ok()
            .and_then(|definition| serde_json::to_string_pretty(&definition).ok())
            .unwrap_or_else(|| {
                "// 当前表单还缺必填内容，暂不能生成完整模板。\n// 保存时会给出具体提示。"
                    .to_string()
            });

        let summary_rows = [
            (
                "模板名称",
                if name.is_empty() {
                    "(未填写)".to_string()
                } else {
                    name.to_string()
                },
            ),
            ("步骤数", steps_value),
            ("输入变量", inputs_value),
            ("自定义变量", vars_value),
            (
                "运行要求",
                if data.require_clean_worktree {
                    "工作区干净".to_string()
                } else {
                    "允许脏工作区".to_string()
                },
            ),
        ];

        div()
            .flex()
            .gap_5()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .w(px(340.0))
                    .flex_none()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .p_3()
                            .border_1()
                            .border_color(rgb(ui_theme::BORDER))
                            .rounded(px(ui_theme::RADIUS_MD))
                            .child(
                                div()
                                    .text_size(px(13.0))
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .text_color(rgb(ui_theme::FOREGROUND))
                                    .child("模板摘要"),
                            )
                            .children(summary_rows.into_iter().map(|(label, value)| {
                                div()
                                    .flex()
                                    .gap_2()
                                    .child(
                                        div()
                                            .w(px(76.0))
                                            .flex_none()
                                            .text_size(px(11.0))
                                            .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                                            .child(label.to_string()),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .text_size(px(11.0))
                                            .text_color(rgb(ui_theme::FOREGROUND))
                                            .child(value),
                                    )
                            })),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .p_2()
                            .rounded(px(ui_theme::RADIUS_XS))
                            .bg(rgb(ui_theme::TILE))
                            .child(
                                div()
                                    .size(px(18.0))
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_full()
                                    .bg(rgb(ui_theme::PRIMARY_SUBTLE))
                                    .text_size(px(10.0))
                                    .text_color(rgb(ui_theme::PRIMARY))
                                    .child("⬇"),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_size(px(11.0))
                                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                                    .child(format!("将保存到工作流模板目录：{save_label}")),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap_1p5()
                    .child(
                        div()
                            .text_size(px(12.0))
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(rgb(ui_theme::FOREGROUND))
                            .child("生成的工作流（JSON5 预览）"),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                            .child("确认无误后点「保存模板」，文件写入模板目录并出现在左侧列表"),
                    )
                    .child(
                        div()
                            .p_3()
                            .border_1()
                            .border_color(rgb(ui_theme::BORDER))
                            .rounded(px(ui_theme::RADIUS_MD))
                            .bg(rgb(ui_theme::TILE))
                            .font_family("monospace")
                            .text_size(px(10.5))
                            .text_color(rgb(ui_theme::FOREGROUND))
                            .child(preview),
                    ),
            )
            .into_any_element()
    }
}

/// 右侧引导卡（accent 软底）：当前步的说明文案。各步共用样式。
fn workflow_guide_card(title: &'static str, body: &'static str) -> gpui::AnyElement {
    div()
        .w(px(250.0))
        .flex_none()
        .flex()
        .flex_col()
        .gap_2()
        .p_3()
        .rounded(px(ui_theme::RADIUS_MD))
        .bg(rgb(ui_theme::PRIMARY_SUBTLE))
        .child(
            div()
                .text_size(px(13.0))
                .font_weight(gpui::FontWeight::BOLD)
                .text_color(rgb(ui_theme::PRIMARY))
                .child(title),
        )
        .child(
            div()
                .text_size(px(11.5))
                .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                .child(body),
        )
        .into_any_element()
}

/// 字段列：label + 输入框的纵向小列（变量卡片两列布局用）。
fn field_column(
    label: &'static str,
    required: bool,
    input: impl gpui::IntoElement,
) -> gpui::AnyElement {
    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .gap_1()
        .child(workflow_editor_field_label(label, required))
        .child(input)
        .into_any_element()
}

/// 变量卡片右上角的 ✕ 删除微按钮。
fn workflow_editor_card_remove_button(
    id: String,
    on_click: impl Fn(&mut RepositoryView, &mut Context<RepositoryView>) + 'static,
    cx: &mut Context<RepositoryView>,
) -> gpui::AnyElement {
    div()
        .id(id)
        .size(px(20.0))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(ui_theme::RADIUS_XS))
        .text_size(px(11.0))
        .text_color(rgb(ui_theme::MUTED_FOREGROUND))
        .cursor_pointer()
        .hover(|this| {
            this.bg(rgb(ui_theme::COLOR_WARNING))
                .text_color(rgb(ui_theme::COLOR_WARNING_FOREGROUND))
        })
        .on_click(cx.listener(move |this, _event, _window, cx| {
            on_click(this, cx);
        }))
        .child("✕")
        .into_any_element()
}

/// 步骤类型在选择器/图标位使用的简短符号。
fn workflow_step_kind_glyph(kind: WorkflowStepKind) -> &'static str {
    match kind {
        WorkflowStepKind::Checkout => "⇄",
        WorkflowStepKind::Fetch => "↙",
        WorkflowStepKind::Pull => "↓",
        WorkflowStepKind::CreateBranch => "⑂",
        WorkflowStepKind::Merge => "⑃",
        WorkflowStepKind::Push => "↑",
        WorkflowStepKind::GuardRemoteBranch => "⊘",
        WorkflowStepKind::EnsureClean => "✓",
        WorkflowStepKind::AssertBranch => "＝",
        WorkflowStepKind::FilterBranches => "⌕",
        WorkflowStepKind::DeleteBranches => "✕",
    }
}
/// 字段 label（带必填红点星号）。
fn workflow_editor_field_label(label: &str, required: bool) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap_1()
        .text_size(px(12.0))
        .text_color(rgb(ui_theme::MUTED_FOREGROUND))
        .child(label.to_string())
        .when(required, |this| {
            this.child(div().text_color(rgb(ui_theme::DESTRUCTIVE)).child("*"))
        })
}

/// 步骤行的 ↑ ↓ ✕ 微型按钮组。
fn workflow_editor_step_row_actions(
    index: usize,
    cx: &mut Context<RepositoryView>,
) -> impl IntoElement {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap_1()
        .child(workflow_editor_step_action_button("up", index, 0, "↑", cx))
        .child(workflow_editor_step_action_button(
            "down", index, 1, "↓", cx,
        ))
        .child(workflow_editor_step_action_button(
            "remove", index, 2, "✕", cx,
        ))
}

/// 单个步骤行微型按钮；action：0 上移 / 1 下移 / 2 删除。
fn workflow_editor_step_action_button(
    kind: &'static str,
    index: usize,
    action: u8,
    label: &'static str,
    cx: &mut Context<RepositoryView>,
) -> impl IntoElement {
    div()
        .id(format!("workflow-editor-step-{index}-{kind}"))
        .size(px(20.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(ui_theme::RADIUS_XS))
        .text_size(px(11.0))
        .text_color(rgb(ui_theme::MUTED_FOREGROUND))
        .cursor_pointer()
        .hover(|this| this.bg(rgb(ui_theme::SECONDARY)))
        .on_click(cx.listener(move |this, _event, _window, cx| {
            match action {
                0 => this.workflow_editor_move_step(index, true),
                1 => this.workflow_editor_move_step(index, false),
                _ => this.workflow_editor_remove_step(index),
            }
            cx.notify();
        }))
        .child(label)
}

/// 步骤类型下拉菜单的选项列表：常用 6 种一组，高级 5 种一组（分组标题），
/// 当前类型带 ✓ 前缀。点击选中后收起菜单。
#[allow(clippy::too_many_arguments)]
fn render_workflow_kind_menu_items(
    index: usize,
    current: WorkflowStepKind,
    menu_id: &str,
    cx: &mut Context<RepositoryView>,
) -> Vec<gpui::AnyElement> {
    let mut items: Vec<gpui::AnyElement> = Vec::new();
    let all_kinds = WorkflowStepKind::all();
    let split = WorkflowStepKind::COMMON_COUNT;
    let groups: [(&str, &[WorkflowStepKind]); 2] =
        [("常用", &all_kinds[..split]), ("高级", &all_kinds[split..])];
    for (group_label, kinds) in groups {
        items.push(
            div()
                .px_3()
                .pt_2()
                .pb_1()
                .text_size(px(10.0))
                .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                .child(group_label.to_string())
                .into_any_element(),
        );
        for candidate in kinds {
            let selected = *candidate == current;
            let kind = *candidate;
            items.push(
                div()
                    .id(format!("{menu_id}-item-{}", candidate.op_name()))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .px_3()
                    .py_1()
                    .text_size(px(12.0))
                    .text_color(rgb(if selected {
                        ui_theme::PRIMARY
                    } else {
                        ui_theme::FOREGROUND
                    }))
                    .bg(rgb(if selected {
                        ui_theme::PRIMARY_SUBTLE
                    } else {
                        ui_theme::CARD
                    }))
                    .cursor_pointer()
                    .hover(|this| this.bg(rgb(ui_theme::PRIMARY_SUBTLE)))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        cx.stop_propagation();
                        this.workflow_editor_set_step_kind(index, kind);
                        this.workflow_editor_close_kind_menu();
                        cx.notify();
                    }))
                    .child(candidate.display_name().to_string())
                    .when(selected, |this| {
                        this.child(
                            div()
                                .text_size(px(11.0))
                                .text_color(rgb(ui_theme::PRIMARY))
                                .child("✓"),
                        )
                    })
                    .into_any_element(),
            );
        }
        if group_label == "常用" {
            items.push(menu_separator().into_any_element());
        }
    }
    items
}

/// 高级区的「+ 添加」按钮。
fn workflow_editor_add_button(
    id: &'static str,
    label: &'static str,
    on_click: impl Fn(&mut RepositoryView) + 'static,
    cx: &mut Context<RepositoryView>,
) -> impl IntoElement {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .min_h(px(28.0))
        .px_3()
        .border_1()
        .border_color(rgb(ui_theme::BORDER))
        .rounded(px(ui_theme::RADIUS_XS))
        .cursor_pointer()
        .text_size(px(12.0))
        .text_color(rgb(ui_theme::PRIMARY))
        .hover(|this| this.bg(rgb(ui_theme::ACCENT)))
        .on_click(cx.listener(move |this, _event, _window, cx| {
            on_click(this);
            cx.notify();
        }))
        .child(label)
}

#[cfg(test)]
#[path = "tests/workflow_editor.rs"]
mod tests;
