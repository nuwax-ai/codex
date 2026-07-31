# OpenAI Responses ↔ Chat Completions 协议转换参考

> 面向 `codex-rust-genai-bridge`（把 codex 的 Responses-API 形态请求桥接到 OpenAI 兼容 Chat Completions）。
>
> 信息来源（2026-07 最新）：OpenAI 官方文档（developers.openai.com）+ 本仓库 bridge 代码审查 + 社区实践（LiteLLM / Vercel AI SDK / genai crate / vLLM / Ollama / DeepSeek）。
>
> 参考文档快照另见 `/Users/soddy/Documents/git-workspace/codex-convert-proxy/docs`（含 `responses_api.yml` / `chat_completions_api.yml` OpenAPI spec）。

---

## 1. 背景与范围

- codex 内部所有工具与对话都按 **Responses API** 形态建模（`ResponsesApiRequest` + `ResponseItem`）。
- 当 provider 配置 `wire_api = "chat"` 时，`codex-rust-genai-bridge` 把 Responses 请求转成 `genai::ChatRequest`（OpenAI 兼容 Chat Completions），并把 Chat Completions 流式响应转回 codex 的 `ResponseEvent`。
- 本文档回答两个问题：**(a)** 两个协议的字段/语义如何对应（官方最新）；**(b)** 当前 bridge 还有哪些信息丢失、该按什么优先级修。

---

## 2. 最新 API 变化（gpt-5.6，2026-07）

相对早期文档的关键更新（影响 bridge 设计）：

| 变化 | 说明 | 对 bridge 的影响 |
|---|---|---|
| **`reasoning.effort` 新增 `none`** | 完整枚举：`none \| minimal \| low \| medium \| high \| xhigh \| max`（7 个，模型支持子集） | bridge 当前对 `XHigh`/`Max` 降级到 `High`（见 §8 P0-3） |
| **`reasoning.mode`**（gpt-5.6） | `standard`（默认）/ `pro`（更高延迟更高智力） | Chat Completions 无对应，可走 `extra_body`（OpenRouter 支持） |
| **`reasoning.context`** | `auto` / `current_turn` / `all_turns`（gpt-5.6 默认 `all_turns`） | Chat 无对应；影响跨轮回放推理 |
| **`reasoning.summary`** | `concise` / `detailed` / `auto` | Chat 无对应 |
| **`phase`**（消息阶段） | `commentary` / `final_answer` | ⚠️ "缺失 phase 会导致 preamble 被当成 final answer"——bridge 目前恒置 `None` |
| **`encrypted_content` 默认内联** | store=false 时 reasoning item 默认带 `encrypted_content`，不再需要 `include:["reasoning.encrypted_content"]` | bridge 已处理 `encrypted_content` 回放 |
| **GPT-5.4 起，Chat Completions + `reasoning:none` 不支持工具调用** | 官方限制 | 罕见组合，但需注意 |
| **`tool_choice` 新增 `allowed_tools`** | `{type:"allowed_tools", mode:"auto", tools:[...]}` 限定子集 | bridge 当前完全不转发 `tool_choice`（见 §8 P0-1） |
| **custom tools（`type:"custom"`）** | 自由文本输入 + 上下文无关文法（lark/regex） | Chat 仅支持 `function`/`custom`，custom 可直传 |
| **namespace 工具 + `defer_loading`** | `{type:"namespace", name, tools:[...]}` 配合 tool search | ✅ bridge 已展平 namespace（本次修复） |

---

## 3. 字段映射总表（Responses ↔ Chat Completions）

### 3.1 请求字段

| Responses API | Chat Completions | 转换说明 |
|---|---|---|
| `POST /v1/responses` | `POST /v1/chat/completions` | endpoint |
| `instructions` | `messages[0]` (role: system) | 系统指令拆出 |
| `input` (string/array) | `messages[]` | 遍历 ResponseItem 转换 |
| `tools`（扁平 `{type,name,parameters}` 或 namespace） | `tools`（嵌套 `{type,function:{name,parameters}}`） | ✅ bridge `parse_tools` 已展平 namespace |
| `tool_choice` | `tool_choice` | ⚠️ 未转发（P0-1）；注意形状差异：Responses 扁平 `{type:"function",name}` vs Chat 嵌套 `{type:"function",function:{name}}` |
| `parallel_tool_calls` | `parallel_tool_calls` | ⚠️ 未转发（P0-2） |
| `reasoning.effort` | `reasoning_effort`（顶层字符串） | ⚠️ XHigh/Max 被降级（P0-3） |
| `reasoning.{summary,context,mode}` | ❌ 无对应 | 走 `extra_body`（部分 provider） |
| `text.format`（扁平 json_schema） | `response_format`（嵌套 `json_schema:{name,strict,schema}`） | ⚠️ `strict` 被丢弃（P0-4） |
| `stream` | `stream` | 直传 |
| `stream_options` | `stream_options:{include_usage:true}` | usage 在末 chunk |
| `store` | `store` | ⚠️ genai Chat 路径不序列化 store，`with_store(true)` 实为 no-op（P3-1） |
| `previous_response_id` | ❌ | Responses-only；codex 已通过重放完整 `input` 模拟无状态 |
| `include`（如 `reasoning.encrypted_content`） | ❌ | 大部分 Responses-only |
| `temperature` / `top_p` / `max_tokens` | 同名 | 直传 |
| `service_tier` | `service_tier` | ✅ 已转发 |
| `prompt_cache_key` | — | ✅ 已转发（genai 透传） |
| `metadata` / `client_metadata`（traceparent） | 自定义 header | ⚠️ 未注入（P3） |

### 3.2 响应字段

| Chat Completions | Responses API | 转换说明 |
|---|---|---|
| `choices[0].message` | `output[]` (type:message) | content 在 `output[].content[].text` |
| `choices[0].message.tool_calls` | `output[]` (type:function_call) | 拆成独立 item，`call_id` 关联 |
| `choices[0].finish_reason` | `output[].status` + `Completed.end_turn` | ⚠️ 粒度丢失（P1，见 §8） |
| `usage.prompt_tokens` | `usage.input_tokens` | 重命名 |
| `usage.completion_tokens` | `usage.output_tokens` | 重命名 |
| `usage.completion_tokens_details.reasoning_tokens` | `usage.output_tokens_details.reasoning_tokens` | ✅ 已映射 |
| `usage.prompt_tokens_details.cache_creation_tokens` | `usage.cache_write_input_tokens` | ⚠️ bridge 硬编码 0（P0-6） |
| `delta.reasoning_content`（DeepSeek/vLLM 扩展） | reasoning item | ✅ genai 已读（reasoning_content + reasoning 双字段） |
| `id` (chatcmpl-) | `id` (resp_) | 前缀替换 |

---

## 4. Reasoning 完整规格

### 4.1 effort 枚举（请求侧）

| 值 | 语义 |
|---|---|
| `none` | 不推理；延迟敏感（语音/分类/检索） |
| `minimal` | 极少推理 |
| `low` | 轻量推理，工具/规划/搜索/草稿/聊天 |
| `medium` | 默认（gpt-5.5）；agent 编码/研究/表格 |
| `high` | 困难推理/复杂调试/深度规划 |
| `xhigh` | 深度研究/异步/长任务 |
| `max` | 最复杂任务 |

> Chat Completions 对应顶层 `reasoning_effort` 字符串。**注意不是所有模型支持所有值**。

### 4.2 三种 reasoning 字段约定（响应侧，社区）

不同 provider 的推理内容字段名不同（这是桥接最大坑）：

| 约定 | 使用者 |
|---|---|
| `reasoning_content` | DeepSeek 原生（canonical）、LiteLLM 归一目标、vLLM 旧名（已废弃为入站别名） |
| `reasoning` | **vLLM 新名**、**Ollama**、OpenRouter（主）、Helicone |
| `thinking` + `thinking_blocks`（带 `signature`） | Anthropic 原生（多轮需 `signature`） |

- **genai v0.6.5 已读双字段**：先 `reasoning_content` 再 fallback `reasoning`（bridge 免费继承）。
- **genai 仅回写 `reasoning_content`**（请求侧）：对 DeepSeek/Kimi 正确，但 **Ollama（用 `reasoning`）无法往返**。

### 4.3 DeepSeek 多轮 footgun（关键正确性）

| 场景 | 历史中的 reasoning_content | 行为 |
|---|---|---|
| `deepseek-reasoner`（R1）**无工具** | **必须剥离** | 否则 HTTP 400（字段被忽略） |
| DeepSeek thinking 模式 **+ 工具** | **必须完整回传** | 否则 HTTP 400 |

> bridge 当前（`convert_request.rs:73-89`）**恒注入**历史 reasoning → 对 R1-无工具场景错误（P1-5）。

### 4.4 reasoning item 形态

```json
{
  "id": "rs_xxx",
  "type": "reasoning",
  "summary": [{"type":"summary_text","text":"..."}],
  "encrypted_content": "..."  // store=false 时默认内联
}
```

- `summary`：需显式 `reasoning.summary` 请求，否则不含。
- `encrypted_content`：store=false 或 ZDR 时默认内联，可用于跨轮延续推理。
- `phase`（在 message item）：`commentary`/`final_answer`，缺失会导致 preamble 被当成最终答案。

---

## 5. Function Calling / Tools 完整规格

### 5.1 工具定义形状

**Responses（扁平）**：
```json
{"type":"function","name":"get_weather","description":"...","parameters":{...},"strict":true}
```
**Chat Completions（嵌套）**：
```json
{"type":"function","function":{"name":"get_weather","description":"...","parameters":{...},"strict":true}}
```

### 5.2 工具类型对照

| Responses `type` | Chat Completions | 处理 |
|---|---|---|
| `function` | `function` | ✅ 直转 |
| `namespace`（含子 tools） | 多个扁平 `function` | ✅ bridge 已展平为 `mcp__<ns>__<tool>` |
| `custom`（自由文本 + 文法） | `custom` | 可直传（Chat 支持） |
| `web_search` / `file_search` / `computer_use` / `code_interpreter` / `image_generation` | ❌ | Responses-only（OpenAI hosted），LiteLLM 也是 drop+warn |

### 5.3 tool_choice

```
"auto" | "none" | "required" | {"type":"function","name":"X"}            // Responses（扁平）
"auto" | "none" | "required" | {"type":"function","function":{"name":"X"}} // Chat（嵌套）
{"type":"allowed_tools","mode":"auto","tools":[...]}                       // Responses 子集（新）
```

> 形状差异：指定函数时 Responses 扁平、Chat 嵌套 `function.name`。LiteLLM 桥接做的就是这个重命名。

### 5.4 parallel_tool_calls

- 设 `false` → 恰好 0 或 1 个工具调用。
- GPT-5 起支持与内置工具并存，但"内置工具不能进入并行 function-call 批次"。

### 5.5 工具结果回传

- **Responses**：`input` 数组追加 `{type:"function_call_output", call_id, output}`（output 通常是字符串）。
- **Chat**：`messages` 追加 `{role:"tool", tool_call_id, content}`。

### 5.6 custom_tool_call（响应侧）

非 JSON-schema 工具的响应 item 是 `{type:"custom_tool_call", call_id, name, input}`（`input` 是纯文本，非 `arguments`）。

---

## 6. Streaming 事件

### 6.1 Responses SSE 事件类型（2026-07 完整列表）

| 类别 | 事件 |
|---|---|
| 生命周期（各 1 次） | `response.created` / `response.in_progress` / `response.failed` / `response.completed` / `error` |
| output item | `response.output_item.added` / `response.output_item.done` |
| content part | `response.content_part.added` / `response.content_part.done` |
| 文本 | `response.output_text.delta` / `response.output_text.annotation.added` / `response.text.done` |
| refusal | `response.refusal.delta` / `response.refusal.done` |
| function call | `response.function_call_arguments.delta` / `response.function_call_arguments.done` |
| file search | `response.file_search_call.in_progress` / `.searching` / `.completed` |
| code interpreter | `response.code_interpreter_call.in_progress` / `.code.delta` / `.code.done` / `.interpreting` / `.completed` |

> ⚠️ **官方列表中没有显式 reasoning 流式事件**。reasoning 内容通过 reasoning item 的 summary/content 体现，第三方 provider 用 `delta.reasoning_content`。

### 6.2 Chat → Responses 事件映射（社区收敛，Vercel 参考）

| Chat delta | Responses 事件 |
|---|---|
| 首 chunk（`delta.role:"assistant"`） | `response.created` + `response.output_item.added`(message) |
| `delta.content` | `response.output_text.delta` |
| `delta.tool_calls[].function.arguments` | `response.function_call_arguments.delta` |
| `delta.reasoning_content`（第三方） | reasoning 增量（bridge 用 `ReasoningContentDelta`） |
| `finish_reason`（末 chunk） | `response.output_item.done` → `response.completed` |

### 6.3 usage 在流式中的交付

- Chat：需 `stream_options:{include_usage:true}`，**末 chunk** 带 usage（`choices:[]`）。
- Responses：含在 `response.completed` 的 response 对象里。
- genai：`capture_usage` 开启时自动处理 DeepSeek 的"null-then-real"模式。

---

## 7. Usage / 计费字段对照

| 概念 | Chat Completions | Responses API |
|---|---|---|
| 输入 token | `prompt_tokens` | `input_tokens` |
| 输出 token | `completion_tokens` | `output_tokens` |
| 输出明细对象 | `completion_tokens_details` | `output_tokens_details` |
| reasoning token | `completion_tokens_details.reasoning_tokens`（可选） | `output_tokens_details.reasoning_tokens`（必填） |
| 缓存读 token | `prompt_tokens_details.cached_tokens` | `input_tokens_details.cached_tokens` |
| 缓存写 token | `prompt_tokens_details.cache_creation_tokens` | `cache_write_input_tokens` |

> LiteLLM 流式 usage 用 **last-wins（末 chunk 累计值）而非求和**，并在 provider 未发 usage 时用本地 tokenizer 合成一个兜底。

---

## 8. Bridge 转换待办清单（三方对齐：代码现状 + 官方语义 + 社区做法）

> 已修复：✅ namespace 工具展平（`parse_tools`）、✅ Responses-Lite `AdditionalTools` 提取、✅ reasoning token 计数、✅ reasoning_content 双字段读取（genai 继承）。

### P0 — 确认 bug / 静默丢弃（优先修）

| # | 项 | 现状（file:line） | 建议 | Provider 注意 |
|---|---|---|---|---|
| P0-1 | `tool_choice` 未转发 | `stream.rs:146-225` 从不设 `options.tool_choice`；codex 默认 `"auto"` | 映射到 `ChatOptions::tool_choice`（genai 支持 `Auto/None/Required/Tool{name}`）；指定函数时序列化为嵌套 `{type:function,function:{name}}` | Ollama 静默忽略；GLM 不支持 |
| P0-2 | `parallel_tool_calls` 未转发 | 同上从不设置 | 走 `ChatOptions::extra_body`（genai v0.6.5 无该字段） | Ollama 静默忽略；DeepSeek 未文档化；vLLM 支持 |
| P0-3 | `reasoning.effort` 的 `XHigh`/`Max` 降级到 `High` | `stream.rs:168-179` 带 warn | genai v0.6.5 原生有 `XHigh`/`Max`，直传（仅当目标 provider 拒绝时才降级） | — |
| P0-4 | `TextFormat.strict` 被丢弃 + genai 强制 `strict:true` | `stream.rs:210-217` | Responses `strict:false` 时经 `extra_body`（`response_format.json_schema.strict`）覆盖 | — |
| P0-5 | 工具调用变体不一致：add 时 `CustomToolCall`、done 时 `FunctionCall` | `convert_response.rs:79` vs `:177` | 统一为 `FunctionCall`（codex dispatch 依赖 `ToolPayload::Function`） | 内部一致性 |
| P0-6 | `cache_write_input_tokens` 硬编码 0 | `convert_response.rs:126` | 从 `Usage.prompt_tokens_details.cache_creation_tokens` 取真实值 | — |

### P1 — Reasoning 正确性（DeepSeek footgun）

| # | 项 | 现状 | 建议 |
|---|---|---|---|
| P1-5 | reasoning_content 回传策略 | `convert_request.rs:73-89` 恒注入历史 reasoning | 按 model + 是否有工具 分策略：`deepseek-reasoner`(R1) 无工具时**剥离**；thinking+工具时**必须回传**。源：[DeepSeek reasoning_model](https://api-docs.deepseek.com/guides/reasoning_model) / [thinking_mode](https://api-docs.deepseek.com/guides/thinking_mode) |
| P1-6 | provider 感知的 reasoning 字段（请求侧） | genai 仅回写 `reasoning_content` | 若目标 Ollama（用 `reasoning`），需 adapter 感知回写 |
| P1-7 | 停止原因粒度丢失 | `convert_response.rs:192-201` 仅算 `end_turn:bool` | `MaxTokens`/`ContentFilter`/`StopSequence` 无法区分；codex `Completed` 暂无字段承接，至少日志化 |

### P2 — 功能缺口

| # | 项 | 现状 | 建议 |
|---|---|---|---|
| P2-8 | 音频输入跳过 | `convert_request.rs:232-235`（注释"国内 LLM 暂不支持"） | Chat Completions 支持 `input_audio:{data,format}`，可启用（genai 可序列化） |
| P2-9 | image `detail` 丢失 | `convert_request.rs:215-231` 仅用 `image_url` | 转发 `detail`（注意 Chat 无 `"original"`，映射到 `"high"`） |
| P2-10 | file/PDF 输入 | `convert_content_items` 只处理 text/image/audio | 加 `input_file`（Chat 支持 `{file:{file_id\|filename+file_data}}`）；PDF-via-URL 是 Responses-only，drop+warn |
| P2-11 | usage 兜底合成 | 无 | 若流结束无 usage chunk，合成兜底（LiteLLM 模式） |
| P2-12 | thought_signatures 未结构化回传 | `convert_response.rs:46` 收集但 `handle_stream_end` 不用 | Anthropic 多轮 thinking 需要 `signature`；用 genai `assistant_tool_calls_with_thoughts` |

### P3 — 加固 / 清晰度

| # | 项 | 建议 |
|---|---|---|
| P3-13 | `with_store(true)` 实为 no-op | 注释说明（genai Chat 路径不序列化 store） |
| P3-14 | 拒绝 Responses-only 模型 | `gpt-5-codex` / `*-pro` / `*-deep-research` / `computer-use-preview` 早 fail |
| P3-15 | `reasoning.summary/context/mode` 显式 drop + debug 日志 | 当前只是未用 |
| P3-16 | 纯 reasoning 时发空 assistant message | `convert_response.rs:139-153` 逻辑 bug，应仅 `text_buffer` 非空才发 message |

---

## 9. Provider 兼容性速查（OpenAI 兼容 Chat Completions）

| Provider | `tool_choice` | `parallel_tool_calls` | reasoning 字段 | 备注 |
|---|---|---|---|---|
| **DeepSeek** | 4 种形式 | 未文档化（勿转发） | `reasoning_content` | thinking 模式 + 工具须回传 reasoning；R1 无工具须剥离 |
| **vLLM** | 4 种形式 | 支持（默认 true） | `reasoning`（新）/ `reasoning_content`（旧别名） | 自身有 `/v1/responses`（无状态） |
| **Ollama** | ❌ 静默忽略 | ❌ 静默忽略 | `reasoning` | 自身有 `/v1/responses` |
| **OpenRouter** | 支持 | 支持 | `reasoning` + `reasoning_details[]` | 接受顶层 `reasoning:{...}`；`/v1/responses` 无状态 |
| **GLM** | ❌ 不支持 | ❌ | — | content 需扁平 string；developer→user |
| **Kimi/MiniMax** | 标准兼容 | 标准兼容 | `reasoning_content` | MiniMax: developer→user |

---

## 10. 协议固有不可磨平（INHERENT，无需修）

- `previous_response_id` / `conversation`（codex 已靠重放 `input` 模拟无状态）
- OpenAI hosted 工具：`web_search` / `file_search` / `computer_use` / `code_interpreter` / `image_generation`（Chat 不支持，LiteLLM 也是 drop+warn）
- Responses-only 模型（`gpt-5-codex`、`*-pro`、`*-deep-research`、`computer-use-preview`）
- 富 Responses 流式事件：`file_search_call.*`、`code_interpreter_call_code.delta`、`image_generation_call.partial_image`、`mcp_call_arguments.delta`、refusal deltas
- `reasoning.encrypted_content` 的 ZDR 语义（可携带不透明 blob，但 Chat 不定义）
- item ID（Chat 不分配 per-item ID，bridge 合成 `txt_<n>`/`rsn_<n>`）
- `n>1`（Responses 也不支持）

---

## 11. 参考来源

**OpenAI 官方（2026-07）**：
- 迁移指南：https://developers.openai.com/api/docs/guides/migrate-to-responses
- Reasoning：https://developers.openai.com/api/docs/guides/reasoning
- Function calling：https://developers.openai.com/api/docs/guides/function-calling
- Streaming：https://developers.openai.com/api/docs/guides/streaming-responses
- Chat Completions create：https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create/
- Responses streaming events：https://developers.openai.com/api/reference/resources/responses/streaming-events/
- OpenAPI spec：https://github.com/openai/openai-openapi/blob/main/openapi.yaml

**社区**：
- LiteLLM Responses 桥接：https://github.com/BerriAI/litellm/tree/main/litellm/responses/litellm_completion_transformation
- LiteLLM reasoning_content：https://docs.litellm.ai/docs/reasoning_content
- Vercel AI SDK：https://github.com/vercel/ai（`packages/openai-compatible`、`packages/openai/src/responses`）
- genai crate v0.6.5：https://github.com/jeremychone/rust-genai/tree/v0.6.5

**Provider**：
- DeepSeek thinking：https://api-docs.deepseek.com/guides/thinking_mode ｜ reasoning：https://api-docs.deepseek.com/guides/reasoning_model
- vLLM reasoning：https://docs.vllm.ai/en/latest/features/reasoning_outputs/
- Ollama OpenAI-compat：https://docs.ollama.com/api/openai-compatibility ｜ thinking：https://ollama.com/blog/thinking
- OpenRouter reasoning：https://openrouter.ai/docs/guides/best-practices/reasoning-tokens
