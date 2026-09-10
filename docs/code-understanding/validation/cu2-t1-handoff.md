# CU2-T1 六个只读工具交接

日期：2026-09-10。状态：完成；T2 agent 尚未实现。

## 实际实现

- 新增 `src/code_understanding/source.rs`：每问固定允许文件清单，复用
  `discover_files` 的 gitignore/固定排除规则；提供 UTF-8 文本读取、源码检索、
  有界目录树、来源 ID 发放和点击前 hash 复验。
- 新增 `src/code_understanding/tools.rs`：实现 `search_symbols`、`get_symbol`、
  `trace_calls`、`search_code`、`read_file`、`get_file_tree` 六工具 Rust 适配及
  `ToolSchema`。索引工具复用现有查询层，不启动 MCP 子进程。
- 符号候选使用本会话 `sc1:` ID；`get_symbol/trace_calls` 不接受模型自行填写
  qualified name。源码引用使用本会话 `sr1:` ID；校验只接受本会话已发放来源。
- 每个响应携带 request_id、index_generation、truncated 与原因。六工具每次执行
  前复核索引代际，代际变化后拒绝继续。
- `get_symbol` 会比较当前源码 hash 与索引发布 hash；不把旧定义位置和新文件内容
  拼为确定证据。直接 `search_code/read_file` 仍可用于重新定位当前源码。

## 运行边界

- 文件限制在 canonical 项目根和发现 allowlist 内；拒绝 `..`、绝对路径、
  `.git`/构建目录、根外符号链接或 junction、二进制和大于 1 MiB 的文件。
- `read_file` 最多 200 行、8K 字符；源码引用绑定实际字节范围和 SHA-256。
- `search_code` 最多扫描 1000 个候选文件、2 秒截止、最多 50 条；支持受限 regex、
  目录前缀和后缀过滤，XML/SQL 无需先有符号节点。
- `get_file_tree` 最多 4 层、200 条；`trace_calls` 最多 3 跳、40 个符号。
- trace 明示“索引线索并非完整调用证明”；search_code 命中也明示关键结论必须
  再 read_file，避免把相邻方法中的 SQL 错绑到业务路径。

## 测试与检查

- `cargo test --lib code_understanding --no-fail-fast`：26 passed，0 failed，
  578 filtered out。
- `cargo test --lib --no-fail-fast`：602 passed，0 failed，2 ignored。
- `cargo check --all-targets`：成功，零编译警告。
- `git diff --check`：通过；仅已有工作区 LF→CRLF 提示。
- `cargo clippy --lib`：成功；本批 `code_understanding` 无 clippy 提示。仓库在
  `-D warnings` 下仍有既存 clippy 问题，位于 AI client、Java 未提交实现、
  credentials、Git 等非本批代码，本批未越界修改。

新增测试覆盖：来源发放/伪造拒绝/hash 失效、路径穿越、`.git`、根外链接、
二进制/大文件、200 行限制、非索引 XML 搜索、目录树、六工具集成、候选 ID、
索引文件版本混用拒绝和索引代际变化。

## 已知限制与下一步

- 符号搜索的 path/language 范围过滤当前基于最多 1000 条 FTS 候选；超出会显式
  truncated。真实项目证明不够时再把过滤下沉 SQL，不先重写查询引擎。
- 当前 source reader 只接受 UTF-8；遇到其他编码返回 `EncodingUnsupported`，不做
  有损证据引用。
- T1 只交付工具与来源注册表，尚未实现 40 次工具/41 轮、HTTP 尝试与累计上下文
  的 agent 总预算；这些属于 CU2-T2。
- 下一步：新增 `agent.rs`，复用 `ChatClient::request_agent_stream` 和 review agent
  的循环/重试模式，用 B01 固定问题完成无 UI 的真实登录业务分析。随后在 T3
  新增独立 `AnalysisResult` 与表读写/步骤结构校验。
