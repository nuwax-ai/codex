# 阶段 D Spec：会话与 hosted 工具完整流程（规范文档）

状态：草案（实施前需评审通过）。日期：2026-10-01。
上游依据：`claude-protocol-stability-handoff-2026-09-30.md` §13 D 行与 §1 契约；
协议事实以官方文档为准（Anthropic server-tools / web-search-tool 页，2026-09 核对）。

## 1. 目标

让 Anthropic 线的 hosted（服务端）工具会话从"能跑通"升级为"可多轮保真续接"：

1. **搜索结果成对持久化**：`server_tool_use` 与 `web_search_tool_result` 块按原始
   顺序进入历史，续轮回放时原样送回（含 `encrypted_content`），不再丢弃。
2. **pause_turn 续接**：`stop_reason: "pause_turn"` 时按官方语义把暂停的 assistant
   内容原样回传续接，续接次数有硬上限；不用"重试原 prompt"代替。
3. **混合 server/client 工具轮**：`tool_use` 与 `server_tool_use` 并存且 server 工具
   结果未决时，按官方语义先回 client `tool_result`，再收 server 结果块。
4. **provenance 参与投影**：桥线输出条目的 provenance 标记进入投影决策，resume/
   fork 不因投影丢 provenance。
5. **保存/恢复**：以上全部跨 rollout 持久（新会话、resume、fork、切模型）。

## 2. 非目标

- 伪造或合成任何 `encrypted_content`/`encrypted_index`/搜索结果（厂商密文不可构造，
  只能原样保存与回放；缺失即报错或降级，见 §4）。
- 跨厂商密文兼容（GLM 网关的 result 块与 Anthropic 官方密文不互通，按厂商隔离）。
- Chat 线 hosted 工具（无此概念，维持丢弃+warn）。
- 通用双向历史投影、存储迁移（阶段 C/E 范畴）。

## 3. 数据模型边界（审查重点）

- 历史仍保持**追加语义**；新条目类型必须是 `core/context` 中的 struct 并实现
  ContextualUserFragment。
- **硬上限**：每条持久化搜索结果对 ≤ 10K tokens；单轮会话累计搜索结果对 ≤ 64 个；
  超限走"截断+标记"而不是无界增长。任何单项 >1K tokens 的注入路径按仓库规则
  标注 P0 人工复审。
- 密文字段按不透明 base64 blob 存储，不解析、不复用、不跨会话拼接。
- rollout 格式变更需带版本化读取（旧 rollout 无新字段时按"丢弃搜索历史"降级，
  与当前行为兼容）。

## 4. 失败与降级矩阵

| 场景 | 行为 |
|---|---|
| 回放时 result 块缺失（旧 rollout/被截断） | 该对整体丢弃 + 警告；请求继续（等价现状） |
| 密文被篡改/无法回放（网关 400） | 明确错误指出哪一对损坏；不静默丢全部历史 |
| pause_turn 超过续接上限 | 终止 turn 并给出明确状态（非成功 Completed） |
| tee 溢出（8 MiB 上限已实施） | 块恢复停止（现状：warn + 跳过），不产生半块 |

## 5. 验收契约

1. 离线：wire 级测试覆盖保存/回放/截断/篡改/上限矩阵（真实出站字节断言）。
2. 真实：GLM 网关双轮搜索，第二轮请求体（wire 捕获）包含第一轮的 result 块原文；
   pause_turn 用官方兼容网关验证（无则登记 not-run）。
3. 既有行为不回归：`anthropic_replay_drops_web_search_call_history` 改写为
   成对回放后删除，替换为新正反例。
