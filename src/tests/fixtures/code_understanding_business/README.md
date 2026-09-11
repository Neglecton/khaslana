# V2 业务理解样例

此目录服务于 CU2 的真实业务问答验收，与旧 `code_understanding/java`、
`code_understanding/spring` 语言题库分开维护。

- `B01`：普通 Java 登录闭环。两个调用入口、JDBC 明确 SQL、用户读取与成功/失败更新、
  尝试记录、内存会话。
- `B02`：Spring + MyBatis 登录。Controller、Service、Mapper 接口 + Mapper XML、
  条件登录日志、Redis 会话。验收重点是从方法读到 XML 再报告表读写，缓存不混为表。
- `B03`：Spring + JPA 登录。Repository 派生查询、显式 `@Table` 实体、无显式映射实体、
  `@Transactional`。验收重点是「有映射才推断表名，无映射标 unknown」。
- `B04`：检索缺口。`LoginController → AuthService.authenticate` 是索引可解析的普通调用边，
  `LegacyJspLogin` / `PluginLoginBridge` 经**反射**调用同一方法（索引必然无边），核心方法
  刻意用全仓库唯一的方法名保证普通边在、缺口边真实存在；Mapper XML 无符号节点。
  验收重点是「AI 靠文本搜索补查入口并说明范围，不强依赖完整静态图」。
- `B05`：边界变体。按月分表（`portal_user_${month}`）、存储过程（`sp_archive_inactive_users`，
  未进登录流程）、无实现接口（`SsoAuthPort`）、条件多实现（`CredentialChecker` 两实现按
  配置选择）、复杂 SQL（`StatsMapper` 的 CTE + JOIN，同样不在登录流程）。验收重点是
  「未知不编造；CTE/别名不当物理表；保留条件与外部边界」。
- `expected.json`：人工标注的事实范围（入口 / 核心方法 / 调用方 / 读写对象与条件 /
  分支说明 / 未知项），**不保存模型生成答案**，避免把模型输出当 oracle。

B01～B05 真实模型验收（各两遍）已全部通过，结论与逐遍判读见
[cu2-t3-live-acceptance.md](../../../../docs/code-understanding/validation/cu2-t3-live-acceptance.md)。

## 真实模型验收

实况测试在 `src/tests/code_understanding_live.rs`（`#[ignore]`，需本机已配置可用的
AI 供应商）：

    cargo test --lib live_tests -- --ignored --nocapture --test-threads=1

未配置模型时打印原因并跳过，不伪造通过。结果写入
`docs/code-understanding/validation/live-runs/`，判读结论见
[cu2-t3-live-acceptance.md](../../../../docs/code-understanding/validation/cu2-t3-live-acceptance.md)。
