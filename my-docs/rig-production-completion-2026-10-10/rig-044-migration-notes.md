# Rig 0.42 → 0.44 迁移工单（复审候选）

分支 `codex/rig-044-20261014`（基于整合分支 `5b6b51457`）。pin 已升
`rig-core = "=0.44.0"`（bridge + live-tests），`cargo update -p rig-core` 引入
`rig-http 0.44`/`rig-tungstenite 0.44`。**已完成编译适配和本机定向离线验证，
尚未合回 test、未 push；不能宣称三平台或国内厂商实测验收完成。**

2026-10-15 Codex 复审发现并修复 10 类问题，Bridge 最终 295/295；详细问题、
首败、命令、二进制与验证边界见 [复审记录](codex-rig-044-review-2026-10-15.md)。
以下各“进展”节保留历史轮次；其旧测试计数不等于当前完整验收结论。
特别更正：resume/compact 281 和 env 62 是 Core 默认 provider 回归，不能证明
全部经过 Rig，也不是 app-server 公共接口回归。Phase C 仍有显式 Rig 生命周期
专项证据待补，付费录制不是唯一剩余项。下一轮见
[Claude 开发交接](claude-next-rig-images-2026-10-15.md)。

## 0.44 API 映射笔记（源码已核，/tmp/rig-044-review + registry）

### 客户端构造（client.rs 8 错）
- `openai::CompletionsClient::builder().api_key().base_url().http_client(h).build()` → 无。
  新：`OpenAIConfig`（字段 api_key/base_url/...）`.connect(impl HttpClientExt)` → `OpenAI`
  → `.chat(model)` → `Model<Chat>`；`.responses(model)` → `Model<Responses>`。
  `http_client!` 宏生成 `.connect()`/`.with_http()`/`.http` 字段。
- `anthropic::Client::builder()` → `AnthropicConfig::new(key).connect(h)`（查
  `src/client/anthropic.rs` 同宏）。anthropic-version 等经 Config 字段或 wire 定制。
- `CompletionModel<C>` 类型消失：Model 是 `{wire, transport}`，无泛型 http 参数。
- HttpClientExt 签名兼容（send/send_multipart/send_streaming 相同）——桥的
  RigHttpClient 装饰器可直接继续实现。`Client is not an HTTP client` 错误来自
  把 reqwest Client 直接交给 builder；需要 `rig-reqwest` 的 `ReqwestClient::from`
  或直接传 RigHttpClient（它实现 HttpClientExt）。桥内建 send_streaming 手驱路径
  可完全绕过 rig client（见 responses.rs 条目）。

### Message 形状（request_messages.rs 25 错 + convert_* + request_content）
- `Message::Assistant { id, content }` → `Message::Assistant(AssistantMessage)`；
  `AssistantMessage { content: Vec<AssistantContent>, origin: Option<Origin>, stop: Option<StopReason> }`
  （#[non_exhaustive]，用 `AssistantMessage::new(content)` / `rolled_back()`）。
- `ToolCall { id: CallId, provider: Option<ProviderCallId>, function, signature, additional_params }`
  → `ToolCall { id: CallId, function: ToolFunction, native: Option<Native> }`。
  provider id/signature/原始 JSON 都进 `native`（查 `native.rs` 的 Native 构造器）。
- `ToolCallId`/`ProviderCallId` → `identity.rs`: `CallId`（统一单一 id）、`ToolName`。
- `Reasoning { text, redacted, native }`：`new_with_signature`/`.signature` → signature
  存 `native`（Native 携带 provider item，含签名）。reasoning.rs 的 v1 rollouts
  存的是 0.42 `Vec<RigReasoning>` 序列化——**必须保留旧 DTO 解码**（handoff 要求），
  不能就地改写。

### 流（stream.rs/stream_pump.rs/convert_response.rs 20 错）
- `StreamedAssistantContent` → `rig_core::streaming::Item<E>`（Event/UnknownPayload），
  completion 流的 E = `StreamEvent { Start{part,kind,name}, Text{part,text},
  Reasoning{part,text}, Arguments{part,json}, End{part,content: AssistantContent} }`。
- `StreamFinal` → `CompletionResponse { choice: Vec<AssistantContent>, usage, origin, error, stop }`。
- `Model::stream(req) -> Streamed<Completion>`（`streaming/mod.rs:92`）；首次 poll 才发请求
  （handoff：验证零发送取消）。
- pump 重写：Start/End 配对维护 part→ResponseItem 映射；Arguments 增量拼 JSON；
  End 的 AssistantContent 是权威终值。tool 名在 Start.name，终名在 End 的 ToolCall。

### Usage / 错误（usage.rs 2 + request_retry.rs 3 + transport 6）
- `Usage` 全字段 Option；`input_tokens` 已含 cache 读——**桥侧不得再加 cache**。
  保留 Anthropic 溢出 sanitize。
- `CompletionError` → `crate::error::ProviderError`（`error.rs`/`error/`）。
  status/request-id/body 访问按 ProviderError API 重接。
- `http_client::Error::InvalidStatusCodeWithMessage` → `InvalidStatusCodeWithDetails`
  （rig-http 0.44 `http_client/mod.rs`；`non_success_headers()` 可用）。
  `Error::InvalidStatusCode`（无细节版）消失。
- `http_client::sse` 模块 → `rig_http::http_client::framing`（SseFramer/NdjsonFramer）。
  桥的 sse.rs 若只引用类型名，改 framing 或自持。
- reqwest13 `ClientBuilder::tls_backend_preconfigured` → 0.44 reqwest 版本下改名/移除，
  custom-CA 预配置路径需查 rig-reqwest 0.44 对应 API（client.rs:97）。

### Responses 透传（responses.rs 2 错）
- `openai::Client` + `post_sse("/responses")` 消失。改法：不再经 rig client——
  直接构造 `http::Request<Bytes>`（POST {base}/responses + query），交给
  `HttpClientExt::send_streaming(&RigHttpClient, request)`（桥已实现），auth/query/
  header 语义全在 transport.prepare 已有。风险：丢失 rig 的 URI join/Bearer 合成
  ——wire_auth 已覆盖。响应仍走 codex-api SSE 解码器（桥自己的 sse.rs）。

### 其他
- `Image.additional_params` 字段删除（request_content.rs 2 处 + 测试）。
- `Client is not an HTTP client`（2 处）：传 RigHttpClient 本身即可（勿传 reqwest）。
- cassette.rs/hosted_tools*: 逐错适配。

## 进展（2026-10-15 第一轮，未提交前的状态见 git log）

Phase A 已完成 12/16 文件（91 错 → 31 错，全部集中于流核心 4 文件 + cassette）：

已完成：transport（rig-reqwest 包装 + BoxedStream 路径）、client（0.44
OpenAIConfig/AnthropicConfig 直构 wire、Model{wire,transport} 公有字段组装、
tls_backend_preconfigured 经 rig-reqwest/rustls feature 到达）、request_messages
（AssistantMessage 包裹 + 兼并）、request_content（CallId::from_wire/ToolName::new/
ToolResult is_error/Image native）、request_tools（ToolDefinition.name: ToolName、
parse_tools/request_tools 变 fallible）、convert_request（CompletionRequest 经
pub ctor + 补字段；ToolChoice::Specific 用 ToolName）、reasoning（v1/v2 双信封：
旧 0.42 DTO 只解码 + 转换到 0.44 Reasoning{text,redacted,native}，新写入 v2 前缀，
旧 rollout 不改写）、usage（apply 适配 Option 字段 + input 已含 cache 不重复加）、
responses（手构 http::Request + urlencode；wire query 克隆；Error 字段不再是
Option 解引用）、request_retry（rig_reqwest::ReqwestClient 包装）、cassette 部分。

## 剩余：流核心重写（31 错，4 文件）——需要按 0.44 语义重设计，非机械替换

0.44 流模型与 0.42 根本不同，bridge 的 convert_response/stream_pump/response_tools/
stream 需要重写：

1. **事件形状**：`StreamedAssistantContent::{Text, ReasoningDelta, Reasoning,
   ToolCallDelta, ToolCall, Final, Unknown}` → `Item<StreamEvent>::{Event(E),
   UnknownPayload}`，E = `StreamEvent::{Start{part,kind,name}, Text{part,text},
   Reasoning{part,text}, Arguments{part,json}, End{part,content: AssistantContent}}`。
   `Part(u32)` 是流内序号；无稳定 provider id —— PendingTools/PendingRigMessage 的
   id 键控需改为 Part 键控（End 的 content 才带权威 ToolCall/CallId）。
2. **终局不在流里**：流耗尽后调 `Streamed::finish() -> CompletionResponse
   {choice, usage, origin, error, stop...}`（或 `.partial()` 取未完视图）。
   `FinishReason` 保留在 CompletionResponse.finish_reason()（私有字段经 accessor），
   映射：Stop→end_turn true；ToolCalls→false；Length→CapExhausted；
   ContentFilter→ApiError::ContentFilter；Other(ctx_window)→ContextWindowExceeded；
   Other(_)→Stream 错误。
3. **pause_turn 被 0.44 折叠**：anthropic decoder 把 `pause_turn` 归入
   FinishReason::Stop（"pi 规则：按原样重放以恢复"）。fork 的 D3 暂停续链必须改为
   从 wire tee 解析 `message_delta.delta.stop_reason=="pause_turn"`（tee 已存在于
   pump，currently only for hosted tools）。
4. **usage 语义**：CompletionResponse.usage 全 Option；`input_tokens` 已含 cache
   读+写（0.42 Anthropic 分开，Chat 原本已包含）。map_usage/rig_usage_report 不得再加 cache；Anthropic
   溢出 sanitize 保留。wire_reported_usage 注入点从流内 Final 移到 finish() 之前。
5. **`Model::stream()` 不再是 future**（返回 Result<Streamed, ProviderError>）；
   stream.rs 的 `.await` 与"首事件预取"逻辑保留但类型重接；零发送取消语义变化
   （新版首次 poll 才发送——需要实测验证）。
6. **错误类型**：`CompletionError` → `ProviderError`；`map_completion_error` 重接。
7. **事件录制/cassette**：rig_events fixture 记录的是 0.42 StreamedAssistantContent
   序列化——cassette.rs 的 RigEventFixture 需要版本化（v2 记 Item<StreamEvent>），
   旧 0.42 fixture 的解码路径保留（与 reasoning v1 同理）。

## 进展（2026-10-15 第二轮：流核心完成，280/280 绿）

流核心 4 文件 + cassette 重写完成（convert_response/response_tools/stream_pump/
stream/cassette + reasoning v1 解码共用），lib+tests 编译 0 错，280 测试全过，
clippy 0 警告，codex-core/codex-live-tests 编译通过（live-tests 的 recorder/
fixture 接口已随迁）。按 0.44 语义的设计决策与坑：

1. **历史适配器 `adapt` 是最大的语义断层**（0.42 无此层）。`Model::stream` 经
   `Operation::prepare` 调 `history::adapt_for`，规则：相邻 user 消息合并、
   空内容/空白文本 turn 丢弃、跨模型 turn 的 Opaque/签名 Reasoning 降级为纯
   文本、无工具声明时 tool call 历史折叠成文本（tools_as_text）。桥接对策：
   - 复审后 Chat/Anthropic 都挂同源 `Origin::new(api, provider, model)`；opaque
     replay 先受 endpoint/model/auth-domain 的 source gate 限制。删除原先的
     Anthropic context 指纹镜像：当前可达 SDK thinking 配置不执行该比较，
     旧测试不能证明镜像算法正确。将来若在 adapt 之前引入显式 thinking 配置，
     必须重新验证 context-bound 模型的 origin，不能伪造当前上下文指纹。
   - v2 签名/redacted reasoning 保留原 native item 及 fingerprint；canonical
     内容被编辑时由 SDK 降级，不能重新 `with_native()` 洗掉失配。只有旧 v1
     DTO 在读侧合成正确 native 块。redacted payload 留在 native item，
     canonical text 为空。Chat 的旧文本推理补 reasoning_content 原生字段。
   - hosted 搜索锚点改为 **marker 文本块**（`SDK_ANCHOR_TEXT`，NUL 前缀）：
     空文本锚点会被 adapt 的 blank 过滤丢弃并导致相邻 user 合并；marker 在所有
     adapt 路径存活，transport 剥掉后拼接原始 wire 块。锚点不计入
     serialized_assistant_len（不上 wire）。
   - hosted server 工具放 `additional_params["tools"]`（rig 原生序列化 raw
     tools，transport 不再拼接 tools 数组）；同时使 declares_tools 为真，
     避免 tools_as_text 折叠历史 tool call。
   - `serialized_assistant_len` 的 Reasoning 计 1（块基数，不再按 text.len()）：
     identity splice 按 content 数组切片，字符数会越界回退到末尾追加。
2. **usage 权威在桥内累加器**：rig 0.44 的 fold 只保留 start+terminal 帧，
   中间累计帧会丢；`AnthropicUsage::apply` 保持 fork 的逐帧覆盖语义（显式 0
   覆盖、null/缺省不动），但写入 `input_tokens` 用包含式值
   （wire input + cache_read + cache_creation，与 0.44 Usage 语义对齐）。
3. **`ProviderError::Http` 的 `source()` 返回 None**（0.44 只对 Json/Url/Request
   暴露 source）：capture/budget 错误分类器需显式下钻 `Http(Arc<Error>)` 内层，
   否则 fail-fast 错误被误分类为可重试网络错误（生产级修复，非仅测试）。
4. 从 wire tee 解析 pause_turn 与 model_context_window_exceeded
   （`anthropic_stop_reason`）：前者保留暂停续链，后者恢复上下文耗尽错误，
   避免 SDK 的 Length 归类误判为输出 cap；tee 同时喂 hosted 工具块。
5. Responses 透传手构 URI 不带 query——transport `prepare` 统一追加（曾双重
   拼接）。
6. `tool_input_streaming: Off`（0.44 默认 Eager 会给每个工具加
   eager_input_streaming 字段，偏离 0.42 wire 形状）。
7. cassette 双版本：fixture v2 记 `Item<StreamEvent>` + TerminalRecord，v1
   （0.42 untagged）在读侧翻译（Name delta→Start、Delta→Arguments、ToolCall→
   End、Final→terminal；`translate_legacy_events`）；`Part` 经 serde 透明形式
   构造（`part_from_index`，失败走 Stream 错误不 panic）。复审修正 legacy usage：
   仅 Anthropic 把 cache 加入 input 并重派生 total；Chat 原 input/total 已包含
   cache，不得再加一次。此 cassette 格式尚不证明多 attempt pause 回放。
8. 测试侧的 0.44 适配（fork-contract adaptations，均有注释标记）：无工具声明
   的纯历史请求在 wire 测试里显式声明工具（生产恒有工具声明）；opt-out/
   去重尾部等无锚点场景的相邻 user 合并期望改为合并后的形状。

## 进展（2026-10-15 第三轮：回放验证 + 残留收敛）

live-tests 离线回放矩阵全绿（`LIVE_CASSETTE=replay LIVE_VENDORS=mimo,glm,step`
bridge_live 84/84），修掉回放暴露的三个问题并补齐两条 origin 覆盖：

1. **v1 reasoning DTO serde 形状错误**（回放 panic `invalid legacy reasoning
   block`）：0.42 的 `ReasoningContent` 是
   `#[serde(tag="type", content="content", rename_all="snake_case")]` 邻接标记
   （`{"type":"text","content":{"text":..,"signature":..}}`），不是外标记单键
   map。已按 0.42 registry 源码核对修正。
2. **legacy usage 换算的 total 口径**：第三轮曾对两协议统一加 cache 并重派生
   total；Codex 复审证实这会重复计算 Chat cache，现已改成 provider-aware。
   Anthropic 加 cache 后重派生 total；Chat 保留旧 input 和非零 reported total。
   真实旧 Chat fixture 的 131 input / 128 cache / 51 output 仍回放为 131/182，
   而非错误的 259/310；另覆盖饱和算术。
3. **exec_live 在 replay 下误激活**：exec 级场景起真实 codex 进程打真端点，
   replay 占位 URL 使其必然失败；matrix 宏统一在 `CassetteMode::Replay` 下
   跳过（与 bridge_live auth 场景的既有模式一致）。
4. **origin 覆盖补齐**（上一轮的残留项 2）：复审后测试改名为
   `same_model_origin_covers_system_prompt_and_hosted_tools`
   （claude-sonnet-4-5：双 developer system、function + hosted 工具、raw tools
   上 wire；钉住前导 system 块及签名保持，不再声称验证 context 指纹镜像）与
   `binds_context_claude_models_replay_signed_thinking_without_a_fingerprint`
   （claude-sonnet-5-5：目录驱动的无指纹分支）。
5. **nuwax 离线三件套修复**（startup 工具往返 / budget 恰好覆盖 / budget
   差一拒绝，3/3 恢复全绿）：loopback 夹具的 `marker_from_prompt` 每条
   user 消息只看第一个 text 块，而合并后的 codex-core 把 environment_context
   作为独立 text 块放在真实 prompt 之前——服务器提前 `?` 退出关闭 socket，
   客户端报 `connection closed before message completed`（看似传输层故障，
   实为夹具假设过时）。修复 = 遍历全部 text 块（本地副本 +
   nuwax_loopback_common 共享版）；另给 read_request 的 content-length
   缺失路径补 headers 上下文便于诊断。排查中重建了 codex-exec（receipt
   以整树状态为准，任何源改动后都需重建二进制再跑）。
6. **replay 模式的套件一致性**：nuwax 离线场景入口在 `CassetteMode::Replay`
   下显式跳过（exec_live matrix 宏同样），使 `LIVE_CASSETTE=replay
   cargo nextest run -p codex-live-tests` 整 crate 188/188 全绿；nuwax
   三件套在无 LIVE_CASSETTE 时照常离线运行（loopback，非 replay 路径）。
7. `just bazel-lock-update` 已跑（Cargo.lock 的 0.44 依赖已同步进
   MODULE.bazel.lock）。

### 已知残留与证据边界（Codex 复审更正）
- 显式 Rig 的 Core 冷/热 resume、compact 已有专项测试，当前候选仍需明确
  复验并补图片历史缺口；须配置
  provider_id/experimental_bridge 并捕获真实 loopback 请求证明路由，不能用
  默认 OpenAI mock 的通过数代替。Anthropic 签名还需跨真实 rollout 冷恢复。
- v2 录制可先用本地 SSE + 实际 recorder 生成并回放，完全离线。厂商付费
  `LIVE_CASSETTE=record` 待另行授权；mock 不证明厂商支持或真实识图。
- Rig-event cassette 当前只把单 attempt 作为有效证据。共享 recorder 跨 pause
  会复用 Part 并覆盖 terminal；0.42 也不支持成功回放该路径。本轮不扩大修复，
  pause/hosted 由真实 loopback wire 测试验证，不能拿单 attempt 回放代替。
- 第四轮报告 codex-core resume/compact 家族
  （suite::{resume, resume_warning, compact*, compact_resume_fork,
  multi_agent_resume, scenarios_compaction, truncation resume 等，
  `--test all -E 'test(resume)+test(compact)'`）**281/281 全过**；
  env 家族（remote_env*/environments/env_startup gate，
  `-E 'test(remote_env)+test(environments)+test(env_startup)'`）
  **62/62 全过**。这些使用默认 Core 测试 provider；更改 display name 或仅启用
  rust-rig feature 不强制走 Rig。env 62 是 Core remote_env/environments/gate，
  不是 app-server env API；本次另跑真实 app-server 公共接口 9/9。
- 旧 CI ledger 不能证明当前 SHA 的 guardian SIGABRT/超时与迁移无关；需相同
  平台、工具链、features、线程栈的候选/基线对照后才能归类。
- 历史 188/188 是 nextest PASS 数，包含 replay 模式下内部 early-Ok；不能写成
  188 个协议场景实际执行。旧 rig-event fixture 回放也不经过 0.44 wire decoder。
- 当前候选 Linux/Windows、全量 CI、ACP dev 和真实厂商视觉仍未验收。

## 进展（2026-10-15 第五轮：生命周期证据补准 + 复审假通过纠正）

按 [Claude 开发交接](claude-next-rig-images-2026-10-15.md) 第一批执行，基线
`29d55354c`（保留全部 Codex 复审修复）。

### 复审收据纠正（重要）

复审记录的「Core cap 含原生对照 30/30」是**假通过**：其沙箱设置了禁网
环境变量，`skip_if_no_network!` 全部提前 return Ok（复审日志中 30 项均为
0.02 秒 PASS；真实执行需 1.6–19 秒）。本机启用网络真实执行同一选择器后
**30 项中 7 项失败**，均为 0.44 迁移引入的真实缺陷（复审测试树与最终提交
仅差 fmt，见 `/private/tmp/codex-rig044-review-core-cap.log` 对照）。已修复
（提交 `6c2d2c562`）：

1. **被输出预算截断的 tool 调用覆盖了 cap 终态**：rig 0.44 在流内以无
   native item 的 End 关闭被 length 截断的调用，桥的 invalid-arguments
   严格检查把它变成 Stream 错误——既替换了本应解释截断的 CapExhausted，
   又因 Stream 可重试把单发 mock 耗成 404（cap_partial 两用例的失败
   机制）。现无 native 的截断关闭标记为 cut：不执行、不发布，cap 终态
   照常出现；仅良性终态遇到 cut 调用才 fail-fast。
2. **0.44 宽容 usage 读取丢掉 fork 的严格契约**：负数计数被 `as_u64`
   静默吞掉、message_start/delta 缺 usage 折叠成默认终态，而 0.42 桥
   一直把这两种情形作为可重试流错误（"invalid value: integer"、
   "missing field `usage`"）。wire 级 usage 观测器恢复该严格性，消息
   原样穿过 provider 错误映射（Transport::Network，core 可重试）。
3. wire-budget 负控 loopback 外层 wrapper 与内层 5s accept 窗同限竞速，
   负载下 Elapsed 假超时；外层给余量（10s），内层 5s 断言不变。

第五轮随后两处补丁（同一批）：`replay_rig_events`/`replay_fixture_events`
改为返回 `RigReplay{events, error}`——live 泵在错误终态前会先送出流式前缀
（Created/deltas），纯 `Result` 回放会丢弃该前缀；`AnthropicUsage::
snapshot_report` 的 input 改为包含式（+cache 读/写），与 `apply`/
`map_usage` 的 Completed 口径一致（core 既有断言的夹具无 cache，两种口径
同值，故此前未暴露）。

### 生命周期证据收据（真实执行，2026-10-15，候选树含上述修复）

选择器、命令与数量（均为断言实际执行的秒级用例，无 early-Ok）：

| 范围 | 结果 | 命令关键参数 |
|---|---|---|
| Bridge 全 crate | 299/299 | `-p codex-rust-rig-bridge --locked --offline --retries 0 --test-threads 8` |
| 显式 Rig core 家族 | 50/50 | `-p codex-core --features rust-rig,rust-genai --test all --locked --offline --retries 0 --test-threads 4 -E 'test(rig_anthropic)+test(rig_responses_bridge)+test(rig_output_cap)+test(rig_image)'` |
| live-tests replay 整 crate | 195/195 | `LIVE_CASSETTE=replay LIVE_VENDORS=mimo,glm,step -p codex-live-tests …` |

- 既有显式套件及真实断言点：`rig_responses_bridge`（Responses resume 不
  回填 provenance、Chat compact 超窗恢复、Chat 凭据实例 reasoning 续链、
  输出 cap 不重采样、Lite 契约）、`rig_anthropic_hosted_tools`/`_identity_/
  _source_rotation_/_credential_instance_`（保存/恢复/fork/来源与凭据轮换/
  暂停续链，bridge=rig provenance 断言内嵌）、`rig_output_cap`（三 wire
  终态矩阵，本轮修复后真实执行）。路由证明不依赖 display name：全部走
  `wire_api` 非 Responses-openai 的第三方 provider + 独立 loopback endpoint
  非零命中（路径断言 `/v1/chat/completions`、`/v1/messages`、`/v1/responses`），
  provenance `bridge=rig` 由 identity/rotation 套件逐项断言。
- 默认 provider 的 281 resume/compact + 62 env 回归保留为一般回归，不冒称
  走 Rig（复审更正继续有效）。

### 新增覆盖

1. **图片历史 × 三 wire × 冷恢复/fork/local compact**（`core/tests/suite/
   rig_image_history.rs`，4/4）：真实 2×2 PNG 经 LocalImage 准备管线入史，
   update_plan 工具往返同行；冷恢复后下一请求逐字节重放同一图片载荷
   （Chat `image_url` data URL／Anthropic base64 `source`+media_type／
   Responses `input_image`）且 call/result 配对完整；Anthropic 单元另含
   fork（热路径）同断言；Chat local compact 后压缩请求经 rig wire、后续
   请求无图片字节（本地压缩弃媒体为设计行为）、rollout 只追加。图片不
   退化为文本副本（data-URL 形态负控）。
2. **离线 v2 cassette 磁盘往返**（`live-tests/tests/
   cassette_v2_roundtrip_offline.rs`，7/7）：本地 Chat/Anthropic SSE 经
   **实际 recorder**（`stream_via_rig_with_recording`）录单 attempt，经
   `save_rig_event_fixture` 同一序列化/布局写入临时目录（新增 `_at`
   变体），重载后 `replay_fixture_events` 回放，完整事件列表逐项相等。
   仅归一化两类生成值：`rigseg_*` 合成 item id 与 v2 envelope 的 endpoint
   source。覆盖 reasoning→文本（含迟到 usage+cache）、reasoning→自定义
   工具（fixture custom_tools 恢复 CustomToolCall 与 call id）、签名
   thinking→文本（SIG 原样存活）、thinking→工具、cap（CapExhausted 及
   usage 报告逐字段等）、content_filter、未知 finish。无手写 v2 fixture。
   旧 v1 fixture 未动。

### cassette 单 attempt 限制（单列）

Rig-event cassette 仍只把单 attempt 作为有效证据：共享 recorder 跨 pause
会复用 Part 并覆盖 terminal（`record_terminal` 每次覆盖 `Option`），多
attempt 录制不经由该路径验证。pause/hosted/HTTP/idle/truncation 继续由
真实 loopback wire 测试覆盖，不以 cassette PASS 代替。本轮未触及该限制；
如需支持，须独立小补丁（显式拒绝多 attempt 录制或 per-attempt 格式）。

### 未验边界（本批后仍开放）

- Linux/Windows 原生、全量 CI/workspace、ACP dev、真实厂商 API/识图——
  未运行（第三批机器定向验证时处理）。
- 付费 `LIVE_CASSETTE=record` 录制轮次仍待授权。
- 复审"30/30"类沙箱禁网收据：后续引用其日志时注意其 PASS 均可能为
  early-return；以本轮真实执行收据为准。

## 进展（2026-10-15 第六轮：MCP／Code Mode 图片入口硬限）

按交接第二批执行。先写额度（具名常量+理由落在
`utils/output-truncation/src/lib.rs` 与 `utils/image/src/lib.rs`），再实现：

### 额度表（单条工具结果维度）

| 常量 | 值 | 理由 |
|---|---|---|
| `MAX_TOOL_OUTPUT_IMAGES` | 8 | Guardian 对话级用 4；工具级 8 容纳多步批量截图，仍界住上下文 |
| `MAX_TOOL_OUTPUT_IMAGE_BYTES` | 16 MiB/图（encoded） | 6000px original PNG 合法体积之上、解码前拒绝炸弹 |
| `MAX_TOOL_OUTPUT_IMAGE_TOTAL_BYTES` | 24 MiB/结果 | 聚合 encoded 上限，超限从最旧开始丢 |
| `TOOL_OUTPUT_IMAGE_VISUAL_TOKENS` | 1,844/图（high/auto） | 与 core history 估算和 bridge wire_images 平价（~7,373B 准备图）三处同步 |
| original detail | 不占文本预算 | 其成本由 patch 粒度上下文记账与压缩承担；仅受数量/字节上限 |
| `MAX_PROMPT_IMAGE_PIXELS` | 40 MP（全局） | 头部声明尺寸预检，>6000×6000 合法上限（36MP）；解码分配前拒绝 |
| `MAX_PREPARED_IMAGE_BYTES` | 32 MiB（全局） | 与 wire 整包硬限对齐：超此值必被 wire 拒，提前给出有界可归因失败 |

### 实施点（最靠近共同入口，零新增传参）

1. **`utils/output-truncation`**（MCP `McpToolOutput::response_payload` 与
   Code Mode `truncate_code_mode_result` 都已调用）：条目截断先做图片
   准入——超单图/数量/聚合上限者从最旧丢起，保最新（任何限额不能静默
   删光），落一条有界省略标记；kept 图片按视觉 token 预扣同一预算
   （Bytes 策略按近似字节换算），文本只能花剩余额度——"文本接近 10K
   后无限加图"不再可能。encrypted_content 仍免费通过。
2. **`core/image_preparation::prepare_tool_output_content`** 提交期兜底：
   对未经过生产者截断的来源（dynamic tools、rollout 重载）幂等重施
   数量/字节上限（token 预算不二次扣），防止旧历史绕过。
3. **`utils/image`** 解码安全：decoder 初始化后先读 header 尺寸，超
   40MP 直接 ImageTooLarge（解压炸弹在栅格分配前拒绝）；准备产物超
   32MiB 同样拒绝（占位符路径已有）。
4. **wall-time 头部移到截断之后**（context.rs）：头部成为与省略标记
   同级的有界常量开销，图片占满预算时头部仍可见。

### 合同保持与负控

- typed image 不变：MCP image 块→`InputImage`、Code Mode `image()`→
  typed、普通 JS 返回不发内容、`text()` 恒为文本（勘路确认，无字符串
  猜图）。structuredContent 优先语义未动（其 Base64 作为 TEXT 计入
  token 截断，天然有界）。未知/畸形 MCP 块仍是 JSON 文本回退。
- 用户消息图片不走工具预算（`prepare_message_content` 独立路径）。
- request capture 2MiB 上限为纯诊断捕获，不拒绝请求（Phase A 设计），
  与 attempt 预算解耦确认。
- 三 wire typed 字段的出站证明沿用 Phase A outbound 测试（用户+工具图
  双覆盖）与 rig_image_history；上限发生在 payload 构造期，先于任何
  HTTP 派发。

### 验证（真实执行）

- output-truncation 29/29（新增 9 项：数量/单图/聚合/共享预算/original
  豁免/always-keep-newest/Bytes 换算/File 引用；更新 3 项钉住旧
  「图片零成本」语义的测试并注明理由）；
- utils/image 15/15（新增像素炸弹拒绝 + 36MP 合法解码）；
- core lib context 370/370（新增 MCP 入口 12 图上限回归）、
  image_preparation 8/8（新增兜底幂等回归）；
- 家族复跑 image/code_mode/rig_/rmcp stdio image/context 929 选中，
  除下述环境簇外全过。step_settings 的小预算用例按新语义更新
  （图片在则省略标记，无图则截断照旧）。

### 环境性失败簇（本 worktree，与代码无关）

`mcp_optional_startup_grace`×3、`interrupt_during_mcp_startup`×2、
`shutdown_cancels_startup_prewarm`×1、`realtime_conversation` startup
context×2：同一提交（c0d07a7a2）干净树在 /tmp 新 worktree 全过
（0.2-0.7s），在本 worktree 稳定失败（deadline 5s 耗尽）——
本 worktree 位于 /Volumes 卷，会话初始化慢于这些 5s 级 deadline。
差分证据：/tmp/baseline-check 基线（同命令同提交）。候选修复：Linux
主验机上重跑该簇；不在本批放宽任何 deadline。

## 进展（2026-10-15 第七轮：连续截图 stress + 平台验证启动）

1. **20 次连续截图 stress**（`rmcp_client::
   twenty_synthetic_screenshots_trigger_formal_compaction`，提交
   `4ca0e4fa0`）：rig Chat wire + 30K auto-compact 限额 + 递增 usage；
   有状态响应器驱动（与边界触发时机解耦）。钉住合同：21 次采样每发
   恰一次（截图以 typed image 入模）、正式 CONTEXT CHECKPOINT
   COMPACTION 先于窗口触发（非中途拒绝）、压缩后请求 shedding 旧图
   （峰值 vs 最后一张图数）、无额外 attempt、压力后冷恢复继续且
   rollout 只追加。
2. **Linux 主验**（192.168.32.163，swufe@，rustc 1.95.0 与 pin 一致）：
   候选 HEAD（4ca0e4fa0）源码 tar + 作用域套件（bridge 全量 / core
   rig 家族 + 图片限额 + stress + 环境性失败簇 / utils 两 crate /
   live-tests replay 整 crate）在远程后台执行——第六轮的环境性
   startup 簇在 Linux 快盘上重验是关键对照。收据见下轮记录。
3. **Windows**（192.168.32.235 `win`，cargo/rustc 在位）：Linux 通过
   后同一流水线执行原生验证（WSL 不用）。
4. **ACP dev 联调**：本轮明确不调用付费/真实厂商 API——截图、取消、
   冷恢复、压缩四项验收都需要真实模型回合，故 ACP dev 实测顺延到
   授权付费 API 的轮次；不把「看到了文件」当作识图成功。
5. Guardian/溢出类新失败如出现，按同 SHA/工具链/features/栈做候选
   与基线对照后归类（本轮未触及）。



## 进展（2026-10-15 第八轮：Linux 主验收据）

候选 `4ca0e4fa0`（源码 tar → 192.168.32.163，Ubuntu 24.04，72 核，
rustc 1.95.0（8af696dd，与 pin 版本一致；`RUSTUP_TOOLCHAIN=1.95.0`
绕过 rustup 处理 rust-toolchain.toml 的 transaction panic）。

| 范围 | 结果 | 备注 |
|---|---|---|
| Bridge 全 crate | 299/299 | `cargo nextest -p codex-rust-rig-bridge --locked --retries 0` |
| core rig 家族 + 图片限额 + stress | 68/70，2 失败均定性 | stress 首跑缺 `test_stdio_server` fixture（构建后重跑 PASS）；`anthropic_interrupt_during_tool_runs_it_once` 因 Ubuntu 24.04 AppArmor 限制非白名单二进制 unshare userns（`uid_map: Operation not permitted`，bwrap loopback 无法建立）——同测试 macOS 通过，环境性，非代码回归；机器证据：`unshare -Urn` 同样失败 |
| utils image/output-truncation | 44/44 | 含像素炸弹拒绝、36MP 合法解码 |
| live-tests replay 整 crate | 195/195 | tar 包无 `.git` 曾致 repo_root/receipt 失败；`git init` 真实仓库后全绿 |
| nuwax env 离线三件套 | 3/3 | 需远程 `cargo build -p codex-exec`（receipt 以整树为准） |

关键对照结论：第六轮在 mac worktree 失败的 startup-context/grace 簇
（mcp_optional_startup_grace×3、interrupt_during_mcp_startup×2、
shutdown_cancels_startup_prewarm、realtime startup×2）在 Linux
**全部通过**——确证那是 mac worktree 所在 /Volumes 卷的 IO 时序问题，
与迁移和图片批代码无关。

未做（Linux）：全量 workspace/CI、代理/TLS、真实厂商 API。Windows
原生验证流水线同日启动（收据见后续轮次）。



## 进展（2026-10-15 第九轮：Windows 原生收据）

候选 `4ca0e4fa0`（源码 tar → 192.168.32.235 `win`，原生 Git Bash + MSVC，
rustc 1.95.0，无 WSL；注意 scp 与交互 bash 的 `/tmp` 映射不同，落点
`C:\tmp\rig044-val`）。

| 范围 | 结果 | 备注 |
|---|---|---|
| Bridge 全 crate | 297/297 | 比 macOS/Linux 少 2 项为平台 cfg 门控 |
| utils image/output-truncation | 44/44 | 同 Linux |
| core rig 家族（wire 级） | 58/59 | 唯一失败 `anthropic_interrupt_during_tool_runs_it_once`：Windows 上 shell 工具走 exec approval（`powershell sleep 60` 请求批准而非直接运行），测试夹具按 Unix 沙箱语义编写——环境性，同测试 macOS 通过、Linux 因 AppArmor userns 失败（第八轮）；迁移与图片批代码无关。后续可给该夹具做平台适配（独立小补丁） |
| live-tests replay 整 crate | 187/187 有效 | tar 无 `.git` 同 Linux；`git init` 真实仓库后 replay 全绿 |
| nuwax env 离线三件套 | 3/3 | 远程构建 codex-exec 后（receipt 以整树为准） |
| 20 截图 stress | PASS | test_stdio_server 构建后 5.8s 真实执行 |

三平台未验边界（统一）：全量 workspace/CI、代理/TLS、真实厂商 API/识图
（待付费授权）、ACP dev（本轮不付费约束下顺延）、`anthropic_interrupt_
during_tool_runs_it_once` 的跨平台夹具适配。



## 进展（2026-10-15 第十轮：ACP dev 联调 + Guardian 第三方审查修复）

用户授权付费 API 后执行（此前顺延项）。设置：ACP 客户端
`fork-codex-acp-ts` dev（v1.2.18，`CODEX_PATH` → 候选 `codex` app-server
二进制，提交点 f6b1ad38f 前重建、sha256 d1f034e5…）；驱动与 MCP 截图
夹具在 ACP 仓库 `tmp/`（gitignored，可复跑：`bash
tmp/run-acp-validation.sh`，密钥仅按环境变量名继承自主工作区
.env.local，不落任何工件）。vendor：MiMo chat wire（mimo-v2.6-flash），
GLM 对照一轮。

| 场景 | 结果 | 证据 |
|---|---|---|
| S1 引导 + MCP server | PASS | session 建立、screenshots server ready |
| S2 环境变量传递 + 真实付费回合 | PASS | ACP→候选→rig→MiMo 往返 "pong" |
| S3 截图工具执行 | PASS（实质） | app-server 日志 `take_screenshot status=completed`（修复后连续多轮）；driver 端 ACP 完成事件的 id/title 关联是夹具形状问题，不影响判定 |
| S3b 真实识图 | PASS | 修复后 MiMo 对纯红 64×64 截图答 "red"/"Red"（修复前同模型答 "gray"/"unknown"——当时工具被拒、模型无图瞎猜） |
| S4 取消 | PASS | stopReason=cancelled |
| S5 压缩 + 续轮 | PASS | /compact（compaction_update×2）后 "post-compact-ok" |
| S6 冷恢复 | PASS | 杀进程→新 agent→loadSession→召回 ACP-COLD-42 |

**发现并修复的候选缺陷**（提交 `f6b1ad38f`）：Guardian 自动审批审查的
审查模型从内置 OpenAI 目录解析（API-key provider 默认 gpt-5.6-luna），
但审查请求走当前配置 provider——第三方端点不认 OpenAI 模型名（MiMo
400 "Unsupported model"、GLM 1211 "模型不存在"）→ 审查必败 → 所有
走 guardian 审批的 MCP 工具被拒执行（生产 nuwax 用户全部受影响）。
修复：`resolve_review_model` 在非内置 OpenAI provider 上优先使用当前
活动模型（provider 唯一确定可服务的 id）；用户显式
auto_review_model_override 仍然优先；remote-v2 压缩审查同函数同样受益。
单测钉住两分支；guardian 族 155/155。GLM 修复前对照轮（S1-S6 除识图
外同样 PASS）保留在 /tmp/acp-val-glm.log。

ACP 侧无独立缺陷：首轮 S6 失败是驱动未先 initialize（调用顺序），
工具完成事件形状为 SDK 既有行为——均未改动 ACP 仓库（tmp/ 均
gitignored）。识图此前未证实的主因正是本缺陷（图片从未送达）；修复
后 MiMo flash 即具备识图能力，与「待指定 vision 模型」的旧结论一并
更正。



## 进展（2026-10-15 第十一轮：记忆模型同类缺陷修复 + 付费 v2 录制）

用户授权后两项收口：

1. **记忆管线同类缺陷**（提交 `d115bfdbe`）：phase1 提取与 phase2 巩固
   此前从 OpenAI 目录取 gpt-5.6-luna／terra，而请求走当前 provider——
   第三方端点 400，记忆管线整体静默失效。两处改经
   `reachable_memory_model`（显式 memories.* 配置恒优先；内置 openai 保
   留目录首选；其他 provider 用当前活动模型）。guardian-v2 async_scorer
   与 sync_reviewer 两处同类点一并接入（经
   `core::guardian_review::reachable_review_model_id` 公开导出）。
   live 验证（MiMo chat，付费）：巩固 agent 线程以 mimo-v2.6-flash
   真实派发（workspace-write／medium effort／rig 桥日志在案）；提取任务
   受 `min_rollout_idle_hours` 小时级门控，新会话按设计不触发。
   遗留观察（非本缺陷）：短生命周期 `codex exec` 在用户回合结束即退
   出，后台巩固 agent 被杀记 `failed_agent`——长驻宿主（desktop/ACP）
   不受影响；单列待后续验证。
2. **付费 LIVE_CASSETTE=record 录制**（fixture 提交同上）：三厂商
   bridge_live **84/84 真实绿**；150 个 fixture 文件以 0.44 v2 格式
   （items+terminal，实际 recorder 写入）替换 v1 载荷（git 历史可溯，
   legacy 翻译单测仍在）。录后离线复验 replay+roundtrip **91/91 全绿**；
   全部 fixture 扫描凭据泄漏（认证头名与真实键值）：零。
   Guardian-v2 两个既有失败（pool_pressure、flaky handshake）在
   /tmp 基线 worktree 复现，与两修复无关，单列。



## 执行清单

- [x] Phase A 非流部分（12/16 文件）
- [x] Phase A 流核心 4 文件 + cassette（280/280 绿，clippy 0 警告）
- [x] Phase B 测试/夹具/cassette 适配（fixture v2 + legacy v1 翻译 + live-tests 编译面）
- [ ] Phase C 验证：三 wire 出站、工具配对、终局/迟到 usage、cap、取消、
      retry/idle/HTTP 预算、代理/TLS、signed reasoning 旧会话、resume/compact、
      app-server env、live-tests 离线；`just bazel-lock-update`
      —— 第五轮真实执行收据（含复审假通过纠正与 7 项缺陷修复）：
      bridge 299/299；显式 Rig core 家族 50/50（含 rig_output_cap 30/30
      真实执行、新增 rig_image_history 4/4）；live-tests replay 195/195
      （含离线 v2 cassette 往返 7/7）。app-server env 公共接口 9/9、
      nuwax env 离线 3/3、Bazel lock 同步沿用复审轮（代码路径未变）。
      —— 三批已收口（第六至九轮）：缺陷修复 + 生命周期证据 + 图片
      入口硬限 + 20 截图 stress；Linux/Windows 定向平台收据见第八/九轮。
      —— 第十轮补齐：ACP dev 联调全场景（含真实识图）+ Guardian 第三方
      审查缺陷修复（f6b1ad38f）。
      —— 第十一轮补齐：记忆模型同类缺陷修复（d115bfdbe）+ 付费 v2
      录制 84/84（录后离线 replay 91/91）。
      —— 未闭环：全量 workspace/CI、代理/TLS、interrupt 夹具跨平台
      适配、guardian-v2 两个既有失败、巩固 agent 短进程宿主行为。
- [x] reasoning v1 旧 DTO 解码保留（v2 写入、v1 解码+转换、rollout 不改写）
