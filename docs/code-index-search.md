# 代码索引搜索与 AI 使用

桌面符号搜索与 MCP 共用检索逻辑。默认搜索定义符号，精确名称和完整限定名优先，名称字段权重高于路径和文档。关键词还会匹配声明签名、紧邻定义的注释和 Python 文档字符串；函数体不会作为全文搜索内容。中文注释支持连续词组的内部匹配。

多词查询先要求全部词命中，完全无结果时才放宽为任意词匹配。它是关键词检索，不是向量语义搜索，也不会自动翻译中英文。

## MCP 调用顺序

1. `list_projects` 选择仓库，将返回的 `repo` 键或仓库绝对路径用于后续查询。
2. `search_graph` 查找定义；现有 `search_symbols` 仍可使用。
3. 从结果取得 `qualified_name`，传给 `get_code_snippet` 读取定义源码，或传给 `trace_path.function_name` 查看调用关系。旧的 `get_symbol_detail` 继续提供源码与直接调用关系。
4. 对作为依据的文件调用 `check_index_coverage`，检查解析错误范围、排除原因、未解析调用和工作区新鲜度；缺失或部分覆盖时直接核对源码。
5. 搜索返回 `has_more=true` 时，保留其他参数，使用 `offset=next_offset` 与返回的 `generation` 继续翻页。调用链也支持分页，两个方向使用相同 offset 分别翻页，`callers_total` / `callees_total` 是指定深度内当前索引中的可达总数。索引代际变化会明确要求从第一页重查。

查询示例（参数中的仓库路径请替换为目标仓库）：

```json
{"query":"凭据回调","repo":"D:/workspace/CodeX-workspace/khaslana","file_pattern":"src/git/*","limit":20}
```

只知道名称规律时，可以省略 query：

```json
{"name_pattern":"^(push|pull).*","label":"Method","file_pattern":"src/git/*","limit":20}
```

`name_pattern` / `qn_pattern` 是正则表达式；`file_pattern` 是仓库相对路径 glob（支持 `*`、`?`、`[...]`，反斜杠会归一为正斜杠）。这些过滤条件同时生效。`search_symbols` 仍要求 query，`search_graph` 可只传任意一个结构过滤条件。

初始化响应包含工具使用指引；搜索结果提供位置、签名和最多 400 字的文档摘要。源码工具最多读取 200 行，超过时返回 `source.truncated=true`。

MCP 工具及参数说明使用紧凑英文，展示标题保持中文。默认值通过 schema 的 `default` 表达；仓库选择、分页与证据核验的通用流程集中在初始化 `instructions`，工具参数仍保留独立调用所需的格式及约束。工具名、参数名和返回格式不变。

覆盖检查示例：

```json
{"paths":["src/git.rs"],"scopes":["src/git"],"limit":50,"offset":0}
```

`indexed` 表示该文件没有记录的语法缺口；`partial` 提供错误行范围（最多 100 段，截断时有标记）；`read_failed` / `parse_failed` 表示失败；`unsupported` 表示尚不支持该语言；`excluded` 表示明确过滤；`not_indexed` 表示该路径没有记录，需要核对忽略规则或源码。忽略遍历器不会枚举所有被忽略文件，报告是尽力记录，不是完整性证明。`unresolved_calls` 是当前图中无法确定目标的调用点数，也包括外部库和动态调用。

工作区 `metadata_matches` 只核对修改时间和大小；`metadata_changed` / `unavailable` 时源码工具不使用旧行号读取当前文件，避免把错误片段当作定义。

## 更新与查询缓存

MCP 查询按需触发后台增量检查，不依赖“本会话已检查过”的永久标记。不同仓库独立排队，同仓库去重；成功后节流 2 秒，失败后按 2～32 秒退避，后续查询可再次触发。`index_status.refresh` 报告 queued / running / idle、失败次数、最近错误和剩余退避时间。这是访问时检查，不是持续文件监听。

`refresh_index` 可覆盖退避，最多等待后台任务 100 毫秒；尚未完成时返回 `status: "indexing"`，客户端应使用 `index_status` 轮询完成或失败，不要重复刷新。本进程已有同仓库任务时，增量请求复用该任务；全量请求明确报错并返回 `scheduled: false`，需等当前任务结束后重试，避免误将增量结果当作全量重建。独立 MCP 进程、GUI 和提交快照的写入使用数据库旁的 `.lock` 文件互斥；其他进程持锁时快速报错，已有索引仍可读取。锁文件保留，句柄关闭或进程退出即解锁。

失败文件即使修改时间和大小未变也会重试。忽略规则或发现覆盖变化会清除旧结果；取消与解析线程异常不会覆盖已完成的旧索引。整库写入、代际与覆盖汇总在同一事务提交，查询的计数和分页数据来自同一读取快照。

符号详情、调用链、影响分析和架构统计在只读事务中按需查询 SQLite，不再驻留整图缓存。覆盖查询只核对当前页文件的磁盘元数据。

限定调用需要匹配模块或容器；`self` / `this` 等结合调用方作用域解析，同名歧义不任取一个候选。增量刷新保留调用点，定义增删后重新解析未改文件的调用，避免全局唯一性改变后残留旧关系；只重解析变更文件的语法树。

## 内置 AI 评审

评审增加 `search_symbols`、`get_symbol_detail`、`trace_path`、`check_index_coverage` 四个工具，优先定位定义与关系，再通过现有 `read_lines` / `search_code` 补查。结果保留完整 JSON、分页与截断标记，受单次工具预算限制。

索引在首次使用时从评审目标提交的 Git blob 构建，存放于数据目录的 `code-index-snapshots/<仓库键>/<提交 OID>/index.db`。同提交复用快照；查询和源码均固定于 `target_commit`，不会切换分支或改变工作区，也不会混入当前 HEAD 或未提交修改。快照与 GUI / MCP 的工作区索引分开，建库共用索引专用任务池。快照文件会留在数据目录供后续评审复用。

## 旧索引升级

v2 索引升级为 v3 时，在事务中重建全文表，保留原有节点、关系、文件信息和仓库元数据；失败会回滚。只读 MCP 查询仍能读取 v2 库。下一次索引刷新会补做一次全量提取以采集签名、文档、作用域、覆盖与调用点记录，之后恢复增量更新流程。

当前数据库 schema 保持 v4，索引内容版本为 v5，用于补全目录祖先节点和清除历史结构残留。内容版本较旧的工作区索引仍可读取，下次刷新事务重建一次，取消或失败保留旧图；提交快照在下次使用时按同一内容版本补建。此后的增量刷新同步维护当前分支与有效目录，仅切换分支不会重解析未改文件。

更换程序后，需要重启挂载它的 MCP 客户端，才能重新加载工具清单。若结果过期，可调用 `refresh_index`；`index_status.indexing` 表示当前仓库是否正在建立或刷新。

## 外部 MCP 连接约定

`khaslana mcp` 使用 stdio，以 UTF-8 换行 JSON 收发消息，并保留逐帧识别 `Content-Length` 的兼容方式；诊断只写 stderr。单帧上限为 10 MiB，兼容帧头部上限为 16 KiB。超限、长度无效或不完整的长度帧会关闭连接，防止剩余正文被误执行为下一条请求；无效 UTF-8 返回解析错误。输出管道断开后停止读取。

工具参数按 `tools/list` 中的 schema 验证。未知工具、缺必填项、类型或枚举错误返回 JSON-RPC `-32602`；工具执行失败仍返回 `result.isError: true`。合法通知没有响应。仓库路径统一解析到 Git 工作区根目录，因此 `.git` 与工作区根使用同一个索引；裸仓库不支持工作区索引。单仓库启动模式仍固定使用启动时的仓库。

## 能力边界

索引仍使用现有 tree-sitter 与启发式调用解析，动态调用、复杂类型推导和部分语法可能遗漏。无搜索结果不能证明代码不存在，调用总数只描述当前图。参考项目的 LSP 类型分析、向量搜索和 Cypher 查询没有在这次改动中移植。

全文索引与协议实现依据 [SQLite FTS5 文档](https://sqlite.org/fts5.html)、[MCP 工具规范](https://modelcontextprotocol.io/specification/2025-06-18/server/tools)及[初始化规范](https://modelcontextprotocol.io/specification/2025-06-18/basic/lifecycle)。
