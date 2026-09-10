# V2 业务理解样例

此目录服务于 CU2 的真实业务问答验收，与旧 `code_understanding/java`、
`code_understanding/spring` 语言题库分开维护。

- `B01`：普通 Java 登录闭环。包含两个调用入口、用户读取、成功/失败更新、
  尝试记录和内存会话，首批只读工具与 agent 闭环优先使用。
- `expected.json`：人工标注的事实范围，不保存模型生成答案。

后续 T3 按开发文档补 B02 Spring + MyBatis、B03 Spring + JPA 及 B04/B05
边界变体。
