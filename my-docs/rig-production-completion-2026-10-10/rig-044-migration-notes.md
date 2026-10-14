# Rig 0.42 → 0.44 迁移工单（进行中）

分支 `codex/rig-044-20261014`（基于整合分支 `5b6b51457`）。pin 已升
`rig-core = "=0.44.0"`（bridge + live-tests），`cargo update -p rig-core` 引入
`rig-http 0.44`/`rig-tungstenite 0.44`。**未完成、未验证、未合回**。

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
   读+写（0.42 是分开的）。map_usage/rig_usage_report 不得再加 cache；Anthropic
   溢出 sanitize 保留。wire_reported_usage 注入点从流内 Final 移到 finish() 之前。
5. **`Model::stream()` 不再是 future**（返回 Result<Streamed, ProviderError>）；
   stream.rs 的 `.await` 与"首事件预取"逻辑保留但类型重接；零发送取消语义变化
   （新版首次 poll 才发送——需要实测验证）。
6. **错误类型**：`CompletionError` → `ProviderError`；`map_completion_error` 重接。
7. **事件录制/cassette**：rig_events fixture 记录的是 0.42 StreamedAssistantContent
   序列化——cassette.rs 的 RigEventFixture 需要版本化（v2 记 Item<StreamEvent>），
   旧 0.42 fixture 的解码路径保留（与 reasoning v1 同理）。

## 执行清单
- [x] Phase A 非流部分（12/16 文件）
- [ ] Phase A 流核心 4 文件 + cassette（上述 7 点设计）
- [ ] Phase B 测试/夹具/cassette 适配（rig_event fixture 结构变化）
- [ ] Phase C 验证：三 wire 出站、工具配对、终局/迟到 usage、cap、取消、
      retry/idle/HTTP 预算、代理/TLS、signed reasoning 旧会话、resume/compact、
      app-server env、live-tests 离线；`just bazel-lock-update`
- [x] reasoning v1 旧 DTO 解码保留（v2 写入、v1 解码+转换、rollout 不改写）
