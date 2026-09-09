# 代码理解验收样例与标注格式（DEV-00 建立基线）

本目录是「代码理解与逻辑可视化」功能的两套人工标注样例：`java/`（普通 Java，J01～J12）与 `spring/`（Spring Boot，S01～S12）。对应 [development.md](../../../../docs/code-understanding/development.md) §5 与 [requirements.md](../../../../docs/code-understanding/requirements.md) §6。

## 约束

- 样例是可分发的最小手写源码，**不需要** Maven/Gradle 构建、网络或数据库；J09 的 `pom.xml` 只作为静态解析输入。
- 索引测试读取本目录的**临时副本**（复制到 `tempdir` 再建索引），不得写回本目录。
- 普通 Java 与 Spring 两套分别维护，不用一个大样例冒充所有场景。
- 禁止任何文件携带 `.git`；J11 显式验证无 `.git`、无构建文件的普通目录闭环。
- Java 语法按 `tree-sitter-java 0.23.5` 固定（升级须重跑 `cargo test --lib java_grammar_probe`）。

## expected.json 标注格式

每套一份 `expected.json`（`java/expected.json`、`spring/expected.json`）。其中顶层 `question_bank` 是 20 题的唯一机器数据源，题号必须严格为 JQ01～JQ20 或 SQ01～SQ20；`validation/question-bank.md` 必须与它逐题同步。各 group 只通过 `question_ids` 引用题目，避免重复内容漂移；若保留兼容字段 `question_bank_ids`，它必须与 canonical 题号集合完全相等。顶层结构：

```jsonc
{
  "suite": "java | spring",
  "grammar_baseline": "tree-sitter-java 0.23.5",
  "question_bank": [ /* 20 个 {id, group, q, allowed_points} */ ],
  "groups": [ /* 见下 */ ]
}
```

每组（group）字段：

| 字段 | 说明 |
| --- | --- |
| `id` | J01～J12 / S01～S12，与 development.md §5 表格一一对应 |
| `scenario` | 场景一句话描述 |
| `files` | 组内相对路径列表（相对本目录） |
| `symbols` | 期望提取的符号声明，用**规范符号引用**描述（见下） |
| `assertions` | 断言数组：调用/关系 → 目标/候选 + certainty |
| `diagnostics` | 应产生的诊断（未解析调用、解析 ERROR、覆盖缺口等），可为空数组 |
| `entry_points` | 框架入口（路由/持久化声明等），普通 Java 组通常为空 |
| `question_ids` | 对顶层 canonical `question_bank` 的稳定题号引用；不得重复或跨 group |

### 规范符号引用（SymbolRef 文本协议）

标注**不得**硬编码数据库行号或生成期随机 ID；符号以人类可读的规范文本描述，由测试代码映射到实际 SymbolKey：

- 类型：`<package>.<外层类型>[.<嵌套类型>]`，如 `com.example.alpha.OrderService`、`...OrderService.Item`
- 方法：`<类型引用>#<方法名>(<参数类型,参数类型>)`，如 `com.example.alpha.OrderService#cancel(String)`
- 构造器：`<类型引用>#<init>(<参数类型>)`
- 参数类型可解析时用 FQN 或语言字面量（`int`、`String`）；不可解析时保留原文并以 `?` 前缀标记不完整，如 `?com.external.Thing`
- Mapper XML statement：`<mapper namespace>#<statement id>`，如 `com.example.shop.mapper.InventoryMapper#release`；资源自身可用 `source_set`/`module` 定位
- 路由：`HTTP <method> <path描述>`，如 `HTTP GET /api/orders/{id}`
- 多模块/源集：规范对象字段使用 `module` 与 `source_set`，不混进符号名称；例如 `module=app-core` + `source_set=test` + `com.example.core.CoreRunnerTest`。旧 J09 的 `module:app-core` 文本前缀仅为兼容性展示，后续实现映射时必须拆成独立字段。

### certainty 取值（对齐设计文档 §4.4）

- `syntactic`：源码声明/表达式可证实（声明调用、继承声明、注解字面量）。
- `inferred`：有限静态规则推断（框架候选、限定缩小），断言须附 `rule` 说明。
- `unknown`：保留歧义/未解析，断言写成候选集合或 `unresolved`，**禁止**写唯一目标。

`assertions` 元素形态：

```jsonc
{
  "kind": "calls | dispatch_candidates | overrides | inherits | implements | injects_candidate | handles | maps_to | annotations | statements | selects_constructor",
  "source": { "owner": "<规范符号引用>", "path": "<组内相对路径>", "anchor": "<文件内精确源码子串>", "callee": "<调用原文>", "occurrence": 1 },
  "target": "<规范符号引用>",
  "candidates": ["<规范符号引用>"],
  "certainty": "syntactic | inferred | unknown",
  "rule": "spring.qualifier 等规则 id（inferred 必填）",
  "note": "补充说明"
}
```

`source` 是每条 assertion 必填的可执行定位字段：必须有 `owner`、`path`、非空 `anchor`、正整数 `occurrence`，并提供 `callee` 或 `expression`。`anchor` 必须是 `source.path` 文件内真实、精确的源码子串；manifest 按非重叠出现次数校验其至少覆盖 `occurrence` 次，重复调用用 `occurrence` 区分。语句结构用 `kind=statements`，注解声明用 `kind=annotations`，构造器选择用 `kind=selects_constructor`，不得再用 `kind=calls` 代替。旧 `call_site` 文本仅作人读兼容，测试不得依赖它。

一致性回归（C01～C14）的期望变化也以本文件为基准做差分（修改 fixture 副本 → 重建 → 比较断言结果，而非比较数据库行）。
