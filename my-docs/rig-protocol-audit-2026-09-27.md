# Rig 协议转换复审与修复（2026-09-27）

## 1. 范围与结论

审查基线：`5443a749535ff9104cc6d68f688ee9e6cd926778`，分支 `test`。依赖按本机锁定的 `rig-core 0.42.0` 源码核实；本轮没有升级依赖。本文记录此基线之上的工作树修复，替代旧审计文档中关于“当前状态”的描述。

**原方案有必须修复的问题，不能把已有 live 绿灯当作字段无损证明。** 本轮修复重点是 thinking 历史、schema、工具结果与生命周期、SSE 终止、错误脱敏和测试失真。最终验证记录见 §7；真实厂商验收仍独立于本地测试。

用户粘贴的 B0–B6、`promotion-full45.log`、`b6bae4ca` 和 `docs/design/claude-handoff-2026-09-26/` 不属于本 fork 的检查证据。本轮不对那份 promotion 声明作认可。

### 实际路由

- Codex 内部 `ResponsesApiRequest` 是桥输入，**不等于所有请求都会发送到 `/responses`**。
- 第三方默认桥为 Rig：实际使用 `/chat/completions` 或 `/messages`；显式 Anthropic 协议优先，旧配置仅根据 URL **path** 中的 `/anthropic` 识别。
- 内置 OpenAI、Bedrock 和 `experimental_bridge = "native"` 保留原生 Responses 路径。当前没有实现 Rig 的 Responses wire 适配。
- 第三方只有 Responses endpoint 时，应使用 native；默认强行转换为 Chat 不会让该 endpoint 自动兼容。GenAI 仍是显式选项，本轮未扩展它。
- 模型预加载等独立辅助路径不因此全部变为 Rig。不能用主推理桥的测试声称“所有模型 HTTP 请求都经过 Rig”。

## 2. 官方基准与分类

核实日期为 2026-09-27；网页随服务更新，字段存在不表示每个型号或国产兼容网关都支持。

- **A：文档/契约冲突**，例如删除必须原样回传的 thinking、修改 schema 含义。
- **B：合法协议字段，但型号/网关能力有条件**，例如 effort、strict、tier、thinking disabled。
- **C：无直接等价项，明确不映射**，例如 Responses include → Messages。
- **D：桥或 SDK 能力缺口**，不能写成“官方 API 不支持”，例如文件 ID、引用元数据、Rig Responses wire。

请求形状以 [OpenAI Responses Create](https://developers.openai.com/api/reference/cli/resources/responses/methods/create)、[OpenAI Chat Create](https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create) 和 [Anthropic Messages Create](https://platform.claude.com/docs/en/api/messages/create) 为公开协议基准；下面表格的“当前行为”来自本仓库及锁定 SDK，而不是把这三份 API 全集当作 Codex 已发送的字段。

## 3. 请求：16 个顶层字段逐项核对

代码入口：`codex-rs/codex-api/src/common.rs::ResponsesApiRequest`、桥的 `convert_request.rs`、`request_messages.rs`、`request_tools.rs` 和 `transport.rs`。

| Codex 输入 | Rig → Chat 当前行为 | Rig → Anthropic 当前行为 / 差异 |
|---|---|---|
| `model` | 原值传入 SDK | 同左；不负责校验网关型号 |
| `instructions` | 领先 system 消息 | SDK 提取为顶层 system 文本块 |
| `input` | 转为 messages；细项见下表 | user/assistant 内容块；SDK 合并/提取 system |
| `tools` | function 参数；custom 用函数包装；namespace 展平 | name/description/input_schema；custom 同样包装 |
| `tool_choice` | auto/none/required/单个函数名 | auto/none/any/tool；字符串名字只产生一个候选，满足 SDK 限制 |
| `parallel_tool_calls` | 传原布尔值 | false 注入 `disable_parallel_tool_use=true`；none 时不加该键；true 不限制 |
| `reasoning` | effort 原字符串；summary/context 不映射 | effort 映射、none 禁用 thinking；summary/context 不映射，详见 §3.1 |
| `store` | 显式保留 | 不映射；不能由此承诺服务端数据保留政策 |
| `stream` | 必须 true，false 在发网前报错 | 同左 |
| `stream_options` | 内部实际类型是 `reasoning_summary_delivery`，不是 include_usage；不转发，Rig 自行请求 usage | 不转发；SDK 读取 usage 事件 |
| `include` | 无对应映射 | 无对应映射；thinking envelope 是桥内部保存机制 |
| `service_tier` | 原值传入 | auto→auto，default/standard→standard_only；其他值告警后省略 |
| `prompt_cache_key` | 原值传入 | 无同义缓存键；未启用 cache_control |
| `text` | verbosity 与完整 response_format（name/strict/schema） | 仅 schema → output_config.format；name/strict/verbosity 无同义字段 |
| `client_metadata` | 不映射内部遥测 | 同左；不能直接充当 metadata.user_id |
| `access_programs` | 不映射内部控制 | 同左 |

本轮为已设置但不映射的控制增加 debug 字段名列表，不记录值。非等价 effort/tier 使用告警。`store=false` 没有注入 Messages，不能解释成服务商的 ZDR 承诺。

Chat 分支发送的 response_format、verbosity、reasoning_effort、service_tier、prompt_cache_key、parallel_tool_calls、store 均能在公开 Chat 参数表找到；**值的型号适用范围另算**。例如 ultra、persistent、自定义 effort 和 standard tier 是 fork/网关扩展，不能因为字段合法就声称这些值均被官方 OpenAI 接受。

### 3.1 Effort、thinking、tier、max_tokens

- Anthropic effort：minimal→low；low/medium/high/xhigh/max 保持；ultra→max 并告警；persistent/custom 不映射并告警。压缩档位是近似，不能称语义完全等价。
- `none` 现在发送 `thinking:{type:"disabled"}`；省略 effort 会采用服务端默认行为，原来的“none 直接省略”不能表达关闭意图。若型号不允许关闭，服务端应拒绝，不能静默换成 low。
- **effort 不等于 thinking 开关或硬 token 预算**；当前没有为各型号自动选择 adaptive/enabled/budget_tokens。仅观察到可见 reasoning，不能证明请求的 effort 强度生效。[Effort](https://platform.claude.com/docs/en/build-with-claude/effort)、[Thinking 配置](https://platform.claude.com/docs/en/build-with-claude/thinking)
- `service_tier_for_request` 会先过滤 Codex 的 default 哨兵及不支持的档位；因此常规 core 路径的 default 不会到桥。桥公开入口仍接受 default→standard_only；standard 是既有网关别名。auto 不是“保证优先容量”，flex/priority/fast 等不能照搬。
- Anthropic 必须给出 max_tokens，当前固定 16384；它包含 thinking 与可见输出，不是所有型号的合理最优值。Chat 没有传 max_tokens/max_completion_tokens，依赖服务端限制。Codex 此输入结构也没有 max_output_tokens。达到上限现在显式失败，已不再伪报成功。[Messages Create](https://platform.claude.com/docs/en/api/messages/create)

### 3.2 Schema 与工具 strict

- Chat 保留完整 schema/name/strict，schema strict 与 function strict 分开处理。
- Anthropic 原先走 Rig typed output_schema。源码中的 sanitizer 会把可选属性列进 required，并删除数值约束。本轮改为原始 schema → `additional_params.output_config.format`，typed output_schema 保持 None，最终传输层只合并 effort。
- **不做隐藏的约束放宽。** 对 Messages 不支持的 minimum/maxLength 等，保留后可能收到 400；这是明确的能力不兼容，优于成功返回但约束已被删除。若未来提供 schema 降级模式，必须同时保留原始约束并做结果校验。
- Anthropic 的 text format 不接收 OpenAI 的 name/strict；tool 顶层 strict 管的是工具参数，不能拿它替代 text.format.strict。两者可以共存。工具 strict 也依赖网关及 schema 子集支持。[Structured outputs](https://platform.claude.com/docs/en/build-with-claude/structured-outputs)

### 3.3 历史、工具、图片与多模态

| 输入内容 | 当前处理 | 损失 / 条件 |
|---|---|---|
| developer | Rig System；Messages 顶层 system | Chat 官方有独立 developer，当前 SDK 通用表示丢失该区别；Messages 中途 system 会被提到顶层（D/C） |
| 相邻 assistant text/reasoning/tool | 合并同轮，内容顺序保留 | 不再截断多 thinking 块 |
| thinking signature / redacted | 原块保存进 envelope，同来源原样回传 | provider/model 切换时不跨来源回放 |
| function arguments | 必须 JSON object；空字符串兼容为 `{}` | 损坏/数组/null 不再删除整个调用及结果，也不让 SDK 静默改为 `{}` |
| custom 工具 | `{input:string}` 函数包装；响应恢复 CustomToolCall | 当前桥策略；不能写成 Chat 官方根本没有 custom tools |
| namespace | 同一 flat_name 处理声明、历史和响应派发 | 不保留独立 namespace wire 字段 |
| 工具结果 success | Anthropic `is_error=!success`；None 保持省略 | Chat 无同义独立状态；旧 rollout 若未保存 success，桥无法推断 |
| 无 call_id 的外部结果 | 有来源名称的 user 内容 | 不再生成孤立 tool_result；不是工具调用错误 |
| 多段工具文本 | 整个结果共享约 9800 token 字节估算预算 | 留出标题/标记空间，低于约 10k；不是供应商 tokenizer 的精确值 |
| 用户 data URL | JPEG/PNG/GIF/WebP → Base64 + MIME | BMP/HEIC/HEIF/SVG 等发网前报错，不再静默删除 |
| HTTP(S) 图片 | URL image | 不校验远端图像实际格式/大小，由服务端检查 |
| 工具结果图片 | Chat 移到紧随结果的 user；Anthropic Base64 留在 tool_result | Anthropic URL 工具图因 SDK 表示限制移至 user，不是官方不支持 URL |
| image detail | auto/low/high；original→high 并告警 | Anthropic 无 Chat detail；original 降级属于 SDK 限制 |
| 文件 ID 图片、音频 | 当前发网前明确报不支持 | 官方部分 API 有这些能力；跨供应商 file ID 不能直接互换（D） |
| assistant 图片/音频 | 明确拒绝 | 避免落入 SDK 无法序列化的分支 |
| opaque OpenAI encrypted reasoning | 不当普通文本回传 | 只能由原生协议解释（C） |
| 加密工具/agent 内容、宿主工具项 | 占位或既有过滤，不能恢复原语义 | 仍有能力损失；不能宣称任意 native rollout 全保真 |

官方图像类型列表也有动画差异：OpenAI 接受非动画 GIF；Anthropic 动画只处理首帧。桥检查 MIME/URL 形状，不解码图片验证帧数、尺寸、像素或 Base64 正确性。[OpenAI Vision](https://developers.openai.com/api/docs/guides/images-vision)、[Anthropic Vision](https://platform.claude.com/docs/en/build-with-claude/vision)

thinking 原块必须完整保留，尤其工具续轮；没有“一个 assistant 只能有一个 thinking 块”的规则。本轮本地两轮 HTTP 测试包含两个签名块、redacted 块、工具调用与失败结果，直接断言第二次实际出站 JSON。[Thinking preservation](https://platform.claude.com/docs/en/build-with-claude/thinking#preserving-thinking-blocks)、[Tool error results](https://platform.claude.com/docs/en/agents-and-tools/tool-use/handle-tool-calls#handling-errors-with-is_error)

## 4. 响应和流契约

| 来源 | Codex 当前映射 | 边界 |
|---|---|---|
| 无 Rig 开始事件 | 合成 Created（live/replay 一致） | response_id 在最终记录才获得 |
| Text | Message Added → text delta → Done | 切换 reasoning/text 前先结束当前项；不重叠 core 的 active_item |
| ReasoningDelta / Reasoning | visible delta + 完整 envelope | 按 ID 合并；完成后迟到的修改报错，避免重复或重写 Done |
| ToolCallDelta / ToolCall | 先缓存；正常 Final 后逐调用连续 Added/Delta/Done | 一项完整结束后才开始下一项；之前没有可执行 tool Done |
| 多个交错工具 | 按第一次出现顺序输出，ID/参数一致 | 因 Codex 单活动项，不展示未确认参数的即时流；后续 text/reasoning 也保序延后 |
| 工具参数 | 原始分片等于最终 JSON 时保留字节，否则使用 Rig 确认/修复值 | 分片/最终值均有 1 MiB 上限；非 object、坏 custom 包装、未确认调用失败 |
| Final Stop / ToolCalls | Completed，end_turn true / false | 无原因时保留 None，不臆造原因 |
| Length / ContentFilter / 其他未知停止 | 显式不完整流错误 | 不发布缓冲的工具调用；pause_turn 不在当前桥实现自动续接 |
| model_context_window_exceeded | ContextWindowExceeded | 可让 core 使用既有上下文处理 |
| Chat [DONE] | 真实 SSE 帧终止 HTTP body | 保留前面的 usage 尾帧；EOF 缺 [DONE] 是错误 |
| Anthropic message_delta(stop_reason) | 暂存至 message_stop 才交给 Rig | 防止 SDK 提前 Final 掩盖截断或中间 error；无需等 HTTP 连接关闭 |
| HTTP 非 2xx | 首事件前失败，保留 status/header/Retry-After | 包含读取错误响应体失败的 URL 脱敏 |
| 最终 response_id / message_id / model | Completed 使用 response_id 优先、message_id 回退；model→ServerModel | 修复 Anthropic msg ID 遗失；不冒充初始 Created 已携带 id |
| usage | Anthropic 新输入+cache read+cache write→input；独立缓存计数保留 | total=0 时以 input+output 兜底；非零采用 SDK 值；转换为 i64 饱和 |
| 原始 usage_metadata、citations、Text.additional_params、provider stop_sequence | 尚无完整映射 | 当前为 D 类损失，不应写“所有字段无损” |
| ToolCall.signature/additional_params | 通用桥尚不持久化 | 本轮两类 function/tool_use 主链不依赖 Gemini 式签名；不承诺任意 SDK provider |

停止与分帧依据：[Anthropic Streaming](https://platform.claude.com/docs/en/build-with-claude/streaming)、[Stop reasons](https://platform.claude.com/docs/en/build-with-claude/handling-stop-reasons)。缓存计数依据：[Prompt caching](https://platform.claude.com/docs/en/build-with-claude/prompt-caching)。SDK 与 core 的实际消费契约一起决定上述实现。

### Envelope 信任与大小

`codex-rig-reasoning-v1:` 是 JSON **封装，不是加密或签名**。source 含 protocol 与 endpoint/query/model 的哈希；查询凭据不以原文持久化。它防止误投不同来源，不证明文件未被修改，不验证 tenant/key 变更，也不承诺同地址不同租户的可移植性。

能修改本地 rollout 的主体也能修改用户/助手消息；此机制不是会话文件的认证边界。原生 ciphertext 不进入兼容纯文本路径。签名 thinking 不能截断来满足上下文预算；已有 reasoning envelope、输出流总缓存、工具参数 1 MiB 仍需独立容量治理，不能把本轮工具文本约 10k 上限写成所有消息都已小于 10k。

## 5. 发现、复现与处理

定位以函数/测试名为稳定入口；精确源码行号见本文末尾“源码索引”。等级按修复前问题记录。

| ID / 等级 | 复现及影响 | 分类 / 本轮处理 | 定位入口 |
|---|---|---|---|
| R01 P1 | 两个 thinking + tool 续轮，只回传第一个，签名历史丢失 | A；移除截断，真实两轮 JSON 验证 | convert_response_items / history_tests |
| R02 P1 | 可选 schema 属性变必填、minimum 消失 | A/D；绕过 typed sanitizer 保留原 schema | responses_request_to_completion_request |
| R03 P1 | reasoning Added 未 Done 就发 text/tool Added，core 活动项错配 | 内部契约；先闭合、工具整项延后并保序 | rig_event_to_response_events |
| R04 P1 | max_tokens/过滤/context/pause 或未确认 tool 被当成功 | 内部契约；错误终止，不执行缓冲工具 | handle_stream_final / PendingTools::finish |
| R05 P1 | finish_reason 已到但终止帧丢失，仍 Completed | A/D；检查真实 SSE 终止；Anthropic 暂存 terminal delta | with_terminal_check |
| R06 P1 | 非 2xx 响应体中途断开，Rig 格式化 URL 后绕过 reqwest 脱敏 | 安全；保留状态，去掉读取错误中的 URL | sanitize_error / error_tests |
| R07 P1 | 坏历史参数删掉调用和输出，或非对象被 SDK 改 `{}` | 历史损失；现在明确报错 | convert_response_items |
| R08 P1 | 外部无 call_id 事件被编造为 tool_result | 内部契约；保留来源和内容为 user 输入 | append_external_output |
| R09 P2 | 执行失败的 output.success=false 丢失 | A/D；注入 is_error，正常/缺省也覆盖 | request_tools / transport |
| R10 P2 | effort none 被省略，服务端仍默认思考 | 语义差异；显式 disabled，型号限制仍由服务端拒绝 | stream / transport |
| R11 P2 | default 标准 tier 未映射；声称其他丢弃均留痕但无日志 | 覆盖/诊断；补映射和不含值的字段日志 | anthropic_service_tier / convert_request |
| R12 P2 | 不合法 MIME 被删除，file/audio 沉默丢内容 | A/D；支持范围外早报错，不声称实现这些能力 | request_content |
| R13 P2 | query/fragment/userinfo 含 /anthropic 导致走错协议 | 路由；解析真实 path；两个入口共用策略 | chat_wire_protocol |
| R14 P2 | 自定义 anthropic-version 被 SDK 默认覆盖 | 配置；传 builder，断言最终 HTTP header | build_anthropic_model |
| R15 P2 | 两种桥 feature 都没编译，第三方 Responses 静默 native | 配置/兼容；请求入口 fail-fast，无网络发送 | ModelClientSession::stream |
| R16 P0* | 工具文本从约 8k 提到 24k，且逐段 cap 可绕过总量 | review 阈值；改整结果共享预算，见下文 | convert_tool_output |
| R17 P2 | 单 thinking 测试把错误规则固定；live 工具续轮只留调用 | 测试；改完整历史和多签名 wire 断言 | convert_request_tests / bridge_live |
| R18 P2 | replay 无 Anthropic URL 静默跳过；env 文件顺序反转 | 测试；占位地址、.env.local 优先 | live-tests/config |
| R19 P2 | 录制 mkdir/serialize/write 失败被吞，可能继续读旧 fixture | 测试；返回 Result，调用方失败 | live-tests/cassette |
| R20 P2 | replay 缺 Created，且终止事件处理与 live 不同 | 测试；同样的开始/终止契约 | bridge cassette |
| R21 P2 | reasoning>0 当 effort 生效；任意 JSON 当 schema obeyed | 证据失真；更正诊断措辞，区分接受与约束执行 | bridge_live |
| R22 P2 | 缺并行工具/坏帧/终止后错误的确定性覆盖 | 测试；新增 lifecycle 与真实 HTTP 错误矩阵 | stream_lifecycle_tests / error_tests |
| R23 P2 | 语义改动与格式改动混在超大模块，难以检查 | 工程；抽取 request_content、SSE、live config/cassette，新增独立测试文件 | 新私有模块 |
| R24 P2 | 二次复核：首文本块恰好耗尽总预算，后续文本无提示消失 | 已修复；首个非空遗漏添加一次截断标记，新增恰好 39,200 字节的 wire 用例 | convert_tool_output / multipart_tool_text_has_one_aggregate_budget |
| R25 P2 | SDK 的 Anthropic 完成 ID 在 message_id，旧转换只取 response_id 得到空串 | 已修复；response_id 优先、message_id 回退，两协议实际 HTTP ID 断言 | handle_stream_final / provider_completion_ids_survive_sdk_normalization |
| R26 P2 | 再复核：Anthropic 缓存计数只在 message_start、中间 delta 出现时丢失，显式 input_tokens=0 被旧值覆盖 | 已修复；按 wire 累计 usage 补正 SDK Final，录制与回放保留补正值，4 个 HTTP 回归 | usage / wire/usage_tests |
| R27 P2 | 再复核：空 call_id 的历史调用与结果被 SDK 分别生成随机 ID，断开关联并改变重试上下文 | 已修复；Function/Custom 的调用及有 ID 结果统一 fail-fast；真正 None 外部事件保持原语义 | convert_response_items / empty_history_call_ids_fail_before_rig_can_mint_replacements |
| R28 P2 | 再复核：旧 HTTP 失败用例只返回文本，“无工具 Done”断言无法检验工具泄漏 | 已补覆盖；实际发送已确认 function/custom，失败矩阵及受通道控制的终止前后生命周期断言 | wire/terminal_tests |
| R29 P2 | 再复核：原四组交付建议仍包含超过 800/500 行的复杂组，不能把全部变更按机械搬移豁免 | 已细化按 hunk 的顺序和依赖；本轮没有提交，实际拆分留给交付阶段 | 下述改动拆分建议 |

`P0*` 是仓库 review skill 对新增 >1k token 单项要求人工复核的分级，不表示已观察到线上安全事故。工具文本采用现有截断工具、共享总预算；正文来源于原工具结果，并不引入新的大提示词。review 还明确保留 §4 中其他大项容量限制，未给整个上下文作无界安全背书。

### 改动拆分建议

本轮未自动提交。再复核快照（最终格式化之前）为 40 文件、约 3,950 变更行；Rust/测试约 3,650 行。保守识别的机械搬移约 421 行，双向计入 diff 约 842 行；扣除后仍有约 2,800 行 Rust/测试需要审阅，并非全部语义行。原四组仍过大，应按相关 hunk 拆分，共享文件不能整文件混装：

1. **最小首组：core 无桥 feature fail-fast + 对应测试，共 75 行，可独立落地。** 只依赖已有 uses_chat_bridge。
2. URL path 判断、Anthropic 版本头分别落地，各自携带回归。
3. 先把 request_content 做纯机械抽取；再分开落地历史/结果身份、预算/多模态拒绝、schema/effort/tier及各自测试。当前新模块混有行为变化，不能整体标成搬移。
4. 先落地 SSE 终止校验；然后 reasoning/text 活动项切换；最后工具事件在成功 Final 后发布并携带 terminal_tests。关键依赖是工具发布改动不能早于终止校验，并非所有响应改动必须一次提交。
5. Anthropic usage 观察、Final 补正和 usage_tests 单独一组，依赖 SSE 基础。HTTP 错误脱敏单独一组。
6. 完成 ID fallback 与 history_tests 中对应的 ID 用例同组；测试文件名不决定归入请求组。
7. live 配置抽取/修复、fixture 写失败处理、桥 replay 契约分开，均不作为生产请求修复的前置依赖。文档随行为同步。

每个阶段交付时仍需核算实际 diff 和验证；这里列出拆分方案，不声称已经生成分组提交。

`live-tests/src/lib.rs` 抽取后仍超过 800 行；本次只把相关新增职责放到新模块，没有声称完成整个测试框架重构。upstream 触点限于公共协议判定和 core 分发保护，其余集中在 fork 桥/测试。

### 再复核：Anthropic usage 累计语义

官方 SDK 保留 `message_start` 中的计数；后续 `message_delta` 的非空计数覆盖旧值，缺省/null 保持原值，显式 0 也是有效覆盖。它们是累计值，不能逐事件相加。Rig 0.42 对缓存总量没有完整回退，还把终止 input_tokens=0 当作缺省，因此在 HTTP 层观察五个已映射计数，并在 Final 转换及录制之前补正；thinking 已包含在 output 内，不重复计入 total。[官方 usage 定义](https://github.com/anthropics/anthropic-sdk-python/blob/main/src/anthropic/types/message_delta_usage.py)、[官方累计实现](https://github.com/anthropics/anthropic-sdk-python/blob/main/src/anthropic/lib/streaming/_messages.py)

此补正使用每次请求独立、固定大小的状态，不重写历史 fixtures；`Final.raw` 仍保留 SDK 的原始结构，因此旧 cassette 中已经丢失的 wire 计数无法凭空恢复。新 wire 测试同时比较录制后的回放 usage。

## 6. 复审提示词 A1–C15 对应结论

| 检查项 | 结论 / 证据章节 |
|---|---|
| A1 Chat additional_params | 字段名正确；非官方枚举仍是网关扩展，§3 |
| A2 Anthropic 专属分支 | 不透传 OpenAI params；body 回归断言 schema/effort/strict 共存且无 OpenAI 键 |
| A3 effort/strict/tier | 位置核实；none/default 已修正，近似档位及型号条件明确，§3.1–3.2 |
| A4 tier 哨兵 | core 先过滤；公开 bridge 入口兼容 default，§3.1 |
| A5 tool_choice | 两线 SDK 映射正确；强制工具与 thinking 模式还受型号限制 |
| A6 max_tokens | 固定 16384 的局限已登记；上限终止错误已修复 |
| A7 图片 | 仅声明支持类型；unsupported MIME/内容早失败，§3.3 |
| A8 缺失控制 | 明确 debug 记录字段名；不能宣称全部已映射，§3 |
| B9 响应字段 | 核心文本/工具/签名/计数保留，引用/原始元数据仍缺，§4 |
| B10 envelope | 来源防误投，不是加密认证；跨源不回放，§4 |
| B11 usage | 缓存并入输入、保留独立计数、total 兜底，§4 |
| B12 工具生命周期 | wire + lifecycle 覆盖；未确认或异常终止不执行，§4/7 |
| C13 Rust/凭据 | 无新增生产 unwrap/expect/unsafe；坏错误体脱敏有真实 HTTP 回归 |
| C14 最小侵入 | 有限 upstream 触点；大改动建议分阶段，§5 |
| C15 测试有效性 | wire 检查最终 SDK HTTP；replay 是响应边界，live 未重跑，§7 |

## 7. 验证记录

### 7.1 上一轮修复验证

下列为上一轮修复阶段的执行记录，不能充当再复核后的新证据；最新结果见 §7.2。该轮列出的最终命令退出码为 0，没有使用历史 551 项或用户粘贴的 113 组数字。

| 命令（工作目录 codex-rs） | 实际结果 | 范围 |
|---|---|---|
| `just test -p codex-rust-rig-bridge --offline` | 73 passed / 2 binaries / 0 skipped | 最新修复含完成 ID、精确预算边界、真实 SDK HTTP、流生命周期、cassette |
| `just test -p codex-api --offline` | 200 passed / 6 binaries / 0 skipped | 公共 API crate，包含 URL 协议判定 |
| `just test -p codex-core --lib --features rust-rig --offline -E 'test(client::tests::)'` | 32 passed / 1 binary / 2593 filtered | Rig feature 打开时的 client、metadata、history filter、native HTTP/WebSocket 契约 |
| `just test -p codex-core --lib --no-default-features -E 'test(missing_bridge_features_reject_before_native_responses_dispatch)'` | 1 passed / 1 binary / 2624 filtered | 无桥 feature；覆盖 5 类配置、未发网络请求 |
| `LIVE_CASSETTE=replay LIVE_VENDORS=mimo,glm,step LIVE_INCLUDE_GENAI=0 just test -p codex-live-tests --offline --lib --test bridge_live` | runner 62 passed / 2 binaries | **实质 24 个 Rig 回放场景 + 7 个基础设施回归；另 31 个 GenAI/auth/A-B case 提前返回，不是协议验证** |

73 项 Rig 测试已包含 3 个新增 cassette 回归，不再把它们另计一次。24 个 Rig 回放场景读取 30 个 turn fixtures；它们验证当前响应转换，不重新验证 HTTP 序列化，也没有重新取得厂商返回。真实出站请求的证明来自本地 wire 测试。

最终 Nextest run IDs：Rig `1193db2e-cb73-4242-8059-19440b2c7559`；API `dd4f54a8-9789-4248-b414-5141fb09b7ca`；core/Rig `aae2765a-be9b-4959-82e7-f39b12931241`；replay `09c1306f-fc4c-4648-bfbc-2706df0e8994`。

过程中的失败也保留口径：第一次 Rig 测试 48/52，4 项旧测试依赖已修正的错误行为，随后改为正确契约；并行开发阶段 cassette 测试文件一度放错路径，另一次 core 测试错误匹配 Fatal 构造函数，均已修正并通过最终验证。

仅运行项目/路径范围内测试。**未运行**完整 workspace、完整 core suite、exec_live、真实供应商请求、Linux/Windows/Bun/Deno 或发布流程。fixture 历史请求未被重写成“新实测”。

首次 lint 阶段的源码复核发现 Anthropic message_id 遗失，因此返回逻辑修复阶段，新增实际 HTTP ID 断言并补跑 Rig/离线回放。最终收尾 scoped fix 和 fmt 后不再运行测试。最终执行 `just fix -p codex-rust-rig-bridge -p codex-api -p codex-core -p codex-live-tests --features codex-core/rust-rig --offline`：退出 0，无 warning/error；`just fmt`：退出 0。格式化产生的 6 个无关 GenAI/config 文件格式变化已剔除；保留本轮文件格式，不声称整个基线通过 fmt-check。随后 `git diff --check` 通过，未在最终 fix/fmt 后重跑测试。

该轮已修复当时确认的问题；再复核新增 R26–R29，见 §5。“三协议全字段无损兼容”的发布标准仍不通过，缺失能力见下一节。未 commit/push。

### 7.2 用户要求再复核后的本地验证（2026-09-27）

再次独立检查当前工作树，修复 R26/R27 并补齐 R28 的 HTTP 证据，细化 R29 的交付分组。所有测试使用 `just test`；模型 HTTP 只访问 loopback，live-tests 显式设置 replay。

本轮失败记录：usage 的 4 个定向 HTTP 用例在修复前 1 passed / 3 failed（退出 100），修复后 4 passed。第一次完整 Rig 为 79/81，新增 terminal 用例的 3 秒客户端初始化期限不足；将测试 setup 期限改为 30 秒、服务端总期限 45 秒，流 idle/后续读取仍为 3 秒、终止前门控检查仍为 100ms，随后 81/81 通过。分支独立的空调用/结果 ID 检查补强后，默认并发重跑虽为 81 passed，但有 2 flaky（初始化超时后重试通过）；没有把该结果当作无重试绿灯。

为降低本机 HTTP 客户端并发初始化的资源争用，在 nextest **local** profile 给 Rig wire binary 增加最多 4 个并发测试的组；CI 默认 profile 与生产超时均未更改。最后用 `--retries 0` 重新执行整个 Rig crate，保证不依赖自动重试。新 usage 的录制回放等价断言也在此轮执行。

最终测试命令全部退出 0：

| 命令（工作目录 codex-rs） | 本轮结果 | 日志 |
|---|---|---|
| `just test -p codex-rust-rig-bridge --offline --retries 0` | **81 passed / 2 binaries / 0 skipped / 0 retries** | `/tmp/rig-recheck-bridge-no-retry.log` |
| `just test -p codex-api --offline` | **200 passed / 6 binaries / 0 skipped** | `/tmp/rig-recheck-api.log` |
| `just test -p codex-core --lib --features rust-rig --offline -E 'test(client::tests::)'` | **32 passed / 1 binary / 2593 filtered** | `/tmp/rig-recheck-core-rig.log` |
| `just test -p codex-core --lib --no-default-features --offline -E 'test(missing_bridge_features_reject_before_native_responses_dispatch)'` | **1 passed / 1 binary / 2624 filtered**；5 类路由配置，均在发网前拒绝 | `/tmp/rig-recheck-core-no-bridge.log` |
| `LIVE_CASSETTE=replay LIVE_VENDORS=mimo,glm,step LIVE_INCLUDE_GENAI=0 just test -p codex-live-tests --offline --lib --test bridge_live` | runner **62 passed / 2 binaries**；实质 **24 Rig 回放场景 + 7 基础设施回归**，31 个禁用路径提前返回 | `/tmp/rig-recheck-replay.log` |

Nextest run IDs：Rig `84b52772-df43-4711-b634-feff14aee385`；API `81ae3a33-aa22-4206-9e54-bc11eaf7743d`；core/Rig `43eea911-37d0-4b42-95d3-c0ddabed42d0`；core/无桥 `e2e4e9e5-9bf3-445f-8a09-68fa0c1caf4b`；replay `6326dd93-8f6c-4da0-bf3b-9393a428a46c`。

最终收尾 `just fix -p codex-rust-rig-bridge -p codex-api -p codex-core -p codex-live-tests --features codex-core/rust-rig --offline` 退出 0，无 warning/error、无自动代码修正（`/tmp/rig-recheck-fix.log`）；`just fmt` 退出 0（`/tmp/rig-recheck-fmt.log`）。确认格式化前干净的 6 个无关 GenAI/config 文件只发生格式变化后，剔除了这部分变化。最终 `git diff --check` 通过。按仓库收尾规则，最终 fix/fmt 后没有再运行测试；未将整个历史基线宣称为 fmt-check 通过。

相对上一轮增加 8 个 Rig 测试：2 个历史 ID、4 个 usage、2 个 HTTP 工具生命周期（包含 8 组协议/终止场景）。不把参数化循环次数重复算成测试函数数。30 个历史 turn fixtures 未改写；parallel-tools 回放只要求至少一次调用，真正多工具顺序和执行前终止门禁来自本地 HTTP/生命周期测试。

范围仍限定 macOS 本地测试与历史回放；未运行完整 workspace、完整 core suite、真实供应商、真实工具进程端到端、Linux/Windows 或发布流程。本轮未 commit/push。

### 7.3 提交前独立复核与后续完整验证

用户随后要求再次审查并本地提交。本轮复核请求/历史、流/usage、外部兼容性和改动规模，核对 §7.2 的五份测试日志与 run ID，未发现新增确定性缺陷。未修改已验证的 Rust 内容，也未重跑或扩大测试范围；§7.2 仍是最近一次执行证据。

改动按依赖拆分为可审查的本地提交；需要一起落地的模块注册、调用方与测试随各自实现提交。完整审查范围始终是本文基线到本批最终提交，不能只看最后一个文档提交。后续交由 Claude 执行的独立复审及完整验证步骤见 [提示词](claude-rig-review-and-full-test-prompt.md)，其中明确分开 workspace、回放和真实厂商/exec 测试。

| 顺序 | 本地提交 | 范围 | 新增＋删除行数 |
|---|---|---|---:|
| 1 | `7ccba94c8` | 路由、版本头、缺失 feature 保护 | 180 |
| 2 | `47f16ea1d` | HTTP 测试辅助与 local 并发限制 | 87 |
| 3 | `0bbe0ec40` | SSE 终止门禁与累计 usage | 388 |
| 4 | `bf2af5e8e` | 响应生命周期与工具延后发布 | 527 |
| 5 | `48be47359` | HTTP 终止矩阵与错误读取脱敏 | 378 |
| 6 | `c17a8535a` | 历史转换、内容抽取与工具预算 | 795 |
| 7 | `da5953613` | schema/请求控制、工具失败及 HTTP 历史 | 466 |
| 8 | `195dcc13f` | live 配置抽取与环境优先级 | 416 |
| 9 | `8f9165c8c` | fixture 写失败与 replay 契约 | 450 |
| 10 | `72b8e4396` | live 工具续轮完整历史与证据措辞 | 87 |

第 4 组包含 340 行独立生命周期测试，第 6 组包含内容转换抽取；保留实现与对应测试为同一提交，其余代码组均少于 500 行。审计与提示词作为第 11 个文档提交。已用 SHA-256 核对这 10 个提交合成后的全部 37 个源码/配置文件，与提交前已审查且经 §7.2 验证的内容逐字节相同；没有逐个中间提交重新编译或运行测试。仅本地提交，未 push。

## 8. 仍缺失的能力 Top 3

1. **真正的 Rig Responses wire 和显式能力路由。** 目前 Responses 输入兼容层不等于 Responses 传输；只有 Responses endpoint 的提供方仍需 native。
2. **按型号的 thinking 模式、输出 cap、effort 能力约束。** hard cap 与 soft effort 分开配置，避免固定 16384 或 unsupported effort 在新型号上误导用户。属于功能设计，不宜用统一预算比例猜测。
3. **Anthropic prompt caching 策略。** 官方现有顶层自动 cache_control，也有块级断点；当前均未用。它不是 prompt_cache_key 的重命名，网关兼容性和成本收益需单独验收。[Prompt caching](https://platform.claude.com/docs/en/build-with-claude/prompt-caching)

其次是引用/原始 usage 元数据、独立 developer 角色、多模态/文件 ID 适配和输出 schema 本地校验。当前发布结论必须限定为修复后的 Chat/Messages 核心路径；“任意三协议全字段无损兼容”仍不成立。

## 9. 源码索引（最终格式化后的工作树）

| 发现 | 精确定位 |
|---|---|
| R01 | [codex-rs/codex-rust-rig-bridge/src/request_messages.rs:104](../codex-rs/codex-rust-rig-bridge/src/request_messages.rs#L104) |
| R02 | [codex-rs/codex-rust-rig-bridge/src/convert_request.rs:131](../codex-rs/codex-rust-rig-bridge/src/convert_request.rs#L131) |
| R03 | [codex-rs/codex-rust-rig-bridge/src/convert_response.rs:106](../codex-rs/codex-rust-rig-bridge/src/convert_response.rs#L106) |
| R04 | [codex-rs/codex-rust-rig-bridge/src/convert_response.rs:207](../codex-rs/codex-rust-rig-bridge/src/convert_response.rs#L207) |
| R05 | [codex-rs/codex-rust-rig-bridge/src/sse.rs:9](../codex-rs/codex-rust-rig-bridge/src/sse.rs#L9) |
| R06 | [codex-rs/codex-rust-rig-bridge/src/transport.rs:237](../codex-rs/codex-rust-rig-bridge/src/transport.rs#L237) |
| R07 | [codex-rs/codex-rust-rig-bridge/src/request_messages.rs:124](../codex-rs/codex-rust-rig-bridge/src/request_messages.rs#L124) |
| R08 | [codex-rs/codex-rust-rig-bridge/src/request_content.rs:81](../codex-rs/codex-rust-rig-bridge/src/request_content.rs#L81) |
| R09 | [codex-rs/codex-rust-rig-bridge/src/transport.rs:194](../codex-rs/codex-rust-rig-bridge/src/transport.rs#L194) |
| R10 | [codex-rs/codex-rust-rig-bridge/src/stream.rs:101](../codex-rs/codex-rust-rig-bridge/src/stream.rs#L101) |
| R11 | [codex-rs/codex-rust-rig-bridge/src/convert_request.rs:203](../codex-rs/codex-rust-rig-bridge/src/convert_request.rs#L203) |
| R12 | [codex-rs/codex-rust-rig-bridge/src/request_content.rs:185](../codex-rs/codex-rust-rig-bridge/src/request_content.rs#L185) |
| R13 | [codex-rs/codex-api/src/bridge.rs:58](../codex-rs/codex-api/src/bridge.rs#L58) |
| R14 | [codex-rs/codex-rust-rig-bridge/src/client.rs:178](../codex-rs/codex-rust-rig-bridge/src/client.rs#L178) |
| R15 | [codex-rs/core/src/client.rs:2393](../codex-rs/core/src/client.rs#L2393) |
| R16 | [codex-rs/codex-rust-rig-bridge/src/request_content.rs:109](../codex-rs/codex-rust-rig-bridge/src/request_content.rs#L109) |
| R17 | [codex-rs/live-tests/tests/bridge_live.rs:647](../codex-rs/live-tests/tests/bridge_live.rs#L647) |
| R18 | [codex-rs/live-tests/src/config.rs:69](../codex-rs/live-tests/src/config.rs#L69) |
| R19 | [codex-rs/live-tests/src/cassette.rs:120](../codex-rs/live-tests/src/cassette.rs#L120) |
| R20 | [codex-rs/codex-rust-rig-bridge/src/cassette.rs:44](../codex-rs/codex-rust-rig-bridge/src/cassette.rs#L44) |
| R21 | [codex-rs/live-tests/tests/bridge_live.rs:270](../codex-rs/live-tests/tests/bridge_live.rs#L270) |
| R22 | [codex-rs/codex-rust-rig-bridge/src/stream_lifecycle_tests.rs:237](../codex-rs/codex-rust-rig-bridge/src/stream_lifecycle_tests.rs#L237) |
| R23 | [codex-rs/codex-rust-rig-bridge/src/lib.rs:14](../codex-rs/codex-rust-rig-bridge/src/lib.rs#L14) |
| R24 | [codex-rs/codex-rust-rig-bridge/src/request_content.rs:116](../codex-rs/codex-rust-rig-bridge/src/request_content.rs#L116) |
| R25 | [codex-rs/codex-rust-rig-bridge/src/convert_response.rs:257](../codex-rs/codex-rust-rig-bridge/src/convert_response.rs#L257) |
| R26 | [codex-rs/codex-rust-rig-bridge/src/usage.rs:18](../codex-rs/codex-rust-rig-bridge/src/usage.rs#L18) |
| R27 | [codex-rs/codex-rust-rig-bridge/src/request_messages.rs:46](../codex-rs/codex-rust-rig-bridge/src/request_messages.rs#L46) |
| R28 | [codex-rs/codex-rust-rig-bridge/tests/wire/terminal_tests.rs:152](../codex-rs/codex-rust-rig-bridge/tests/wire/terminal_tests.rs#L152) |
| R29 | [my-docs/rig-protocol-audit-2026-09-27.md:165](../my-docs/rig-protocol-audit-2026-09-27.md#L165) |
