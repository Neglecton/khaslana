// 文件追溯（blame）相关的领域类型。
//
// 「文件追溯」视图逐行展示文件内容的归属提交：每个追溯块（hunk）对应
// 一段连续同源行，携带该段最后修改它的提交信息；工作区中未提交的改动行
// 不属于任何提交，`commit` 为 None 并以「未提交」徽标展示。

use crate::types::DiffEncodingChoice;

/// 追溯块所属提交的展示信息。
///
/// owned 结构，可跨线程从后台 Git 任务传回 UI 线程；
/// summary/作者名在服务层用字节读取 + 有损转换填充。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlameCommitInfo {
    pub oid: String,
    pub short_oid: String,
    pub author: String,
    /// Unix 秒。
    pub time: i64,
    pub summary: String,
}

/// 追溯块：文件中一段连续同源行。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlameHunkInfo {
    /// 该段行的归属提交；None 表示工作区未提交的行（blame_buffer 差异行）。
    pub commit: Option<BlameCommitInfo>,
    /// 块首行行号（1 基）。
    pub start_line: usize,
    /// 块内行数。
    pub line_count: usize,
}

/// 追溯视图的完整数据。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlameView {
    /// git 风格相对路径。
    pub path: String,
    /// 解码后的文件行（与追溯块行号同一坐标系）。
    pub lines: Vec<String>,
    pub hunks: Vec<BlameHunkInfo>,
    /// 行索引（0 基）-> hunk 索引，渲染时按行查所属块。
    pub line_hunk: Vec<usize>,
    /// 实际使用的编码（检测或用户手动选择后的结果）。
    pub encoding: DiffEncodingChoice,
}
