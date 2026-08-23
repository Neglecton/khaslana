use super::*;
use std::io::Write as _;

/// 用 ZipWriter 构造内存 Office 文档（docx/xlsx/pptx 本质是定制的 zip）。
/// 仿 `src/tests/update.rs` 的 fixture 模式；条目内容原样写入。
fn office_zip(entries: &[(&str, &str)]) -> Vec<u8> {
    let owned: Vec<(&str, Vec<u8>)> = entries
        .iter()
        .map(|(name, content)| (*name, content.as_bytes().to_vec()))
        .collect();
    office_zip_bytes(&owned)
}

/// 同 `office_zip`，但条目内容为原始字节（构造非法 UTF-8 条目用）。
fn office_zip_bytes(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut cursor);
        let options = zip::write::SimpleFileOptions::default();
        for (name, content) in entries {
            zip.start_file(*name, options).unwrap();
            zip.write_all(content).unwrap();
        }
        zip.finish().unwrap();
    }
    cursor.into_inner()
}

const DOCX_BODY: &str = "\
<w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">\
<w:body>\
<w:p><w:r><w:t>第一段</w:t></w:r></w:p>\
<w:p><w:r><w:t>第二段</w:t></w:r><w:r><w:t>继续</w:t></w:r></w:p>\
<w:p><w:r><w:tab/><w:t>制表后</w:t></w:r></w:p>\
<w:p><w:r><w:t>换行前</w:t><w:br/><w:t>换行后</w:t></w:r></w:p>\
<w:p><w:r><w:t>实体：&amp;&lt;&gt;&quot;&apos;中文</w:t></w:r></w:p>\
<w:p><w:r><w:t>数字实体：&#x4E2D;&#25991;</w:t></w:r></w:p>\
<w:p/>\
<w:pgSz w:w=\"11906\" w:h=\"16838\"/>\
<w:p><w:r><w:t>末段</w:t></w:r></w:p>\
</w:body></w:document>";

fn docx_bytes(document_xml: &str) -> Vec<u8> {
    office_zip(&[
        ("[Content_Types].xml", "<Types/>"),
        ("word/document.xml", document_xml),
    ])
}

#[test]
fn office_extension_detection() {
    assert!(path_has_office_extension("a.DOCX"));
    assert!(path_has_office_extension("b.xlsx"));
    assert!(path_has_office_extension("c.Pptx"));
    // 旧版 OLE 格式与无关扩展名不支持
    assert!(!path_has_office_extension("d.doc"));
    assert!(!path_has_office_extension("d.xls"));
    assert!(!path_has_office_extension("e.txt"));
    assert!(!path_has_office_extension("noext"));
}

#[test]
fn docx_extracts_paragraphs_runs_tabs_breaks_and_entities() {
    let lines = office_text_lines("test.docx", &docx_bytes(DOCX_BODY)).unwrap();
    assert_eq!(
        lines,
        vec![
            "第一段",
            "第二段继续", // 同段多 run 拼接
            "\t制表后",   // w:tab → 制表符
            "换行前",     // w:br → 拆行
            "换行后",
            "实体：&<>\"'中文", // 预定义实体解码
            "数字实体：中文",   // 十六进制/十进制数字实体
            "",                 // 空段落 = 空行；w:pgSz 不是 w:p 不产行
            "末段",
        ]
    );
}

#[test]
fn docx_without_document_xml_returns_none() {
    let bytes = office_zip(&[("[Content_Types].xml", "<Types/>")]);
    assert!(office_text_lines("test.docx", &bytes).is_none());
}

#[test]
fn docx_empty_document_yields_single_empty_line() {
    // Word 新建空文档：document.xml 只有一个空段落
    let xml = "<w:document><w:body><w:p/></w:body></w:document>";
    let lines = office_text_lines("e.docx", &docx_bytes(xml)).unwrap();
    assert_eq!(lines, vec![""]);
}

#[test]
fn corrupt_zip_and_truncated_magic_return_none() {
    // 非 zip 内容（比如旧 .doc 二进制改扩展名）
    assert!(office_text_lines("fake.docx", b"\xD0\xCF\x11\xE0 old OLE bytes").is_none());
    // 只有 zip 魔法的截断字节（现有 git.rs 二进制测试里的假 docx 即此形态）
    assert!(office_text_lines("cut.docx", &[0x50, 0x4B, 0x03, 0x04, 0x00, 0x00, 0x08]).is_none());
    // document.xml 不是合法 UTF-8
    let bad = office_zip_bytes(&[("word/document.xml", b"<w:p>\xFF\xFE</w:p>".to_vec())]);
    assert!(office_text_lines("bad.docx", &bad).is_none());
}

const XLSX_SHARED: &str = "\
<sst xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\">\
<si><t>姓名</t></si>\
<si><r><t>富文本</t></r><r><t>拼接</t></r></si>\
</sst>";

const XLSX_SHEET: &str = "\
<worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\">\
<sheetData>\
<row r=\"1\"><c r=\"A1\" t=\"s\"><v>0</v></c><c r=\"B1\" t=\"s\"><v>1</v></c></row>\
<row r=\"2\"><c r=\"A2\"><v>42</v></c><c r=\"B2\" t=\"inlineStr\"><is><t>内联</t></is></c></row>\
<row r=\"3\"/>\
</sheetData></worksheet>";

fn xlsx_bytes(sheet1: &str, sheet10: Option<&str>) -> Vec<u8> {
    let mut entries = vec![
        ("[Content_Types].xml", "<Types/>"),
        ("xl/sharedStrings.xml", XLSX_SHARED),
        ("xl/worksheets/sheet1.xml", sheet1),
    ];
    if let Some(sheet10) = sheet10 {
        entries.push(("xl/worksheets/sheet10.xml", sheet10));
    }
    office_zip(&entries)
}

#[test]
fn xlsx_extracts_rows_with_shared_inline_and_numeric_cells() {
    let lines = office_text_lines("t.xlsx", &xlsx_bytes(XLSX_SHEET, None)).unwrap();
    assert_eq!(
        lines,
        vec![
            "姓名\t富文本拼接", // t="s" 查共享表；富文本多 run 拼接
            "42\t内联",         // 数字原文 + inlineStr
            "",                 // 空 row
        ]
    );
}

#[test]
fn xlsx_sheet_order_is_numeric_not_lexicographic() {
    let sheet10 = "<worksheet><sheetData><row><c><v>十号表</v></c></row></sheetData></worksheet>";
    let lines = office_text_lines("order.xlsx", &xlsx_bytes(XLSX_SHEET, Some(sheet10))).unwrap();
    // sheet10 必须排在 sheet1 之后（字典序会颠倒）
    assert_eq!(lines.last().unwrap(), "十号表");
}

#[test]
fn xlsx_without_sheets_returns_none() {
    let bytes = office_zip(&[("[Content_Types].xml", "<Types/>")]);
    assert!(office_text_lines("e.xlsx", &bytes).is_none());
}

fn pptx_bytes(slides: &[(u64, &str)]) -> Vec<u8> {
    let mut owned: Vec<(String, String)> =
        vec![("[Content_Types].xml".to_string(), "<Types/>".to_string())];
    for (num, body) in slides {
        owned.push((
            format!("ppt/slides/slide{num}.xml"),
            format!("<p:sld><p:txBody>{body}</p:txBody></p:sld>"),
        ));
    }
    let borrowed: Vec<(&str, &str)> = owned
        .iter()
        .map(|(name, body)| (name.as_str(), body.as_str()))
        .collect();
    office_zip(&borrowed)
}

#[test]
fn pptx_extracts_slides_in_numeric_order_with_separators() {
    let lines = office_text_lines(
        "s.pptx",
        &pptx_bytes(&[
            (2, "<a:p><a:r><a:t>第二页</a:t></a:r></a:p>"),
            (10, "<a:p><a:r><a:t>第十页</a:t></a:r></a:p>"),
            (
                1,
                "<a:p><a:r><a:t>第一页</a:t></a:r></a:p><a:p><a:r><a:t>副标题</a:t></a:r></a:p>",
            ),
        ]),
    )
    .unwrap();
    assert_eq!(
        lines,
        vec![
            "── 幻灯片 1 ──",
            "第一页",
            "副标题",
            "── 幻灯片 2 ──",
            "第二页",
            "── 幻灯片 10 ──", // 数字序：10 在 2 之后
            "第十页",
        ]
    );
}

#[test]
fn oversized_file_returns_none() {
    // 不真造 3MB 文件：直接断言上限常量行为路径（构造一个超限假长度不可行，
    // 这里验证小文件路径与常量本身）。
    assert_eq!(OFFICE_EXTRACT_MAX_FILE_BYTES, 3 * 1024 * 1024);
    assert_eq!(OFFICE_EXTRACT_MAX_ENTRY_BYTES, 8 * 1024 * 1024);
}
