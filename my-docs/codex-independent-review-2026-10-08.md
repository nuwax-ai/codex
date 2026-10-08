# Codex 独立审查与跨电脑交付（2026-10-08）

## 结论与审查基线

三协议 Rig、进程环境配置、历史投影与 owner 主链路已基本实现；生产验收尚未完成。Claude 此轮含有效的产品修复和 fixture 修复，也有编译、重试簿记、测试假阳性及报告口径问题，不能接受“自查全部通过、无遗留问题”的总括。剩余工作仍按 **2 个实质功能包 + 6 个验收/稳定性包** 管理，不给没有分母的完成百分比。

- 本机仓库 `/Volumes/soddygo/git-workspace/fork-nuwax-codex`，分支 `test`；开始 HEAD `79bbe0e3fe61e5b03356f4b16d3498ca3b111b9a`。开始 21 tracked modified + 2 untracked 文档，无已 staged 内容；本轮新增文件另列身份工件。
- 已推送检查点 `7b891587981723fcd42f90bac0773387d57de005` 是当前 HEAD 祖先。开始 fetch 后 origin/test 正是该检查点，本地已有 34 个阶段提交未推送，不能把这次工作树当全部差异。作者记录均为 soddy，不能仅凭 author 字段归属 Claude/Codex；以阶段报告与实际 patch 为准。
- 最近上游合并 `1f9841304c3ad1c673f6a90102a83b2096b84f37`，父提交 `a1d51977830ab2eea19e271a484675ea328dd870` / `67727e7cf114cf3e1b71db368d74b24e32f6cb12`；没有新增合并。原合并保留 SSE strict/lenient、provider pin、TUI per-thread projection、compaction 回归并适配 stream interrupt 等。此次针对合并后实际代码和最近 Claude 工作树复核，不重新宣称整个合并全量验证。
- 根 AGENTS 与四个 code-review-* 技能分别独立审查；保留配置、SQLite、tmp、`.env.local`、tracked `.snap.new`。未 reset/clean/stash。用户此次明确授权 commit/push；未授权真实厂商、完整 workspace、CI dispatch 或发布，均未运行。
- 锁定 `rig-core 0.42.0`、`native-tls 0.2.14`。第三方 Responses 默认 Rig `/responses`，不是改成 Chat；Chat/Anthropic 各自走协议端点。原生 fixture 钉 native 恢复上游 WS/Guardian V2 测试前提，不代表 Rig 获得这些原生能力。

## 全部发现与修复

行号以本轮最终源码为准；同源问题合并描述，全部独立审查意见保留。设计项只修订提案，未实现未批准语义。

| # /级别 | 文件位置 | 触发、影响与处理 |
|---|---|---|
| 1 P1 | `codex-rs/network-proxy/src/connect_policy.rs:71` | `StateDnsResolver`/imports 限 macOS，trait impl 却未同门控，Linux/Windows 编译引用不存在类型。同步 cfg；本机 macOS 已测试，跨平台编译仍待真实执行。 |
| 2 P2 | `codex-rs/network-proxy/src/connect_policy.rs:88` | fixture 只控制 A 查询，AAAA 回系统 DNS，可重新遭 fake-IP/解析不确定性并泄出测试网络。两族都取 fixture 并按 IP 家族过滤；None/localhost 保留 native，大小写/尾点规范化。新增 5 个 resolver/policy 回归。 |
| 3 P1 | `codex-rs/app-server/tests/suite/v2/guardian_v2_history_tests.rs:535` | 已持有 parent_requests 的非重入 Mutex，新增诊断再次 lock，路径会死锁。使用现存 parent guard；保留 checkpoint 精确断言，R1b 原有失败没有因此关闭。 |
| 4 P1 | `codex-rs/file-watcher/src/backend.rs:47` | Claude 把 backend 移出 async 的方向正确，但提交命令即记成功，实际 watch 失败后不重试；无界命令队列会在 backend 阻塞时积压。改为 desired/active 分离、成功才入 active、失败退避重试、一个 pending wake 合并最新状态；专职线程执行/销毁 backend。阻塞平台调用仍可使该 backend 退化，未宣称底层 FSEvents 已治愈。 |
| 5 P2 | `codex-rs/file-watcher/src/lib.rs:579` `reconfigure_watch_inner` | 注册选中路径至 exists 检查之间消失，desired 被直接移除，无事件/新注册就不会重试。删除 exists filter；新增消失→重建且不重新注册的回归。 |
| 6 P2 | `codex-rs/app-server/src/fs_watch.rs:89` / `codex-rs/file-watcher/src/registration.rs:25` | 复审第一版用 synthetic Modify(Any) 覆盖安装窗口，会对外伪造 fs/changed(root)。分离 readiness 与实际 filesystem 事件；readiness不沿用in-flight unwatch的旧Installed状态，actual祖先→文件迁移后重新等待；changes-only 订阅异步等待成功安装，安装失败报错，成功响应后只转发真实变更；duplicate ID在平台等待前拒绝，末尾entry检查仍处理并发注册。缓存订阅可接收粗粒度 readiness 失效；公共 fs 精确 child-path 断言保持。 |
| 7 P2 | `codex-rs/app-server/src/config_manager.rs:313` | 仅填 provider_id 能修普通定义误报，但 built-in bridge-only requirements 的 raw/merged 比较仍误拒；shape 判断又可命中 custom。要求 built-in 身份与 shape 同时成立，按保留的 loaded 定义合并，保留 endpoint/env defaults；custom 同键补 identity。新增 openai/ollama/lmstudio 公共 turn/start 3 例，两次实际保留 route，组织 transport 改变后拒绝且无第三次请求。 |
| 8 P2 | `codex-rs/cli/tests/admin_startup_matrix.rs:390` / `:520` | profile 负例先被脏 base strict 错误挡住，OSS 显式 provider 与默认相同，PASS 不证明目标分支/优先级。恢复合法 base 后断言具体 unknown provider 和原 rollout；无效 OSS 默认必须失败，有效显式选择覆盖无效默认，错误显式项保持失败。 |
| 9 P2 | `codex-rs/cli/src/main.rs:909` | 加强公共测试后暴露生产错误链只显示最外层，unknown-provider 原因丢失。用 anyhow alternate Display 保留 cause 链；CLI 6/6 复验通过。 |
| 10 P2 | `codex-rs/http-client/src/route_aware_tls_fallback_tests.rs:481` / `codex-rs/aws-auth/src/network_policy_tests.rs:478` warmup | 初版各平台重复预热且缺 I/O 期限/线程收敛；TLS fixture 吞空 ClientHello 可掩盖额外连接。预热仅 macOS、Once、独立 peer/connector、socket 与 accept 期限、join；恢复空读/超时失败。OS 初始化本身不由 socket 期限硬限，暖态结果不能排除冷态产品缺陷。AWS IMDS 是 HTTP，TLS 栈不能直接证明 AWS 根因。 |
| 11 P2证据 | `my-docs/validation-2026-10-07/pkg3-failure-signatures-worklist.md:3` / `other-computer-validation-results.md` | 124 标签混入此前已通过 9 项；原249新增恢复115，仍134。71 是名称切片（模块内63+模块外8），首批8 PASS含3金丝雀；旧R10实际9项8PASS/1CoreFAIL。逐项账目/主报告/tasks校正；不把状态恢复或所选基线同败当整个套件零回归。 |
| 12 P2证据 | `my-docs/validation-2026-10-07/pkg3-failure-signatures-worklist.md:134` R11 | 模块并发四次 stack overflow（含一次16MiB）支持该窗口失败，不能证明无界递归、任意栈均失败或全 workspace 必败。保留事实，递归/大 future/并发时序均为待查候选。 |
| 13 P1设计 / P0人工门 | `my-docs/cap-partial-usage-implementation-proposal-2026-10-07.md:168` | provider整response usage、bytes/4不能证明逐fragment硬限；只量正文会漏wrapper/ID/声明，先累积后裁剪亦无界。提案改为完整render exact/proven upper bound、采集期byte/count限制与serialized限额。2k候选仍触发>1k P0人工门；40,960 bytes不认证10k token。 |
| 14 P1设计 | 同提案 `:139` | 新模型partial若只加struct不注册matcher，会投影成真实人类user/授权。补 contextual matcher、type_markers、harness/guardian/authorization分类与parse_turn_item验收；禁止截断工具进入可执行路径。 |
| 15 P1设计 | 同提案 `:72` / `:220` | cap已知usage若不进全response账目，后续成功/多sampling/恢复可丢完整性或重复累计；Anthropic值聚合丢presence/显式零。提案加入稳定host response_key、快照去重、全响应已知小计及持久completeness；导出当前raw Anthropic Option计数，不只依靠SDK.has_values。旧success subtotal与新口径明确分开。 |
| 16 P2设计 | 同提案 `:139` / `:95` | 示意trait漏必需type_markers、虚构ContentItemKind::new与现有rollout通用schema版本。改为真实API；未知记录必须分别验证legacy/paginated/旧读侧，不能用cli_version冒充协商版本。7个决策点仍未批准/实施。 |
| 17 P2规模 | `my-docs/codex-independent-review-2026-10-08.md:1` 所述整体diff | 混合产品、fixture、验证诊断和提案超过800行。按真实依赖分阶段保存；必要模块/注册、Cargo边、公共消费者同步。各中间阶段未分别重新构建测试，不把分commit当单独验证。 |

R1 native fixture 及 TUI 三处重复 TOML bridge key 删除正确；无 UI 文本/布局变化，无需新 UI 快照。aws-auth 的新增 dev-dep 使用仓库已有 native-tls 版本（workspace 没有对应依赖键），Cargo.lock 仅新增既有依赖边；Bazel lock 更新没有漂移。没有 ConfigToml/API wire 类型变更，不需重生成 schema/SDK；新 Rust 模块由现有 Bazel source globs 包含，并实际编译执行。

## 独立执行与首轮失败

本机 macOS，仅本地 HTTP/SSE/WebSocket mock 和离线 crate/Bazel 验证。原电脑日志是历史参考，不升级为本机验收。工件保留 `logs/review-round4/`（ignored，不提交请求/鉴权原始输出），目标目录 `/Volumes/soddygo/git-workspace/codex-targets/validation-20261005`；正常 Bazel 测试环境与 Codex Cargo 执行环境分开。

| 日志/执行 | 实际结果 | 结论/复验 |
|---|---|---|
| `file-watcher.log` 初批 | 24 PASS | 当时尚未新增严格真实 readiness/后续变化用例，不能当最终 watcher 验收。 |
| `rust-scoped.log` | compile FAIL E0659 | 新 resolver 测试的两层 glob assert_eq 歧义；显式导入 pretty_assertions 后修复，不放宽断言。 |
| `rust-scoped-rerun.log` | 369 run = 349 PASS / 20 FAIL，744 filtered | network-proxy 319/319，AWS 1/1，watcher24/25，CLI4/6，exec-server1/18。20=17 registration/relay +2 CLI原因链 +1 watcher；HTTP selector 错用文件名实际0，不算TLS通过。 |
| `tls-warm.log` | 8 run = 4 PASS / 3 FAIL / 1 TIMEOUT，128 filtered | Cargo环境预热后仍失败，复验不能接受旧“R10全绿”；fallback后第二连接被拒绝、一次nextest 60s timeout。正常Bazel环境另验，冷态归因继续开放。 |
| `cli-rerun.log` | 6/6 PASS | 更强的错误原因/显式覆盖断言在CLI修复后通过，无guard早退。 |
| `watcher-rerun.log` /临时诊断 | 24/25，真实安装 FAIL | 原5s等待预算不变；探针显示进入首次平台watch后未返回。临时探针已删除。 |
| `provider-bazel.log` 首试 | 0 tests | 错用文件名，Bazel target PASS不算验收；正确函数名重跑。 |
| `provider-builtin-bazel.log` | 3/3 PASS，1469 filtered | 三内建provider实际两次请求/漂移拒绝。 |
| `provider-final-bazel.log` | 7/7 PASS，1466 filtered | 最终watcher源码下整个model_provider_enforcement切片，包含custom/built-in/retained route。 |
| `fs-watch-bazel.log` | 5/5 PASS，1468 filtered | 新严格public case必须收到真实exact child，并证明ready不发通知、unwatch后停止；原4例OS事件可选不计成必达证明。 |
| `fs-watch-final-bazel.log` | 公共5/5 + manager4/4 PASS | 最终duplicate-ID快速拒绝/并发entry检查源码，公共strict真实通知及管理器连接清理/重复ID通过。 |
| `cli-final.log` | 6/6 PASS，0skip | 最终watcher源码下，显式--test admin_startup_matrix复验，真实CLI无guard早退。 |
| `attestation-bazel.log` | 2/2 PASS，1470 filtered | raw native fixture走实际握手并检查header。 |
| `tui-native-handoff.log` / `tui-final-bazel.log` | 1/1 PASS，5621 filtered | native共享fixture的公共Core/TUI路径。 |
| `tls-warm-bazel.log` 首试 | target不存在，0执行 | 查询正确unit-tests标签后重跑；不是测试通过。 |
| `tls-warm-bazel-rerun.log` / `aws-warm-bazel.log` | TLS7/7 + AWS1/1 PASS | 正常Bazel暖态mock通过；Cargo4/8结果保留，冷态/环境归因未关闭。 |
| `watcher-ready-bazel.log` 首试 | compile FAIL E0308 | 复审新增notify.add_path需要拥有PathBuf；clone修正，保留首次编译失败。 |
| `watcher-ready-bazel-rerun.log` | 30/30 PASS，0filtered | failed install、blocked unwatch时不提前ready、迁移/重建、只真实事件订阅与实际OS变化全部执行。 |
| `watcher-bazel-final.log`（后一轮修改前） | 25/25 PASS | 正常Bazel严格安装与实际变化；随后增加race/ready与公共fs兼容回归，最终结果追加。 |
| `bridge-gate.log` / `bridge-gate-final.log` | exec19/19 + Core2/2 PASS；关桥0连接 | 3协议真实path/model/auth/cap/attempt/config字节、Core11旋转场景；无Skipping。负控非零exit+具名错误，最后恢复bridges-on产物。 |

`rust-scoped-audit.json` 按(run_id,ordinal)去重Summary的20条重复结果，按(binary,name)计数；349/20不是按字符串猜测。744/filtered等是未选择用例，不能写成产品skip。网络guard未更改；所有成功输出保存并核查Skipping。最终新增用例、TLS暖态结果及检查结果追加于下。

完整命令（仓库根，测试都先于最终 fix/fmt）：

```sh
export CARGO_TARGET_DIR=/Volumes/soddygo/git-workspace/codex-targets/validation-20261005
export RUSTY_V8_ARCHIVE=/Volumes/soddygo/git-workspace/codex-targets/rusty-v8-cache/librusty_v8.a.gz
export RUSTY_V8_SRC_BINDING_PATH=/Volumes/soddygo/git-workspace/codex-targets/rusty-v8-cache/src_binding.rs
just test -p codex-file-watcher --offline --locked --retries 0 --test-threads 1
# compile首败和修复后使用同一构建图/选择器；记录首次失败，禁止用rerun覆盖。
just test -p codex-network-proxy -p codex-file-watcher -p codex-http-client -p codex-aws-auth -p codex-exec-server -p codex-cli --features codex-core/rust-rig -E 'package(codex-network-proxy) | package(codex-file-watcher) | (package(codex-http-client) & test(route_aware_tls_fallback_tests)) | (package(codex-aws-auth) & test(real_imds_credentials_stop_after_policy_revocation)) | (package(codex-exec-server) & test(registration_retry)) | (package(codex-cli) & binary(admin_startup_matrix))' --offline --locked --retries 0 --test-threads 1 --success-output immediate
just test -p codex-network-proxy -p codex-file-watcher -p codex-http-client -p codex-aws-auth -p codex-exec-server -p codex-cli --features codex-core/rust-rig --test admin_startup_matrix --offline --locked --retries 0 --test-threads 1 --success-output immediate
just test -p codex-network-proxy -p codex-file-watcher -p codex-http-client -p codex-aws-auth -p codex-exec-server -p codex-cli --features codex-core/rust-rig -E '(package(codex-http-client) & test(tls_fallback_tests)) | (package(codex-aws-auth) & test(real_imds_credentials_stop_after_policy_revocation))' --offline --locked --retries 0 --test-threads 1 --success-output immediate
bazelisk test //codex-rs/app-server:app-server-all-test --//:enable_model_bridges=true --jobs=4 --test_sharding_strategy=disabled --test_filter=builtin_bridge_requirements_keep_retained_routes_and_reject_transport_changes --cache_test_results=no --test_output=all
bazelisk test //codex-rs/app-server:app-server-all-test --//:enable_model_bridges=true --jobs=4 --test_sharding_strategy=disabled --test_filter=attestation_generate_round_trip_adds_header --cache_test_results=no --test_output=all
bazelisk test //codex-rs/tui:tui-unit-tests --//:enable_model_bridges=true --jobs=4 --test_sharding_strategy=disabled --test_filter=delegated_core_events_keep_private_output_hidden_and_deliver_final_speech --cache_test_results=no --test_output=all
bazelisk test //codex-rs/http-client:http-client-unit-tests --jobs=4 --test_sharding_strategy=disabled --test_filter=tls_fallback_tests --cache_test_results=no --test_output=all
bazelisk test //codex-rs/aws-auth:aws-auth-unit-tests --jobs=4 --test_sharding_strategy=disabled --test_filter=real_imds_credentials_stop_after_policy_revocation --cache_test_results=no --test_output=all
bazelisk test //codex-rs/app-server:app-server-all-test --//:enable_model_bridges=true --jobs=4 --test_sharding_strategy=disabled --test_filter=fs_watch --cache_test_results=no --test_output=all
bazelisk test //codex-rs/app-server:app-server-all-test //codex-rs/app-server:app-server-unit-tests --//:enable_model_bridges=true --jobs=4 --test_sharding_strategy=disabled --test_filter=fs_watch --cache_test_results=no --test_output=all
bazelisk test //codex-rs/app-server:app-server-all-test --//:enable_model_bridges=true --jobs=4 --test_sharding_strategy=disabled --test_filter=model_provider_enforcement --cache_test_results=no --test_output=all
bazelisk test //codex-rs/file-watcher:file-watcher-unit-tests --jobs=4 --test_sharding_strategy=disabled --cache_test_results=no --test_output=all
CODEX_BAZEL_JOBS=4 just bazel-bridge-gate
just bazel-lock-update
python3 scripts/nextest_log_summary.py logs/review-round4/rust-scoped-rerun.log --output logs/review-round4/rust-scoped-audit.json
```

首次 provider selector 是 `model_provider_enforcement_tests`（0）；watcher 临时诊断仅用于确定进入哪一平台调用，删后又跑正常Bazel。没有放宽 deadline、削弱关键断言、增加skip、改sandbox变量相关代码，也没有通过“与baseline同败”关闭这里的17个registration失败。

## 尚未完成与下一台电脑

1. **P1稳定性**：原249集合仍134历史项未收口（不是当前所有失败总数）；R1b13 checkpoint/二次采样、R7 HTTP间歇慢、R9 sqlx pending、未覆盖TUI快照/超时及SIGABRT。此轮registration/relay17失败保留，需准确配对窗口和真实栈定位；不只凭同域等待就合并R7/R9。Core跨crate proxy fixture注入仍缺。
2. **cap partial/usage**：7个决策点、双历史保存/读取/resume、三协议usage presence与多response去重、app-server失败关闭/exec事件、token硬界证明；当前均为提案。不是仅做一个新的错误枚举就算完成。
3. **D6**：token-aware增强仍设计，40,960-byte兼容行为保持；>1k片段P0人工门、SDK/framing/tokenizer证据与签名/opaque保全。若当前发布延期这项，应明确保留LegacyBytes/unverified，不能假称已认证10k。
4. **行政与owner**：active/inactive/incomplete/corrupted环境组、loaded/subscribed/running与owner crash/restart、daemon/remote/profile/OSS等剩余格。现有两格不是完整行政矩阵。
5. **历史来源**：Responses/Chat、credential-only、不同来源/模型/协议、legacy缺戳、RawValue数字与encrypted/signed/hosted工具合法顺序。已有Anthropic场景不可代替全部协议。
6. **跨平台/冷态**：Linux/Windows真实编译/测试、Ctrl-C/锁/skills根/异OS remote，native_tls未预热独立进程及AWS HTTP具体路径；watcher backend永久阻塞降级、取消/资源回收与fs公开契约还需跨平台证明。
7. **全套与发布**：完整workspace、新机vendor/live触顶、CI与发布工件未验收；先完成离线切片，再按每项授权执行。真实凭据/鉴权和原始请求不进入提交报告。

完整可粘贴工作令：`other-computer-development-prompt-2026-10-08.md`。下一台电脑重新记录源码/程序身份与实际执行，不能复用本机通过结论。

## 最终检查、身份与保存

所有测试已在最终fix/fmt前结束；没有在格式化后重跑。`logs/review-round4/identity-before-fix.json`捕获27个变化源文件及9个实际Cargo/Bazel程序SHA/字节数（CLI/exec/app-server与测试driver）；源码与产物身份不同，未作任何真实厂商请求。最终格式化后的源码差异与提交记录在本节后续补齐。tracked pending快照与起始HEAD字节相同（git blob `812eede8db52fc8d36c14d4b01ca868fd446cc15`），没有清理既有或新增用户数据目录。

首轮scoped fix保留`final-fix.log`：Clippy needless_collect建议删Vec快照，引发E0502；自动工具回滚并继续检查，不能写0 warnings。后续等价retain改写保持“移除成功/WatchNotFound才删除、失败保留active”的分支与回调顺序；不采用--broken-code、不关lint。最终无警告复查/格式化状态另行登记。

最终八crate `just fix -p codex-file-watcher -p codex-network-proxy -p codex-app-server -p codex-cli -p codex-http-client -p codex-aws-auth -p codex-exec-server -p codex-tui --features codex-core/rust-rig --offline --locked` exit0，21m13s；仅watcher自动建议失败的警告，其余crate无警告。等价retain/contains_key改写、移除单次使用构造器转发、纠正旧“unwatch完成”测试名后，`just fix -p codex-file-watcher --offline --locked` exit0、47.79s、无警告。`just fmt` exit0；`git diff --check` exit0。之后没有运行测试。

可随仓库带走的脱敏工件在 `validation-2026-10-08/`：三个nextest摘要（结果身份/计数/源日志SHA，不含stdout/stderr），以及 `source-binary-identities.json` 的27源文件、9实际程序before/after SHA。变化文件列表见JSON；上述lint等价改写和format发生在已测源码之后，**不声称最终源码与已测试binary字节相同**。最终二进制/完整工作区/跨平台需下一台电脑独立构建验证。

代码阶段保存（共11批，全部<500 changed lines）：

| commit | 内容 | changed lines |
|---|---|---|
| `4f92326cdc8f37048727937b6495f52ad5d6742c` | test(native): retain upstream mock transport capabilities | 13 |
| `1ed7287c3f522240513a2f8312a5be6bba32df92` | fix(app-server): normalize retained provider requirements | 134 |
| `12afb93c751c60485c9cafbc0850e7fee94c4425` | fix(cli): preserve configuration causes and verify admin routing | 241 |
| `a688deeb518113251aba4a1f543def46bfd39ca3` | test(network-proxy): control both DNS families in fixtures | 344 |
| `3f96d85772e88fea417acf3bf66e44d31c6dd2f7` | refactor(file-watcher): keep registration lifecycle in its own module | 59 |
| `86ff4692f6f1c89f82868e2d3efaf37ec33e7b2a` | fix(file-watcher): coalesce backend operations and track readiness | 240 |
| `4951b76f86f304882ca1fe52b337937aa45bcb17` | test(file-watcher): cover failed and blocked backend operations | 225 |
| `3ebc2c1addd7cc886fde72183ee511e9174211b8` | fix(file-watcher): await readiness without fabricating fs changes | 286 |
| `cdbef803a4800e249f606c35a0702a9d7289ff72` | test(file-watcher): verify live readiness and ancestor migration | 206 |
| `8d280fe629b6161227ad1bc4f7915d8c0d324d77` | test(tls): bound macOS warmup and preserve handshake assertions | 114 |
| `7e46fa22ff2e30272475a2c0d3d8b49bbcc27516` | fix(tests): avoid guardian diagnostic deadlock and expose failures | 22 |

代码检查点 `7e46fa22ff2e30272475a2c0d3d8b49bbcc27516`。中间只改Git索引构造依赖阶段，未覆盖完整工作树；一次临时机械阶段EOF空行被cached diff-check拒绝，修正索引后继续，最终源码/身份不变。中间阶段未单独构建/测试，尤其backend准备阶段不等于功能完成。

用户已授权将全部阶段连同此前34个未推送祖先正常推送origin/test；推送不使用force，不创建发布tag。最终报告/提示词的提交SHA用git log查询；真实push确认与远端SHA在本轮最终回复给出。推送可自动触发fork-cargo-pr工作流，结果须另查，未执行workflow_dispatch、厂商请求或发布。
