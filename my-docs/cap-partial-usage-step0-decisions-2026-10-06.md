# cap 终止契约：第 0 步兼容决策提案（2026-10-06，待过审）

状态：**提案，未实施**。对应修订后 Spec/Plan（cap-partial-output-usage-done-{spec,plan}-2026-10-06.md）。本文给出 Plan 第 0 步要求的具体决策项与建议方案，供产品/复核裁决；每项列出选项、建议、兼容影响与验证点。未过审不进入第 1 步。

## 决策 D1：失败 turn 的 partial 落盘表示（rollout）

现状：`rollout/src/policy.rs` 将 `EventMsg::Error` 与文本/reasoning delta 排除在持久化外；失败关闭由持久化的 `TurnComplete(error)` 表达（当前 policy.rs:113-119、141 起）。Paginated 历史以 ItemCompleted+TurnItems 持久化完整 item；legacy 的持久化投影不同，不能据此排除 legacy。

- **选项 A（建议，表示尚待裁决）**：新增显式失败/截断标识的 partial 记录，例如独立 PartialItem 类型或 TurnItem 变体；只由 cap 终止路径产出，不借 `ItemCompleted` 将片段标为完整项。legacy 与 paginated **均须落盘、读取和 resume 恢复**；两种历史可以采用不同投影，但可见 partial、失败状态与截断工具禁执行的语义一致。
- 备选 B（不建议，未裁决）：复用 `ResponseItem::Message{status:"truncated"}`；若选择此路，必须明确读侧如何保留失败/截断状态，避免把片段升级为完整模型产出 item，不能只靠 status 字符串猜测 resume 语义。
- **未决边界**：产品须给出单片段与每 turn 的 token/byte/count 硬上限、计数口径和超限行为（拒绝、保留前缀或其他受控投影），不能把“有界”当作已选算法或已实施限制。模型可见 fragment 必须有硬上限，单项不得超过 10K tokens；新项可能超过 1k tokens 必须标为 P0 人工复审。具体更低阈值及累计上限仍待裁决。
- 兼容影响：新增记录对旧读端、旧历史与双模式投影的行为须实测；不能预设旧读端会跳过未知变体。实现须写明升级/降级策略，保留既有失败关闭且不改写旧 rollout。
- 验证点：legacy/paginated 双模式落盘+读取+resume；旧读取器遇新记录的明确兼容结果；token/byte/count 临界值与超限分支。

## 决策 D2：截断帧 usage 的提取与"未知"表示（bridge→Core）

现状：Chat/Anthropic 的 bridge 转换路径与 Responses 的解码落点不同。Responses 在 `codex-api/src/sse/responses.rs::process_responses_event` 遇 `response.incomplete` / `max_output_tokens` 时先返回 `ApiError::InvalidRequest`，尚未处理 `response.usage`；此函数由 SSE 与 WebSocket 共用（WebSocket 调用在 `endpoint/responses_websocket.rs`）。不能把三协议改动都定位为 `convert_response.rs`。

- **建议**：分别在 Chat/Anthropic bridge 路径和 Responses 共用解码路径从实际帧提取厂商已报告 usage，再随 cap 专用错误/结果载体传向 Core。载体与层间类型仍须裁决；codex-api 现有规范化 usage 是 `codex_protocol::protocol::TokenUsage`，Core 再聚合为 `TokenUsageInfo`，不能未经评估直接向 API 层引入 Core 聚合状态。
- **未知=None**，绝不填 0。按实际帧顺序识别同一 response 的 usage；Chat 的终止 chunk 与后续 usage chunk、Anthropic 初始与终止 usage 的聚合须避免提前丢帧或重复计数。是否新增 `ApiError::CapExhausted` 及消费方迁移尚未实施；不合成成功 `Completed` 来取得 usage。
- 兼容影响：ApiError 新变体属内部错误枚举，非 wire schema；现有按 message 匹配的错误文案断言需同步（文案保持 "Output token limit reached; increase …" 前缀不变）。
- 验证点：Chat/Anthropic 的有/无 usage 与实际帧顺序；Responses SSE/WebSocket 各自通过共用解码路径，`response.incomplete.response.usage` 已知或未知均只产生失败，Core 不重复累加。

## 决策 D3：exec `--json` 失败事件的 usage/partial 字段

现状：`TurnFailedEvent { error }` 仅一字段。

- **建议**：`TurnFailedEvent` 增加两个**可选**字段（serde 默认缺席）：`usage?: TokenUsage`（终止帧报告则有）、`partial_items?: [ … ]`（按 D1 映射的截断片段摘要）。需验证既有消费者能忽略未知字段，TS 导出同步；不预设所有外部消费者均兼容。
- 备选：新增独立 `turn.partial` 事件，**尚待裁决**。若在失败终局前发出且不能成为成功终局，它不天然违反“关闭后不再发该 turn 的 delta”；需要与 D1 的持久记录及 app-server 映射一起评估排序、稳定 ID 和旧消费者兼容。
- 验证点：exec JSON 快照（新增字段缺席=旧形态字节一致；存在=新形态）；SDK 重生成。

## 决策 D4：app-server v2 的失败 usage 与累计完整性

现状：类型名为 `ThreadTokenUsage`（`app-server-protocol/src/protocol/v2/thread.rs:1934`），其 `total`、`last` 均为必填 `TokenUsageBreakdown` 数字对象；`modelContextWindow` 可为 null。`TurnCompletedNotification` 包装 `Turn`，现有 `Turn` 不含 usage/完整性字段。

- **建议（字段位置与形状待裁决）**：保留既有已知数值的统计口径，另增加每 turn 明确的 reported/unknown 状态与线程累计 complete/incomplete/unknown 状态；也可采用 nullable usage 与显式完整性字段的等价方案。失败 turn 的 usage、unknown 与累计完整性必须**持久化且可重放**，`thread/read`、历史分页、resume 和后续 usage 查询须得到相同语义；不能仅靠 `turn/completed` 的 live 字段缺席，或 `thread/tokenUsage/updated` 通知未收到来推断未知。
- 累计含未知 turn 后只能声称已知数值小计，不能继续标为完整总量；未知不累加也不填 0。如何表达 `last`（最近 turn 未知时不能把前一 turn 当作最新已知值而不说明）和旧历史缺少状态时的默认值，均列入产品/兼容裁决；缺少证据的旧历史不能默认 complete。
- 走 v2 schema 流程：camelCase、`#[ts(export_to = "v2/")]`、`just write-app-server-schema`、fixture 同步。v2 response/notification 的 `Option<T>` 字段必须在 wire 中序列化为 `T | null`，不得用 `skip_serializing_if = "Option::is_none"` 表示缺席；`#[ts(optional = nullable)]` 仅适用 client→server `*Params`，不能套在此处。D3 exec JSON 的缺席约定不直接套用到 v2。
- 验证点：schema fixture + 旧客户端兼容；有/无厂商 usage；客户端断连错过通知后重新读取；legacy/paginated 重放、resume 后完整性一致；旧历史无状态时保持未知。

## 决策 D5：partial 不升级为成功结果（读侧/resume）

- **建议**：legacy/paginated resume 重建时 partial 片段可见于上下文（按 D1 已裁决的硬上限），但不生成最终 assistant message、不计入"completed items"统计；截断工具参数永不执行（既有 D3 回归保持）。模型可见的 partial 注入须由 `core/context` 中明确 struct 实现 `ContextualUserFragment`，不能直接拼接无上限原始字符串或改写既有模型历史。
- 验证点：双模式 resume 后的请求 wire 断言（片段由受控 fragment 出现或按裁决投影）；边界/超限与统计断言；失败状态和 usage 完整性均可重放。

## 明确不做（维持 Spec 禁止清单）

合成桥层成功 `Completed`；猜 usage/以 0 冒充未知；执行截断工具；删除任一层失败关闭；改 cap 触发/不重采样语义。

## 过审后实施顺序

仍按 Plan 的依赖形成每批 <500 changed lines 的可编译阶段：①D2（三协议实际解析路径+Core 聚合及必要消费方）→ ②D1/D5（双模式 partial 持久化/读取/resume、受控 context fragment 与边界测试）→ ③D4（持久化/重放 usage 状态及 app-server v2 字段+schema）→ ④D3（依赖第③批的 exec JSON+SDK 映射）→ ⑤（另授权）Responses live 触顶。字段迁移若需要先引入最小基础类型，应与必要调用方同批；具体拆分以实施 diff 和测试依赖为准，不能承诺按文件拆开即可编译。

以上仍为第 0 步**决策提案，未实施、未批准**。D1/D3/D4 表示与兼容策略、token/byte/count 硬限和超限行为、旧历史完整性默认值均须产品裁决；本文修订不代表批准新产品逻辑。
