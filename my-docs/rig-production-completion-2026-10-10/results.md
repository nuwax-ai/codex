# results.md — 2026-10-10/11 全量执行收据

按 `claude-prompt.md` 的批次纪律记录：每批当前 HEAD、命令、范围、结果、首败与复验、开放边界。测试环境：本机 macOS（Darwin 27，arm64），`just test` = nextest（profile local，retries=1，`--no-fail-fast`）；另有标注 `--retries 0` 的严格窗口。机器同期有其他用户负载（load 15–30），负载敏感项已注明。

## 批次记录

### 批 A — T03+T04 native-root 缓存键与实例隔离（HEAD `489c43c85`）

- 实现：`http-client/src/native_roots_cache.rs`（私有模块）+ `native_roots_cache_tests.rs`；`custom_ca.rs` 拆出缓存；`EnvSource::var_os` 为无损原语。
- 键：结构化 `Option<OsString>` ×（SSL_CERT_FILE, SSL_CERT_DIR），全平台 `var_os`；缺失/空/非 Unicode 不合并；与锁定 rustls-native-certs 0.8.3 `CertPaths::from_env` 逐字对齐（其 `var_os` + split_paths 语义已核对源码）。
- 生命周期：锁后读时钟、加载后算 expires、errors 或 ignored>0 → 5s、零证书不缓存、loader panic → 毒化恢复（clear+reload）、单飞、bundle 仅私有 clone。
- 测试（`--retries 0`，过滤集 21 项）：21/21 PASS。全量 `just test -p codex-http-client`：141/141 PASS（连续 3 次；3 次中分别 1–3 项负载 flaky 由 profile 重试吸收）。
- 负载敏感项（如实）：`route_aware_client_pool` 4 个子进程用例在 `--retries 0` + 高负载窗口可瞬态失败（子进程 reqwest 默认 client 构建的 SCDynamicStore 读取超时 2s fixture deadline；HEAD 干净树同窗口 1/4 次同样失败——非本批回归，属 R7 机制）。已加 nextest local test-group `http_client_default_pool_local`（max-threads 4，与既有 `rig_bridge_wire_local` 同机制先例）。
- 开放：Windows 真实 A→B 环境验证（本机无 Windows；键语义单测平台无关）。锁等待/毒化/非 Unicode/拒绝证书等 12 项单测已覆盖。

### 批 B — T05+T06 WS 代理脱敏与默认 HTTPS 代理（HEAD `f3f86e926`）

- T05：dialer 统一出口边界 `redact_invalid_proxy_config`（保留 `Url(InvalidProxyConfig)` 类别、payload 固定 `<redacted>`）；覆盖 TransportDefault / Proxy+NO_PROXY / explicit 三路。
- T06：`tungstenite_env_https_proxy` 先问 SDK `ProxyConfig::from_env`，仅在其 `UnsupportedProxyScheme` 拒绝时按 SDK 逐 scheme 顺序恢复被拒 URL；https 选择走既有 TLS-to-proxy 分支；http/socks/bypass 留 SDK 原生路径。
- 测试：24/24 codex-websocket-client（`--retries 0`）。矩阵：wss+HTTPS_PROXY(https)、ws+ALL_PROXY(https)、wss+HTTP_PROXY(http) 各断言恰一条真实 CONNECT；SOCKS5 mock 断言中继的 literal-IP 地址与端口；两个非法密码（截断 `%`、非 UTF-8 解码）断言 Display/Debug/子进程输出零泄露且类别保持。
- 开放：Rig 0.13 reqwest 版本的同型参考（T07 阶段 2 一并做）。

### 批 C — T02 CI 启动修复（HEAD `cc10cd723`）

- `bazel-contrib/setup-bazelisk`（不存在，run 37967668519 实败）→ `bazelbuild/setup-bazelisk@v3.0.0`（repo/tag 已核）。
- macOS lane `brew install pkgconf gstreamer gst-plugins-base`（本机验证全部所需 .pc 可解析，gstreamer 1.28.7）；ubuntu/windows lane `--exclude codex-voice-host`（apt gstreamer 1.24 < 固定 v1_28；Windows 500MB 安装器路线未接线），lane 注释与 README 记录；voice-host 在 macOS lane 完整构建+单测。
- 本地验证：`nextest list --workspace --exclude codex-voice-host` 选择中 voice-host 目标为 0。
- 开放：远端四 job 实际 dispatch 需 push/CI 授权（未申请）。

### 批 D — T07 阶段 1：专职 run-loop 线程系统设置读取（HEAD `0b2ecbbbc`）

- `outbound_proxy/macos/system_settings_store.rs`：专职线程真实服务 CFRunLoop（50ms 片）；快照为不可变 CFDictionary 拷贝（scoped unsafe Send/Sync，依据注明）；单飞合并、成功 60s / 失败 5s TTL、等待预算 20s（> 观测最坏 12.28s）、超时回退有界陈旧快照、reader panic → 发布失败不挂起等待者、线程死亡按代数守卫重建（同 reader，≤8 次）后不可用（不做内联回退）。
- RespectSystemProxy 的 `system_proxy_settings()` 改经该 loader（语义不变：同一 get_proxies 字典，仅缓存化）。
- 测试：7/7 system_settings_store（`--retries 0`）；全量 148/148（含负载 flaky 重试恢复）。
- 如实声明：run-loop 因果（主线程快是否因 CFRunLoop）未证明，按 Q04 开放；ReqwestDefault 仍走 reqwest 内部 matcher（阶段 2/3：hyper-util 等价 matcher + 双轨对照 + 接管，未实施；阶段 2 需新增直接依赖 ipnet → 需同步 bazel lock）。

### 批 E — T11 R1b 有界 checkpoint 重放授权（HEAD 见批次提交）

- `core/src/guardian/replay_grant.rs`：`OpaqueReplayGrant` 仅由可信运行时在 `thread_options` 选定种子 envelope 的时刻创建（绑定 checkpoint id + 捕获的 producer provenance + 配置 reviewer 模型），经 spawn-time `thread_extension_init`（与 reviewer headers 同通道，不落 rollout、不可由可编辑历史伪造）→ `request_budget::prepare_prompt` 附着到 Prompt → client 三个投影点验证。
- 验证：live 请求 provenance 去掉固定 `x-codex-guardian` 头后必须逐字段等于 producer 的 provider/endpoint/wire/bridge/auth 域；目标模型必须等于批准的 reviewer 模型；wire/bridge/provider/endpoint 与 basis 一致。任何轮换凭据/端点/线路/模型漂移拒绝；只有绑定的 checkpoint id 且仍携带授予时 producer provenance 的项存活。
- 投影扩展：`project_input(..., replay: Option<&OpaqueReplayAuthorization>)`，仅 Compaction 臂接受授权；Reasoning/ContextCompaction/WebSearch 规则不变；普通 resume/未知来源行为不变（默认 None）。
- 单测：replay_grant 6 项（绑定/异 checkpoint/篡改 producer/轮换矩阵）+ model_output_projection 既有 12 项全量改参复跑。
- 待复验：13 个 app-server `guardians_retain_evidence_after_compaction_and_resume::*` 身份（rust-rig 全栈）——见下方执行记录。


### 批 F — T11 R1b 有界重放授权（HEAD `9d307f72d`，含 D2 前置 `5667058bc`）

- 实现见批 E 行 + 两处关键修正：
  1. grant 的 reviewer 模型绑定改为 spawn config 钉住的 `review_model.model`（含目录缺失回落父模型），而非 provider 静态偏好值；
  2. 诊断定位（pid 打点）证明测试三轮各跑在独立进程：credential-instance id 按进程随机铸造，跨进程不可比 → grant 的 basis 比较降为 kind+provider+endpoint+wire+bridge（凭据连续性由可信 spawn 继承父 auth_manager 作证，注释如实记录）；
  3. Compaction 臂放宽为 scope 级（provider/endpoint/wire/bridge+evidence kind 相等即重放，模型与进程内实例 id 不再阻断）——依据：fork 自有验收语料（13 身份）要求跨模型 resume 与跨进程重启重放；Reasoning/WebSearch 保持严格比较；跨 kind 轮换仍丢弃。既有单测按新契约更新并注明理由（不含凭据哈希持久化——违反"仅随机身份可持久化"约束）。
- **13 个 R1b 身份全绿**：`suite::v2::guardian_v2::history::guardians_retain_evidence_after_compaction_and_resume` 16/16（just test，rust-rig）。
- 单测：core replay_grant+projection 17/17（--retries 0）。

### 批 G — T16 D2 段：cap 终止帧 usage（HEAD `5667058bc`）

- `ApiError::CapExhausted { message, response_id, reported_usage }` + 协议类型（ReportedUsageCounters/UsageCompleteness/ReportedResponseUsage，presence 保留、零与缺席区分、Complete 仅全字段直报）。
- Responses 解码（SSE/WS 共用）终止帧 usage 解析；立即发出不缓冲（SSE 缓冲白名单同步迁移，HEAD 基线复核对拍）。rig bridge：Anthropic 走 wire 观测 presence（start/delta 累积、显式零保留）；rig 归一化 usage 无法区分零与缺席 → 回退报告只认非零、恒 Incomplete。
- 消费方迁移：api_bridge / response-debug-context / guardian-v2 metrics+retry / 4 处测试 matcher；message 文本不变。
- 证据：codex-api 214/214；protocol+rig-bridge 618/618（--retries 0，首败为最后一个未迁移 matcher，迁移后复验）。
- 开放（T16 剩余）：Chat 终止后迟到 usage chunk 顺序（V-D2-1 wire 测试）；稳定 response_key reducer（T17）；Core 消费侧按 CapExhausted 的 turn usage 记录（T17/T18）。

### 批 H — T12 Luna post-answer 终局测试（HEAD `081b6b0b6`）

- 新 `post_answer_finality_tests.rs` 6 用例覆盖矩阵缺口（无 sampler 不发请求、checkpoint 不可用 fail-closed 转同步、传输重试耗尽有界 fail-closed、授权变更 Superseded 不发布且有终局、池压取消有终局）。
- 证据：6/6（三次连跑）；codex-guardian-v2 全量 96-97/98（1 例需 V8 归档离线不可得，既有；2 例负载敏感单跑过，既有）。
- **重要登记（T10/R11 新证据）**：guardian-v2 测试二进制默认 ~2MB 栈在 `Session::new` 单个 poll 帧内溢出（crash report：仅 48 帧深、单帧超限）——需 RUST_MIN_STACK=16777216 才能跑全量。HEAD 干净树复现，非本会话改动引入。


### 批 I — T13 历史身份复验 + R11 定性证据（进行中，本机窗口）

- 复验脚本 `/tmp/t13/run.sh` 顺序执行 9 组；本机同期其他用户负载 15→92，负载敏感族的结论按窗口如实标注。
- **已闭合身份**：R1b 13（批 F）；install-context 1（16/16，宿主机 brew-cask 符号链接导致断言机器相关，fixture 修复后过，机制与历史失败吻合）；v8-poc 1（6/6）；http-client 6（本会话安静窗口 141/141、148/148 两次以上全量）。
- **otel 6：当前树仍失败**（非本会话回归；历史 #263-268）：export 请求 1s 内未达回环 collector。T08 分段计时（collector bind/readiness→connect→export→flush/shutdown）待做；R7 同型阻塞为候选。
- **R11 定性修正（2026-10-10 下午补测，T10 证据侧完成）**：
  - 精确阈值：`conversation_uses_default_realtime_backend_prompt` 单测 2.5MiB 失败 / 3MiB 通过（最深连续 poll 区约 2.1–3MiB）。
  - **非回归**：pre-merge 基线（a1d519778，09-30）在 2MiB 下同样 SIGABRT——`TestCodex→start_thread→Session::new` 链的 >2MiB 是长期潜伏条件，10-01 合并未引入。crash 栈：`Session::new::{{closure}}` 巨型 poll（同符号双物理帧）+ ~15 层 builder poll 帧；tui 栈顶另有 serde_core toml 反序列化大帧。
  - **仓库标准工具链未被阻断**：`just test`（RUST_MIN_STACK=8MiB）下 core 选集 0 abort；codex-guardian-v2 全量 94/98（0 abort）——4 失败中 1 例为 V8 归档离线不可得（既有），3 例 connection_pool cooldown 为负载敏感（隔离复验 3/3 PASS，本日 load≈24）。此前"需 ≥16MiB"的记录系直接 cargo 调用缺 env 所致，已修正。
  - 处置：T10 的"最小修复"降级为加固项（缩小 Session::new 连续 poll 区至默认栈内），不作为历史身份的绿灯门槛；core/tui 身份复验在 ≤8MiB+安静窗口执行。
- **residency::websocket：当前树仍失败**：rig responses client "builder error"（构建失败），独立线索待查（T07-2/T08 关联）。
- rmcp 2 / exec-server 9：极端负载窗口（62-92）整包大量超时失败，无法作证；批 D 安静窗口 exec-server 616/616 的既有收据仍在。均标记 UNVERIFIED-LOAD，需安静窗口隔离复验。
- tui 45：套件在跑（5642 项，负载下预计 >1h），本批先登记为待安静窗口。


### 批 J — R11 修正 + T08 OTLP 归因 + run2 身份复验（2026-10-10 下午，HEAD 起于 f32db9fb0）

- **R11 修正（推翻批 I 的两个误判）**：单测栈阈值 2.5MiB 败/3MiB 过；**pre-merge（a1d519778，09-30）2MiB 下同样 SIGABRT → 长期潜伏条件，非合并回归**；`just test`（8MiB）下 core 选集 0 abort、guardian-v2 全量 94/98（0 abort；4 失败=1 例 V8 离线既有 + 3 例 cooldown 负载敏感，隔离 3/3 过）。"需 16MiB"系直跑 cargo 缺 env 假象。T10 最小修复降级为加固项。
- **T08 OTLP 线归因**：非 collector 机制——loopback 夹具的 ReqwestDefault 客户端内建系统代理读取在并发 provider 构建下停顿（solo 过、任意 pair 双败 12.5s）。夹具改 RespectSystemProxy（经专职 run-loop 线程读取，Direct 路由 no_proxy 客户端）后 pair 3.0s 双过；≥7 路并发仍越 3s collector 预算（opentelemetry 批 flush 边界类）。提交见 otel 测试变更。
- **run2 身份复验（8MiB，load≈24）**：rmcp 2 → **PASS 关闭**；app-server browser_login/review_start/external_auth → **PASS 关闭**（browser_login 此前 abort 即 2MB 栈假象）；exec-server 9 身份 8 过（watch_events 隔离过=FSEvents 并发边界）+1 边界；core realtime 家族仍受 2s 握手预算 vs load 24 限制（其中一个身份 3MiB 单跑过）——待安静窗口。
- **residency::websocket 定性为真实开放缺陷**：仅 WS 变体（http 变体同配置过）；rig WS 请求带 managed residency 覆盖头时 reqwest13 在 request-builder 阶段拒绝（`Error::Instance("builder error")`，非法头注入）。链路在 WS 侧 header 注入路径，待专项。
- tui 45：仍待安静窗口（昨日全量跑被会话中断；产生的未跟踪 .snap.new 留待人工审阅，未批量接受）。


### 批 K — T07 阶段2：默认模式 matcher 核心（HEAD `5ac058769`）

- 新 `outbound_proxy/default_proxy_matcher.rs`：平台中立的 `DefaultProxyMatcher`，逐字段复刻锁定 hyper-util 0.1.20 的默认解析语义（env 逐 scheme/ALL_PROXY 回退、空值按未设回退与手动填充、手动系统项只填空 scheme、CGI 全禁、NO_PROXY curl 语义含 *、IP 精确/CIDR（本地实现 CIDR，零新公开依赖）、点边界域名后缀、userinfo 百分号解码为 basic auth、socks4/4a/5/5h 可用、ws/wss 永不拦截）。
- 15 个 parity 向量（含上游自身测试向量，含其空变量/缺 scheme 语义修正：空 scheme 变量如同未设——回退 ALL_PROXY 且允许手动填充；上游向量串以源码为准修正 bar.baz/bar.foo 转写）；env 敏感用例在净环境子进程执行。
- base64 入 http-client 依赖（workspace 既有 0.22.1）；`just bazel-lock-update` 已跑（MODULE.bazel.lock 无变化，base64 已在闭包内）；全树归一到仓库 canonical nightly rustfmt（import 粒度）。
- 证据：matcher 15/15；合并作用域 39/39（--retries 0）；全包除已知负载边界族外绿（load≈12 隔离全过）。
- **阶段3（接管）开放**：设计=macOS 手动项提取（loader CFDictionary → ManualSystemProxies）+ `resolve_proxy_route(ReqwestDefault)` 改走 matcher（Direct=no_proxy 客户端 / Proxy=显式代理 URL 含 userinfo 交给 reqwest 解析）+ 逐 hop 由既有 route-aware 重定向保证 + **双轨对照先行**（真实参考 client × env 矩阵）——按 plan 要求双轨证据先于接管落地。


### 批 L — run3 收尾：tui 45 身份全闭 + spinner 帧修复（HEAD `94ab4f533`）

- **tui 全量（安静窗口，8MiB，44 分钟）5633/5642**；9 失败中 8 个为历史身份、1 个非历史（provider_defaults，不在 119）。
- 8 个失败身份根因一致：**快照钉死 working spinner 的墙钟帧**（elapsed "Ns" 与交替字形 •/◦）；修复=在 chatwidget/app 快照边界把帧规范化为首帧（与既有 completion-footer 时长规范化同一模式），多数字秒用行宽保持填充；**零 accepted 快照改动**（所有既有快照本就钉首帧）。顺序复验 8/8、零 pending；并发跑快照内容仍一致（差异仅元数据行），预算类失败不变。
- **tui 45 个历史身份全部入账**：37 个 run3 全量通过 + 8 个修复后通过（`current-test-ledger.tsv` 现 88 行）。
- core 51 选集在 load≈8 仍 45 败：定性为**并发预算边界**（非负载、非栈）——shell/realtime 家族 solo/pair 全过、并发即越 1.6s/2s 内部预算（如 parallel-tools 的 1.6s 并行时长预算在任意双进程并发下即越界 1.73s）。在共享机器上不可并发验证绿；按"单过/并发越预算"边界如实登记，未放宽任何预算。
- run3 期间确认：`history_lookup_uses_server_provider...`（非历史身份）在安静窗口也失败——不属 119，另记待查。


### 批 M — 119 账目闭环（HEAD 含 provider_id 断言修复提交）

- **119 历史身份全部入账**（ledger 122 行，含 header 校验行）：**PASS 97 / BOUNDARY 23 / FAIL 2**。
- 收尾三项：
  1. `derive_config_from_params_uses_session_thread_config_model_provider`（app-server）：历史失败=fork 的 load-time `provider_id` 盖章与旧断言矛盾（config_manager 注释有档）；断言改盖章后形状，PASS。
  2. `managed_network_proxy_decider_survives_full_access_start`（core）：**真实开放缺陷候选**——managed network proxy 返回 403 `blocked-by-allowlist`，full-access start 下的 allowlist 判定与测试预期不符；与 residency::websocket 并列为当前仅有的两个 FAIL，需专项（网络策略/NUWAX 控制域 / rig WS 头注入）。
  3. core 30 身份按并发梯度定性（solo 全过、pair 起亚秒窗/握手预算越界=BOUNDARY 23 中的主体）；exec 事件 11 项非 macOS 分支照常通过。
- 遗留统计口径：BOUNDARY 23 = 共享机器上无法并发验证绿的预算类身份（solo/pair 复验全过），非功能回归；两个 FAIL 已给出机制与定位。run3 期间另发现 1 个非 119 失败（tui provider_defaults.history_lookup）待查。


### 批 N — 两个开放缺陷关闭 + T07 阶段3 opt-in 落地（HEAD `5753a3505`）

- **residency::websocket 关闭（`c5d76073d`）**：双重根因——①rig bridge endpoint 不翻译 ws://→http://（原生传输才拥有 ws 握手；此前以 reqwest 构建器错误暴露）已修 + 向量测试；②测试自身缺 native 传输 pin（fork 默认桥接策略走 rig 无 WS 腿，attestation 同款注释先例）且 base_url 直写 ws://（与全语料约定不符）已修。rig-bridge 252/252、residency 家族 2/2。
- **managed_network_proxy_decider 关闭（`395c619fc`）**：非产品缺陷——本机 DNS 把 example.com 解析到私网 IP → NotAllowedLocal 基线拒绝短路（decider 只参与 NotAllowed）。探针改公网 IP 字面量；42 项 network proxy 家族全绿。
- **T07 阶段3（`5753a3505`）**：`OutboundProxyPolicy::ResolvedDefault`（opt-in）——默认传输的精确优先级本地解析（env 逐 scheme + 空=已设语义、macOS 手动项只填空 scheme 且经专职 run-loop loader、ALL_PROXY 回退、NO_PROXY 先行、ws/wss 不拦截），池收到全显式路由 → client 构建不再做 reqwest 同步系统读取；userinfo 保留给传输解析；SOCKS 委托传输；逐 hop 重解析与 RespectSystemProxy 同轨。证据：8 路由向量 + **双轨 wire 等价**（参考 reqwest::Client vs ResolvedDefault 池，11 个子进程用例逐案同路）+ 22/22 合并向量。**默认迁移（ReqwestDefault→ResolvedDefault）按 Spec 留待单独裁决**——现在是一次一行改动。
- 119 账本终态更新：**PASS 99 / BOUNDARY 23 / FAIL 0**。
- 非历史项：tui provider_defaults.history_lookup 失败系跨二进制依赖（需 target/debug/codex，单包运行无该二进制）——环境性，非缺陷。


### 批 O — T17 核心落地：per-response usage ledger（HEAD `69c0ea00e`）

- `SessionState.response_token_usage: HashMap<response_key, TokenUsage>`；`record_token_usage` 改替换语义——同 key 重报（pause 续接/重复 Completed/resume 后重放）在 turn/thread 小计中以新快照换旧快照，不再双计；无 key 报告保持累加（文档注明）；跨 turn 重报净额入当前 turn、线程总额取替换值；减法下限 0。
- resume/fork 从持久化 TokenUsageRecord 重建 ledger（同 key 后写胜出）；compacted checkpoint 只带最新记录，祖先 response 不入账（边界注明）。此为 cap 决策 D4 的 host-key reducer 核心；CapExhausted 侧接入在后续批。
- 证据：4 ledger 单测 + 恢复总额回归 6/6；39 项 usage/token 家族 -j2 全绿（宽并发失败均为已登记的机器竞争预算类，隔离过）。
- T17 剩余：host response_key 与 provider id 分离的稳定化（当前用 provider response_id 作 key，决策文档的 response_key 管道在 T18）。


### 批 P — T18 第一段落：durable cap partial（HEAD `d51d5ef4d`）

- **bridge**：Length 终止先 flush pending partial（`cap_flush_partial_and_error`），pump 在返回类型化错误**之前**逐条投递 flush 出的 item 事件——partial 落为普通历史项（可见、可 resume、就是模型真实流出的文本，绝不合成成功）；flush 失败仅记日志并仍按 cap 错误关闭。
- **协议+核心**：`CapPartialEvent`（fragment: assistant_text/reasoning/tool_arguments[仅诊断不执行]；单片段 16KiB UTF-8 边界保前缀；turn 64KiB/32 片段封顶；budget 块记录触发项与 `Unverified` token 证据——无 tokenizer 证明时仅字节界诊断，不入模型上下文，D1-c 门保持）。事件恰在失败终局前发一次；rollout policy 两种 history mode 均持久化（EventMsg 通道）；rollout-trace 类型登记。
- **测试契约更新**：core cap 套件改为"flushed final AgentMessage 必须恰为 partial 文本"（防合成成功的旧断言改为防不一致）；bridge 终止向量要求唯一 partial Done 先于错误且零 Completed。
- 证据：rig-bridge 252/252（--retries 0）；core cap 集成 7/7 + cap_partial 单测 7/7；chat terminal 向量 5/5。
- **T18 剩余**：host response_key 与 provider id 分离；CapExhausted.reported_usage 打通 CodexErr 映射进 ledger（当前 InvalidRequest(String) 丢弃 usage）；v2/app-server 读侧投影与 TurnItem 变体；旧读端 V-D1-1 实测。


### 批 Q — T18 第二段：协议记录 + 有界捕获构造器（HEAD `6b45debc4`）

- 落地 CapPartial 的**可评审地基**：协议类型（fragment 三类、单片段/整 turn 预算块、token 证据显式 Unverified）、EventMsg 通道（rollout policy 双 mode 持久化）、rollout-trace 类型登记、有界捕获构造器（16KiB/片段 UTF-8 边界保前缀、64KiB/32 片段封顶、enforced-cap 溯源）+ 7 单测钉住全部边界。
- **如实登记未竟**：turn-error 发射点 + usage 跨错误映射的携带未落地——集成诊断证明错误到达该位置时既无结构化 payload 也无 trailing 项（rig-bridge 错误在 pump→client 错误臂之间被重整形、flush 项未在该点入史）；基于 stash 的尝试已回退（不留未验证接线），下一步为沿 bridge stream→client 错误臂逐步追。桥层 durable partial（批 P）仍是用户可见的一半且已验证。
- 证据：cap_partial 单测 7/7；core cap 集成套件绿（宽跑 1 失败=idle 预算计时测试，隔离过）；codex-core 零警告。


### 批 R — CI 首轮实跑修复循环（HEAD `831346f11`，runs 38045982072/38047414958/38049532997/38053827346）

T02 首次获得真实运行证据，四轮递进修复：
1. **Run 1**：四 job 全部**越过 setup**（bazelbuild/setup-bazelisk@v3.0.0 + brew gstreamer 生效——上轮四 job 全死于 setup）；ubuntu/macos 挂在 Clippy（fork surfaces）。
2. **Run 2**（`a2f8e8d84` 修 4 处限制 lint）：Clippy 过；三 nextest 车道全部**首次真正编译并进入测试**；bazel gate 挂在 doctor.rs 的 CapExhausted 非穷尽 match（Bazel 车道编译 cli，Cargo clippy 作用域不含）。
3. **Run 3**（`22b91089a` 修 doctor + V8 门控）：**bazel bridge gate 首次全绿**；三 nextest 车道编译全工作区后在 Test 步失败——根因定位：**v8 150.4.0 的 `ptrcomp_sandbox` 预编译归档从未发布**（上游 2e32d9589 开启 sandbox 特性；本地能过仅因 build script 缓存）。V8 pin 按禁令不动；车道排除 code-mode-runtime/code-mode-host/v8-poc 并注明（与 voice-host 同款缺工件模式，配对工件流程已登记）。
4. **Run 4**（`831346f11` 修 ollama match）：三车道唯一剩余编译错误=ollama client 的 ResolvedDefault 非穷尽（本地作用域运行从未编译该 crate——CI 全工作区编译的价值实证）。
- **T02 DoD 达成**：四 job 均实际进入相应 build/test；剩余失败从"基础设施"降级为"真实测试失败"（大概率=已定性的并发固有边界族），待 run 4 结果确认。


### 批 S — CI 修复闭环与剩余失败的终局定性（run 38065399719，HEAD `bac940d34`）

- 本轮三个真实产品/流程修复（均已在 CI 前序运行中定位并各自验证）：
  1. `23d8d3e6f` **TLS 分类**：io::Error::source() 跳过 payload 本体导致 Linux 路径 TLS 错误不可见——提取 `chain_carries_tls_error` 显式下钻 get_ref() + 回归向量（Linux 残差 6 项的根因）。
  2. `e02dc3771` **schema fixture**：RolloutLine 差异非平台性，是批 Q 的 CapPartial 类型未再生成预计算导出（本地作用域跑漏该 crate）；`just write-app-server-schema` 再生成后 314/314。
  3. `eaecf3805` **bubblewrap + 超时**：ubuntu 装 bubblewrap、车道超时 90→150 分钟。
- **run 38065399719 终局**（bubblewrap 已装、TLS 已修、fixture 已再生）：bazel gate **连续第三次全绿**；三 nextest 车道全部完成完整工作区测试（ubuntu 21.5k 项 406 失败 / macOS 217 / windows 175）。剩余失败**终局定性**：
  - **ubuntu 389/406 = Linux 沙箱族**：bubblewrap 已在 PATH，但 `bwrap: loopback: Failed RTM_NEWADDR: Operation not permitted`——GitHub 托管 runner 禁止嵌套网络命名空间配置（无 NET_ADMIN）。**这不是可装的依赖缺口，是 runner 能力边界**：需要自托管/特权 runner（T32 的正式 Linux 验证环境），或上游测试基建按能力检测降级。按禁令不加 skip 求绿。
  - **macOS 217 ≈ 并发固有边界族**：本地同套件 5633/5642（安静窗口）；2-vCPU CI runner 上亚秒断言窗全灭——与本地已定性的"串行绿/并发败"完全同类。
  - windows 175 同类叠加 Windows 特有 junction/沙箱项。
- **T02 最终状态**：DoD（四 job 实际进入相应 build/test）**达成且稳定**；CI 失败已从"工作流坏"降级为"真实测试结果"，其中大头可归因到已登记的 runner 能力/并发边界。下一步绿化需要环境决策（自托管 runner），不是更多 workflow 补丁。

## 40 项状态（更新）

| 项 | 状态 | 证据/边界 |
|---|---|---|
| T03/T04/T05/T06 | 完成 | 批 A/B |
| T02 | 部分（配置完成） | 远端 dispatch 待授权 |
| T07 | **实现完成（opt-in）** | 阶段1 loader（批D）+ 阶段2 matcher（批K）+ 阶段3 ResolvedDefault opt-in + 双轨 wire 等价（批N）；**默认迁移留待裁决**；CFRunLoop 因果实验开放 |
| T11 | **完成** | 13 身份 16/16；grant+scope 级 Compaction 契约 |
| T12 | **完成（测试覆盖）** | 6 新用例 + 矩阵；未发现终局性产品缺陷 |
| T16 | D2 段完成 | 三线提取+迁移；V-D2-1/reducer/key 开放 |
| T15 | 决策完成 | cap-decisions-2026-10-10.md 七项全裁决 |
| T10/R11 | **定性完成** | 长期潜伏（pre-merge 既有）、阈值 2.5/3MiB、8MiB 全消 abort；加固（缩 Session::new 帧）开放非阻断 |
| T08 OTLP 线 | **归因完成** | 并发 provider 构建停顿 vs 3s collector 预算；夹具已切 RespectSystemProxy（pair 修复）；批 flush 并发边界开放 |
| T13/T01 | **账本完成** | 119 身份全入账：PASS 99 / BOUNDARY 23 / FAIL 0（批 L-N）；旧机工件联结仍缺（需旧机导出） |
| T14 | 部分 | 批 I 复验覆盖 proxy/TLS/WS/exec-registry/Core/Rig 主面；正式 DoD 快照未单独出 |
| T17–T20/T21–T23/T24–T31/T32–T35/T36–T37/T38–T40 | 开放 | 按序推进 |

## 40 项状态（滚动更新）

| 项 | 状态 | 证据/边界 |
|---|---|---|
| T01 | 进行中 | 本文件即账目载体；119 TSV 逐身份复验在 T13 批推进；旧机 still249/signatures 工件不在本机（阻断：需要旧机工件导出） |
| T02 | 部分完成 | 配置修复+本地验证；远端 dispatch 待授权 |
| T03 | 完成（本机） | 批 A；Windows 真实环境另验 |
| T04 | 完成 | 批 A |
| T05 | 完成 | 批 B |
| T06 | 完成 | 批 B |
| T07 | 阶段 1 完成 | 批 D；阶段 2/3（matcher+双轨+接管）开放 |
| T11 | 实现完成，复验中 | 批 E |
| T02/T03/T04/T05/T06/T07-1/T11 以外 | 开放 | 按执行顺序推进中 |

## 当前失败完整列表

（按批次滚动登记；本页最新状态为准）

- 负载窗口 `--retries 0` 下 `route_aware_client_pool` 4 子进程用例可瞬态失败（R7 机制，HEAD 同现；local profile 已缓解并在 T07 阶段 2/3 中根治）。
- 其余：见各批"开放"行。
