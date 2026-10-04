# Claude 独立复查结果 — checkpoint ead6f2bca（2026-10-04）

复查人：Claude（独立于 Codex 开发轮）。本文件为
`my-docs/claude-checkpoint-review-2026-10-03.md` 的执行结果报告。

## 1. 目标与验证树

- 目标提交（`git log --diff-filter=A` 确认本文档首次入库）：`ead6f2bcae3e0cac4a8374708e9dc1d1d31296a6`，与用户提供的 Codex 回执 SHA 一致。
- 基线：`20898140f2b3638aac6ab328b24d2f604871bfc8`。diff：173 文件，+12,911/−2,390（codex-rs 部分 158 文件，+11,810/−2,283）。
- 实际验证树：`ead6f2bca` 检出、分支 `test`；工作树起始干净（仅未跟踪 SQLite/tmp 运行时产物，未暂存、未清理）。复查期间的修复见 §5（均未 commit）。
- 环境：macOS aarch64（Darwin 27），rustc 1.95；外部负载峰值 load≈87（OrbStack/ToDesk 等外部进程），测试并发与超时判定按该边界单独登记。
- 构建产物：全新隔离 `CARGO_TARGET_DIR=/tmp/codex-review-2026-10-03-target`，未复用旧 `/tmp` 产物；Codex 上轮日志中的数字未用作为本轮证据（仅作对照）。
- 收据核对：目标提交 `codex-exec --build-receipt` 与当前树重算的 source identity 比对（结果见 §4.1）。

## 2. 审查方法

1. 依次阅读 AGENTS.md（根 + tui 子目录）、codex-review-prompt.md、direct-development-2026-10-03-{spec,plan,tasks,targeted-validation}、container-multiprocess-env-review-2026-10-03、provider-request-controls-2026-10-03/{spec,plan,tasks}、field-mapping-audit、FORK.md。
2. 主干人工逐行审查：request_retry / wire_budget / request_capture / wire_auth / transport / stream（pause future）/ responses passthrough / client（pool+retry(never)+provenance）/ core client（cap/WS reuse/provenance 接线）/ sse decoder 终止语义 / hosted_replay v3 envelope 与预算 / credential_instance / model_output_projection / config loader 兼容 / TUI guard / daemon / exec receipt。
3. 五个并行专题代码审查（identity/provenance、credential/auth、responses cap/decoder、env/config/daemon/TUI、live-tests harness+receipt），发现逐条按 file:line 复核后采信。
4. 对照锁定版 rig-core 0.42.0（cargo registry 源码）核对 max_completion_tokens 映射与 Anthropic default_max_tokens 行为。
5. 按仓库要求以 `just test`（nextest，NEXTEST_PROFILE=local，RUST_MIN_STACK=8MiB）执行定向验证；完整 workspace 未运行（按 AGENTS 需单独授权）。

## 3. 代码审查发现（P0/P1/P2）

本轮确认并修复 **1 个 P1（R3）和 3 个 P2（R1/R2/R4）**；未确认新的 P0 功能缺陷。上下文预算的 P0 人工复审要求与已修复缺陷分开登记。其余登记项和 P3 备忘列后。

### 已直接修复（见 §5）

| # | 级别 | 位置 | 问题 | 处理 |
|---|---|---|---|---|
| R1 | P2 | `tui/src/session_archive_commands.rs`（remote 分支）、经 `session_queue_commands.rs` 影响 `codex queue --remote` | 新增的「TUI `--remote` + 活跃 NUWAX 组 fail-fast」未覆盖 session-command 启动面：该路径在 guard 之前直接 early-return，本地活跃 NUWAX 组 + `codex queue/archive/delete --remote` 会静默改由远端 provider 解析会话。 | 在 remote early-return 分支加入与交互启动完全相同的 fail-fast（同一谓词、同一错误文案、同一 NUWAX 解析优先级），未引入 seeds 注入等新行为。回归：谓词正/负用例（app_server_target_tests）。 |
| R2 | P2 | `tui/src/lib.rs:1032`、`tui/src/daemon_startup.rs:47` | 远程 guard 与 daemon 排除把保留 id 硬编码为字符串字面量 `"model_providers.nuwax_env"`，与 `codex_protocol::config_types::NUWAX_ENV_PROVIDER_ID` 脱钩；常量漂移时 guard/排除静默失效而配置层隔离仍生效。 | 抽出共享谓词 `nuwax_env_provider_seed_active`，由协议常量构造保留键，三处（lib guard、daemon exclusion、session-command guard）共用。 |
| R3 | **P1** | `codex-rust-rig-bridge/src/stream.rs:235`（对 `hosted_replay.rs:13 MAX_PAIR_BYTES` 的复用） | `MAX_PAIR_BYTES` 从 40,960 收紧到 9,800 时，把**无关语义**的 pause 续接内容硬失败预算一并缩小到约 1/4：任何单条暂停 assistant 消息（thinking+签名+文本+搜索块）序列化超 9.8KB 即整个 turn 报错（"Paused content exceeds the raw replay byte budget"）；检查点之前 40,960 内可正常续接。规格只要求 envelope/layout 为 9,800。thinking 块数 KB 即可触发，属真实回归风险。 | 拆出独立 `MAX_PAUSE_CONTENT_BYTES = 40_960`（保持检查点前数值与 fail-closed 语义），`MAX_PAIR_BYTES=9,800` 只作用于持久化 envelope/layout/reasoning 丢弃。回归：`paused_content_between_envelope_and_pause_budgets_still_continues`（20,000 字节内容续接成功、逐字回放），既有 41,000 fail-closed 测试继续钉住上界。 |
| R4 | **P2** | `codex-rust-rig-bridge/src/transport_identity.rs:120-185` | 迟到搜索结果的外来 call 克隆（`call_index = UNKNOWN_WIRE_INDEX`，仅当原 call envelope 被预算整体丢弃/缺失时保留）在**成功**的身份 rebuild 中被静默吞掉：`splice_identity` 把它标记为 handled，但 `rebuild_response` 的 pair 双射只覆盖有 wire index 的位点——重建后线上出现孤立的 `web_search_tool_result`（tool_use_id 指向不存在的 server_tool_use），Anthropic 线网关会拒绝或至少丢失该调用，违反「每个接受的调用块恰好出现一次」。失败路径（fallback）反而会保留 [call, result]。 | rebuild 成功时在响应块前部补发该响应下全部 UNKNOWN-wire 位点的块（envelope 校验已保证其为 server_tool_use），使 call 先于其 result 同消息出现；不参与 pair 双射。回归：`successful_rebuild_keeps_unindexed_foreign_call_clones_with_their_result`。 |

### 登记、未修复（判定为行为/文档项而非小缺陷；理由附后）

| # | 级别 | 位置 | 问题 | 判定 |
|---|---|---|---|---|
| N1 | P2 | `live-tests/src/artifacts.rs:196-235` | 声称的「dirty 指纹精确比对」未实现：manifest 记录 harness `git_dirty` 但从不与 receipt 交叉校验；实际比对是完整 HEAD SHA + 全部在范围内构建输入的内容摘要（比旧「rev 前缀+dirty」更强）。 | 内容摘要已覆盖所有参与构建的未跟踪文件，out-of-scope 文件（sqlite/tmp/logs）不可能影响构建产物，dirty 交叉校验属冗余；属文档措辞与实现的偏差（verification.md 旧段）。不改代码；在本报告更正表述。 |
| N2 | P2 | `exec/src/build_source_identity.rs:161-179` | 指纹包含 codex-rs/ 下所有未忽略未跟踪文件（含 live-tests/tests/fixtures 录制品，未 gitignore）。构建后录制 fixture 会让 receipt 与树失配。 | 这是内容绑定收据的诚实语义（树变了→二进制不再匹配树），失败是显式「rebuild」而非静默；cargo rerun-if-changed 会自动重建自愈。登记为已知工作流边界，不改。 |
| N3 | P2 | `codex-rust-genai-bridge`（整个 crate） | genai 桥不消费 `max_output_tokens`：`experimental_bridge="genai"` + 配置 cap 时 Chat 线不发 cap、Anthropic 线缺 `max_tokens` 可能 400。 | 预先存在（genai 从未消费该字段）、genai 已搁置（LIVE_INCLUDE_GENAI 才启用）；本轮 checkpoint 对 genai 仅构造器迁移。属登记缺口，不为搁置桥新增功能。 |
| N4 | P2 | `tui/src/session_archive_commands.rs`（daemon 复用路径） | 本地 session-command 仍不注入 env seeds，daemon 选择只用 `config_exclusion`。按名字查询时 `model_provider=None` 被服务端解释为其默认 provider 过滤，因此默认 openai daemon 下可能找不到 nuwax_env 会话；UUID queue 绕过列表过滤，而且 queue-add 本身不加载 provider，不能断言它必然报 provider 缺失。 | 预先存在；需要先明确行政查询与 queue 的服务器归属、跨 provider 查找和同名歧义策略。不能仅改成 seed-aware exclusion：运行中 daemon 存在时另启 embedded queue 已被禁止。见 Codex 2026-10-04 复查及后续 Spec/Plan/Tasks。 |
| N5 | P2 | `app-server/src/request_processors/thread_processor.rs:230-247` | resume 时 `config:{model_provider:X}` 与持久化 provider 相同的 echo 会跳过 merge，持久化模型被启动默认模型替换（显式路由语义的有意变更）。 | 本轮设计决策（显式覆盖优先），文档已声明；登记行为变化提醒嵌入方。 |
| N6 | P2 | `utils/cli/src/nuwax_env.rs:111-117` | 孤立 `NUWAX_MAX_OUTPUT_TOKENS`（组未激活）即使显式 `-m` 也会启动失败，与 `NUWAX_MODEL` 在 `-m` 下的抑制不对称。 | 有意 fail-fast 且有测试钉死（`standalone_output_cap_requires_the_complete_group_but_unselected_groups_are_ignored`）；不改。 |
| N7 | P2 | `request_retry.rs:66` | `RetryLimit` 兜底分支实际不可达（`should_retry` 在末次尝试返回 false，真实末次 HTTP 错误被保留并有 wire 测试钉死）。 | 防御性死代码，与 native `run_with_retry` 同形；不动。 |
| N8 | P2 | `responses.rs` idle_timeout 现在同时限定 Retry-After 等待 | 超过 `stream_idle_timeout` 的 Retry-After 会以 Timeout 上抛。 | 本轮注释已明示该语义；属已知组合边界。 |
| N9 | P2 | `codex-api/src/model_source.rs:13-58`（endpoint digest 的 query 凭据排除表） | query 凭据识别是**名称 denylist**：识别表覆盖 `authorization/api_key/.../_key/_token/_signature` 等常见名，但网关若用未列入的名称（如 `sessionkey`、`sig`、`hmac`）传凭据，其**值**会被哈希进持久化的 endpoint_identity（rollout 可读）——低熵密钥可被离线字典攻击还原（其余 preimage——URL/路由——攻击者自己就有）。已核验：当前 core 侧检测集是 api 侧排除集的子集，**今天没有**任何名称同时「被识别为凭据」又「进入摘要」，无现行泄漏；但「无秘密进入摘要」的绝对表述仅对已识别名称成立。 | 登记为加固项：未识别 query 名应保守处理或合并两表（见 N10）。改摘要算法会改变 replay source 兼容性，非小修复。 |
| N10 | P2 | `model_source.rs:33-58` vs `core/src/model_output_projection.rs:147-171` | 凭据名称 denylist 手工维护了两份（api 侧摘要排除表、core 侧凭据检测表）；单边漂移会直接制造 N9 的泄漏形态。 | 登记后续项：从 codex-api 导出单一共享判定；本轮不改（两表现值等价已核验）。 |
| N11 | P2 | `core/src/model_output_projection.rs:111-118` | credentialInstance 比较范围是 `provider.headers + auth 快照`，**不含**每轮 `extra_headers`；已核验当前所有 extra header 均非鉴权（originator/trace/attestation 等），今日无分叉，但未来新增含凭据的 extra header 时 wire 与身份可静默背离，旧 opaque replay 可能跨越凭据变更。 | 登记加固项（把非良性 extra-header 名并入比较或断言名称集合）。 |
| N12 | P2 | `codex-rust-rig-bridge/src/client.rs:174-181`（`reasoning_source` 兼容包装） | 旧入口硬编码 `legacy-unscoped`，使所有调用方共享一个跨凭据 replay 域；仓内已无生产调用（仅测试），属休眠 foot-gun。 | 登记后续项（删除或标注 test-only）；spec 已声明其为源码兼容入口的边界。 |
| N13 | P2 | `core/tests/suite/rig_anthropic_hosted_tools.rs:928-931` 挂接的 credential/identity 子模块受 `#![cfg(feature = "rust-rig")]` 门控 | `just test -p codex-core` 单包运行**匹配 0 个**这些测试（已实测复现）；仅当同一 cargo 调用选中 cli/app-server/exec（feature 统一开启 rust-rig）或显式 `--features rust-rig` 时才执行。按 AGENTS 的「改哪个包测哪个包」流程复核本检查点会静默跳过其核心测试。 | 已知 feature-unification 锐边：本次复查的 core 批次与 Codex 轮次同样以 `-p codex-core -p codex-app-server -p codex-exec` 联合运行规避；登记验证要求（单包验证 core 桥接面必须显式 `--features rust-rig`）。 |

### P3 备忘（不处理）

- `rollouts.rs:158` 验证器只查 `input.is_object()`，不强制 `query`/`search_query` 键存在（生产桥读取任一键）。
- receipt 状态命名实际为 `invalid/unresolved/mismatch/source_and_package_validated`；"unknown" 是字段哨兵不是状态。
- `exec/build.rs:51` `ends_with("codex-rs")` 可能过度 watch 同名根目录（仅重建噪声）。
- `bridge_turns.rs` 测试新增 `.expect()` 在 crate 级 `#![allow(clippy::expect_used)]` 下，风格与存量一致。
- Claude 本轮未覆盖 `session_archive_commands.rs` 的 `--remote` 实际启动分支。Codex 后续补了 CLI 子进程回归；子进程 `Command::env` 不修改测试 runner 的环境，因此仓库约定不阻止这种验证。最新执行结果见 `codex-review-2026-10-04.md`。

## 4. 验证证据

（所有命令、退出码、计数为实际执行记录；外部负载 load≈45-87 贯穿全程。）

### 4.1 二进制重建与收据核对

- 全新 `CARGO_TARGET_DIR=/tmp/codex-review-2026-10-03-target`，`cargo build -p codex-exec --bin codex-exec --offline --locked`：**exit 0**（26m07s，`CARGO_BUILD_JOBS=4`）。
- 二进制 SHA256：`90db1d0235b8ad7135983ac072f55b3c5515a2adc67492d4f54c1225e7f8b950`（505,3xx,xxx bytes debug）。
- `--build-receipt`：`git_sha = ead6f2bcae…96a6`（= 目标提交完整 SHA）；`target=aarch64-apple-darwin`、`profile=debug`、`source.algorithm=git-build-input-content-sha256-v1`；`git_dirty=dirty`（未跟踪 sqlite/tmp 运行时产物导致，符合预期——dirty 位从不参与校验判定）；`dependency_features=unknown`（如实申报，非供应链证明）。
- **独立复算**：用 Python 重实现同一算法（git ls-files --cached+--others --exclude-standard → is_build_input 过滤 → 逐文件 len 前缀+内容 SHA-256，含符号链接分支），当前树计算得 `fe521cd9b4cb1d41cda62ee267846133cd700d8bd5246d75427d8edecc913f82`，**与收据逐字节一致**（inputs=8648；含 vendor 符号链接 1 处）。
- 时间线说明：build.rs 在本复查的 4 个 tui 修复文件落盘之后运行，因此该收据绑定的是「目标提交 + 复查修复」的工作树（正是待验证树；4 个文件均不在 exec 请求路径上）。若后续再有源码改动，live 前按同一流程重建并复核。
- 收据设计核对（§3 N1/N2 结论来源）：digest 覆盖全部未忽略未跟踪构建输入；sqlite/tmp/logs/.env* 被排除（运行时产物不影响指纹）；manifest 侧（live-tests artifacts）与 build.rs 共享同一 `build_source_identity.rs`（#[path] 引用），两侧算法不可能分叉；dirty 位记录但不参与比较。

### 4.2 离线定向测试（just test / nextest，隔离 target，`--offline --locked --retries 0 --test-threads 2`，`CARGO_BUILD_JOBS=4`）

批次 2 — core/app-server/exec 定向（三包联合以统一 rust-rig feature）
: `just test -p codex-core -p codex-app-server -p codex-exec -E 'test(rig_anthropic) | test(rig_responses_bridge) | test(model_output_projection) | test(stream_events_utils) | test(nuwax_) | test(build_receipt) | test(listen_off_honors_persisted_remote_control_enable) | test(config_manager) | test(thread_resume) | test(websocket_incremental_reuse)' …`
: **exit 100：190 run / 179 passed / 11 failed / 6816 filter-skipped**（测试段 434.9s）。通过面覆盖本轮全部核心断言：rig_anthropic hosted/identity/credential 全套、rig_responses_bridge（含 cap 耗尽不重采样、resume 不回填 provenance）、model_output_projection、build_receipt（非 UTF-8 argv 不 panic、单独使用校验）、nuwax 正向/隔离/并发双进程、config-only resume 覆盖、WS cap 复用。
: 11 个失败全部是**既有** `thread_resume_*`（10 项）与 `model_auto_review::thread_resume_and_fork_upgrade_legacy_protected_model_settings`（1 项），失败耗时均 10.5-17.7s（10s 公共事件等待 Elapsed 形态）；该 11 项与本 checkpoint 的 diff 无交集（thread_processor 仅 +1 行显式路由识别，其新增测试已通过）。已按隔离串行复验（批次 7），结果见下。

批次 3 — tui/daemon 定向（含 R1/R2 修复；V8 helper 用已校验 pin pair）
: `RUSTY_V8_ARCHIVE=… RUSTY_V8_SRC_BINDING_PATH=… just test -p codex-tui -p codex-app-server-daemon -E 'test(app_server_target) | test(daemon) | test(nuwax) | test(output_token_limit_error_snapshot) | test(remote_nuwax)' …`
: 首轮 exit 101：R2 谓词一处 `&String == String` 编译错误（被 30 分钟后台超时杀掉的先行 `cargo check` 未能提前暴露，属复查修复自身笔误）。修正 `*key == reserved` 后重跑：**exit 0：53 run / 53 passed（1 slow 42.8s）**，含新谓词回归 `nuwax_env_provider_seed_active_detects_the_seed_group_regardless_of_site`、remote guard 快照（错误文案未变）与 daemon 排除用例。

批次 7 — 批次 2 的 11 个失败项隔离串行复验（`--test-threads 1`）
: **exit 100：11 run / 10 passed / 1 failed**。10 个 `thread_resume_*` 全部通过（复验支持「并发+外部负载下 10s 事件等待超时」的判定，非回归）。唯一持续失败：`model_auto_review::thread_resume_and_fork_upgrade_legacy_protected_model_settings`——失败于子进程 app-server `initialize` 的 10s deadline（日志明示 `writing message to stdin: initialize … Error: deadline has elapsed`）；该文件在本 checkpoint diff 中**零改动**，属既有子进程套件在外部持续负载（load≈45-60，全程其它会话重编译竞争）下的已知环境边界（本仓 memory 已登记同类），与 Codex 轮次低载通过记录一致；单独重试结果见 §4.4。

批次 4 — bridge_live 二进制内离线单元（LIVE_VENDORS=" " 强制跳过厂商场景）

: `LIVE_VENDORS=" " just test -p codex-live-tests --test bridge_live …`
: **exit 0：84 run / 84 passed**；其中厂商 step_* 用例按设计 no-op 跳过（vendor()=None），离线单元（history/controls 断言）实际执行。

批次 5 — cassette 离线回放（LIVE_CASSETTE=replay）
: `LIVE_CASSETTE=replay just test -p codex-live-tests --test bridge_live …`
: **exit 0：84 run / 84 passed**；fixture 反序列化/截断/缺失按审计文档直接失败的语义保持，无失败。

批次 6 — rig-bridge 全 crate 复跑（含 R3/R4 修复与两个新回归）
: `just test -p codex-rust-rig-bridge --offline --locked --retries 0 --test-threads 2`
: **exit 0：241 run / 241 passed**（26.9s；241 = 批次 1 时点 239 + 新增 2 个回归；`paused_content_between_envelope_and_pause_budgets_still_continues` 与 `successful_rebuild_keeps_unindexed_foreign_call_clones_with_their_result` 均执行并通过，既有 pause/identity/replay 测试无一破坏）。


批次 1 — bridge/api/config/model-provider/utils-cli/history/live-tests lib
: `just test -p codex-rust-rig-bridge -p codex-api -p codex-config -p codex-model-provider -p codex-utils-cli -p codex-history -p codex-live-tests -E 'not binary(exec_live) & not binary(bridge_live)' …`
: **exit 0：1042 run / 1042 passed / 1 skipped**（62.7s 测试段；含 H2 REFUSED_STREAM 负控 3/1 与生产 1/1、1/1、2/2 断言，retry 逐次采集/末次错误保留/429/transport/取消、三协议 cap wire、auth 归一化 wire（匿名/网关 Basic/Token/显式空 key/非 ASCII key 拒绝）、identity v3 replay wire、capture 脱敏/容量、wire_budget、credential_instance、model_source、loader 六参兼容、env 组隔离、live-tests lib 含收据/工件矩阵）。

### 4.3 真实厂商最小验证（最终树二进制，收据绑定后执行）

前置：R1-R4 修复 + scoped `just fix`（exit 0，无自动改动，16m51s）+ `just fmt`（exit 0）后，重建 `codex-exec`（exit 0，6m47s）。新收据 `git_sha=ead6f2bca…`、source digest `831b07e4b56f300f353246476a82986720add937f1b013ae42bc3e17dfd992c8`，独立 Python 复算**逐字节一致**；二进制 SHA256 `93ae5891b262a2e23650110732f32ab5234de92ccfc1d46c9fb988991bc0bd04`。凭据仅经 `.env.local` 由 harness 自行加载（密钥未出现在任何命令行/日志/本文档）。

命令：`env CARGO_BIN_EXE_codex-exec=… LIVE_VENDORS=mimo,glm just test -p codex-live-tests --test exec_live -E 'test(/^(mimo|glm)_(chat_rig|anthropic_rig|responses_rig_default)$/) | test(glm_websearch_anthropic_rig)' --offline --locked --retries 0 --test-threads 1`

- **exit 0：7 selected / 7 executed / 7 passed（1 slow）/ 24 filter-skipped**，run ID `525e5297-653b-47ec-bb9f-ef0dfb7e6720`，测试段 501.5s。
- 通过：GLM anthropic/chat/responses marker、MiMo anthropic/chat/responses marker、GLM websearch 双轮（109s）。每项 marker = 真实工具执行（不可伪造 nonce、exit 0）+ 回答。Codex 复查发现这七份 `request-capture-evidence.json` 均为 `wire_asserted=false`：harness 解析并计数捕获记录，但没有逐项断言实际 HTTP path/model；捕获格式不记录 headers，因此不能据此宣称 live 鉴权头已断言。鉴权和请求字段的确定性断言来自独立离线 wire 套件。
- 工件证据（logs/live-*，本轮新建）：manifest `status=source_and_package_validated`、`source_validated=true`、`binary_unchanged_during_receipt=true`、source digest 与上文一致；GLM websearch `requests.jsonl` 恰 2 个 attempt（每轮一个），search-evidence：turn1 1 个匹配完成对 + 614 字符回答、turn2 1 个匹配完成对 + 55 字符回答；`citation_capability_asserted=false`（0/0 引用，如实不升级为加密引用验收）。
- 本轮 live 未执行：compact/上下文边界付费压力、cap 实际截断、Step、bridge_live 在线矩阵、genai。按指示控制调用量，未盲跑。

### 4.4 环境边界与单独重试

Codex 2026-10-04 证据口径补充：以下采样支持“等待消耗在进入 Rust 用户代码之前”，热/冷探针支持启动成本不稳定；单凭 `_dyld_start` 栈、二进制大小和 CodeDirectory 页哈希数量，尚不能定量证明究竟多少耗时来自签名校验、动态链接或换页。因此下面的具体成本归因应视作待进一步 profiling 的解释，而非已完成因果排除。diff 无交集及热启动更快也不等于已完成新旧二进制低负载冷启动对照。原失败和未放宽 deadline 的处理保留。

- `model_auto_review::thread_resume_and_fork_upgrade_legacy_protected_model_settings`：批次 2 失败 → 隔离串行失败 → 单独重试（load≈19 与 ≈48 各一次）仍失败，耗时恒为 ~10.7-11.0s。**根因已实测定位（非推测）**：
  - macOS `sample` 抓取卡住子进程的调用栈：整个采样期（2.6s/2606 样本）全部停在 `_dyld_start`，物理内存占用仅 112K——**子进程从未进入任何 Rust/用户代码**，10s initialize 预算全部消耗在 exec 阶段（dyld 动态链接 + adhoc 代码签名校验 + 冷页换入）。
  - 该二进制为 521MB debug Mach-O、`Signature=adhoc (linker-signed)`、CodeDirectory 含 **126,368 个页哈希**——macOS 在 exec 时逐页校验，成本与负载正相关；手工探针实测同一二进制 initialize 延迟呈双峰：热 0.36s / 冷争用 6.7-20.5s，正好横跨 10s 预算线。
  - 预算不对称实锤：该测试与 mac 默认 `DEFAULT_REQUEST_TIMEOUT` 均为 **10s**（Windows 分支因 exec 慢而给 **25s**）；`nuwax_positive` 显式 30s，实测 14.8-30.2s 恰好越过 10s 线仍通过。批次 7 中本测试排第 1（首个冷启动子进程）失败、其后 10 个 thread_resume 复用已热二进制全部通过——冷启动惩罚集中落在首个子进程。
  - 与 checkpoint 无关的实证：`model_auto_review.rs` 在 diff 中零改动；新旧二进制同机对比（旧=20898140f 工作树构建）warm initialize 0.8s vs **0.36s（新更快）**——checkpoint 未增加启动成本（收据为编译期、capture 默认关闭、config 仅 +1 显式通道）。
  - 处置：按「不放宽 deadline 制造绿灯」保留失败登记不改测试。如需根治属上游设计问题（mac 非 Windows 分支是否也应 25s），已列入后续任务。
- 全程外部负载 load≈36-87（其它会话的 duck-* rustc 重编译与本机 OrbStack 等持续竞争）；多个测试 >30s slow 但在自身预算内通过。批次 2 的 10 个 thread_resume 失败已由隔离串行复验（批次 7）证实为并发负载超时，非回归。
- 首轮批次 2/3 曾被本会话后台任务 30 分钟默认超时终止（未影响结果正确性，已用 2h 超时重跑取得上述数字）；被杀的先行 `cargo check`（本可提前暴露 R2 的类型笔误）教训已在本报告如实记录。



### 4.5 复核代理独立验证（补充证据，标注为子代理执行）

- codex-api + rig-bridge + model-provider（`just test -p …`）：**553/553 pass**（含 wire auth 归一化全量用例）。
- codex-core（显式 `--features rust-rig`，credential_instance + identity 过滤）：**4/4 pass**，含端到端 `anthropic_actual_credentials_keep_signed_replay_and_rotation_drops_only_opaque`（换凭据 resume 只降级 opaque、rollout append-only、持久化仅 authDomainKind=credentialInstance、key 值不落 rollout）。
- rig-bridge 全量 239/239 与 core model_output_projection 9/9（修复前树）。这些与主批次分别登记，不与主批次数字合并为单一用例数。

## 5. 修复可提交性与审查 diff

- 复查修复共 8 个文件、+156/−11（`git diff --check` 干净；`just fix` 无自动改动；`just fmt` exit 0；**未 stage、未 commit、未 push**）：
  - `codex-rust-rig-bridge/src/stream.rs` — R3 独立 pause 内容预算常量。
  - `codex-rust-rig-bridge/src/transport_identity.rs` — R4 保留无 wire index 的外来 call 克隆。
  - `codex-rust-rig-bridge/src/{transport_identity_tests.rs, tests/wire/pause_turn_tests.rs}` — 两个新回归。
  - `codex-tui/src/{lib.rs, daemon_startup.rs, session_archive_commands.rs, app_server_target_tests.rs}` — R1/R2 guard 补面 + 共享谓词 + 回归。
- 建议提交分组：(1) rig-bridge R3+回归；(2) rig-bridge R4+回归；(3) tui R1/R2+回归。三组均可独立构建（批次 6 与批次 3 分别验证过两组所在 crate 全量）。
- 其余登记项（N1-N13、P3）不建议随本复查提交混入；按 §7 优先级另立。

## 6. 字段支持与协议限制现状（以本轮源码+测试为据）

| 面 | Responses 直传 | Chat | Anthropic | 依据 |
|---|---|---|---|---|
| `max_output_tokens` | typed HTTP/WS/Rig body/预算同值；None 不加键；请求 cap > provider fallback | `max_tokens`（rig 0.42 对 OpenAI reasoning 模型自动转 `max_completion_tokens`） | `max_tokens`；未配置默认 16384 | wire 测试三协议 + WS reuse 比对 + core 构造器唯一来源 |
| 输出耗尽 | `incomplete(max_output_tokens)` → 不可重试 InvalidRequest（提高 cap 提示），HTTP/S 采样 retries=3 也仅 1 次请求；不能被 pending/断网/completed 覆盖 | — | — | decoder 单测 + `responses_bridge_output_cap_exhaustion_does_not_resample` + 三种尾部状态回归 |
| HTTP retry | 握手层 provider `request_max_retries`（0=仍发一次；429/5xx/transport flags、Retry-After、取消、逐 attempt 采集、保留末次真实错误）；reqwest 内层（含 H2 NACK）`retry(never)` 关闭（负控 wire3/capture1 vs 生产 1/1、1/1、2/2）；成功取得 SSE 后不再自动重放；与 Core stream 采样重试分开计数 | 同左 | 同左 | wire retry 六场景 + h2 单测 + client_retry_tests |
| 凭据/鉴权 | 无主凭据不合成空 Bearer；显式空值保留存在性；Basic/Token 网关 Authorization 原样；非 ASCII key 发前拒绝且不回显 | 同左 | x-api-key/Authorization 按解析结果置/删 | auth_normalization wire（3 协议×多形态真实 TCP） |
| provenance/回放 | source=endpoint(脱敏 query)+model+auth-domain(kind)；account/credentialInstance/selector/anonymous 分级；selector 永不回放 opaque；跨进程新实例 ID 保守降级；持久化数据只投影不改写 | Chat 保留投影后可见 reasoning_content（>9.8KB 降级告警） | v3 response/segment 身份；签名/密文不截断整体丢弃；v1/v2 仅 pair 回放不注入引用 | model_output_projection 单测 + core 身份/凭据集成 + wire identity_replay |
| hosted 搜索回放 | 不适用 | 丢弃并告警 | ≤64 对、单 envelope 9,800B、64 layouts、64KiB 总量；覆盖校验失败降级 pair-only；跨响应配对/去重/迟到结果（含本轮 R4 修复） | hosted_replay(_budget)/transport_identity 单测 + wire + GLM 真实双轮 |
| 请求捕获（诊断开关） | `CODEX_RIG_REQUEST_CAPTURE_FILE`：逐 attempt、最终 body raw、URL userinfo/query 值脱敏、无 headers、0600、2MiB/body、64 attempts、16MiB 总量、超限/IO 显式失败 | 同左 | 同左 | request_capture 单测 + wire capture 用例 |
| 预算护栏 | 32MiB 硬字节 → InvalidRequest；已知窗口 bytes/4+输出预留 → ContextWindowExceeded（compact 可识别）；窗口非正/≤输出 cap → 配置错误 | 同左 | 同左 | wire_budget 单测 |

新增/残余功能遗漏：genai 桥 cap 不消费（N3）；临时 NUWAX provider 的 retries/timeout/headers 等 direct env 开关未开发（P2 登记项，容器文档 §6 已列）；RateLimits/ModelsEtag、Text.additional_params 细字段仍按 field-mapping-audit 登记为未承载。

## 7. 未执行 / 阻断门禁与后续任务

未执行（按指示或授权门禁）：
- 完整 workspace 测试（AGENTS 要求单独授权）。
- 远程 CI（fork-cargo-pr.yml / live-tests.yml dispatch）、Windows/Linux 矩阵、npm 发布——均需授权。
- live 付费扩展矩阵：compact/上下文边界、cap 实际截断压力、Step 厂商、bridge_live 在线、genai、加密 citation 字段级保真（本轮 live 仅 §4.3 列明范围）。
- Bazel 构建（仅做了 `just bazel-lock-update` 无漂移核验；lock 同步 ≠ Bazel build 通过）。

后续任务（按优先级，含完成标准）：
1. **R3 相关**：将 pause 内容预算与 envelope 预算在 spec/verification 文档中显式分为两个独立条目（防止未来再耦合）；完成标准：direct-development spec 更新 + 常量注释已就位。
2. N9/N10：统一两处 query 凭据 denylist 为 codex-api 单一导出，并对未识别 query 名选择保守策略；完成标准：新共享函数 + 「未识别名不得进摘要」回归 + source 兼容性评估记录。
3. N11：credentialInstance 比较并入非良性 extra-header 名；完成标准：比较覆盖断言回归。
4. N4：先明确跨 provider 名字查找和 queue 的服务器归属，再处理 seeds/daemon 策略；完成标准：UUID/名字、同名歧义、运行中 daemon/embedded 的回归矩阵，禁止第二个 queue writer。仅接入 seed-aware `exclusion()` 不算完成。
5. N13 验证流程：在 AGENTS 或测试 README 登记「core 桥接面单包验证须 `--features rust-rig`」；完成标准：文档条目 + 至少一条带该 flag 的 CI/本地流程示例。
6. GLM websearch 跨进程 opaque 回放、MiMo hosted 搜索（网关能力）、genai cap——维持既有登记。

## 8. 结论

- 检查点整体质量高：请求控制主干（cap 一致性、retry 预算与采集、终止单调性、匿名鉴权、capture 脱敏、provenance 分级、v3 身份回放、收据绑定）在源码与测试两个层面均与文档声明一致；发现并修复 1 个 P1（R3 pause 预算连带缩水）与 3 个 P2（R1/R2/R4），全部带可复现回归。
- 一切验证基于目标提交 + 复查修复的最终树（收据逐字节绑定、live 工件 `source_validated=true`）；离线 6 批次 + live 7/7 通过；唯一未通过项为与本 checkpoint 无关的既有子进程超时环境边界（已单独复验并如实登记）。
- 可交付状态：8 文件审查 diff 未提交，交用户确认后按 §5 分组提交；未 push、未发布。
