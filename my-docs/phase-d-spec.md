# 阶段 D Spec：会话与 hosted 工具完整流程（规范文档）

状态：初版已实施，尚未完整验收（2026-10-01）。Codex 后续复查发现来源隔离、
恢复侧硬上限、完整暂停块及混合轮仍需修复，见 `codex-review-2026-10-01.md`。
此前评审修正 6 处已写回本文：持久化载体为
WebSearchCall.wire_blocks 单 opaque 字段（非 context fragment）；pause_turn 为桥内
透明续接（原缺陷为直接 Stream 错误）；顺序保真限于对间/对内；上限在桥边界
强制（bytes/4 近似）；开关走 provider 字段 hosted_results_replay；GLM 结果块按
原样形状回放。
上游依据：`claude-protocol-stability-handoff-2026-09-30.md` §13 D 行与 §1 契约；
协议事实以官方文档为准（Anthropic server-tools / web-search-tool 页，2026-09 核对）。

## 1. 目标

让 Anthropic 线的 hosted（服务端）工具会话从"能跑通"升级为"可多轮保真续接"：

1. **搜索结果成对持久化**：`server_tool_use` 与结果块（官方
   `web_search_tool_result`；GLM 的 assistant 侧非标准 `tool_result` 按其原样
   形状）成对进入历史，续轮回放时原样送回（含 `encrypted_content`），不再丢弃。
   顺序保真范围：**对间顺序与对内顺序保持**；与 assistant 文本的原始字节级
   交错不保证（事件时序决定文本项先落、对在后）——登记为已知近似。
2. **pause_turn 续接**：现状缺陷——桥把 `pause_turn` 归入未知 finish_reason
   直接报 Stream 错误。修复为**桥内透明续接**：从 tee 提取本次全部 assistant
   原始块，按官方语义原样追加回请求续接（工具数组不变），续接次数硬上限 4；
   超限给出明确错误。不引入新的核心协议状态（codex-core 无 Paused 概念，
   桥内闭环）。
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

- 历史仍保持**追加语义**。持久化载体是 `ResponseItem::WebSearchCall` 新增的
  单个可选 `wire_blocks` 字段（原始 Anthropic 块的不透明 JSON 数组，向后兼容：
  旧 rollout 反序列化为 None）。这些是**历史条目**而非 app 注入 fragment，故不
  走 ContextualUserFragment；仓库 context 规则（有界、硬上限）以桥边界强制的
  方式满足（见下）。
- **硬上限**（桥边界强制）：单对载荷 ≤ 10K tokens（按 bytes/4 近似估算，
  超限丢弃该对载荷并告警——降级为可回放性丢失，不截断密文本身）；单次请求
  投影 ≤ 64 对（超出丢弃最旧并告警）。任何单项 >1K tokens 的注入路径按仓库
  规则标注 P0 人工复审。
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
