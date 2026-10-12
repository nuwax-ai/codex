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

### 批 FD — `9088aca8f`：读侧矩阵收口（第一批第 5 项剩余）

- 双历史 mode 的 emits/persists/resume 用例扩展为两次连续 resume + 截断 crash 尾行（写者中途被杀的半行 JSON）：第二次 resume 仍能从同一 rollout 起动并正常完成后续轮次。
- thread-store 物化测试：含 `EventMsg::CapPartial` 行的 rollout 干净物化（全部行解码消费、offset 到文件尾、双 turn 正常投影、partial 不产生任何 item）。
- 边界登记：archive/delete 是文件搬移+元数据操作，不解析 rollout 行内容，由其通用套件覆盖；本 fork 无 thread-export RPC 可测。
- 证据：materialization 家族 39/39；rig_output_cap 串行 21/21。

### 批 FE — `e4797d0dc`：cap 采样花销进入共享账目与 v2 表面（第一批第 7 项主体）

- cap 拦截点在 ledger 记账后运行与 Completed 相同的后记账（`update_token_usage_info`：context 总量、按模型统计、共享 rollout 预算、TokenCount 事件）；记账失败只记日志，cap 错误保持唯一终局。
- 由此 v2 `thread/tokenUsage/updated` 在 cap 失败的 turn 上到达（notification 由既有 TokenCount 映射，无新增 wire 面）。core 原生 Responses 终局 fixture 补带 usage，钉住 SSE incomplete 帧计数同样以 host key 落账。
- 新 v2 RPC 集成 `capped_sampling_reports_terminal_usage_over_v2`：cap 终局 → tokenUsage 通知（last 与 thread total 均含终局帧计数）、turn/completed 带错误、零 rawResponseItem/completed、无完成的 agentMessage item。
- 证据：core rig_output_cap 13/13 串行（含原生 wire usage 断言）；app-server 28/28（turn_cap_usage、token_usage 家族、R1b guardians）threads 1；workspace check 干净。

### 批 FF — `7922659b7`：真实认证/CONNECT/SNI wire 双轨（第二批第 1 项）

- 新 `resolved_default_auth_wire_tests`：子进程双轨记录两侧发往代理的**完整请求头**并断言逐字节一致——plain userinfo 的 `Proxy-Authorization`（精确 base64）与绝对 URI 请求行、percent-encoded userinfo 解码一致、**真隧道 CONNECT**（CONNECT 行+认证头一致；rustls origin 用 SNI 嗅探 cert resolver 实测两侧 ClientHello 的 SNI=URL host，随后两侧均按约拒绝自签证书）、407 同状态零重试风暴、IPv6 代理字面量。模块并入 `http_client_default_pool_local` 组；http-client 增加 tokio-rustls dev-dep（workspace 版本，MODULE.bazel.lock 无漂移）。
- 证据：resolved_default+matcher 家族 29/29（threads 1）。
- 窗口登记：load 7–13 下四个 `route_aware_client_pool` fixture 用例（managed_redirect、direct_and_routed、legacy_custom_ca、streams_request_bodies）在 2s accept 预算上失败——已在 `c1a5dd024` 与 `7922659b7` 两个无本批改动的树上复现（批 A 的 R7 负载敏感族），保留开放。

### 批 FG — `8d908c96d`：loader 线程生命周期注入收口（第二批第 3 项 / F12 余项）

- cfg(test) spawn 失败注入缝（与既有 simulate_thread_death 同族）+ 三测试：spawn 失败发布短 failure TTL 且无重生风暴、条件清除后恢复；重建预算耗尽（8 次）后永久不可用（跨 TTL 窗口无无限重生）；wedged 读的 stale 宽限过期后在等待预算内回答 None（有界陈旧）。
- 证据：system_settings_store 11/11（threads 1）。

### 批 FH — 跨版本 parity + 迁移裁决材料（第二批第 2/4 项）

- 澄清事实：workspace 双 reqwest = 0.12.28（codex 核心栈，含 http-client 与本双轨的参照 client）与 0.13.4（rig bridge transport）；lock 中 0.12 的另一消费者是 code-mode（V8 门控，对照随配对工件工作顺延）。
- 新 `proxy_version_parity_tests`（bridge 内，子进程）：0.12 参照 / 0.13 bridge 形态（retry-never，与 `client::http_client` 同构）/ ResolvedDefault 池三栈 × env http/https、NO_PROXY 命中与未命中、CGI 直连可达 origin、重定向链保持代理（且三栈完成）六场景同侧。模块并入 `rig_bridge_wire_local` 组；bridge 增加 reqwest 0.12 dev-dep（同 lock 内既有版本，Cargo.lock 仅加 dev 边）。
- 迁移裁决材料成文：`default-migration-adjudication-2026-10-11.md`——具体 diff（config 两处主干一行 + 各显式调用点）、兼容证据汇总、四个待裁点（SystemProxyFallback 组合 gate 不能无声丢失、CFRunLoop 因果、同步读取差异面、OTLP 并发窗口复测）、A/B/C 选项。**未实施，未裁决。**
- 证据：parity 六用例绿；codex-rust-rig-bridge 全量 259/259。

### 批 FI — run6 827 终态逐身份归因 + code-mode 主根因闭环（第三批第 1 项）

- 解析三平台日志全部失败块，只用显式证据（panic 原文/assert_cmd 错误/信号行）分类；`ci-run6-failure-ledger.tsv` 重写为 827 行 × (category + 首败摘录)。未归类从 564 降至 154（140 assertion-open + 14 no-evidence，待逐测 triage）。
- **主根因（567/827=69%）**：code-mode host 家族——上游 `97576b179`（#36217）移除进程内 provider；host 二进制因 V8 工件缺口未构建；`Disabled` provider → `effective_tool_mode` 静默 Direct 回退 → `unsupported custom tool call: exec`。四环因果链全证据；**本机复现同签名失败**（`code_mode_can_return_exec_command_output`、`code_mode_can_resume_after_set_timeout`、`app_tool_exposure::connector_omissions_*`）。归因纪律抽验：按套件名过激的重分类被本地通过的反例（guardian_denial 4/4、extension_sandbox file_access）纠正回退。本机构建 host 亦命中同一 v8 404——家族修复验证与 CI 同被工件缺口阻塞。未归类降至 93（79 assertion-open + 14 no-evidence）。
- 其余：harness deadline 显式 53（处置逐测试开放）、bwrap 109（runner 能力，T32 同事项）、stack-overflow 2 身份（ubuntu SIGABRT=windows 0xc00000fd 同测试）、Windows runner PowerShell 2；另发现 **Windows 311 个"通过但泄漏"终态行**（不在 827 内，独立卫生信号）。
- 详见 `ci-run6-attribution-2026-10-11.md`。不称任何项通过；分类只升级证据。

### 批 FJ — V8 配对工件接通 + macOS 家族样本验证（第三批第 2 项）

- **事实修正**：此前"ptrcomp_sandbox 归档从未发布"的结论只对 denoland 默认源成立——**上游 openai/codex 的 `rusty-v8-v150.4.0` release 就是为本 pin 发布的配对工件**（8 个 target 的归档/binding/checksum），且仓库自带完整基础设施：`setup-rusty-v8` action（下载 + 双层校验：对 `third_party/v8/rusty_v8_150_4_0_release_manifests.sha256` 的 manifest-of-manifest + 文件 sha256）+ `rusty_v8_bazel.py` 版本解析。V8 pin 未动。
- **本机判别实验（决定性）**：按 action 同一流程下载 aarch64-apple-darwin 工件并双层校验 → `RUSTY_V8_ARCHIVE`/`RUSTY_V8_SRC_BINDING_PATH` 下 host 二进制构建成功（1m46s）→ macOS 代表性集合通过（不能覆盖三平台 567 个历史身份）：core `code_mode.rs` **143/143**（run6 全崩）、app-server `code_mode_host` **18/18**（"could not locate binary" 家族）、guardian code-mode 系 **23/23**、`code-mode-runtime/host` crate 测试通过。无 host 时同批测试确定性失败（批 FI 已证）——根因与修复闭环。
- **CI 接线**：`fork-cargo-pr.yml` 三车道接入 `setup-rusty-v8`（ubuntu x86_64-unknown-linux-gnu / macos aarch64-apple-darwin / windows x86_64-pc-windows-msvc），解除 `codex-code-mode-runtime/-host/v8-poc` 三个排除（voice-host 排除不变，gstreamer 独立事项）；车道加 `codex-v8-poc/sandbox` feature——v8-poc 守卫测试要求 crate feature 与链接归档一致（车道 RUSTY_V8_ARCHIVE 强制 sandbox 归档；本地无归档默认关、用 denoland 普通归档，两环境自洽，6/6 验证）。YAML 结构校验通过。
- **边界**：dispatch 仍需授权（未 push）；Linux/Windows 工件未本机验证（target 不同），CI 首跑见分晓；voice-host 排除与其余失败类别不变。

### 批 FK — 栈假设数据点 + bwrap 能力校准工具（第三批第 3/4 项本机可做部分）

- **栈溢出 macOS 数据点**：`guardian_ephemeral_retry_preserves_parallel_trunk_and_fork_history`（run6 ubuntu SIGABRT / windows 0xc00000fd 的同一身份）在本机显式 4MiB 与 2MiB 栈下均通过（4MiB 1/1；2MiB 3/3）——溢出条件是平台/调度相关而非仅栈尺寸常数；Linux/Windows 显式 4MiB 复现保持开放（需对应平台）。
- **bwrap 能力校准**：新增 `scripts/bwrap_capability_check.sh`——按 codex 沙箱同一 unshare 标志集（user+ipc+pid / +net）分级探测，输出四类结论（全可用 / userns 被拒 / RTM_NEWADDR 边界即 hosted-runner 现状 / 意外失败），供 T32 的 runner 裁决在候选环境上一跑定型。bash -n 与非 Linux 优雅跳过已验证；Linux 判别路径待首个 Linux 环境实跑。

### 本轮_receipt（串行 threads 1，offline --locked，retries 0）

- 聚合 receipt（Codex 复审末批同一过滤器 + 新家族，fresh app-server helper binary 重建后）：**108/109**。唯一失败 `legacy_custom_ca_fallback_is_limited_to_reqwest_default` 为 `route_aware_client_pool` 子进程族（Codex 复审首批同样失败过、隔离即过的已知负载敏感族，批 A 登记 R7 机制：子进程默认 client 的 SCDynamicStore 读取 vs 2s fixture 预算）；本会话未改 http-client 产品代码；隔离复验 PASS。不以此标签关闭该族在 CI 的失败。
- codex-features 63/63；schemas（config + app-server）再生成并随批提交；`git diff --check` 干净。

## 40 项状态（仅列本轮有变化的项；其余沿用 results.md 表）

| 项 | 本轮变化 | 证据 | 仍开放 |
|---|---|---|---|
| T16 | cap 终局 usage 经类型化错误进入 core 账本并持久化（Chat+Responses 双 wire）；共享账目与 v2 tokenUsage 表面接通（批 FE） | 批 FA/FB/FE；585/585、256/256、13/13、28/28 | Chat 迟到 usage chunk 顺序（V-D2-1）；WS 传输 cap fixture |
| T17 | host sampling key 落地（mint/持久化/重建）；跨请求同 provider id 不别名有公共集成证明 | 批 FB；11/11 ledger+rollout 家族 | compaction 采样仍 legacy 键（None，行为不变）；app-server/读侧按 key 的展示 |
| T18 | 观测点有界采集 + 门控发射 + 双 mode 持久化 + resume；D1-c 边界全钉；crash 尾行 + 两次 resume + 物化矩阵（批 FD）；cap usage 进 v2 表面（批 FE） | 批 FC/FD/FE；21/21、39/39、28/28 | 模型可见注入（P0 门）；exec API 线程专用串联用例；Responses WS 传输的 cap fixture；export 无 RPC 可测 |
| T15 | 无变化（七项裁决仍为准） | — | — |
| 其余 T01–T14、T19–T40 | 无变化 | 见 results.md 及 codex-review §3 | 按 codex-review §3 主线推进 |

## 开放与下一步（依赖顺序）

1. T18 读侧收口：crash 后 rollout 半行、export/archive/delete 与两次 resume 矩阵；app-server v2 ThreadTokenUsage/completeness 投影 + rawResponseItem 兼容；exec/SDK/UI unknown 持久性（第一批第 7 项）。
2. 第二批（代理/生产默认）：第 1/2/3 项已落地（批 FF/FG/FH）；第 4 项迁移裁决材料就绪（`default-migration-adjudication-2026-10-11.md`），待用户裁决 A/B/C；冷/暖原预算与 runloop 因果实验仍待安静窗口。
3. 第三批（CI/栈）：827 逐身份已完成（批 FI）；**V8 配对工件已接通，macOS 样本支持修复机制（批 FJ）；567 个历史身份仍需新 CI 对照**——CI 三车道已解除 V8 排除并接入校验过的配对工件，提交推送后由 push 事件首跑验证 Linux/Windows target；手动 dispatch 单独授权。剩余：bwrap 109（runner 能力/T32——校准脚本已备 `scripts/bwrap_capability_check.sh`）、栈溢出 2 身份（macOS 4MiB/2MiB 均过，复现需 Linux/Windows）、assertion-open 79 + no-evidence 14 的 Linux 侧 triage、Windows pass+leak 311 卫生项、voice-host 的 gstreamer 供给。
4. 第四批：T21–T40 原范围。

本轮无 push、无 tag、无发布；全部为本地提交，按序：`46f2f87e3`、`243ddb5a1`、`78acb6df4`、`0ec9e1b04`(docs)、`9088aca8f`、`e4797d0dc`、`c1a5dd024`(docs) 为第一批；第二批为 `7922659b7`（wire 双轨）、`8d908c96d`（生命周期注入）、`c40172775`（跨版本 parity）、`17c4b28c5`(docs：迁移裁决材料)。

## 2026-10-12 新复审入口

请以 `codex-review-2026-10-12.md` 与 `claude-followup-2026-10-12.md` 为当前增量结论。此前 cap 工具“零执行”仅适用于未完成调用/桥层未发出的工具；native Responses 已收到完整 tool Done 时按既有调度执行，诊断记录不额外执行。完整 Done 后再收到 cap 的新测试原来使用未注册 shell，不能证明可执行工具被阻止。本轮修正该假绿断言，未改变 native 生产调度合同。
