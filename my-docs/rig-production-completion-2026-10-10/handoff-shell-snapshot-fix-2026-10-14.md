# Codex 独立复审交接书 — shell_snapshot 回放修复（2026-10-14）

复审对象：**单提交 `609068679`**（`fix(exec-server): stop sandboxed snapshot replay from re-sourcing ~/.bashrc`，本地 test 分支，未 push）。改动面：2 个产品/测试源文件 + 2 个文档，+84/−8 行。证据全文见 `results-shell-snapshot-2026-10-14.md`（同目录）。

## 改了什么

1. `codex-rs/exec-server/src/shell_snapshot.rs` `ShellSnapshotCache::prepare()` 尾部：bash 快照回放的启动旗标由单个 `"-pc"` 改为 `["--norc", "-pc"]`；argv 尾部组装从"改写下标 1、2"改为 `truncate(shell_start + 1)` 后 `extend(flags)` + `push(replay_script)`。zsh 由 `"-fc"` 拆为 `["-f", "-c"]`（语义不变，仅为统一 &[&str] 类型）；sh 不变。
2. `codex-rs/exec-server/src/shell_snapshot_tests.rs`：并发单测对两份 prepared command 的 fd 归一化，从固定下标 `command[2]` 改为末元素（回放脚本位置随旗标数量移动）。

## 主张的机制（可直接复核的事实链）

- 上游 bash 5.2 `shell.c run_startup_files()`：非交互 `-c` shell 在
  `(SSH_CLIENT/SSH2_CLIENT || isnetconn(stdin)) && shell_level < 2` 时执行 `~/.bashrc`；
  `lib/sh/netconn.c` 的 `isnetconn` = `getpeername(fd)`，**除 ENOTSOCK/ENOTCONN/EINVAL/EBADF
  外的任何失败（含 EPERM）都判"是网络连接"**。
- `codex-rs/linux-sandbox/src/landlock.rs:208-209`：NetworkSeccompMode::Restricted 无条件
  deny `SYS_getpeername`/`SYS_getsockname`（EPERM）。沙箱内 strace 实证
  `getpeername(0) = -1 EPERM`，沙箱外同 stdin 为 ENOTSOCK。
- 因此回放 bash 在执行恢复脚本**之前**自动 source `~/.bashrc`：captures 计数翻倍、
  只读家目录 EROFS、rc 定义的 `exec() { exit 41; }` 影子先于传输文件 `\exec 18<&-`
  载体关闭生效（`\` 只绕过别名，不绕过函数）、concurrent 测试的 BASH_SOURCE 探测失败。
  macOS 无 seccomp，故 macOS CI 一直绿；CI（无 ssh 变量）与本机同机制（env dump 全净）。

## 建议复核点（按风险排序）

1. **`--norc` 的语义边界**：确认它只置 no_rc、只闸 isnetconn 分支；不影响 BASH_ENV 分支
   （该分支由 `privileged_mode == 0` 闸，`-p` 保留故 BASH_ENV 行为不变）、不影响 login
   分支与 POSIX ENV。`local_bash_env`/`remote_bash_env` 变体已随全 crate 回归通过。
2. **argv 尾部重建的安全性**：入口已有 `ends_with(&params.argv)` 守卫且 argv.len()≥3；
   capture 路径（同文件 `capture_snapshot`）有自己的 `shell_start`，且在改写前执行。
   请确认无其他消费点假定回放尾部长度为 3（我已 grep，仅两处 shell_start）。
3. **`--norc` 必须打头**的包装链缺陷声明：`bash -p --norc -c …` 经 wrapper 后 bash 收到
   游离 `--`（实测 `bash: --: invalid option`）；`--norc` 打头则正常。根因未修（疑在
   launcher/argv0 回退对第一个 `--` 的切分重组），已登记。复审如认为是同一机制可修，
   请单独立项，不要并入本提交。
4. **测试适配的等价性**：归一化末元素后两份 prepared（各自持有不同 reader fd）仍同形可比；
   断言语义与原先一致。
5. **可移植性**：`--norc` 为 bash 长选项（bash 2.0+，macOS bash 3.2 亦支持）；该路径
   仅 bash/zsh/sh 三种 ShellType 触达，PowerShell/Cmd unreachable 不变。

## 已验证据（Linux 实机 swufe@192.168.32.163，Ubuntu 24.04 x86_64，rustc 1.99.0）

- 原 CI 失败 4 身份中 3 个转绿：`remote_sandbox`、`remote_pipe_recovery`、`remote_tty_recovery`。
- `snapshot_failure_retries_are_bounded_and_single_flight` 8 变体 + concurrent bash/bash_tty
  选择器共 11/11 过。
- 全 crate `-p codex-exec-server` 回归：**610 跑 / 609 过 / 1 败**，唯一残败 =
  `blocked_descriptor_path`（/dev/fd Deny 变体）——**独立第二机制**，双形态（0.3s 并发某路
  stdout 空；25.3s exec-server 无声死亡→websocket 重置→恢复超时，stderr 零输出疑信号致死），
  本批只登记不修。
- 已知未跑项：本机未跑 clippy（Linux 独立工具链缺 clippy 组件；Mac 侧无 target 缓存）。
  代码已按 workspace `redundant_closure_for_method_calls = deny` 规避
  （`iter().copied().map(String::from)`）。CI clippy lane 现不含 exec-server。
- macOS/Windows 平台回归依赖 CI（提交尚未 push，等你指令）。

## 明确不主张的

- 不主张修复 blocked_descriptor_path（独立机制，证据已登记待下批）。
- 不主张修复 wrapper 长选项脆性（登记项）。
- 不主张 seccomp 层改 errno（ENOSYS 同样不在 bash 的"非 socket"错误集；返回 ENOTSOK
  会污染真实 socket 查询——旗标层是正确层级）。
- 不扩称为"shell_snapshot 全族已稳"：concurrent 的 WorkspaceWrite/tty 变体在 Linux 实机过、
  macOS 由 CI 判。
