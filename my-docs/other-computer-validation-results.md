# 另一台电脑验证结果（2026-10-05；2026-10-06 独立复审追加）

新机（本文档所在机器）对已推送 checkpoint 的独立复验与本轮补齐记录。入口：`other-computer-handoff-2026-10-05.md`。2026-10-05 Claude 轮未 commit/push。2026-10-06 用户另行授权独立审查、修复及阶段 commit；本轮不 push。下方历史结果不自动升级为当前树证据。

## 2026-10-05 第一轮环境与源码身份（历史）

- 机器：macOS（Darwin 27.0，aarch64），18 逻辑核。仓库卷 `/Volumes/soddygo`。
- 工具链：rustc/cargo 1.95.0（仓库 pin，rustup 自动安装）、cargo-nextest 0.9.146、`just` 1.58.0（本机新装，brew）。cargo-insta 未装（本轮无快照变更）。第一轮时 **bazel/bazelisk 未安装**（仓库 pin `.bazelversion` 9.0.0）；第二轮已安装并构建，见下方记录。
- 隔离 `CARGO_TARGET_DIR=/Volumes/soddygo/git-workspace/codex-targets/validation-20261005`，各批次 package/feature 图与命令一致（core 图显式 `--features codex-core/rust-rig`）。
- 安全同步：工作区干净 → `git fetch origin test` → 已在 `test` → `git pull --ff-only origin test`（Already up to date，无分歧）→ **HEAD `7b891587981723fcd42f90bac0773387d57de005`**，交接文档首次加入提交即此 HEAD。
- 工作树改动（全部为测试与测试注册，**无生产逻辑改动**；live 收据按 dirty 树重建并绑定）：
  - `codex-rs/cli/tests/nuwax_session_remote.rs`（B3 四命令化）
  - `codex-rs/exec/tests/suite/nuwax_env_stream_wait.rs`（新增）+ `suite/mod.rs`（注册）
  - `codex-rs/app-server/tests/suite/v2/thread_resume.rs`（另一连接对照）
  - `codex-rs/live-tests/src/binary_turns/runner.rs`、`scenarios.rs`、`binary_turns.rs`、`src/lib.rs`、`tests/exec_live.rs`（cap 触顶场景）
- `.env.local` 本机新建（`git check-ignore` 确认命中 `.gitignore:53 .env*`，chmod 600），三厂商凭据来自用户本轮聊天提供；凭据不出现在本文档、工件引用或 commit 中。live 工件 `logs/live-*/` 的 manifest.json 仅含 vendor/model/SHA，无凭据（已核对）。

## 批次证据

| 批次 | 源码摘要 | 完整命令（除前缀 `just` 于仓库根、`CARGO_TARGET_DIR` 如上） | selected/executed/asserted | pass/fail/skip/timeout | 首轮失败与复验 | 工件/日志 |
|---|---|---|---|---|---|---|
| 1. 基线复验（10 包图，与 push 文档同过滤器） | HEAD 7b8915879 + 本轮测试改动前 | `just test -p codex-api -p codex-rust-rig-bridge -p codex-live-tests -p codex-exec -p codex-cli -p codex-tui -p codex-config -p codex-utils-cli -p codex-core -p codex-app-server --features codex-core/rust-rig -E '<push 文档 10 包表达式>' --locked --retries 0 --test-threads 2` | 628 run / 12,996 filtered；wire/cap/owner/remote 断言全执行 | **628 pass / 0 fail / 0 skip / 0 timeout**；exit 0；测试 52.3s（含冷构建全程约 6m） | 无首轮失败 | /tmp/codex-oct05-newpc-baseline.log |
| 2. 新增 B3/C2 测试 + 既有 nuwax 图 | 含本轮 exec/cli 测试改动 | `just test -p codex-exec -p codex-cli -E '(package(codex-exec) & test(nuwax_env)) \| (package(codex-cli) & binary(nuwax_session_remote))' --locked --retries 0 --test-threads 2` | 22 run（4 session_remote + 8 nuwax_env + 8 controls + 2 新 stream_wait） | **22 pass / 0 fail**；exit 0；25.7s | 无失败。inactive 测试 0.11s 完成四命令曾疑假通过：经查同进程内 codex 子进程热 spawn ~8ms/命令（未改动的 incomplete 四命令测试同为本机 ~14ms/命令），且每轮必须在 30s 内 accept 到真实连接否则失败——覆盖真实，非跳过 | /tmp/codex-oct05-newpc-newtests.log |
| 3. app-server 新对照 + live-tests 库 | 含本轮 thread_resume/live-tests 改动 | `just test -p codex-app-server -p codex-live-tests -E '(package(codex-app-server) & test(thread_resume_echo_override_is_ignored_when_another_connection_subscribes)) \| (package(codex-live-tests) & not binary(exec_live) & not binary(bridge_live))' --locked --retries 0` | 43 run（含 FakeRunner capture/pipe/retention 套件，覆盖 runner.rs 改动） | **43 pass / 0 fail**；exit 0；6.1s；新对照单测 3.6s | 无 | /tmp/codex-oct05-newpc-appserver-ws.log |
| 4. live 矩阵（11 场景） | 重建 codex-exec：source sha `864ced315b84…`、binary sha `872753342c8f…`、receipt `source_and_package_validated`（11 场景同源同二进制） | 先 `cargo build -p codex-exec --bin codex-exec --locked`（codex-rs 目录）；`just test -p codex-live-tests --test exec_live -E 'test(/^(mimo\|glm)_(chat_rig\|anthropic_rig\|responses_rig_default)$/)\|test(glm_websearch_anthropic_rig)\|test(/^step_(chat_rig\|anthropic_rig)$/)\|test(/^(mimo\|glm)_chat_rig_cap_exhausted$/)' --retries 0 --test-threads 1` | 11 run / 26 filtered；逐场景 wire 断言 url.scheme/authority/path/body.model（+cap 字段） | **11 pass（1 slow）/ 0 fail**；exit 0；236.0s | 无 | /tmp/codex-oct05-newpc-live.log；logs/live-{glm,mimo,step}/ |

批次 4 明细（capture attempts 为捕获计数，非服务端收件证明）：

| vendor | scenario | attempts | 备注 |
|---|---|---|---|
| glm | anthropic-rig / chat-rig / responses-rig-default | 各 2 | marker 闭环 |
| glm | websearch-anthropic-rig | 7 | 跨轮搜索回放 |
| glm | chat-rig-cap-exhausted | 1 | **触顶**，见下 |
| mimo | anthropic-rig / chat-rig / responses-rig-default | 各 2 | marker 闭环 |
| mimo | chat-rig-cap-exhausted | 1 | **触顶**，见下 |
| step | anthropic-rig / chat-rig | 2 / 3 | 用户本轮提供凭据；Responses 无端点按规则跳过（未计数） |

**真实 live cap 触顶（D5 缺口，单独计数）**：新场景 `chat_rig_cap_exhausted`（cap=32）在真实 GLM-5.3-Flash 与 MiMo mimo-v2.6-flash 上：wire `body.max_tokens=32` 逐 attempt 断言（asserted_fields 含 cap 字段）、**恰 1 次 POST（零重采样）**、`turn.failed` 事件携带产品 cap terminal `Output token limit reached; increase max_tokens before retrying`、零 `turn.completed`、stderr 同文案。场景代码：`run_cap_exhausted_turn`（`TurnOutcomeExpectation::Failure` 预期失败契约）。

**D4 低载冷/热对照（单独计数）**：负载 ~2.2–2.5（18 核，真低载窗口；对照另一机 load 20–65 振荡无窗口）。3 轮 × 2 测试（`thread_resume_echo_override_is_ignored_on_loaded_and_running_threads`、`thread_resume_same_provider_echo_uses_current_default_model`）= **6/6 pass**，每轮 ~4.1s，冷启动 initialize 均在 deadline 内。命令与批次 3 同过滤器（`--test-threads 2 --retries 0`），日志 /tmp/codex-oct05-newpc-d4-lowload.log。该类在本机低载下稳定通过；另一机高载失败的根因 profiling（spawn-to-ready 分项、dyld/签名成本）仍未做，D4 完整闭环保持开放。

## 本轮新增测试与对应缺口

1. **B3 四命令 corrupted/inactive**（原仅 archive）：`session_commands_reject_corrupted_nuwax_group_values_before_remote_connection`、`session_commands_without_nuwax_group_reach_the_remote_connection` 改为 `SUBCOMMANDS`（queue/archive/unarchive/delete）循环；抽 `clear_nuwax_environment`/`apply_thread_arguments` 共用。四命令行为一致：坏值 fail-fast 具名变量不回显、inactive 抵达远端连接。
2. **C2 SSE-wait 取消**：`nuwax_env_interrupt_while_awaiting_sse_response_closes_the_request`（Unix）。网关收完请求后不回应答，子进程处于等待首字节状态；SIGINT 后 exit=1、取消及时（<15s，远低于 60s idle 预算）、stderr 无 `request timed out`（证明走取消而非预算到期）、恰一连接。Windows 原生 Ctrl-C 仍未验（平台）。
3. **C2 并行 idle 行为差分**：`nuwax_env_parallel_processes_enforce_distinct_idle_budgets`。两并发子进程同型停滞流，idle 1500ms vs 12000ms：双双以各自预算失败（stderr `request timed out`）、停滞时长 ≥ 各自预算且短 < 长、总时长分化、每网关恰一连接；并断言各自 model 与 Bearer，补齐原并行测试只查 config 字节的缺口。
4. **warm/另一连接订阅对照**：`thread_resume_echo_override_is_ignored_when_another_connection_subscribes`。websocket 双客户端：A 加载+订阅 gpt-5.4 线程，B 以 echo override resume——跨连接 live-owner 规则成立（不重路由到当前默认 5.2），wire 全程 [5.4, 5.4]。
5. **D5 live cap 触顶场景**：见上。

未发现需修复的生产缺陷（四命令策略一致、跨连接 owner 规则与既有同连接行为一致、触顶契约与 D3 离线钉死一致）。

## 第二轮补齐（同日，用户授权"macOS 可测的全部开发验证"）

历史报告登记六项测试/场景通过；loaded 无订阅行为当时被称为分歧。2026-10-06 源码复审确认这是明确的 cache-entry 契约，测试证明缺口已修复，见复审记录。

| 批次 | 完整命令（`just`，同前缀） | selected/executed | pass/fail | 首轮失败与复验 | 日志 |
|---|---|---|---|---|---|
| B2 owner/writer 矩阵 | `just test -p codex-cli -E 'binary(queue_owner_matrix) \| binary(queue_owner_dispatch)' --locked --retries 0 --test-threads 2` | 7（3 既有 + 4 新） | **7/7** | 冷入队测试首轮挂在自身 bug：派发请求打进了 gated server 永不释放的门（30s 超时）；修复为测试内预开门后过。诊断期确认 enqueue 本身 ~4s 完成、embedded writer 不留 socket | /tmp/codex-oct05-newpc-*(queue owner 批次内联于终端输出) |
| loaded 无订阅对照 + npm 单元格 | `just test -p codex-app-server -E 'test(thread_resume_echo_override_applies_to_an_unsubscribed_loaded_thread) \| test(thread_resume_echo_override_is_ignored_when_another_connection_subscribes)' --locked --retries 0` 与 `just test -p codex-cli -E 'test(npm_install_method_keeps_cold_queue_and_archive)' --locked --retries 0` | 2 + 1 | **3/3** | 无订阅对照首轮按"loaded⇒保留内存模型"断言失败——暴露真实分歧（见下），改钉实际行为后过 | 同上 |
| Anthropic live 触顶 | 重建 exec（source `1af78354…`/binary `c29324c0…`，receipt `source_and_package_validated`）后 `just test -p codex-live-tests --test exec_live -E 'test(/^(mimo\|glm)_anthropic_rig_cap_exhausted$/)' --retries 0 --test-threads 1` | 2 | **2/2** | 无 | /tmp/codex-oct05-newpc-live-anthropic.log；logs/live-*/ |

**新测试与钉住的契约**：

- `queue_from_a_differently_envd_client_runs_on_the_owner_environment`（B2"active client 向已有 owner"）：持完整异环境（模型/密钥/网关均不同）的客户端 enqueue，owner 以自身环境执行（wire model/Bearer/turn_trigger=queue），客户端侧 mock **零请求**，config 字节不变。
- `queue_refuses_an_embedded_writer_beside_the_running_owner_daemon`（B2"禁止第二 writer"真实 CLI 面）：daemon 在默认 socket + 非白名单 `-c` override 会使该次调用回落 embedded——CLI 必须以 second-writer 守卫拒绝（具体文案断言），队列保持 0、零模型请求。
- `queue_enqueued_while_the_owner_turn_runs_dispatches_after_without_a_second_writer`（B2"running owner"）：owner 的首个模型请求被网关真实门控在途（wire 收到即证 running），此时 enqueue 成功、队列=1；放门后首 turn 完成、队列由**同一 writer** 派发：恰 2 请求、均为 owner 凭据、第二请求 `turn_trigger=queue`、无第三连接。
- `queue_without_a_running_owner_persists_and_a_later_owner_drains_it`（B2"owner 缺失"）：无任何 owner 时冷入队被 embedded writer 接受并持久化（不执行、不留 socket、不改 config），后继持环境 owner resume 触发派发，wire 证明按 owner 环境执行。
- `npm_install_method_keeps_cold_queue_and_archive_on_the_embedded_server`（行政启动矩阵 install-method 单元格）：`CODEX_INSTALL_SOURCE=npm_nuwax` 下冷 queue 与 archive 均经 embedded 成功、零执行、无残留 socket。
- `thread_resume_echo_override_applies_to_an_unsubscribed_loaded_thread`（item 2"loaded 无订阅对照"）：见下述分歧。

**loaded 无订阅行为（2026-10-06 复审校正）**：最后一个订阅者断开后，线程仍出现在 `thread/loaded/list`，但此时 echo override **已按 fresh-load 路径生效**（服务端日志 `resuming session with different model: previous=gpt-5.4, current=gpt-5.2-codex`）——与"已加载+有订阅（含另一连接）保留内存模型"的行为不一致。即 loaded-list 成员资格 ≠ 模型保留。测试钉住实际行为（override 生效、报告模型与 wire 一致 [5.4, 5.2]），`thread_processor.rs` 的 `resume_running_thread` 明确规定：可接收直接输入的 loaded idle runtime 在无订阅者、非运行中且 override 不匹配时，先等待 Shutdown 完成，再移除并冷恢复。有订阅或 running 时保留 owner；这是当前已有契约，不是待裁决的实现缺陷。改变该契约才需要独立兼容方案。

## workspace 全套与 Bazel（第二轮门禁）

- **环境补齐（均为本机缺失，按仓库流程安装，未改任何 pin）**：`just`/`dotslash`/`pkg-config`+`glib`+`gstreamer`+`gst-plugins-base`（brew）；bazelisk 1.29.0（自动按 `.bazelversion` 取 Bazel 9.0.0，签名校验通过）。
- **V8 工件**：全套图含 v8 沙箱 crate，上游预编译 404。按 `third_party/v8/README.md` 流程从 `openai/codex` 发布标签 `rusty-v8-v150.4.0` 下载 aarch64-apple-darwin 配对工件，**SHA-256 与仓内 pinned manifest 逐一相符**（archive `00adbb48…`、binding `ca5adf0c…`），经 `RUSTY_V8_ARCHIVE`/`RUSTY_V8_SRC_BINDING_PATH` 注入；未禁用 TLS、未改 pin、未从临时路径复制。
- **误跑事故（如实登记）**：我以无过滤 `just test` 启动全套，导致 `exec_live`/`bridge_live` 一并执行——91 个 live 用例已跑（含数十次真实厂商调用、live 重试可见），发现后立即终止。责任在我启动时未按惯例排除 live 二进制。随后按离线口径 `just test -E 'not binary(exec_live) & not binary(bridge_live)'` 重跑（结果见下"最终汇总"）。该误跑不作为任何验收证据。

### 离线全套结果（本机首次全库执行，任何机器的首份记录）

命令：`env RUSTY_V8_ARCHIVE=… RUSTY_V8_SRC_BINDING_PATH=… just test -E 'not binary(exec_live) & not binary(bridge_live)'`（V8 配对工件如上）。
**21,561 run：21,178 pass（108 slow、53 flaky）/ 270 fail / 113 timeout / 68 skip；exit 100；3,683.9s**（日志 /tmp/codex-oct05-newpc-workspace.log attempt 5 段）。

**Claude 报告新增测试在套件内通过；没有同机同环境的基线对照，不能据此断言全部失败与 fork 或本轮改动无关。** 抽样单跑对照（`--retries 0 --test-threads 1`）分类：

- **争用/时限类（单跑即过）**：codex-http-client route_aware_client_pool TLS 组、codex-rmcp-client oauth/sse/streamable 组、codex-v8-poc（套件内有 linked 沙箱=true vs poc feature=off 的观测；应先在相同 feature 图复现再归因，单包通过不构成同图对照）。套件内大量 10.3–10.4s/30s 失败集中在 app-server guardian_v2/history_notes 等重组件并发时限。
- **确定性失败（单跑仍挂，根因需逐项判定）**：codex-exec-server remote::registration_retry（≥15 例，"unexpected registration failure kind"）；codex-network-proxy mitm/http_proxy/runtime/socks5/network_policy（~25 例秒级失败）；codex-app-server-protocol `stable_precomputed_exports_match_schema_fixtures`（2026-10-06 已确认是 fork 来源身份字段对应的预计算导出遗漏，并修复；不是路径归一化问题）；codex-skills-extension host_service 快照（2 例）；codex-tui guardian/reconnect 快照；guardian_v2 单跑 34–56s 失败（非争用）；另有 otel/aws-auth/install-context/core 个别例未逐一定性。
- 快照类的机器路径/渲染差异仍只是假设，需要检查实际 diff。预计算导出已单独复现为来源身份 schema 漂移，2026-10-06 使用仓库生成命令修复；不能合并归因。

**结论**：workspace 全套在本机达到"可执行、已表征"：约 98.22% 通过（21,178/21,561）；383 例 fail/timeout 尚未全部定性，抽样结果不足以证明均属既有环境/争用问题，登记为独立后续项，不以此宣称全套通过。

### Bazel 门禁（第二轮，发现并修复三处 fork 构建图缺陷）

bazelisk 1.29.0 自动取 Bazel 9.0.0（.bazelversion pin，签名校验过）。`bazelisk build //codex-rs/exec:exec` 首跑即抓到真实缺陷，逐层修复后通过：

1. **[缺陷] fork 新 crate 未注册 Bazel 图**：`codex-rust-rig-bridge`、`codex-rust-genai-bridge` 无 BUILD.bazel，core 的自动推导依赖指向不存在的包——任何依赖 core 的 Bazel 构建在此 fork 上从未可能成功。修复：按 `codex-api` 最小惯例补两个 BUILD.bazel。
2. **[缺陷] exec 的 build.rs `#[path]` 共享模块不在编译源**（fork 引入 build receipt 时新增）：`build_script_data` 只是 runfile，rustc 沙箱缺 `src/build_source_identity.rs`。修复：`defs.bzl` 宏新增 `build_script_srcs_extra` 参数（并显式 `crate_root = "build.rs"`），exec/BUILD.bazel 声明该模块为 build-script 编译源。
3. **[缺陷] Cargo 重命名依赖对 rules_rust 不可见**：`reqwest-rig = { package = "reqwest", version = "0.13" }` 编译报 `unresolved import reqwest_rig`。修复：宏从生成数据接 `aliases` 到全部 7 个 rust 规则调用点；因 aliases 是 label-dict 属性会制造依赖边，而生成映射混入非 normal 依赖与第一方冗余项（曾直接造成 protocol↔network-proxy 假环），只保留 `@crates//` 的重命名条目。

结果：`//codex-rs/exec:exec` **exit 0**（覆盖 core→rig-bridge→protocol/tools/config 等全图）；`//codex-rs/cli:codex` 复核 **exit 0**。改动：`defs.bzl`、`codex-rs/exec/BUILD.bazel`（修改），两个桥 crate `BUILD.bazel`（新增）；未动 MODULE.bazel（无 lockfile 同步需求）。日志 /tmp/codex-oct05-newpc-bazel.log。

## 未执行/剩余项（逐项登记，未执行不算通过）

- ~~B2 完整 owner/writer 矩阵~~（第二轮已补：异环境 active client、第二 writer 守卫、running owner 真实 gate 派发、owner 缺失冷入队持久化，7/7）——剩余：embedded/daemon 并存时 writer 数量的进程级计数（当前以"客户端侧 mock 零请求 + 全部请求同凭据"为观察面）。
- ~~行政启动矩阵~~（install-method 单元格已补 npm_nuwax × 无 daemon × queue+archive）——剩余：install-method × daemon 存在性 的其余组合无可观察 CLI 级差异（选择逻辑已由 tui 策略/排除矩阵单测钉住）；`codex archive` 纳入 daemon_startup 命令表。
- ~~loaded 无订阅对照~~（第二轮已钉实际行为并登记分歧，统一待产品裁决）。
- ~~Anthropic live cap 触顶~~（第二轮已执行 2/2）。Responses 协议触顶仍未做。
- **Windows 原生 Ctrl-C**：macOS 不可执行；需原生控制台 fixture。
- **loaded 无订阅对照**：2026-10-06 已以 unsubscribe ACK + loaded/list 完整响应确认前置状态，生产 cache-entry 契约已明确；跨平台仍需验收。
- **cap 截断的 partial text/usage/Done 保留契约**：产品语义未裁决，本轮未改（维持 D3 已登记边界）；触顶场景只钉失败契约，不合成成功 Completed。
- **D6**：无 exact/证明上界交付；legacy/unverified 标注维持；40,960-byte fail-fast、签名、搜索块均未动；P0 人工复审保持开放。
- ~~workspace 全套~~（第二轮已执行但失败，见上；98.22% 通过，383 例仍需逐项定性）。
- ~~Bazel build~~（第二轮已执行：发现并修复三处 fork 构建图缺陷后 `//codex-rs/exec:exec` exit 0，详见上节"Bazel 门禁"）。
- **Windows/Linux 平台矩阵、远程 CI**：CI 禁止自动 dispatch。
- **加密引用、跨进程 opaque 回放矩阵**：未执行。
- **Responses 协议 live 触顶**：未执行。Chat/Anthropic 有 2026-10-05 历史证据；2026-10-06 没有新增厂商请求，不能称为修后 live 通过。
- **模型面**：mimo-v2.6-flash/GLM-5.3-Flash/step-5-preview 各一；mimo-v2.5、step-3.7-flash 未测。

## 2026-10-05 历史收尾

- 第一轮：scoped `just fix -p codex-cli -p codex-exec -p codex-app-server -p codex-live-tests --locked` exit 0（1 处等价 lint 修正）；补装 dotslash 后 `just fmt` exit 0（全五组）；`git diff --check` exit 0。
- 第二轮（全部测试后）：同 scoped fix exit 0（2 处等价 lint 修正：stream_wait 签名、dispatch 闭包）；`just fmt` exit 0（含 buildifier 对新增 BUILD.bazel/defs.bzl）；`git diff --check` exit 0。按 AGENTS，最终 fix/fmt 后未重跑测试。
- workspace 全套失败快照测试遗留的 11 个未跟踪 `.snap.new` 已删除（不接受本机分歧渲染）；另有 1 个**被跟踪的 `.snap.new`**（merge 提交 1f9841304 带入的既有脏点，`core/tests/suite/snapshots/all__suite__scenarios__astra_kickoff_remote_compaction_windows.snap.new`）保持原样并在此登记。
- 未改 Cargo 依赖/ConfigToml/API shape——无 bazel-lock/config-schema 同步需求（MODULE.bazel 未动）。
- 本轮不 commit/push；运行时文件（.env.local、logs/、target、rusty-v8-cache）不入库。

## 2026-10-05 按依赖划分的提交建议（当时未执行）

1. `cli/tests/nuwax_session_remote.rs`——B3 四命令（单文件，独立）。
2. `exec/tests/suite/nuwax_env_stream_wait.rs` + `suite/mod.rs`——C2 stream-wait 两测试（新文件+两行注册，独立）。
3. `app-server/tests/suite/v2/thread_resume.rs`——另一连接对照 + 无订阅分歧钉住（同文件两测试，独立）。
4. `codex-live-tests`（runner/scenarios/binary_turns/lib/exec_live）——cap 触顶场景（Chat+Anthropic，内部 API 自洽一批）。
5. `cli/tests/queue_owner_matrix.rs` + 新文件 `cli/tests/queue_owner_dispatch.rs`——B2 owner/writer 与派发矩阵（同主题一批）。
6. **Bazel 修复**：`defs.bzl` + `codex-rs/exec/BUILD.bazel` + 新增两桥 `BUILD.bazel`——修复 fork 构建图三处缺陷（exec/cli 两目标 exit 0 实证）；建议先于任何 CI 依赖 Bazel 的变更合入。
7. 文档：本文件 + `rig-stability-followup-2026-10-04/tasks.md` 更新。

1–5 为纯测试（离线可验），6 为构建系统修复（`bazelisk build //codex-rs/exec:exec //codex-rs/cli:codex` 复验），7 收尾。各批互不依赖、可独立审查回滚；本轮无生产 Rust 逻辑改动。

## 2026-10-06 Codex 独立复审

### 基线、范围和边界

- 仓库 `/Volumes/soddygo/git-workspace/fork-nuwax-codex`，分支 `test`，审查起点 HEAD `7b891587981723fcd42f90bac0773387d57de005`。检查点等于 HEAD，祖先检查成功；没有检查点之后的提交。Claude 的这版是工作树 diff，不是额外的已推送提交。
- 开始时 12 个 tracked modified、5 个 untracked 文件：remote/owner/app-server/exec/live-tests 测试，Bazel 接线及本报告/tasks。没有 staged diff。`git status`/diff 与用户粘贴的执行记录交叉检查，不强制切回旧 SHA。
- 最近上游 merge 为 `1f9841304c3ad1c673f6a90102a83b2096b84f37`，父提交 `a1d51977830ab2eea19e271a484675ea328dd870` 与 `67727e7cf114cf3e1b71db368d74b24e32f6cb12`。提交描述的九处冲突解决已作为背景读取；本轮没有独立构建两个父提交，不能称全部合并结果都已验收。
- 阅读根 AGENTS、交接/push 文档、Rig spec/plan/tasks、field mapping、container env 报告及当前调用方；四个独立审查分支检查测试真实性、兼容/owner、模型上下文、改动规模。以当前源码和运行结果校正 Claude 报告。
- 本轮修复生产构建接线、stable precomputed schema 漂移及测试/证据问题。没有修改三桥生产 Rust 转换逻辑、ConfigToml、依赖锁、API shape、历史 rollout、pause/signature 截断策略。
- 当前第三方 Responses 仍由 Rig 请求 `/responses`，Chat 请求 `/chat/completions`，Anthropic 请求 `/messages`；本轮实测 Bazel 公共 exec 路径。native/第一方/显式 bridge 边界没有扩展。
- 保留 `.env.local`、SQLite、tmp、配置、既有 tracked `.snap.new`。没有 reset/clean/stash。用户本轮明确授权阶段 commit；没有授权 push、发布、CI dispatch 或新增真实厂商请求。

### 问题清单及修复

位置为仓库相对路径，行号以最终格式化后工作树为准，可按符号定位。P1 为影响运行或关键验收可信度，P2 为局部产物/隔离/报告问题。

| 级别 | 文件/符号 | 触发条件、影响 | 修复与证据 |
|---|---|---|---|
| P1 | `codex-rs/core/BUILD.bazel:codex_rust_crate` | Bazel 构建成功但 Core 未开启 bridge feature。第三方模型在发送请求前报缺 bridge，实际 0 POST；只检查 build exit 0 会漏掉。 | 显式启用 `rust-rig`/`rust-genai`，保留 Claude 两桥 BUILD、registry alias 和 exec build-script 编译输入。修前公共 CLI 有缺 bridge 错误；修后实际 Bazel exec 三协议 3/3 pass、各 1 POST。genai 未单独请求验收。 |
| P2 | `app-server-protocol/schema/precomputed/app-server-exports-stable.json.zst` | 已有来源身份字段未同步 stable 内部 RolloutLine schema，fixtures 对比确定失败；不是机器路径差异。 | `just write-app-server-schema` 重生成，补 auth_domain/auth_domain_kind/endpoint_identity 及 nullable bridge 文档字段；4 个 schema fixture 测试通过，SDK 无 diff。 |
| P1 | `cli/tests/queue_owner_dispatch.rs:npm_install_method_keeps_cold_queue_and_archive_on_the_embedded_server` | 零请求 mock 未进入 CLI 配置，计数永远为零；只看命令成功也没有证明 queue/archive 落盘。 | 配置真实 mock_provider、rollout 绑定该 provider，验证 archived 路径和后继已 initialize 的 public app-server QueueList 中精确输入。该测试实际执行通过。 |
| P2 | 同文件 `cleaned_environment`、npm 子进程环境 | 父进程 CODEX_MANAGED_BY_* 优先于 CODEX_INSTALL_SOURCE，所谓 npm 单元格可能实际用其他 install method。 | 清除四个 managed markers 和来源，测试显式设 npm_nuwax。只代表无 daemon 的冷 queue/archive 单元格，不宣称完整行政矩阵。 |
| P2 | `cli/tests/{queue_owner_matrix,queue_owner_dispatch,nuwax_session_remote}.rs`、`exec/tests/suite/nuwax_env_stream_wait.rs` | 继承 reasoning/context/compact 控制项或全局 SQLite 会使新机测试使用外部预算/共享状态。 | 清理四个 CODEX 控制变量，remote 显式隔离 SQLite；NUWAX/home 隔离继续保持。相关 CLI 12、exec 19 个测试通过。 |
| P1 | `exec/tests/suite/nuwax_env_stream_wait.rs:reject_late_requests_until_child_exits` | 原网关在首连接 EOF 时即丢弃 listener，子进程退出前的迟到重试无法被观察；“无后续请求”证据不成立。 | listener 保持到子进程输出收集完成，biased accept 拒绝迟到连接；新增真实 TCP EOF 后的迟到连接负控。原两公共 exec 场景及负控 3/3 通过；40/45s deadline 和 1500/12000ms 下限未放宽。 |
| P1 | `app-server/tests/suite/v2/thread_resume.rs:thread_resume_echo_override_applies_to_an_unsubscribed_loaded_thread` | drop 连接没有 unsubscribe ACK，loaded-list 仅打印，可能实际走 cold 或仍存在订阅，无法证明 loaded 无订阅分支。 | 显式 unsubscribe，断言 Unsubscribed 和完整 loaded-list，再断言 model 和 wire [5.4,5.2]。另连接订阅对照及相关四个 echo/resume 测试通过。 |
| P1 | `live-tests/src/binary_turns/runner.rs:execute_expecting` | 预期 cap failure 只检查非零退出，stdout/stderr capture 出错仍会被接受。 | 两个 pipe error 均拒绝；保留已捕获字节及主要失败原因。新增负控覆盖两种 pipe failure。 |
| P1 | `live-tests/src/binary_turns/scenarios.rs:cap_exhausted` | lossy UTF8/filter_map 静默丢弃坏事件，残缺输出可能被当成合法 terminal。 | 从持久 events.jsonl 严格 UTF8 与逐行 JSON 解析，非法数据失败；合法空行允许。坏 JSON/UTF8 负控通过。 |
| P1 | 同 cap 场景的 capture validation | 没有限制 attempt=1，真实 cap 重采样可能仍被验收。 | 三协议统一 required capture、cap/wire 验证和恰一 attempt；0/2 attempt、成功 terminal、错误终止/ stderr 负控全部拒绝。 |
| P2 | 同 cap 场景的 `retain_best_effort` | 产品失败作为测试成功时临时 home 被删，partial rollout 不进入工件，无法支持保留情况声明。 | accepted cap failure 也保留 rollout，保留失败不能 pass；新增正例与工件冲突/主错误优先级验证。没有改变 Core 的 partial usage/Done 契约。 |
| P2 | 本报告及 `rig-stability-followup-2026-10-04/tasks.md` | loaded cache 被误称未裁决、98.22% 写成 94%、全部 383 失败被笼统归因为环境、D6 未实施可能被当作完成。 | 校正为源码明确 cache-entry 契约，撤回整体归因，保留失败清单及 D6/P0。`resume_running_thread` 对 idle 无订阅 cache 先 Shutdown 后冷恢复是现行行为，不应据误读改产品。 |
| P2 | 整体 diff 规模 | 新增文件计入后超过 800 changed lines，单批难以复核。 | 按构建、schema、remote/matrix、running dispatch、cold/npm、exec、app-server、live harness、文档分别保存；复杂批尽量低于 500，普通批低于 800。中间阶段没有独立运行测试，完整代码树证据见下。 |

B2 的“无第二 writer”测试证明 owner 请求/持久化归属及 embedded 守卫，不能仅由两个请求同凭据证明进程级 writer 数。后者继续作为后续验收任务。字段映射与 SDK 限制没有本轮新增生产实现；251 个 Rig、214 个 API 测试是回归证据，不代替完整字段逐项审计或官方厂商兼容矩阵。

### 当前源码测试：完整命令及结果

命令从仓库根执行，除特别注明外均使用以下同一隔离 target；没有执行完整 workspace 或真实 live binary。Nextest skipped 为选择过滤/已有 ignored 项，未新增跳过。没有自动 retries、没有扩大 deadline。

```sh
export CARGO_TARGET_DIR=/Volumes/soddygo/git-workspace/codex-targets/validation-20261005

# 修前独立复现 schema 漂移
just test -p codex-app-server-protocol -E 'test(stable_precomputed_exports_match_schema_fixtures)' --offline --locked --retries 0 --test-threads 1

# 第一次生成：Rust 成功，Python 3.14 的 SDK generator 失败
CARGO_NET_OFFLINE=true just write-app-server-schema

# 安装隔离 Python 3.13 后复验生成（没有升级仓库依赖）
uv python install 3.13
CARGO_NET_OFFLINE=true UV_PYTHON=3.13 UV_PROJECT_ENVIRONMENT=/tmp/codex-oct06-review-sdk-venv just write-app-server-schema

bazelisk build //codex-rs/exec:exec //codex-rs/cli:codex --jobs=4
bazelisk build //codex-rs/exec:codex-exec --jobs=4
python3 /tmp/codex-oct06-bazel-wire.py

just test -p codex-api -p codex-rust-rig-bridge -p codex-live-tests -p codex-exec -p codex-cli -p codex-tui -p codex-config -p codex-utils-cli -p codex-core -p codex-app-server -p codex-app-server-protocol --features codex-core/rust-rig -E 'package(codex-api) | package(codex-rust-rig-bridge) | (package(codex-live-tests) & not binary(exec_live) & not binary(bridge_live)) | (package(codex-exec) & test(nuwax_env)) | (package(codex-cli) & (binary(queue) | binary(queue_owner_matrix) | binary(queue_owner_dispatch) | binary(nuwax_session_remote))) | (package(codex-tui) & (test(named_session_lookup) | test(session_archive_commands) | test(session_queue_commands) | test(app_server_target))) | (package(codex-config) & test(env_group)) | (package(codex-utils-cli) & test(nuwax)) | (package(codex-core) & (test(rig_output_cap) | test(rig_anthropic) | test(model_output_projection))) | (package(codex-app-server) & (test(thread_resume_echo_override) | test(thread_resume_same_provider_echo_uses_current_default_model))) | (package(codex-app-server-protocol) & test(schema_fixtures_tests))' --offline --locked --retries 0 --test-threads 2

# 修复 npm verifier initialize 与补迟到连接负控后的定向复验
just test -p codex-api -p codex-rust-rig-bridge -p codex-live-tests -p codex-exec -p codex-cli -p codex-tui -p codex-config -p codex-utils-cli -p codex-core -p codex-app-server -p codex-app-server-protocol --features codex-core/rust-rig -E '(package(codex-exec) & test(nuwax_env)) | (package(codex-cli) & (binary(queue_owner_matrix) | binary(queue_owner_dispatch) | binary(nuwax_session_remote)))' --offline --locked --retries 0 --test-threads 2
```

| 批次 | 实际执行/结果 | 首轮失败、根因与复验 | 日志 |
|---|---|---|---|
| schema 修前 | 1 run / 0 pass / 1 fail / 314 skip / 0 timeout；exit 100 | 内部 rollout provenance 产物漂移。修后包含在下面 4/4 schema fixtures 通过中。 | `/tmp/codex-oct06-review-schema.log` |
| schema 首次生成 | Rust ignored generator 1 pass；整体 exit 1 | 锁定 datamodel-code-generator 0.31.2 不支持所选 Python 3.14，报 ValueError: 3.14；不是 Rust schema 生成失败。 | `/tmp/codex-oct06-review-schema-generate.log` |
| schema 生成复验 | generator 1 pass；整体 exit 0 | uv 隔离 Python 3.13.16，SDK 完成且无 diff；只更新 .zst。 | `/tmp/codex-oct06-review-schema-generate-py313.log` |
| Bazel build 两批 | 目标均 exit 0；112.55s / 13s | 修复 Core feature 后真实启动单独验证，build 不能充当 wire 验收。 | `/tmp/codex-oct06-review-bazel.log`、`/tmp/codex-oct06-review-bazel-exec.log` |
| Bazel localhost mock | 3 selected / 3 executed / 3 pass / 0 fail / 0 skip / 0 timeout | 三协议分别 1 POST、model/cap/auth 匹配、exit 0、Completed=1/Failed=0、config 原字节。 | `/tmp/codex-oct06-bazel-wire.log`；脱敏工件见下面 |
| 11 包图首次回归 | 644 run / 643 pass / 1 fail / 13,307 skip / 0 timeout；exit 100；111.710s | Codex 新增 npm 持久化 verifier 使用 `.build()` 默认 skip_initialize，QueueList 收到 Not initialized。改 `.build_initialized()`；产品 CLI queue/archive 已成功，错误在新测试 setup。 | `/tmp/codex-oct06-review-tests.log` |
| 定向复验 | 31 run / 31 pass / 163 skip / 0 timeout；exit 0；19.503s | CLI remote 4、owner matrix 5、dispatch 3；exec env 19（含新增迟到连接负控）。 | `/tmp/codex-oct06-review-rerun.log` |

按 package/binary/test 名称去重：首批 644、复验 31，重叠 30，**645 个不同离线测试最终通过**。不是 675 个独立测试。首批 API 214、Rig 251、Core 28（含 7 个 rig_output_cap 公共 suite，feature 显式开启）、live harness 45（新增 cap 测试函数 3，含三协议正例和 10 类负例）、app-server echo 4、schema fixtures 4 均实际执行；其余选择覆盖 CLI/env/TUI/config。

### 源码、二进制与请求证据

- 测试运行时为上述 HEAD 的 dirty 树，包含 Claude 修改及 Codex 修复；在 final fix/fmt 前对 19 个改动代码/构建/schema 文件逐个 SHA-256 登记。完整清单在 `validation-2026-10-06/identity-before-fix.json`。这些 hash 表示已测试时刻，不冒充格式化后 commit 的 byte-for-byte binary receipt。
- Cargo 公共程序 SHA-256：codex `fdb72f2c7caf34ee40c88e9d4a93d9934d55af6413c6e5fcccb877745deb4857`；codex-exec `762daeef86c2fab3d889b8cc4933c6b6acd137f4c1f1dbf2623b439b4da89e87`；codex-app-server `dfa3a4d077a32cdcd40b385370c7fe4547fab1a2591de6b720255eb00a233a0f`。路径/范围见 identity JSON；不将旧 live source/package receipt 迁移到本轮。
- Bazel 实际 exec SHA-256 `b49fd7adc199743bee5975ad87231210e929b70087d273c678da927f62c7184a`，解析到真实 Bazel output，而非 Cargo debug 程序。`validation-2026-10-06/bazel-wire-evidence.json` 只保留 localhost 路径、synthetic model、cap、auth_matches 布尔、attempt/final state，无原始鉴权/敏感 request。复现实验脚本 `validation-2026-10-06/bazel-wire-mock.py` 是当次脚本副本，尚未纳入长期 Bazel test gate；后续任务明确要求补门禁。
- **本轮全部请求是本地 mock，没有真实厂商调用。** 上面的 2026-10-05 live 历史由 Claude 报告；本轮未重新验证对应现场工件或当前 commit 的 live 能力。没有 Linux/Windows、完整 workspace、CI 验证。

### 剩余功能及后续开发

交付 `claude-code-followup-2026-10-06.md`，含可直接执行的分批任务和验收要求：P1 确定性 workspace 失败归因/修复、Bazel 公共三协议长期门禁、B2/B3/行政与进程 writer/opaque 来源矩阵、cap partial output/usage/Done 的 Spec/Plan；P2 Windows/Linux、D4 profiling、Python codegen 运行时；D6 token-aware 为 P0 人工复审未关闭的设计项。

40,960-byte pause 上限仍为兼容边界，不能宣称精确 10K-token。当前方案 KeepWholeOrFail，缺证据 LegacyBytes/unverified；未实现 verified exact/proven upper bound，不引入签名截断/旧历史改写。实际生产级门禁尚未全绿。

### 最终检查与阶段提交

测试完成后运行 scoped `just fix`、`just fmt`、`git diff --check`。执行结果及提交列表如下。遵守 AGENTS：最后 fix/fmt 后不再重跑测试；中间 commit 不单独声称通过测试。只保存本轮已审查文件，不纳入运行时数据。


```sh
# codex-rs 目录；使用上面同一隔离 CARGO_TARGET_DIR
just fix -p codex-cli -p codex-exec -p codex-app-server -p codex-live-tests -p codex-app-server-protocol --features codex-core/rust-rig --offline --locked
just fmt
# 仓库根目录
git diff --check
```

三项 exit 0。scoped fix 用时 1m36s，无新增 lint 修复；final fmt 为确定性格式化，五个代码/构建文件的字节发生变化，详见 `validation-2026-10-06/identity-after-format.json`，未改变产品逻辑。日志 `/tmp/codex-oct06-review-fix.log`、`/tmp/codex-oct06-review-fmt.log`（格式化成功时无输出）。没有在这之后运行测试。拆分 dispatch 的索引草稿曾被 `git diff --cached --check` 拒绝（新 EOF 空行）；仅修正草稿尾空行，完整工作树未被覆盖，最终检查通过。

用户授权的阶段保存（均在 test，无 push）：

| commit | 范围 | changed lines（加+删，binary 单列） |
|---|---|---|
| `fc87bf45e8b6de5e06c2ecf3baa3e289e813c294` | fix(bazel): enable model bridges in public binaries | 47 |
| `9069fb90c0470ae072ea89281c491d1fd641bbe0` | fix(protocol): refresh rollout provenance schema exports | binary .zst |
| `ef6c9ddbe4aa78ed5e2efe5323e366443f317c56` | test(cli): verify remote administration and owner routing | 462 |
| `af1496a5e13293a17c106481080038d1cf7ec996` | test(cli): verify queued dispatch through a running owner | 449 |
| `ae66f8ef8b83a4272b3db45ff9e16750d74a4f6c` | test(cli): verify cold queue and npm persistence | 273 |
| `426a67411495e84b0c5c471215668a26b2a358d6` | test(exec): verify stream cancellation and distinct idle budgets | 420 |
| `fc567476c21780d962ecb118f36559f02c790d5e` | test(app-server): verify subscriber and idle cache resume routing | 272 |
| `c8b8b60567858f50d3c68ded4772c81561308f0d` | test(live-tests): require durable evidence for capped turns | 441 |

最后文档批保存本报告、修订 tasks、Claude 开发任务及脱敏身份/本地 mock 工件。该批 SHA 可用 `git log -1 -- my-docs/other-computer-validation-results.md` 查询；避免在报告中制造自引用 SHA。8 个代码/测试批均小于 500 changed lines；文档批遵守 800 上限。所有中间批保持完整工作树不变，仅审查保存；测试证据属于完整修复树，不称每个中间 commit 都被单独构建/测试。
