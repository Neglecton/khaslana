// Markdown 渲染模块：把 Markdown 文本解析为「块 / 行 / 行内 span」的纯数据
// 结构，再映射为 gpui 元素。用于 AI 评审结果等富文本展示。
//
// 解析层（`parse_markdown_blocks`）与渲染层分离：解析层是纯函数、可单测；
// 渲染层复用 `StyledText::with_highlights`（语法高亮同款先例）做行内样式。
// 支持子集：标题、段落（软换行分行）、加粗/斜体/删除线、行内代码、围栏
// 代码块、有序/无序列表（扁平化为带前缀与缩进的行）、引用块、分隔线。
// 链接与图片仅保留文字（不渲染交互元素）；表格/脚注/HTML 不启用，相关
// 文本按普通内容流出。流式半截输入（未闭合围栏/加粗）由 CommonMark 解析
// 器在 EOF 自动收尾，天然安全。

use gpui::{
    FontStyle, FontWeight, HighlightStyle, IntoElement, StrikethroughStyle, StyledText, div,
    prelude::*, px,
};
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

use crate::ui::theme::rgb;
use crate::ui_theme;

/// 行内 span：一段文本 + 样式标记。code 与 muted 为独立观感，不与其他样式叠加。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct MdInlineSpan {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    /// 行内代码：TILE 底 + PRIMARY 字（chip 观感）。
    pub code: bool,
    /// 弱化文字（列表符号等辅助标记）。
    pub muted: bool,
}

impl MdInlineSpan {
    fn style_key(&self) -> (bool, bool, bool, bool, bool) {
        (self.bold, self.italic, self.strike, self.code, self.muted)
    }
}

/// 一行：列表嵌套缩进（渲染每级 14px）+ 行内 span 序列。
/// 列表前缀（「• 」/「3. 」）在解析层作为首个 muted span 插入。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct MdLine {
    pub indent: usize,
    pub spans: Vec<MdInlineSpan>,
}

/// 块级结构。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum MdBlock {
    Heading {
        level: u8,
        spans: Vec<MdInlineSpan>,
    },
    /// 一段（或一个列表扁平化后）的行序列。
    Paragraph(Vec<MdLine>),
    /// 围栏代码块原文（语言暂不用于高亮，留作后续接 syntect）。
    Code {
        text: String,
    },
    /// 引用块（内部为完整块序列）。
    Quote(Vec<MdBlock>),
    /// 水平分隔线。
    Rule,
}

/// 解析 Markdown 文本为块序列（纯函数）。
pub(crate) fn parse_markdown_blocks(source: &str) -> Vec<MdBlock> {
    let mut parser = MdParser::default();
    // 显式启用删除线（GFM 基础扩展）；表格/脚注保持关闭
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    for event in Parser::new_ext(source, options) {
        parser.on_event(event);
    }
    parser.seal_paragraph();
    parser.blocks
}

#[derive(Default)]
struct MdParser {
    /// 当前容器内的块（顶层，或最深的引用块）。
    blocks: Vec<MdBlock>,
    /// 引用块嵌套时暂存的外层容器。
    saved: Vec<Vec<MdBlock>>,
    /// 行内样式嵌套计数（Start/End 配对递增递减）。
    bold: u32,
    italic: u32,
    strike: u32,
    /// 正在累积的行内 span。
    line: Vec<MdInlineSpan>,
    /// 当前段落已完成的行（列表扁平化：一个列表通常汇成一段）。
    para: Vec<MdLine>,
    /// 当前行缩进（列表深度），行 flush 后按当前列表深度重置。
    line_indent: usize,
    /// 当前列表项前缀（挂在项内首行行首）。
    item_marker: Option<MdInlineSpan>,
    /// 列表栈：(是否有序, 下一个序号)。
    lists: Vec<(bool, u64)>,
    /// 标题级别（Heading 期间有效）。
    heading: Option<u8>,
    /// 代码块文本收集（期间 Text 事件原样追加）。
    code: Option<String>,
}

impl MdParser {
    fn on_event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.on_start(tag),
            Event::End(tag_end) => self.on_end(tag_end),
            Event::Text(text) => {
                if let Some(code) = self.code.as_mut() {
                    code.push_str(&text);
                } else {
                    self.push_span(&text, false);
                }
            }
            Event::Code(text) => self.push_span(&text, true),
            // HTML 片段按原文保留（不渲染标签语义）
            Event::Html(html) | Event::InlineHtml(html) => {
                if let Some(code) = self.code.as_mut() {
                    code.push_str(&html);
                } else {
                    self.push_span(&html, false);
                }
            }
            // 软/硬换行都作为换行处理（渲染上同为另起一行）
            Event::SoftBreak | Event::HardBreak => self.flush_line(),
            // 任务列表勾选框：以 "[ ]"/"[x]" 文本并入行首，避免被吞掉后
            // 列表项内容凭空少一段。
            Event::TaskListMarker(checked) => {
                let checkbox = if checked { "[x] " } else { "[ ] " };
                self.push_span(checkbox, false);
            }
            Event::Rule => {
                self.seal_paragraph();
                self.blocks.push(MdBlock::Rule);
            }
            // 数学/脚注引用等不启用，忽略
            _ => {}
        }
    }

    fn on_start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => {}
            Tag::Heading { level, .. } => {
                self.seal_paragraph();
                self.heading = Some(level as u8);
            }
            Tag::CodeBlock(_) => {
                self.seal_paragraph();
                self.code = Some(String::new());
            }
            Tag::List(start) => {
                self.flush_line();
                let ordered = start.is_some();
                self.lists.push((ordered, start.unwrap_or(1)));
            }
            Tag::Item => {
                self.flush_line();
                let (indent, marker) = self.list_marker();
                self.line_indent = indent;
                self.item_marker = Some(MdInlineSpan {
                    text: marker,
                    muted: true,
                    ..Default::default()
                });
            }
            Tag::BlockQuote(_) => {
                self.seal_paragraph();
                self.saved.push(std::mem::take(&mut self.blocks));
            }
            Tag::Strong => self.bold += 1,
            Tag::Emphasis => self.italic += 1,
            Tag::Strikethrough => self.strike += 1,
            // 链接/图片：保留其文字（Text 事件照常流出），不渲染交互元素
            Tag::Link { .. } | Tag::Image { .. } => {}
            _ => {}
        }
    }

    fn on_end(&mut self, tag_end: TagEnd) {
        match tag_end {
            TagEnd::Paragraph => {
                self.flush_line();
                // 段落边界：把已累积行落成块，避免相邻段落粘连
                self.seal_paragraph();
            }
            TagEnd::Heading(_) => {
                self.flush_line();
                let spans = self
                    .para
                    .drain(..)
                    .flat_map(|line| line.spans)
                    .collect::<Vec<_>>();
                if !spans.is_empty() {
                    let level = self.heading.take().unwrap_or(3);
                    self.blocks.push(MdBlock::Heading { level, spans });
                }
            }
            TagEnd::CodeBlock => {
                let text = self.code.take().unwrap_or_default();
                self.blocks.push(MdBlock::Code { text });
            }
            TagEnd::List(_) => {
                self.flush_line();
                self.lists.pop();
            }
            TagEnd::Item => self.flush_line(),
            TagEnd::BlockQuote(_) => {
                self.seal_paragraph();
                let inner = std::mem::take(&mut self.blocks);
                self.blocks = self.saved.pop().unwrap_or_default();
                self.blocks.push(MdBlock::Quote(inner));
            }
            TagEnd::Strong => self.bold = self.bold.saturating_sub(1),
            TagEnd::Emphasis => self.italic = self.italic.saturating_sub(1),
            TagEnd::Strikethrough => self.strike = self.strike.saturating_sub(1),
            _ => {}
        }
    }

    /// 生成当前列表项的缩进与前缀（有序取序号并递增，无序用圆点）。
    fn list_marker(&mut self) -> (usize, String) {
        let depth = self.lists.len();
        match self.lists.last_mut() {
            Some((true, next)) => {
                let marker = format!("{next}. ");
                *next += 1;
                (depth, marker)
            }
            _ => (depth, "• ".to_string()),
        }
    }

    fn push_span(&mut self, text: &str, code: bool) {
        if text.is_empty() {
            return;
        }
        let span = if code {
            MdInlineSpan {
                text: text.to_string(),
                code: true,
                ..Default::default()
            }
        } else {
            MdInlineSpan {
                text: text.to_string(),
                bold: self.bold > 0,
                italic: self.italic > 0,
                strike: self.strike > 0,
                ..Default::default()
            }
        };
        // 相邻同样式 span 合并，减少渲染 run 数
        match self.line.last_mut() {
            Some(last) if last.style_key() == span.style_key() => {
                last.text.push_str(&span.text);
            }
            _ => self.line.push(span),
        }
    }

    /// 把正在累积的行收进当前段落（列表项前缀挂到行首）。
    fn flush_line(&mut self) {
        if self.line.is_empty() && self.item_marker.is_none() {
            return;
        }
        let mut spans = Vec::new();
        if let Some(marker) = self.item_marker.take() {
            spans.push(marker);
        }
        spans.append(&mut self.line);
        self.para.push(MdLine {
            indent: self.line_indent,
            spans,
        });
        // 后续行延续当前列表缩进（项内软换行）；列表外归零
        self.line_indent = self.lists.len();
    }

    /// 把当前段落的行落成 Paragraph 块（空段丢弃）。
    fn seal_paragraph(&mut self) {
        self.flush_line();
        if !self.para.is_empty() {
            let lines = std::mem::take(&mut self.para);
            self.blocks.push(MdBlock::Paragraph(lines));
        }
    }
}

// ── 渲染层 ────────────────────────────────────────────────────────────────

/// 渲染 Markdown 文本为元素（解析失败/不支持的结构已退化为纯文本）。
pub(crate) fn render_markdown(source: &str) -> impl IntoElement {
    let blocks = parse_markdown_blocks(source);
    div()
        .flex()
        .flex_col()
        .gap_1()
        .min_w(px(0.0))
        .children(blocks.iter().map(render_block))
}

fn render_block(block: &MdBlock) -> gpui::AnyElement {
    match block {
        MdBlock::Heading { level, spans } => {
            let size = match level {
                1 => 15.0,
                2 => 13.0,
                _ => 12.0,
            };
            div()
                .flex_none()
                .mt_1()
                .text_size(px(size))
                .font_weight(FontWeight::BOLD)
                .text_color(rgb(ui_theme::FOREGROUND))
                .child(md_styled_text(spans))
                .into_any_element()
        }
        MdBlock::Paragraph(lines) => div()
            .flex()
            .flex_col()
            .min_w(px(0.0))
            .children(lines.iter().map(render_line))
            .into_any_element(),
        MdBlock::Code { text } => {
            // 逐行渲染以保留空行；长行随容器宽度折行
            let body = text.strip_suffix('\n').unwrap_or(text);
            div()
                .flex_none()
                .flex()
                .flex_col()
                .my_1()
                .px_2()
                .py_1()
                .rounded_sm()
                .bg(rgb(ui_theme::TILE))
                .font_family("Consolas, monospace")
                .text_size(px(11.0))
                .line_height(px(16.0))
                .children(
                    body.split('\n')
                        .map(|line| div().min_h(px(16.0)).child(line.to_string())),
                )
                .into_any_element()
        }
        MdBlock::Quote(blocks) => div()
            .flex_none()
            .flex()
            .flex_col()
            .gap_1()
            .pl_2()
            .py_1()
            .border_l_2()
            .border_color(rgb(ui_theme::BORDER))
            .text_color(rgb(ui_theme::MUTED_FOREGROUND))
            .children(blocks.iter().map(render_block))
            .into_any_element(),
        MdBlock::Rule => div()
            .flex_none()
            .my_1()
            .h(px(1.0))
            .bg(rgb(ui_theme::BORDER))
            .into_any_element(),
    }
}

fn render_line(line: &MdLine) -> impl IntoElement {
    div()
        .flex_none()
        .min_w(px(0.0))
        .pl(px(line.indent as f32 * 14.0))
        .line_height(px(18.0))
        .child(md_styled_text(&line.spans))
}

/// 行内 span 序列 → 整行一个 StyledText（未覆盖区间沿用容器样式，
/// 如标题的加粗与字号的继承不受影响）。
fn md_styled_text(spans: &[MdInlineSpan]) -> gpui::AnyElement {
    if spans.is_empty() {
        return div().into_any_element();
    }
    let mut text = String::new();
    let mut highlights = Vec::new();
    for span in spans {
        let start = text.len();
        text.push_str(&span.text);
        let end = text.len();
        if end <= start {
            continue;
        }
        highlights.push((start..end, span_highlight(span)));
    }
    StyledText::new(text)
        .with_highlights(highlights)
        .into_any_element()
}

fn span_highlight(span: &MdInlineSpan) -> HighlightStyle {
    let mut style = HighlightStyle::default();
    if span.bold {
        style.font_weight = Some(FontWeight::BOLD);
    }
    if span.italic {
        style.font_style = Some(FontStyle::Italic);
    }
    if span.strike {
        style.strikethrough = Some(StrikethroughStyle::default());
    }
    if span.code {
        style.background_color = Some(rgb(ui_theme::TILE).into());
        style.color = Some(rgb(ui_theme::PRIMARY).into());
    } else if span.muted {
        style.color = Some(rgb(ui_theme::MUTED_FOREGROUND).into());
    }
    style
}

#[cfg(test)]
#[path = "tests/markdown_view.rs"]
mod tests;
