# Codex 独立复审：shell snapshot 启动修复

复审对象：`609068679`、`3137e712d`，基线 `7df7dc8f8`。用户授权发现问题直接修复、git commit并push阶段保存。此文记录独立复审与本轮验证，不把新证据反算成原提交已经完成。

## 发现与处理

1. **P2，真实位置参数回归，已修。** `exec-server/src/shell_snapshot.rs:300–305` 的尾部重建遗漏 `params.argv[3..]`，但入口允许argv.len()≥3。原来的 `$0`、`$1`、`$@`因此被改变。现在脚本后追加原始suffix；bash/zsh/sh都保留，zsh保持原`-fc`以减少无关改动。
2. **P2，Bash选项排序归因错误，已修。** `shell_snapshot.rs:290`、原results/交接书将`bash -p --norc -c`失败归因wrapper重组，但Bash自身就要求长选项在单字符选项之前。直接运行即可产生同一错误；本机Bash3.2手册和GNU Bash5.2 `doc/bash.1:159`及两阶段解析源码共同确认。保留`--norc`前置，撤销无独立证据的wrapper残差任务，没有改seccomp errno。
3. **P3，原验证变体计数错误，已修。** `shell_snapshot_tests.rs:28–34`只有7个retry变体，不是8个；原文已纠正。旧“11/11”摘要不能代替精确身份列表，本轮按实际nextest输出报告。
4. **P3，本轮新sh回归夹具可被普通login fallback满足，已增强。** 新测试最初用`.profile`，改为沿用已有sh测试的`.snapshot-env`+ENV。非交互普通启动不会读取该文件，结果必须来自snapshot捕获/恢复；不会把“绕开snapshot”当成PASS。

独立breaking/testing/change-size/context复审均参与；无额外未处理的新行为/API/context问题。无新模型可见片段、history改写、依赖/config/schema变更，不涉及新的P0上下文项。新fixture在运行期写TempDir，没有新增compile-time资源。

## 新回归与首败

`tests/exec_process/shell_snapshot.rs:20`新增真实进程集成回归：local与localhost exec-server分别运行bash/sh，macOS另含zsh。比较完整输出、stderr、status、closed和captures；涵盖带空格/引号的$0/$1、形似选项的$2以及参数总数，且captures必须恰1。

新增测试首次编译时修正了测试辅助API引用，并纠正了同时创建多个startup文件的夹具；这些没有改动生产行为。正式pre-fix验证六条全部FAIL，输出为`restored|/bin/<shell>|0||`，明确定位尾参数丢失。随后追加suffix，最终新增六条macOS/四条Linux均PASS。

local/remote在这里指同机直接backend/localhost exec-server，不冒称跨OS测试。

## 独立验证结果

| 平台及源码 | 实际全crate | shell_snapshot相关身份 | 失败/限制 |
|---|---|---|---|
| macOS arm64，1.95.0，修复候选 | 622运行，621 PASS，1 FAIL，11 skip | 51/51 PASS，含新6条及Bash3.2/zsh | native watcher并发超时 |
| Linux x86_64，1.99.0，独立checkout候选 | 614运行，613 PASS，1 FAIL，10 skip | 36 PASS、1 FAIL，含新4条 | 已登记blocked_descriptor_path |
| macOS arm64，原提交3137e712d源码快照 | 616运行，615 PASS，1 FAIL，11 skip | 无本轮新增6条 | 与候选相同native watcher并发失败 |

macOS失败是 `discover_v2::capability_manager::capability_watchers::tests::watch_events_invalidate_discovery_and_no_subscription_disables_caching`，错误为native watcher未收到global-root创建事件。原提交和候选在全crate并发均失败，两侧单跑均PASS，独立记录为基线已有并发/监听机制，不称全crate全绿。

Linux唯一失败是 `shell_snapshot::shell_snapshot_concurrent_replays_keep_independent_readers::blocked_descriptor_path`，本轮连续两次full均复现；Claude原610测试也登记同身份。remote_sandbox、remote_pipe_recovery、remote_tty_recovery实际PASS。本轮未修独立FD-Deny机制，交给main整合后的优先复现。

未运行完整workspace、付费模型或Windows原生测试。Windows回放实现受cfg(unix)限定；Windows CI结果仍待新SHA。skip不是PASS。

### 源码与工件边界

- macOS候选基于3137e712d加本轮源补丁，Linux独立路径 `/home/swufe/nuwax-acceptance/codex-review-20261014` 同样基于3137e712d；原Linuxcheckout及其未提交改动保留。
- 最终测试源patch SHA256：`4cd9daf6bba7013a007b1d073ce22769e502a9fa543260f2d77aee4faa02a15a`。Linux源文件hash核对：产品 `06b2015a6933f7f383a40dfaccc613b4799fe171be196fab31fd74ab7d36ec74`、新测试 `896cd0327b25d8b8ae87a08f339be5434b624c68a8b84b95c8ef14a7235e84e7`。
- 原提交基线通过git archive固定。共享target时曾出现一轮命令使用了基线的627个测试身份，没有新增6条；该轮不计候选验证。随后强制重建本crate，确认候选633身份（622执行+11skip）和新增6条实际运行。今后不同checkout的验证优先独立target目录，并核实际测试名，而不是只信命令cwd。
- 测试后执行scoped fix、just fmt和diff-check；不在最终fix/fmt后重跑测试。格式化差异单独核对，不冒称测试二进制已经包含最后文档/格式化提交。

`RUSTUP_TOOLCHAIN=1.95.0 just fix -p codex-exec-server --locked --offline`通过，无warning/error；`just fmt`和最终diff-check通过。格式化后产品文件与新增集成测试的hash仍与上述Linux核对值相同；旧retry单测仅换行，app-server既有live测试仅拆分tokio imports。这项范围外的机械import格式化单独style提交，没有执行付费测试。

日志及SHA256：

- pre-fix：`/private/tmp/codex-snapshot-review-before.log`，`12b07035cd596f6e7cfa1dbb7b09fa3f7ca41fb270a91662698d0f1b22d188e1`。
- macOS候选：`/private/tmp/codex-snapshot-review-macos-verified.log`，`f136751df7baa0cafd9806b46590231ac9c0afa9f1fd6bdbce4b4fbc148556c4`。
- Linux候选：`/private/tmp/codex-snapshot-review-linux-final.log`，`aa1e0dd3fc372a3aa6cec46963893688acbdc451557c50e3b57bc21645a8102e`。
- macOS原提交全crate：`/private/tmp/codex-snapshot-review-baseline-full.log`，`4626610a9e1b78820adb5583d6aa2a44b66fe1103516911cce68488f085f2928`。
- 两侧watcher单跑：`codex-snapshot-review-watcher-alone.log`、`codex-snapshot-review-watcher-baseline.log`；scoped lint：`codex-snapshot-review-clippy.log`，均位于`/private/tmp`。

## 后续任务

依次执行图片wire-budget最小修复、独立main合并、独立Rig迁移、图片入口及截图累积治理。详细任务见 `claude-main-rig-upgrade-followup-2026-10-14.md` 与 `claude-image-budget-followup-2026-10-14.md`。当前push授权用于本阶段；Claude下一轮先本地阶段提交、交复审，不自动发布或扩大付费测试。
