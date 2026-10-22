# O2/O3 沙箱 deny 原语设计(2026-10-22,探针轮)

状态:**O2/O3 已实现并验收(2026-10-22 实现轮,台账 §6.56)**——显式构建 /
dev、netns sysctl 缺席分类已入库,`.45` 一手:239/239 合同、`blocked_descriptor_path`
PASS、strace 28 次 bwrap execve 零 `/dev/fd`。**O4(`/proc/net` 符号链接)与
`/proc/self/environ` 可读边界未收口**,宿主 pts bind 偏差有效(见文末)。
宿主:192.168.31.45(Ubuntu 26.04.1,内核 7.0.0-38-generic,bwrap 0.11.1,
`apparmor_restrict_unprivileged_userns=1` 但 bwrap 可用——与 §6.33 同一台)。
探针脚本:`/tmp/nuwax-o2-probe{2,3,4}.sh`(会话内);本文表格即其输出摘录。

## 问题回顾

- **O2(#42)**:策略 deny 指向 bwrap 自建的 `/dev` 符号链接项(`fd`、`core`、
  `pts`、`ptmx`、`console`、stdio 三链)时,`--tmpfs /dev/fd` 类 mask 因挂载目的
  穿过符号链接解析失败,bwrap 在命令启动前 abort(fail-closed 但功能失效);
  §6.35 修正后的现状是对这批条目"不 mask 即 abort",`blocked_descriptor_path` 腿红。
- **O3**:deny 指向新 netns 中不存在的 `/proc/sys/net/*` 条目时,after-proc 的
  mask 挂载同样找不到目的地而 abort。

## 探针矩阵(bwrap 0.11.1 实测,每行 rc 与沙箱内读数)

| # | 原语 | rc | 沙箱内结果 | 结论 |
| --- | --- | --- | --- | --- |
| P2 | `--tmpfs /dev/fd`(现状路径) | 1 | `Can't mount tmpfs on /newroot/dev/fd: ENOENT` | O2 复现 |
| P1/P4/P5 | mask `/proc/self{,/fd}` | 0 | 命令可运行但 **mask 全部落空**(setup 期 self ≠ 最终命令进程) | 此路不通 |
| P6 | `--tmpfs /proc/kcore` | 1 | `Can't mkdir … Not a directory` | tmpfs 不能盖 proc 文件 |
| Q1 | 手工 `/dev`(`--dir` + ro-bind null),无 fd | 0 | `/dev/fd`、`/dev/core` ENOENT,命令运行 | 缺席即拒绝 ✓;但 `--ro-bind` 设备节点 userns 内 EPERM |
| Q2 | 同上 + `--symlink /proc/self/fd /dev/fd` | 0 | fd 列表 0 1 2 3 正常 | 等价对照成立 ✓ |
| Q3 | `--devpts` 旗标 | 1 | `Unknown option` | 0.11.1 无此原语 |
| Q4 | `--dev /dev` 基线 netns 清单 | 0 | `/proc/sys/net` = bridge core ipv4 ipv6 mptcp netfilter unix;ip_forward=1 | netns 有自己的 sysctl 树 |
| Q5 | `--tmpfs …/ip_forward` | 1 | `Can't mkdir … Not a directory` | 同 P6 |
| R1 | `--dev-bind /dev/null /dev/null` | 0 | null 可读 rc=0 | **设备节点用 dev-bind** ✓ |
| R2 | `--perms 000 --ro-bind-data 0 …/ip_forward` | 0 | `cat` → **Permission denied** | **proc 文件 mask 原语** ✓(比缺席更强) |
| R3 | 全手工 dev(tmpfs /dev + dev-bind×4 + symlink×2 + shm + bind host pts) | 0 | null/fd 全部正常 | 完整等价可构建 ✓ |
| R4 | R3 去掉 fd/core 符号链接 | 0 | 仅 fd ENOENT,其余正常 | **O2 目标形态** ✓ |

## 设计

### O2:deny 命中 bwrap 自建 /dev 条目时,改为显式构建 /dev

- 触发条件:denied 集合与 §6.35 的 15 项清单(`fd`/`core`/`pts`/`ptmx`/
  `console`/stdio 链/`shm` 等 bwrap 自建项)相交时,放弃 `--dev /dev`,改发:
  `--tmpfs /dev` + 未被拒设备节点逐个 `--dev-bind`(null/zero/full/random/urandom/tty)
  + 未被拒符号链接逐个 `--symlink`(fd→/proc/self/fd、core→/proc/kcore、
  ptmx→pts/ptmx、stdio 三链)+ 未被拒 `--tmpfs /dev/shm` + `--bind /dev/pts /dev/pts`。
- 被拒条目**不构建**(缺席即拒绝,§6.33 已确立并入库的语义;R4 证明命令可运行
  且读取 ENOENT)。
- **已知偏差(须单列,不顺手放大)**:宿主 pts bind 替代了 bwrap SETUP_MOUNT_DEV
  的私有 devpts(0.11.1 无 `--devpts` 旗标)。语义差异=沙箱可见宿主 ptys;deny
  合同不受影响,但隔离面变化要在实现轮明确登记并评估(vendored bwrap 的
  SETUP_MOUNT_DEV 用 newinstance 挂 devpts,手工路径目前无法等价复刻)。
- 能力探测:构建前仍按 §6.35 清单静态判定,不做运行时探测(与
  `procfs_entry_survives_new_pid_namespace` 同一风格:静态名单 + 响亮前置断言)。

### O3:netns proc 条目按"新树必建/宿主特有"分类

- 新 netns 必有的顶层结构(Q4:bridge/core/ipv4/ipv6/mptcp/netfilter/unix)
  与其中的文件:**用 R2 的 `--perms 000 --ro-bind-data` 原语**(EACCES,强于缺席;
  R2 已证 netns proc 文件可盖)。
- 宿主特有条目(如 `ipv4/conf/<宿主网卡>/…`,新 netns 只回 lo):mask 挂载必
  失败,分类为**缺席即拒绝**——但分类依据必须是静态映射(netns 必建清单),不是
  运行时"挂不上就算了"(§6.50 教训:那会 fail-open)。落点:扩展
  `procfs_entry_survives_new_pid_namespace` 的名单语义到 netns 维度。

## 实现轮验收标准(沿交接边界)

- bwrap.rs 变更 + 构造合同单测(argv 形状,含响亮前置);
- `.45` 实机 E2E:deny /dev/fd 策略下命令运行 + `</dev/fd/18` 走失败分支
  (strace/argv 证据,同 §6.33 手法);deny 宿主特有 proc/sys/net 条目同构验收;
- 不删断言、不跳过失败转绿、read-only carveout/daemon socket mask 不搬迁;
- 单测 168+ 全绿只是构造合同,E2E 收据才是关闭条件。

## 开放问题

1. vendored bubblewrap 与 0.11.1 行为差异核对(SETUP_MOUNT_DEV 逐行,§6.33 已
   对齐过一次;`--symlink`/`--dev-bind`/`--ro-bind-data` 在 vendored 版本的等价
   实现须复核)。
2. `console`(stdio 为 tty 时)在手工路径的取舍:缺席是否破坏 tty 场景,还是
   需 `--symlink /dev/console`?实现轮用 pty 会话探针定。
3. O3 静态映射的维护成本:上游若扩充 proc 拒绝面,映射要同步(同 §6.50 的
   名单维护承诺)。
