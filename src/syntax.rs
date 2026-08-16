// 语法高亮纯函数层：把已解码的文本行映射为「行内字节区间 + RGB」span 序列。
//
// 供追溯视图、浏览分支内容视图、冲突工作台三栏和差异全文视图做只读代码着色。
// 全部为纯函数 + OnceLock 全局只读状态，线程安全，可在任意后台线程调用；
// UI 层负责在内容加载后调度后台计算并经事件带回。
//
// 回退语义：语言无法识别、文件过大（超过 SYNTAX_MAX_BYTES / SYNTAX_MAX_LINES）
// 时返回 None，渲染侧退回既有的整行单色路径。
//
// 主题采用 syntect 内置主题按深浅二选一（浅色 InspiredGitHub / 深色
// base16-ocean.dark），不自建 scope -> 应用色板映射（需低层 API，留作后续）。

use std::path::Path;
use std::sync::OnceLock;

use syntect::easy::HighlightLines;
use syntect::highlighting::{Style, Theme, ThemeSet};
use syntect::parsing::{SyntaxReference, SyntaxSet};

use crate::types::{DiffLineKind, FileDiff};

/// 语法高亮的体积守卫：超过则回退纯文本，避免大文件解析卡后台线程。
pub const SYNTAX_MAX_BYTES: usize = 1024 * 1024;
/// 语法高亮的行数守卫：超过则回退纯文本。
pub const SYNTAX_MAX_LINES: usize = 20_000;

/// 浅色主题名（syntect 内置）。
const THEME_NAME_LIGHT: &str = "InspiredGitHub";
/// 深色主题名（syntect 内置）。
const THEME_NAME_DARK: &str = "base16-ocean.dark";

/// 一行内的一个着色片段：`line[start..end]`（utf8 字节区间）用 `color` 前景。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SyntaxSpan {
    pub start: usize,
    pub end: usize,
    pub color: u32,
}

/// 一份内容的语法高亮结果：`lines` 与源行数组严格索引对齐；
/// 空 vec 表示该行不高亮（回退整行默认前景色）。
#[derive(Clone, Debug)]
pub struct SyntaxSpans {
    /// 计算时使用的深浅主题；主题切换后旧结果不再渲染。
    pub dark: bool,
    pub lines: Vec<Vec<SyntaxSpan>>,
}

static SYNTAX_SET: OnceLock<SyntaxSet> = OnceLock::new();
static THEME_LIGHT: OnceLock<Theme> = OnceLock::new();
static THEME_DARK: OnceLock<Theme> = OnceLock::new();

fn syntax_set() -> &'static SyntaxSet {
    SYNTAX_SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

/// 取指定深浅变体的内置主题；目标主题缺失时退回主题表第一项，保证可用。
fn theme(dark: bool) -> &'static Theme {
    if dark {
        THEME_DARK.get_or_init(|| load_theme(THEME_NAME_DARK))
    } else {
        THEME_LIGHT.get_or_init(|| load_theme(THEME_NAME_LIGHT))
    }
}

fn load_theme(name: &str) -> Theme {
    let themes = ThemeSet::load_defaults();
    themes
        .themes
        .get(name)
        .cloned()
        .or_else(|| themes.themes.values().next().cloned())
        .expect("syntect 默认主题集为空")
}

/// 按路径检测语言：扩展名优先，无扩展名或未命中时用文件名整体
/// （Makefile/Dockerfile 类按语法名匹配），仍无则返回 None。
fn detect_syntax<'set>(path: &str, set: &'set SyntaxSet) -> Option<&'set SyntaxReference> {
    let lower = path.to_ascii_lowercase();
    let lower = lower.as_str();
    if let Some(ext) = Path::new(lower).extension().and_then(|ext| ext.to_str()) {
        if let Some(syntax) = set.find_syntax_by_extension(ext) {
            return Some(syntax);
        }
    }
    let file_name = Path::new(lower)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(lower);
    set.find_syntax_by_token(file_name)
}

/// 守卫：体积或行数超限时放弃高亮。
fn within_limits(total_bytes: usize, line_count: usize) -> bool {
    total_bytes <= SYNTAX_MAX_BYTES && line_count <= SYNTAX_MAX_LINES
}

/// 对行数组做语法高亮，返回与源行索引对齐的 span 结果。
///
/// `None` 表示不高亮（语言未识别或超出守卫），调用方回退纯文本渲染。
pub fn highlight(path: &str, lines: &[String], dark: bool) -> Option<SyntaxSpans> {
    let total_bytes = lines.iter().map(|line| line.len()).sum::<usize>();
    if !within_limits(total_bytes, lines.len()) {
        return None;
    }
    let set = syntax_set();
    let syntax = detect_syntax(path, set)?;
    let theme = theme(dark);
    let highlighted = highlight_lines_with(syntax, theme, set, lines, dark);
    Some(highlighted)
}

/// 对差异全文的行做语法高亮：与 `FileDiff.lines` 严格索引对齐。
///
/// 文件头/hunk 头/EOFNL 标记行不参与解析（产空 vec，渲染侧回退 kind 纯色），
/// 其余 Context/Added/Removed 按出现顺序喂入有状态解析器。紧凑差异同样可喂
/// （调用方决定是否使用），只是 v1 仅在全文模式下调度。
pub fn highlight_diff_lines(diff: &FileDiff, dark: bool) -> Option<SyntaxSpans> {
    let total_bytes = diff
        .lines
        .iter()
        .map(|line| line.content.len())
        .sum::<usize>();
    if !within_limits(total_bytes, diff.lines.len()) {
        return None;
    }
    let set = syntax_set();
    let syntax = detect_syntax(&diff.path, set)?;
    let theme = theme(dark);

    let mut result = Vec::with_capacity(diff.lines.len());
    let mut state = HighlightLines::new(syntax, theme);
    for line in &diff.lines {
        // diff --git / index / --- / +++ 文件头与 @@ hunk 头是 Header kind；
        // EOFNL 标记行被上游折为 Context，按内容前缀识别后跳过。
        let feedable = line.kind == DiffLineKind::Context
            || line.kind == DiffLineKind::Added
            || line.kind == DiffLineKind::Removed;
        let is_eofnl_marker = line.content.starts_with("\\ No newline");
        if !feedable || is_eofnl_marker {
            result.push(Vec::new());
            continue;
        }
        result.push(highlight_one_line(&mut state, set, &line.content));
    }
    Some(SyntaxSpans {
        dark,
        lines: result,
    })
}

/// 用同一个有状态解析器逐行高亮（多行注释/字符串等跨行结构依赖状态延续）。
fn highlight_lines_with(
    syntax: &SyntaxReference,
    theme: &Theme,
    set: &SyntaxSet,
    lines: &[String],
    dark: bool,
) -> SyntaxSpans {
    let mut state = HighlightLines::new(syntax, theme);
    let result = lines
        .iter()
        .map(|line| highlight_one_line(&mut state, set, line))
        .collect();
    SyntaxSpans {
        dark,
        lines: result,
    }
}

/// 高亮单行：syntect 的 newlines 模式要求行尾带 \n，结果按字节偏移累积成
/// span（末尾换行被裁掉），相邻同色合并、无零长度片段。
fn highlight_one_line(
    state: &mut HighlightLines<'_>,
    set: &SyntaxSet,
    line: &str,
) -> Vec<SyntaxSpan> {
    let mut feed = String::with_capacity(line.len() + 1);
    feed.push_str(line);
    feed.push('\n');
    let ranges = state.highlight_line(&feed, set).unwrap_or_default();

    let mut spans: Vec<SyntaxSpan> = Vec::new();
    let mut offset = 0usize;
    let line_len = line.len();
    for (style, text) in ranges {
        let start = offset.min(line_len);
        let end = (offset + text.len()).min(line_len);
        offset += text.len();
        if end <= start {
            continue;
        }
        let color = style_foreground(style);
        match spans.last_mut() {
            Some(last) if last.color == color && last.end == start => last.end = end,
            _ => spans.push(SyntaxSpan { start, end, color }),
        }
    }
    spans
}

/// Style 前景色 -> 0xRRGGBB。
fn style_foreground(style: Style) -> u32 {
    let color = style.foreground;
    (u32::from(color.r) << 16) | (u32::from(color.g) << 8) | u32::from(color.b)
}

#[cfg(test)]
#[path = "tests/syntax.rs"]
mod tests;
