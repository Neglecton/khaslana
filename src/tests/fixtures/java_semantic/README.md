# JLS-T0 Java 语义样例

此目录只服务于真实 Eclipse JDT LS 验证，与 `code_understanding_business` 的 CU2 样例分离。
样例源码中的 `JLS:` 注释是稳定查询锚点，不参与业务语义。

- `maven-multi/`：无外部依赖的 Maven 三模块工程，覆盖同名 `login`、重载、接口双实现和跨模块调用。
- `plain-java/`：无构建工具的普通 Java 21 源码工程。
- `gradle-java/`：无外部依赖的 Gradle Java 17 工程；真实导入是否可用由 T0 探针记录。
- `spring-data/`：Spring、MyBatis、JPA 形状样例，仅用于记录缺依赖/框架静态关系边界，不运行应用、SQL 或数据库连接。

核心查询的源码位置与候选期望写在 `expectations.json`。行列由探针根据锚点动态计算，避免格式调整后报告引用旧位置。
