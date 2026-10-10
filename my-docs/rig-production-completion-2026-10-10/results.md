# results.md — 2026-10-10/11 全量执行收据

按 `claude-prompt.md` 的批次纪律记录历史 HEAD、命令、范围、结果、首败与复验、开放边界。历史本机环境：macOS（Darwin 27，arm64），`just test` = nextest（profile local，retries=1，`--no-fail-fast`）；另有标注 `--retries 0` 的严格窗口。机器同期有其他用户负载（load 15–30），负载敏感项已注明。2026-10-11 独立复审只读核对了已有 CI 日志和 metadata，并修正文档/账目；本轮没有运行新测试，旧收据不能证明本轮未提交修正已通过。

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
- **T08 OTLP 旧窗口记录**：loopback 夹具使用 ReqwestDefault 时观察到并发 provider 构建停顿（solo 过、pair 双败 12.5s）；改 RespectSystemProxy 后 pair 3.0s 双过，≥7 路并发仍失败。这只证明替代 fixture 的行为差异，未单独证明 flush 根因。生产默认 factory 仍使用 ReqwestDefault，因此不能把夹具切换称为默认生产路径修复。run 6 的六个 OTLP 历史身份均 PASS，见账目追加列。
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
- core旧窗口记录：51选集在load≈8时45败，shell/realtime隔离通过、部分并发运行越1.6s/2s预算（parallel-tools观测1.73s）。这些记录支持该本机窗口受并发影响；原“非负载、并发固有”因果定性撤回，不能据此关闭其他环境的失败。预算未放宽。
- run3 期间确认：`history_lookup_uses_server_provider...`（非历史身份）在安静窗口也失败——不属 119，另记待查。


### 批 M — 119 账目闭环（HEAD 含 provider_id 断言修复提交）

- **119 历史身份登记齐全**。原记录的“PASS 97 / BOUNDARY 23 / FAIL 2”混用了行与身份，撤回该总计；三组路径别名及最新统计见 `canonical-ledger-map.tsv` 和下方状态表。
- 收尾三项：
  1. `derive_config_from_params_uses_session_thread_config_model_provider`（app-server）：历史失败=fork 的 load-time `provider_id` 盖章与旧断言矛盾（config_manager 注释有档）；断言改盖章后形状，PASS。
  2. `managed_network_proxy_decider_survives_full_access_start`（core）：**真实开放缺陷候选**——managed network proxy 返回 403 `blocked-by-allowlist`，full-access start 下的 allowlist 判定与测试预期不符；与 residency::websocket 并列为当前仅有的两个 FAIL，需专项（网络策略/NUWAX 控制域 / rig WS 头注入）。
  3. core 30 身份按并发梯度定性（solo 全过、pair 起亚秒窗/握手预算越界=BOUNDARY 23 中的主体）；exec 事件 11 项非 macOS 分支照常通过。
- 遗留窗口口径：BOUNDARY23保留本机隔离/并发不同结果，不能单凭该结果证明非功能回归或关闭CI失败；两个FAIL在后续批处理。run3另发现非119身份tui provider_defaults.history_lookup，见批N及最新状态。


### 批 N — 两个开放缺陷关闭 + T07 阶段3 opt-in 落地（HEAD `5753a3505`）

- **residency::websocket 关闭（`c5d76073d`）**：双重根因——①rig bridge endpoint 不翻译 ws://→http://（原生传输才拥有 ws 握手；此前以 reqwest 构建器错误暴露）已修 + 向量测试；②测试自身缺 native 传输 pin（fork 默认桥接策略走 rig 无 WS 腿，attestation 同款注释先例）且 base_url 直写 ws://（与全语料约定不符）已修。rig-bridge 252/252、residency 家族 2/2。
- **managed_network_proxy_decider 关闭（`395c619fc`）**：非产品缺陷——本机 DNS 把 example.com 解析到私网 IP → NotAllowedLocal 基线拒绝短路（decider 只参与 NotAllowed）。探针改公网 IP 字面量；42 项 network proxy 家族全绿。
- **T07 阶段3（`5753a3505`）**：`OutboundProxyPolicy::ResolvedDefault`（opt-in）——默认传输的精确优先级本地解析（env 逐 scheme + 空=已设语义、macOS 手动项只填空 scheme 且经专职 run-loop loader、ALL_PROXY 回退、NO_PROXY 先行、ws/wss 不拦截），池收到全显式路由 → client 构建不再做 reqwest 同步系统读取；userinfo 保留给传输解析；SOCKS 委托传输；逐 hop 重解析与 RespectSystemProxy 同轨。证据：8 路由向量 + **双轨 wire 等价**（参考 reqwest::Client vs ResolvedDefault 池，11 个子进程用例逐案同路）+ 22/22 合并向量。**默认迁移（ReqwestDefault→ResolvedDefault）按 Spec 留待单独裁决**——现在是一次一行改动。
- 两个开放身份在该批复验通过。原“PASS 99 / BOUNDARY 23 / FAIL 0”混入路径别名，不能作为 119 身份终态；最新历史集合/全部记录统计分别列在下方。
- 非历史项：tui provider_defaults.history_lookup 失败系跨二进制依赖（需 target/debug/codex，单包运行无该二进制）——环境性，非缺陷。


### 批 O — T17 核心落地：per-response usage ledger（HEAD `69c0ea00e`）

- `SessionState.response_token_usage: HashMap<response_key, TokenUsage>`；`record_token_usage` 改替换语义——同 key 重报（pause 续接/重复 Completed/resume 后重放）在 turn/thread 小计中以新快照换旧快照，不再双计；无 key 报告保持累加（文档注明）；跨 turn 重报净额入当前 turn、线程总额取替换值；减法下限 0。
- resume/fork 从持久化 TokenUsageRecord 重建 ledger（同 key 后写胜出）；compacted checkpoint 只带最新记录，祖先 response 不入账（边界注明）。此为 cap 决策 D4 的 host-key reducer 核心；CapExhausted 侧接入在后续批。
- 历史证据：4 ledger 单测 + 恢复总额回归 6/6；39 项 usage/token 家族 -j2 通过。新替换分支只在直接调用 SessionState 的单测中验证；既有 token_usage_rollout 集成用不同 response ID，不能证明重复快照或恢复 ledger 后重报路径。
- T17 剩余：host response_key 与 provider id 分离的稳定化；公共 Responses→Core→rollout 的重复快照、跨 turn、shutdown/resume 后重报回归。当前仍以 provider response_id 作 key。


### 批 P — T18 第一段落的历史尝试（HEAD `d51d5ef4d`，durable 验收未成立）

- **历史 bridge 尝试**：Length 终止曾 flush pending partial 并作为普通 item Done 投递；这改变了 cap 的普通完成事件合同，不能据此称 durable partial 已验收。独立复审修正已恢复 cap 不发普通 Done/最终 AgentMessage 的合同，本轮未运行测试。
- **协议+核心地基**：CapPartialEvent 类型、16KiB/片段与64KiB/32片段预算、Unverified token 证据、rollout policy 和 trace 登记存在；发射点未接通，因此类型与持久化白名单不等于已实际发射/写入事件。
- **历史测试断言边界**：当时 core cap 仅在 final AgentMessage 为 Some 时比较文本，None 同样通过；没有读取 rollout/resume 来断言 partial。因此不能证明“最终 partial 必须存在”或“可 resume”。本轮已恢复零普通完成事件的断言，尚未运行。
- 历史收据：rig-bridge 252/252、core cap 7/7、cap_partial 单测7/7、chat terminal 5/5；这些是当时源码的结果，不证明 durable 读写/resume，也不证明本轮修正通过。
- **T18 剩余**：host response_key 与 provider id 分离；CapExhausted.reported_usage 打通 CodexErr 映射进 ledger（当前 InvalidRequest(String) 丢弃 usage）；v2/app-server 读侧投影与 TurnItem 变体；旧读端 V-D1-1 实测。


### 批 Q — T18 第二段：协议记录 + 有界捕获构造器（HEAD `6b45debc4`）

- 落地 CapPartial 的**可评审地基**：协议类型（fragment 三类、单片段/整 turn 预算块、token 证据显式 Unverified）、EventMsg 通道（rollout policy 双 mode 持久化）、rollout-trace 类型登记、有界捕获构造器（16KiB/片段 UTF-8 边界保前缀、64KiB/32 片段封顶、enforced-cap 溯源）+ 7 单测钉住全部边界。
- **未完成**：turn-error 发射点 + usage 跨错误映射的携带未落地，基于 stash 的尝试已回退。批 P 的普通 Done 尝试已在独立复审中撤回；durable partial 的实际采集、发射、持久化、公共读侧和 resume 仍需接线及集成证明。
- 证据：cap_partial 单测 7/7；core cap 集成套件绿（宽跑 1 失败=idle 预算计时测试，隔离过）；codex-core 零警告。


### 批 R — CI 首轮实跑修复循环（HEAD `831346f11`，runs 38045982072/38047414958/38049532997/38053827346）

T02 首次获得真实运行证据，四轮递进修复：
1. **Run 1**：四 job 全部**越过 setup**（bazelbuild/setup-bazelisk@v3.0.0 + brew gstreamer 生效——上轮四 job 全死于 setup）；ubuntu/macos 挂在 Clippy（fork surfaces）。
2. **Run 2**（`a2f8e8d84` 修 4 处限制 lint）：Clippy 过；三 nextest 车道全部**首次真正编译并进入测试**；bazel gate 挂在 doctor.rs 的 CapExhausted 非穷尽 match（Bazel 车道编译 cli，Cargo clippy 作用域不含）。
3. **Run 3**（`22b91089a` 修 doctor + V8 门控）：**bazel bridge gate 首次全绿**；三 nextest 车道编译全工作区后在 Test 步失败——根因定位：**v8 150.4.0 的 `ptrcomp_sandbox` 预编译归档从未发布**（上游 2e32d9589 开启 sandbox 特性；本地能过仅因 build script 缓存）。V8 pin 按禁令不动；车道排除 code-mode-runtime/code-mode-host/v8-poc 并注明（与 voice-host 同款缺工件模式，配对工件流程已登记）。
4. **Run 4**（`831346f11` 修 ollama match）：三车道唯一剩余编译错误=ollama client 的 ResolvedDefault 非穷尽（本地作用域运行从未编译该 crate——CI 全工作区编译的价值实证）。
- **T02 启动/构建入口 DoD 达成**：四 job 均实际进入相应 build/test。剩余失败包含缺工件和真实测试结果，不能统一降级为已定性并发族；T38 完整门禁仍开放。


### 批 S — CI 已有结果核对（run 38065399719，测试 SHA `e02dc3771710c0a4e6fd8b3dd125715b08c39b55`）

- 本轮三个真实产品/流程修复（均已在 CI 前序运行中定位并各自验证）：
  1. `23d8d3e6f` **TLS 分类**：io::Error::source() 跳过 payload 本体导致 Linux 路径 TLS 错误不可见——提取 `chain_carries_tls_error` 显式下钻 get_ref() + 回归向量（Linux 残差 6 项的根因）。
  2. `e02dc3771` **schema fixture**：RolloutLine 差异非平台性，是批 Q 的 CapPartial 类型未再生成预计算导出（本地作用域跑漏该 crate）；`just write-app-server-schema` 再生成后 314/314。
  3. `eaecf3805` **bubblewrap + 超时**：ubuntu 装 bubblewrap、车道超时 90→150 分钟。
- 2026-10-11 只读核对该 run metadata：event=push，四 job 均进入各自 build/test；Bazel gate success，三个 nextest job failure。该 SHA 的结果不覆盖本轮未提交修正。
- 三车道执行的是经 workflow 排除后的集合，均使用 nextest profile local、retries=0；不称完整未排除 workspace 通过。150分钟为 job 总预算，单测试 deadline 未随之修改。

| 平台 | run | PASS | Summary FAIL桶 | 其中 ABORT | 其中 FAIL+LEAK | TIMEOUT | SKIP |
|---|---:|---:|---:|---:|---:|---:|---:|
| Ubuntu | 21561 | 21151 | 408 | 1 SIGABRT | 0 | 2 | 47 |
| macOS | 21514 | 21292 | 217 | 0 | 0 | 5 | 68 |
| Windows | 20400 | 20205 | 176 | 1 ABORT | 1 | 19 | 138 |

- Summary FAIL桶包含 ABORT/FAIL+LEAK，不能再与这些子类重复求和。`ci-run6-failure-ledger.tsv` 完整列出827个非PASS终态：Ubuntu407 FAIL+1 SIGABRT+2 TIMEOUT，macOS217 FAIL+5 TIMEOUT，Windows174 FAIL+1 FAIL+LEAK+1 ABORT+19 TIMEOUT。文件只保存平台、完整身份、状态和分类，不含stdout或凭据。
- **撤回原终局分类**：明确缺 code-mode-host 的输出块至少 Ubuntu60/macOS60/Windows36；排除 host crate 未同步解决依赖它的 app-server/Core 测试。Ubuntu105个失败块明确出现 bwrap loopback RTM_NEWADDR EPERM；这证明该运行操作被拒绝，尚不能证明389项同因、缺NET_ADMIN是唯一原因或必须更换runner。其余保留unclassified，不能用预算/runner结论关闭。
- **23个旧BOUNDARY身份均在该run macOS PASS**，已追加到current ledger；保留本机旧窗口，但撤回“macOS217失败就是这些边界族”的推断。
- **CI栈失败另列**：Ubuntu SIGABRT与Windows ABORT均为 `codex-core guardian::tests::guardian_ephemeral_retry_preserves_parallel_trunk_and_fork_history`；Windows明确报告0xc00000fd stack overflow。此前8MiB下0 abort仅适用于旧macOS选集，不能外推跨平台公共路径已关闭。
- 证据入口：`/tmp/run6logs/{1_nextest (ubuntu-latest),2_nextest (macos-latest),3_nextest (windows)}.txt` 的Summary分别在37460/32119/29317行；`gh run view 38065399719 --repo nuwax-ai/codex --json headSha,conclusion,event,jobs,url`。这些是已有工件/metadata，本轮未dispatch或重跑。
- **T02仅启动/构建入口已验；T38未达成**。下一步需补工件、查Linux namespace能力与栈失败，并逐身份归因，不能凭当前红色车道宣布测试门禁或发布验收完成。

## 独立复审后的账目口径（2026-10-11）

- `current-test-ledger.tsv`保留126条旧窗口记录：98 PASS / 3 FIXED-linux / 23 BOUNDARY / 2 FAIL-linux。这是行数，包含3条路径别名和2条家族摘要，不能称126个完整身份。
- `canonical-ledger-map.tsv`逐行映射：119 historical-identity、3 alias、2 summary-not-identity、2 new-identity。去掉别名/摘要后为121个完整canonical身份；家族摘要未展开，不推算其新增identity数量。
- 119历史tuple均有登记，旧窗口状态为95 PASS / 1 FIXED-linux / 23 BOUNDARY；23 BOUNDARY另有该run macOS PASS列。原始窗口与CI结果分别保留，不能用一个合计覆盖全部平台/批次。TSV占位 `-` 表示该列未登记结果/身份，不表示SKIP或通过。
- CI非PASS全表另见 `ci-run6-failure-ledger.tsv`。只有输出中明确出现原因的记录才归入explicit类别；unclassified不是已证明的runner或负载边界。

## 40 项状态（以本表为准；历史收据不验收本轮修正）

| 项 | 状态 | 证据/边界 |
|---|---|---|
| T01 | 部分 | 119历史tuple登记齐全，canonical/CI账目已修正；旧机原始工件联结仍缺 |
| T02 | 启动/构建入口已验 | 四job进入build/test；三个nextest仍失败，完整门禁T38未达成 |
| T03 | 部分（旧本机收据） | 批A键与TLS测试；Windows真实A→B仍缺，本轮修正未验 |
| T04/T05/T06 | 旧作用域收据完成 | 批A/B；本轮修正未验，平台/公共路径边界另列 |
| T07 | opt-in旧实现收据，完整DoD开放 | 批D/K/N；默认迁移未裁决、CFRunLoop因果与默认生产路径未关闭，本轮修正未验 |
| T08 | 部分归因 | OTLP替代fixture pair通过不能证明默认生产根治；Core/rmcp三线仍须各自证据 |
| T09 | 开放 | 冷首请求原deadline与分段证据未齐 |
| T11 | 旧实现复验收据；本轮修正未验 | 旧13身份16/16不能验收独立复审后的授权/投影修改 |
| T12 | 六个新增fixture收据完成 | 有请求/终局断言；不扩大为全部公共入口已验 |
| T16 | D2 段完成 | 三线提取+迁移；V-D2-1/reducer/key 开放 |
| T15 | 决策完成 | cap-decisions-2026-10-10.md 七项全裁决 |
| T10/R11 | 旧macOS选集定性收据；CI栈失败开放 | pre-merge/阈值是历史本机证据；run6 Ubuntu/Windows有公共guardian测试abort，不能外推8MiB跨平台已关闭 |
| T13 | 身份登记完成，验收开放 | 119历史tuple和23项CI PASS已登记；run6全部827个非PASS终态另列，仍有未归因项 |
| T14 | 部分 | 批 I 复验覆盖 proxy/TLS/WS/exec-registry/Core/Rig 主面；正式 DoD 快照未单独出 |
| T17 | reducer单测切片完成，公共集成开放 | host key/重复ID与恢复后重报仍需实际Responses→Core→rollout证明 |
| T18 | 协议/预算地基，durable功能开放 | cap不发普通Done的合同已恢复但本轮未验；采集/发射/持久化/两次resume/旧reader未闭环 |
| T19–T23/T24–T31/T32–T35/T36–T37 | 开放 | API/exec、D6、owner、来源/历史、原生/异OS、live按原序推进 |
| T38 | 未达成 | 排除后的三平台nextest均红；缺host工件、sandbox、abort/timeout及未归因断言分别处理 |
| T39/T40 | 开放 | 独立复审修正待验证；可发布工件/回滚和发布范围未验收 |

## 已有失败与未验项

- run6的827个非PASS终态见完整CI ledger；26 TIMEOUT、2 ABORT、1 FAIL+LEAK单列，不用Summary FAIL桶掩盖。
- 旧本机route_aware_client_pool负载窗口失败记录保留；local group和ResolvedDefault opt-in不能证明仍使用ReqwestDefault的默认生产路径已根治。
- T17/T18以及各批开放项继续保留。本轮文档/代码修正没有新测试收据，不声称当前树或CI通过。
