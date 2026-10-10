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
