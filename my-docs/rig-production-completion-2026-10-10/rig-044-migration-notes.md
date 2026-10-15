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

## 进展（2026-10-15 第二轮：流核心完成，280/280 绿）

流核心 4 文件 + cassette 重写完成（convert_response/response_tools/stream_pump/
stream/cassette + reasoning v1 解码共用），lib+tests 编译 0 错，280 测试全过，
clippy 0 警告，codex-core/codex-live-tests 编译通过（live-tests 的 recorder/
fixture 接口已随迁）。按 0.44 语义的设计决策与坑：

1. **历史适配器 `adapt` 是最大的语义断层**（0.42 无此层）。`Model::stream` 经
   `Operation::prepare` 调 `history::adapt_for`，规则：相邻 user 消息合并、
   空内容/空白文本 turn 丢弃、跨模型 turn 的 Opaque/签名 Reasoning 降级为纯
   文本、无工具声明时 tool call 历史折叠成文本（tools_as_text）。桥接对策：
   - 每个重建的历史 assistant turn 挂同源 `Origin{api:"anthropic.messages",
     provider:"anthropic", model, context}`（`anthropic_history_origin`）；context
     指纹镜像 rig 私有的 `history::context_of`（`anthropic_context_fingerprint`，
     输入 = 排序后 tools + leading system + raw tools），仅在模型不绑定上下文时
     计算（binds_context 模型在 adaptive thinking 下不做指纹比较——与 driver 的
     drops_unbound_items 谓词一致）。**rig 升级时这两处镜像必须同步**。
   - 签名/redacted reasoning 挂 `with_native(wire item)`：同源 turn 的 native
     item 逐字回放（`Replay::Item`），这是签名 thinking/redacted_thinking 上
     wire 的唯一通道；流式解码时 redacted 的 payload 只在 native item 里
     （canonical text 为空），legacy v1 签名才合成 wire 块。
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
4. pause_turn 从 wire tee 解析（`anthropic_stop_reason`），tee 同时喂 hosted
   工具块与暂停续链。
5. Responses 透传手构 URI 不带 query——transport `prepare` 统一追加（曾双重
   拼接）。
6. `tool_input_streaming: Off`（0.44 默认 Eager 会给每个工具加
   eager_input_streaming 字段，偏离 0.42 wire 形状）。
7. cassette 双版本：fixture v2 记 `Item<StreamEvent>` + TerminalRecord，v1
   （0.42 untagged）在读侧翻译（Name delta→Start、Delta→Arguments、ToolCall→
   End、Final→terminal；`translate_legacy_events`）；`Part` 经 serde 透明形式
   构造（`part_from_index`，失败走 Stream 错误不 panic）。legacy usage 换算：
   0.44 input = 旧 input + cached + cache_creation。
8. 测试侧的 0.44 适配（fork-contract adaptations，均有注释标记）：无工具声明
   的纯历史请求在 wire 测试里显式声明工具（生产恒有工具声明）；opt-out/
   去重尾部等无锚点场景的相邻 user 合并期望改为合并后的形状。

### 已知残留（Phase C 复核项）
- `anthropic_context_fingerprint` 与 `anthropic_history_origin` 镜像 rig 内部
  逻辑，rig 版本升级时需人工核对 `Operation::prepare`/`history::context_of`/
  `drops_unbound_items` 三处。
- 绑定上下文的 Claude 模型（binds_context=true）走"无指纹比较"分支，依赖
  provider 侧 drop_block；该路径无离线测试覆盖（模型目录驱动的行为）。
- live-tests 仅离线编译验证（recorder/fixture 迁移后），录制/回放轮次未跑。

## 执行清单
- [x] Phase A 非流部分（12/16 文件）
- [x] Phase A 流核心 4 文件 + cassette（280/280 绿，clippy 0 警告）
- [x] Phase B 测试/夹具/cassette 适配（fixture v2 + legacy v1 翻译 + live-tests 编译面）
- [ ] Phase C 验证：三 wire 出站、工具配对、终局/迟到 usage、cap、取消、
      retry/idle/HTTP 预算、代理/TLS、signed reasoning 旧会话、resume/compact、
      app-server env、live-tests 离线；`just bazel-lock-update`
- [x] reasoning v1 旧 DTO 解码保留（v2 写入、v1 解码+转换、rollout 不改写）
