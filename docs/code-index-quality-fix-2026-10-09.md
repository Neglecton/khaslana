# 诊断报告对应的索引质量修正（2026-10-09）

依据：[原始诊断报告](khaslana-code-index-diagnosis.md)。原报告保留，不改写用户的实测记录。

## 根因与修正

| 原报告 | 已在当前源码确认的原因 | 修正与验证 |
| --- | --- | --- |
| P1 同名方法错误归属 | Java method_invocation 只取 name，丢弃 object；Java 裸方法能走全仓库唯一方法兜底，且没有类作用域和私有可见性守卫 | 保留 this/Integer/变量等原始接收者；this 仅匹配当前文件的当前类；裸方法只匹配当前类或明确静态导入。限定静态调用匹配类身份与私有可见性，无法确定的实例接收者不猜测。普通类导入与静态成员导入分开 |
| P2 Java 连到 JS | 名字注册表没有候选语言隔离，Java 对象创建的 HashMap 能命中 JS 全局唯一函数 | 调用和类型关系先检查语言；JS/TS/TSX、C/C++ 各自允许互操作，Java 与 JS 不互连。测试保留 TS 导入 JS 的真实调用 |
| P3 Java 全在 other | 文件节点名称由 rsplit('/').next_back() 取得，实际取了首段目录；架构统计又从 name 取扩展名 | File.name 改用 basename，语言统计直接从 file_path 取扩展名。目录中的 Java/JS 样例分别正确统计为 java/js |
| P4 测试折叠到文件节点 | 顶层匿名回调没有函数归属，回退文件节点；CALLS 按调用者/被调用者去重，又没有保存各调用点行号 | 顶层 JS/TS 匿名回调建立 callback@行:列 节点；直接边增加 call_sites.count/file_path/lines/truncated。节点数仍去重，不把调用次数伪装成 callers_total；每条边最多返回 100 个行号，次数不截断 |
| P5 unresolved 数量难以解释 | 只给无法确定目标的总数，无仓库候选与有候选但无法安全匹配混在一起 | 增加 unresolved_no_candidate 与 unresolved_with_candidates；保留原总数，明确不是语法解析失败率。无候选可能是外部库，不能据此断言全部为外部调用 |

命名函数内部的闭包继续归属外层函数，保留原报告 §8 这类调用图语义。CALLS 边仍包含 confidence/strategy，原始调用表达式保留；新增位置证据不会把多跳关系误当作直接调用证据。

## 旧数据升级

数据库 schema 仍为 v4，内容版本升到 v6。旧库定义搜索可以读取；调用链、含关系的符号详情、架构热点及符号影响分析拒绝历史关系，并提示先重建。index_status.needs_rebuild 显示升级需求。

下一次后台检查或 refresh_index 会对旧内容做一次事务全量重建，包括未修改源码的调用记录，避免错误边长期残留。后续恢复增量。取消或失败保留旧库，旧关系仍不会作为当前证据输出；提交快照按同一内容版本重建。

调用记录编码从 KCALL1 升至 KCALL2，增加行号；读取仍兼容 KCALL1，旧记录的未知行号为 0。增量重新解析未修改调用文件时，行号和真实调用次数保留。

## 验证

- 首批 5 项针对原报告的复现测试在旧实现全部失败，修正后全部通过。
- 本轮新增 11 项回归：Java 接收者与私有方法、隐式当前类调用及限定静态调用、跨语言隔离与 TS→JS 导入、文件名和语言构成、顶层测试回调、120 次重复调用的精确次数与有界行号及增量恢复、未解析分类、具体/通配静态导入、v5 污染清理与一次升级、调用记录编码兼容、本地重载歧义不转移到导入的同名方法。最后一项亦先在原策略上复现失败，再修正通过。
- 更新旧库测试，验证定义搜索可继续使用，旧关系须升级后才能查询。
- cargo check --all-targets：通过，无警告。
- cargo test --lib --bin khaslana --quiet：1009 项通过（lib 639、bin 370），7 项忽略，0 失败。随后消除新代码逐符号临时 HashSet 分配，再次通过全目标检查和索引模块 99 项测试（3 项忽略）。
- 默认 cargo build --release：通过，零错误、零警告。
- 使用该 release 产物在临时便携目录启动真实 stdio 进程，以 6 文件混合仓库验证完整建库与查询：java=3、ts=2、js=1；私有方法错误调用者为 0；Java→JS 错误调用者为 0；真实限定静态调用为 1；2 个测试回调分别记录 2 次、1 次调用；未解析分类为 1 个无候选、2 个有候选。随后注入一条错误跨语言边并把内容版本改为 v5，重启后自动升级，代际变化且错误边清除，needs_rebuild=false。进程及验证脚本最终退出码均为 0，不使用真实用户索引。
- 冒烟脚本第一次在断言全部通过后，因自身 sqlite3 连接未关闭而无法清理临时目录；关闭脚本连接后完整重跑通过，非产品异常。
- 首次运行的残留目录 C:/Users/chen/AppData/Local/Temp/khaslana-mcp-quality-wnoev6un 的显式清理命令被自动审批以 blocked by policy 拒绝，目录暂时保留；重跑创建的目录已正常清理。
- git diff --check：通过，只有 Git 的 LF/CRLF 转换提示。

代码发现采用结构图定位、调用关系与覆盖检查；涉及文件的覆盖新鲜度提示 metadata_changed，相关结论回读当前源码验证，不依赖旧图结果。范围仅限此次诊断涉及的索引质量问题，不声称完整语义解析。

## 使用与验证边界

原报告的 D:/deepseek-harness 和 D:/workspace/bcs_utf8 在当前机器不可用，未实测原 Java 大仓库或原 TS 包的修复后 fan-in 数值，也未重新进行模型工具选择实验。实例变量的类型推导、继承自仓库外基类的方法、动态派发和重载仍可能无法解析；此时保留未解析证据，不任取同名方法补边。

原 MCP 命令为 D:/khaslana/khaslana.exe mcp。源码构建不会替换该安装路径；使用新程序后需重启 MCP，再等待 needs_rebuild=false。对于大仓库，refresh_index 返回 indexing 后用 index_status 轮询，不要重复刷新。完整调用约定见[使用文档](code-index-search.md)。

## Java 项目与 rg 对照补充

使用现有 release 的真实 MCP 进程，对含 7 个 Java 源文件和 pom.xml 的临时 Git 仓库建库，再用 rg --json 直接检索源码对照。执行脚本：[verify-code-index-java-grep.py](../scripts/verify-code-index-java-grep.py)，命令为 `python scripts/verify-code-index-java-grep.py`；可用 `--exe` 指定待验程序。数据仍使用临时便携目录，不操作真实用户索引。

| 对照项 | rg / 作用域核对 | MCP 索引 |
| --- | ---: | ---: |
| Java 文件 | 7 | 7，语言为 java；pom.xml 单列 xml |
| 类、接口、枚举定义 | 8 | 8，名称、文件、行号逐项一致 |
| 方法定义 | 10 | 10，名称、文件、行号逐项一致 |
| Tools.parseInt 直接调用点 | 2（Caller.java 第 5、6 行） | call_sites.count=2，行号一致；去重调用者为 1 |
| 静态导入 mappingDataSet 调用 | 1 | 1 |
| PrivateOwner 私有 newResponseContext 合规调用 | 0；其他类的 this 同名调用不属于它 | 0 个错误调用者 |
| 同文件 local 裸调用 | rg 命中 2；按类作用域只有 1 处属于目标私有方法 | 仅连接 invoke，次数和行号一致 |

样例包含注释和字符串中的 Tools.parseInt 文本，未当成实际调用。脚本完整执行退出码为 0，needs_rebuild=false。

此处的 rg 正则针对已知样例，不是通用 Java 语义分析器；跨类私有调用由样例作用域核对，不能把裸名称命中数当调用图真值。原 bcs_utf8 大仓库仍不在当前机器，未执行原库的全量 grep 对照。实例接收者类型、外部继承、动态派发和重载等限制依旧适用。
