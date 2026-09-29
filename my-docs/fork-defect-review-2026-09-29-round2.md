# fork 缺陷清单独立复核 — 第二轮（2026-09-29）

范围：目标 1–4（第一轮存疑项 + 对方反向挑战）。B1/B3 已定案不再复议。
方法：逐条读码 + rig/serde_json 上游源码作证。未改任何代码。

---

## 目标 1｜reasoning.rs 专项 — 结论：**不可达（契约内），防御性跳过合理；nit 一个**

### Q1：首 part 非 Text 时 delta 会丢吗？
**契约内不会。** 证据链：
- `ReasoningState::delta`（reasoning.rs:40）新建块用 `Reasoning::new("")`；
  rig 源码 `completion/message.rs:176-183`：`new_with_signature` 构造
  `content: vec![ReasoningContent::Text{..}]` —— **delta 创建的块首 part 恒为 Text**，
  reasoning.rs:44-49 的 `first_mut` 匹配恒成功。
- 首 part 非 Text 的块只有一个来源：`complete()`（reasoning.rs:57-64）用 provider 的
  完整 `Reasoning`（可能 Summary/Redacted/Encrypted 开头）替换。此后同一 id 再来
  delta —— rig 的流契约（streaming/mod.rs:3039-3046，Reasoning 变体文档）明确
  **"supersedes any prior ReasoningDelta carrying the same correlator: render it as
  a replacement"**：complete 之后的同 id delta 是**契约外输入**，防御性跳过正确。

### Q2：delta 事件与 Done 项会不会不一致？
**不会（按构造）。**
- 纯 delta 流：Done 内容 = finish()（reasoning.rs:66-95）抽取所有 Text part 按序拼接
  == 流出的 ReasoningContentDelta 累积 ✓（content_index 由 blocks[..index] 的 Text
  计数保证稳定，reasoning.rs:50-54）。
- delta 后被 complete 替换：Done 用 complete 块的 Text —— 与 native codex 相同语义
  （Done 项是权威替换，UI 以 Done 为准）。
- Summary/Encrypted/Redacted 不进 model-visible 内容：**按设计**——它们只进
  replay envelope（encrypted_content）原样回放给原端点（文件头注释 1-3 行明示）。

### 证伪用例（给将来若要加防御日志/断言的人）
```
delta("a","think") → complete("a", Reasoning{content:[Redacted]}) → delta("a","post")
```
"post" 被静默丢弃、finish 内容为空、无警告 —— 演示防御跳过存在；但该输入序列
违反 rig 超替契约，不会由 rig 适配器产生。

**nit（P3）**：reasoning.rs:44-49 跳过时可加一条 `debug!` 日志，便于契约外流排查。

---

## 目标 2｜B2 反向挑战 — 结论：**丢弃正确，挑战推翻**（缺的不是 id，是结果内容）

对方假设"若 WebSearchCall 保留了原始 block id 则忠实回放可行"。逐环检验：

1. **id 确实保留了** ✓：codex-protocol `WebSearchCall`（protocol/src/models.rs:1190-1203）
   字段 = `id / status / action / internal_..._passthrough`；桥侧 hosted_tools.rs:145-163
   把网关签发的 block id 原样存入（rollout 实证：`"id":"call_d93c..."`）。
2. **但结果内容从未持久化** ✗：`WebSearchCall` **没有 results 字段**；Anthropic 线的
   `web_search_tool_result` / GLM 的 `tool_result` 块被 hosted_tools.rs 明确忽略
   （文件头注释"carry no Codex item and are ignored"）。
3. **忠实回放需要成对块**：Anthropic 多轮回放的形态是 assistant 消息含
   `[server_tool_use(id,name,input), web_search_tool_result(tool_use_id, 内容)]`。
   只回放 server_tool_use（id 有了）而没有 result 块 → 悬空 tool_use（网关配对校验
   风险，恰是第一轮已论证的）；**合成空 result = 伪造检索证据**，不可接受。
4. **旁证**：OpenAI native 回放 web_search_call 也只带 id+action（无结果内容）——
   OpenAI 靠服务端按 id 复水；兼容网关不提供这种复水。

**结论：挑战的前提（id 足够）不成立——缺失的关键拼图是检索结果载荷。**
丢弃是当前持久化模型下的唯一正确行为。"多轮检索能力"若要做，路径是：
新增结果持久化字段（phase-3 特性）→ 回放成对块，属**增强**而非缺陷修复。
FORK.md §4 应把它记为已知局限+规划项。

---

## 目标 3｜N1 收口 — 判断确认 + 测试方案

### 3.1 确认"现有 core 回归测试走 native"
- core/tests/suite/compact.rs:284-291：`non_openai_model_provider` 克隆
  built-in `"openai"`，只改 name/base_url/websockets；
- model-provider-info/src/lib.rs:600：`create_openai_provider` 设置
  `provider_id: Some(OPENAI_PROVIDER_ID)`（克隆保留）；
- lib.rs:689-691 + 703-712：`is_first_party()` 按 provider_id 匹配 "openai" →
  `uses_model_bridge()`（wire=Responses、bridge=None、first-party）= **false → native**。
证据闭合：我的 `auto_compact_recovers_...` 测试覆盖 native 线，桥线确实只有
分段证据（wire 400-body 分类断言 + live Step 旁证），无端到端。

### 3.2 桥线端到端测试方案（只设计，未写）
- **位置**：`core/tests/suite/rig_responses_bridge.rs`（该文件已
  `#![cfg(feature = "rust-rig")]` 门控并手写 `ModelProviderInfo`——正是"自定义
  provider 强制走桥"的现成样板）。
- **测试**：`bridge_chat_compact_recovers_from_context_window_rejection`
  1. provider：`provider_id=Some("mock-chat")`、`wire_api=Chat`、
     `base_url=mock server`、无 first-party id → `uses_model_bridge()=true`；
  2. `mount_response_sequence`（复用现有 mock）：
     `[SSE 轮1(usage 70k), SSE 轮2(330k), **400 JSON(context_length_exceeded)**,
     SSE 摘要, SSE 轮3]`；
  3. 三轮 turn，断言：共 5 个请求、第 3 个是 400、第 4 个（重试）input 条目数
     **严格少于**第 3 个、轮 3 正常完成、rollout 含 compacted；
  4. **桥路径证明**：断言 codex 日志/stderr 含 `"Dispatching chat stream via rig"`
     （该串是桥分派日志，native 无）。
- 同构可加 anthropic 变体（wire_api=Anthropic）覆盖 with_terminal_check 与 400
  共存的路径。

---

## 目标 4｜两个次要点

### 4a｜B4 "必成功"修正 — **对方挑战部分成立，实际结论不变**
深挖 serde_json 1.0.151 源码后的完整链条：
- `ResponsesApiTools(Arc<RawValue>)`（codex-api/common.rs:247-272，Serialize 直接
  委托 RawValue）；
- RawValue 的 Serialize（raw.rs:345-351）发射 token 结构体
  `"$serde_json::private::RawValue"`；
- **to_value 并非直通**：value/ser.rs:276 识别 token → RawValueEmitter → 其
  `serialize_str` = `crate::from_str(value)`（value/ser.rs:925-927）——**重新解析
  原始 JSON 文本**。
- 因此 to_value 的可达失败面：① 原文非法 JSON（不可能——RawValue 本身来自已验证
  JSON）；② **递归深度 >128（serde_json 默认 limit）** —— 荒谬深嵌套的工具 schema
  才会触发；③ 非字符串 key（JSON 不存在）。数字超 u64 → f64（有损但不失败，
  phase-1 已知并记录）。
**修正我上轮表述**："必成功"改为"唯一可达失败是 >128 层嵌套"。实用结论不变：
`unwrap_or_default` 实际不可达，静默零工具不会发生；补 `debug_assert!`/warn 仍是
合理的 P3 nit。

### 4b｜"200 + 非 SSE JSON 错误体"病态网关 — **记已知边界即可**
- live 证据：三家厂商全部以规范 4xx/5xx + JSON body 报错（Step 400 input_invalid、
  MiMo 400 feature_not_supported、GLM 405/400——本轮复核与既往 live 日志）。
  无一例 200+JSON 错误体。
- 建议：FORK.md 已知局限登记；可选 P3 加固——with_terminal_check 在"零 SSE 帧
  即 EOF"时把原始 body 前 N 字节附进错误信息（一次小改，不紧急）。

---

## 更新后的修复批次建议

| 批 | 项 | 优先级 |
|---|---|---|
| P2 文档 | A1 错名×2、FORK.md 补已知局限（§4 跨轮检索丢失+phase3 路径、200+非SSE 边界）、旧计划文档盖戳、删顶层 `codex-rust-rig-bridge/` | 维持 |
| **P2 测试** | **N1 桥线端到端（本报告 3.2 方案）**；跨轮 web_search live 场景 | 维持 |
| P3 代码 | tee 无 hosted 工具时跳过；request_tools debug_assert；**reasoning.rs 契约外 delta 加 debug 日志（本轮新增）**；with_terminal_check EOF 附 body 摘要（4b 可选） | 扩充 |
| 增强登记（非缺陷） | web_search 结果持久化 + 成对块回放（目标 2 的 phase-3 路径） | 新增 |

无新发现的正确性缺陷；第二轮全部四个目标收口。
