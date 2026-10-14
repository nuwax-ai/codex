# shell_snapshot 修复批次 — 2026-10-14

本批单一机制：**Linux 沙箱内 bash 回放 shell snapshot 时自动重跑 `~/.bashrc`**。上一轮登记的 CI ubuntu 残差 `shell_snapshot×4` 中 3 个由此机制闭环，1 个（blocked_descriptor_path）确认为独立第二机制后登记保留。

## 机制（完整证据链，全部实机取证）

1. **上游 bash 行为**（GNU bash 5.2 `shell.c run_startup_files`）：非交互 `-c` shell 在
   `(SSH_CLIENT/SSH2_CLIENT 存在 || isnetconn(stdin)) && shell_level < 2` 时会执行 `~/.bashrc`
   （rshd/sshd 兼容路径）。`isnetconn`（`lib/sh/netconn.c`）= `getpeername(fd)`，
   **只有 ENOTSOCK/ENOTCONN/EINVAL/EBADF 判"非 socket"；EPERM 等其余错误一律判"是网络连接"**。
2. **fork 沙箱侧**：`codex-rs/linux-sandbox/src/landlock.rs:208-209` 网络受限 seccomp 无条件
   deny `SYS_getpeername`/`SYS_getsockname`（strace 实证：沙箱内 bash `getpeername(0) = -1 EPERM`，
   沙箱外同 stdin 为 `ENOTSOCK`）。外层（bwrap）与内层（`--apply-seccomp-then-exec`）阶段均复现；
   环境变量全链路干净（`env` dump 仅 HOME/PATH/PWD），与 ssh 变量无关 → **CI（无 ssh 变量）同机制**。
3. **后果**：回放 bash（原 `bash -pc '<restore><cmd>'`）在执行脚本前先自动 source `~/.bashrc`——
   - 捕获后 captures 计数翻倍（remote_pipe/tty_recovery 的 xx↔xxxx）；
   - 只读沙箱内首行追加写 EROFS（remote_sandbox）；
   - **rc 里的影子 `exec() { exit 41; }` 先于传输文件载体的 `\exec 18<&-` 关闭指令生效**
     （反斜杠只跳过别名，不跳过函数），回放以 exit 41 终止——transport 文件
     "先关载体再恢复函数"的设计前提被 bash 启动期 source 击穿；
   - concurrent 测试的 `BASH_SOURCE` 探测（函数定义必须来自 /dev/fd/*）失败。
4. **macOS 为何过**：无 seccomp（Seatbelt 路径），`getpeername` 对管道返回 ENOTSOCK → 不触发。
5. **附带回放**（本机 ssh 会话环境）：环境含 `SSH_CLIENT` 时同一 bashrc 分支也会触发
   （Debian `SSH_SOURCE_BASHRC`），与沙箱机制无关但症状相同——早期本机"复现"部分由此叠加。

## 修复（1 处产品代码 + 1 处单测适配）

- `codex-rs/exec-server/src/shell_snapshot.rs`：bash 回放 flags 从 `-pc` 改为
  **`["--norc", "-pc"]`**（zsh 已有等价物 `-f`，sh 不变）。`--norc`（no_rc）是
  isnetconn 分支的唯一闸门；`-p` 只压制 BASH_ENV、压不了该分支（bash 源码 1206 行：
  BASH_ENV 分支另查 `privileged_mode`），故 BASH_ENV 变体行为不变。
  argv 组装改为 truncate+extend（tail 重建）。Codex 复审发现初版遗漏了脚本后的
  `argv[3..]`，已补回原始位置参数，并增加实际执行回归。
- **`--norc` 必须打头**：这是 Bash 自身要求长选项位于单字符选项之前。
  macOS Bash 3.2 与 GNU Bash 5.2 的手册均明确此规则，直接执行
  `bash -p --norc -c …` 就会产生 `bash: --: invalid option`，无需经过 wrapper。
  原文将错误归因于包装链重组证据不足，Codex 复审撤销这项归因和残差登记。
- `shell_snapshot_tests.rs`：归一化断言从 `command[2]` 改为末元素（回放脚本位置随
  flags 数量移动），两份 prepared 仍同形可比。

## 验证（Linux 实机 swufe@192.168.32.163，Ubuntu 24.04 x86_64，rustc 1.99.0）

- 目标 4 项中 3 项转绿：`remote_sandbox`、`remote_pipe_recovery`、`remote_tty_recovery`
  （此前与 CI 逐签名一致地失败）。
- `snapshot_failure_retries_are_bounded_and_single_flight` 定义有 **7** 个变体；原文的8个计数错误已更正。
- 全 crate `-p codex-exec-server` 回归：见 §结果行（609/610，唯一残败 = blocked_descriptor_path）。
- 取证工具链留档：fixture 自报插桩（$0/$-/BASH_SOURCE/父 cmdline/env dump）、
  argv0 别名复刻 wrapper（`codex-linux-sandbox` 符号链接→exec_process 测试二进制）、
  strace getpeername 对照、GNU bash 5.2 源码（/tmp/bash-5.2）。Linux clone
  `~/nuwax-acceptance/codex-src` 已还原插桩，仅保留修复文件（工作区未提交）。

## 残差登记（下一批候选）

1. **blocked_descriptor_path（concurrent_replays，/dev/fd Deny 变体）——独立第二机制**：
   修复主机制后仍败，双形态：快败（0.3s，8 路并发中某路回放 stdout 为空，
   `shell_snapshot.rs:173` 断言）与慢败（25.3s，exec-server 进程无声死亡——stderr 零输出
   ——websocket 无握手重置，客户端 25s 恢复超时）。疑点指向 Deny /dev/fd 下 fd 传递/探针
   与 wrapper `--ro-bind-fd` 校验（`verify_fd_mounts` panic 路线）或 seccomp 信号致死，
   待下一批发掘。
2. 上一轮其余登记项不变（Windows idle 族×2、skills /tmp 污染 GH 前置条件等）。

Codex 独立复审、修复及新增平台验证见 `codex-shell-snapshot-review-2026-10-14.md`。
