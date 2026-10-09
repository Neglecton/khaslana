# khaslana-code-index 索引质量诊断报告

本报告只记录**实际执行的命令与观察到的输出**。未经验证的因果机制、推测性根因均不写入（见文末「未验证事项」）。

---

## 1. 测试对象与环境

### 1.1 被测服务

| 项 | 值 |
|---|---|
| MCP 连接名 | `khaslana-code-index` |
| 传输 | stdio |
| 命令 | `D:/khaslana/khaslana.exe mcp` |
| 注入模式 | `always`（始终注入） |
| 配置位置 | `C:\Users\0100066975\.dsh\storages\mcp_connector.json` → `connections["json-khaslana-code-index"]` |

### 1.2 被测仓库

| repo 键 | 路径 | 语言 | 文件 | 符号 | 边 | calls | branch |
|---|---|---|---|---|---|---|---|
| `0d7c5ae7` | `d:\deepseek-harness` | TypeScript | 5,465 | 36,533 | 74,651 | 18,034 | master |
| `13a07f6d` | `d:\workspace\bcs_utf8` | Java（见 §5） | 45,375 | 759,644 | 1,894,379 | 808,322 | master |

### 1.3 测量方法

- 索引侧数据：`get_architecture`、`trace_path`、`search_symbols`、`check_index_coverage`、`index_status` 的直接返回值。
- 源码侧数据：对仓库文件的直接文件系统检索（`Select-String` / `grep` 工具），作为对照组。
- 静态调用点分类：自建 Node 脚本，先屏蔽注释与字符串（保留字节长度与换行以维持行号），排除声明行，再用正则提取 `new X(`、`f(`、`.m(` 三类调用形态。**该脚本为近似统计，已知误判见 §12.8。**
- token 估算：`CJK 字符数 × 1 + ASCII 字符数 ÷ 4`。**这是估算，非真实分词器输出。**

---

## 2. 结论摘要（仅陈述观察到的差异）

| 编号 | 观察到的现象 | 影响仓库 | 严重度依据 |
|---|---|---|---|
| P1 | 调用边被归到同名但无关的方法定义上 | Java | `private` 方法报告 12,526 个 fan_in |
| P2 | 跨语言归属：Java 文件的调用边挂到 JS 文件 | Java | 3,114 个 `.java` 文件 → 1 个 `.js` 文件 |
| P3 | `languages` 统计中无 `java` 条目 | Java | 45,371 个文件归入 `other` |
| P4 | 测试文件的多个调用点聚合为 1 个文件级节点 | TS + Java | 11 个调用点 → 1 个节点 |
| P5 | `unresolved_calls` 的构成以无仓库定义的对象方法调用为主 | TS | 5 个被测文件中 5 个均如此（§7.2） |

P1–P4 为索引返回数据与文件系统直接检索的比对结果。P5 为索引字段与自建近似脚本的比对，脚本精度有限（§12.8），故其结论强度低于 P1–P4。

---

## 3. P1：同名方法的调用边归属错误（Java）

### 3.1 fan-in 榜单前 10 名的定义处核对

`get_architecture` 返回的 `hotspots` 前 10 名，逐个查其定义处源码：

| 方法 | 定义文件:行 | 索引报告 fan_in | 定义处可见性 | 仓库内限定调用文件数 | 该名字的定义文件数 |
|---|---|---|---|---|---|
| `newResponseContext` | `ZXA0_4000_mod.java:61` | **12,526** | **`private`** instance | **0** | 1 |
| `substring` | `AKH4_6160.java:515` | 9,714 | `public` `static` | **0** | 1 |
| `info` | `pdfjs/pdf.js` | 8,907 | （JS，不适用） | — | — |
| `HashMap` | `HashMap.js` | 6,427 | （JS，不适用） | — | — |
| `parseInt` | `AD_COMM.java:223` | 5,651 | `public` `static` | **4** | 1 |
| `getMsgid` | `ACC0_2900_dbal_vo.java:198` | 4,988 | `public` instance | — | 4 |
| `setMsgDesc` | `FetchResult.java:73` | 4,297 | `public` instance | — | 14 |
| `isNotBlank` | `AAA1_0451.java:570` | 4,240 | `public` instance | — | 1 |
| `setReturnCode` | `FetchResult.java:89` | 4,204 | `public` instance | — | 13 |
| `mappingDataSet` | `VOHelper.java:604` | **2,816** | `public` `static` | **1,900** | 1,351 |

（「该名字的定义文件数」用正则 `(private|public|protected)[\w<>\[\],\.\s]*\s<名字>\s*\(` 统计。）

### 3.2 逐条验证

#### 3.2.1 `newResponseContext` —— `private` 方法报告 12,526 fan_in

```
定义（ZXA0_4000_mod.java:61）:
    private ResponseContext newResponseContext() {

该方法在其自身文件中的出现次数: 1（仅第 61 行的声明本身）
```

```
$ Select-String -Path $root -Recurse -Filter *.java -Pattern "ZXA0_4000_mod\s*\.\s*newResponseContext" -List
qualified calls: 0 file(s)

$ Select-String -Path $root -Recurse -Filter *.java -Pattern "newResponseContext\s*\(" -List
files containing newResponseContext(: 3082
```

其他文件中的实际写法抽样（各文件 `this.` 前缀）：

```
AAA0_0100.java:77    respCxt = this.newResponseContext();
AAA0_0200.java:38    ResponseContext resp = this.newResponseContext();
AAA1_0100.java:146   ResponseContext resp = this.newResponseContext();
AAA1_0102.java:42    ResponseContext resp = this.newResponseContext();
AAA1_0103.java:81    ResponseContext resp = this.newResponseContext();
AAA1_0104.java:52    ResponseContext resp = this.newResponseContext();
```

`trace_path` 返回：

```json
{"function":"newResponseContext",
 "qualified_name":"bcs_utf8...ZXA0_4000_mod.java.ZXA0_4000_mod.newResponseContext",
 "direction":"inbound","callers_total":13336,
 "callers":[{"name":"doAdd",   "file_path":"src/main/java/com/cathay/aa/a0/trx/AAA0_0100.java","hop":1},
            {"name":"doCancel","file_path":"src/main/java/com/cathay/aa/a0/trx/AAA0_0100.java","hop":1},
            {"name":"doCancelSubmit","file_path":"...AAA0_0100.java","hop":1},
            {"name":"doIndex", "file_path":"...AAA0_0100.java","hop":1},
            {"name":"doModify","file_path":"...AAA0_0100.java","hop":1}]}
```

#### 3.2.2 `substring` —— 0 处限定调用，报告 9,714 fan_in

```
定义（AKH4_6160.java:515）:
    public static String substring(String orignal, int count) throws UnsupportedEncodingException {

同文件内的调用（AKH4_6160.java:503）:
    eventContent.replace(srt+("] 诊断病名 [").length(), end, substring(disease, len-4)+"...");
```

```
$ grep -r "AKH4_6160\s*\.\s*substring"  →  No matches found

$ 提及 AKH4_6160 的 .java 文件数: 2
$ 其中调用 .substring 的:        0
```

`trace_path` 返回的 caller 抽样（均非 `AKH4_6160` 的调用者）：

```
AAUtil.getChineseCheckItem, AAUtil.getDivNo1, AAUtil.getDivNo2, AAUtil.isBetweenTheDay,
AAUtil.isPolicyServiceOrAgency, AAUtil.voToMap, AAA0_0100_mod.doInsert, AAA0_0200_mod.doQuery,
AAA0_0100.doCheck, AAA1_0401.doBatch, AAA1_0401.getDestinationFile, ...
callers_total: 9714
```

> 对照：`AAUtil.java` 中 15 处 `substring` 的实际写法均为 `aFullDivNo.substring(0, 6)`、`someDay.substring(0,4)`、`StringUtils.substringBetween(...)` 等，无一处 `AKH4_6160.substring`。

#### 3.2.3 `parseInt` —— 4 处限定调用，报告 5,651 fan_in

```
定义（AD_COMM.java:223）:
    public static int parseInt (String i) throws Exception {

$ grep "AD_COMM\s*\.\s*parseInt" -List        →  4 file(s)
$ grep "parseInt\s*\(" -List                  →  3354 file(s)
```

同文件内 `parseInt` 的实际写法（`AD_COMM.java`）：

```
L165    int year = Integer.parseInt(date.substring(0,4));
L170    month = Integer.parseInt(monthT.substring(1,2));
L172    month = Integer.parseInt(monthT);
L176    day = Integer.parseInt(dayT.substring(1,2));
L229    return Integer.parseInt(i.substring(0, pos));
```

#### 3.2.4 `mappingDataSet` —— 量级吻合的对照项

```
$ grep "VOHelper\s*\.\s*mappingDataSet" -List   →  1900 file(s)
$ grep "mappingDataSet\s*\(" -List              →  3668 file(s)
$ grep "VOHelper" -List                         →  5032 file(s)
索引报告 fan_in: 2816
```

限定调用 1,900 处 vs 报告 2,816，**同一量级**。

### 3.3 同名字义观察（机制未验证）

将 §3.1 的「该名字的定义文件数」与污染程度并列：

| 方法 | 定义文件数 | 限定调用文件数 | 报告 fan_in | 量级是否吻合 |
|---|---|---|---|---|
| `newResponseContext` | 1 | 0 | 12,526 | 否 |
| `substring` | 1 | 0 | 9,714 | 否 |
| `parseInt` | 1 | 4 | 5,651 | 否 |
| `isNotBlank` | 1 | — | 4,240 | 未测 |
| `getMsgid` | 4 | — | 4,988 | 未测 |
| `setReturnCode` | 13 | — | 4,204 | 未测 |
| `setMsgDesc` | 14 | — | 4,297 | 未测 |
| `mappingDataSet` | 1,351 | 1,900 | 2,816 | **是** |

观察到的相关性：定义文件数为 1 的条目全部严重虚高；定义文件数为 1,351 的条目量级吻合。

**此相关性的底层机制未经验证，本报告不作断言。**

---

## 4. P2：跨语言归属（Java 调用边 → JS 文件）

`HashMap` 与 `info` 的定义处位于 `.js` 文件，但其 caller 列表包含 `.java` 文件中的方法。

### 4.1 `HashMap`

```
定义文件: src/main/webapp/html/AJ/js/HashMap.js  (1182 bytes)
内容开头:
    function OneElement(key,value){ this.key = key; this.value = value; }
    function HashMap(){ this.elements = new Array(); ... }
```

```
$ 在 *.java 与 *.js 中检索 "new\s+HashMap\s*\(" -List，按扩展名分组:
    .java  3114
    .js       0
```

Java 侧的实际用法（`AAA0_0100.java`）：

```
L407    HashMap map=new HashMap();
L729    HashMap map = new HashMap();
L784    HashMap map = new HashMap();

import java.util.HashMap;      ← 该文件显式导入 JDK 的 HashMap
```

`trace_path` 返回（callers 为 Java 文件中的方法）：

```json
{"function":"HashMap",
 "qualified_name":"bcs_utf8.src.main.webapp.html.AJ.js.HashMap.js.HashMap",
 "direction":"inbound","callers_total":13060,
 "callers":[{"name":"src",              "qualified_name":"...com.cathay.aa.AAUtil.java", "hop":1},
            {"name":"dataSetToMaps",    "qualified_name":"...AAUtil.java.AAUtil.dataSetToMaps","hop":1},
            {"name":"getCities",        "qualified_name":"...AAUtil.java.AAUtil.getCities","hop":1},
            {"name":"getGDmap",         "qualified_name":"...AAUtil.java.AAUtil.getGDmap","hop":1},
            {"name":"getMemo",          "qualified_name":"...AAUtil.java.AAUtil.getMemo","hop":1},
            {"name":"getNewProdId",     "qualified_name":"...AAUtil.java.AAUtil.getNewProdId","hop":1}]}
```

### 4.2 `info`

```
定义文件: src/main/webapp/html/AB/js/pdfjs/pdf.js  (532458 bytes)
trace_path callers_total: 17828
callers 抽样: AAUtil.getEntryAndSubAndItem#3, AAUtil.isAcceptServiceOrAgency,
              AAUtil.isAutoClaim, AAUtil.isOverdueForApproval, AAUtil.isPolicyServiceOrAgency
              （均为 src/main/java/... 下的 Java 方法）
```

---

## 5. P3：`languages` 统计中无 `java`

`get_architecture` 对 `13a07f6d` 的语言构成返回：

```json
"languages": [["other", 45371], ["md", 2], ["xml", 1]]
```

同时 `label_counts` 为：

```json
[["Method",442075],["Field",283038],["File",45374],["Class",23796],["Module",13544],
 ["Function",10544],["Folder",6630],["Enum",116],["Interface",75],["Branch",1],["Project",1]]
```

仓库实际 Java 文件数：

```
$ (Get-ChildItem $root -Recurse -File -Filter *.java | Measure-Object).Count
22745
```

`src` 下全部文件数：`51030`（索引 `files` 字段为 `45375`）。

### 5.1 结构统计对照

| 项 | 索引报告 | 文件系统检索 |
|---|---|---|
| 声明 `interface` 的文件 | 75（`Interface` 节点） | 72 |
| 声明 `enum` 的文件 | 116（`Enum` 节点） | 87 |

（正则 `^\s*(public\s+)?interface\s+\w+` 与 `^\s*(public\s+)?enum\s+\w+`。）

---

## 6. P4：测试调用聚合为文件级节点

对 TS 仓库 `resolveReconnectPolicy` 执行 `trace_path`（`direction=inbound`, `depth=2`）：

```json
{"callers_total":2,
 "callers":[{"name":"apply","qualified_name":"deepseek-harness.packages.mcp.mcp-client.src.index.ts.apply","hop":1},
            {"name":"packages",
             "qualified_name":"deepseek-harness.packages.mcp.mcp-client.tests.reconnect.spec.ts",
             "file_path":"packages/mcp/mcp-client/tests/reconnect.spec.ts","hop":1}]}
```

文件系统实际调用点：

```
生产代码:  packages/mcp/mcp-client/src/index.ts:144          （1 处）
测试代码:  packages/mcp/mcp-client/tests/reconnect.spec.ts
           L265, L284, L483, L489, L496, L501, L502, L503, L507, L512, L513   （11 处）
合计: 12 处调用点
```

索引返回 2 个 caller 节点。其中测试文件的 11 处调用点聚合为 1 个节点，`name` 为 `"packages"`，`qualified_name` 为文件路径。

`startConnection` 同样表现：

```
索引: callers_total=2 → apply(hop1) + packages/reconnect.spec.ts(hop1)
实际: src/index.ts:166 (1 处) + reconnect.spec.ts L265, L284 (2 处) = 3 处调用点
```

---

## 7. P5：`unresolved_calls` 的构成

### 7.1 索引报告值

`check_index_coverage` 对 `packages/mcp/mcp-client/src/*.ts`：

| 文件 | `call_sites` | `unresolved_calls` | 比例 |
|---|---|---|---|
| `connection.ts` | 78 | 54 | 69% |
| `index.ts` | 61 | 58 | 95% |
| `tools.ts` | 111 | 80 | 72% |
| `transport.ts` | 5 | 3 | 60% |
| `invariant.ts` | 2 | 2 | 100% |

### 7.2 自建静态分类结果

脚本：屏蔽注释/字符串 → 排除声明行 → 提取 `new X(` / `f(` / `.m(` → 按「是否指向仓库内定义」分类。

| 文件 | 可指向仓库内定义 | 不指向仓库内定义 | 不指向者的构成 |
|---|---|---|---|
| `connection.ts` | 24（本地 22 + 导入 2） | 52 | method 35, ctor 11, builtin 2, globalFn 4 |
| `index.ts` | 2（本地 0 + 导入 2） | 59 | method 55, ctor 4 |
| `tools.ts` | 36（本地 31 + 导入 5） | 73 | method 54, ctor 18, builtin 1 |
| `transport.ts` | 1（本地 1 + 导入 0） | 4 | method 1, ctor 3 |
| `invariant.ts` | 0 | 2 | method 2 |

脚本在 `connection.ts` 上对照人工核对结果通过了 7 项校验：

```
OK   isCurrent:         expected 6, got 6   [lines 164,174,260,283,287,302]
OK   generationDown:    expected 4, got 4   [lines 253,275,294,299]
OK   enqueueSync:       expected 2, got 2   [lines 263,278]
OK   hasClosed:         expected 3, got 3   [lines 273,285,298]
OK   scheduleReconnect: expected 1, got 1   [lines 177]
OK   connectGeneration: expected 2, got 2   [lines 221,308]
OK   waitForClose:      expected 2, got 2   [lines 285,339]
=> ALL MATCH
```

### 7.3 高频「不指向仓库内定义」的名字

```
connection.ts: Error×6  error×5  then×4  Map×3  freeze×2  isFinite×2  resolve×2
               unref×2  now×2  values×2  warn×2  info×2  close×2
index.ts:      default×12  number×5  string×5  required×4  object×3  boolean×3
               min×3  max×3  const×2  pattern×2  dict×2  effect×2
tools.ts:      Error×12  push×11  get×5  Map×5  set×4  request×2  values×2
               isArray×2  includes×2  entries×2  map×2  join×2
```

`invariant.ts` 全文相关代码：

```ts
export const apply = (ctx: Context): Promise<() => void> =>
  Promise.resolve(ctx.invariants.register(PACKAGE_NAME, install))
```

两处调用为 `Promise.resolve` 与 `ctx.invariants.register`，二者在仓库内均无定义，与索引报告的 `2/2` 一致。

另需说明：`invariant.ts` 的 `install` 被作为**值**传递（`register(PACKAGE_NAME, install)`），非调用，故不计入调用点。

### 7.4 数量对照

| 文件 | 索引 `call_sites` | 脚本统计总数 | 索引 `unresolved` | 脚本统计「不指向仓库内定义」 |
|---|---|---|---|---|
| `connection.ts` | 78 | 76 | 54 | 52 |
| `index.ts` | 61 | 61 | 58 | 59 |
| `tools.ts` | 111 | 109 | 80 | 73 |
| `transport.ts` | 5 | 5 | 3 | 4 |
| `invariant.ts` | 2 | 2 | 2 | 2 |

`connection.ts` 与 `tools.ts` 的 `call_sites` 与脚本总数相差 2，其余 3 个文件完全一致。差异未追查到具体条目（见 §12.3）。

`connection.ts` 中脚本识别出的「可指向仓库内定义」调用明细：

```
本地: isCurrent×6   generationDown×4  hasClosed×3  dispose×2
      connectGeneration×2  enqueueSync×2  waitForClose×2  scheduleReconnect×1
导入: syncTools×1   createTransport×1
```

---

## 8. 索引表现正确的对照项（TS）

以下条目中，索引报告的**直接调用者（hop 1）与源码实际一致**。

### 8.1 `isCurrent`

```
源码定义: connection.ts:153
实际调用点（6 处）及其所属函数:
    L164  → enqueueSync        (L162-170)
    L174  → generationDown     (L173-178)
    L260  → connectGeneration  (L237-305，位于 notification handler 闭包内)
    L283  → connectGeneration
    L287  → connectGeneration
    L302  → connectGeneration
直接调用者（去重）: connectGeneration, enqueueSync, generationDown  （3 个）
```

索引返回（`depth=3`）：`callers_total: 7`

```
hop1: connectGeneration, enqueueSync, generationDown        ← 与实际直接调用者完全一致
hop2: scheduleReconnect, startConnection
hop3: apply, packages/reconnect.spec.ts
```

### 8.2 `generationDown`

```
源码定义: connection.ts:173
实际调用点（4 处）: L253, L275, L294, L299 —— 全部位于 connectGeneration (L237-305)
直接调用者: connectGeneration  （1 个）
```

索引返回：`callers_total: 5`，`hop1: connectGeneration` ← 一致。

### 8.3 `enqueueSync`

```
源码定义: connection.ts:162
实际调用点（2 处）: L263, L278 —— 均位于 connectGeneration (L237-305)
直接调用者: connectGeneration  （1 个）
```

索引返回：`callers_total: 1`，`caller: connectGeneration (hop1)` ← 一致。

### 8.4 `syncTools`

```
源码定义: tools.ts
connection.ts 内调用点: L165，位于 enqueueSync 内
直接调用者: enqueueSync  （1 个）
```

索引返回：`callers_total: 5`，`hop1: enqueueSync` ← 一致。

### 8.5 `startConnection` / `resolveReconnectPolicy`

两者的 `hop1` 均包含 `apply`（生产代码中的唯一调用者），与源码一致。差异仅在测试调用点的聚合（见 §6）。

---

## 9. 行为测试：真实会话中模型的工具选择

两个独立 subagent（全新上下文，等同新会话），任务均为「给定方法名 + 修改内容，调查涉及多文件多方法的改动面」。要求其如实按序报告工具调用。

### 9.1 样本 A

任务：修改 `resolveReconnectPolicy` 使 `maxAttempts` 支持 0。

```
工具调用总数: 34 次（2 次失败，7 次无匹配）
其中索引工具: 5 次
    list_projects（无参数）
    search_symbols{query:"resolveReconnectPolicy"}                    → 报错：多仓库须指定 repo
    search_symbols{repo:"0d7c5ae7",query:"resolveReconnectPolicy"}
    trace_path{function_name:"...connection.ts.resolveReconnectPolicy",direction:"inbound",depth:2}
    check_index_coverage（4 路径）
其余: grep / read / glob
```

该样本对索引结果的原始评价（摘自其报告）：

> `trace_path{...}（callers_total=2，启发式聚合，不能替代逐行 grep）`

其结论中列出**需修改文件 9 个**，其中 5 个为文档/门禁类（README 双语、`README.i18n.yaml`、`docs/config-catalog.md`、Agent Note）。

### 9.2 样本 B

任务：为重连退避增加 jitter。

```
工具调用总数: 20 轮（成对并行）
其中索引工具: 3 次
    list_projects（无参数）
    check_index_coverage（未传 repo）    → 报错：本机 3 个仓库需指定 repo
    check_index_coverage{repo:"0d7c5ae7", paths:[6 个文件]}
其余: grep / read / glob
```

该样本第 1 步即 `list_projects`（索引）与 `grep startConnection` 并行发起；第 2 步起转为 `read` 整文件（`connection.ts` 351 行、`index.ts` 181 行、`reconnect.spec.ts` 521 行）。

### 9.3 被测文件的索引覆盖状态（样本 B 实测）

`check_index_coverage` 对改动涉及的文件：

| 文件 | status |
|---|---|
| `connection.ts` | `indexed` / `metadata_matches` |
| `index.ts` | `indexed` / `metadata_matches` |
| `reconnect.spec.ts` | `indexed` / `metadata_matches` |
| `docs/config-catalog.md` | `unsupported`（当前语言不支持符号解析） |
| `packages/mcp/mcp-client/README.md` | `unsupported` |
| `packages/mcp/mcp-client/README.i18n.yaml` | `unsupported` |
| `.agents/notes/implemented/feature/2026-08-06-mcp-client-auto-reconnect.md` | **`not_indexed`**（未记录该文件） |

---

## 10. 体积测量（token 估算）

估算公式：`CJK 字符数 × 1 + ASCII 字符数 ÷ 4`。**非真实分词器输出。**

### 10.1 常驻开销（每个会话固定付出）

| 项 | 估算 tokens |
|---|---|
| `khaslana` 11 个工具 schema | 3,527 |
| 全部 68 个可见工具 schema | 15,128 |
| `khaslana` 占工具 schema 比例 | 23.3% |
| 服务器 instructions 段落 | 232 |
| MCP resource 名称段落 | 36 |
| **固定合计（schema + 提示词）** | **3,795** |

单工具 schema 开销（前 5）：

```
search_graph            556
search_symbols          549
trace_path              440
check_index_coverage    361
detect_changes          310
```

### 10.2 单次调用开销对照

同一查询：查找 `resolveReconnectPolicy` 的定义。

| 路径 | 返回规模 | 估算 tokens | 匹配数 |
|---|---|---|---|
| `grep` 全仓库 | 4,648 chars | 1,489 | 22 处（跨 4 文件，其中 18 处在 `reconnect.spec.ts`） |
| `search_symbols` | 2,074 chars | 519 | 3 个定义 |
| `trace_path`（inbound） | — | 217 | 2 个 caller 节点 |
| `grep` 限定单文件 `connection.ts` | 346 chars | 87 | 3 处 |
| 函数体（`connection.ts` L65-90） | 1,515 chars | 379 | — |

---

## 11. 索引自身声明的状态

### 11.1 `unresolved_calls` 字段的官方说明

`check_index_coverage` 返回的 `coverage_note`：

> 尽力记录。metadata_matches 只证明时间和大小一致，不证明语义完整；部分解析、未解析调用和未索引文件需回读源码。

`index_status` 返回的 `coverage_note`：

> 统计为已记录覆盖问题，不是完整性证明；未解析调用也包括外部库和动态调用

### 11.2 `trace_path` 的 `total` 语义

工具描述：

> callers_total/callees_total 是当前索引中该深度内的可达总数

`coverage_note`：

> 调用边来自 tree-sitter 与启发式解析，可能缺少动态调用或类型信息；total 只统计当前索引中指定深度内的可达节点。

### 11.3 `bcs_utf8` 的覆盖统计

```json
{"coverage":{"counts":{"indexed":23602,"partial":68,"unsupported":21700,"excluded":5662},
 "unresolved_calls":2783216},
 "files":45375,"symbols":759644,"calls":808322,"db_bytes":1238577152,
 "status":"indexing",
 "refresh":{"active":true,"phase":"running","failures":0,"last_error":null}}
```

被核对文件的覆盖状态：

| 文件 | status | `unresolved_calls` / `call_sites` |
|---|---|---|
| `AKH4_6160.java` | `indexed` / `metadata_matches` | 128 / 149 |
| `FetchResult.java` | `indexed` / `metadata_matches` | 12 / 24 |
| `VOHelper.java` | `indexed` / `metadata_matches` | 223 / 291 |
| `ZXA0_4000_mod.java` | `indexed` / `metadata_matches` | 16 / 16 |

---

## 12. 未验证事项（本报告不作断言）

1. **污染的产生机制。** §3.3 观察到「定义文件数为 1 的方法虚高、定义文件数为 1,351 的方法吻合」这一相关性，但未验证其实现层面的原因。
2. **`newResponseContext` 的真实定义处。** `AAA0_0100.java` 声明 `extends UCBean`，但：
   ```
   $ grep "class\s+UCBean"（*.java）        → 0 file(s)
   $ grep "extends\s+UCBean"（*.java）      → 2331 file(s)
   ```
   该类不在仓库内，且 `AAA0_0100.java` 的 import 列表中无 `UCBean`。其来源未确认。
3. **`unresolved_calls` 的精确口径。** §7.4 显示脚本近似统计与索引值接近但不相等，未确定索引的确切计数规则。
4. **`languages: other` 与 §3/§4 现象之间是否存在因果关系。** 二者在同一仓库上同时观察到，未验证关联。
5. **token 估算的准确性。** 公式为近似，未用真实分词器校验。
6. **TS 仓库是否也存在同类同名归属问题。** 仅抽取了 `mcp-client` 包的 5 个文件，未做全仓库扫描。
7. **报告撰写期间索引处于 `indexing` 状态**（`refresh.phase = "running"`），上述 Java 仓库数据取自该状态下的返回。
8. **§7.2 分类脚本自身的偏差与迭代过程。** 该脚本用正则近似 TypeScript 语法，非精确解析器。在得到 §7.2 最终数值前，它曾因两类缺陷给出错误结果：

   | 版本 | 缺陷 | `connection.ts` 本地调用计数 | 真值 |
   |---|---|---|---|
   | v1 | 未排除声明行 | 31 | — |
   | v2 | 把 `if (hasClosed()) {` 等控制流语句误判为方法定义并整行屏蔽 | 17 | 22 |
   | v3（最终） | 已修正上述两类，并通过 7 项真值校验 | **22** | 22 |

   剩余已知限制：对象字面量中的简写方法可能被计入 `method`；`re-export` 形式的导入未登记。因此 §7.2 / §7.4 的绝对值仅供量级参考；`call_sites` 与脚本总数在 `connection.ts`、`tools.ts` 上相差 2 的原因未追查。
9. **§3.3 相关性未做统计检验**，也未扩大到 fan-in 前 10 名之外的样本。
10. **`frozen` 结论的适用范围。** §8 列出的 5 个符号是**抽样**，不构成「索引在 TS 仓库上整体正确」的结论；同理 §3/§4 的 Java 案例也不构成全仓库污染率的结论。

---

## 13. 复现命令

### 13.1 索引侧

```
get_architecture   { repo: "13a07f6d" }
trace_path         { repo:"13a07f6d", function_name:"<qualified_name>", direction:"inbound", limit:5 }
check_index_coverage { repo:"0d7c5ae7", paths:[...] }
index_status       { repo: "13a07f6d" }
```

### 13.2 源码侧（PowerShell）

```powershell
$root = "D:\workspace\bcs_utf8"

# 定义处可见性
Select-String -LiteralPath "$root\src\main\java\...\ZXA0_4000_mod.java" -Pattern "newResponseContext"

# 限定调用计数
(Get-ChildItem -Path $root -Recurse -File -Filter *.java |
  Select-String -Pattern "ZXA0_4000_mod\s*\.\s*newResponseContext" -List).Count

# 裸名调用计数
(Get-ChildItem -Path $root -Recurse -File -Filter *.java |
  Select-String -Pattern "newResponseContext\s*\(" -List).Count

# 跨语言归属（按扩展名分组）
Get-ChildItem -Path $root -Recurse -File -Include *.java,*.js |
  Select-String -Pattern "new\s+HashMap\s*\(" -List |
  Group-Object { [System.IO.Path]::GetExtension($_.Path) } | Select-Object Name,Count
```

### 13.3 TS 仓库调用点核对

```powershell
$root = "D:\deepseek-harness"

# 生产 + 测试调用点
Select-String -Path "$root\packages\mcp\mcp-client\src\index.ts"         -Pattern "startConnection"
Select-String -Path "$root\packages\mcp\mcp-client\tests\reconnect.spec.ts" -Pattern "resolveReconnectPolicy"

# 本地调用的所属函数（逐行核对）
Select-String -Path "$root\packages\mcp\mcp-client\src\connection.ts" -Pattern "isCurrent|generationDown"
```

---

*报告生成时的索引状态：`13a07f6d` 为 `indexing / refresh.phase=running`；`0d7c5ae7` 为 `indexing`。*
