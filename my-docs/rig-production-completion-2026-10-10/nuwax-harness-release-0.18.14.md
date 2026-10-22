# Nuwax Harness 0.18.14 发布与验收记录

更新日期：2026-10-23。**v0.18.14 已推送；Linux/Windows 五目标成功，macOS 两目标仍在构建，npm 尚未发布。**
前一候选 v0.18.13 的 Release 由维护者主动取消，从未发布到 npm；原 tag 保留，不覆盖或改写。

## 当前交付坐标

| 项目 | 坐标或状态 |
| --- | --- |
| 权威工作副本 | `/Volumes/zhitai/git_workspace/fork-nuwax-codex`，`test` 分支 |
| 当前发布 tag | `v0.18.14` |
| tag 对应源码 | `f8273097515a97dea54d4ffe4e7a37e51bd30de1` |
| Release CI | [run 39792664843](https://github.com/nuwax-ai/codex/actions/runs/39792664843)，五目标成功、macOS 两目标运行中 |
| npm 包 / 目标版本 | `nuwax-codex` / `0.18.14`；实际 registry 发布结果待回填 |
| 主二进制 / 主命令 | `nuwax-harness`，Windows 为 `nuwax-harness.exe` |
| 兼容入口 | 保留 `nuwax-codex` npm 命令与归档内同内容别名，供现有 ACP 使用 |
| 默认数据目录 | `~/.nuwax-harness`；显式 `CODEX_HOME` 优先，显式路径须预先创建 |
| npm 下载缓存 | `~/.nuwax-harness-cache`，与会话数据目录分开 |

用户级旧目录不做兼容、迁移或删除。项目级 `.codex/` 仍属于项目配置/资源加载规则，
与用户级默认目录改名分开。入口与示例见[环境变量指南](../../docs/nuwax-harness/README.md)。

## 候选替换与本轮修复

[v0.18.13 的前置记录](nuwax-harness-release-0.18.13.md)和
[打包前机器收据](receipts/2026-10-22-harness-release-preflight.json)保留原始源码和计数。
其中 `in_progress` 是当时快照，当前状态由本文更正：
[Release 39779558934](https://github.com/nuwax-ai/codex/actions/runs/39779558934) 已取消，
`Publish to npm` 未执行（job 终态为 cancelled）。不能把前置 debug probe、已上传的单个平台工件或 tag 存在
写成 npm 已发布，也不能把 v0.18.13 指向新的修复树。

| 提交 | 范围与结果 |
| --- | --- |
| `b6080e115` | 用户级独立默认目录、发布主程序改名、npm 安装/配套文件、环境变量指南；Rust 内部目标名称保持原样 |
| `15283cc98` | 修复真实 TUI 确认回归：显式 `:danger-full-access` 选择 canonical `full-access` preset，避免同 profile ID 的 Ask 抢先匹配而绕过确认；保留 Ask 菜单语义 |
| `5191723db` | 修正 Responses mock 在自定义 matcher 拒绝请求前就记账的问题；cleanup 路由用例明确要求被测 MCP 客户端已经 ready，不让调度偶然性替代前置条件 |
| `f82730975` | 发布 R2：macOS 改用当前标准 runner，musl 编译并行降至 2，Linux 构建保留内存/磁盘监测；不改 Rust/V8/rig 版本 |
| `519d43824` | R2 之后的 test-only 跟进：跨平台 lifecycle hook shell 及 MCP 64 变体拆分；只改测试，原 `v0.18.14` 仍绑定 `f82730975` |
| `105fb7f9a` | test-only：board 场景精确等待本轮 child 完成及父队列处理栅栏，保留完整 final-mail 数量/通知断言；不修改发布 tag |

TUI 原断言保持不变，新增 `/permissions` 参数解析的 `--approval-policy=on-request` + `user|auto_review`
组合：确认前不能发送选择事件，用户确认后仍保留审批字段。最终权限模块 **49/49**。

共享 Responses recorder 的两个真实 HTTP 负控先失败：非匹配线程得到 404，recorder 却不为空；
修复后共享支持库 **53/53**。加入后述跨平台 hook shell 合同后，最终带 `rust-rig` 的共享支持库
**54/54**。这些是前后重叠选集，不相加。
cleanup 五变体和 child-completion 原场景定向 **6/6**，没有放宽原请求数量、超时或快照。

一次新增的 `subagent_browser_unused` 探针取得 **6/7、1 failed**：实际 attach/正常模型准备已
激活该 server，否定了“未使用”的测试前提。该未提交变体已撤，不将它误报为产品保证或
新增 Apps 专用负控；首败日志保留。已有 controlled cached-MCP 与 connection-status
不启动连接合同的 CI 证据单独保留。

## 旧完整 Core CI 的真实终态

下面只描述 [run 39774635523](https://github.com/nuwax-ai/codex/actions/runs/39774635523)
的源码 `8ed1cbfbd`，不能代验 `v0.18.14` 的七平台制品。
该手已结束：**一个 Bazel gate 成功，三条 nextest 均失败，整体不是全绿。**

| 平台 | Summary 原始口径 | 判读 |
| --- | --- | --- |
| Ubuntu | `23518 tests run: 23515 passed (16 slow), 3 failed, 49 skipped` | 三红逐项取证，见下 |
| Windows | `22296 tests run: 22286 passed (24 slow, 356 leaky), 9 failed, 1 timed out, 138 skipped` | 保留十项非通过，未统称环境失败 |
| macOS | `23452 tests run: 23448 passed (30 slow), 3 failed, 1 timed out, 72 skipped` | 四项非通过逐项分型，旧 CI 结果保留 |

Ubuntu 三项：

1. `hooks_executor::executor_browser_and_computer_use_cleanup_hooks_use_separate_mcp_routes::subagent_enabled`：
   原 fixture 未固定被测客户端 ready；`5191723db` 补齐前置，不改变 cleanup 产品行为。
2. `scenarios::child_completion_wakes_parent_after_final_answer`：
   实际首败为 `expected 1 request, got 2`，不是 snapshot 或 deadline。共享 recorder 抢在
   自定义 matcher 前记录别的线程候选请求；双 HTTP 负控复现后由 `5191723db` 修复。
3. `permissions::permissions_command_preserves_full_access_confirmation`：
   真实产品回归，由 `15283cc98` 修复，不能归入环境项。

Windows 十项非通过：

| 用例 | 本手判读/范围 |
| --- | --- |
| `thread_delete_recovers_only_corrupt_board_with_feature_disabled` | 保留首败，待定向复核；未定性为环境 |
| `remote_model_override_uses_catalog_model_for_strict_auto_review` | 等待 TurnComplete 的 15 秒 deadline，最后仅知事件类型 GuardianAssessment；最终 HTTP model/path 断言未执行，不能声称 Windows 字段正确或丢失；本手 Ubuntu PASS、当前 macOS 单条 PASS，均不抵销 Windows 原失败 |
| `mcp_code_mode_exclusion_does_not_change_direct_mode_tool_exposure` | `60.055s` 超时；将单条 64 组合循环拆为 64 条独立用例，保留既有 MCP 就绪握手和原断言/超时/skip；本机 64 变体全部通过，Windows Rust 原用例尚未重跑 |
| `anthropic_cap_terminal_fires_post_model_hook_once_without_resampling` | 共享 hook fixture 原来生成 POSIX 命令，Windows 默认由 `cmd.exe` 执行；日志未写。真实 cmd 首红后补 PowerShell 编码脚本，原 cap 断言及 10 秒等待保持不变 |
| `managed_worktree_transitions_bind_owner_and_preserve_only_fork_history` | 保留首败，待定向复核 |
| `permissions_command_preserves_full_access_confirmation` | 与 Ubuntu 同一真实回归；修复见 `15283cc98`，本地 49/49 不冒称 Windows 全量已验 |
| `legacy_capture_cancellation_terminates_descendants_without_timeout` | 历史 legacy 尾项同名再现，保留原失败，不扩写 Windows 后端 |
| `legacy_capture_emits_output_and_preserves_descendant_after_normal_exit` | 同上，原期限/断言保留 |
| `legacy_tty_job_terminates_and_preserves_descendants` | 同上，原期限/断言保留 |
| `legacy_workspace_write_delete_is_limited_to_writable_roots` | 用户已接受 B+警告的 legacy 删除约束限制，继续保持失败可见 |

Windows hook 夹具的真实 shell 取证：旧命令经 COMSPEC 退出 1 且日志不存在；新的
`powershell.exe -NoProfile -NonInteractive -EncodedCommand` 两次追加均通过，
空格、中文、单引号路径以及全部 JSON 字段、原 observer stdout JSON、UTF-8 无 BOM 都已核对。
macOS `/bin/sh` 两次追加同样通过；最终 Rust shell 合同已计入共享支持库 54/54。
这些是测试夹具补充，不能扩大为 Windows cap 原用例已经重跑通过。

后续本机独立 nextest list 确认 Matrix 64 变体 + 原 cap 1 条恰为 65 条；
`--retries 0 --test-threads 2` 的最终 Summary 为
`65 tests run: 65 passed, 2667 skipped`（95.890 秒），其中 2667 是本次选择器过滤。
全部原断言保留。它是 macOS 上对测试夹具的验证，Windows 原生部分只有前述 shell 2/2；
不写成 Windows Rust 65/65，也不把它并入已打 tag 的源码来改写版本绑定。

## macOS 四项非通过与后续定向边界

| 用例 | 原 CI / 后续取证 |
| --- | --- |
| `codex-cli::daemon_startup discovered_managed_daemon_can_restart_with_auto_start_disabled` | nextest 外层 `60.013s` TIMEOUT；内部阻塞位置未定，不因用例名猜机器条件，也未修改 deadline |
| `scenarios::agent_message_board::board_unsubscribe_survives_post_and_resume_until_resubscribed::channels_only` | `1.770s` FAIL，resubscribed 阶段 FINAL_ANSWER 计数实际 2、期望 3；原 fixture 把任意 mailbox activity 当作 child final 已排干的屏障，该前提过强 |
| `permissions::permissions_command_preserves_full_access_confirmation` | `0.293s` FAIL，与 Ubuntu/Windows 同一产品确认回归；由 `15283cc98` 修复，旧 CI 红保留 |
| `codex-utils-pty child_command::descriptor_tests::high_stdio_descriptors_use_compatible_fallback` | `0.028s` FAIL；子进程在构造高 fd 的前置阶段得到 EMFILE / os error 24，尚未进入兼容 fallback 断言；源与 main 一致，不能推断 hosted runner 的确切 fd 上限 |

board fixture 的 `wait_agent` 只等待任意 mailbox activity，排队中的 CHANNEL_POST 可以先唤醒它；
child 的 TurnComplete 又早于父 completion 通知提交，因而不能证明“最终邮件已进入父模型输入”。
`105fb7f9a` 仅使用现有测试/公开 API 加屏障：精确 child turn ID 的完成事件 → child idle →
父 submission 队列的空 settings ack，然后释放原 gate。原 **2 vs 3** 严格数量断言及完整通知
比较均保留，无 sleep、retry、skip、新生产 API 或 deadline 放宽。
修后两个变体单次 **2/2**（Summary 3.778s，retries0/test-threads1），不消除旧 CI 红。

PTY 原码未改，本机单条 **1/1**（case 0.038s、Summary 0.040s）通过；它只证明本地前置
可建立，不是 hosted CI 已验 fallback，也不能反推那台机器的确切 cap。无主机策略修改。

严格审查模型用例的阶段审计另有外盘 JSON：Windows 已越过 catalog override、权限 call ID
和 strict response 提交，在最终 TurnComplete 等待处失败；实际 guardian HTTP model/path
断言尚未执行。Ubuntu 同手 PASS 1.121s，当前 macOS 单条 PASS 4.295s；不拿后两者覆盖
Windows 首败，也没有可证实的模型字段丢失结论，不因此新开主线开发。

## 收据位置与剩余发布动作

- [前置打包与改名验收](receipts/2026-10-22-harness-release-preflight.json)：home 12、npm 合同 12、
  七目标归档合同 4、实际改名 ACP fixture 14；绑定该文件记载的 debug probe/source，不冒称 R2 release 制品。
- [旧 Core CI Ubuntu Summary](/Volumes/zhitai/tmp/codex-core-ci-39774635523/ubuntu-final-summary.json)、
  [Windows Summary](/Volumes/zhitai/tmp/codex-core-ci-39774635523/windows-final-summary.json)、
  [macOS Summary](/Volumes/zhitai/tmp/codex-core-ci-39774635523/macos-final-summary.json)：完整原日志同目录留存。
- [双 recorder 负控首红](/Volumes/zhitai/tmp/nuwax-harness-release-20261022/recorder-negative-both-before.log)、
  [修后共享库 53/53](/Volumes/zhitai/tmp/nuwax-harness-release-20261022/recorder-all-after.log)、
  [新增 shell 合同后 54/54](/Volumes/zhitai/tmp/nuwax-harness-release-20261022/hooks-recorder-rig-final.log)。
- [Matrix64 + cap1 的 65/65](/Volumes/zhitai/tmp/nuwax-harness-release-20261022/windows-fixture-selected-final.log)、
  [独立 list](/Volumes/zhitai/tmp/nuwax-harness-release-20261022/windows-fixture-list.json)：本机 macOS、无重试；
  原 Windows 首败保留，跨平台执行边界不扩大。
- [TUI 49/49](/Volumes/zhitai/tmp/nuwax-harness-release-20261022/tui-permissions-final.log)、
  [cleanup + child 6/6](/Volumes/zhitai/tmp/nuwax-harness-release-20261022/core-six-final.log)、
  [撤回探针首败 6/7](/Volumes/zhitai/tmp/nuwax-harness-release-20261022/core-cached-routes-final.log)。
- [Windows shell 对照](/Volumes/zhitai/tmp/nuwax-hooks-portability-20261022/new-windows-asserted.json)、
  [Unix shell 对照](/Volumes/zhitai/tmp/nuwax-hooks-portability-20261022/new-unix-asserted.json)：原失败与探针自身错误也保留在该外盘目录。
- [board 修后两变体](/Volumes/zhitai/tmp/nuwax-harness-release-20261022/mac-board-fenced-final.log)、
  [PTY 原码单条](/Volumes/zhitai/tmp/nuwax-harness-release-20261022/mac-pty-high-fd-focused-final.log)、
  [严格审查模型阶段审计](/Volumes/zhitai/tmp/nuwax-harness-release-20261022/strict-auto-review-focused-audit.json)、
  [本机严格审查单条](/Volumes/zhitai/tmp/nuwax-harness-release-20261022/strict-auto-review-focused-final.log)。
- [13 取消状态](/Volumes/zhitai/tmp/nuwax-harness-release-20261022/release13-final-status.json)、
  [14 早期状态快照](/Volumes/zhitai/tmp/nuwax-harness-release-20261022/release14-doc-status.json)：后者是早期七目标运行快照，当前五绿/两运行状态另以 Release 实际进度为准。

外盘链接用于本机溯源，不把大体积或敏感工件复制入库。版本 tag、实际构建源码和最终安装
二进制必须分别绑定；R2 之后的测试夹具验证不修改既有 tag。

本次发布剩余 macOS 两目标完成、总计七目标归档与 OSS 可下载校验、npm registry
版本/别名、隔离安装及 `nuwax-harness --version`/helper 短验收的终态回填。
当前 Linux/Windows 五目标已构建成功，npm 尚未发布；后续结果在本节续写。
模型接入主线不再扩展泛化平台维护：冻结 main、rig 与现有协议范围，Windows 已接受限制
继续明示；未定性失败保留，不以“环境”一词消账。
