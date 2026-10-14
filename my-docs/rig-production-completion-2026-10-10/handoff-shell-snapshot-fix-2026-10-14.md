# Codex 独立复审交接书 — shell_snapshot 回放修复（2026-10-14）

复审对象：**单提交 `609068679`**（`fix(exec-server): stop sandboxed snapshot replay from re-sourcing ~/.bashrc`，本地 test 分支，未 push）。改动面：2 个产品/测试源文件 + 2 个文档，+84/−8 行。证据全文见 `results-shell-snapshot-2026-10-14.md`（同目录）。

## 改了什么

1. 原提交 `codex-rs/exec-server/src/shell_snapshot.rs` 的 bash 快照回放旗标由 `"-pc"` 改为 `["--norc", "-pc"]`，尾部组装改为 truncate 后重建。Codex 复审已补回遗漏的 `params.argv[3..]`，保持脚本位置参数；zsh 保留原 `"-fc"`，sh 不变。
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
2. **argv 尾部重建的安全性**：入口 `ends_with(&params.argv)` 与 argv.len()≥3 只保证
   shell_start 可定位，不保证没有脚本后的参数。初版遗漏这些参数的回归已由 Codex 修复。
   capture 路径保留原参数，新增 local/localhost-remote 真执行回归验证 $0/$1/$2/$#。
3. **`--norc` 必须打头**：Bash 自身要求长选项先于短选项；直接执行相反顺序即可报错。
   包装链缺陷的原归因没有独立证据，已撤销，不再作为下一批任务。
4. **测试适配的等价性**：现有三参数单测的末元素仍是脚本；一般带位置参数的命令
   不满足这一点。新增回归实际执行并比较整个结果，不依赖脚本是最后一个argv。
5. **可移植性**：`--norc` 为 bash 长选项（bash 2.0+，macOS bash 3.2 亦支持）；该路径
   仅 bash/zsh/sh 三种 ShellType 触达，PowerShell/Cmd unreachable 不变。

## 已验证据（Linux 实机 swufe@192.168.32.163，Ubuntu 24.04 x86_64，rustc 1.99.0）

- 原 CI 失败 4 身份中 3 个转绿：`remote_sandbox`、`remote_pipe_recovery`、`remote_tty_recovery`。
- `snapshot_failure_retries_are_bounded_and_single_flight` 源码定义有 **7** 个变体。
  原摘要记录选择器11/11，但将单测写成8个错误；平台独立实际身份/数量见 Codex 复审记录。
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
- 不主张存在已证实的 wrapper 长选项缺陷；原登记由 Codex 复审撤销。
- 不主张 seccomp 层改 errno（ENOSYS 同样不在 bash 的"非 socket"错误集；返回 ENOTSOCK
  会污染真实 socket 查询——旗标层是正确层级）。

后续 Codex 复审修复及验证见 `codex-shell-snapshot-review-2026-10-14.md`；本交接对象仍指原始提交，不把复审结果反算为原提交已完成。
- 不扩称为"shell_snapshot 全族已稳"：concurrent 的 WorkspaceWrite/tty 变体在 Linux 实机过、
  macOS 由 CI 判。
