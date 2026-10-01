# 阶段 D Plan：会话与 hosted 工具完整流程（计划文档）

对应 Spec：`phase-d-spec.md`。批次均 <500 改动行、可独立构建验证。

## 模块归属

- 采集/回放转换：`codex-rust-rig-bridge`（hosted_tools、sse tee 泵、request_messages）。
- 历史条目类型：`codex-protocol/models.rs` 新 `ResponseItem::WebSearchResultBlock`
  （或复用现有 passthrough 通道，见 B1 决策点）。
- 上下文注入与上限：`codex-core/context`（ContextualUserFragment 实现 + 硬上限）。
- 配置：`web_search` 组新增 `persist_results`（默认开）与 `max_persisted_pairs`。

## 批次

### D1：结果对采集（只读路径）
- tee 泵在回收 `server_tool_use` 的同时回收配对的 `web_search_tool_result`
  （按 `tool_use_id` 配对；GLM 的 assistant 侧 `tool_result` 单独映射）。
- 转为 `WebSearchCall` 的伴随结果条目入 rollout（追加）。
- 测试：wire 级——GLM 形态与官方形态的配对采集、乱序 id、无 id 降级。

### D2：回放投影
- `request_messages` 投影 assistant 消息时，把持久化结果对还原为
  `server_tool_use` + `web_search_tool_result` 原始块（保持原顺序、原密文）。
- 决策点：还原为 Anthropic 原生块 vs Responses 形态中转——按"跨线不回放"
  原则仅 Anthropic→Anthropic 生效，其它线丢弃+警告。
- 测试：第二轮请求字节包含原文块；缺失对丢弃矩阵。

### D3：pause_turn 与混合轮
- 流终端策略识别 `stop_reason: "pause_turn"` → 状态 `Paused`（非 Completed）；
  会话层按官方语义回传续接，计数上限（默认 4）进配置。
- 混合轮：`tool_use` 存在且 server 结果未决 → 现有 client 工具闭环后自动续收。
- 测试：wire 级暂停/续接序列、上限终止、混合轮顺序。

### D4：上限与 provenance
- 硬上限实现（10K/对、64 对/会话）+ 截断标记；provenance 参与投影。
- 全链回归 + GLM 真实验收（双轮结果回放）+ 文档更新（FORK.md §10 收敛）。

## 风险与回退

- 网关密文兼容性未知 → D2 起所有回放带 `persist_results` 开关，默认可回退为
  现行为（丢弃回放）。
- rollout 体积增长 → 上限 + prune 策略与 D4 一并落地。
