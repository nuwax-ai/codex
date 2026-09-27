# Rig Responses 第一阶段 — Plan（技术方案）

## 1. 模块与接口变化

### codex-api（共享桥接口泛化）
- `src/bridge.rs`：
  - `ChatWireProtocol` → **`ModelWireProtocol`**，新增 `Responses` 变体
    （Responses | ChatCompletions | Anthropic）。
  - `ChatModelBridge` trait → **`ModelBridge`**（`stream()` 签名不变，协议参数换新枚举）。
  - 删除 `chat_wire_protocol(wire_anthropic, base_url)`（URL 嗅探退出分发）。
    协议映射在 core 分发点按 `WireApi` 显式 match。
- `src/sse/mod.rs`：`pub(crate) mod responses` → `pub mod responses`，
  公开 `ResponsesStreamEvent` / `process_responses_event`（复用解码器，
  不复制解析器；native 行为零变化）。

### model-provider-info
- `uses_chat_bridge()` → **`uses_model_bridge()`**（语义：路由到桥，桥内再按
  wire_api 分协议；内置 OpenAI/Bedrock 与 `experimental_bridge="native"` 保持原生）。

### codex-rust-rig-bridge（新增 Responses 模块）
- `client.rs`：`RigProtocol` 增加 `Responses`；`api_key_from_headers`/
  `default_headers` 对 Responses 按 Chat 处理（Bearer 重建、其余头透传）；
  新增 `build_responses_client()` →
  `rig_core::providers::openai::Client<RigHttpClient>`（Responses 客户端）。
  `reasoning_source` 不用于 Responses 路径（无 envelope 回放）。
- `transport.rs`：`RigProtocol::Responses` 时跳过全部 body 注入与 SSE 终止
  改写（透传原始字节；请求关联 ID 捕获、错误脱敏、查询参数重编码保留）。
- `responses.rs`（新文件，<500 LoC 目标）：
  `stream_responses_via_rig(request, provider, auth, extra_headers, idle_timeout)`：
  1. Fail Fast：input 中存在 `codex-rig-reasoning-v1:` envelope → InvalidRequest。
  2. 头合并 + auth 解析（与 stream_via_rig 相同）。
  3. 构造 RigHttpClient（protocol=Responses，无注入字段）+ rig Responses client。
  4. `serde_json::to_vec(&request)` → `client.post_sse("/responses")?.body(bytes)?`
     → `HttpClientExt::send_streaming(&client, req)`。
  5. 错误映射：`http_client::Error::InvalidStatusCodeWithDetails{status,headers,body}`
     → `ApiError::Transport(TransportError::Http{..})`（含 Retry-After 解析），
     供 core 401 恢复环触发；其余 → Network/InvalidRequest（沿用 sanitize 脱敏）。
  6. 字节流适配：rig `BoxedStream<Result<Bytes, rig Error>>` →
     `codex_http_client::ByteStream`（`TransportError::Network`，字符串经
     `without_url` 脱敏后转换）；组装 `StreamResponse{status, headers, bytes}`。
  7. 严格 pump（见 §2）→ `ResponseStream{rx, upstream_request_id}`。
- `bridge_impl.rs`：`RigChatBridge` → **`RigModelBridge`**；协议映射含 Responses。
- `stream.rs`：`stream_via_rig` 对 `RigProtocol::Responses` 委托 responses 模块
  （Chat/Anthropic 路径与 recorder/cassette 完全不变）。

### codex-rust-genai-bridge（仅编译适配）
- trait 更名适配；`ModelWireProtocol::Responses` → InvalidRequest 明确报错。

### core/src/client.rs
- `dispatch_chat_bridge` → `dispatch_model_bridge`：`match wire_api`
  显式映射协议（无 URL 嗅探）。
- `stream_chat_api` → `stream_model_bridge`：span/telemetry 端点按实际
  wire_api 计算（responses → `api.path="/responses"`）。
- 路由骨架不变：`uses_model_bridge()` → 桥分发；native/websocket 分支不动。
- 错误提示文案同步（"model bridge"）。

### codex-history（provenance）
- `CodexHarnessMetadata` 新增 `model_output_provenance: Option<ModelOutputProvenance>`
  （`#[serde(default, skip_serializing_if=Option::is_none)]`；struct 派生
  Serialize/Deserialize/JsonSchema/Eq/Clone/Default）。
- 持久化点：core 在把模型生成的输出 item 写入会话历史时，从当前
  provider info（wire_api + 是否桥 + 桥名）与模型 slug 捕获；旧记录缺省
  读取不变。

## 2. 严格终止 pump（bridge `responses.rs`）

- `eventsource_stream` 逐帧；每帧 idle timeout（`tokio::time::timeout`）。
- `data: [DONE]` → 忽略（兼容 GLM 附带）。
- 帧非合法 JSON → `Err(Stream("malformed SSE event ..."))` 终止。
- `process_responses_event` 返回：
  - `Ok(Some(ev))` → 转发；`Completed` → 转发后 return。
  - `Ok(None)` 且 kind ∈ 关键集合 {response.created, response.output_item.added,
    response.output_item.done, response.output_text.delta,
    response.custom_tool_call_input.delta, response.reasoning_summary_text.delta,
    response.reasoning_summary_text.done, response.reasoning_text.delta,
    response.completed, response.failed, response.incomplete}
    → `Err(Stream("event <kind> missing required fields"))` 终止。
  - `Ok(None)` 其他 kind → 忽略（trace）。
  - `Err(ResponsesEventError)` → 立即转发错误并 return（及时报错，
    终止后不再发成功 Completed）。
- 流尽无终态 → `Err(Stream("stream closed before response.completed"))`
  （或先前缓冲的失败错误）。
- 接收端 drop → 停止（取消语义）。

## 3. 数据流错误处理

- Fail Fast：envelope 混入、非 2xx、坏帧、关键字段缺失、断流、超时——全部
  显式错误；无自动重试（core 层已有 stream_max_retries）。
- 凭证安全：沿用 transport 的 `without_url`/Debug 脱敏；错误消息不含
  Authorization/查询参数值。

## 4. 测试架构

| 层 | 位置 | 内容 |
|---|---|---|
| 单元 | rig-bridge `responses_tests.rs` | 关键帧分类、[DONE] 容忍、坏帧、断流、envelope 拒绝 |
| wire（本地 HTTP） | rig-bridge `tests/wire/responses_*.rs` | 出站 JSON 逐字段对照、鉴权/头/查询参数、SSE fixtures（文本/reasoning/并行工具/回传/failed/incomplete/坏事件/断流/超时）、native 不受影响回归 |
| 集成 | `core/suite`（test_codex + mock /responses） | 多轮工具闭环、保存恢复（同协议）、历史顺序、工具配对、provenance 持久化、旧记录缺省读取、chat/anthropic/native 回归 |
| live | `live-tests` | bridge 级 responses 场景 + exec 级 responses-rig-default（修正为显式 Responses 端点）；SSE fixture 录制/回放 |

## 5. 分批

1. **批次 1**：接口泛化 + Rig Responses 传输 + wire 级测试（路由矩阵/JSON 对照/传输错误）。
2. **批次 2**：严格终止 + provenance + 集成测试。
3. **批次 3**：live 厂商验证 + 文档修正（审计结论、配置示例）+ 收尾核对。

约束：生产代码无 unsafe/unwrap/expect；模块 <500 LoC（不含测试）；测试经
`just test`；完成后 scoped `just fix` + `just fmt`；不自动 commit/push。
