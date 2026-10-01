# R2+R3 Plan：hosted 搜索历史保真与暂停续接（计划文档）

对应 Spec：`r2-r3-hosted-fidelity-spec.md`。批次均 <500 改动行、可独立构建。

## 技术方案

### 1. 版本化 envelope 与来源门禁（S1/S2）

- `hosted_tools.rs::web_search_call_events` 增参 `source: &str`，产出
  `{"version":1,"source":source,"blocks":[call,result?],"cited_text":[...]}`。
  调用点（stream.rs 的 completed 注入与暂停分支）传入既有的
  `reasoning_source(provider, protocol, model)` 结果——不新造身份构造。
- 新模块 `hosted_replay.rs`（避免 hosted_tools.rs 超 500 行）：
  - `parse_envelope(&Value) -> Envelope`：识别 v1 envelope / 旧版裸数组
    （`Envelope{version, source: Option<String>, blocks, cited_text}`）。
  - `replayable(envelope, current_source) -> Option<(blocks, cited_text)>`：
    v1 且 source 相等 → Some；其余（旧版、source 不符、形状坏）→ None +
    `tracing::warn`（一次一对，不含载荷内容）。
  - 上限与形状校验（S3）：
    `sanitize_for_request(groups) -> groups'`——按对计数 ≤64（丢最旧对并
    重建分组）、单对序列化 ≤40,960 字节（丢载荷保 call）、server_tool_use
    缺 id/name 或 result.tool_use_id 不匹配 → 丢该对载荷。保证任一 assistant
    组丢光对后不产生空 anchor 消息（复用 F08 的"先过滤再建组"路径与
    transport 空消息清理）。
- `request_messages.rs`：WebSearchCall 分支改走 envelope 解析；不可回放的
  对跳过（不建组、不加 anchor）；`cited_text` 与 blocks 同组投影。
- 事件发射侧的 40,960 检查保留（第一道），请求侧为权威（S3）。

### 2. 暂停原样续接（S4）

- `hosted_tools.rs` 新增 `raw_assistant_content(bytes) -> Vec<Value>`：按
  block index 顺序重组**全部** assistant 块——以 `content_block_start` 的
  原始 JSON 为基底，累加已知增量（text_delta→text、input_json_delta→input
  解析合并、thinking_delta→thinking、signature_delta→signature），其余
  start 帧字段原样保留；`redacted_thinking`/result 类块整帧保留。未知
  delta 类型无法忠实重建 → 该块按基底 + 警告降级（不静默捏造）。
- stream.rs 暂停分支：用户可见事件不变（文本 Message + WebSearchCall），
  另将 `raw_content` 存入 paused 槽；chainer 构造续接请求时把
  `raw_content` 放入请求 meta（新字段 `pause_raw_content: Vec<Value>`，
  经 `websearch_replay` 同一通道族传给 transport）。
- `transport.rs`：`ChatFamilyRewrite` 增加 `anthropic_raw_content:
  Option<&[(usize, Vec<Value>)]>`——splice 时**整体替换**目标 assistant
  消息的 content 数组（区别于 websearch_replay 的追加）。WebSearchCall
  续接条目产生的 anchor assistant 经 F08 清理后为空被移除，天然不会与
  raw content 重复（wire 测试深度相等断言固化）。
- `assistant_continuation_items` 保留（事件侧），返回值扩展为
  `(items, raw_content)`；`assembled_text` 并入 raw 重组后删除。

### 3. 跨响应配对（S5）

- stream.rs completed 注入分支：`pair_web_search_blocks` 后剩余未配对
  result，按 tool_use_id 在**请求输入**的 WebSearchCall(in_progress) 条目
  wire_blocks 中找同 id call；找到 → 以
  `web_search_call_events(PairedWebSearchBlocks{call, result:Some})` 发出
  **新的追加完成条目**（新条目即 completed；旧 in_progress 条目不动）。
- `request_messages.rs`：同 id 的 in_progress 与 completed 条目去重
  （completed 优先；投影级，不触碰 rollout）。
- 暂停 chainer 内部：pump 的未配对 call 跨 attempt 保留（链内内存态），
  后续 attempt 的 result 配对后正常发出。

### 4. 引用持久化（S6）

- `raw_assistant_content` 重组时，带非空 `citations` 的 text 块在
  envelope 的 `cited_text` 中登记 `{text, citations}`（顺序保留）。
- 正常 completed 轮：`web_search_blocks_from_anthropic_sse` 同轮捕获带
  citations 的 text 块（新增捕获通道，与 uses/results 并列），随对所在
  envelope 保存。citations 不属于任何对时丢弃并警告（无法定位回放位置）。
- 回放：`cited_text` 条目按保存顺序拼在 blocks 之后、同组内（GLM live
  已证明网关接受 assistant content 中的原始块序列；官方配方的原样回传
  语义一致）。

### 5. 协议与投影不变量（S7）

- `protocol/src/models.rs` 的 `wire_blocks: Option<Value>` 类型不变（载荷
  结构变化对它是 opaque）；F09 的 Responses/native 清空与
  `hosted_results_replay=false` 的整体关闭语义不变（含旧格式）。
- `core/context_manager/history.rs` 估算继续按 wire_blocks JSON 字节
  （envelope 后自然涵盖 cited_text）。

## 测试设计（对齐审查最小验证矩阵）

- `hosted_replay` 单元：envelope 解析/门禁/上限/形状/去重（合法+篡改+旧版）。
- wire（真实 HTTP）：S4 深度相等——fixture 含 thinking+signature→搜索→
  结果→带 citations 文本→pause，断言请求 2 的 assistant content 与原始块
  数组完全相等（不用 contains）；仅 thinking 的 pause；重复 pause 与上限；
  server/client 并行→client result→server result（跨请求配对，两个请求）；
  乱序/多 id；缺 id/未知块/篡改 source；取消与错误路径（依赖 R4 的取消
  通道，先以错误注入覆盖）。
- core 集成：resume/fork 后请求副本清空 wire_blocks、rollout 原前缀保持
  （扩展既有 rig_responses_bridge 场景至 envelope 格式）。
- 断言强度：暂停/回放对 content 与 tools 用整体相等（pretty_assertions），
  禁止 contains/抽查 id 代替。

## 风险与回退

- envelope 改变持久化格式：旧 rollout 裸数组按未知来源降级（有 wire 测试
  固化）；GLM live 需复验（T06）。
- transport 替换式 splice 与追加式 splice 并存：以两个独立 wire 测试锁定
  各自语义，防止互相污染。
