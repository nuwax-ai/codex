# results-followup-2026-10-11.md — Codex 复审后的后续开发收据

基线：`test` 分支，起点 `d92053516`（Codex 独立复审修复检查点，保留全部安全修复，未回退任何一项）。本文件记录本轮新开发；历史收据见 `results.md`（批 A–S），其结论边界以 `codex-review-2026-10-11.md` 修正为准。

环境：macOS（Darwin 27，arm64）；全部命令 offline --locked、retries 0（注明处除外）。今日机器负载中等（测试窗口 load 观测 <10），无极端并发窗口。

## 本轮批次（本地提交，未 push）

### 批 FA — `46f2f87e3`：CapExhausted 类型化贯穿错误映射（follow-up 第一批第 2 项前半）

- `CodexErrorDetails::CapExhausted { message, response_id, reported_usage }`：Display 沿用 InvalidRequest 时代的逐字文案；`retry_delay` 归入终局（None，不重试）；`to_codex_protocol_error` 落 `Other`（与旧 InvalidRequest 映射一致的客户端分类）。`api_bridge` 改为整体搬运 payload，不再丢弃 usage/response id。
- 证据：codex-api + protocol 585/585（新增 2 个映射用例：Incomplete 报告保 presence、无伪造 normalized；无 usage 终端完整携带）；sse/responses 旧断言更新为类型化变体（终局不重试断言保留）；rig-bridge 256/256；core rig_output_cap 8/8 串行。
- 首败记录：批内一次 `chat_stalled_stream_after_length_chunk_costs_the_idle_budget` 并行联合跑失败（墙钟窗口 vs 并行负载），隔离与串行复验均过——该用例自身 4.5–25s 墙钟断言的负载敏感窗口，未改断言。

### 批 FB — `243ddb5a1`：host sampling key + cap 采样记账（第一批第 2/3 项）

- 账本身份：每个 wire 请求铸造 `sampling:{turn_id}:{seq}`（Session 上进程单调 AtomicU64；follow-up 步、重试、传输回退均不重用），`TokenUsageRecord.response_key` 持久化（serde default 兼容旧 rollout）。账本按 host key 归并；无 key 的 legacy 记录沿用 provider response id 键；无 id 无 key 保持加法。`ResponseLedgerKey` 命名空间分离：provider id 字面上撞形 minted key 也不可别名。resume 重建按同规则（`ResponseLedgerKey::of`）。
- cap 记账：turn 流循环在 `Some(Err(err))` 且 `CapExhausted` 时，将 reported counters 经同一 reducer 记入账本并持久化（缺失计数记 0，total 不做求和伪造）；错误仍恰一次、不可重试、不发 RawResponseCompleted。
- Chat wire usage presence：新增 `ChatUsage` 观察者（terminal check 喂原始帧；每帧 usage 对象整替快照；显式 0 保留），修掉 cap 报告从 rig 归一化回退丢 total 的问题。Complete 仍要求全字段直报，故 Chat 报告形式上恒 Incomplete（该 wire 无 cache-write 计数）——按既有 D2 契约如实保留。
- 集成重写：`same_id_responses_across_distinct_samplings_count_separately_after_resume`——5 个复用同一 provider id 的不同采样全部独立计数（30/40/85/135/190）、host key 唯一、5 请求；`chat_output_cap_error_is_terminal_without_core_resampling` 追加固化断言（usage record 4/64/68 落在 `sampling:` key 下）。单测：跨请求不别名、host key 跨 turn 迁移保他采样、key 命名空间分离。
- 发现并修复实现缺陷：per-call 序列计数在 follow-up 步重置导致两个 wire 请求同 key（集成测试首败暴露，左 75 右 55）——改进程单调计数后闭合。

### 批 FC — `78acb6df4`：观测点有界 partial 采集 + CapPartial 发射（第一批第 4/5 项）

- `CapPartialCapture`：流观测点增量采集（OutputTextDelta / ReasoningSummaryDelta（非截断模式）/ ReasoningSummaryDone（截断模式）/ ReasoningContentDelta / ToolCallInputDelta / OutputItemDone 整项），16KiB/片段、64KiB、32 片段，UTF-8 边界保前缀，超预算字节即时丢弃（不先收集后截断）；证据记录哪个预算丢弃了内容（含关闭后到达内容的 close-reason）。整项与增量不双采（key 去重）。
- 发射：cap 终局在记账后、错误传播前，`Feature::CapPartialDiagnostics`（新 feature，默认 OFF）开启时发送恰一个 `EventMsg::CapPartial`；rollout policy 双 mode 持久化（复用 send_event → 策略过滤，无双重写）。默认关闭即升级/降级策略：不开启则 rollout 永不含该行，旧版本读者不会遇到。
- 如实边界（测试钉住）：rig bridge 缓冲工具事件到完成，Chat wire 的 cap 在循环观测点看不到工具参数（断言无 ToolArguments 片段）；原生 Responses wire 终局前完成整项，整个工具调用可作为非执行诊断捕获（断言含 `call shell: ...` 且 exec 0）。模型可见注入未做（tokenizer 未验证，token_evidence 恒 Unverified，P0 人工门开放）。
- 集成：default OFF 无事件无 rollout 行；开启后 default/legacy 双 mode 写入、与 usage record 同 host key、rollout 行 event_msg/cap_partial 线形、resume 后续轮次正常完成；Responses 整工具诊断用例。单测 8/8（边界/UTF-8/丢弃后不增长/证据类）。
- 规模登记：本批 +912/−336（含整文件重写 cap_partial.rs 与其测试），净增约 576，超过普通 800 行指导的边缘由测试重写占比解释；未再拆分以免留下无测试的中间态。

### 本轮_receipt（串行 threads 1，offline --locked，retries 0）

- 聚合 receipt（Codex 复审末批同一过滤器 + 新家族，fresh app-server helper binary 重建后）：**108/109**。唯一失败 `legacy_custom_ca_fallback_is_limited_to_reqwest_default` 为 `route_aware_client_pool` 子进程族（Codex 复审首批同样失败过、隔离即过的已知负载敏感族，批 A 登记 R7 机制：子进程默认 client 的 SCDynamicStore 读取 vs 2s fixture 预算）；本会话未改 http-client 产品代码；隔离复验 PASS。不以此标签关闭该族在 CI 的失败。
- codex-features 63/63；schemas（config + app-server）再生成并随批提交；`git diff --check` 干净。

## 40 项状态（仅列本轮有变化的项；其余沿用 results.md 表）

| 项 | 本轮变化 | 证据 | 仍开放 |
|---|---|---|---|
| T16 | cap 终局 usage 经类型化错误进入 core 账本并持久化；Chat wire presence 观察补齐 | 批 FA/FB；585/585、256/256、集成双用例 | Chat 迟到 usage chunk 顺序（V-D2-1）；Anthropic start/delta 之外的 wire 家族细节见 T19 |
| T17 | host sampling key 落地（mint/持久化/重建）；跨请求同 provider id 不别名有公共集成证明 | 批 FB；11/11 ledger+rollout 家族 | compaction 采样仍 legacy 键（None，行为不变）；app-server/读侧按 key 的展示 |
| T18 | 观测点有界采集 + 门控发射 + 双 mode 持久化 + resume；D1-c 边界全钉 | 批 FC；21/21 cap 家族、双 mode resume 用例 | crash/export/archive/delete 矩阵、两次连续 resume、v2/RPC 读侧投影、模型可见注入（P0 门）、exec/SDK 串联 |
| T15 | 无变化（七项裁决仍为准） | — | — |
| 其余 T01–T14、T19–T40 | 无变化 | 见 results.md 及 codex-review §3 | 按 codex-review §3 主线推进 |

## 开放与下一步（依赖顺序）

1. T18 读侧收口：crash 后 rollout 半行、export/archive/delete 与两次 resume 矩阵；app-server v2 ThreadTokenUsage/completeness 投影 + rawResponseItem 兼容；exec/SDK/UI unknown 持久性（第一批第 7 项）。
2. 第二批（代理/生产默认）：真实 Basic Auth/CONNECT/407/TLS/SNI/IPv6 头双轨；两版本 reqwest 对照；线程生命周期注入；默认迁移 diff 待裁决。
3. 第三批（CI/栈）：827 逐身份；V8 配对工件；bwrap 能力定位；Ubuntu SIGABRT/Windows 0xc00000fd 复现（显式 4MiB 线程）。
4. 第四批：T21–T40 原范围。

本轮无 push、无 tag、无发布；全部为本地提交（`46f2f87e3`、`243ddb5a1`、`78acb6df4`）。
