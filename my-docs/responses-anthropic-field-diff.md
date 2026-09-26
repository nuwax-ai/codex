# Responses → Anthropic Messages 字段差异逐项清单

日期:2026-09-26。本清单供人工/Codex 逐项复核。

**三方基准来源**:
1. codex 实际发出的字段全集:`codex-rs/codex-api/src/common.rs:278` 的 `ResponsesApiRequest`(桥的真实输入)
2. OpenAI Responses API 官方参数表(developers.openai.com/api/reference → responses/create,经搜索交叉验证;platform.openai.com 直接抓取被 403)
3. Anthropic Messages API 官方参数表(platform.claude.com/docs/en/api/messages,2026-09 抓取)

**重要前提**:我们的 Anthropic 线对接的是国产兼容网关(GLM/StepFun/MiMo),不是真 Anthropic。
协议层"有对应物"≠ 网关接受——GLM 实测对未知字段有静默降级副作用(禁 thinking)。
任何新映射都必须经过 live 矩阵实测,必要时做成可配置开关。

---

## A. 有等价物(已映射 ✅ 或协议支持但桥未映射 🔧)

| Responses 字段 | Anthropic 等价物 | 桥现状 |
|---|---|---|
| `model` | `model` | ✅ 直传 |
| `instructions` | `system`(顶层,无 system 角色) | ✅ SDK 提取 |
| `input`(消息项) | `messages`(user/assistant 交替) | ✅ 转换 + 相邻合并 |
| `tools`(function) | `tools`(name/description/input_schema) | ✅ 转换 |
| `tools`(custom freeform) | 无专属类型,包装为 input_schema `{"input": string}` | ✅ 桥方案 |
| `tool_choice: auto/none/required` | `tool_choice: auto/none/any` | ✅ SDK 映射 |
| `tool_choice: "工具名"` | `tool_choice: {type:"tool", name}` | ✅ SDK 映射 |
| `parallel_tool_calls: false` | `tool_choice.disable_parallel_tool_use: true` | ✅ 传输层注入(经 GLM 实测教训,不发 OpenAI 字段) |
| `text.format.schema` | `output_config.format: {type:"json_schema", schema}` | ✅ typed output_schema |
| `stream` | `stream` | ✅ 桥始终流式 |
| `stream_options.include_usage` | 无需请求:Anthropic SSE 的 `message_delta` 事件总带 usage | ✅ 内部消化 |
| `reasoning.effort` | `output_config.effort`(low/medium/high/xhigh/max) | ✅ 已映射(2026-09-26):minimal→low、low/medium/high/xhigh/max 直传、ultra→max 钳制告警;none/persistent/custom 不注入 |
| `service_tier: auto/standard` | `service_tier: auto/standard_only` | ✅ 已映射(2026-09-26);flex/priority 仍丢弃(B-7) |
| `reasoning.summary` | `thinking.display`(summarized/omitted) | 🔧 语义近似(见 B-6,低价值) |
| 工具 `strict` 标志 | tool 定义新增 `strict: boolean`(2026 文档确认) | ✅ 已映射(2026-09-26):Chat 注入 function.strict,Anthropic 注入 tool 顶层 strict |

## B. 无等价物——Responses/OpenAI 侧独有(核心清单,逐项说明)

### B-1. `store: boolean`
- **OpenAI 语义**:是否在 OpenAI 服务端存储响应,供 `previous_response_id` 链式引用和 dashboard 检索。
- **为什么对不上**:Anthropic Messages 是纯无状态 API,没有服务端会话存储概念;每次请求自带全量历史。
- **桥处理**:丢弃(正确)。
- **变通可能**:无意义。codex 自身就是无状态模式(store=false),原生路径也几乎不用。

### B-2. `include: string[]`(如 `"reasoning.encrypted_content"`)
- **OpenAI 语义**:控制响应中附带哪些额外内容(加密推理块、文件引用等)。
- **为什么对不上**:Anthropic 的 thinking 块默认返回(受 `thinking.display` 控制),没有按需"包含什么"的请求机制;加密形态不同(thinking 带 signature,redacted_thinking 为不透明块)。
- **桥处理**:丢弃。响应侧由桥自行把 thinking 块存入版本化 envelope(`codex-rig-reasoning-v1:`)实现跨轮回放,不依赖协议的 encrypted_content。
- **变通可能**:已用桥内 envelope 等效实现 codex 需要的部分。

### B-3. `prompt_cache_key: string`
- **OpenAI 语义**:提示词缓存路由提示——OpenAI 取前 ~256 token + key 哈希,尽力把同类请求路由到同一台缓存机。
- **为什么对不上**:两家的缓存机制**范式不同**。OpenAI 是隐式自动缓存 + key 路由;Anthropic 是**显式断点**——请求方在 system/tools/messages 上标 `cache_control: {type:"ephemeral", ttl:"5m"|"1h"}`,命中前缀才计费打折。没有用户可控的缓存 key。
- **桥处理**:丢弃(Anthropic 线不发任何缓存控制)。
- **变通可能(高价值改进)**:桥可以在稳定前缀(system 指令 + 工具定义)自动打 `cache_control` 断点,获得与 prompt_cache_key 同等的经济收益——codex 长会话每轮重发全量工具表,缓存收益显著。**但必须先实测国产网关是否实现 cache_control**(计费与兼容均未知)。见修复机会 #3。

### B-4. `text.verbosity`(low/medium/high)
- **OpenAI 语义**:控制输出详略程度(GPT-5 系)。
- **为什么对不上**:Anthropic 无输出详略参数。
- **桥处理**:Chat 线发送;Anthropic 线丢弃。
- **变通可能**:拼进 system 指令("回答保持简洁")——不推荐,污染指令且效果不稳定。

### B-5. `reasoning.context`(压缩思考上下文窗口)
- **OpenAI 语义**:GPT-5.x 控制推理时可引用的先前思考量(内部调优字段)。
- **为什么对不上**:Anthropic thinking 没有对应的调节维度(budget_tokens 只控总量)。
- **桥处理**:丢弃(正确;codex 也基本不设置)。

### B-6. `reasoning.summary: auto/concise/detailed`
- **OpenAI 语义**:请求返回推理摘要流(codex 用于展示思考过程)。
- **为什么算无等价**:Anthropic `thinking.display`(summarized/omitted)只控展示形态,不产生独立的"摘要流"事件;语义近似但不是同一机制。
- **桥处理**:丢弃。codex 的推理展示走 ReasoningContentDelta(真 thinking 增量),不依赖摘要流。
- **变通可能**:映射 display 字段价值很低,不建议。

### B-7. `service_tier: flex / priority` 档位
- **OpenAI 语义**:flex=更便宜更慢,priority=更快更贵。
- **为什么对不上**:Anthropic 只有 `auto/standard_only`(是否允许优先容量),没有价格档位概念。
- **桥处理**:Anthropic 线整个丢弃。
- **变通可能**:仅 auto→auto、standard→standard_only 可保守映射;flex/priority 只能丢。

### B-8. `text.format.name` / `text.format.strict`(元数据部分)
- **OpenAI 语义**:结构化输出的 schema 命名与严格模式开关。
- **为什么对不上**:Anthropic `output_config.format` 只接收 `{type:"json_schema", schema}`——schema 本体有对应,**name 和 strict 元数据没有落点**。
- **桥处理**:schema 走 output_config;name/strict 丢弃。
- **变通可能**:strict 可映射到 tool 的新 strict 字段(同机制),text.format.strict 或可用近似;待验证。

### B-9. `client_metadata` / `access_programs`
- **OpenAI 语义**:codex/OpenAI 内部遥测与访问控制字段。
- **为什么对不上**:属于 OpenAI 专属内部协议,发给第三方网关属于字段泄漏(GLM 教训的直接对象)。
- **桥处理**:丢弃(正确且必须)。

### B-10. Responses 输入项里的 OpenAI 专属变体
- `LocalShellCall`/`WebSearchCall`/`ImageGenerationCall`/`ToolSearchCall` 等宿主工具调用项:Anthropic 对应的是**服务端工具**(`web_search_20260318` 等),但那是 Anthropic 自家云的功能,国产网关无此实现,且语义不互通 → 丢弃(正确)。
- `Reasoning` 项的 OpenAI encrypted_content 密文:只对 OpenAI 后端有效 → 桥用自有 envelope 替代(来源哈希隔离,跨厂商不回放)。
- `FunctionCall.namespace`/`encrypted_function_args`:Chat/Anthropic 均无 → namespace 编码进平名,encrypted 丢弃。

## C. 反向差异——Anthropic 有、Responses 没有(桥目前未利用,列出供完整性)

| Anthropic 字段 | 语义 | 与本桥的关系 |
|---|---|---|
| `thinking.budget_tokens`(≥1024) | 预算式思考控制 | effort 可换算映射(需启发式,如 effort→预算比例) |
| `cache_control`(5m/1h 断点) | 显式提示词缓存 | 见 B-3,潜在收益最大的未利用能力 |
| `container` + skills | 有状态执行容器复用 | 服务端功能,网关无实现,不适用 |
| `diagnostics.previous_message_id` | 缓存未命中归因 | 排障价值,网关支持未知 |
| `inference_geo` | 推理地域 | 企业特性,不适用 |
| tool 级 `defer_loading`/`allowed_callers`/`eager_input_streaming`/`input_examples`/`strict` | 工具定义增强 | strict 见 A 表;其余 codex 无对应概念 |
| `metadata.user_id` | 滥用检测标识 | OpenAI `user` 字段对应,但 codex 不发送 |
| `top_k` | 已被新模型废弃 | 无关 |
| 服务端工具(bash/web_search/code_execution 等) | Anthropic 云专属 | 国产网关无,不适用 |

## D. OpenAI Responses 有、但 codex 根本不发送的字段(桥范围外)

`temperature`、`top_p`、`max_output_tokens`、`user`、`metadata`、`previous_response_id`、`truncation`、`background`、`prompt`、`max_tool_calls` —— `ResponsesApiRequest` 结构体中没有这些字段,桥永远不会遇到。列出仅为防"文档全量 vs 实际子集"混淆。

---

## 给 Codex 的复核与修复要点

按价值排序的候选修复(每项都必须:①确认协议字段 ②live 矩阵实测国产网关行为 ③不破坏 GLM thinking 修复):

1. ✅ **`reasoning.effort` → `output_config.effort`**(2026-09-26 完成,传输层注入,与已有 output_config.format 合并;无对应档位不注入以避免未知字段降级)
2. ✅ **工具 `strict` → Anthropic tool `strict`**(2026-09-26 完成,与 Chat 线共用 tool_strict 映射,传输层按协议选择注入位置)
3. ⏳ **自动 `cache_control` 断点**:system + tools 稳定前缀打断点,长会话成本收益大;**未做**——涉及网关计费行为验证,属于性能优化而非正确性修复,单独实施。
4. ✅ **`service_tier` 保守映射**(2026-09-26 完成:auto→auto、standard→standard_only;flex/priority 丢弃)
5. 复核本清单每一条"桥处理"描述与当前代码([convert_request.rs](../codex-rust-rig-bridge/src/convert_request.rs)、[transport.rs](../codex-rs/codex-rust-rig-bridge/src/transport.rs))是否一致——尤其 B 类各项是否仍为"丢弃",有无回归。

## 来源

- Anthropic Messages API(2026-09 抓取):https://platform.claude.com/docs/en/api/messages
- OpenAI Responses API 参考:https://developers.openai.com/api/reference/cli/resources/responses/methods/create
- OpenAI prompt_cache_key 行为:https://developers.openai.com/api/docs/guides/prompt-caching
- codex 实际字段集:codex-rs/codex-api/src/common.rs:278
