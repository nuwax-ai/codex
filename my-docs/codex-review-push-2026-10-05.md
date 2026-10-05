# Codex 验收复查与提交记录（2026-10-05）

基线：test 分支 `11f86b03e6a2a5302c490134226c4b5e60fb1a73`。包含上一轮未提交修复与 Claude 新验收资产。用户本轮授权修复、commit、push origin/test；远端 fetch 后与基线无分歧。运行时 SQLite/tmp/.env.local 不纳入提交。

## 确认发现（行号以修复前资产为准）

1. **P1，SSE 终止回归。** `codex-rust-rig-bridge/src/sse.rs:148` 把空字符串或次候选 finish_reason 认作终止，与 Rig 的主候选/空字符串规则不同；之后会丢掉真实 length 并错误完成。修为主候选非空字符串判定；首终止后丢弃迟到主候选内容，保留 usage-only 和 DONE。新五个真实 HTTP 回归；EOF/idle 截断契约不变。
2. **P2，idle 测试没有 HTTP。** `exec/tests/suite/nuwax_env_controls.rs:274` 直接写 SSE 到 socket；冷启动耗时可掩盖无效 HTTP。修成合法 HTTP/SSE，从首帧到客户端关闭计时，核对请求/模型、timeout 原因、无后续连接，并观察 gateway 结果，不扩大 deadline。
3. **P2，取消断言恒真及平台缺口。** 同文件 `386,408` 无 Unix gate 调 kill、无界等待，任何退出都满足断言。修为 Unix 条件、实际 retry trace 同步、日志有界、child kill-on-drop/有界等待，校验应用处理后的 exit=1。Windows 原生 Ctrl-C、SSE-wait 取消继续列为未验。
4. **P2，并行配置断言无效。** 同文件 `517` 只检查运行前 Chat 配置、丢弃另一快照。修为两个进程运行后各自完整字节比较。立即完成的 SSE 仍不足以证明并行 idle 值的行为差异，未将该部分升级完成。
5. **P2，warm/running fixture 错误。** `app-server/tests/suite/v2/thread_resume.rs:6541,6654` 期待三请求但 helper 只发一 seed；running 挂两个即时响应，真实第二请求不受延迟。修为两请求和真实 response gate，等待 assistant item 开始后 echo，断言仍 Active，释放 gate 后核对两次真实 model。不是单凭环境负载可以解释的失败。
6. **P2，raw 捕获误验。** `live-tests/src/binary_turns/capture_validation.rs:79,114` 依赖便利 body；合法 1e999 可使其为 null，便利视图还能掩盖错误 raw model/cap。修为借用顶层 RawValue，仅缺 body_raw 时可 fallback；保留数字/出站字节。假 fixture 的 raw 也补真实 model。
7. **P2，Anthropic endpoint 误拒。** 同文件 `63` 未覆盖 SDK 支持的 /messages 和 /v1/messages base。按固定 SDK suffix 顺序归一并保留 namespace；补所有形态负控。
8. **P2，测试隔离。** `cli/tests/queue_owner_matrix.rs:57,122` 继承 CODEX_SQLITE_HOME 可访问外部数据库；新 owner/客户端清除该变量，以独立 home fallback。缺 provider 回归现在匹配 provider not found，不能接受任意 transport/timeout 错误。
9. **P3，inactive fixture 没实际断开。** `cli/tests/nuwax_session_remote.rs:178` 持有 stream 后等退出。显式 drop 后等待，不靠产品握手 timeout。
10. **P2，历史 live 收据混用。** Claude 报告/任务称 live4 共用 97720d19/c17787a4，实际 Chat/Anthropic 分别来自两份源码，较早 Anthropic field_mismatch。已纠正，脱敏摘要单列 4 场景/8 次捕获和较早两次失败，不提交 raw logs。
11. **P2，604 证据范围。** 历史 final-sweep.log 确有 604 PASS，但没有执行 nuwax_session_remote；完整 -E 未从日志恢复。报告更正，本轮命令和结果另外登记。
12. **P1/P0 设计阻断，D6。** 原 Plan 用经验 bytes/4 与 95 分位系数当保守 token 上限，不能证明硬限制；DropOpaque/DropAll 又改变现行超限 fail-fast。Spec/Plan 已修订为未实施的 exact/proven-upper-bound、KeepWholeOrFail；P0 继续开放，不缩小 cap、不截断签名、不重写历史。

## 实际覆盖与剩余边界

- B2 已有实际 owner、retarget UUID/名字、缺 provider 队列保留、同名歧义；缺 owner/embedded、running owner、active client 向已有 owner、writer/RPC 数量完整矩阵仍待补。
- B3 active/incomplete 覆盖四命令，新增 corrupted/inactive 仅 archive；行政启动 install-method/daemon/embedded 矩阵未完成。
- C2 新文件原 8 个测试，加原 Chat retry 计为 9；三协议握手、Chat resampling、坏值及并发 model/auth 已有。SSE-wait 取消、Windows 原生取消、并行 idle 观测尚未完成。
- cap 终止的 usage/Done 保留及 warm/cold 统一是独立产品问题，不合成成功 Completed。真正低载冷启动、跨平台、Bazel、完整 workspace、远程 CI、Step、跨进程 opaque、加密引用和 live cap 触顶未升级验收。
- D4 的 dyld 栈/代码签名大小只能定位或提示成本；没有低载及分项 profiling 时不能宣称签名校验是已排除其他原因的根因。

## 本轮证据

相关 10 包、显式 core/rust-rig、just test、offline/locked/retries=0，排除 exec_live/bridge_live。所有模型请求为本地 mock，未重复付费 live。

- 首轮：exit 100，628 run / 626 pass / 2 fail / 12,996 filtered；501.463s，构建 19m00s。日志 `/tmp/codex-oct05-review-tests.log`。
- 两失败是新增精确断言错误：queue 实际拒绝为保留 provider 的环境组未激活（不是 generic not found）；idle 文案为 request timed out（不是 timeout 单词）。只修测试期望，未改产品逻辑或 deadline。
- 同图定向复验：exit 0，15/15 pass（1 slow）、171 filtered，60.489s。包括两失败、全部新 controls、owner/remote；日志 `/tmp/codex-oct05-review-rerun.log`。不与首轮求和为唯一用例总数。
- 修复后的 warm/running、三协议 Core cap、五个 Chat terminal、raw capture、实际 Retry-After/SIGINT 及并行配置后置断言均实际执行。
- 测试后 scoped just fix（8 包）exit 0（20m23s），有 raw helper 命名及测试单次 loop 两告警；两处等价修改后，just clippy -p codex-live-tests -p codex-exec --offline --locked -- -D warnings 终检 exit 0、零告警（7m09s）。最终 just fmt exit 0、git diff --check exit 0。按 AGENTS，最终 fix/fmt 后未重跑测试。日志为 /tmp/codex-oct05-review-fix.log、/tmp/codex-oct05-review-final-clippy.log、/tmp/codex-oct05-review-fmt.log。

可复验的首轮过滤表达式（package 图保持 api、rig-bridge、live-tests、exec、cli、tui、config、utils-cli、core、app-server）：

```text
package(codex-api) | package(codex-rust-rig-bridge) | (package(codex-live-tests) & not binary(exec_live) & not binary(bridge_live)) | (package(codex-exec) & test(nuwax_env)) | (package(codex-cli) & (binary(queue) | binary(queue_owner_matrix) | binary(nuwax_session_remote))) | (package(codex-tui) & (test(named_session_lookup) | test(session_archive_commands) | test(session_queue_commands) | test(app_server_target))) | (package(codex-config) & test(env_group)) | (package(codex-utils-cli) & test(nuwax)) | (package(codex-core) & (test(rig_output_cap) | test(rig_anthropic) | test(model_output_projection))) | (package(codex-app-server) & (test(thread_resume_echo_override_is_ignored_on_loaded_and_running_threads) | test(thread_resume_same_provider_echo_uses_current_default_model)))
```

对应命令：上述每个包 `-p` 选中，`just test --features codex-core/rust-rig -E '<上述表达式>' --offline --locked --retries 0 --test-threads 2`。定向复验的 -E 为 `(package(codex-exec) & test(nuwax_env_controls)) | (package(codex-cli) & (binary(queue_owner_matrix) | binary(nuwax_session_remote)))`。所有计数为实际执行记录，不将历史 Claude 604 或 live4 算为本轮当前树证据。

计划按源码依赖分批提交；只对完整最终树运行相关验证，不能将分批中间树称为逐个实测。严格避免 registration 引用未提交的新模块；不重写旧提交或 force-push。后续入口见 `other-computer-handoff-2026-10-05.md`。

## 提交与远端核对

分批提交脚本只暂存审查清单，逐批检查 cached diff/凭据匹配；排除运行时文件。最终推送与远端 SHA 以聊天回执为准，后续入口文档所属的首次加入提交即 checkpoint。每批依赖由源码审查确认；完整最终树已有上述测试证据，分批中间树没有逐一重新构建/测试。
