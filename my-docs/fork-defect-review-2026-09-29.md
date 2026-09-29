# fork 改造缺陷复核清单（2026-09-29）

> 用途：交给独立复核者（Claude Code）核对。**本文所有"严重度"与"是否真缺陷"均为
> 待证伪假设，不是结论**。每条都给了代码坐标，请自行打开文件验证；若判断我错了，
> 直接推翻并说明依据。我不确定的地方已显式标注"需 live 验证"。
>
> 复核范围：`codex-rs/codex-rust-rig-bridge/`（rig 桥）、`my-docs/FORK.md`（权威索引）、
> 以及 `core/src/client.rs` 的桥路由。上游基线见 FORK.md 头部。

## 0. 背景（事实，非判断）

- fork 用 `rig-core =0.42.0` 承载 OpenAI Chat / OpenAI Responses / Anthropic Messages
  三种线协议，服务中国厂商网关（GLM/MiMo/StepFun/DeepSeek/Ollama）。
- 中立 trait `ModelBridge`/`ModelWireProtocol`/`ModelBridgeOptions` 在
  `codex-api/src/bridge.rs:30/53/64`（已核对存在）。
- core 经 `&dyn ModelBridge` 分派：`core/src/client.rs:1681 stream_model_bridge`、
  `:3181 dispatch_model_bridge`、`info.uses_model_bridge()`（已核对存在）。
- Responses 第三方走"同协议透传"（非转 Chat）：`client.rs` ~2400 行注释与
  `codex-rust-rig-bridge/src/responses.rs` 一致（已核对）。

## A. FORK.md 文档准确性（待证伪）

### A1 — §3.3 点名了不存在的函数【假设：文档缺陷】
- 断言：FORK.md §3.3 写 `responses_routes_via_chat_bridge`。
- 证据：`grep -rn responses_routes_via_chat_bridge codex-rs/` 无匹配；真实符号是
  `stream_model_bridge` / `dispatch_model_bridge` / `uses_model_bridge()`。
  旧 `rig-bridge-implementation-plan.md §8` 也用了同一错名。
- 开放问题：是否还有别处引用这个错名？更正后是否影响其它文档交叉引用？

### A2 — 缺"已知局限"章节【假设：文档缺陷】
- 断言：FORK.md §9 只讲范围未及，未登记审计文档自认的开放项。
- 需登记的开放项（来源见 §C）：Anthropic 固定 `max_tokens`、无 prompt caching
  (`cache_control`)、每轮新建 reqwest 0.13 client（每轮 TLS 握手）。
- 开放问题：这些是否确实仍未做？还是已在某次提交里解决而我没看到？

### A3 — 旧文档与 FORK.md 冲突且未标废止【假设：文档缺陷】
- 断言：`rig-bridge-implementation-plan.md §8`（说 Responses→Chat 转换）与 §12.4
  （trait 名 `ChatModelBridge`）与现状矛盾；FORK.md 是对的。
- 开放问题：除这两处外，my-docs 里还有哪些已被 FORK.md 取代但未盖戳的旧结论？

## B. 代码缺陷（待证伪，按我假设的严重度排序）

### B1 — Anthropic 固定 max_tokens 会让长输出整轮失败【假设 P1，已读码确认链路】
- 坐标：`client.rs:120 DEFAULT_ANTHROPIC_MAX_TOKENS = 16384`；
  `convert_request.rs:169-170`（仅 Anthropic 注入）；
  `convert_response.rs:212-224`：`FinishReason::Length` →
  `Err(ApiError::Stream("Incomplete Rig response..."))`。
- 断言：codex 不建模输出上限；命中 16384 → Length → **整轮报错而非返回截断结果**；
  扩展思考吃预算时更易触发。
- 开放问题：
  1. native Responses 路径遇到 `response.incomplete` 是否也报错？若一致则属"对齐官方"
     而非 fork 缺陷（`convert_response.rs:210` 注释声称"Match native Responses behavior"）。
  2. 16384 对目标厂商模型是否够用？是否应按模型/配置可调？

### B2 — WebSearchCall 历史回放时被丢弃【假设 P1，已读码确认；跨轮行为需 live 验证】
- 坐标：`request_messages.rs:207-219` 把 `WebSearchCall{..}` 归入"internal to Codex"跳过；
  但响应侧 `hosted_tools.rs` + `stream.rs:277-294` 会采集并产出 `WebSearchCall`。
- 断言：多轮 Anthropic web_search 时，上一轮的 server_tool_use / web_search_tool_result
  块不回传，模型丢失检索上下文；要求块配对的网关可能 400。这削弱 FORK.md §4 宣称的能力。
- 开放问题：
  1. codex 是否真的把 `WebSearchCall` 当作需要回传的 model-visible 历史？还是设计上
     就由 native 层处理、桥路径本就不该回传？（需查 codex 官方对 WebSearchCall 的语义）
  2. 该跨轮路径 live 测试从未覆盖（`rig-live-provider-audit` 未测）——是真断裂还是我误读？

### B3 — Anthropic 错误体被销毁，可能连带打断 §5.1 超窗分类【假设 P1，但机制需 live 验证】
- 坐标：`transport.rs:273-284` Anthropic 分支**无条件**用 `sse::with_terminal_check`
  包裹 body；Chat 分支用 `check_terminal`(=`status().is_success()`) 门控（`:258/278`）。
  `sanitize_error`（`transport.rs:~305`）在 rig 读体失败时抹成
  `"failed to read error response body"`。
- 断言：非 SSE 的 JSON 错误体（Anthropic 4xx/5xx 常态）解析不出 SSE 帧 → rig 读体失败 →
  厂商诊断信息丢失（status/headers 保留，故 401 重登录仍可用）。
- **关键联动假设**：§5.1 的 `ContextWindowExceeded` 分类依赖读 400 body 里的
  `prompt is too long` / `context_length_exceeded` 标记（分类逻辑见
  `core/src/compact.rs:314`、`core/src/guardian/input_budget.rs:62/160`）。若 Anthropic
  错误体在分类前已被抹掉，则 Anthropic 线的压缩兜底循环可能永远触发不了。
- 开放问题（**我无法只靠读码定论，需真实 Anthropic 400 用例**）：
  1. `with_terminal_check` 对非 SSE body 到底返回什么？是否真导致 rig 读体失败？
     （`sse.rs:32-41` 我读到的是 UnexpectedEof，但 rig 0.42 内部行为未验证）
  2. 超窗分类读的是 rig 透传的 body 还是 sanitize 之后的？分类点与 sanitize 点的先后顺序？
  3. Chat 线是否因 `check_terminal` 门控而不受影响（即只有 Anthropic 线有此问题）？

### B4 — 工具列表序列化失败时静默清空【假设 P2，已读码确认】
- 坐标：`request_tools.rs:41-47`：`serde_json::to_value(tools).ok()` →
  `from_value(..).ok()` → `unwrap_or_default()`。
- 断言：若工具序列化失败，请求带**零工具**发出，无 warn/error，模型永远无法调用工具。
- 开放问题：`request.tools` 本就是可序列化结构，实际触发概率有多低？是否值得改为
  失败即报错 / 至少 warn？还是当前"尽力而为"是有意设计？

### B5 — 其余 P2（仅列坐标，未逐一深读，供复核者自行判断）
- `convert_response.rs:216`：finish_reason 用精确串 `"model_context_window_exceeded"`
  匹配；其它 `FinishReason::Other`（pause_turn/refusal/网关拼写）→ 硬报错并丢弃可用补全。
- `convert_response.rs:257`：`response_id.unwrap_or_default()` → 空串 id 下发。
- `reasoning.rs:44-49 / 75-82`：首个 content part 非 Text 时 delta 丢失；Summary/Encrypted/
  Redacted 不进 model-visible Done 项。
- `stream.rs:114-115` + `transport.rs:tee_wire_bytes`：Anthropic 每个请求都把整段响应
  tee 进无界 `Vec<u8>`，即便无 hosted 工具、tee 永不被读。
- `convert_request.rs:180-189 map_tool_choice`：未知串当成具体函数名 → `ToolChoice::Specific`
  指向不存在的工具；Anthropic 上可能 mid-request ProviderError。
- `hosted_tools.rs:110-141`：`content_block_start` 覆盖已开块、`content_block_stop` 不校验
  index；交错/重叠 server-tool 块可能错配。

## C. 仓库卫生（待证伪）

### C1 — 顶层游离死代码副本【假设：应删除】
- 坐标：`codex-rust-rig-bridge/src/convert_response.rs`（顶层，330 行，git 跟踪，
  初始提交 `2385149bf` 带入）。真实 crate 在 `codex-rs/codex-rust-rig-bridge/`
  （workspace member 指向后者，`codex-rs/Cargo.toml:147/182`）。
- 断言：顶层那份不参与编译，是孤立副本，会腐烂并误导。
- 开放问题：是否真无任何构建/脚本引用顶层路径？删除是否安全？两份内容差异（330 vs 295 行）
  是否意味着顶层那份反而有真实桥缺失的代码？（**请 diff 两份再决定删哪份**）

## D. 审计文档已自认的开放项（供交叉核对，非我的新发现）

来源：`rig-protocol-audit-2026-09-27.md §8`、`responses-anthropic-field-diff.md`、
`rig-live-provider-audit-2026-09-27.md §6`、`rig-responses-phase1|2/tasks.md`。
- Anthropic 固定 max_tokens（= B1）。
- 无 Anthropic prompt caching（`cache_control`）——多处标"未做/挂起"。
- 每轮新建 reqwest 0.13 client（TLS 握手）——标"非本期/backlog"。
- 测试覆盖缺口：parallel-tool 只断言 ≥1 次调用；真实 Anthropic 401/重登录"未在本轮执行"；
  跨轮 web_search 未测（= B2）。
- phase2 遗留：provenance 未喂进 projection；非信封外来密文透传可能被拒；
  per-provider hosted-tools 白名单未配置化。
- `claude-rig-full-validation.md` 从未产出 → **全 workspace 测试是否真跑过存疑**。

## E. 给复核者的请求

1. 逐条打开坐标验证我的断言，**优先证伪**：哪几条是我读错了 / 其实是对齐官方行为 / 已修复？
2. B3 的"超窗分类被错误体销毁打断"是我最不确定、影响最大的一条——请重点查
   sanitize 与分类的先后顺序，必要时建议一个能证伪的 live/集成测试。
3. C1 请先 diff 两份 convert_response.rs 再判断删哪份。
4. 有没有我完全没提到的缺陷类别（并发/锁、feature-gate 组合、Bazel 构建面、
   app-server v2 API 影响、rollout 恢复兼容性）？
