# DEV-04 交接

状态：已完成（2026-09-08）。可作为 DEV-05 开工依据。

## 实现

- 新增 `src/code_index/java/`，将 Java 专用事实拆为 `types.rs`、`extract.rs` 与 `project.rs`，通用 tree-sitter walk 不再承担 Java 签名和作用域推断。
- Java 文件事实保存 package、显式/静态/通配 import、类型与成员声明、参数、返回/字段类型、修饰符、注解、Javadoc、继承/实现/permits、参数/局部变量/record component 的词法范围。
- 方法按规范化参数类型形成签名，构造器统一使用 `<init>`，varargs、数组、泛型、显式 import、`java.lang`、同包及本文件嵌套类型保留可验证的规范类型；不能唯一证明的类型以 `?` 标记，不伪造 FQN。
- record component 同时产生作用域变量事实与隐式 accessor 声明；嵌套类型 owner chain、局部类型稳定性和 Java 声明字节/行范围进入持久化元数据。
- Java `SymbolKey` 使用项目、项目内模块根、source set、package、嵌套类型链、种类、名称与参数签名；不把源码相对路径写入稳定成员身份。局部声明额外使用 AST 局部位置并标记 `stability=local`。
- Maven 静态读取明确 `<module>` 与常规 `src/main/java`、`src/test/java`；Gradle 静态读取 `include`、`projectDir` 与常规源码根。模块键使用项目内模块根，不同层级的同名模块不会因末段同名碰撞；动态 include/projectDir 产出诊断，不执行 DSL、构建或网络下载。
- 项目描述文件复用同一轮索引已经读取的字节，不二次读盘；Java 文件事实与 `symbol_semantics` 在原子发布事务中同步落库。POM/Gradle 描述文件作为已支持的项目事实输入，不再误报 unsupported language。
- 兼容图中的 Java 方法 qualified name 与 owner 定位带签名，重载声明不会再互相覆盖；DEV-05 前现有通用调用解析仍只作为旧兼容投影。

## 验证

- `cargo test --lib`：579 passed，0 failed，2 ignored。
- `cargo test --lib dev04_ -- --nocapture`：3 passed，0 failed。
- `cargo test --lib code_index::java --no-fail-fast`：3 passed，0 failed。
- `cargo test --bin khaslana dedicated_fields`：2 passed，0 failed。
- `rustfmt --edition 2024 --check src/code_index/extract.rs src/code_index/pipeline.rs src/code_index/store.rs src/code_index/facts.rs src/code_index/mod.rs src/code_index/java/mod.rs src/code_index/java/types.rs src/code_index/java/extract.rs src/code_index/java/project.rs src/tests/code_index.rs`：通过。
- `cargo build --release`：通过，零警告。
- `git diff --check`：通过。
- DEV-04 专项覆盖：重载签名、构造器命名、嵌套类、静态/通配 import、参数与局部变量遮蔽范围、有限 `var` 推断、record accessor、sealed permits、Maven 多模块、Gradle 静态 include/projectDir、main/test 同名类型隔离，以及源根内文件移动后的稳定 ID。

## 边界

DEV-04 只交付 Java 声明事实、类型/作用域和静态项目边界，不实现 Java 调用目标选择、重载决议、override 或运行时分派候选；这些由 DEV-05 消费本任务保存的 receiver、参数表达式、签名和项目范围完成。项目模型不执行 Maven/Gradle，不下载依赖，也不把静态声明的源码组织误称为真实 classpath；动态构建逻辑以诊断保留未知边界。外部 jar 不展开，复杂泛型/通配 import 无法证明的类型继续保留规范化原文与不完整标记。
