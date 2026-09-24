# live-tests 模块设计(Plan 层文档)

> 对象:`codex-rs/live-tests`(独立集成测试 crate,不与任何业务 crate 耦合)。
> 定位:用**真实模型厂商**验证 fork 的桥接层与编译产物,是发布前的最后一道验收。
> 关联:`rig-bridge-implementation-plan.md`(桥本体)、`openai-responses-chat-bridge.md`(协议映射)。

## 1. 设计原则(为什么这样设计)

1. **单一职责、零耦合**:测试逻辑全部收在本 crate;被测物(两个桥 crate、codex-exec
   二进制)不知道测试的存在。加厂商 = 改 `.env.local`,不改代码。
2. **测用户真正运行的东西**:L3 套件只测**编译出来的 codex-exec 二进制**,不测进程内
   函数——配置解析、feature 组合、链接产物全部被真实覆盖。
3. **不可伪造的验收信号**:marker 技术(随机 nonce → 模型必须真实执行 `echo` →
   exit 0 + nonce 出现在命令聚合输出)。主判据是本地执行证据;模型转述是加分项
   (MiMo 偶尔偷懒不转述,不应连坐)。
4. **失败可回溯**:每次运行落盘完整产物(events.jsonl / final_message / stderr /
   桥层逐事件日志),失败现场永不丢失。
5. **无凭据即绿**:未配置 key 时全部跳过,CI 与本地无密环境零干扰。

## 2. 三层验证金字塔

| 层 | 套件 | 测什么 | 失败定位 |
|---|---|---|---|
| L1 桥层 | `tests/bridge_live.rs`(7) | 协议转换正确性:事件顺序、增量重组、end_turn、usage;两桥 A/B | 桥的 convert 代码 |
| L3 二进制层 | `tests/exec_live.rs`(7) | 完整 agent 闭环:配置解析 → 分派 → 工具执行 → 多轮回放 → 默认桥策略 | core 集成/配置/二进制构建 |
| (未来 L2) | — | codex-core 会话级(进程内 Session),mock provider 已由 upstream `core/tests/suite` 覆盖,暂不重复 | — |

分派矩阵(L3 覆盖 fork 全部路由分支):

| 用例 | wire_api | experimental_bridge | 期望路径 |
|---|---|---|---|
| chat-genai / chat-rig | chat | 显式 genai / rig | 对应桥 |
| chat-default | chat | 不配 | **rig**(fork 默认) |
| responses-rig-default | responses | 不配 | **rig**(第三方默认走桥) |
| responses-native | responses | "native" | 原生(逃生舱) |
| anthropic-genai / anthropic-rig | chat + /anthropic URL | 显式 | 对应桥的 Anthropic 路由 |

## 3. 厂商模型(vendor 抽象)

```
.env.local(.env.example 为模板,gitignored)
LIVE_VENDOR_NAME=mimo                        # 厂商标识,决定产物目录 logs/live-<vendor>/
LIVE_VENDOR_API_KEY=…                        # 必填,缺失 → 全部跳过
LIVE_VENDOR_CHAT_URL=…                       # 必填(非 mimo 厂商);mimo 有内置默认
LIVE_VENDOR_ANTHROPIC_URL=…                  # 可选;缺省 → anthropic 套件跳过(绝不跨厂商回退)
LIVE_VENDOR_MODEL=…
```

规则:**任何 URL 都不做跨厂商回退**——厂商 A 的 key 发到厂商 B 的端点是最危险的
静默错误。回退链仅 `LIVE_VENDOR_* → MIMO_*(历史兼容)`。

## 4. 稳定性策略(与真实模型共处)

- **主判据抗模型方差**:命令执行证据(exit 0 + marker)为主,转述为辅(已吸收
  MiMo "跑完工具不总结" 与 "不调工具直接输出文本" 两个真实方差)。
- **重试**:依赖仓库 nextest 配置的自动重试(网络/模型抖动),主断言保持严格。
- **二进制新鲜度守卫**:`codex_exec_binary()` 对 binary mtime 与桥/core 源码 mtime
  做比较,过期即打印醒目警告——"测了旧二进制"曾经真实咬过我们(unknown variant)。
- **超时**:每事件 180s、整进程 300s,防挂死;日志提示重建命令,缺失即失败
  (fail-fast,不静默跳过)。

## 5. 产物目录(排查体验)

```
logs/live-<vendor>/                     # gitignored(/logs/)
├── bridge/<tag>-<nonce>.log            # L1:每个 turn 的逐事件 Debug 流(两桥可 diff)
├── mimo-<scenario>-<nonce>-<pid>/      # L3:每次运行
│   ├── events.jsonl                    #   codex-exec 完整事件流(核心排查文件)
│   ├── final_message.txt               #   模型最终回答
│   └── stderr.log                      #   RUST_LOG=info 进程日志(含 dispatch 分派记录)
└── (无索引时的约定:ls -t 第一个即最新)
```

## 6. 演进路线(按需实施,当前均未阻塞)

| # | 演进项 | 动机 | 形态 |
|---|---|---|---|
| E1 | 多厂商矩阵 | **已实现(2026-09-24)**:`LIVE_VENDORS=mimo,glm` + `bridge_matrix!`/`exec_matrix!` 宏(paste)按 厂商×桥×场景 生成测试 fn;30 用例全绿。加厂商 = .env 一段 + 宏列表加一个名字 |
| E2 | 能力声明 | 部分实现:anthropic 网关(可选 URL→跳过)、responses 端点(可选独立 URL,GLM 与 chat 不同源)已按"可选即能力"落地;reasoning/tools 能力开关待真正需要时再加 |
| E3 | 运行清单 | 知道某次产物测的是什么代码 | 每次运行写 `manifest.json`(git rev、binary mtime、vendor、场景);E1 前置收益最大 |
| E4 | 产物保留策略 | logs 无限增长 | 保留最近 N 次运行的清理钩子(测试开始时执行) |
| E5 | rig 桥单测移植 | 23 个 genai 桥单测的 rig 版 | 纯离线,补齐 rig 桥的转换边角覆盖(当前由 L1 live 兜底) |

## 7. 运行手册

```bash
cargo build -p codex-exec --bin codex-exec   # L3 前置(改桥/core 后必须重建)
cargo nextest run -p codex-live-tests        # 14 用例
cargo nextest run -p codex-live-tests --no-capture   # 查看逐事件输出与产物路径
```

## 8. 社区工具调研(2026-09-24)

Rust 社区**没有**成熟的"live LLM/agent E2E 测试框架"——本 crate 的手写 harness
填补的正是这个空缺,核心资产(不可伪造 marker、分派矩阵、产物落盘)没有现成替代。
但以下工具值得按需接入,均与现有结构正交:

| 工具 | 是什么 | 接入价值 | 建议 |
|---|---|---|---|
| **rig-cassette**(rig 自带,workspace 内) | HTTP 录制/回放(VCR) | 录一次 MiMo/GLM 真实流 → 离线确定性回归桥转换逻辑,不怕限流/厂商改版;rig 自己的 fixture 语料就这么做的 | **首选**,下个迭代评估其公开 API 能否挂到我们的桥传输层 |
| **insta** | 快照测试 | 对事件流"形状"(事件种类序列)做脱敏快照,L1 加离线回归层 | 次选,与 cassette 二选一或并用 |
| **wiremock**(upstream 已用) | mock HTTP | upstream 套件在用;live 场景不适用 | 不动 |
| **cargo-nextest**(已用) | 测试运行器 | retries 已在吸收 flake;后续可用 test group 把 live 套件限并发,防厂商限流 | 按需 |
| promptfoo / LangSmith / Braintrust | JS 生态/托管评测 | 不匹配 Rust 嵌入式需求 | 不引入 |

原则:框架不引入,胶水保持自有——我们的核心竞争力是"验证 codex 全链路"这件事
本身,通用评测工具覆盖不了。

## 9. rig-cassette 评估结论与优化总盘点(2026-09-24)

### 9.1 rig-cassette:暂不接入(三个硬事实)

1. **版本线断裂**:crates.io 上只有 0.0.1(独立实验版本),不在我们锁定的
   rig-core v0.42.0 tag 内——它是 tag 之后新增的 crate。
2. **绑定 rig HEAD 架构**:其抽象(EffectLog、provider cassette engine、
   agent/ECS runtime 集成)面向 rig 自家 0.42 之后的 Wire/Bound/effects 体系,
   接它 = 接触我们刻意回避的不稳定层。
3. **覆盖面不匹配**:它录的是 rig 传输层;我们的核心资产是
   "ResponsesApiRequest → ResponseEvent" 的桥边界,且 genai 桥它覆盖不了。

**替代方案(推荐):桥边界自制 cassette** —— 在我们自己的公共 API 面上录制回放:

- `LIVE_RECORD=1` 时 run_turn 把 (request, events) 落成 JSON fixture
  (`tests/fixtures/<vendor>/<bridge>-<scenario>.json`);
- 回放模式无网络构造同样的事件流,**跑同一套断言**——桥重构/rig 升级后全量
  回放即可验证转换逻辑等价性;
- 两桥通用、断言零重复、不依赖 rig 内部;
- 前置小改动:`ResponseEvent` 补 `Serialize, Deserialize` derive(目前仅 Debug);
  估算 ~150 行 + 每场景录一次。

### 9.2 rig-bridge 优化盘点(按优先级)

| 级 | 项 | 动机 | 估算 |
|---|---|---|---|
| P0 | 错误路径 L1 场景:坏 key → 断言 `Http{401}` 透传 | `map_completion_error` 的状态码提取**当前零测试**;401 触发 core 重登录循环是关键链路 | 半天 |
| P0 | 桥边界 cassette(9.1) | 离线确定性回归,防 rig 升级/桥重构回归 | 1 天 |
| P1 | Anthropic 工具两轮场景(thinking 签名回放) | anthropic 桥最经典的断裂点;当前 anthropic 只测了纯对话 | 半天 |
| P1 | 并行工具调用场景(一轮两个 function call) | 多 internal_call_id 累积器逻辑未被真实验证 | 半天 |
| P1 | usage 合理性不变量(input>0、total≥input+output) | 一行断言,捕获 usage 映射错位 | 0.5h |
| P2 | 丢弃宿主工具从 debug 提到 warn + 每次 dispatch 带 span | 兼容性静默损失可见化 | 1h |
| P2 | E5:genai 桥 23 个单测的 rig 版移植 | 离线覆盖转换边角(老 uint 边界等) | 1 天 |
| P3 | HTTP 客户端复用(当前每轮新建 = 每轮 TLS 握手) | 长会话性能;受 rig 无 per-request header 限制,**等 v0.43+ 传输层稳定做原生传输时一并解决** | 阻塞中 |

### 9.3 live-tests 优化盘点

| 级 | 项 | 动机 | 估算 |
|---|---|---|---|
| P0 | E3:每次运行写 manifest.json(git rev、binary mtime、vendor)+ INDEX.md 汇总(每测试 pass/fail + 产物链接) | logs/ 从"目录堆"变成"可浏览报告";排查直接定位 | 半天 |
| P1 | A/B 自动 diff:两桥同场景事件**种类序列**对比,不一致即报 | 把"两桥可对比"从可能变自动 | 半天 |
| P1 | CI nightly:GitHub Actions manual-dispatch + LIVE_* secrets 跑全矩阵 | 厂商改版/限流当日发现 | 半天 |
| P2 | E4:产物保留策略(留最近 N 组) | logs 无限增长 | 1h |
| P2 | test-group 限并发(nextest groups) | 多厂商并发打限流 | 1h |

## 10. 落地记录(2026-09-24,第二批交付)

P0/P1/P2 全部完成,含过程中测试体系抓到的两个新真问题:

| 项 | 状态 | 备注 |
|---|---|---|
| P0 错误路径场景(auth_rejected × 厂商 × 桥) | ✅ | **抓到真问题:rig 把 401 延迟到流内,core 的重登录恢复循环永不触发**。修复:桥在返回前急切拉取首个流事件;并补齐 `ProviderResponseError` 变体的状态码映射(此前只认 `HttpError` 变体) |
| P0 桥边界 cassette | ✅ | `LIVE_CASSETTE=record/replay`;fixtures 按厂商×桥×场景落盘;**replay 模式无凭据零网络**(29 用例 0.077s 全绿);`ResponseEvent` 补 serde derive |
| P1 anthropic 工具两轮 | ✅ | 两个厂商全过(thinking 签名回放验证) |
| P1 并行工具调用 | ✅ | 场景 + 离线单测双覆盖 |
| P1 usage 合理性不变量 | ✅ | input>0、total≥in+out |
| P1 A/B 自动 diff | ✅ | **抓到真问题:rig 桥从不发 `Created` 事件**(rig 流无 Start 概念)。修复:泵前合成 `Created`。断言用"折叠后种类序列"(增量分块粒度是流内部行为,非语义差异);已知分歧白名单:`glm/anthropic-tool-t1/t2`(GLM 网关对两适配器请求形态反应不同,语义不变量均通过;rig-anthropic 请求对齐留在 backlog) |
| P2 丢弃宿主工具 debug→warn | ✅ | |
| P2 E5 rig 桥单测 | ✅ | 16 个(convert_request 9 + convert_response 7),含增量字节一致性回归 |
| P2 manifest + INDEX | ✅ | 每次 L3 运行写 manifest.json(git rev/二进制 mtime);`cargo run -p codex-live-tests --bin index-logs` 生成可浏览 INDEX.md |
| P2 E4 产物保留 | ✅ | 每厂商留最近 25 组运行 + 120 条桥日志,自动清理 |
| P2 nextest 限并发 | ✅ | live 组 max-threads=2 + 范围化重试 {3 次, 10s 退避}(GLM 限流实战调出) |
| P1 nightly CI | ✅ | `.github/workflows/live-tests.yml`(手动+每日定时,secrets 注入,产物上传) |
| P3 客户端复用 | ⏸ | 维持阻塞记录(等 rig 传输层稳定) |

新增 backkog:rig-anthropic 适配器请求对齐(GLM 下 t1 前导文本/t2 thinking 的触发差异)。

## 11. 第三批交付 + StepFun 接入 + 限流实证(2026-09-24 晚)

### StepFun(step)接入:显式 Anthropic 协议

StepFun 的 Anthropic 网关(`api.stepfun.com/step_plan`)**URL 无 `/anthropic`
标记**,URL 嗅探失效——促成 `wire_api = "anthropic"` 显式协议落地:
- `WireApi::Anthropic` 变体;core 分派到桥并显式指定协议(rig: `RigProtocol`
  参数;genai: 直接 Anthropic adapter);`wire_api = "chat"` 的 URL 嗅探保留为
  兼容回退。L1 场景与 E2E anthropic 套件全部改走显式协议。
- 注意:genai 适配器要求 base_url 带 `/v1`(直接拼 `messages`);rig 会归一化。
  配置统一带 `/v1` 后缀。

### 厂商限流实测数据(排障定论)

| 厂商 | 限制类型 | 实测证据 |
|---|---|---|
| MiMo | **账户级并发 ≤ 5** | `429: concurrency reached, current: 6, limit: 5`(codex-exec 单进程即开多条连接,测试并发会放大) |
| GLM | **请求频率配额**(code 1302) | 64 用例 45 秒打完触发;串行 430 秒不触发 |
| Step | 偶发 429 瞬时 | 重试吸收 |

### 并发治理终版(.config/nextest.toml)

- **按厂商分组、组内串行、组间并行**(`test(~<vendor>)` 正则过滤——注意
  `test(name)` 是精确匹配,`sub()` 不存在,首版配置静默失效过)
- 64 用例 68 秒全绿;残留瞬时限流由重试(3 次/15s 退避)吸收为 flaky
- 新厂商 = 加一个 test-group + 一条 override

### 其他修复

- ab_diff 白名单重构为**场景族**判定(工具族=模型非确定性:思考/前导文本
  可有无,两桥结构等价由场景断言保证);稳定场景(chat/effort/anthropic/auth)
  保持严格 diff——两个真 bug(Created 缺失、OpenAI 参数泄入 anthropic 线)都是
  在稳定场景上抓到的
- marker 前缀硬编码 "mimo" 修复为厂商名(StepFun 产物曾落成 mimo-xxx)
- StepFun 端点能力:chat(`/step_plan/v1`)+ anthropic(`/step_plan/v1` base)
  + responses(`/step_plan/v1/responses` 200)全有

### 状态

- 在线全矩阵:**64/64**(3 厂商 × 7 L1 场景 × 2 桥 + 21 E2E + ab_diff)
- 离线回放:**43/43,0.119 秒**(无凭据)
- rig 桥单测 16/16

## 12. 外部 review 分诊与第四批修复(2026-09-25)

Codex 独立 review 给出 44 项发现;分诊结论:**证实并立即修复 7 项,证实待修 20 项
(排入下批),部分不成立 2 项**。本轮修复:

| # | 发现 | 修复 |
|---|---|---|
| 22 | codex-config/core 测试目标 11 处编译错误(experimental_bridge 缺字段 + Anthropic 未覆盖) | 全部补齐;三种 feature 组合 + 测试目标编译验证 |
| 9 | **MCP 扁平名回查在生产路径失效**(router 先包 default namespace,回查条件 namespace.is_none() 永假)——原单测直接调 registry 绕过了 router,假通过 | 回查改为不依赖入参 namespace(flat index 只含 namespaced 工具,误配不可能);新增 router 包裹形态的回归测试 |
| 8 | txt_0/rsn_0 固定 ID 使 retained history 每轮覆盖上一条 | 两桥 ID 改为纳秒+计数器唯一后缀 |
| 31 | nextest 配置位置(仓库根,cargo workspace 在 codex-rs)与语法([[test-group]]/group=)双错,从未生效;此前"分组修好限流"的结论不成立 | 合并进 codex-rs/.config/nextest.toml,官方语法 [test-groups.*] + test-group =,补 ci profile;已验证解析生效 |
| 29 | replay 缺 fixture 静默转 live,离线跑可能消耗配额 | 缺 fixture 即 panic(fail-fast) |
| 30 | nightly 只传 key 不传 URL/model → 全部场景静默跳过 | workflow 传完整厂商配置 + "零执行即失败"守卫 |
| 35 | marker 断言匹配整行 JSON(命令文本也含 marker),stdout 捕获坏了也能过 | 解析 JSON 精确断言 aggregated_output 字段 |

待修 backlog(证实,按 review 建议顺序):#1(P0 图片 base64 文本化)、#3(reasoning
回放顺序)、#4(AgentMessage)、#5(custom 工具往返)、#7(data URL)、#10/11/44(工具
流状态机)、#12(anthropic usage)、#14(首事件超时)、#15/16(genai 状态透传/网关头)、
#36-43(发布链 8 项)。

Review 的价值确认:**#9 和 #31 都是我自建测试的盲区**(单测绕过真实调用路径;配置
静默不生效),外部视角抓到了内部测试无法发现的结构性问题。

## 13. Review backlog 第一批桥修复(2026-09-25)

review 证实的桥正确性七项全部修复(单元 22/22 + 全矩阵 64/64):

| # | 修复 | 实现 |
|---|---|---|
| 1 P0 | 工具返回图片 base64 文本化 | rig:逐项转换,data URL 解码为 base64 图片块 + 200KB 文本上限;genai(纯文本 ToolResponse):提取文本,data URL 换占位符;EncryptedContent 换占位符 |
| 3 | reasoning 回放顺序 | 两桥:连续 assistant 项(Reasoning→Message→FunctionCall)合并为**一条** assistant 消息;新回归测试断言三种部件在同消息内且顺序正确 |
| 4 | AgentMessage 丢弃 | 两桥:明文部分作为 assistant 文本转发(合并入前一条);加密部分跳过(跨 provider 不可回放) |
| 7 | data URL 当网络 URL | rig:输入图片(用户+assistant+工具结果)的 data URL 统一解码为 base64 source(anthropic 线会丢远程链接内容) |
| 44 | Added 晚于 delta | rig:名称未知时参数增量缓冲,名称到达或完整 ToolCall 时先发 Added 再按序释放 |
| 14 | 首事件无超时 | 急切首事件拉取包 idle_timeout |
| 12 | anthropic usage 语义 | rig anthropic 的 cache read/write 在 input_tokens 之外 → 归一化相加(按 provider 标签判定,OpenAI 线不变) |

剩余 backlog:#5(custom 工具往返,需工具类型追踪)、#6(output_schema 带工具首请求)、
#10/#11(Final 时的不完整调用/参数覆盖)、#15/#16(genai 状态透传/网关 OAuth 头)、
#13(Ask 预设权限)、#17-21(config/proto/schema/Bazel/app-server)、#36-43(发布链)。

## 14. Review backlog 第二批(2026-09-25):工具状态机 + custom 工具

| # | 修复 | 实现 |
|---|---|---|
| 10 | Final 复活 rig 已丢弃的不完整调用 | PendingRigTool.confirmed 仅由完整 ToolCall 事件置位;Final 只结束已确认调用,未确认的 warn 后丢弃(覆盖单测) |
| 11 | 原始拼接覆盖权威参数 + 无上限 | 参数累积 1MB 上限;拼接非法 JSON 时回退 rig 权威值(rig 会修复 null 前缀/残缺 JSON);合法时维持字节一致性契约 |
| 5 | custom/freeform 工具无法往返 | 请求:custom 工具声明为 `{"input": string}` 包装 schema + 名单随请求传递;历史回放同包装;响应:名单内工具的 Done 还原为 CustomToolCall(解包 input 字符串),apply_patch 类 handler 恢复可用(往返单测) |

单元 25/25 + 全矩阵 64/64。
