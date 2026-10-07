//! 搜索目录只持有导航目标；确认后仍调用业务入口，不直接执行危险 Git 操作。

use crate::{MainMode, RepoTabId, SettingsCategory, ai_extensions_view::AiSettingsTab};
use khaslana::{BranchKind, RepositorySnapshot};
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum SearchTarget {
    Mode(RepoTabId, MainMode),
    Feature(RepoTabId, &'static str, MainMode),
    Settings(SettingsCategory),
    AiSettings(AiSettingsTab),
    Repository(RepoTabId),
    SavedRepository(PathBuf),
    SavedBranch(PathBuf, String, BranchKind),
    Branch(RepoTabId, String, BranchKind),
    Tag(RepoTabId, String),
    Remote(RepoTabId, String),
    Stash(RepoTabId, String),
    Workflow(RepoTabId, PathBuf),
    OpenRepository,
    CloneRepository,
    CreateBranch(RepoTabId),
    CreateTag(RepoTabId),
    CreateStash(RepoTabId),
    Submodules(RepoTabId),
    Remotes(RepoTabId),
    Pull(RepoTabId),
    Push(RepoTabId),
    WorkflowEditor(RepoTabId),
    ReviewHistory(RepoTabId),
}

#[derive(Clone, Debug)]
pub(super) struct SearchEntry {
    pub title: String,
    pub subtitle: String,
    pub group: &'static str,
    keywords: &'static str,
    pub target: SearchTarget,
    pub disabled_reason: Option<&'static str>,
}

impl SearchEntry {
    pub fn new(
        title: impl Into<String>,
        subtitle: impl Into<String>,
        group: &'static str,
        keywords: &'static str,
        target: SearchTarget,
    ) -> Self {
        Self {
            title: title.into(),
            subtitle: subtitle.into(),
            group,
            keywords,
            target,
            disabled_reason: None,
        }
    }

    fn disabled(mut self, reason: Option<&'static str>) -> Self {
        self.disabled_reason = reason;
        self
    }
}

pub(super) fn function_entries(
    tab_id: Option<RepoTabId>,
    busy: bool,
    snapshot: Option<&RepositorySnapshot>,
    ai_enabled: bool,
) -> Vec<SearchEntry> {
    let id = tab_id.unwrap_or(RepoTabId(0));
    let repo_reason = if tab_id.is_none() {
        Some("请先打开仓库")
    } else {
        None
    };
    let operation_reason = repo_reason.or(busy.then_some("当前操作进行中，请稍候"));
    let remote_reason = operation_reason
        .or_else(|| {
            snapshot
                .filter(|snapshot| !snapshot.remotes.is_empty())
                .is_none()
                .then_some("当前仓库没有可用远端")
        })
        .or_else(|| {
            snapshot
                .is_some_and(|snapshot| snapshot.merge_in_progress)
                .then_some("请先完成或中止合并")
        });
    let mut entries = vec![
        SearchEntry::new(
            "打开仓库",
            "选择本地 Git 仓库文件夹",
            "功能",
            "打开 项目 open repository repo",
            SearchTarget::OpenRepository,
        )
        .disabled(busy.then_some("当前操作进行中，请稍候")),
        SearchEntry::new(
            "克隆仓库",
            "填写远端地址和本地目录",
            "功能",
            "下载 项目 clone",
            SearchTarget::CloneRepository,
        )
        .disabled(busy.then_some("当前操作进行中，请稍候")),
    ];
    for (title, description, keywords, mode) in [
        (
            "工作区",
            "查看修改、暂存文件和提交",
            "修改 提交 暂存 取消暂存 修补 amend commit stage unstage diff refresh 刷新",
            MainMode::Worktree,
        ),
        (
            "提交记录",
            "查看历史、差异及提交操作",
            "历史 日志 撤销 重置 回退 拣选 cherry-pick reset revert log history",
            MainMode::History,
        ),
        (
            "提交图谱",
            "查看分支谱系和提交关系",
            "图谱 分支 graph",
            MainMode::CommitGraph,
        ),
        (
            "工作流",
            "选择模板、配置参数和查看执行日志",
            "自动化 模板 workflow",
            MainMode::Workflow,
        ),
        (
            "冲突处理",
            "查看冲突并按块接受或使用 AI 合并",
            "合并 解决冲突 merge conflict AI",
            MainMode::Conflict,
        ),
        (
            "贮藏列表",
            "查看已保存的贮藏和文件差异",
            "储藏 stash",
            MainMode::Stash,
        ),
        (
            "浏览文件",
            "浏览当前分支文件内容和比较差异",
            "文件 查看 分支比较 compare browse",
            MainMode::Browse,
        ),
        (
            "追溯",
            "查看文件每一行的提交来源",
            "blame 作者 代码追溯",
            MainMode::Blame,
        ),
    ] {
        let reason = if mode == MainMode::Conflict {
            repo_reason.or_else(|| {
                snapshot
                    .is_none_or(|snapshot| snapshot.conflicts.is_empty())
                    .then_some("当前仓库没有冲突")
            })
        } else {
            repo_reason
        };
        entries.push(
            SearchEntry::new(
                title,
                description,
                "功能",
                keywords,
                SearchTarget::Mode(id, mode),
            )
            .disabled(reason),
        );
    }
    // 需要先选择文件、分支或提交的操作，定位到它们所在页面，不代替用户选对象。
    for (title, description, keywords, mode) in [
        (
            "提交修改",
            "工作区 · 暂存修改后在差异下方填写提交信息",
            "commit 提交 暂存 stage",
            MainMode::Worktree,
        ),
        (
            "AI 生成提交信息",
            "工作区 · 提交区的 AI 生成按钮",
            "提交说明 commit message",
            MainMode::Worktree,
        ),
        (
            "AI 代码评审",
            "工作区 · 选择差异后打开 AI 评审",
            "审查 review",
            MainMode::Worktree,
        ),
        (
            "刷新仓库",
            "顶栏 · 刷新按钮",
            "refresh 刷新",
            MainMode::Worktree,
        ),
        (
            "获取远端更新",
            "顶栏 · 获取按钮",
            "fetch 获取",
            MainMode::Worktree,
        ),
        (
            "切换分支",
            "工作区 · 在分支列表中选择检出目标",
            "checkout 检出",
            MainMode::Worktree,
        ),
        (
            "合并分支",
            "工作区 · 分支右键菜单中的合并操作",
            "merge",
            MainMode::Worktree,
        ),
        (
            "变基",
            "工作区 · 分支右键菜单中的变基操作",
            "rebase",
            MainMode::Worktree,
        ),
        (
            "重命名分支",
            "工作区 · 分支右键菜单中的重命名操作",
            "rename",
            MainMode::Worktree,
        ),
        (
            "删除分支",
            "工作区 · 分支右键菜单中的删除确认入口",
            "delete branch",
            MainMode::Worktree,
        ),
        (
            "分支比较",
            "工作区 · 分支右键菜单中的比较操作",
            "compare",
            MainMode::Worktree,
        ),
        (
            "设置上游分支",
            "工作区 · 分支右键菜单中的上游设置",
            "upstream 跟踪",
            MainMode::Worktree,
        ),
        (
            "拣选提交",
            "提交记录 · 选择提交后在右键菜单中拣选",
            "cherry-pick",
            MainMode::History,
        ),
        (
            "重置到提交",
            "提交记录 · 选择提交后在右键菜单中重置",
            "reset",
            MainMode::History,
        ),
        (
            "还原提交",
            "提交记录 · 选择提交后在右键菜单中还原",
            "revert 回退",
            MainMode::History,
        ),
        (
            "撤销提交",
            "提交记录 · 选择提交后在右键菜单中撤销",
            "uncommit",
            MainMode::History,
        ),
        (
            "文件历史",
            "工作区 · 文件右键菜单中的历史入口",
            "历史 history",
            MainMode::Worktree,
        ),
    ] {
        entries.push(
            SearchEntry::new(
                title,
                description,
                "功能",
                keywords,
                SearchTarget::Feature(id, title, mode),
            )
            .disabled(repo_reason),
        );
    }
    entries.push(SearchEntry::new(
        "设置中心",
        "打开应用设置和偏好",
        "设置",
        "settings preferences",
        SearchTarget::Settings(SettingsCategory::Credentials),
    ));
    for category in crate::settings_center::SETTINGS_CATEGORY_ORDER {
        let meta = crate::settings_center::settings_pane_meta(category);
        let keywords = match category {
            SettingsCategory::Credentials => "设置 密钥 令牌 账号 SSH 密码 credentials token",
            SettingsCategory::Proxy => "设置 代理 socks http https proxy",
            SettingsCategory::Ai => "设置 模型 大模型 接口 服务 商 provider API AI",
            SettingsCategory::ExternalMerge => "设置 外部合并 IntelliJ IDEA merge tool",
            SettingsCategory::CodeIndex => "设置 代码索引 知识图谱 重建 code index",
            SettingsCategory::Theme => "设置 主题 深色 浅色 配色 字体 theme appearance",
            SettingsCategory::Update => "设置 版本 检查更新 数据目录 迁移 update",
            SettingsCategory::Shortcuts => "设置 按键 键盘 快捷键 keyboard shortcut",
            SettingsCategory::About => "设置 关于 版本 说明 about",
        };
        entries.push(SearchEntry::new(
            meta.title,
            meta.description,
            "设置",
            keywords,
            SearchTarget::Settings(category),
        ));
    }
    for (title, description, keywords, tab) in [
        (
            "Skill 管理",
            "AI 设置 · 导入、查看和管理技能",
            "技能 扩展 skills",
            AiSettingsTab::Skill,
        ),
        (
            "MCP 服务",
            "AI 设置 · 配置服务器和工具权限",
            "工具 扩展 mcp server",
            AiSettingsTab::Mcp,
        ),
        (
            "浏览器运行环境",
            "AI 设置 · 配置工作流浏览器运行环境",
            "browser runtime playwright",
            AiSettingsTab::Runtime,
        ),
    ] {
        entries.push(
            SearchEntry::new(
                title,
                description,
                "设置",
                keywords,
                SearchTarget::AiSettings(tab),
            )
            .disabled((!ai_enabled).then_some("请先在 AI 设置中启用 AI 功能")),
        );
    }
    for (title, description, keywords, target, reason) in [
        (
            "创建分支",
            "打开新分支表单",
            "新建 branch",
            SearchTarget::CreateBranch(id),
            operation_reason,
        ),
        (
            "创建标签",
            "为当前 HEAD 创建标签",
            "新建 tag",
            SearchTarget::CreateTag(id),
            operation_reason,
        ),
        (
            "创建贮藏",
            "打开贮藏表单，保存当前修改",
            "储藏 stash",
            SearchTarget::CreateStash(id),
            operation_reason.or_else(|| {
                snapshot
                    .is_some_and(|snapshot| snapshot.merge_in_progress)
                    .then_some("请先完成或中止合并")
            }),
        ),
        (
            "子模块",
            "管理、初始化和更新子模块",
            "submodule",
            SearchTarget::Submodules(id),
            operation_reason,
        ),
        (
            "远端管理",
            "添加、编辑远端并配置凭据",
            "remote 远程 地址 获取 fetch",
            SearchTarget::Remotes(id),
            operation_reason,
        ),
        (
            "拉取",
            "选择远端和分支后拉取",
            "pull 下载",
            SearchTarget::Pull(id),
            remote_reason,
        ),
        (
            "推送",
            "选择远端和分支后推送",
            "push 上传",
            SearchTarget::Push(id),
            remote_reason,
        ),
        (
            "工作流编辑器",
            "创建或编辑工作流模板",
            "模板 新建 修改 编辑 workflow editor",
            SearchTarget::WorkflowEditor(id),
            repo_reason,
        ),
        (
            "AI 评审历史",
            "查看已保存的代码评审报告",
            "审查 review",
            SearchTarget::ReviewHistory(id),
            repo_reason,
        ),
    ] {
        entries
            .push(SearchEntry::new(title, description, "功能", keywords, target).disabled(reason));
    }
    entries
}

pub(super) fn append_repository_entries(
    entries: &mut Vec<SearchEntry>,
    id: RepoTabId,
    repo_name: &str,
    snapshot: &RepositorySnapshot,
) {
    for branch in &snapshot.branches {
        let group = if branch.kind == BranchKind::Local {
            "本地分支"
        } else {
            "远端分支"
        };
        entries.push(SearchEntry::new(
            &branch.name,
            format!(
                "{repo_name} · 定位并高亮分支{}",
                if branch.is_head {
                    " · 当前分支"
                } else {
                    ""
                }
            ),
            group,
            "branch 分支",
            SearchTarget::Branch(id, branch.name.clone(), branch.kind.clone()),
        ));
    }
    for tag in &snapshot.tags {
        entries.push(SearchEntry::new(
            &tag.name,
            format!("{repo_name} · 浏览标签文件"),
            "标签",
            "tag",
            SearchTarget::Tag(id, tag.name.clone()),
        ));
    }
    for remote in &snapshot.remotes {
        entries.push(SearchEntry::new(
            &remote.name,
            format!("{repo_name} · {}", remote.url),
            "远端",
            "remote",
            SearchTarget::Remote(id, remote.name.clone()),
        ));
    }
    for stash in &snapshot.stashes {
        entries.push(SearchEntry::new(
            format!("stash@{{{}}} {}", stash.index, stash.message),
            format!("{repo_name} · 查看贮藏差异"),
            "贮藏",
            "stash 储藏",
            SearchTarget::Stash(id, stash.oid.clone()),
        ));
    }
}

/// 名称精确匹配优先于前缀、子串和说明/别名；多词查询需要全部命中。
/// 不按固定数量截断结果，滚动与大仓库列表由 Kit 虚拟化处理。
pub(super) fn filter_entries(entries: Vec<SearchEntry>, query: &str) -> Vec<SearchEntry> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return entries;
    }
    let tokens: Vec<_> = query.split_whitespace().collect();
    let mut matches: Vec<_> = entries
        .into_iter()
        .filter_map(|entry| {
            let title = entry.title.to_lowercase();
            let extra =
                format!("{} {} {}", entry.subtitle, entry.group, entry.keywords).to_lowercase();
            let mut score = 0;
            for token in &tokens {
                score += if title == *token {
                    0
                } else if title.starts_with(token) {
                    1
                } else if title.contains(token) {
                    2
                } else if extra.contains(token) {
                    3
                } else {
                    return None;
                };
            }
            Some((score, entry))
        })
        .collect();
    matches.sort_by_key(|(score, _)| *score);
    matches.into_iter().map(|(_, entry)| entry).collect()
}

pub(super) fn next_selection(
    entries: &[SearchEntry],
    selected: Option<usize>,
    forward: bool,
) -> Option<usize> {
    let enabled: Vec<_> = entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| entry.disabled_reason.is_none().then_some(index))
        .collect();
    let position =
        selected.and_then(|selected| enabled.iter().position(|index| *index == selected));
    match position {
        Some(position) => enabled
            .get(if forward {
                (position + 1).min(enabled.len() - 1)
            } else {
                position.saturating_sub(1)
            })
            .copied(),
        None => {
            if forward {
                enabled.first().copied()
            } else {
                enabled.last().copied()
            }
        }
    }
}

#[cfg(test)]
#[path = "../tests/code_palette.rs"]
mod tests;
