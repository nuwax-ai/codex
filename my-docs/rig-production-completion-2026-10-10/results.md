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
- **R11 决定性证据（T10 DoD 证据侧基本满足）**：
  - 默认 2MiB 线程栈：core::all 30 个历史身份中 50/51 选集 **SIGABRT（栈溢出）**；guardian-v2 语料需 RUST_MIN_STACK≥16MiB；`browser_login_bootstraps_through_system_proxy` 在 tokio worker 溢出（crash report 定位 `Session::new` 单 poll 帧，仅 48 帧深）。
  - RUST_MIN_STACK=8388608：0 abort；同批 50 个转为 ~17s 普通失败（2s 握手等待超时，负载 70-90 窗口）——即 8MiB 消除溢出后，剩余为负载敏感时序。
  - 待做：定位并最小化超大 poll 帧（Box::pin/任务边界单变量实验，见 plan §2.4 R11）；core/tui 身份需安静窗口复验。
- **residency::websocket：当前树仍失败**：rig responses client "builder error"（构建失败），独立线索待查（T07-2/T08 关联）。
- rmcp 2 / exec-server 9：极端负载窗口（62-92）整包大量超时失败，无法作证；批 D 安静窗口 exec-server 616/616 的既有收据仍在。均标记 UNVERIFIED-LOAD，需安静窗口隔离复验。
- tui 45：套件在跑（5642 项，负载下预计 >1h），本批先登记为待安静窗口。

## 40 项状态（更新）

| 项 | 状态 | 证据/边界 |
|---|---|---|
| T03/T04/T05/T06 | 完成 | 批 A/B |
| T02 | 部分（配置完成） | 远端 dispatch 待授权 |
| T07 | 阶段 1 完成 | matcher+双轨+接管开放（阶段 2 需 ipnet 直依 + bazel lock 同步） |
| T11 | **完成** | 13 身份 16/16；grant+scope 级 Compaction 契约 |
| T12 | **完成（测试覆盖）** | 6 新用例 + 矩阵；未发现终局性产品缺陷 |
| T16 | D2 段完成 | 三线提取+迁移；V-D2-1/reducer/key 开放 |
| T10 | 新证据登记 | Session::new 单 poll 帧溢出（guardian-v2 二进制，HEAD 复现） |
| 其余 | 开放 | 按序推进 |

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
