# 三协议字段转换完整审计(2026-09-25,逐行代码核对)

> 基于对 codex-rust-rig-bridge(1196+766 行)和 codex-rust-genai-bridge(977+541+320 行)
> 的逐行审计。每条结论附代码行号。图例:
> ✅ 完整映射 | ⚠️ 映射但有限制 | ❌ inherent 丢失(协议无对应)| 🔴 可修丢失(有方案但未修)

---

## 一、请求方向:ResponsesApiRequest → 三种协议

### 1.1 顶层字段

| codex 字段 | rig→Chat Completions | rig→Anthropic Messages | genai→Chat/Anthropic |
|---|---|---|---|
| `model` | ✅ `CompletionRequest.model`(req:99) | ✅ 同左 | ✅ `exec_chat_stream(&model,...)` 独立参数 |
| `instructions` | ✅ 首条 `Message::System`(req:29-36) | ✅ rig Anthropic adapter 提取 system 到顶层 `system` 字段 | ✅ `chat_req.with_system()` |
| `input` | ✅ → `chat_history: Vec<Message>`(req:25) | ✅ 同左(rig adapter 按角色分发) | ✅ → `ChatRequest.messages` |
| `tools` | ⚠️ 经 JSON 往返(req:40-45),`strict` 字段丢失 | ⚠️ 同左 + Anthropic adapter 生成 `input_schema` | ⚠️ 同左 |
| `tool_choice` | ✅ `map_tool_choice`(req:106,177-191) | ✅ 同左(rig adapter 映射 `tool_choice`) | ✅ `ChatOptions.tool_choice`(stream:220,237-244) |
| `parallel_tool_calls` | ⚠️ 仅 false 时进 `additional_params`(req:164-166) | ❌ **Anthropic wire 全部丢弃**(stream:79,`additional_params=None`) | ⚠️ `extra_body`(stream:221-223);Anthropic 由 genai adapter 自行决定 |
| `reasoning.effort` | ✅ 全档直传 `additional_params["reasoning_effort"]`(req:127-130,152-160) | ❌ Anthropic wire 丢弃(同上) | ⚠️ genai 原生 effort;`Ultra→Max`、`Persistent→Max`、`Custom→High` 降级+warn(stream:252-281) |
| `reasoning.summary` | ❌ 未读(req:152-160 只取 effort) | ❌ 同左 | ❌ 未读 |
| `reasoning.context` | ❌ 未读 | ❌ 同左 | ❌ 未读 |
| `store` | ❌ 无 Chat 对应 | ❌ 同左 | ⚠️ `with_store(true)` 但 genai Chat 路径不序列化(P3-1) |
| `stream` | ✅ 桥始终流式 | ✅ 同左 | ✅ 同左 |
| `stream_options` | ❌ rig 自动注入 `include_usage` | N/A | ✅ genai `capture_usage=true` 自动 |
| `include` | ❌ Responses-only | ❌ 同左 | ❌ 同左 |
| `service_tier` | ⚠️ `additional_params`(req:161-163) | ❌ Anthropic wire 丢弃 | ✅ `ChatOptions.service_tier`(stream:185-196) |
| `prompt_cache_key` | ⚠️ `additional_params`(req:167-169) | ❌ Anthropic wire 丢弃 | ✅ `ChatOptions.prompt_cache_key`(stream:215) |
| `text.verbosity` | ⚠️ `additional_params["verbosity"]`(req:135-151) | ❌ Anthropic wire 丢弃 | ✅ `ChatOptions.verbosity`(stream:198-205) |
| `text.format` | ⚠️ → `output_schema`(req:55-59);`name`/`strict` 丢失;gated 时注 `response_format` 进 `additional_params`(req:67-96) | ❌ Anthropic wire 丢弃 | ⚠️ → `JsonSpec`;`description=None`、`strict` 由 genai 决定(stream:206-213) |
| `text.format.strict` | ❌ rig 恒 `true` | N/A | ❌ genai JsonSpec 无 strict 控制 |
| `client_metadata` | ❌ 未读 | ❌ 同左 | ❌ 同左 |
| `access_programs` | ❌ 未读(内部字段) | ❌ 同左 | ❌ 同左 |

### 1.2 Anthropic wire 专有丢弃(rig 桥,stream.rs:68-80)

rig 桥在 Anthropic 协议下 **清空整个 `additional_params`**(GLM 实测:未知字段会禁 thinking)。
被丢弃的:`parallel_tool_calls`、`reasoning_effort`、`service_tier`、`prompt_cache_key`、
`verbosity`、`response_format`。同时 `max_tokens` 默认 16384。

### 1.3 ResponseItem 输入变体

| 变体 | rig 桥 | genai 桥 |
|---|---|---|
| `Message(user)` | ✅ → `Message::User` | ✅ → `ChatRole::User` |
| `Message(assistant)` | ✅ → `Message::Assistant`,**与前一 assistant 合并**(req:219-227) | ✅ 同左(req:76-84) |
| `Message(developer/system)` | ⚠️ 降为 `Message::User`(Chat 只有两种角色,req:229-236) | ✅ → `ChatRole::System`(req:316) |
| `Message.id` | ❌ 丢弃 | ❌ 丢弃 |
| `Message.phase` | ❌ 丢弃 | ❌ 丢弃 |
| `Reasoning.encrypted_content` | ✅ → `AssistantContent::Reasoning`(DeepSeek 回传,req:238-256) | ✅ → `ContentPart::ReasoningContent` + `thought_signatures`(req:86-121) |
| `Reasoning.summary` | ❌ 丢弃 | ❌ 丢弃 |
| `Reasoning.content`(ReasoningText) | ❌ 只读 encrypted_content | ❌ 同左 |
| `FunctionCall(name/arguments/call_id)` | ✅ → `AssistantContent::ToolCall`(req:257-273) | ✅ → `ContentPart::ToolCall`(req:103-135) |
| `FunctionCall.namespace` | ❌ 丢弃 | ❌ 丢弃 |
| `FunctionCall.encrypted_function_args` | ❌ 丢弃 | ❌ 丢弃 |
| `FunctionCallOutput(call_id/output)` | ✅ → `UserContent::ToolResult`(req:274-304) | ✅ → `ChatMessage::tool`(req:159-178) |
| `FunctionCallOutput.success` | ❌ 丢弃 | ❌ 丢弃 |
| `CustomToolCall(name/input/call_id)` | ✅ 包装为 `{"input": string}` schema(req:305-323) | ⚠️ 与 function 同一路径,无包装(req:136-158) |
| `CustomToolCallOutput` | ✅ → ToolResult text(req:324-350) | ✅ 同左(req:179-195) |
| `AgentMessage(InputText)` | ✅ → assistant text(req:364-400) | ✅ 同左(req:209-238) |
| `AgentMessage(EncryptedContent)` | ⚠️ 跳过+debug 日志 | ⚠️ 跳过 |
| `AdditionalTools` | ✅ 工具定义合并(req:48-52),消息跳过 | ✅ 同左(req:46-50) |
| `LocalShellCall`~`CompactionTrigger` | ❌ 跳过(内部事件) | ❌ 同左 |
| `Other` | ❌ 跳过+warn | ❌ 同左 |

### 1.4 ContentItem(Message 内)

| 变体 | rig 桥 | genai 桥 |
|---|---|---|
| `InputText` / `OutputText` | ✅ → `Text`(req:515/550) | ✅ → `ContentPart::Text` |
| `InputImage(Inline http URL)` | ✅ → `DocumentSourceKind::Url`(req:528-533) | ✅ → `Binary::from_url(content_type, url)` |
| `InputImage(Inline data:URL)` | ✅ 解码为 `Base64` source + mime 映射(req:482-505)——Anthropic wire 必需 | ⚠️ **data:URL 当作 URL 传**(content_type 推断:按扩展名,默认 png;genai Anthropic 会 warn 丢弃) |
| `InputImage(File)` | ❌ 丢弃+warn(req:519-524) | ❌ 丢弃+warn(req:334-339) |
| `InputImage.detail` | ❌ 恒 None(rig 有字段但 codex 枚举不匹配) | ❌ 丢弃 |
| `InputAudio` | ❌ 丢弃+warn(req:537-541) | ❌ 丢弃+warn(中文注释) |

### 1.5 工具输出图片(rig 桥修复后)

| FunctionCallOutputContentItem | rig 桥 | genai 桥 |
|---|---|---|
| `InputText` | ✅ 200KB 上限(req:436-438) | ✅ 200KB 上限 |
| `InputImage(data:URL)` | ✅ → Base64 image block + mime(req:439-461) | ⚠️ → 占位符 `[image attached — omitted]` |
| `InputImage(http URL)` | ✅ → URL source image block | ⚠️ → 占位符 `[image: url]` |
| `InputAudio` | ❌ 丢弃+warn | ⚠️ → 占位符 `[audio omitted]` |
| `EncryptedContent` | ⚠️ → 占位符 `(encrypted content omitted)` | ⚠️ 同左 |

### 1.6 工具定义详情

| 类型 | rig 桥 | genai 桥 |
|---|---|---|
| `function` 工具 | ✅ name+description+parameters;**`strict` 丢失** | ✅ 同左;`strict` 可选保留 |
| `custom` 工具 | ✅ 包装为 `{"input": string}` schema + 响应侧还原 CustomToolCall | ⚠️ 与 function 同路径;**无 schema 包装,响应侧不还原 Custom** |
| `namespace` 工具 | ✅ 展平为 `mcp__ns__tool`(req:612-635) | ✅ 同左(req:375-390) |
| 宿主工具(web_search 等) | ❌ drop+warn(req:640-642)——fork 设计 | ⚠️ 与 function 同路径(有名则透传,无名则丢弃) |
| 工具的 `defer_loading` | ❌ 丢弃 | ❌ 丢弃 |

---

## 二、响应方向:Provider → ResponseEvent

### 2.1 rig 桥(StreamedAssistantContent → ResponseEvent)

| rig 事件 | codex 事件 | 字段映射 |
|---|---|---|
| `Text{text}` | `OutputTextDelta(text)` + 首次 `OutputItemAdded(Message)` | ✅;`Text.additional_params`(citations 等)❌ 丢弃 |
| `ReasoningDelta{reasoning}` | `ReasoningContentDelta{delta, content_index}` + 首次 `OutputItemAdded(Reasoning)` | ✅;`id`/`provider_id` ❌ 丢弃 |
| `ToolCallDelta Name` | 首次 `OutputItemAdded(FunctionCall)`(call_id=rig internal id) | ⚠️ 此时 wire id 未知,用 internal id |
| `ToolCallDelta Delta` | `ToolCallInputDelta{delta}` | ✅ 1MB 上限;名称未到时缓冲 |
| `ToolCall`(完整) | 状态更新 + 补发 Added | ✅;`signature`/`additional_params` ❌ 丢弃 |
| `Reasoning`(完整块) | 替换 delta 累积(零事件) | ⚠️ 只取 `Text` 变体;`Encrypted`/`Redacted`/`Summary` ❌ 丢弃 |
| `Final(StreamFinal)` | `OutputItemDone(Reasoning→Message→FunctionCall/CustomToolCall)→Completed` | 见下表 |
| `Unknown` | 零事件+debug | ❌ |

### 2.2 rig StreamFinal 字段

| StreamFinal 字段 | 去向 |
|---|---|
| `usage.input_tokens` | ✅ → `TokenUsage.input_tokens`;Anthropic 时加 cached+cache_write(map_usage:402-409) |
| `usage.cached_input_tokens` | ✅ → `cached_input_tokens` |
| `usage.cache_creation_input_tokens` | ✅ → `cache_write_input_tokens` |
| `usage.output_tokens` | ✅ → `output_tokens` |
| `usage.reasoning_tokens` | ✅ → `reasoning_output_tokens` |
| `usage.total_tokens` | ✅ → `total_tokens` |
| `usage.tool_use_prompt_tokens` | ❌ codex 无对应 |
| `finish_reason` | ✅ → `end_turn = Some(!ToolCalls)` |
| `response_id` | ✅ → `Completed.response_id`(空串兜底) |
| `message_id` | ❌ 丢弃 |
| `provider_request_id` | ❌ 丢弃(`upstream_request_id` 恒 None) |
| `model` | ❌ 丢弃(不发 `ServerModel` 事件) |
| `raw` | ❌ 丢弃 |

### 2.3 genai 桥(ChatStreamEvent → ResponseEvent)

| genai 事件 | codex 事件 | 差异点(vs rig) |
|---|---|---|
| `Start` | `Created{response_id: None}` | ✅ 相同 |
| `Chunk{text}` | `OutputTextDelta` + `OutputItemAdded(Message)` | ✅ 相同 |
| `ReasoningChunk{content}` | `ReasoningContentDelta` + `OutputItemAdded(Reasoning)` | ✅;content_index 也是增量计数 |
| `ThoughtSignatureChunk` | 同 ReasoningChunk + 收集 thought_signatures | rig 无此事件 |
| `ToolCallChunk{tool_call}` | `OutputItemAdded(FunctionCall)` + `ToolCallInputDelta` | ⚠️ 增量靠**前缀 diff**(genai 给的是累积值) |
| `End(StreamEnd)` | Done(Reasoning→Message→FunctionCall)→ Completed | 见 usage 映射 |

### 2.4 桥的合成 vs 接收

| 合成项 | 说明 |
|---|---|
| `Created{response_id: None}` | 两桥均合成(rig 无 start 事件;genai 的 Start 无 id) |
| `txt_<uniq>` / `rsn_<uniq>` ID | 纳秒+计数器,防 retained history 按 ID 覆盖 |
| `reasoning.encrypted_content` | 存的是**明文**推理(非真加密)——供下一轮回放 |
| `usage_metadata: None` | 恒 None(OpenAI 专属) |
| `codex_rollout_budget_units: None` | 恒 None(fork 内部) |
| `Message.phase: None` | 恒 None |
| `FunctionCall.namespace: None` | 恒 None(平铺名) |
| `ServerModel`/`RateLimits`/`ModelsEtag` 事件 | **两桥均不产出**(Chat 无对应;native 路径有) |

---

## 三、已修复(曾是 bug,现为 ✅)

| 项 | 修复 |
|---|---|
| 工具输出图片 base64 文本化 | rig:逐项转 Base64 image block;genai:占位符+200KB 上限 |
| reasoning 回放顺序 | 两桥:连续 assistant 项合并为一条消息 |
| AgentMessage 丢弃 | 两桥:明文转发为 assistant text |
| data:URL 当远程链接 | rig:解码为 Base64 source |
| custom 工具往返 | rig:包装 schema + CustomToolCall 还原;genai:仍与 function 同路径 |
| response_format 带工具丢失 | rig:gated 时注入 additional_params |
| 401 延迟到流内 | rig:急切首事件;genai:错误链提取 status |
| 固定 ID 覆盖历史 | 两桥:唯一后缀 |
| 不完整工具调用复活 | rig:confirmed 追踪 |
| anthropic usage 语义 | rig:cache read/write 并入 input_tokens |

---

## 四、剩余丢失分类

### ❌ inherent(协议无对应,不可修)

`reasoning.summary`、`reasoning.context`、`store`、`include`、`client_metadata`、
`stream_options`、`Message.phase`、`FunctionCall.namespace`、`encrypted_function_args`、
`usage_metadata`、`ServerModel`/`RateLimits`/`ModelsEtag` 事件、`message_id`、
`provider_request_id`、`tool_use_prompt_tokens`、`codex_rollout_budget_units`

### 🔴 可修但未修(有方案,按需实施)

| 项 | 方案 | 影响 |
|---|---|---|
| `InputImage.detail` 丢失 | rig 有 `detail` 字段但 codex `ImageDetail` 枚举不匹配;可做映射 | 低(大多数 provider 忽略) |
| genai `data:URL` 图片丢失 | genai Anthropic 分支丢弃 data:URL;可预解码为 base64 | 中(anthropic 线上本地图片) |
| genai custom 工具无包装 | 同 rig 桥的 `{"input":string}` schema 方案 | 中(apply_patch 等) |
| 工具 `strict` 丢失(rig) | rig `ToolDefinition` 无 strict 字段 | 低 |
| `text.format.strict` 丢失 | rig 恒 `true`;codex `strict=false` 不生效 | 低 |
| genai `ReasoningSummaryDelta` | rig 不产出此事件系列 | 低 |
| Anthropic wire 全清 `additional_params` | `reasoning_effort` 等对 Anthropic 有真实意义(映射 thinking budget);可选择性保留 | 中(GLM 实测约束已缓解) |
| `provider_request_id` 不回填 | 可填入 `ResponseStream.upstream_request_id` | 低(排障价值) |
