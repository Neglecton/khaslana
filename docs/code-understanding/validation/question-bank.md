# AI 评估固定题库 v1（DEV-00 冻结）

对应 [requirements.md](../requirements.md) §6.2/§7：普通 Java 与 Spring 各 20 题（8 中文业务定位 + 4 调用解释 + 4 类型/重载或注入边界 + 4 未知/缺失边界）。题库冻结后用同一供应商、模型、提示版本与索引版本运行 3 次，报告每次与平均值，不挑最好一次。

- 每题标注：稳定题号（JQxx/SQxx）、题型、fixture 来源（`src/tests/fixtures/code_understanding/` 下的组）、入口 Recall@5 的有效入口集合、允许答案要点（同 expected.json 的 `questions.allowed_points`）。两份 expected.json 的 `question_bank_ids` 是本 20+20 题的机器可校验主索引；组内 `questions` 是确定性 fixture smoke 问题子集。
- 评估指标：检索入口 Recall@5 ≥ 85%；关系 precision/recall 用 fixture 标注独立计算；引用有效率 100%（ID 存在、hash 匹配、范围有效）；歧义题不得宣称唯一执行链；关键结论不得有 unsupported claim。
- fixture 未实现索引提取前，本题库用于冻结评估口径；DEV-09/11 落地后按本表逐题运行。

## 一、普通 Java 题库（J 组）

### 业务定位（8 题，中文提问、定位英文符号）

| # | 问题 | fixture | 有效入口 |
| --- | --- | --- | --- |
| JQ01 | Runner 运行时用的 format 工具类在哪里？ | J01 | `com.example.alpha.Util#format(String)` |
| JQ02 | 这个项目里叫 Util 的类有几个？分别在哪？ | J01 | `alpha.Util`、`beta.Util` |
| JQ03 | 找一下把字符串拼成订单编号的构造器构建逻辑。 | J06 | `Builder#build()`、`Use#chained()` |
| JQ04 | 现代订单模块里订单状态有哪些？ | J08 | `Order.Status` |
| JQ05 | 多模块项目里核心模块的入口类是什么？ | J09 | `module:app-core CoreRunner` |
| JQ06 | Plain 程序的问候逻辑在哪里实现？ | J11 | `Plain#greet()`、`util.Helper#join()` |
| JQ07 | 有没有会自己调用自己的计算逻辑？ | J12 | `Recursor#fact(int)`、`isEven/isOdd` |
| JQ08 | 数字拆解的处理流程方法在哪，里面有哪些分支？ | J10 | `Flow#process(int)` |

### 调用解释（4 题）

| # | 问题 | fixture | 允许要点 |
| --- | --- | --- | --- |
| JQ09 | Service.load 会调用哪个 findById？实现可能有几个？ | J03 | CALLS 指接口声明；候选 2 个实现 |
| JQ10 | Child 构造时发生了什么调用？ | J04 | `Base#<init>(int)`、`super.greet()`、`this.log` |
| JQ11 | wire 方法里 run() 被哪些结构引用？ | J07 | lambda/方法引用/匿名类/局部类 4 处，不假设执行时机 |
| JQ12 | upperNames 的流式链每一步的目标是什么？ | J06 | stream() 外部边界；filter/map/collect 呈现边界，不虚构 |

### 类型/重载规则（4 题；其余复杂泛型仍可保留歧义）

| # | 问题 | fixture | 允许要点 |
| --- | --- | --- | --- |
| JQ13 | f(null) 走哪个重载？ | J02 | 唯一选择 `f(String)`；null 不适用于基本类型重载 |
| JQ14 | o.g(5) 传 int 会进入 g(Integer) 还是 g(long)？ | J02 | 选择 `g(long)`；primitive widening 优先于 boxing |
| JQ15 | rename 里 name.trim 用的是哪个 name？ | J05 | 参数遮蔽字段，最近作用域 |
| JQ16 | itemLabel 里 item.sku() 定义在哪？ | J08 | record 头声明的 accessor `Order.Item#sku()` |

### 未知/缺失边界（4 题）

| # | 问题 | fixture | 允许要点 |
| --- | --- | --- | --- |
| JQ17 | max(a,b) 是本项目的方法吗？ | J04 | 静态导入外部 `?java.lang.Math`，外部边界 |
| JQ18 | CoreRunnerTest 属于生产代码吗？ | J09 | source_set=test，默认不入业务链 |
| JQ19 | 项目里没有订单支付模块吗？ | J11 | 零命中呈现检索范围，不说"项目不存在该功能" |
| JQ20 | twice 调 step 几次？图上是一条边还是两条？ | J12 | 2 个调用点，图聚合为一条边但证据不丢 |

## 二、Spring 题库（S 组）

### 业务定位（8 题，中文提问、定位英文符号）

| # | 问题 | fixture | 有效入口 |
| --- | --- | --- | --- |
| SQ01 | 订单详情接口的 URL 和处理方法是什么？ | S01 | `GET /api/orders/{id}`、`GET /api/orders/detail/{id}` → `OrderController#detail` |
| SQ02 | 批量下单接口在哪里？ | S01 | `POST /api/orders/batch` |
| SQ03 | 价格计算有哪些实现类？ | S04 | `VipPriceCalculator`、`NormalPriceCalculator` |
| SQ04 | 缓存读取接口的 URL 是什么？背后用哪个存储？ | S05 | `GET /cache/get` → `RedisCacheStore` |
| SQ05 | 通知服务有哪几种渠道的实现？ | S06 | `EmailNotifyService`、`SmsNotifyService` |
| SQ06 | 用户查询的 SQL 写在哪里？ | S07 | `UserMapper.xml` namespace `com.example.shop.mapper.UserMapper` |
| SQ07 | 订单实体对应的数据库表是什么？ | S09 | `@Table(name="t_order")` → `OrderEntity` |
| SQ08 | 订单取消后在哪里释放库存？ | S12 | Controller → OrderService → InventoryMapper#release → XML |

### 调用解释（4 题）

| # | 问题 | fixture | 允许要点 |
| --- | --- | --- | --- |
| SQ09 | 取消订单接口的完整声明链是什么？ | S12 | cancel → cancelOrder(声明) → DefaultOrderService#cancelOrder → mapper.release |
| SQ10 | ProfileController 的依赖是怎么注入的？ | S03 | 构造器 + setter 注入点；单构造器无注解也成立 |
| SQ11 | listActive 的 SQL 是怎么拼出来的？ | S08 | include 片段 + if/foreach 条件模板，不合并唯一 SQL |
| SQ12 | JobService 里哪些方法有定时/消息触发标记？ | S10 | nightly @Scheduled、onOrderEvent @KafkaListener；仅声明证据 |

### 类型/注入歧义（4 题）

| # | 问题 | fixture | 允许要点 |
| --- | --- | --- | --- |
| SQ13 | PriceController 的两个注入点各拿到什么？ | S04 | Qualifier 唯一；List 保留两个候选，Primary 不强选 |
| SQ14 | AmbiguousCtorController 用哪个构造器？ | S03 | `@Autowired(required=true)` 构造器是唯一选择；其中 `int flag` 依赖不可满足时报告诊断 |
| SQ15 | CacheController 的 store 是哪个 Bean？backupCache 呢？ | S05 | @Resource 按名称；@Bean 返回接口按可证明表达式关联，别名保留 |
| SQ16 | 生产环境通知走哪个实现？ | S06 | 运行条件未知，候选两个，不唯一断言 |

### 未知/缺失边界（4 题）

| # | 问题 | fixture | 允许要点 |
| --- | --- | --- | --- |
| SQ17 | LegacyController 上的 @Service 是 Spring 的吗？ | S02 | FQN 不符不识别；RestController 才是识别依据 |
| SQ18 | /combined/ping 这个组合注解路由可靠吗？ | S02 | `ApiGet` 基于合法 `@RequestMapping(method=GET)`，固定 GET 语义可解析；未声明 AliasFor 的属性传播仍是覆盖边界 |
| SQ19 | LombokService 的构造器和 getName 定义在哪行？ | S11 | 生成代码不可见，无行号，诊断 generated_source_unavailable |
| SQ20 | legacy() 接口的完整 URL 是什么？ | S01 | `${legacy.base}` 占位符不展开，保留表达式并说明缺口 |

## 运行与判定

1. 冻结：修改本题库或 fixture 标注必须在交接记录中声明版本变化并重跑全部 3 次。
2. 每次运行记录：供应商、模型、提示版本、索引 generation、索引 adapter 版本、日期。
3. `partial` 答案按允许要点部分计分；歧义题把唯一解析记为失败（强行唯一解析数必须为 0）。
4. 拒答质量：无证据时应给出范围与可继续动作，编造项目行为记 0 分并计入失败。
