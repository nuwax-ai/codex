# 阶段 D Plan：会话与 hosted 工具完整流程（计划文档）

对应 Spec：`phase-d-spec.md`。批次均 <500 改动行、可独立构建验证。

D1–D4 已于 2026-10-01 全部实施：D1 采集配对（含 type 字段修复）；D2 回放投影
与开关；D3 pause_turn 桥内续接（上限 4）；D4 上限与 GLM live 验收（turn2 网关
400→修复后整场景 PASS；此前 400 诊断本身证明块已到达 turn2 wire）。

## 模块归属

- 采集/回放转换：`codex-rust-rig-bridge`（hosted_tools、sse tee 泵、request_messages）。
- 历史条目：`codex-protocol/models.rs` 的 `ResponseItem::WebSearchCall` 增加可选
  `wire_blocks`（评审决议：单 opaque 字段，优于新 variant——合并摩擦与 rollout
  兼容性都更好）。
- 上限强制：桥边界（发射侧单对上限、投影侧每请求上限），非 core/context。
- 配置：provider 级 `hosted_results_replay`（默认 true，走 ModelProviderInfo →
  api Provider → 桥，与 max_output_tokens 同管线）。

## 批次

### D1：结果对采集（只读路径）
- SSE 汇编器同时回收 `server_tool_use` 与结果块（官方 `web_search_tool_result`
  单帧完整；GLM assistant 侧 `tool_result`），按 id/tool_use_id 配对；未配对
  （混合轮 pending）→ status `in_progress`。`WebSearchCall` 增加可选
  `wire_blocks`（原始块数组，opaque）；配对完整 → status `completed` + 双块载荷。
- 测试：wire 级——GLM 形态与官方形态的配对采集、乱序 id、无 id（in_progress）
  降级、发射项携带 wire_blocks 载荷。

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

- 网关密文兼容性未知 → D2 起所有回放带 `hosted_results_replay` 开关（provider
  级，默认 true），一行可回退为现行为（丢弃回放）。
- rollout 体积增长 → 上限 + prune 策略与 D4 一并落地。
