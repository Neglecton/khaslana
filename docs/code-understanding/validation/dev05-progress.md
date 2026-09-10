# DEV-05 进度记录

状态：进行中，普通 Java 调用断言（含外部边界）与语义关系断言均已严格验收，DEV-05 整体未达验收条件（2026-09-10）。

## 本轮实现

基础批次（重载选择、显式导入遮蔽、接口分派分离、匿名类身份、局部类外层方法查找）之上，本轮补齐：

- **方法引用调用点**：`lang_spec.rs` 新增 `CallNameStrategy::ScopeAndName`，Java call 表加入 `method_reference`（`this::run`、`Refs::value`），receiver 取首个非 type_arguments 子节点、方法名取末个 identifier；参数列表来自函数式接口静态不可知，按空参数保守匹配。
- **字段访问调用点**：Java call 表加入 `field_access`（`self.name`），object/field 字段与 method_invocation 同构；解析层限定只绑定字段声明，同名方法不参与，避免「字段 name + 方法 name()」把访问点误判歧义；字段访问不参与动态分派。**体积代价**：所有 `a.b` 表达式（含 `System.out`、枚举常量、静态常量）都生成调用点 + evidence + relation，Java 项目的 call_sites/evidence/relations 体积与索引时间接近翻倍——这是为 J05 字段断言付出的范围扩展，性能验收时需复核。
- **record 模式变量**：`java/extract.rs` 新增 `extract_pattern_variable`，处理 `type_pattern`（`instanceof Err e`、`case Item item`）与 `record_pattern`（`instanceof Ok(Item item)`，组件在 `record_pattern_body` 的 `record_pattern_component`，嵌套递归），登记为 `kind="pattern"` 变量。**作用域经审查修复**：switch case 模式的流作用域止于所在分支（箭头式 `switch_rule`、冒号式 `switch_block_statement_group`），instanceof 场景沿用词法块（if 条件的 pattern 对 then 块可见）——此前一律上溯方法体会把非法跨 case 引用放宽成可解析。
- **类型表达式剥泛型**：`resolve.rs` 的 `type_name` 剥掉 `<...>` 实参（`List<String>`、`new ArrayList<>` 按原始类型解析），不再因泛型直接返回 None；菱形与通配导入构造（`import java.util.*` 下 `new ArrayList<>()`）正确判定 external。
- **唯一通配导入的外部类型标记**：类型简单名仅命中一个通配导入且不在索引内时，返回 `?包名.类型` 外部标记名（位于包限定分支之后，包限定名不会被拼接出错误前缀）；成员访问据此标 external 而非 unresolved（J05 的 `list.add` 等）。
- **词法局部类构造**：`lexical_type` 按文件内简单名 + 声明早于调用点 + 宿主 callable 同时包含声明与调用点定位局部类（`new LocalHelper()`），跨方法不隐式可见；宿主检查使用预建的 per-file callable 索引（`file_callables`），避免每调用点全表扫描。
- **隐式默认构造器合成**：`java/extract.rs` 的 `synthesize_default_constructors` 为无显式构造器的类合成 `<init>()` 符号（stability="synthetic"），可见性拷贝类声明的访问修饰符（public/protected，包私有类保持包私有）。**合成范围经审查收窄**：只对 `class`——record 的规范构造器带全部组件参数（无参形态是错误语义，`new Item("a")` 保持保守 unresolved），enum 不可 new，匿名类不可被显式构造均不合成。合成在提取层完成，通用 defs → nodes → 实体链路一致，避免 DB FK 失败。
- **匿名类 owner_chain 补宿主方法段**：匿名类符号与其成员的 owner 链都含宿主方法（`wire()`），与局部类规则一致，匿名类内递归调用指向自身恢复。
- **facts_json 调用点补 `kind` 键**：pipeline 落盘的调用点事实包含节点 kind（method_invocation / field_access / method_reference 等），事实层可区分访问形态。**注意**：该键改变了事实 JSON 形态，且 `stability` 引入 `synthetic` 第三取值——当前靠「含 Java 增量转全量」兜底新旧混存，DEV-07 恢复增量时需配套适配器/事实版本号。
- **含 Java 的增量执行暂时转全量重建**（基础批次引入，本轮保留）。

## 验收范围扩展（2026-09-10 第二轮）

语义关系与边界断言纳入题库严格验收，测试层新增/扩展（解析器零改动，验收前能力已就绪）：

- **语义关系断言验收**：新测试 `dev05_fixture_semantic_relations_bind_exactly` 对 expected.json 全部 7 条语义断言逐条核对——implements×3 / inherits×1 / overrides×2 要求 relations 表存在 kind 对应、certainty=syntactic、resolution_state=resolved 的关系，source/target 语义 FQN 与题库完全相等，且关系证据（relation_evidence → evidence.refs_json）的源码区间覆盖断言锚点；dispatch_candidates×1 要求调用点先 resolved 且分派候选集合恰等于题库候选（无缺失、无多余、certainty=inferred）。
- **语义 FQN 重构而非 QN 匹配**：节点 QN 携带文件路径段（`project.J03.…UserRepository.java.UserRepository.findById(int)`），与题库 FQN（`com.example.repo.UserRepository#findById(int)`）不同构，无法后缀匹配；验收工具改从 `nodes.properties` 的 `java.package + owner_chain + name + signature` 重构语义 FQN（方法用 `#` 分隔、类型用 `.`），与题库直接字符串相等。
- **J09 跨模块隔离验收**：题库 `module:xxx` 前缀要求 target 节点的 `java.module_key` 与之相等——两个模块的同名接口 `com.example.core.Named` 是不同符号，implements 关系不得跨模块串并。
- **unknown 边界断言纳入**：calls 验收从「仅 syntactic」扩展为「syntactic + unknown」——J06 的 Stream 链中段（`certainty=unknown, candidates=[]`）预期 unresolved/external（不虚构外部 API 语义），34/34 通过。
- **锚点主调用名精确匹配**：锚点先剥掉平衡括号内容再取最后标识符（`filter(n -> !n.isEmpty())` 的主调用是 filter 而非 lambda 体内的 isEmpty、`helper().join()` 是 join），命中失败再回退原有「标识符集合 + 区间交集」规则——避免嵌套调用误命中内层。
- **防回退计数**：两个验收测试独立重算题库中应纳入的断言数与实际处理数比对，过滤条件回归跳类时直接报错。

## 审查与修复记录（2026-09-10）

对本轮改动做了一轮代码审查，按发现的问题修复：

- **P1 switch 模式变量跨 case 泄漏（高）**：case 间互不可见的模式变量此前被解析为确定绑定（伪确定），违反「歧义用例唯一误判为 0」精神。修复：`pattern_scope` 识别 `switch_rule` / `switch_block_statement_group` 边界；回归测试验证「本 case 内合法引用 resolved、跨 case 非法引用 unresolved」。
- **P2/P3 合成构造器范围与可见性（中）**：record/enum/annotation/匿名类不再合成无参构造器；modifiers 从硬编码 public 改为拷贝类声明访问修饰符（包私有类不再被跨包伪可解析）。回归测试验证 record 规范构造不伪解析、匿名类零合成。
- **P5 重复/垃圾逻辑（低）**：`type_name` 尾部改为「通配索引内命中 → 包限定 → 唯一通配索引外 `?` 兜底」的顺序，`new java.util.ArrayList<>()` 不再产生 `?java.util.java.util.…` 垃圾限定名；删除不可达的 `external_constructor`（其职责由 type_name 兜底与 type_id None 分支承担）。
- **P6 验收工具命中语义（中）**：命中排序从「start 降序优先」改为「end 最大（最外层调用）优先」，与链式/嵌套形态的断言语义一致；注释明确「锚点须指向最外层调用」的题库约定；identifiers 简化。
- **P7 词法查找性能（中）**：`lexical_type` 宿主检查改用预建 `file_callables` 索引（O(本文件 callable)），不再对每个调用点做 O(全表) 扫描。
- 审查中撤销的疑点（实测无误）：后置成员类声明（container 循环覆盖）、同包同名类遮蔽唯一通配导入（same_package 先于 wildcard）。

## 验证

- `cargo test --lib --no-fail-fast`：594 passed，0 failed，2 ignored。
- `cargo test --bin khaslana --no-fail-fast`：303 passed，0 failed。
- `cargo build --release`（清理产物完整重建）：成功，零编译错误零警告。
- `git diff --check`：通过（仅工作区换行符转换提示）。
- **题库调用断言严格验收**：测试 `dev05_fixture_syntactic_calls_bind_exactly` 对 expected.json 全部 34 条 calls 断言逐条核对——33 条 syntactic（`?` 前缀预期 external，其余预期 resolved）+ 1 条 unknown（外部 Stream 链边界，unresolved/external 均算不虚构），ambiguous/无对应调用点均失败。**34/34 通过**。
- **题库语义关系断言严格验收**：测试 `dev05_fixture_semantic_relations_bind_exactly` 对全部 7 条语义断言（implements×3、inherits×1、overrides×2、dispatch_candidates×1）逐条核对（详见上文「验收范围扩展」）。**7/7 通过**，含 J09 跨模块隔离与证据锚点覆盖。
- 针对性回归测试 7 个：方法引用绑定声明作用域、record 模式变量绑定 accessor 调用、字段访问绑定字段声明（含 facts kind 断言）、通配导入索引外构造判 external、词法局部类构造与方法绑定、switch 模式变量不跨 case 泄漏、合成构造器范围与可见性。

## 题库修订（用户先前已确认）

1. J07 局部类 `help()` 内 `run()` 断言的锚点从 `void help()`（声明锚点）修订为 `run();` occurrence 3（真实调用点）。
2. J07 匿名类预期 `wire.<anonymous Runnable>` 与实现的 `$anonymous@{字节}` 命名差异经验收工具的锚点对齐消化，不要求实现改名。

## 已知限制

- **字段初始化器的调用归属**：合成默认构造器使实例字段初始化器内的调用点（含方法引用 `Runnable direct = this::run;`，实测可解析）归属 `<init>`——实例字段语义成立；static 字段初始化器归属 `<init>` 不准确（Java 语义为 clinit），影响仅 owner 标注。
- 泛型方法返回类型（`Stream<String> stream()`）不参与链式 receiver 推导，`names.stream().filter(...)` 链中段保持 unresolved，这是保守语义而非误判（J06 验收按外部链入口处理）。
- record 的规范构造器（带组件参数）未合成，`new Item("a")` 保持 unresolved；若后续题库需要可按组件参数合成。
- J10 的 statements 断言（if/for/while/try-catch/return 语句级事实）尚未实现提取，是题库中唯一未接入验收的断言类型；属语句结构能力线，不阻塞调用/关系语义验收。
- 增量性能：含 Java 的索引走全量重建，DEV-07 才处理事实复用；field_access 的三表体积代价（见上文）届时一并复核。

## 下一步（DEV-05 剩余或 DEV-06 前置）

- 歧义巡检已完成（2026-09-10）：临时探针对题库全部 fixture 索引后 dump ambiguous 调用点，结果为 **0 条**——题库场景内无调用点落入歧义状态，「歧义保留的唯一误判为 0」核对通过；歧义保留语义由专项单测（`dev05_unrelated_reference_overloads_preserve_ambiguity` 等）持续覆盖。
- J10 语句级事实（statements 断言）实现并纳入验收，或明确划出 DEV-05 范围。
- 性能守卫细化（含 field_access 体积基准）。
- 完成后按 DEV-05 交付要求整理实际改动清单与验收命令，再进入 DEV-06（Spring 框架语义）。
