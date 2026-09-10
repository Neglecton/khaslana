# v2 范围调整：从静态分析扩展转向 AI 业务理解

日期：2026-09-10。状态：当前需求与排期依据。

## 调整原因

用户明确指出旧方案过重：目标不是小型 IntelliJ Code Insight 或 Spring 静态分析器，而是借助已有索引，让 AI 找代码、读代码并讲清业务逻辑。典型问题是“登录逻辑怎么实现”，应回答调用上下游、读写哪些表及其他相关逻辑。

此前“普通 Java 与 Spring Boot 同等覆盖”的偏好仍体现在两类业务样例都必须可用，但不再解释为完整语言语义与框架静态规则同等全部实现。这是后续明确需求对首版深度的修订。

## 主要差异

| v1 | v2 |
| --- | --- |
| 索引尽可能先形成完整语义图，AI 解释图内子集 | 索引导航，AI 沿源码补查并形成当前问题的业务解释 |
| 强制补 Java 类型/重载/override/框架规则矩阵 | 复用已有能力；索引不足用搜索/阅读补足或明确不确定 |
| 业务关系必须先属于数据库权威 GraphSlice | 严格静态图契约保留；AI 业务发现用独立会话结果，必须有源码来源 |
| MyBatis/JPA 静态适配器先行，表读写范围弱 | AI 读 SQL/XML/实体映射，读写对象与条件是首版核心输出 |
| 概览图、局部图、复杂布局与宽窄多栏工作台 | 问答主区、按需源码侧栏、少量步骤简图 |
| 先 DEV-05/06/07 等长基础链，再开始 agent | 盘点已成资产后直接做工具与 agent 业务闭环 |

## 保留资产与历史记录

旧文档在 [archive/v1](archive/v1/README.md) 冻结（包括本轮开始前的工作区修改，归档调整了相对链接）。已存在的验证文档原样保留：

- [DEV-00 基线](validation/dev00-baseline.md)
- [DEV-01 类型与协议](validation/dev01-handoff.md)
- [DEV-02 存储](validation/dev02-handoff.md)
- [DEV-03 事实](validation/dev03-handoff.md)
- [DEV-04 Java 元数据](validation/dev04-handoff.md)
- [DEV-05 进行中](validation/dev05-progress.md)
- [schema v3 DDL 记录](validation/schema-v3-ddl.md)
- [旧固定题库](validation/question-bank.md)

这些记录的旧节号、DEV 排期及“下一步”应对照归档阅读。验证结论是历史批次报告，本次未重跑其测试，不自动升级为 v2 业务验收通过。

## 代码处理边界

本轮只修改文档，不删除、回退、格式化或继续开发已有源码/测试，也不改 validation 记录。后续开发如发现已有 Java 增量全量重建或字段访问体积影响体验，按测得问题最小修复；不为满足旧语言题库继续扩展静态引擎。

schema v3、SourceRef、EvidenceBundle 和已完成的类型等按需复用；不为 AI 表节点和流程步骤再扩数据库。新 AnalysisResult 是会话层协议，不削弱原 AnswerDocument/GraphSlice 的验证。

## 新的执行入口

当前有效范围是 [requirements.md](requirements.md)，机制是 [design.md](design.md)，执行使用 [development.md](development.md) 的 CU2-T0～T6。

后续 agent 不应继续依照 DEV-05 进度记录的旧“下一步”自动进入语句提取或 DEV-06 框架分析；应先做复用盘点，再让 AI 在登录样例中完成一次可核对的业务分析。
