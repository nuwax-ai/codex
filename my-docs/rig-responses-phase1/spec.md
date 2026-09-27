# Rig Responses 第一阶段 — Spec（规范）

日期：2026-09-27（实施完成 2026-09-28）· 状态：已完成（证据见 tasks.md）· 上游文档：
`my-docs/rig-bridge-implementation-plan.md`、`my-docs/rig-protocol-audit-2026-09-27.md`、
`my-docs/rig-live-provider-audit-2026-09-27.md`（工作区未提交修正以当前文件为准）。

## 1. 目标

第三方 provider 通过 Rig 请求模型时，**显式 `wire_api` 决定实际线协议**：

| 配置 | 实际请求 |
|---|---|
| `wire_api = "responses"`（默认 rig 桥） | Rig OpenAI Responses 客户端 POST `{base}/responses` |
| `wire_api = "responses"` + `experimental_bridge = "native"` | Codex 原生 transport（不变） |
| `wire_api = "chat"`（默认 rig 桥） | Chat Completions（不变） |
| `wire_api = "anthropic"`（默认 rig 桥） | Anthropic Messages（不变） |
| 内置 OpenAI / Bedrock | 原生默认路径（不变） |

修正的既有事实错误：rig-core 0.42.0 **已支持** Responses
（`providers::openai::Client` 默认即 Responses 客户端，`CompletionsClient` 才是 Chat）。
缺口在 fork 接入层——此前 `wire_api="responses"` 被静默转为 Chat，且 URL 猜测
（`/anthropic` 路径嗅探）可覆盖显式协议。

### 架构：同协议直通（same-protocol passthrough）

```
Codex ResponsesApiRequest
→ 直接 serde 序列化（不经 rig CompletionRequest / 归一化事件）
→ rig Client::post_sse("/responses") 构造请求（URI 拼接、Bearer 头、Ext 定制）
→ RigHttpClient（fork 装饰器，复用现有鉴权/查询参数/CA/请求关联 ID/错误脱敏）
→ rig HttpClientExt::send_streaming（reqwest 0.13 真正发送）
→ 原始 HTTP/SSE 字节流
→ codex-api Responses 解码器（ResponsesStreamEvent + process_responses_event）
→ 严格终止 pump（fork 桥新增）
→ Codex ResponseStream
```

Rig 真正参与 HTTP 发送（非仅类型装饰）；Codex 已建模字段全量保留
（instructions、input、tools、tool_choice、parallel_tool_calls、reasoning、text、
include、store、stream_options、service_tier、prompt_cache_key、client_metadata 等
随 `ResponsesApiRequest` 直接序列化）。

## 2. 非目标（第二阶段）

- 旧会话跨协议切换的历史迁移与请求时投影（原始历史＋来源信息＋目标协议能力）。
- 厂商专属推理签名/加密内容跨厂商重放。
- WebSocket Responses、远程压缩。
- GenAI 桥功能扩展（仅做接口泛化的编译适配；`wire_api="responses"` 时明确报错）。

## 3. 验收行为

### 路由
- `uses_chat_bridge` 更名为 `uses_model_bridge`：使用桥 ≠ 实际发 Chat。
- 显式 wire_api 决定协议；URL 嗅探不再参与桥分发（`chat_wire_protocol` 移除，
  rig 桥内 `from_base_url` 保留为显式回退工具，不用于分发）。
- 日志/监控 span 反映真实协议与端点（responses 不再记为 chat/completions）。

### Responses 传输
- 保留现有鉴权（Bearer/网关头透传）、自定义请求头、查询参数重编码、自定义 CA、
  idle timeout、取消（drop 即断）、HTTP 状态及错误响应头、请求关联 ID
  （x-request-id 回退链）。无叠加重试：非 2xx 在请求 await 即返回
  `TransportError::Http`（保留 status/headers/body/retry_after，供 401 恢复环使用）。
- Responses 路径**不经过**任何 Chat 专属处理：角色改写、消息合并、工具过滤/
  strict 注入、namespace 展平、reasoning envelope 生成/回放。
- 历史中若出现 rig chat/anthropic 回放 envelope（跨协议旧会话）→ **Fail Fast**
  报错（InvalidRequest），不静默丢弃、不猜测来源。第一阶段仅支持新会话与
  本阶段创建的同协议会话恢复。

### 事件与严格终止（仅新增 Rig Responses 路径；native 行为不变）
- `response.failed` / `response.incomplete` → 立即报错并停止（沿用 codex-api
  错误分类：ContextWindow/Quota/RateLimit/…）。
- 流结束而无合法完成事件 → 报错（含已缓冲的失败原因）。
- 已识别关键事件的必需字段损坏（如 `output_item.done` 的 item 不可解析、
  `response.created` 缺 response）→ 报错，不吞掉后假装成功。
- 未知非关键事件（`response.in_progress`、`*.done` 非 delta 类等）→ 兼容忽略。
- 容忍兼容网关附带的 `data: [DONE]` 帧（GLM 实测会附带；终态以
  `response.completed` 为准）。
- 终止后不再发送成功 Completed；不合成与服务端重复的 Created/item ID/Completed。

### 来源元数据（provenance）
- `codex-history` 的 `CodexHarnessMetadata` 新增可选
  `model_output_provenance: Option<ModelOutputProvenance>`
  （wire_protocol / bridge / provider / model，均不含凭证）。
- 在生成该输出的请求上下文中于持久化时捕获；恢复时不以当前配置回填。
- 不发送给模型（envelope 拆包仅取 ResponseItem，请求构建只消费 ResponseItem）。
- 旧记录缺字段仍可读取（`#[serde(default)]`）。

## 4. 真实厂商边界

- MiMo / GLM：Rig Responses 文本、工具调用、第二轮结果回传（端点见
  rig-live-provider-audit：MiMo `/v1/responses`、GLM `/api/v1/responses`）。
- Step：无 Responses 端点实测——`LIVE_<VENDOR>_RESPONSES_URL` 未配置时
  Responses 场景**显式跳过**（不得回退 Chat URL；跳过不计为通过）。
- hosted tools：不由通用 Responses 传输层静默删除——由配置层能力声明处理
  （沿用 `web_search = "disabled"` 等既有机制）。
- "参数被接受"与"参数被遵守"分别报告。

## 5. 破坏性变更登记

- `wire_api="responses"` + `/anthropic` URL + 默认桥：此前嗅探为 Anthropic 线，
  现在发真实 Responses——须改为显式 `wire_api="anthropic"`。
- genai 桥 + `wire_api="responses"`：此前被转为 Chat，现在明确报错。
- `ChatModelBridge`/`ChatWireProtocol`/`uses_chat_bridge`/`chat_wire_protocol`
  更名为 ModelBridge 族（fork 内部 API，全调用点同步）。
