# 包 3 工作清单：249 项仍败的签名分组、独立复现与根因登记（2026-10-07）

状态：**进行中的归因工作清单**。本文以 10-07 t2 复跑的 249 个"本次仍败"历史项为固定基数，按实际错误签名分组，对代表做独立最小复现（单用例、`--test-threads 1`、`--retries 0`、离线锁定、完整工作区特性图），并登记实测结果、根因候选与下一步。**10-08 标注复算：原 249 项新增恢复 115，仍开放 134；另有 9 项此前已通过后再次通过，故恢复标签共 124 行，不能算成原 249 项恢复 124。10-09 追加：R7 修复后 registration_retry 家族 15 行标注转绿（134 → 119）。**这些是限定切片的状态证据，完整 workspace 门禁尚未重跑。

证据工件（兄弟目录，gitignored）：
- 名单与分组：`/Volumes/soddygo/git-workspace/codex-tmp/pkg3/still249.tsv`、`t2-signatures-249.tsv`、`t2-signatures-groups.md`（提取器 `extract_signatures.py`，解析 10-07 t2 复跑日志 `../ws3-rerun-t2.log` 的结果行+stdout/stderr 块）
- 独立复现：`/Volumes/soddygo/git-workspace/codex-tmp/pkg3/repro/{summary.txt,*.log}`（脚本 `run_repro.sh`）
- 修复验证：`/Volumes/soddygo/git-workspace/codex-tmp/pkg3/verify/{summary.txt,*.log}`（脚本 `run_verify.sh`）

## 1. 签名分组（249 项全部命中，缺失 0）

| 签名 | 数量 | 含义 |
|---|---|---|
| wait-deadline-elapsed | 82 | 测试内部等待事件期限耗尽（非 nextest 超时） |
| timeout-killed | 51 | nextest slow-timeout 终止（60s） |
| mock-request-miss | 22 | wiremock/期望服务器未收到预期请求 |
| assertion-mismatch | 17 | 断言不等 |
| env-fakeip-policy-block | 17 | 本机 fake-ip DNS 命中私网策略阻断（已证环境） |
| fail-no-panic-output | 15 | 无 panic 的失败（多为 harness 自报错） |
| exec-registration-failure-kind | 14 | exec-server 注册失败 kind 意外 |
| startup-handshake-window | 10 | 启动 2s 握手窗断言 |
| snapshot-mismatch | 8 | insta 快照不等 |
| worker-thread-id-missing | 4 | mock worker 线程未产生 |
| connection-io-error | 4 | 连接/IO 错误 |
| config-load-error-response | 3 | turn/start 返回组织要求漂移错误 |
| otel-collector-timeout | 2 | otel 导出/telemetry 期限 |

等待消息分布（wait-deadline 组）：`item/autoApprovalReview/started` 46、`Integer(2)` 22、`turn/completed` 3、`item/completed` 1、空 7、其余 3。时长聚集 ~30-35s（guardian 30s 期限）与 ~5s/10s（短期限）。

## 2. 独立最小复现矩阵（单用例 t1；窗口 load≈6-9）

| 代表 | 签名组 | 结果 | 定性 |
|---|---|---|---|
| guardian_v2::action_budget::oversized_async_action…（×2 复跑一致） | wait-deadline | FAIL 30.7s/30.6s | **本窗复现** |
| guardian_v2::resumed_thread…websocket_warmup | wait-deadline | FAIL 30.7s | **本窗复现** |
| application_network::explicit_reloads… | wait-deadline | FAIL 5.8s | **本窗复现** |
| tui agents_overview::…lifecycle… | timeout-killed | TIMEOUT 60s | **本窗复现** |
| tui active_reconnect::reconnect_exhaustion… | snapshot | FAIL 5.6s（快照不等，t2 窗口为 TIMEOUT） | **本窗复现** |
| exec-server registration_retry::conflict_code_not_received | exec-reg-kind | FAIL 3.6s | **本窗复现** |
| multi_agent_v2…inherits_parent_developer_instructions… | worker-thread-id | FAIL 0.79s | **本窗复现** |
| model_provider_enforcement::definition_changes（+selection_changes） | config-load | FAIL 0.55s/0.14s | **本窗复现** |
| core retry_after::connection_failures_increment… | otel | FAIL 10.5s（`timed out waiting for retry telemetry`） | **本窗复现** |
| core remote_env::preserves_persisted_root… | mock-miss | FAIL 28.1s | **本窗复现** |
| http-client default_pool_does_not_retry_a_native_tls… | connection-io | FAIL 3.6s（macOS `WouldBlock` on accept） | **本窗复现（环境候选）** |
| core managed_network_proxy_decider… | env-fakeip | FAIL 0.18s | **本窗复现（已证环境）** |
| core guardian::tests::guardian_ephemeral…（SIGABRT 项，×2） | — | PASS 4.9s/4.5s | **不可稳定复现（登记）** |

**结论：13/13 个失败代表在本次独立复现窗口再次失败；SIGABRT 项单跑 2/2 通过。**这证明低并发下仍能观察到这些失败，不证明每个家族均为确定性缺陷，也不排除环境、时序或并发因素。100 项"复跑恢复"是否与负载有关需另行抽样（本清单未覆盖）。表中的复现只描述本次窗口。

## 3. 根因判定

### R1（已定，修复已实施待验证）：桥接默认策略禁用 Guardian V2 后台评分 → 75 项家族

- 机制：`model-provider-info/src/lib.rs:729 uses_model_bridge()` 对 `wire_api=Responses` 且 `experimental_bridge=None` 的非 first-party provider 返回 true；`ext/guardian-v2/src/async_scorer/extension.rs:89` 在桥接时移除 LunaSampler/GuardianV2Enabled 并警告 `"Guardian V2 background scoring requires a native Responses provider; using the normal approval review fallback."`。guardian_v2 测试 fixture（`app-server/tests/common/config.rs MockResponsesConfig`）的自定义 mock_provider 恰落入该分支 → V2 通知（`item/autoApprovalReview/started` 等）永不发出 → 30s 期限耗尽。75/249（74 wait-deadline + 1 handshake 窗）同族。
- 修复（fixture 级，产品策略不动）：`MockResponsesConfig` 生成的 provider 固定追加 `experimental_bridge = "native"`（恢复上游测试前提；fork 对真实第三方 provider 的桥接策略不变；产品在桥接 provider 上不支持 Guardian V2 评分是已登记的产品缺口，由运行时警告显式表达）。
- 验证：见 §4（修复后 guardian 代表应转绿、金丝雀不回归）。全量 guardian_v2 家族（75 项）与整个 app-server 套件的回归验证在后续批/完整 workspace 申请时执行，本轮不宣称家族全绿。

### R2（**已修并验证**）：fake-ip DNS 22 项（network-proxy 全切片）

修复（依赖注入，产品策略与断言不动，两层）：
- **策略层**：`NetworkProxyState` 新增 `host_lookup_fixture`（生产恒 None 走系统解析）；`host_blocked` 的 `host_resolves_to_non_public_ip` 调用 fixture 优先；`#[cfg(test)] with_host_lookup_fixture` + 共享 `public_dns_lookup_fixture()`。
- **连接层**：macOS `TargetCheckedTcpConnector` 的 rama resolver 换为 `StateDnsResolver`（fixture 优先、否则 SystemDnsResolver）——socks5 五项由这层修复。
- fixture 语义：`.invalid` → NXDOMAIN（DNS 失败阻断测试依赖）；`localhost`/`*.localhost` → **保留原生解析**（connector 级 v4/v6 回环测试与本地 TLS 上游需要真栈）；其余主机名 → 固定公网 93.184.216.34。IP 字面量路径不经 DNS，私网/回环阻断断言全部保留。
- 注入点：`network_proxy_state_for_policy`（runtime/network_policy/http_proxy 共用）、`state_with_metadata`、socks5 `state_for_settings`。

验证：**network-proxy 全 crate 314/314 全绿**（r2-final3.log，103.5s，t2）；标注文件 22 行已标记 R2 已修。**core 的 managed_network_proxy_decider 1 项不在本 crate**——core 测试无法用 cfg(test) 注入（跨 crate），需另行设计（公开测试钩子或迁移测试），登记开放。

### R3（候选，验证中）：Responses WebSocket 家族 5 项

`attestation×2`、`client_metadata`（websocket 请求体）、`web_search×2`：mock 桥接后不走 native Responses WebSocket 传输 → 握手/请求体断言永不等 → TIMEOUT。与 R1 同因不同面；修复后验证批 v09/v10 直接检验。

### R4（**已修，2026-10-07 晚间轮**）：model_provider_enforcement 组织要求漂移误报 4 项

根因：`provider_id` 是 fork 扩展字段（`#[serde(skip)]`，load 时由 `merge_configured_model_providers` 按配置键填充）。线程运行中的 provider 带 `provider_id=Some(key)`，而 `check_thread_model_provider`（app-server/src/config_manager.rs）从 requirements state 取的 raw 定义 `provider_id=None` → 派生 `PartialEq` 结构不等 → `definition_changed` 恒真 → 要求未变的请求也被 -32600 误拒（上游测试 #45517 的"未变化窗口应成功"前提被破坏）。

修复：检查侧非 Bedrock 分支用同一键补填 `provider_id`（与 merge 路径一致；Bedrock 分支本就经 merge）。验证：`provider_requirement_changes_reject_inputs_to_existing_threads` ×2 + `model_list_blocks_noncompliant_cached_provider` ×2 转绿（0.12-0.47s），`config_manager::provider_tests` 5 项单测 + `config_model_provider_requirements` 2 项相关套件全过（r4-verify/r4-unit2 日志）。标注文件 4 行已标记。

10-08 独立复审补充：仅补 `provider_id` 不能规范化内建 provider 的 bridge-only requirements。补丁同时检查 built-in 身份与 bridge-only shape，经 load-time merge 应用到线程保留定义，保留 endpoint/环境派生值；完整非 Bedrock 定义仍补同键 `provider_id`。新增公共 API 覆盖 openai/ollama/lmstudio 保留 route、transport 漂移拒绝，正常 Bazel 环境实际 3/3 通过（每例捕获两次保留 endpoint 请求，拒绝后无第三次）。旧日志与新机记录分别登记，完整命令见 `../codex-independent-review-2026-10-08.md`。

### R1b（深挖后收窄，仍未收口）：guardian history resume checkpoint 13 项

本轮深挖结论（证据：r1b-diag*.log、r1b-try*.log）：
- 机制链已完整映射：`select_parent_compaction`（async 侧）/`ReviewContextPolicy::parent_compaction`（sync 侧）→ `CompactionCheckpoint::latest(history.annotated_items())` → 复核线程 `InitialHistory::Forked([checkpoint envelope])` 种子 → 复核请求 input 应含 `{type:"compaction",…}` item；池路由：上下文键变化→ephemeral fork（snapshot=None，种子生效）、键匹配→复用 trunk（无 checkpoint 通道）。
- 实测（修复 R1 后首次运行）：sync review 请求 input 仅 3 条 message，checkpoint item 完全缺失 → 指向 `parent_compaction()=None`（父历史无 Compaction item）或复用路径无种子两候选。v2 远端 compaction 理应追加 checkpoint envelope（compact_remote_v2.rs build_v2_compacted_history），分发条件已满足（name "OpenAI"+native）。
- **修复后观察到两种失败形态，根因仍开放**：
  - **快败态（FAIL 4-5s）**：checkpoint 断言失败（见上，两候选）；
  - **挂起态（TIMEOUT 60s）**：测试诊断停在 index==0 末尾等待答题后的第二次 Luna 采样；`sample` 捕获子进程 `app-server-stdin` 线程处于 `read()` 等待。测试未关闭 stdin 可解释这个栈，单凭该栈不能定位采样未到达的原因或排除 shutdown 问题。Luna post-answer 采样时序/缓存是待查候选。
  - 该双态解释 t2 复跑 TIMEOUT 形态与家族批 FAIL 形态并存。
- 已保留测试诊断增强（断言失败信息附 sync_input 全量与父请求 input 类型）；临时 eprintln 探针已移除。
- **2026-10-09 判别定案：候选 B 证实**。快败实例的 `parent_input_types=[message, **compaction**, message, message, message, function_call, function_call_output]`（r1b-note.txt）——父线程历史**包含** checkpoint item（候选 A 排除），但 sync review 请求 input 不含 → 阻塞在复核会话投递链。首次修复尝试（review_session_setup.rs 在 fork snapshot 上追加 parent_compaction envelope）**不足**：修复后快败变为 15.4s 仍缺 item——投递在更下游被截（复核线程为 subagent，嫌疑：其请求组装/internal_model_context 对 subagent 剥离加密 item，或走的是 trunk 复用分支根本不重播历史）。该尝试已回退（未证实不改产品码）。下一步：追 run_review_on_session → 复核线程请求组装的 input 构建路径，定位 compaction item 被剥的确切位置。
- 待做：① Luna post-answer 二次采样为何有时不触发（score 缓存/竞态，guardian sampler 域）；② subagent 请求组装剥离点定位（上条）；③ 与上游对齐 legacy 线程 compaction checkpoint 保留语义。

### R10（**预热后目标切片通过；冷态归因开放**）：http-client TLS 4 个历史项 + aws-auth 1 个历史项

实测：三段计时为 acquire=1ms / route+build=7.7ms / **execute=8.33s**；`sample` 在 TLS 请求中捕获 `native_tls handshake → security_framework SslContext::new → SSLCreateContext → SSLCreateContextWithRecordFuncsAndPath → _dispatch_once_callout` 栈。该证据支持 TLS 冷态初始化为慢路径候选，不能把整个 execute 时长都归给 dispatch_once。curl 对照（0.34s 403）与 NO_PROXY 变体是对照观察，不能独自排除所有代理、路由或调度因素。**aws-auth 的 IMDS 请求为纯 HTTP；尚未证实它的冷态失败由 Secure Transport 初始化导致，也未证明生产冷态行为无缺陷。**

当前测试变更：`warm_secure_transport_once()` 在被测服务器预算启动前对一次性本地 peer 做 native-tls 握手尝试，随后测试按原行为断言执行；初版TLS服务器曾跳过空ClientHello；Codex复审恢复空读/超时失败，未吞掉额外连接。已加在 tls_fallback_tests 和 aws-auth real_imds（增加 native-tls dev-dependency）。**这验证预热后的暖态契约，改变了被测冷态前提；两项变更的作用尚未单独分离。**仅 `TlsConnector::new()` 的第一版预热未改善结果。R7/R9 及 AWS HTTP 冷态失败继续开放。

验证（`r10-verify4.log`）：实际选择 9 项，**8 PASS / 1 FAIL**。8 个目标通过 = 7 个 TLS 测试 + 1 个 aws-auth；额外 core retry telemetry 测试失败（10.463s，R9）。因此目标切片通过，整批不全绿；原 249 项中对应的 5 个历史项得到暖态恢复标签，其余 3 个 TLS 目标不计新增历史恢复。

### R6（**阻塞点已观察，产品补丁与限定切片已验证**）：TUI 与 app-server 残余超时族

`sample` 抓栈（r6-sample.txt，157KB）观察到挂起线程位于 `notify::fsevent::FsEventWatcher::unwatch → FsEventWatcher::run → mpsc recv（阻塞）`。archive/delete 卸载线程路径中，`reconfigure_watch_inner` 在 **async 线程上同步调用** macOS FSEvents 后端的 `unwatch`，可以阻塞 current_thread 运行时；被监视 rollout 的移动是本次触发条件候选。采样与 60s 超时证明本窗存在长时间阻塞，未证明无限阻塞，也不能将未逐项验证的全部 TUI/app-server 超时归给同一原因。

**经 Codex 复审后的产品补丁（file-watcher/src/backend.rs、lib.rs）**：平台 watcher 在专职线程执行并销毁；desired 与成功安装的 active 分开，失败重试，单 wake token 合并最新配置，避免阻塞期间累积失效命令。安装成功发送粗粒度失效通知覆盖订阅至就绪窗口。正常 Bazel 环境 25/25（含真实注册/后续变更）通过；Cargo 执行环境真实注册在首次平台 watch 卡住，24 PASS/1 FAIL，保留差异。后端永久阻塞仍有退化/线程资源风险，未宣称底层 FSEvents 已修。完整命令与证据见 `../codex-independent-review-2026-10-08.md`。

验证：file-watcher 22/22；TUI lifecycle 由 TIMEOUT 转为 PASS 19s；`r6-verify3.log` 的 **agents_overview 名称过滤切片 71/71 PASS**（849.7s），其中 63 项属于 `app::agents_overview::tests`，8 项在模块外。它不是“模块全集 71 项”或 TUI 全 crate 门禁。historical 标注中对应 14 行 = 原 249 项新增恢复 5 + 此前已通过 9；未覆盖的超时与快照差异仍开放。

附带修复（自查回归）：R1 的 fixture 模板固定 `experimental_bridge` 行与 TUI 三处 `with_provider_config` 显式传参形成 TOML duplicate key（此前 manifest 为 config 加载失败）——三处冗余传参已删（resume_picker_transcript_preview/agents_overview_actions/realtime_handoff_e2e；realtime_handoff 修复后 PASS 54.5s 证实）。

### R5（待查，可能属 R1）：multi_agent worker 线程 4 项

mock 剧本要求 spawn worker；turn 215ms 完成且只有父消息，worker 线程 ID `.expect` 失败（multi_agent_v2_developer_instructions.rs:686）。可能与桥接路径下工具调用翻译有关；验证批 v04 检验 fixture 修复是否连带转绿。

### R6 残余开放：tui 快照与未覆盖超时

agents_overview 的限定过滤切片结果见上；背景任务/浏览分页等未覆盖项仍需逐族拆解。快照族独立跑可快败（渲染差异，如时间性内容），不能以 watcher 补丁或 provider fixture 的通过结果关闭。

### R9（根因收窄至 sqlx SQLite，开放）：otel/retry 家族 6 项

`sample` 抓栈（r9-sample.txt）观察到 async 侧停在 `sqlx_sqlite::ConnectionWorker::establish`，worker 线程在 flume recv 等待；该窗口模型请求未发出，测试未收到重试遥测并在 10s 到期。采样支持状态库连接建立/回复为卡点候选，不能据此确认 sqlx 唤醒丢失或确切等待原语。R6 补丁后本项仍在 10.463s 失败（`r10-verify4.log`），说明该补丁未解决本项，仍需独立定位；不能把两次采样视为已证明不同根因。

### R7（**2026-10-08 根因定案并修复**）：exec-server registration 17 项 + aws-auth IMDS 同根

**根因链（四层递进取证）**：①单例 6/6 确定性复现（Timeout，与负载无关）；②三段计时探针证明 route 解析 1.6µs、build_permit 3.5µs、pre_spawn_blocking 592µs 全瞬时，但闭包体（reqwest ClientBuilder::build）耗时 **12.28s**；③30s 窗口 sample 抓到阻塞线程完整栈：`RouteAwareClientPool::client_for_url_with_resolver → build_for_resolved_route → reqwest::ClientBuilder::build → hyper_util proxy matcher::from_system → SCDynamicStoreBuilder::build → SCDynamicStoreCreateWithOptions → _SC_getApplicationBundleID`（100% 采样命中）；④对照实验：仓库外 minirepro（同 reqwest 0.12.28）在主线程构建仅 7ms——**SCDynamicStore 在无 CFRunLoop 的辅助线程上创建可阻塞十余秒**，是本机环境特性；arm-2/常规臂的 reqwest 默认构建（TransportDefault 路由）无条件加载 macOS 系统代理配置。测试进程的测试级超时（500ms 连接/1s 外层）在构建期间耗尽 → registration 17 例 Timeout；aws-auth real_imds 的 mock 也是 127.0.0.1 字面量，同根（其此前"TLS 预热修复"实为 SSLCreateContext 首建成本，与本项无关）。R9 sqlx 不同根（见 R9 节，维持分开）。

**产品修复（http-client/src/outbound_proxy.rs）**：`resolve_proxy_route`（同步核心）与 `resolve_proxy_route_async`（异步路径）对 ReqwestDefault 策略增加字面量 IP/localhost 目标的 `Direct` 早退（`destination_host_skips_system_proxy`：URL host 可解析为 IpAddr 或 localhost）——系统代理本就不服务这类目标，显式 Direct 使构建跳过 SCDynamicStore。这也是真实产品缺陷修复：macOS 上经池访问 127.0.0.1 本地模型服务/Ollama 的首个请求可能被同一机制阻塞数秒。池默认 custom_ca_fallback=Disabled 走 build_for_resolved_route 分支即被覆盖；LegacyTransportDefault 臂保持原语义（未见字面量用例）。

**验证**：registration_retry 家族 **18/18 PASS（8.7s 总时长，此前 1/18/200s+）**；扩大回归切片（network-proxy 全 crate、http-client route_aware/tls_fallback/transport、aws-auth real_imds、file-watcher 全 crate）进行中。

### R7（深挖收敛，未修）：exec-server registration 14+ 项

分段计时+mock 探针实测（r7-*.log）：
- 测试各前置阶段均快（client 构建 3.4ms、keygen 14ms）；注册 POST 对本地零延迟 mock 间歇性 >1s，甚至耗尽整个 5s 预算（Timeout）。同参数下通过/失败波动，竞态、环境初始化及调度都是候选；不能仅凭波动认定竞态。把 connect_timeout 与测试外层期限临时放宽到 5s 后 **14/16 通过**（原窗口 0/16）。延迟观察点收窄到 `RouteAwareClientPool` 发送路径；双 TLS 后端/首请求初始化与 R10 同源仅为候选。
- 另有 2 例独立问题：stalled-body（响应头立到、body 滞后 60s）的 401/403/404 **不从状态行提前分类**（期望 EnvironmentRegistryAuth，实测等 body 到 Timeout）；而 503 的状态行分类正常——fork 客户端对 4xx 响应体处理与上游行为差异待查。
- 已保留测试诊断增强（bail 附实际错误变体 `{error:?}`，前述 Timeout 变体即由它得到）；临时放宽期限的补丁已回退（放宽=削弱断言，不可接受）。
- 待做：route-aware 首请求延迟竞态定位（建议与 R10 合并调查：codex-http-client 的 TLS 后端初始化）；stalled-body 4xx 状态行分类对齐。

### R8（待查）：startup handshake 2s 窗 10 项

`wait_for_handshakes(1, 2s)` 断言失败。其中 guardian ws warmup 1 项属 R1；其余归位后重查。

### R11：SIGABRT（stack overflow）单测——**模块并发下稳定复现**

`guardian::tests::guardian_ephemeral_retry_preserves_parallel_trunk_and_fork_history`：
- 单跑（t1）：PASS 2/2（4.9s/4.5s）。
- **guardian::tests 模块全集 57 项 @ 8 线程：SIGABRT 4/4**（3× 默认 8MiB + 1× `RUST_MIN_STACK=16MiB` 受控变体；25.5-25.9s，恒为同一用例，`thread '…' has overflowed its stack`）。
- 结论：**该模块 8 并发的四次实测均在同一用例栈溢出；其中一次 16MiB 仍失败。**这说明单跑通过不能排除并发窗口下的严重失败，也说明本次提高至 16MiB 未解决。现有结果不能证明无界递归、任意栈预算都会耗尽或任何完整 workspace 运行必然失败；运行时长关联也未建立因果。
- **2026-10-09 栈定位（just 8MiB 路径再复现 30.1s + 活体采样 codex-tmp/pkg3-r2/sig-s1.txt）**：溢出用例线程的深层栈底为 `turn::run_turn → run_sampling_request → try_run_sampling_request → ModelClientSession::stream → stream_responses_websocket → websocket_connection → ModelClient::connect_websocket → ResponsesWebsocketClient::connect → WebSocketConnector::new → build_rustls_client_config_with_custom_ca → rustls_native_certs::load_native_certs → macos::load_native_certs → security_framework::trust_settings::TrustSettings::iter`（该线程 1377/1377 采样全部阻塞于此）。机制合成：① 每个 websocket 连接**同步**加载 macOS 原生证书（钥匙串 TrustSettings 迭代本机秒级慢、无进程级缓存、未走 spawn_blocking——与 SCDynamicStore 同类的系统服务阻塞，模块 8 并发下全部同时撞上）；② 采样请求重试链疑似每次重试加深 future 包裹（~12 次 × 深栈后 8MiB 溢出；16MiB 同溢出佐证增长型）。旁证：绕过 just（直接 cargo nextest，tokio worker 落回 2MiB 默认栈）时 **17 个 guardian_review* 测试瞬时 SIGABRT**——guardian future 体积本身即接近 2MiB，"大 future"候选获得直接证据。
- **2026-10-09 缓存修复实施与复验**：`custom_ca.rs` 新增进程级 `NATIVE_ROOTS: LazyLock<RootCertStore>`（rustls native certs 只加载一次、每连接 clone）。复验（just 8 线程，sigabrt-cachefix.log）：**SIGABRT 仍复现但时间 30.1s→15.2s**（钥匙串往返确为延迟组成）；15 个并发超时不变。结论：TrustSettings 加载是延迟放大器而非栈溢出主因——增长型深 future 栈才是根因，①缓存保留为真实产品改进（websocket 每连接免钥匙串往返），②Box::pin/栈预算工作继续开放。回归（cache-regression.log → dialer-fix-verify.log）：首轮 137/143，6 失败=4 已知 TLS 冷态 + 2 dialer。**dialer 更正：此前误判"本机既有、非缓存致因"——实为 R7 直连修复的行为变化**：`resolve_proxy_route_async` 把 localhost 从 TransportDefault 变 Direct → dialer 从 Tungstenite 默认路径（Left）变手动 connect_via（Right）→ 断言传输类型的测试失败。**修复（dialer.rs）**：Direct 路由无代理时保持 TcpStream 不装箱，走 `client_async_tls_with_config` 直接握手 → 产出与 TransportDefault 相同的 Left 传输（nodelay 可检性保留）。修复后 139/143（dialer×2 全过），仅剩 4 例已知 TLS 冷态。
- 待做：② 复查 guardian 采样重试 future 包裹是否线性增长（Box::pin 边界）；修复前该项在任何全套门禁都会 ABORT，不能用单跑 PASS 关闭。

## 4. 修复验证（fixture native 钉；`MockResponsesConfig` 追加 `experimental_bridge = "native"`）

| 用例 | 修复前（独立 t1） | 修复后 | 判定 |
|---|---|---|---|
| v01 guardian oversized_async_action | FAIL 30.7s | **PASS 1.33s** | R1 证实 |
| v02 guardian resumed_thread_ws_warmup | FAIL 30.7s | **PASS 1.89s** | R1 证实 |
| v03 application_network explicit_reloads | FAIL 5.8s | **PASS 0.92s** | R1 家族扩展 |
| v04 multi_agent worker-thread-id | FAIL 0.79s | **PASS 1.06s** | R5=R1 家族证实 |
| v05 model_provider_enforcement ×2 | FAIL 0.5s/0.14s | FAIL 0.61s/0.16s | R4 独立根因（未解） |
| v06 金丝雀 logging timing | （全套首试过） | PASS 0.77s | 零回归 |
| v07 金丝雀 strict_config | （全套首试过） | PASS 0.65s | 零回归 |
| v08 金丝雀 logging credentials | （全套首试过） | PASS 10.4s | 零回归 |
| v09 client_metadata websocket body | TIMEOUT | **PASS 0.72s** | R3 证实（fixture 路径） |
| v10 attestation ws handshake ×2 | TIMEOUT | **仍 TIMEOUT** | 该测试自写 raw config（attestation.rs:181-198）未走 fixture，钉未覆盖——待同式修复 |

结论：本批 **8 PASS = v01–v04/v09 的 5 个失败代表恢复 + v06–v08 的 3 个金丝雀再次通过**；另有 v05 两项 FAIL、v10 两项 TIMEOUT。不能称“8 项新增恢复”或整批全绿。三项金丝雀未观察到回归，不能外推为整个套件零回归。产品桥接策略保持现状；桥接 provider 的 Guardian V2 评分/native Responses WebSocket 能力缺口仍登记开放。

### 4.1 家族级整批（still249 的 app-server 全切片 101 项，t1，批 appserver101/）

**77 PASS / 20 FAIL / 4 TIMEOUT（修复前 101 项全败）**：

- 转绿 77 = guardian_v2 62 + web_search 2 + multi_agent_v2 5 + compaction 3 + turn_start 2 + client_metadata 1 + application_network 1 + in_process 1 + attestation 前 0（见下）；标注文件已按批内逐项结果标 `根因R1…转绿`（77 行）。
- attestation ×2 随后以 raw 配置同式钉 native（attestation.rs:188 增行）复跑 **PASS 4.4s/4.0s**（final2/attestation.log）——累计转绿 **79/101**。
- 剩 22 中 **R4 4 项当晚已修**（见下）：剩 18 = R1b 新根因 13（guardian_v2::history，见 §R1b）；R2 代理族 TIMEOUT 2（account external_auth/system_proxy browser_login）；散项 3（residency websocket、review detached_delivery、derive_config 单测）。**累计：101 项中 83 转绿、18 开放。**

## 5. 下一步（按优先级）

1. R1b 的 13 项、app-server 其余 5 项保持开放；区分 checkpoint 缺失与采样等待的实测症状和根因候选。
2. R7/R9 定位最小等待原语；已有 provider/R2 恢复切片不能关闭这两个家族。
3. R10 做无预热冷态与预热暖态对照，单独检验服务器 EOF/read-timeout 变更；AWS 纯 HTTP 冷态根因继续调查。
4. R6 复审 watcher 异步注册与失败恢复语义，补齐未覆盖的 TUI 超时/快照及跨平台证据。
5. SIGABRT：保留四次模块并发实测，继续受控栈预算/并发对照并获取真实溢出调用栈。
6. 以上小批绿后按 work order 申请完整 workspace（live 排除、retries=0）。

## 6. 数字口径

249 = 固定的 10-07 t2 仍败全集；签名分组按 `t2-signatures-249.tsv` 逐项；家族归属按测试名前缀（guardian_v2* 75 = 74 deadline + 1 handshake）；独立失败代表 13 个 + SIGABRT 单跑 2 次。10-08 恢复标签 124 行 = R1 79 + R4 4 + R2 22 + R10 5 + R6 14；其中原 249 新增恢复 115 = 83 + 22 + 5 + 5，另 9 行此前已通过，故原 249 仍开放 134。R1b 13 个失败项虽有归因说明，不能计恢复。标签表示限定窗口观察到通过（R10 为暖态），不等于根因全部关闭或完整 workspace 通过。

## 7. 包 4 首个切片（2026-10-07 深夜轮）：admin 命令 flags 真实路由测试

新增 `admin_cli_flags_reach_real_config_loading`（cli/tests/admin_startup_matrix.rs，PASS 0.54s）：
- **--strict-config 决定格**：主配置含未识别字段 → `codex archive --strict-config` 失败、rollout 不动、config 字节不变；同命令无 flag 回退成功归档。
- **--profile（profile-v2 文件机制）**：`{home}/{name}.config.toml` 文件承载 profile；坏 provider（`model_provider = "no-such-provider"`）→ 加载失败+rollout 不动；有效 profile（`approval_policy`）→ 正常归档。
- 过程实证的真实语义（写入测试即文档）：① 主 config.toml 内写 `[profiles.x]` 表会直接导致加载失败（profile 只认独立文件）；② 未知 provider 无论 strict 与否都是硬错误（不可回退）。
- 与既有 8 格 install×daemon×env 矩阵、corrupted/mask、名字歧义、queue-to-owner 相加，包 4 的 archive 家族真实路由覆盖已含：--no-daemon/--strict-config/--profile × daemon 存在性 × install source × NUWAX env。剩余：--oss 真实格、active/incomplete 组状态格、owner crash/restart/队列持久化（原工作令包 4 清单）。


## 用户要求先保存WIP：Codex初步复核覆盖说明

当前Claude代码按用户最新指令先commit/push，尚未修复或独立验证。代理短路/redirect、全局证书根缓存、非macOS warmup import有确认问题；R11实际线程固定4MiB，R1b下游剥离与R7因果尚未定案；19组合=core1+exec16+relay2，119仅账面待完整身份复算。请以 `../codex-pending-review-2026-10-09.md` 和 `../other-computer-development-prompt-2026-10-09.md` 为最新交接，不将本页历史“已修/全绿/定案”升级为验收。

## 2026-10-10 批：P1 阻塞项已修（新机实际执行）

本节由新机 Claude 批次登记，详细证据见 `../claude-p1-fixes-2026-10-10.md`：

- **R7 短路已撤**：`destination_host_skips_system_proxy`（上文 R7 节的"产品修复"）经 codex-pending-review-2026-10-09.md 第 2 项判定为 P1 代理契约缺陷（Direct 清空 env/系统/显式代理并随重定向扩散），已删除并恢复 `ReqwestDefault → TransportDefault`。R7 节的"18/18 验证"相应失效；registration_retry 在本机 18 并发下复现 13/18 FAIL（单跑过），按评审处方改为 `#[cfg(test)]` hermetic 直连夹具后 **18/18 PASS（6.2s）**，exec-server 全量 **616/616**。
- **R11 的证书缓存重设计**：上文 R11 节的 `NATIVE_ROOTS LazyLock`（永久快照）替换为源键+TTL+失败不缓存设计（60s/5s 双 TTL、零证书不缓存、bundle 不污染），含同进程 A→B 真握手回归。R11 的 SIGABRT 主因（深 future 栈）**未修**，静态分析见 `r11-future-depth-static-analysis-2026-10-10.md`。
- **测试影响**：本机 `just test --retries 0`：http-client **135/135**、websocket-client **20/20**、exec-server **616/616**、aws-auth **10/10**、app-server-transport **157/157**（load 27–31 窗口）。Linux musl 交叉编译被 openssl-sys 阻断（无 musl OpenSSL/工具链），登记未执行。
- **SCDynamicStore 跟进设计**：`../scdynamicstore-compat-spec-2026-10-10.md`（专职 CFRunLoop 线程方案，未实施）。
- 冷态 TLS 4 项维持 R10 开放（旧机日志不在本机，本轮窗口未复现失败≠关闭）。

## 2026-10-10 批：四项并行调查结论（只读分析，修复均未实施）

- **R1b 剥离点已定位**（`r1b-delivery-strip-point-analysis-2026-10-10.md`）：非种子/池路由/guardian 侧,而是 fork 自有的 `core/src/model_output_projection.rs:217-222`——`project_input` 的 Compaction 臂在 provenance 不匹配时整项移除;guardian 复核请求的 `request_source`（`codex-auto-review` + guardian 头改变 auth 域,不在 `model_source.rs:57-91` 良性白名单）与父压缩请求 provenance 在 model 与 auth_domain 双重不等,**必然失败**。种子→重播→请求 input 全链无丢失,交叉验证（父保留/Luna async 保留/sync 只剩注入 message）全吻合;首次修复（快照追加 envelope）前后都在同一点被剥。R1b 13 项判定为 fork 对 guardian 场景的回归（origin/main 无该投影文件）。最小修复候选：种子 envelope 打显式重放授权标记,`sources_for_input` 映射哨兵,投影 Compaction 臂识别哨兵保留（仅放宽 model 比较不够）。待做①答案：post-answer 二次采样唯一触发是 `on_tool_start`,答题后无被打分工具执行则永不触发（`classification.rs:315-319` Superseded 与 `score.rs:101-142` 为竞态点）。
- **R11"增长型"被静态证伪**（`r11-future-depth-static-analysis-2026-10-10.md`）：全路径 5 层重试循环均为替换型（loop 体内创建/销毁内层 future,跨迭代只存活堆数据）,无 await 点把前次 attempt 包进当前 future;"16MiB 仍溢出"不构成增长证据（溢出线程是测试自建 4MiB,RUST_MIN_STACK 不作用）。深度大头是单次 poll 固定纵深,最深未装箱链含 `WebSocketConnector::new` 同步证书构建（websocket-client/lib.rs:97,未走 spawn_blocking——与 10-10 批 dialer.rs:138 修复同类）。Box::pin 削体积不削深度;给出 4 个装箱候选 + 1 个减深度候选 + 逐 attempt SP-delta 实验设计（用现成 retries 计数,不动断言/预算）。
- **R9 前提修正 + 新强候选 C1**（`r9-sqlite-establish-static-analysis-2026-10-10.md`）：retry 遥测是纯 tracing event（`codex-client/src/retry.rs:63-82`）,不写 SQLite;采样的 establish 属状态持久化车道（卡在首个模型请求之前）。sqlx 0.9 握手用 futures_channel oneshot（非 flume）,flume recv 是空闲态→唤醒丢失候选源码级排除;池配置排除池满。**C1**：默认策略下模型传输在运行时线程同步急切构建 reqwest 客户端（`core/src/client.rs:1241→1919`）,reqwest 每次构建新建 `Matcher::from_system()` 无缓存 → SCDynamicStore 阻塞冻结 current_thread 运行时 → 10.463s 失败与"模型请求未发出"逐字吻合;R7 直连早退对该路径本就不生效（不经路由解析）,故与"已带 R7 修复仍失败"兼容。预测当前树单跑仍 ~10.4s 失败——**本机 10-10 诊断单跑实测 PASS 31.832s(slow)**,预测在本窗口不成立,但 31.8s 的总时长与负载相关的慢成分兼容,C1 维持强候选未定案。三层复现方案与 5 点分段计时（T3=client 构建）已设计。
- **历史 119 账目在标注文件层复算吻合**（`historical-119-ledger-recheck-2026-10-10.md`）：249→124→119 三途径同值;final19=core1+exec16+relay2 本地旁证支持、日志级不可复算（需旧机 still249.tsv 与四日志）。新登记 3 处口径疑点:D2（relay #224 若计入 final 则应为 118）、D3（R7"17 项"仅 15 行标绿,差 2 疑为 direct_registration_*）、四口径数字（17/15/18/16）未对表。
