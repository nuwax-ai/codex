# Codex 复审提示词:rig 桥接大模型请求的字段差异与映射方案

> 使用方式:整段复制给 Codex(建议在仓库根目录运行,让它能读代码和文档)。

---

你是一位严格的协议与 API 集成审查专家。请对这个仓库(fork 自 openai/codex)的 **rig 桥接层**做独立复审,重点审查:使用 rig 请求大模型时的**字段差异**与**字段转换映射**,并对照**官方 API 文档**逐项核实。不要信任仓库内的自审文档结论,一切以代码和官方文档为准;文档只用于定位意图。

## 背景(必读)

- codex 内部以 **OpenAI Responses API 形状**构造请求(`codex-rs/codex-api/src/common.rs` 的 `ResponsesApiRequest`,16 个顶层字段)。
- fork 把第三方厂商请求路由到自建桥 `codex-rs/codex-rust-rig-bridge`(rig-core 0.42.0),转换为两种线上协议:**OpenAI Chat Completions** 与 **Anthropic Messages**。
- 国产厂商(MiMo/GLM/StepFun)通过 OpenAI 兼容或 Anthropic 兼容网关接入;**实测教训**:GLM 的 Anthropic 网关对未知字段会静默降级(禁用 thinking),因此"协议文档说可以"不等于"网关接受",任何映射结论都要区分协议层与网关层。

## 必读材料(按顺序)

1. `codex-rs/codex-rust-rig-bridge/src/convert_request.rs` — 请求顶层字段转换与协议分流
2. `codex-rs/codex-rust-rig-bridge/src/request_messages.rs` — 历史/消息/图片转换
3. `codex-rs/codex-rust-rig-bridge/src/request_tools.rs` — 工具声明展平、custom 包装、strict 映射
4. `codex-rs/codex-rust-rig-bridge/src/transport.rs` — 最终 HTTP body 注入(tool strict、output_config.effort、service_tier、disable_parallel_tool_use、query 重拼、SSE DONE 终止)
5. `codex-rs/codex-rust-rig-bridge/src/stream.rs` + `convert_response.rs` + `response_tools.rs` + `reasoning.rs` — 响应方向转换、reasoning envelope
6. `my-docs/responses-anthropic-field-diff.md` — Responses→Anthropic 逐字段对照清单(复审对象之一)
7. `my-docs/field-mapping-audit.md` — 现行字段审计(复审对象之一)
8. `my-docs/rig-review-fixes-2026-09-25.md` — 历史修复台账(第 52-58 条为 round 2)

## 官方 API 文档对照(逐字段核实,不要凭记忆)

- OpenAI Responses API:`https://developers.openai.com/api/reference/cli/resources/responses/methods/create`(注意 platform.openai.com 会拒绝非浏览器抓取,可用镜像或搜索交叉验证)
- OpenAI Chat Completions API:同站 api-reference/chat
- Anthropic Messages API:`https://platform.claude.com/docs/en/api/messages`(注意 2026 版新增:顶层 `service_tier`、`output_config.effort/format`、tool 级 `strict`、`thinking` 三变体、`cache_control`)

## 重点审查项(逐项给出结论)

### A. 请求方向映射正确性
1. Chat 线注入的每个 additional_params 字段(response_format/verbosity/reasoning_effort/service_tier/prompt_cache_key/parallel_tool_calls/store)是否为 Chat Completions 文档合法字段?有无 OpenAI 已废弃/改名而我们照发的?
2. Anthropic 线是否**结构性不可能**泄漏 OpenAI 专属字段?检查 convert_request.rs 的 params 构造分支与 transport 注入路径。
3. 新映射核实:`reasoning.effort → output_config.effort`(minimal→low、ultra→max、none/persistent/custom 不注入)、工具 `strict`(Chat 在 function 层、Anthropic 在 tool 顶层)、`service_tier`(auto→auto、standard→standard_only、flex/priority 丢弃)。对照文档核实字段名、嵌套位置、枚举值是否正确。
4. `service_tier` 值域:codex 侧 `"default"` 哨兵在 `codex-rs/protocol/src/openai_models.rs` 的 `service_tier_for_request` 被过滤,真正到达桥的值是什么?映射是否覆盖?
5. tool_choice 四变体(auto/none/required/specific)在两条线上的 rig 序列化是否与文档一致?rig 对 Anthropic specific 的单名限制处理是否安全?
6. max_tokens:Anthropic 必填默认 16384 是否合理?Chat 线不发 max_tokens 的后果?
7. 图片:data:URL 解码 MIME 白名单(jpeg/png/gif/webp/heic/heif/svg)是否与两家文档的支持列表一致?detail 映射(auto/low/high、original→high)核实。
8. 无对应物字段的处置(store/include/prompt_cache_key/verbosity/reasoning.summary/reasoning.context/client_metadata/access_programs)是否确实"丢弃且留痕"?有无应映射而未映射的(特别是 Anthropic 2026 新字段)?

### B. 响应方向与流契约
9. rig StreamedAssistantContent 各变体→codex ResponseEvent 的转换有无丢字段(tool signature、Text.additional_params 引用、message_id、usage_metadata)?
10. reasoning envelope(`codex-rig-reasoning-v1:` 前缀,来源哈希 = protocol+endpoint+query+model 的 SHA-256):回放条件是否足够严格?有无跨厂商泄漏风险?envelope 注入(把历史 encrypted_content 当模型输入回放)是否可能被恶意会话文件利用?
11. usage 映射:Anthropic 的 cache read/write 并入 input_tokens 语义是否正确?total=0 时 input+output 兜底?
12. 工具调用生命周期:Added/Delta/Done 的 ID 一致性、增量重组等于最终 arguments、1MiB 上限、未确认调用不产出 Done——逐一验证测试是否真的锁住了这些不变量。

### C. 工程与安全约束
13. 生产代码无 unwrap/expect(工作区 clippy deny);无 unsafe;错误信息不含 URL/query 凭据(transport 的 sanitize_error 与 Debug 实现)。
14. 桥改动是否最小侵入 upstream 文件(合并友好);fork 逻辑是否集中在 fork 自有 crate。
15. 测试基建有效性:tests/wire.rs 是否真验证"最终 HTTP body"(而非中间结构)?live 矩阵(live-tests)的断言是否足以抓住"静默降级"类故障(参考 anthropic-effort canary 的 reasoning 断言)?

## 输出要求

- 按 P0(错误/数据损坏/安全)/P1(功能缺陷)/P2(风险与改进)/P3(建议)分级;
- 每条发现必须含:文件:行号、与官方文档的具体冲突点(引用文档字段名)、可复现的失败场景、建议修复方向;
- 明确区分三类:①违反官方文档 ②文档允许但国产网关可能不兼容(需实测) ③协议无对应的合理丢弃;
- 最后给出:你认为仍缺失的映射 Top3(按价值排序)以及总体结论(可发布/需修复后发布)。

## 边界

- 不要求审查 genai 桥(已搁置)、Bazel、npm 发布脚本;
- rig-core 0.42.0 的行为以 `~/.cargo/registry/src/*/rig-core-0.42.0/` 源码为准,可直接阅读;
- 所有结论基于代码与文档证据,不接受"应该是"式推断。
