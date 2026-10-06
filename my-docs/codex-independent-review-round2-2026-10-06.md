# Codex 第二轮独立审查（2026-10-06）

## 基线与范围

仓库 `/Volumes/soddygo/git-workspace/fork-nuwax-codex`，分支 test，起点 HEAD `8017fb76c55b00a023101803d1e7b3f79ce5a10d`；Claude 修改尚未提交。开始时22个 tracked changes（含一项 D）及7个untracked文件，与“已恢复快照”的总结不一致。记录过初始状态后仅恢复交接明确要求保留的 tracked `all__suite__scenarios__astra_kickoff_remote_compaction_windows.snap.new`。未reset/clean/stash，未覆盖配置、SQLite、既有tmp、.env.local。

读 AGENTS 与四个 code-review-* 技能，独立检查生产调用方、测试真实性、外部兼容和改动大小；路径注入按 path-types。保留的模型生产修复为：bridge请求调用既有ID preparation、default namespace不进入flat fallback、Guardian播种完整checkpoint envelope。不存在本轮声称的“本地加密摘要漏戳”：local compact生成visible summary。删除client-wide last-output缓存和不可信opaque来源猜测；保留remote_v2的本请求显式来源戳。

本轮授权修复和阶段commit，不push/发布，不真实厂商调用，不完整workspace，不CI dispatch。阶段提交和最终check记录见本文末尾。

## 全部发现与处理

行号为审查开始时或当前符号位置；已删除的代码用“原”标明。重复发现合并在同一项，未遗漏子审查问题。

| # /级别 | 文件位置 | 触发/影响 | 修复与边界 |
|---|---|---|---|
| 1 P1 | 原 `core/src/session/mod.rs:4017`、`client.rs:1847` | 最近请求不能证明未知旧checkpoint来源；记录点仅stream建立，非completed。替换未知opaque后会被误判同源。 | 删除缓存/记录/补戳，新增真实compaction完成后的recorded正例和unknown负例，实际Guardian wire与全envelope断言。正常生产3个replacement调用均不需要兜底；未发现正常resume直接触发。 |
| 2 P2 | 原 `core/src/compact.rs:369`；总报告:305/325 | 本地构造CompactionSummary可见Message，“加密摘要漏戳”归因错误。 | 撤销多余戳与第四缺陷宣称；Guardian原metadata丢失为实际保留修复。 |
| 3 P1 | 原 `core/tests/suite/scenarios.rs:570` | unsafe改HOME，无恢复，污染Bazel同进程/并行测试；TempDir删除后仍指向失效目录。 | HostSkillsService实例级AbsolutePathBuf注入与缓存失效；两个Astra测试不改进程环境。 |
| 4 P2 | 同上:570 | Windows dirs::home_dir使用Known Folder Profile，不读HOME；原隔离无效。 | 显式home绕过平台全局探测；代码修复及本机回归通过，Windows实际验收仍缺。 |
| 5 P2 | `cli/tests/admin_startup_matrix.rs:391/451`（原） | 仅is_some不能证明失败未archive或改写文件。 | 原路径+完整字节+无archived副本+config比较，保留错误断言。 |
| 6 P2 | 同文件原:465/508 | pgrep两次采样漏过瞬时writer且受系统其他测试影响，错误/空匹配都归0。 | 删除假证明，改为owner QueueList与CLI精确submission ID/content一致。writer生命周期唯一性仍开放。 |
| 7 P2 | 同文件:211（原） | 2×2×2仅4cell却称完整矩阵。 | 补8cell，实际archive/unarchive/delete终态、daemon存活与config不变；不代替queue/install全矩阵。 |
| 8 P2 | `exec/tests/suite/nuwax_cross_process_scope.rs:295`（原） | 所有cell都改变进程随机credential-instance，无法独立检验credential/query rotation；get(1)漏额外POST，未验证实际auth/query/model。 | 恰2POST及path/query/auth/model断言，改名为跨进程保守降级；同进程same-source阳性/独立旋转负例仍是后续任务。 |
| 9 P2 | 同文件原:138/195/229 | 继承4个CODEX预算控制项；阻塞child output无自身deadline。 | 清12个相关变量，child60s预算，保持现有nextest预算与断言。 |
| 10 P1 | `justfile:58`（原） | Python解释Bash，公共门禁入口SyntaxError，exit1。 | recipe调用bash；实际just入口执行Bazel验证。 |
| 11 P2 | `scripts/bazel_bridge_gate_negative.py:114`（原） | sleep后关闭listener却不join，零连接结论可能漏掉backlog。 | 观察直至child完成并排空accept队列；4个合成子进程负控包括真实TCP连接。不打印原始stderr/请求。 |
| 12 P2 | `scripts/bazel_bridge_gate.sh:28/65` | 数量阈值可设0；仅总数不足以证明三线；负控留下bazel-bin指向关桥产物。 | 正整数阈值+3个具名wire test通过，显式on/off，退出时恢复公开on产物。 |
| 13 P1 | `scripts/d4_stage_profiling.py` 原initialize帧/argv/readline | LSP帧不符合JSONL，--listen stdio不合法；阻塞readline使60s判断无效；stderr停止读取可能阻塞child，旧“bind≈10ms”非成功初始化证据。 | stdio://、JSONL、队列真实deadline、持续drain、成功result验证，isolated cwd/home/SQLite/env；stdio bind明确N/A。4个子进程回归与实际app-server握手。首轮真实握手暴露argv错误后修复，不以mock绿冒充实际通过。 |
| 14 P1 | cap Spec原:18 | 删除app-server turn/completed将删掉status=failed收尾，exec据此输出turn.failed并退出。 | 设计区分桥成功Completed、Core错误关闭、app-server失败关闭、exec失败；未实施新产品行为。 |
| 15 P2 | cap Spec原:17/20、Plan原:12 | 声称partial deltas已落盘，实际rollout policy不持久化delta或独立Error。 | 设计增加有界partial持久化/读取路径及legacy/paginated/resume验收。 |
| 16 P2 | cap Spec原:21 | “全部零schema变更”与必填数字usage/失败类型仅error矛盾。 | 第0步兼容/schema决策，明确Unknown≠0、累计完整性和客户端迁移待实现。 |
| 17 P2 | cap Plan原:9 | 只从Responses completed取usage不能保留cap数据，实际cap为incomplete。 | 设计改从response.incomplete.response.usage在映射失败前提取，并测有/无usage。 |
| 18 P2 | D6 Spec新增:33-60 | API名称、JSON bytes、有限样本tokenizer一致不证明Exact/上界；厂商framing事实未核。 | 全部候选Unverified，保留40,960-byte legacy与P0人工复审；未实施token-aware。 |
| 19 P2 | 总报告:326/末尾、tasks | 117/124不同集合不构成决定性A/B；单跑过不能证明负载因果；源码相同不豁免“fix/fmt后不测试”。 | 撤回严格更优/全383归因/第四缺陷等宣称，保留历史flake证据与首轮失败，明确顺序要求。 |
| 20 P2 | tracked `.snap.new` | 总结说恢复，实际D；测试运行也可能清理pending文件。 | 从HEAD恢复并最终比对字节；不接受/清理其他快照。 |
| 21 P2 | 约1700行独立功能混合 | 生产、Bazel、行政、诊断和设计一次不可review。 | 按调用依赖分阶段保存，复杂批<500、普通批<800；中间批未被逐个测试。 |
| 22 P1 | `.github/workflows/fork-cargo-pr.yml:72/末尾` | “offline”workspace无过滤会选择live binaries，重演历史误跑。 | 两平台命令排除exec_live/bridge_live并retries0；Bazel job运行8个Python helper回归。workflow仅review diff，未dispatch。 |
| 23 P2 | `ext/skills/src/host_service_tests.rs:381/425` | 完整项目回归暴露两旧测试仍读真实用户技能，预期2项被ego/gpui等污染。 | 用实例级home限定fixture，完整断言不变；skills项目177/177复验。 |

兼容性复核：Azure既有test的provider_id/bridge均None并不意味着native。开启rust-rig时uses_model_bridge判第三方、None默认Rig，故现有Azure五项确实测试新ID preparation；prefixed保留、legacy/empty删除。此前“缺bridge回归”的疑点经调用链核对撤销，没有添加重复测试。没有ConfigToml/依赖/外部RPC/schema形状变化；history checkpoint携带既有metadata及其唯一Guardian消费者一起保存，metadata不入模型wire。

## 测试与首轮失败

所有Rust测试使用隔离 `CARGO_TARGET_DIR=/Volumes/soddygo/git-workspace/codex-targets/validation-20261005`，显式 `codex-core/rust-rig`，`--offline --locked --retries 0 --test-threads 2`，只选择相关项目，不是完整workspace。

最早启动批选中1200测试；在可读工具输出中两个新Guardian来源用例通过、一个request_permissions用例于5s等待预算失败（Elapsed；随后Tokio runtime shutdown panic）。后续用户继续时工具会话及/tmp日志已失效，最终汇总不可核实，不能计为完整pass；改为repo忽略目录logs持久保存并重跑同图。

有完整日志的批次：1200 run /1198 pass /2 fail /6888 skipped，362.966s，exit100；两个skills fixture失败的diff明确出现本机用户技能。修复后同图仅选择skills项目177 run/177 pass/0 skipped，1.804s，exit0。按名称去重1200不同测试最终通过，不把177复验重复加成1377。先前guardian超时用例此批PASS 4.578s，deferred_executor PASS 4.231s；不能由此证明负载因果或全部383无回归。

Python门禁负控4/4、profiling回归4/4。本轮profiling首次真实binary调用因非法listen值exit1，日志保存；修为stdio://后实际initialize result成功，exit0：spawn call0.0017s、首stderr0.2787s、initialize roundtrip0.3362s、总0.3571s，load≈7.20；stdio bind=N/A。这是单样本局部握手证据，不是D4高载失败定位。

完整命令（仓库根；输出到logs/review-round2对应文件）：

```sh
export CARGO_TARGET_DIR=/Volumes/soddygo/git-workspace/codex-targets/validation-20261005
just test -p codex-api -p codex-rust-rig-bridge -p codex-live-tests -p codex-exec -p codex-cli -p codex-tui -p codex-config -p codex-utils-cli -p codex-core -p codex-app-server -p codex-app-server-protocol -p codex-history -p codex-skills-extension --features codex-core/rust-rig -E 'package(codex-api) | package(codex-rust-rig-bridge) | package(codex-history) | package(codex-skills-extension) | (package(codex-live-tests) & not binary(exec_live) & not binary(bridge_live)) | (package(codex-exec) & test(nuwax)) | (package(codex-cli) & (binary(admin_startup_matrix) | binary(queue_owner_matrix) | binary(queue_owner_dispatch) | binary(nuwax_session_remote))) | (package(codex-core) & (test(guardian) | test(azure) | test(flat_name) | test(registry) | test(rig_output_cap) | test(rig_anthropic) | test(rig_responses_bridge) | test(model_output_projection) | test(astra_kickoff_with_skills_plugins_and_remote_compaction) | test(astra_refreshes_plugin_tools_and_skills_in_an_existing_thread))) | (package(codex-app-server) & (test(thread_resume_echo_override) | test(thread_resume_same_provider_echo_uses_current_default_model))) | (package(codex-app-server-protocol) & test(schema_fixtures_tests))' --offline --locked --retries 0 --test-threads 2
just test -p codex-api -p codex-rust-rig-bridge -p codex-live-tests -p codex-exec -p codex-cli -p codex-tui -p codex-config -p codex-utils-cli -p codex-core -p codex-app-server -p codex-app-server-protocol -p codex-history -p codex-skills-extension --features codex-core/rust-rig -E 'package(codex-skills-extension)' --offline --locked --retries 0 --test-threads 2
python3 -m unittest discover -s scripts -p '*gate_tests.py' -v
python3 -m unittest discover -s scripts -p 'd4_stage_profiling_tests.py' -v
python3 scripts/d4_stage_profiling.py 1 /Volumes/soddygo/git-workspace/codex-targets/validation-20261005/debug/codex-app-server
CODEX_BAZEL_JOBS=4 just bazel-bridge-gate
```

日志：`logs/review-round2/rust-regression.log`、`skills-rerun.log`、`python-gate.log`、`python-profiling.log`、`profiling-real-binary.log`（首轮失败）、`profiling-real-binary-rerun.log`、`bazel-gate.log`。日志在ignored目录，不进入commit；脱敏identity/counts及历史383清单另存 `my-docs/validation-2026-10-06-round2/`。没有真实厂商、完整workspace、Linux/Windows或CI结果，未升级旧live证据。

## 剩余开发与保存

`claude-code-followup-2026-10-06-round2.md` 是下一轮任务：未归因失败的受控A/B、同进程独立opaque轮换、writer生命周期、cap兼容决策与分批实施、跨平台/CI/D4真实socket阶段；D6仍P0未实现。历史383清单仅核数量与源摘要，不冒充逐项根因关闭。

Bazel最终公共入口19 run/19 pass/0 fail/0 ignore/100 filtered，13.56s；关桥负控零连接，结束恢复on产物，exit0。完整构建耗时与两次门禁分别见bazel-gate.log/bazel-gate-final.log，不把重复执行计成38个不同测试。最终Python8/8；stdio最后样本initialize0.3772s，总0.3969s、load≈4.54，均为成功握手，非高载因果证明。

身份清单：validation-2026-10-06-round2/identity-before-fix.json，26个代码/构建/测试文件hash、Cargo和Bazel实际bin路径/SHA/bytes、日志hash、去重计数。Bazel exec SHA df881ba795c571d21e1d0040bb534db1daa6e87f5c91e11e895e87f3d9e18800；Cargo exec SHA 32fd7ac801d6eefe0b9a784813805fd4d6844dfbd077ec09cb787f1f073fca9e。预format的dirty树测试，不声称最终commit与旧binary byte-for-byte同源；final hash另记。门禁与profiling仅脱敏事实JSON保存，不提交原始request/auth。

final checks及阶段commit执行后追加；最后fix/fmt之后不测试。


## 最终收尾与阶段保存

```sh
# codex-rs目录，同一隔离CARGO_TARGET_DIR
just fix -p codex-core -p codex-history -p codex-skills-extension -p codex-cli -p codex-exec --features codex-core/rust-rig --offline --locked
# unwrap_used警告仅改诊断为expect后，再检查exec
just fix -p codex-exec --features codex-core/rust-rig --offline --locked
just fmt
# 仓库根
git diff --check
```

两次fix exit0（4m20s/3m01s）；首轮一项unwrap_used warning，改expect后最终lint无警告。fmt与diff-check exit0。26个最终源码hash、测试后变化文件在identity-after-format.json；变化为fixture诊断和格式化，无放宽断言/deadline、跳过或新增产品行为。final fix/fmt之后不再测试。tracked pending快照字节仍与起点HEAD一致。没有ConfigToml/依赖/外部API形状变化，不需要额外锁/schema更新；SDK生成脚本只pin子进程Python默认3.13。

代码阶段commit列表保存后追加，文档批自身SHA用git log路径查询，避免自引用。中间阶段没有逐个测试；本报告证据属于完整修复树。


| commit | 范围 | changed lines（加+删） |
|---|---|---|
| `5e0753ebd0e75c92cc0d2be96272c84c776adb5c` | fix(core): prepare outbound items on model bridge requests | 5 |
| `5d6003b35d4828cc866efd81fd6f07a2bd4b2aa4` | fix(core): exclude default tools from flat fallback | 30 |
| `e9996566b56e20876587a700f7c672d8bf36e34b` | fix(guardian): preserve checkpoint producer metadata | 257 |
| `1cc543c1d911380a00f2dc2218a505a0219d6f40` | fix(skills): isolate user home per host service | 159 |
| `623d74304e7d5ec28bcf1125ae0c91deedf55d24` | test(cli): cover administrative startup combinations | 379 |
| `3a31f4df8318098530fbdd1ca814c31f52681aa1` | test(cli): verify failed administration and queued ownership | 218 |
| `7a0bc56ca4f63f697ad457c8bf096aaf2d5d07bd` | test(exec): verify conservative cross-process replay | 365 |
| `83be3c3e4373f0825d4fd11faa5d4d7646b068b6` | test(bazel): gate public model bridge paths | 339 |
| `4e864a3102411d7970a95d49917e29748a15a359` | fix(codegen): default SDK generation to Python 3.13 | 7 |
| `90877f99dc04a2a7d24ac3916392d6e1a17b0f99` | fix(diagnostics): measure successful app-server initialization | 248 |
| `0ad8350932cbfe41b65b78999dbd24ff278f00b4` | docs: correct cap failure and token-budget proposals | 85 |
| `c231b20f0bf1ea4c2f83e67ba686308895feb86e` | docs: preserve historical workspace failure inventory | 391 |

最后文档批保存审查结果、校正总报告/tasks、Claude后续任务及脱敏identity/gate/profiling证据。该批SHA用 `git log -1 -- my-docs/codex-independent-review-round2-2026-10-06.md` 查询；不push。所有代码阶段<500行，文档阶段<800；原完整工作树未被中间索引拆分覆盖。
