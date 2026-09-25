# Rig 请求与响应字段审计

更新：2026-09-25。修复基于 `17e336236`，SDK 固定为 `rig-core = 0.42.0`。
本表描述当前 Rig 路径；GenAI 暂停扩展，不能用本表证明其字段兼容性。

## 1. 实际路由

| 配置 / 入口 | 当前行为 |
|---|---|
| Chat、Anthropic，bridge 未设置 | Rig；Anthropic 显式配置优先，兼容旧 `/anthropic` URL 判定 |
| 第三方 Responses，bridge 未设置 | Rig **Chat Completions**；此策略不代表请求仍发往 `/responses` |
| 内置 OpenAI、Bedrock、Bedrock Runtime Responses | 原生 Responses，保留 WebSocket、ChatGPT 认证、SigV4 等能力 |
| Responses + `experimental_bridge = "native"` | 显式保留原生 Responses |
| `experimental_bridge = "genai"` | 仍可显式使用；没有从 Rig 到 GenAI 的自动回退 |
| remote compaction | 与实际传输使用同一 `uses_chat_bridge()` 判定；桥接时采用本地压缩 |
| Guardian V2 固定 Luna 后台评分 | 第三方桥接时不启动原生 scorer/prewarm；保留普通审批回退，评分启用时提示 |
| LMStudio 模型预加载 | 上游仍有独立原生 `/responses` 预加载；不属于本次 Rig 主推理适配 |

所以不能笼统称“所有模型 HTTP 请求均使用 Rig”。用户主回合与辅助请求应分别审计。

## 2. 请求字段

| 字段 | Rig → Chat | Rig → Anthropic |
|---|---|---|
| model | 保留 | 保留 |
| instructions | System 消息 | SDK 提取至 system |
| developer / system 消息 | 保留 System 权限层级 | SDK 提取至 system；Messages 无独立 developer 角色 |
| assistant 文本 / reasoning / tool calls | 相邻同轮 assistant 内容合并，保留工具关联 | 同左 |
| function 与 custom 输出 | 共用文本和图片转换，custom 不再 JSON 文本化图片 | 同左 |
| tools / AdditionalTools / namespace | 统一展平；声明与历史使用同一名称函数 | 同左 |
| custom 工具 | 声明 `{input: string}`，响应恢复 CustomToolCall | 同左 |
| function strict | 在实际 HTTP function 定义中保留显式值 | 当前通用 Rig ToolDefinition 未承载此值；不能称为协议本身不支持 |
| tool_choice | SDK 映射 auto / none / required / 指定名称 | SDK 对应映射 |
| parallel_tool_calls | 显式布尔值 | false 映射 `tool_choice.disable_parallel_tool_use`，不发送 OpenAI 字段 |
| reasoning.effort | 包括 medium 在内，原值传递；厂商是否接受需真实验收 | 不盲目复制 OpenAI 枚举，当前未实现模型专属 thinking budget/effort 策略 |
| text.format | 原始 name / strict / schema，经 response_format 发送，首轮带工具也保留 | typed output_schema → output_config.format；不受 Chat 的首轮 gate 影响 |
| text.verbosity / prompt_cache_key / service_tier | 保留原值 | 未实现语义映射；不能混入 Messages 请求 |
| store | Chat 请求显式保留 | 未映射 |
| stream / stream_options | 桥始终流式；SDK 收集 usage；并发 reasoning summary 控制未映射 | 同左 |
| include / reasoning.context / client_metadata / access_programs | 未映射 Responses 专属控制或内部字段 | 同左 |
| malformed function arguments / 非法消息角色 | 请求启动前报错，不替换为 `{}` 或悄悄降为 user | 同左 |

Chat 的 schema `strict` 和 function `strict` 是不同字段，分别保留。
Anthropic 输出 schema 没有直接照搬 Chat 的 name/strict 控制。

## 3. 图片与工具结果

- 用户 data URL 转为 Rig Base64 + MIME；普通 URL 保留。
- Chat 的 tool 消息只能放文本。工具返回的图片移至紧随工具结果的 user 消息；并行调用的所有相邻 tool results 排在这些图片之前。
- Anthropic Base64 图片保留在 tool_result 内；SDK 不能表示 tool_result URL 图片时，图片作为同轮 user 图片发送。
- FunctionCallOutput 与 CustomToolCallOutput 使用同一路径；工具名称同时索引 function/custom，避免 custom 输出变为 unknown_tool。
- `detail=auto/low/high` 映射至 Rig；`original` 当前降为 high 并告警。Anthropic wire 无 Chat 的 detail 字段。
- 文本使用公共 UTF-8 安全截断，单段预算为约 8,000 token（仓库的字节估计）。这不是精确 tokenizer 计数。
- 文件 ID 图片、音频、provider 加密工具输出尚未实现完整适配；应与支持的 inline 图片区分，不能宣称无字段损失。

## 4. 推理历史

新历史使用带版本的 `codex-rig-reasoning-v1:` envelope 保存完整 Rig Reasoning：文本、签名、redacted、encrypted、summary 和 reasoning ID。
`encrypted_content` 在这里是历史持久化载体，**envelope 本身不加密**；不会把该 JSON 当作模型明文发送。

来源键包含协议、模型、有效 endpoint 与 query 路由的 SHA-256 摘要。query 中的凭据不明文写入 envelope；重复 query key 的顺序保留。换模型或来源后不重放 provider 专属块。

- 回放恢复完整思考块及签名，不再把最后一个 complete block 覆盖整轮内容。
- content_index 按可见文本块计算，不按网络分片递增；redacted 块不占可见文本索引。
- 原生 Responses 密文不会被解释为明文 reasoning。
- 旧版“content 与 encrypted_content 相同”的无签名历史仅在 Chat 下兼容；不能回放为 Anthropic signed thinking。
- 原生请求过滤新 envelope 和严格识别的旧桥明文；GenAI 请求过滤新 envelope。
- 请求适配只操作请求副本，不修改保存的会话历史。

## 5. 工具流与完成事件

工具参数在 Rig 确认 ToolCall 前暂存：Rig 可能修复 null 前缀、参数碎片，且到此时才能得到稳定的 provider call ID。
确认后发送 Added 和参数增量；Final 时按调用顺序发送 Done。

这个选择会延后工具参数预览，但保证：

1. Added 先于 delta；Added/Done 使用相同 item 类型、item ID、call ID。
2. function 参数保留合法 raw JSON 的原始空白；修复过的 JSON 只发送最终一致的值。
3. custom 的 delta 和 Done 都是解包后的 input，不把 JSON wrapper 当 freeform 输入。
4. 增量累计与完整 ToolCall 都有 1 MiB 上限；超限立即以流错误结束，不截成可执行参数。
5. 未确认调用不产生 Done，非法 custom wrapper 不执行；重复完成调用报错。
6. Reasoning Done 先于 Message Done；恰好一个 Completed，终端后停止流泵。

Rig 0.42 的 Chat adapter 在 `[DONE]` 后仍等 HTTP EOF 才输出 Final。
传输层现在用 SSE parser 识别完整 `[DONE]` 帧，透传后结束输入流；不会在 `finish_reason` 提前截断并丢掉 usage 尾帧。
本地测试覆盖跨 HTTP chunk、CRLF、多行 data、UTF-8 和服务端保持连接的情况。

## 6. 响应与传输元数据

| 字段 / 行为 | 当前映射 |
|---|---|
| input/cache read/cache write/output/reasoning tokens | 保留；Anthropic cache read/write 计入 Codex 总 input |
| total_tokens | SDK 总数；缺省为 0 时由 input+output 兜底；整数转化饱和处理 |
| response_id / finish_reason | Completed.response_id / end_turn |
| model | ServerModel 事件；这是可表示字段，不属于“协议不可支持” |
| HTTP request-id / x-request-id | 首个事件返回前填入 ResponseStream.upstream_request_id |
| query_params | SDK 拼接 endpoint 后再编码到最终 URI；base URL 已有 query 也保留 |
| 认证刷新 | 每次请求只异步调用一次 resolve_auth_headers；刷新失败和网关冲突在发网前返回 |
| 次级认证头 | 按 header name 排除 SDK 重建的主认证；同值异名头及敏感标志保留；Basic/Token 网关 Authorization 原样传送，静态 x-api-key 可与网关认证共存 |
| 自定义 CA | 复用 Codex CA helper 的 rustls 配置；同时支持 CODEX_CA_CERTIFICATE / SSL_CERT_FILE 优先级 |
| 网络错误 | 移除 reqwest URL 后再进入 Rig 错误层与日志；包含 response body 流错误 |
| 401 / 429 / 5xx | 急切读取首事件，在启动阶段保留 HTTP status、原始 JSON body、headers 与 Retry-After，供重认证、错误解析和重试机制识别 |
| SDK 请求构造错误 | RequestError / UrlError 返回 InvalidRequest，不作为网络错误重试 |

仍需区分的边界：RateLimits/ModelsEtag 未映射；Text.additional_params 中引用信息、tool signature、message_id、usage_metadata 等尚未完整承载。部分受 Codex 类型限制，部分受 SDK 归一化限制，部分只是桥尚未实现；不能统一标成“协议 inherent”。

每轮仍新建 HTTP client；自定义 CA 复用不等于完整复用原生 HttpClientFactory 的动态代理策略。

## 7. 验证分层

- `codex-rust-rig-bridge/tests/wire.rs` 及 `tests/wire/`：本地 HTTP 接收器检查最终 URL/headers/body，并将 SSE 通过真实 Rig parser 和桥流泵回传；覆盖错误 HTTP 状态、认证刷新、首事件超时、截断与终端保持连接。
- `stream_contract_tests.rs`：工具参数修复/超限/custom 类型、签名/redaction/source、可见 reasoning 索引、终端契约。
- 配置/provider 测试：Bedrock override 二次验证、内置 bridge override、远端枚举严格解析、统一路由。
- `LIVE_CASSETTE=replay --test bridge_live`：24 份已有 **Rig 中间事件** fixture 必须进入当前转换。旧 fixture 缺 custom_tools 时按空列表兼容；缺文件/反序列化失败/截断直接失败。
- Rig-event cassette **不是 HTTP/SSE cassette**，不验证 SDK 序列化；不能代替 wire.rs。
- `exec_live` 不支持离线回放，入口直接拒绝 replay，避免误耗厂商配额。
- 在线 CI 在运行前检查声明厂商和必要配置；不再用 nextest list 冒充“实际执行”。

最终测试命令、数量和未验证边界以本次修复报告为准；离线绿灯不能代替 MiMo/GLM/Step 的真实协议验收。
