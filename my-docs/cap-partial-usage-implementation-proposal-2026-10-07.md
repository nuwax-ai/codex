# cap partial output / usage：具体可裁决实施提案（2026-10-07）

状态：**提案，未实施、未批准**。本文是 Step0 决策文档（cap-partial-usage-step0-decisions-2026-10-06.md，D1–D5）要求的"具体可 review 的类型/字段/样例 diff 和有界策略"，供产品/复核裁决。所有代码块为建议形状与真实插入点（行号基于 HEAD `79bbe0e3f`），不是已落地代码。裁决通过前不实施 Step1–5；不静默缩小 legacy 契约。

---

## 0. 现状锚点（已核对源码）

| 位置 | 现状 |
|---|---|
| `codex-rs/codex-api/src/sse/responses.rs:443-477` | `response.incomplete` + reason=`max_output_tokens` 在解析 `response.usage` **之前**返回 `ApiError::InvalidRequest`，同帧 usage 被丢弃；该函数为 SSE 与 WebSocket 共用（`endpoint/responses_websocket.rs` 同路径） |
| `codex-rs/codex-rust-rig-bridge/src/convert_response.rs` `handle_stream_final`（Length 分支） | `FinishReason::Length` 返回 `ApiError::InvalidRequest`，`record.usage`（此时已可用）与 `finish_pending_output(pending)` 的未冲刷片段一并丢弃；已流出的 delta 仍可见 |
| `codex-rs/rollout/src/policy.rs:112-119` | `EventMsg::Error` 为 transient 不落盘；失败关闭由持久化的 `TurnComplete(error)` 表达 |
| `codex-rs/protocol/src/protocol.rs:1358` `EventMsg`、`:2241` `TokenUsage`、`:2264` `TokenUsageRecord`、`:2343` `TokenCountEvent` | usage 规范化类型与已持久化的 TokenCount 通道（`info: Option<TokenUsageInfo>`） |
| `codex-rs/protocol/src/items.rs:46` `TurnItem` | paginated 历史 item 枚举，无 partial 变体 |
| `codex-rs/exec/src/exec_events.rs:55` `TurnFailedEvent { error }` | 单字段 |
| `codex-rs/app-server-protocol/src/protocol/v2/thread.rs:1934` `ThreadTokenUsage` | `total`/`last` 必填 `TokenUsageBreakdown`；`:1894` `ThreadTokenUsageUpdatedNotification` |
| `codex-rs/context-fragments/src/fragment.rs:64` `ContextualUserFragment` | 模型可见注入的统一 trait（role/content_kind/markers/body/render_fragment） |
| `codex-rs/codex-rust-rig-bridge/src/usage.rs:9-65`、`src/stream_pump.rs:120-133` | bridge 用 `AnthropicUsage` 的 `Option<u64>` 保留 start/delta 的 presence 与显式零，再修正 rig 的 final usage；不是完全依靠 rig 聚合 |
| `codex-rs/core/src/context/contextual_user_message.rs:22-37` | user-role context 的文本识别器列表；新 fragment 必须同步注册，避免被投影为人类输入 |

---

## 1. D2：终止帧 usage 提取与失败载体（建议形状）

### 1.1 新错误变体（codex-api 内部枚举，非 wire schema）

```rust
// codex-rs/codex-api/src/error.rs
/// The caller-selected output cap was exhausted. Non-retryable, same
/// semantics as today's InvalidRequest("Output token limit reached; …").
/// `reported_usage` preserves observed counters and their completeness.
/// None means no usable report, including absent or invalid usage.
CapExhausted {
    message: String,
    response_id: Option<String>,
    reported_usage: Option<ReportedResponseUsage>,
},
```

- `ReportedResponseUsage` 是**拟新增**的协议载体，不是当前已存在的类型。建议含 `counts: ReportedUsageCounters`（input/output/cached/cache_write/reasoning/total 各为 `Option<i64>`，保留缺席与显式零）、`completeness: UsageCompleteness`，以及仅在计数完整、可按协议映射时有值的 `normalized: Option<TokenUsage>`。可由完整已报告计数推导的 total 必须注明 derived，不能冒称厂商直接报告；缺失计数绝不默认成零。协议明确约定缺席等于零的可选计数须单独核实，不能凭 `.has_values()` 判定 presence 或完整性。
- `message` 保持现文案前缀不变（`"Output token limit reached; increase max_tokens before retrying"` / `…max_output_tokens…`），既有按 message 匹配的断言（`stream_lifecycle_tests.rs:382` 等）继续命中；消费方迁移把 `ApiError::InvalidRequest { message } if message.contains("Output token limit reached")` 的匹配分支换成 `ApiError::CapExhausted { .. }`，行为等价。
- **决策点 D2-a**：新变体 vs 复用 InvalidRequest+side-channel。建议新变体（类型化、不可误配）；备选是不加变体、把 usage 放进独立事件，但那样 Core 需要按 turn 关联两条异步消息，竞态面更大。

### 1.2 Responses 共用解码（SSE + WebSocket 同一函数）

```rust
// codex-rs/codex-api/src/sse/responses.rs  reason == "max_output_tokens" 分支
let response_id = event
    .response
    .as_ref()
    .and_then(|response| response.get("id"))
    .and_then(Value::as_str)
    .map(str::to_owned);
let reported_usage = event
    .response
    .as_ref()
    .and_then(|r| r.get("usage"))
    .filter(|u| !u.is_null())
    .and_then(parse_reported_response_usage); // 拟新增：保留字段 presence
return Err(ResponsesEventError::Api(ApiError::CapExhausted {
    message: "Output token limit reached; increase max_output_tokens before retrying".into(),
    response_id,
    reported_usage,
}));
```

- `parse_reported_response_usage` 是拟新增解析器。完整报告的规范化复用现有 `ResponseCompletedUsage -> TokenUsage` 映射；字段缺失时保留可解析计数与 Incomplete，整份缺失/不可解析时为 None/Unknown。不得用成功 Completed 事件承载它。
- 整份未知/缺帧/整体不可解析 → `None`；部分可解析字段保留为 Incomplete，不阻塞失败路径（fail-fast 保持）。
- WebSocket 路径零额外改动（共用 `process_responses_event`），验收要求两侧各自出测试。

### 1.3 Chat / Anthropic bridge

```rust
// convert_response.rs  handle_stream_final  Some(FinishReason::Length) 分支
// 拟将 stream_pump 收集的原始 usage presence 随 final 传入转换器。
Some(FinishReason::Length) => {
    let reported_usage = final_usage_report; // 拟新增，不能仅凭 record.usage.has_values()
    return Err(ApiError::CapExhausted {
        message: "Output token limit reached; increase max_tokens before retrying".to_string(),
        response_id: record.response_id.or(record.message_id),
        reported_usage,
    });
}
```

- **Chat 终止后 usage chunk 顺序**：需实测 rig `StreamFinal.usage` 是否包含终止 chunk 之后的 usage chunk。若 rig 在 finish_reason 后立即 finalize 而丢掉后续 chunk，则需在 bridge 泵（`stream_pump.rs`）加"终止后短暂收尾读"或上游修复——列为实施验证点 V-D2-1，新增 wire 测试 `chat_length_finish_then_usage_chunk` 钉住。
- **Anthropic 聚合**：现有 `usage.rs::AnthropicUsage::observe` 保留 message_start/message_delta 的 Option 计数，`stream_pump.rs` 在 Final 上调用 `apply` 修正 rig 数值；`apply` 写回普通数值后 presence 信息仍留在 bridge 状态中，实施需在该落点导出报告。start/delta 均完整、仅 start/仅 delta、均缺、显式全零各自出 wire 用例：仅 start 不能把未知 output 当零，显式零不能被 `.has_values()` 当缺席。
- Core 以 host 为每次 sampling response 创建并持久化的稳定 `response_key` 去重（provider response/message id 只是辅助信息，缺席、重用或不同 response 的同名 id 不合并）。成功 Completed 和 cap 错误共用同一 usage reducer；同一 response 的累积 usage 快照取最新值，不相加 start/delta。完整已报告的 cap usage **必须进入新的全响应已知小计**，不重复写入 legacy `TokenUsageInfo`；旧数值口径和新增小计的差别见 D4。

---

## 2. D1/D5：partial 落盘表示与读侧不升级（建议形状）

### 2.1 新事件 + 新 item（两种历史模式共用一份载体）

```rust
// codex-rs/protocol/src/protocol.rs  EventMsg 新变体
/// Bounded visible partial transcript of a turn that exhausted the output
/// cap. Emitted once, immediately BEFORE the failure terminal
/// (Error → TurnComplete(error)); persisted in BOTH history modes.
CapPartial(CapPartialEvent),

// codex-rs/protocol/src/items.rs  TurnItem 新变体（paginated 投影）
CapPartial(CapPartialItem),
```

```rust
#[serde(tag = "type", rename_all = "camelCase")]
pub struct CapPartialEvent {          // == CapPartialItem 载荷
    pub turn_id: String,
    pub response_key: String,         // host 创建并持久化的稳定 sampling key
    pub response_id: Option<String>,  // 终止帧带的 response/message id
    pub fragments: Vec<CapPartialFragment>,
    pub reported_usage: Option<ReportedResponseUsage>, // D2 提取值，保留完整性
    pub budget: CapPartialBudget,
}

pub struct CapPartialFragment {
    pub fragment_key: String,         // 同 response 内稳定身份，重复 resume 不重复注入
    pub kind: CapPartialFragmentKind, // assistant_text | reasoning | tool_arguments
    pub item_id: Option<String>,
    pub text: String,                 // 与完整 render 的预算一起裁剪的前缀
    pub truncated: bool,              // 是否因单片段上限被截断
}

pub struct CapPartialBudget {
    pub fragment_count: u32,
    pub total_bytes: u64,
    pub enforced_caps: Vec<CapBudgetKind>, // fragment_tokens|fragment_bytes|turn_tokens|turn_bytes|turn_items
    pub token_budget_evidence: TokenBudgetEvidence, // verified_tokenizer | proven_upper_bound | unverified
}
```

- 落盘策略（policy.rs）：`EventMsg::CapPartial(_) => true`（两模式都持久化）；`EventMsg::Error` 维持 transient 不变；失败关闭 `TurnComplete(error)` 不变。
- 截断的工具参数**只记录字符串前缀**，`kind=tool_arguments` 的片段在 resume 投影里永不进入可执行路径（读侧过滤，见 2.3）。
- **决策点 D1-a**：独立事件+item（建议）vs 复用 `ResponseItem::Message{status:"truncated"}`（不建议，读侧无法区分失败 partial 与完整 item 的 resume 语义）。**决策点 D1-b**：旧读取器兼容策略——实测未知变体行为（V-D1-1）。当前 `load_rollout_items` 对单行解码错误计数并跳过，不代表所有分页/客户端读取器都如此；须分别记录整份失败、单行跳过或保留。`SessionMeta` 没有可直接复用的通用 rollout schema 协商版本（cli_version/multi_agent_version/history_mode 不能冒充）；若裁决采用版本协商，需另设计升级/降级规则，或选择独立 sidecar。不能预设旧读端无损兼容。

### 2.2 模型可见注入（ContextualUserFragment）

```rust
// codex-rs/core/src/context/cap_partial_output.rs（新文件，注册进 context/mod.rs）
pub struct CapPartialOutputFragment {
    body: String, // 含有界 turn/item 标识与截断声明；最终 render 另经预算校验
}

impl ContextualUserFragment for CapPartialOutputFragment {
    fn role(&self) -> &'static str { "user" }
    fn content_kind(&self) -> ContentItemKind {
        ContentItemKind("cap.partial_output".to_string())
    }
    fn markers(&self) -> (&'static str, &'static str) {
        Self::type_markers()
    }
    fn type_markers() -> (&'static str, &'static str) {
        ("<cap-partial-output>", "</cap-partial-output>")
    }
    fn body(&self) -> String { self.body.clone() }
    fn requires_separate_message(&self) -> bool { true }
}
```

- 同步将 `CapPartialOutputFragment::matches_text` 注册到 `context/contextual_user_message.rs::CONTEXTUAL_USER_FRAGMENT_MATCHERS`，并通过 `render_fragment()` 保留 `cap.partial_output` harness 分类。固定 type markers 用于兼容识别，动态 ID 放在预算内的 body，不能拼入静态 markers。测试覆盖 `is_contextual_user_fragment=true`、`is_guardian_context_message=true`、`is_user_authorization_message=false`，以及 `parse_turn_item` 不生成 UserMessage；继承/旧读取路径也不能将模型 partial 记为人类授权。
- resume 重建时：CapPartial 片段经此 struct 注入上下文；不生成最终 assistant message、不计入 completed items 统计（D5）；不改写既有模型历史前缀。
- 每个持久 fragment 对应一个独立模型 message，不能将整 turn 片段重新拼成一个超限 item；用稳定 turn/response/fragment key 保证原位重建、重复 resume 不重复注入。当前会话 cap 后继续与 resume 后继续须投影一致，只追加新 fragment，不重写旧前缀。插入点还包括 `session/turn.rs` 的有界 delta 累计与失败分支、`session/rollout_reconstruction.rs` 的重建，以及 app-server history/item 映射。Responses 目前忽略 function-call arguments delta（`sse/responses.rs`），若保留 tool_arguments，还需非执行性的有界采集路径，不能从空的 added item 假装恢复。
- 单片段 >1k tokens 即达 **P0 人工复审线**（见 §2.3 表），>10k 硬禁。下列 2,000 数值并不代表已通过人工复审或 token 上界证明。

### 2.3 有界策略（数值决策表——D1 未决边界的具体提案）

| 维度 | 建议默认 | 超限行为 | 说明 |
|---|---|---|---|
| 单片段 token | 2,000 | 保前缀，`truncated=true` | 对完整 render 计数或证明上界；**P0 人工复审未关闭**，不得超过 10k |
| 单片段 byte | 16 KiB | 同上 | 完整 render 的 UTF-8 字节，包含 markers/ID/截断声明 |
| 每 turn 累计 token | 8,192 | 停止追加后续片段，记 `turn_tokens` | 所有独立完整 render 的计数/上界之和；不影响已流出 delta |
| 每 turn 累计 byte | 64 KiB | 同上，记 `turn_bytes` | 完整 renders 之和；序列化载荷也须满足同值独立上限 |
| 每 turn 片段数 | 32 | 同上，记 `turn_items` | |
| token 预算证据 | 已验证 tokenizer 或可证明保守上界 | 无证明保持 unverified，不能认证硬限 | 固定算法/版本/适用模型范围并验收；response usage、bytes/4 均不能证明逐 fragment 上限 |
| 超限时 turn 结局 | 不变：Error → TurnComplete(error) 失败关闭 | — | partial 是附属证据，绝不升级为成功 |

- 建议算法：先构造一个 fragment 的完整 render（正文、固定 markers、有界元数据、truncated 声明），按选定 tokenizer 实测或证明其保守 token 上界；超过单片段或 turn 剩余预算则在 UTF-8 边界缩短正文，重新 render/计数，直到满足限制。即使正文为空仍超限时不追加该 fragment。计数证据随预算记录；模型切换后重新验证适用范围，不能用上个 provider 的 reported usage 给下个模型上下文背书。
- byte/count 在 delta 收集过程中即强制，不能先无界累计再终局裁剪；全部已序列化 CapPartial 载荷还须受 64 KiB 写入上限约束（ID、预算证据等也计入），必要时缩短/省略元数据或末尾片段并标明截断。最终 render token 校验是在此基础上的另一道门槛，不是 byte 上限的自动推论。
- 没有可信 tokenizer/上界时，byte/count 可证明，token 仍须记录 unverified；不得声称满足模型 token 硬限、勾选 D1/D5 验收，或仅凭 bytes/4 将未验证正文注入模型。需先补证明或由产品另裁决可验证的投影；不能静默缩小双模式 partial 可见契约。估算不写进 reported usage/ThreadTokenUsage。
- **决策点 D1-c**：上表数值与超限行为（保前缀 vs 丢弃整片段）。建议保前缀（可见性最大化 + truncated 标记可判读）；备选 KeepWholeOrFail（整片段要么完整要么不记）。
- legacy 与 paginated 双模式都必须：落盘、读取、resume 恢复可见 partial（投影可以不同，语义一致）。验收 V-D1-2/V-D1-3 各出双模式用例。

---

## 3. D3：exec `--json` 失败事件字段（建议形状）

```rust
// codex-rs/exec/src/exec_events.rs
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
pub struct TurnFailedEvent {
    pub error: ThreadErrorEvent,
    /// Complete reported usage summed over this turn's sampling responses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    /// Known counters when turn usage is partial; missing counters stay null.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reported_usage: Option<ReportedUsageCounters>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_completeness: Option<UsageCompleteness>,
    /// Bounded partial-item summaries mapped from CapPartialEvent (D1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partial_items: Option<Vec<CapPartialSummary>>,
}

pub struct CapPartialSummary {
    pub kind: CapPartialFragmentKind,
    pub item_id: Option<String>,
    pub truncated: bool,
    pub byte_len: u64,   // 摘要不携带正文，正文只进 rollout/context
}
```

- exec JSON 不是 v2 wire：缺席字段用 `skip_serializing_if` 保持**旧形态字节一致**（快照断言：无 cap 时序列化结果与改前逐字节相同，V-D3-1）；TS 导出同步重生成。
- D3 使用 D4 的 turn reducer：不能把最后一个 cap response 的 usage 当整 turn usage。完整时 `usage` 为成功 responses 加 capped response 的去重累计；Incomplete 时 `usage` 缺席，`reported_usage` 保留已知小计及 completeness；完全未知时报告 Unknown，不能回填前一 turn 或输出零。新增字段与旧 SDK 消费者兼容仍须裁决/实测。
- 事件顺序：`turn.failed` 是终局；partial 信息内嵌其上，不新增 `turn.partial` 事件（**决策点 D3-a**：内嵌 vs 独立 `turn.partial` 前置事件。建议内嵌——少一个可乱序的流事件；若裁决要独立事件，须与 D1 的 CapPartial 事件排序、稳定 ID 一起定）。

## 4. D4：app-server v2 usage 完整性（建议形状）

```rust
// codex-rs/app-server-protocol/src/protocol/v2/thread.rs
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum UsageCompleteness { Complete, Incomplete, Unknown } // wire: complete|incomplete|unknown

// ReportedUsageCounts（拟新增 v2 类型）各计数字段为 Option<i64>，
// serde/TS camelCase、export_to="v2/"；缺席计数序列化为 null，不填零。
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadTokenUsage {
    // 保留 legacy 数值与映射口径；这些字段不宣称覆盖新 cap usage。
    pub total: TokenUsageBreakdown,
    pub last: TokenUsageBreakdown,
    pub model_context_window: Option<i64>,
    // 全部 sampling responses（包括 known cap）的已知计数小计。
    pub reported_total: ReportedUsageCounts,
    pub reported_total_completeness: UsageCompleteness,
    // 最新 turn 的小计，不是最新 response/最新有值 turn；未知时 null。
    pub last_reported: Option<ReportedUsageCounts>,
    pub last_completeness: UsageCompleteness,
}
```

- `Turn`（v2）增：`reported_usage: Option<ReportedUsageCounts>` + `usage_completeness: UsageCompleteness`；只有全部 response 计数完整时才可转换成非 nullable 的完整 usage 对象。Option 字段在 wire 中为 T|null；旧历史没有完整性证据 → Unknown，不能默认 Complete。`reported_total_completeness` 描述新增全响应小计，不套在保持旧口径的 `total` 上。
- **去重与累计建议**：sampling 开始时生成 host response_key 并持久化；终止时记录 `{turn_id, response_key, provider_response_id?, counts, completeness}`。成功 Completed 与 CapExhausted 共用 reducer。同一 response 的迟到/重复累积快照替换同 key 的旧报告并按差值重算小计，不按帧累加；不同 response 逐计数相加（未报告字段保持未知），同一 turn 的工具循环、多次采样、重试和桥内部 pause continuation 必须各有可区分的 key。stable key 的跨层携带是 D2 实施工作，不能仅依赖 provider ID 或进程内未持久化序号。
- **状态转移建议**：某 turn 所有已开始 response 都完整报告且归属可证明 → Complete；部分计数/response 未报告而已有已知小计 → Incomplete；完全没有可靠报告 → Unknown。线程全历史可证明完整才为 Complete；混入旧历史无状态或未知 response 后，已有已知数值也只能为 Incomplete（没有任何可靠数值则 Unknown）。后续新成功 turn 不抹掉旧缺口；只有对同一 stable key 补齐真实报告并重放验证覆盖全部缺口后才可恢复 Complete，不能估算追认。
- **例子**：turn A 先成功 response 100 tokens，再 cap response 完整报告 20 → A 的 reported total/last=120，Complete；cap response 未报告 → A 小计=100，Incomplete。下一 turn B 完整报告 30 → thread 小计分别为 150/130；第二种线程仍 Incomplete，last=30/Complete。如果当前 turn 完全未知则 last_reported=null/Unknown，不能展示前一 turn 的 usage。其他计数字段按同样规则保留 presence；例中的 total 不代替各字段证据。
- **持久化与重放建议**：成功路径已有 `RolloutItem::TokenUsageRecord`（逐 response 的 usage/turn/thread 数值）及 `EventMsg::TokenCount`，但没有完整的未知状态。建议扩展或新增 response usage 记录保留 key/presence/completeness，再由 turn 终止记录覆盖范围；不要只给失败 turn 添加一个无去重关联的 TurnUsageRecord。CapPartialEvent 中的 usage 是同 key 的附属证据，重放不能第二次累加。线程完整性 checkpoint 必须保留历史缺口，compaction/resume/分页不能清零它；`thread/read`、历史分页、resume、重连与 live reducer 得到同一结果。
- schema 流程：`just write-app-server-schema` + fixture 同步；`#[ts(export_to = "v2/")]`；`#[ts(optional = nullable)]` 不用于此处（非 Params）。
- **决策点 D4-a**：枚举三值 vs 两级（known/unknown + subtotal）——建议三值；D4-b：旧历史默认 Unknown 是否可由产品定级为 Incomplete。

## 5. 兼容与验收矩阵（实施时逐格出测试）

1. 三协议 × {终止帧带完整 usage、部分字段、显式零、缺席、终止后迟到 usage chunk} → 恰一失败、无重采样、无合成 Completed；截断工具零执行（沿用 rig_output_cap.rs 既有断言扩展）。Responses native SSE/WebSocket 和 rig SSE 分别覆盖，native fixture 通过不能冒充默认 rig 路径通过。
2. legacy/paginated 双模式：partial 落盘→读取→resume 注入（wire 断言 fragment 出现且带标记、不作为人类授权）；token/byte/items 临界与超限分支、完整 wrapper/metadata、无 token 证明、模型切换、UTF-8、重复 resume、同会话继续的投影一致性。
3. 旧读取器实测（V-D1-1）：改前二进制读改后 rollout；改后二进制读旧 rollout（无 CapPartial 记录）。分别验证 recorder、分页历史与客户端未知变体行为，记录实际丢弃/报错结果；不宣称已有 schema 版本协商。
4. exec 快照：无 cap 字段缺席=旧字节；有 cap=新形态；SDK 重生成。
5. v2 schema fixture + 客户端断连重连后 thread/read 一致性；旧历史 Unknown；成功 response 后 cap、多 response turn、重复/迟到 usage、provider ID 缺席/重复、pause continuation、compaction checkpoint 的去重小计与 completeness。
6. 事件顺序：CapPartial 在 Error→TurnComplete 之前；app-server item/* 映射、turn/completed(status=failed) 保持。

## 6. 实施分批（每批 <500 行，依赖序）

① D2（CapExhausted + presence 载体/稳定 response key + 三协议提取 + 消费方迁移 + V-D2-1 顺序测试）→ ② D1/D5（载体 + token 证明及 P0 人工 gate + 双模式落盘/读/resume + fragment 注入/识别 + 边界测试）→ ③ D4（去重累计/持久化/重放完整性 + v2 字段 + schema）→ ④ D3（exec 字段 + 快照 + SDK）→ ⑤ Responses live 触顶（另行授权）。批内含必要调用方，保证可编译；分批行数和依赖须以实际 diff 核验，不代表上述所有设计已有实现。

---

**未决决策点汇总**：D1-a（独立记录 vs Message status）、D1-b（旧读取器兼容策略）、D1-c（上限数值表+保前缀+计数证明/P0 人工 gate）、D2-a（新变体及 presence 载体）、D3-a（内嵌 vs 独立事件）、D4-a（三值枚举及全响应小计）、D4-b（旧历史默认级）。对应批次的全部前置决策、兼容验证与 token 证明须满足后才可实施；不能将任一单项裁决等同全部批准。本文全部内容仍为提案。
