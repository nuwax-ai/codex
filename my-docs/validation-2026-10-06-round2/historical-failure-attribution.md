# 历史失败清单逐项归因（2026-10-06 第三轮，Claude）

依据：`historical-workspace-failures.md`（383 项，Oct-05 attempt-5 记录）+ 本轮真正配对 A/B + 直接环境证据。
**归因纪律**：仅时序关联不写因果；单跑通过只作参考证据；配对 A/B 是回归归属的唯一依据。

## 配对 A/B（本轮新增的决定性证据）

方法：独立 worktree checkout 基线 `8017fb76c`（/Volumes/soddygo/git-workspace/codex-tmp/ab-baseline，不触碰工作树），与当前树（HEAD `47dabce9b`）运行**完全相同**的 119 测试集（六优先类全集：deferred guardian、mcp_grace×4、skill mcp oauth、session decider、exec-server registration_retry/direct/noise/oauth_http_client、network-proxy http_connect/mitm/socks5/runtime、otel×6、install-context brew、aws-auth imds）。同参数：`--offline --locked --retries 0 --test-threads 1`、同 feature 图（`--features codex-core/rust-rig`）、同 6 包选择、同环境（PATH/TMPDIR/RUSTUP/CARGO_HOME）、独立 target dir。运行顺序 BASE→BASE→CURR（交错受首轮 CURR 编译错误打断：新测试文件类型错误，修复后补跑）。负载记录于各日志头部（7.2–10.9 区间，两树交错覆盖）。

| 轮 | 树 | 结果 | 日志 |
|---|---|---|---|
| BASE r1 | 8017fb76c | 119 run / 99 pass / 20 fail | codex-tmp/ab-paired/BASE-round1.log |
| BASE r2 | 8017fb76c | 119 run / 99 pass / 20 fail（与 r1 集合相同） | BASE-round2.log |
| CURR r3 | 47dabce9b | 119 run / 99 pass / 20 fail | CURR-round3.log |

**三轮失败集合逐名对比零差异（comm 为空）**：20 项失败在基线与当前树完全一致 → 全部为既有问题，与本轮/上轮改动无关的回归归属成立。

## 逐类归因表

| 类（历史数） | 归因 | 证据链 |
|---|---|---|
| network-proxy http_connect/mitm/socks5/runtime（22 FAIL） | **已确认环境：本机 fake-ip DNS** | 直接证据：`api.github.com`→198.18.1.89、`example.com`→198.18.4.139（实测 getaddrinfo）；198.18.0.0/15 在 Rust `Ipv4Addr::is_private()` 为真（policy.rs:518 自带断言 `is_non_public_ip("198.18.0.1")`）；失败签名一致（"blocked-by-allowlist" 先于 mitm 判定/"Sandbox policy blocks local/private network addresses"）。配对 A/B：两树确定性同样失败（0.15s 快失败，非时序） |
| core session::tests::managed_network_proxy_decider（1） | **已确认环境：同上 fake-ip** | 同一 DNS 证据；decider 直接打印解析后的 403 响应含 `"host":"example.com","reason":"not_allowed_local"`。配对 A/B 两树同败 |
| install-context brew（1） | **已确认环境：本机真实 brew cask 安装** | 直接证据：`/opt/homebrew/bin/codex -> /opt/homebrew/Caskroom/codex/0.160.0/bin/codex`（实测符号链接）；fixture 假路径被真实布局解析。配对 A/B 两树同败 |
| mcp_optional_startup_grace（6：4 FAIL+2 其他） | **flaky（历史高载），当前已不可复现** | 配对 A/B：4 用例在两树三轮全部 PASS（0.18–0.36s）；历史失败发生于全库并发高载窗口。无产品缺陷证据；不作"机器速度"因果断言，登记为 load-sensitive flaky |
| deferred_executor_guardian（1 TMT） | **flaky（历史高载），当前已不可复现** | 配对 A/B 两树三轮 PASS（0.26–0.3s）。Codex 二轮报告亦记录其在本修复树 1200 批 PASS 4.231s。不归因负载为"根因"——仅登记时序关联 |
| exec-server registration_retry 15+oauth_http_client 1（FAIL）+direct/noise 3（TMT） | **flaky（历史高载），当前已不可复现** | 配对 A/B：全部 PASS（0.05–2.6s）。此前"macOS TLS 首建 ~1s"假说被配对数据否定（同机同载两树全过）——撤回该归因 |
| otel otlp_http_exporter（6） | **flaky（历史高载），当前已不可复现** | 配对 A/B 三轮 6/6 PASS（~3.0s 各） |
| aws-auth real_imds（1） | **flaky（历史），当前不可复现** | 配对 A/B 三轮 PASS（0.18s）。注：本机无 IMDS 端点，该测试用 mock；此前"无 IMDS 环境"归因不当，撤回 |
| core azure_responses_request（1）+tools registry（2） | **fork 缺陷，已修复并回归** | Codex 二轮确认保留修复（5e0753ebd/5d6003b35）+ 既有回归测试全绿 |
| guardian compaction（unit+scenario 2） | **fork 语义 vs 注入前提**，guardian envelope 传递已修复（e9996566b） | Codex 二轮确认；剩余为合成注入无 provenance 的降级语义，非缺陷 |
| astra scenarios（3，含 tracked snap.new 关联 2） | **已确认环境：用户技能泄漏**，已由实例级 home 隔离修复（1cc543c1d） | Codex 二轮 177/177 复验 |
| schema_fixtures（1） | **fixture 漂移**，已修复（9069fb90c） | Codex 二轮确认 |
| tui 46 TMT+9 FAIL、app-server guardian_v2 124、realtime 10、turn_input 7、unified_exec 25、core 其余、exec-server 6 TMT、rmcp 14、http-client 10、skills 2 等（合计 ~300） | **未归因（本机未复验）** | 历史全库高载窗口一次性记录；本轮未获完整 workspace 授权，未逐项复跑。多数落在重子进程套件，与已证 flaky 类同形态，但在复验前保持未归因 |

## 纪律说明

- 上述 flaky 类的"当前不可复现"是**本机本轮证据**，不是断言永不再现；根因定位需授权后于完整 workspace 复验窗口进行。
- 已撤回此前两处不当归因（macOS TLS 首建、无 IMDS 环境），以配对数据为准。
- network-proxy/decider/brew 三类环境归因随附可复现命令（getaddrinfo/ls 实测），Codex 复核可直接重跑。

## 2026-10-07 完整 workspace 复验结论（授权后执行）

完整口径 `just test -E 'not binary(exec_live) & not binary(bridge_live)' --offline --locked --retries 0`（21,576 run：18,512 pass / 2,060 fail + 1,004 timeout / 68 skip；日志 ws-20261006-round3.log）。原三级名称统计 3,062 → 恢复 570 → 再恢复 2,164 → 仍败 328 保留为历史记录，但漏掉 SIGABRT 并合并跨 binary 同名测试。独立离线复算按 `(binary, test_name)` 为 **3,064 → 恢复 570 → 再恢复 2,165 → 本次仍败 328 + 未复跑 SIGABRT 1**；nextest 的 2,060 fail 是 FAIL 2,059 + SIGABRT 1。名称去重首试为 3,063，不是原登记 3,062。单跑 `abort_lifecycle…shutdown` 通过只证明该次执行结果，不证明负载因果，也不是遗漏的 stack overflow 用例。

**对 383 的复验状态分布**（完整测试身份逐项与日志一致；状态复验完成，根因与门禁未收口）：

- 本次 threads=2 仍败 **249**：201 项归因仍未知、20 项已有环境证据（network-proxy 18 + decider 1 + brew 1）、28 项保留此前 A/B 三轮通过的 flaky 证据；这 28 项此后再次失败，旧表的“当前不可复现”仅指当时运行窗口，不能当作当前关闭状态；
- 全套重载首试即过 **34**（含已修复项 schema_fixtures/azure_responses/astra 隔离/host_service——与修复史吻合）；
- 首次分类复跑恢复 **11**、threads=2 恢复 **89**——100 项观察到不同运行条件下结果变化；时间与并发度都变了，负载因果待定位。

**328 中的 79 项不在历史 383**：配对复跑仅支持所选 79 项未发现持续树差异，不证明全套零回归。漏项 `codex-core guardian::tests::guardian_ephemeral_retry_preserves_parallel_trunk_and_fork_history` 在完整首试中 SIGABRT（stack overflow），不在三级复跑名单内，继续开放。201 历史未归因项继续逐项定位。复算工具 `scripts/nextest_log_summary.py` 与脱敏元数据 `/Volumes/soddygo/git-workspace/fork-nuwax-codex/logs/nextest-round3-{audit,paired-audit}.json` 的命令、完整计数和输入 SHA-256 见 other-computer-validation-results.md 纠正节；原始请求诊断未复制。
