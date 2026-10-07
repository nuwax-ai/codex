# Codex独立复审与完成路线（2026-10-07）

## 结论与基线

HEAD `47dabce9b3ffde8beb60d9ce30fe99e1a7080bfc`，test分支，开始6个tracked modified+6个untracked；Clause总结“无遗留问题/全部收口”不成立。三协议Rig、进程env和会话投影主能力已实现，但生产门禁未全绿；按交付边界尚有**2个实质功能包+6个验收/稳定性包**，详见 `claude-code-completion-plan-2026-10-07.md`，不是API计数或完成百分比。cap和D6仍设计，201项未归因、平台/厂商/CI继续开放。

读根AGENTS和四个独立code-review技能。保留现有修改、配置、SQLite、tmp、tracked pending快照，没有reset/clean/stash，未完整workspace、厂商调用、CI dispatch或发布。原日志/worktree保留于兄弟codex-tmp/codex-targets；仅离线检查。

## 全部发现与处理

原行号为审查起点，重叠finding合并；保留所有独立审查结论。

| # /级别 | 位置 | 问题与影响 | 处理/边界 |
|---|---|---|---|
| 1 P1 | 总报告原462；ws-round3.log9158 | 失败名单只含FAIL/TIMEOUT，漏stack overflow SIGABRT；以name去重又合并跨binary同名。 | 新nextest_log_summary按run/序号及(binary,name)解析所有终态/重印/冲突；3064=570+2165+328+1abort。补abort精确用例，未把单跑PASS当根因关闭。 |
| 2 P2 | tasks原40、labels表 | “383全部根因收口”与249中201明确未归因冲突。 | 状态249/34/11/89与日志一致，但249=20环境+28此前flaky标签+201未归因。79所选项没持续树差异，不是全套零回归或负载因果证明。 |
| 3 P2 | source_rotation原166 | query负例同时改名字，endpoint identity变化掩盖private值隔离。 | 固定query-name集合，只变值/重复顺序/空值，补provider.query_params；持久来源endpoint identity相同、auth-domain变更分别断言。 |
| 4 P2 | source_rotation原132/260 | 仅body/model，没有真实path/query/auth/header；endpoint producer capture被丢。 | capture实际HTTP请求及exactattempt，深比较各维度，保留producer和resume。 |
| 5 P2 | source_rotation原67/149 | fixture无search ciphertext/citation，仅signature contains，报告夸大hosted验证。 | 三类opaque完整fixture、正例完整块顺序深比，负例全部消失/可见文本保留、原rollout只追加。原6+1，修后10+1场景，2函数。 |
| 6 P2 | lifecycle原239；contract原36 | QueueList不加载，未有owner持锁正校准；goal RPC前停probe，错误称写入期锁。 | 实际ThreadResume建立live recorder，held到unsubscribe/ThreadClosed，验证Free→Held→Free和queued turn。 |
| 7 P2 | lifecycle原123 | 不取coordination，可能锁旧inode并干扰生产。 | coordination下只open现有thread锁，probe释放先于coordination释放。 |
| 8 P2 | lifecycle原198/239 | abort不await，后台Err丢弃。 | 停止信号+await传播结果，并补ready后错误负控。 |
| 9 P2 | lifecycle原230 | 非空样本可全部在client退出后，无时间/ready。 | ready屏障、采样始末时间、保守真实运行窗口样本；50ms/PID限制仍不声称任意时刻绝无。 |
| 10 P1设计 | Step0原9/11 | 排除legacy保存/resume与既有双模式契约冲突。 | 提案要求legacy/paginated均有有界保存/读取/恢复；旧读取器未知变体行为需验证，仍未批准/实施。 |
| 11 P1设计 | Step0原33 | live失败通知usage缺席不持久，后续成功/重连/resume丢累计incomplete。 | 提案明确持久化及可重放completeness/unknown，v2 nullable；不预设wire省略兼容。 |
| 12 P2设计 | Step0原16/31 | Responses不在converter，ThreadUsage不是total/last类型。 | 纳入codex-api共用SSE/WebSocket解码；正名ThreadTokenUsage并区分账单ThreadUsage。 |
| 13 P1设计 | Step0原9/43 | “有界”缺具体token/byte/count上限/超限行为；D3错误否决终局前partial事件。 | 预算/旧读侧/事件顺序留具体裁决，要求core/context+ContextualUserFragment；未来>1k项P0人工审。未产生新实现超限项。 |
| 14 P1 | D4原WebSocketClient147/recv_text | Pong被当文本、分片序列缺验证、upgrade只看101、socket逐读timeout不保证总预算，buffer无上限。 | 新d4_websocket组件：accept及headers校验、ping/pong/fragment顺序、absolute deadline、帧/消息1MiB及upgrade64KiB诊断上限、失败关闭；真实socket负控。依据RFC6455 4.2.2/5.4/5.5，不改产品协议。 |
| 15 P2 | D4 spawn/drain/cleanup | stderr首line冒充首byte，pipe读线程未收敛、失败client/socket不可靠关闭，Unix子进程已退仍等bind。 | 记录首byte、join readers、finally关闭client/socket、Unix进程退出fail-fast；readyz绝对预算。 |
| 16 P2 | 总报告原434/441/478 | 每个样本新spawn+freshhome，不能称loaded-runtime warm；低高load相关性不能关闭历史timeout根因。 | 保留数值为历史采样，撤回warm/根因闭合；本轮三模式真实initialize仅单样本。 |
| 17 P2 | 总报告原430 HOME投毒 | canary放.claude/.codex而真实用户根.agents/skills，没证明污染真实root。 | 自建真正.agents canary，Bazel三目标146+21+10=177通过，未删原工件；Windows仍未验。 |
| 18 P1 | defs.bzl原integration rust_test分支 | 库启用feature但integration target没传，Core cfg桥用例编译排除；Bazelgreen实际0tests。 | 同步sharded/unsharded/windows-cross三个分支crate_features。重验实际2tests/2361filtered，无guard skip，真实HTTP深断言；长期bridge gate加入Core非零且不跳过要求。 |
| 19 P2证据 | Cargo桥run | 2个显示PASS其实skip guard提前返回，不能当请求验证。 | --no-capture保存Skipping输出，原guard完全不改；Bazel正常环境执行替代。本轮不把这些CargoPASS升级验收，历史结果也须有真实request/工件才可信。 |
| 20 P2 | 整体1898行/新profiling554 | 独立功能混合，超review门槛。 | helper/transport/parser/CLI/旋转/writer/文档按依赖拆分；必要新模块和注册同批。 |

CLI新增archive test只验证字段合并，未发现外部API破坏，但不证明真实daemon/embedded选择，--no-daemon/strict/profile/oss公共矩阵留后续。没有改变ConfigToml、锁依赖、外部RPC或持久化类型。所有新增cap设计保持提案。

WebSocket规范依据：[RFC 6455](https://www.rfc-editor.org/rfc/rfc6455.html)；本轮通过ego-browser只读核对后关闭task space，无外部写入。

## 当前源码独立验证

日志在ignored `logs/review-round3/`，不提交raw请求/auth。target=`/Volumes/soddygo/git-workspace/codex-targets/validation-20261005`。本轮不重跑全套。

- workspace构建图精确选择4用例：writer lifecycle/错误负控/CLI参数/漏掉abort，4/4；重复显式rust-rig仍4/4，桥目标缺失并非通过。abort单跑4.407/4.684s通过，只登记未稳定复现。
- Core单包显式rust-rig选2，NextestPASS但stdout明确Skipping（network guard），实际请求验证skip=2。随后Bazel旧图running0，非验收。
- 修defs后Bazel Core实际running2，2pass/0fail/0ignored/2361filtered，1.84s，构建总60.624s；10+1旋转场景通过完整request/provenance/blocks/bytes断言。
- 正确user技能root HOME canary：三Bazel目标177/177，0fail；构建执行11.820s，非缓存test。
- D4 Python原14+新增8真实socket=22/22；outcome parser8/8。实际stdio/websocket/unix_socket各1sample，均initialize成功；bind约N/A/0.0768/0.0809s，initialize约0.4046/0.0031/0.0045s；load≈9.6–9.9，不能作为低高载因果结论。
- full日志离线复算3064执行身份=570第一轮恢复+2165第二轮恢复+328仍败+1漏SIGABRT；3063唯一name不能替代(binary,name)。383状态核对和所选79同窗结果保留，201根因继续开放。

命令（仓库根）：

```sh
export CARGO_TARGET_DIR=/Volumes/soddygo/git-workspace/codex-targets/validation-20261005
export RUSTY_V8_ARCHIVE=/Volumes/soddygo/git-workspace/codex-targets/rusty-v8-cache/librusty_v8.a.gz
export RUSTY_V8_SRC_BINDING_PATH=/Volumes/soddygo/git-workspace/codex-targets/rusty-v8-cache/src_binding.rs
just test --features codex-core/rust-rig -E 'not binary(exec_live) & not binary(bridge_live) & (test(anthropic_source_rotation_preserves_or_degrades_opaque_per_dimension) | test(anthropic_endpoint_rotation_degrades_opaque_and_keeps_visible) | binary(queue_writer_lifecycle) | test(archive_family_flags_reach_daemon_policy_fields) | test(guardian_ephemeral_retry_preserves_parallel_trunk_and_fork_history))' --offline --locked --retries 0 --test-threads 1
just test -p codex-core --features rust-rig -E 'test(source_rotation_tests)' --offline --locked --retries 0 --test-threads 1 --no-capture
bazelisk test //codex-rs/core:core-all-test --//:enable_model_bridges=true --jobs=4 --test_sharding_strategy=disabled --test_filter=source_rotation_tests --cache_test_results=no --test_output=all
bazelisk test //codex-rs/ext/skills:skills-unit-tests //codex-rs/ext/skills:skills-skills_extension-test //codex-rs/ext/skills:skills-executor_file_system_authority-test --jobs=4 --test_env=HOME=/Volumes/soddygo/git-workspace/fork-nuwax-codex/logs/review-round3/canary-home --cache_test_results=no --test_output=all
python3 -m unittest discover -s scripts -p 'd4*tests.py' -v
python3 -m unittest discover -s scripts -p 'nextest_log_summary_tests.py' -v
# mode分别stdio/websocket/unix_socket，各1sample
python3 scripts/d4_stage_profiling.py --mode websocket 1 /Volumes/soddygo/git-workspace/codex-targets/validation-20261005/debug/codex-app-server
python3 scripts/nextest_log_summary.py --classify /Volumes/soddygo/git-workspace/codex-tmp/ws-20261006-round3.log /Volumes/soddygo/git-workspace/codex-tmp/ws3-rerun-positional.log /Volumes/soddygo/git-workspace/codex-tmp/ws3-rerun-t2.log --output logs/review-round3/full-failure-audit.json
CODEX_BAZEL_JOBS=4 just bazel-bridge-gate
```

最早test过滤器用文件名而非module alias，匹配未到桥目标；原Bazelcrate feature缺失与Cargoguard均独立查清。未放宽deadline、弱化断言或改sandbox变量代码。

## 剩余范围与收尾

全部安排在 `claude-code-completion-plan-2026-10-07.md`：cap和D6两个功能包；workspace稳定性、行政owner、历史来源、跨平台、真实厂商、CI发布门禁六个验收包。可立即推进离线工作；未批准的语义、厂商请求、完整workspace/CI/推送发布在具体review结果完成后申请对应授权。

最终scoped fix、fmt、diff-check及源码/bin身份、阶段保存结果执行后追加；之后不再测试。源码/worktree不清理，旧验收数字保持历史范围，不以相同基线失败冒充生产完成。


## 最终检查与阶段保存

相关测试完成后，在codex-rs运行 `CARGO_TARGET_DIR=/Volumes/soddygo/git-workspace/codex-targets/validation-20261005 just fix -p codex-core -p codex-cli --features codex-core/rust-rig --offline --locked`，exit0、2m41s、无warnings；`just fmt`及根目录`git diff --check`均exit0。之后没有重跑tests。identity-before-fix/identity-after-format.json登记13个代码文件和实际Cargo/Bazel程序SHA；格式化变化单独列出，不冒充最终commit byte-identical旧binary receipt。

新增长期gate实际exec19/19+Core2/2（11场景），0fail/ignore，19与2分别filter100/2361；关桥拒绝非零退出、具名错误、0连接，产物恢复on。Bazel测试没有Skipping，Cargo桥2skip单列。

| commit | 范围 | changed lines |
|---|---|---|
| `8cabb8c76b765bb1ada945ddd934711e3e94557e` | test(cli): cover archive policy argument merging | 49 |
| `59c11e83b64cf529fdd455e658598534908fb4ac` | test(core): verify hosted replay across endpoint rotation | 422 |
| `370565482382d4cb21bb922e486fb12e17fdea9f` | test(core): verify private query and header rotation | 81 |
| `ae52f58b18d020d4837fce630049096166a8e656` | test(cli): observe loaded writer lock lifecycle | 493 |
| `1ea7eae92b48bcc397f62a867da51036914c97b6` | fix(diagnostics): validate bounded WebSocket control flow | 369 |
| `9ea691507a68b1636ee4e124d739cc65fe5b6095` | fix(diagnostics): profile socket transports reliably | 454 |
| `169e891c9a0bd561e30daa54d0f134884b139f7d` | fix(validation): include aborted and binary-distinct results | 407 |
| `d84faf2ecdbb0c88260138fd035ae0e38e930e5c` | fix(bazel): propagate integration test features and gate coverage | 19 |
| `3ae87e2bfae89a7f534ca29120e7a1ac60848c64` | docs: refine pending cap compatibility decisions | 54 |
| `bde8d62f6a2c5e3f7342fd9131caa8b4ed75b180` | docs: distinguish failure status from root-cause closure | 601 |
| `2b2c997fc87c6615f6941842e7aa3e12b4ef02d8` | docs: retain independent outcome audit summaries | 551 |

最后审查/完整任务书/身份文档批SHA可用git log路径查询；不push。所有代码批<500、普通文档批<800；中间索引拆分不改完整工作树，中间commit未单独构建测试。
