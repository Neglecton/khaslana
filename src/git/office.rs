//! Office Open XML（docx/xlsx/pptx）文本提取纯函数层。
//!
//! 差异视图对 Office 文档默认只能显示二进制占位卡；这里把 ZIP 内的
//! 主文档 XML 解出为纯文本行（docx 按段落、xlsx 按表格行、pptx 按幻灯片
//! 段落），让 `file_diff_from_diff` 能对提取文本做两侧 diff，实现
//! SourceTree 式的「文本化差异预览」。提取结果只用于展示——不能反向写回
//! Office 文件，因此调用侧保持 `is_binary = true` 以沿用部分暂存等门控。
//!
//! 不支持旧版二进制格式（.doc/.xls/.ppt 为 OLE 复合文档，结构完全不同）。
//! 任何解析失败（损坏、加密、缺条目、超限）返回 `None`，调用方回退到
//! 二进制占位卡。

use std::io::Read;

/// 提取入口的文件体积上限：与全文差异视图同一档（`FULL_FILE_MAX_BYTES`），
/// 超限的 Office 文档不做提取（避免超大文档解压内存峰值）。
const OFFICE_EXTRACT_MAX_FILE_BYTES: u64 = 3 * 1024 * 1024;
/// 单个 ZIP 条目解压后的体积上限：document.xml / sheet XML 可能远大于
/// 压缩体积，解压前先看声明大小，超限放弃。
const OFFICE_EXTRACT_MAX_ENTRY_BYTES: u64 = 8 * 1024 * 1024;

/// 路径是否为支持的 Office Open XML 文档（按扩展名，大小写不敏感）。
pub(crate) fn path_has_office_extension(path: &str) -> bool {
    office_format_of(path).is_some()
}

/// 从路径扩展名判断 Office 格式。
fn office_format_of(path: &str) -> Option<OfficeFormat> {
    let ext = path.rsplit_once('.')?.1.to_ascii_lowercase();
    match ext.as_str() {
        "docx" => Some(OfficeFormat::Docx),
        "xlsx" => Some(OfficeFormat::Xlsx),
        "pptx" => Some(OfficeFormat::Pptx),
        _ => None,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum OfficeFormat {
    Docx,
    Xlsx,
    Pptx,
}

/// 把 Office 文档字节提取为文本行（UTF-8）。失败返回 `None`。
pub(crate) fn office_text_lines(path: &str, bytes: &[u8]) -> Option<Vec<String>> {
    let format = office_format_of(path)?;
    if bytes.len() as u64 > OFFICE_EXTRACT_MAX_FILE_BYTES {
        return None;
    }
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).ok()?;
    match format {
        OfficeFormat::Docx => extract_docx(&mut archive),
        OfficeFormat::Xlsx => extract_xlsx(&mut archive),
        OfficeFormat::Pptx => extract_pptx(&mut archive),
    }
}

/// 读取指定条目的解压字节（带体积上限预检）。
fn read_entry(
    archive: &mut zip::ZipArchive<std::io::Cursor<&[u8]>>,
    name: &str,
) -> Option<Vec<u8>> {
    let mut entry = archive.by_name(name).ok()?;
    if entry.size() > OFFICE_EXTRACT_MAX_ENTRY_BYTES {
        return None;
    }
    let mut out = Vec::with_capacity(entry.size() as usize);
    entry.read_to_end(&mut out).ok()?;
    if out.len() as u64 > OFFICE_EXTRACT_MAX_ENTRY_BYTES {
        return None;
    }
    Some(out)
}

// ── 通用 XML 轻量扫描 ──────────────────────────────────────────────────────
// Office 主文档 XML 的结构固定且扁平（段落/单元格/文本 run），不需要完整
// XML 解析器：按字节扫描标签边界即可，遇到不认识的标签整体跳过其内容。

/// 按顺序产出匹配 `tag` 的完整元素片段（含开闭标签的字符串切片）。
/// 简化假设：目标元素不自嵌套（w:p / si / row / a:p 均如此）；`<x .../>`
/// 自闭合视为空元素。`<w:p` 后必须跟 `>`/`/`/空白，防止误匹配 `<w:pgSz`
/// 这类同名前缀元素。
fn scan_elements<'a>(xml: &'a str, tag: &str) -> Vec<&'a str> {
    let mut out = Vec::new();
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut pos = 0;
    while let Some(open_at) = xml[pos..].find(&open) {
        let elem_start = pos + open_at;
        let after = &xml[elem_start + open.len()..];
        if !is_tag_boundary(after) {
            pos = elem_start + open.len();
            continue;
        }
        // 先定位开标签的 '>'：若其前是 '/' 即自闭合元素（整体到 '>' 为止）；
        // 否则找配对闭标签（目标元素不自嵌套，首个闭标签即本元素的）。
        // 不能全局搜 "/>"——子元素（如 <w:r> 内的 <w:tab/>）的自闭合会
        // 被误当成父元素的结束。
        let Some(open_tag_end_rel) = xml[elem_start..].find('>') else {
            break;
        };
        let open_tag_end = elem_start + open_tag_end_rel + 1;
        let end = if xml[..open_tag_end].ends_with("/>") {
            open_tag_end
        } else {
            match xml[open_tag_end..].find(&close) {
                Some(offset) => open_tag_end + offset + close.len(),
                None => break,
            }
        };
        out.push(&xml[elem_start..end]);
        pos = end;
    }
    out
}

/// `<w:t` 之后是否是标签边界（`>` / `/>` / 空白），排除 `<w:tabs>`、
/// `<w:tc>` 这类同名前缀元素。
fn is_tag_boundary(after: &str) -> bool {
    matches!(
        after.chars().next(),
        Some('>') | Some('/') | Some(' ') | Some('\t') | Some('\r') | Some('\n')
    )
}

/// 解码 XML 文本节点里的预定义实体与数字实体。
fn decode_xml_entities(raw: &str) -> String {
    if !raw.contains('&') {
        return raw.to_string();
    }
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let tail = &rest[amp..];
        let Some(semi) = tail.find(';') else {
            // 无分号的孤立 &：原样保留。
            out.push('&');
            rest = &tail[1..];
            continue;
        };
        let entity = &tail[1..semi];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => {
                if let Some(hex) = entity
                    .strip_prefix("#x")
                    .or_else(|| entity.strip_prefix("#X"))
                    .and_then(|v| u32::from_str_radix(v, 16).ok())
                {
                    char::from_u32(hex)
                } else if let Some(dec) =
                    entity.strip_prefix('#').and_then(|v| v.parse::<u32>().ok())
                {
                    char::from_u32(dec)
                } else {
                    None
                }
            }
        };
        match decoded {
            Some(ch) => out.push(ch),
            None => out.push_str(&tail[..=semi]),
        }
        rest = &rest[amp + semi + 1..];
    }
    out.push_str(rest);
    out
}

// ── docx ───────────────────────────────────────────────────────────────────

fn extract_docx(archive: &mut zip::ZipArchive<std::io::Cursor<&[u8]>>) -> Option<Vec<String>> {
    let xml_bytes = read_entry(archive, "word/document.xml")?;
    let xml = std::str::from_utf8(&xml_bytes).ok()?;
    let mut lines = Vec::new();
    for element in scan_elements(xml, "w:p") {
        lines.extend(paragraph_text_lines(element, "w:t"));
    }
    Some(lines)
}

/// 提取段落元素内文本：`{text_tag}` run 拼接、`<w:tab/>` → 制表符、
/// `<w:br/>` → 拆行。空段落产出一个空行（Word 段落 = 一行）。
fn paragraph_text_lines(element: &str, text_tag: &str) -> Vec<String> {
    let mut lines = vec![String::new()];
    let tab = "<w:tab";
    let br = "<w:br";
    let text_open = format!("<{text_tag}");
    let text_close = format!("</{text_tag}>");
    let mut pos = 0;
    while pos < element.len() {
        // 找下一个感兴趣的结构：tab / br / 文本 run 开标签。前缀同名防护：
        // <w:tab 后必须是边界（排除 <w:tabs> 自定义制表位定义），
        // <w:br 同理（排除 <w:brClr>），text_tag 复用 is_tag_boundary。
        let next_tab = element[pos..]
            .find(tab)
            .map(|o| (pos + o, 0))
            .filter(|(at, _)| is_tag_boundary(&element[*at + tab.len()..]));
        let next_br = element[pos..]
            .find(br)
            .map(|o| (pos + o, 1))
            .filter(|(at, _)| is_tag_boundary(&element[*at + br.len()..]));
        let next_text = element[pos..]
            .find(&text_open)
            .map(|o| (pos + o, 2))
            .filter(|(at, _)| is_tag_boundary(&element[*at + text_open.len()..]));
        let Some((at, which)) = [next_tab, next_br, next_text]
            .into_iter()
            .flatten()
            .min_by_key(|(at, which)| (*at, *which))
        else {
            break;
        };
        match which {
            0 => {
                lines.last_mut().unwrap().push('\t');
                pos = at + tab.len();
            }
            1 => {
                lines.push(String::new());
                pos = at + br.len();
            }
            _ => {
                // 文本 run：<w:t>…</w:t>（可能有属性，如 xml:space="preserve"）。
                let content_start = element[at..]
                    .find('>')
                    .map(|o| at + o + 1)
                    .unwrap_or(element.len());
                if let Some(close_at) = element[content_start..].find(&text_close) {
                    let raw = &element[content_start..content_start + close_at];
                    lines
                        .last_mut()
                        .unwrap()
                        .push_str(&decode_xml_entities(raw));
                    pos = content_start + close_at + text_close.len();
                } else {
                    pos = element.len();
                }
            }
        }
    }
    lines
}

// ── xlsx ───────────────────────────────────────────────────────────────────

fn extract_xlsx(archive: &mut zip::ZipArchive<std::io::Cursor<&[u8]>>) -> Option<Vec<String>> {
    let shared = read_entry(archive, "xl/sharedStrings.xml")
        .and_then(|bytes| std::str::from_utf8(&bytes).ok().map(shared_string_table));
    let mut sheet_names: Vec<String> = archive.file_names().map(str::to_string).collect();
    sheet_names.retain(|name| {
        name.strip_prefix("xl/worksheets/sheet")
            .and_then(|rest| rest.strip_suffix(".xml"))
            .is_some_and(|stem| stem.chars().all(|c| c.is_ascii_digit()))
    });
    // sheet10 必须排在 sheet9 之后：按数字排序而非字典序。
    sheet_names.sort_by_key(|name| {
        name.rsplit("sheet")
            .next()
            .and_then(|stem| stem.strip_suffix(".xml"))
            .and_then(|digits| digits.parse::<u64>().ok())
            .unwrap_or(u64::MAX)
    });
    if sheet_names.is_empty() {
        return None;
    }
    let mut lines = Vec::new();
    for name in &sheet_names {
        let xml_bytes = read_entry(archive, name)?;
        let xml = std::str::from_utf8(&xml_bytes).ok()?;
        for element in scan_elements(xml, "row") {
            lines.push(row_cells_text(element, shared.as_deref()));
        }
    }
    Some(lines)
}

/// 解析 sharedStrings.xml 为字符串表：每个 `<si>` 拼接其内全部 `<t>` run
/// （富文本会把一个字符串拆成多个带样式的 run）。
fn shared_string_table(xml: &str) -> Vec<String> {
    scan_elements(xml, "si")
        .into_iter()
        .map(|element| paragraph_text_lines(element, "t").join(""))
        .collect()
}

/// 一行 `<row>` 的单元格文本：以制表符连接。`t="s"` 查共享表、
/// `t="inlineStr"` 取内联 `<is><t>`、其余（数字/日期/公式值）取 `<v>` 原文。
fn row_cells_text(row_element: &str, shared: Option<&[String]>) -> String {
    let mut cells = Vec::new();
    for element in scan_elements(row_element, "c") {
        let is_shared = element.contains("t=\"s\"");
        let is_inline = element.contains("t=\"inlineStr\"");
        if is_shared
            && let Some(idx) =
                tag_inner_text(element, "v").and_then(|v| v.trim().parse::<usize>().ok())
            && let Some(table) = shared
        {
            cells.push(table.get(idx).cloned().unwrap_or_default());
        } else if is_inline {
            cells.push(
                scan_elements(element, "is")
                    .into_iter()
                    .map(|is_element| paragraph_text_lines(is_element, "t").join(""))
                    .collect::<Vec<_>>()
                    .join(""),
            );
        } else if let Some(value) = tag_inner_text(element, "v") {
            cells.push(decode_xml_entities(value));
        }
    }
    cells.join("\t")
}

/// 取元素内 `<{tag}>…</{tag}>` 的首段文本（无则 None）。
fn tag_inner_text<'a>(element: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut pos = 0;
    while let Some(at) = element[pos..].find(&open) {
        let at = pos + at;
        let after = &element[at + open.len()..];
        if !matches!(
            after.chars().next(),
            Some('>') | Some(' ') | Some('\t') | Some('\r') | Some('\n')
        ) {
            pos = at + open.len();
            continue;
        }
        let content_start = at + open.len() + after.find('>')? + 1;
        let close_at = element[content_start..].find(&close)?;
        return Some(&element[content_start..content_start + close_at]);
    }
    None
}

// ── pptx ───────────────────────────────────────────────────────────────────

fn extract_pptx(archive: &mut zip::ZipArchive<std::io::Cursor<&[u8]>>) -> Option<Vec<String>> {
    let mut slide_names: Vec<(u64, String)> = archive
        .file_names()
        .map(str::to_string)
        .filter_map(|name| {
            let stem = name.strip_prefix("ppt/slides/slide")?;
            let num = stem.strip_suffix(".xml")?.parse::<u64>().ok()?;
            Some((num, name))
        })
        .collect();
    slide_names.sort_by_key(|(num, _)| *num);
    if slide_names.is_empty() {
        return None;
    }
    let mut lines = Vec::new();
    for (num, name) in &slide_names {
        lines.push(format!("── 幻灯片 {num} ──"));
        let xml_bytes = read_entry(archive, name)?;
        let xml = std::str::from_utf8(&xml_bytes).ok()?;
        for element in scan_elements(xml, "a:p") {
            lines.extend(paragraph_text_lines(element, "a:t"));
        }
    }
    Some(lines)
}

#[cfg(test)]
#[path = "../tests/git/office.rs"]
mod tests;
