# 事故复盘：自建 Gitea CI 踩过的坑

按「现象 → 根因 → 修法」记录。**每一条都真实发生过**，新机器初始化时对照检查可少走弯路。

---

## 1. 三个 Linux 目标同时失败，日志停在编译中且没有任何 error

**现象**：`x86_64-linux-gnu` / `x86_64-linux-musl` / `aarch64-linux-musl` 在同一分钟内
一起失败；日志末尾是普通的 `Compiling ...` 或 warning，没有失败标记。随后该机器
`ping` 正常、22 端口 open，但 SSH 报 `Connection timed out during banner exchange`。

**根因**：runner 的 `capacity: 3` 且 cargo 用默认并行度（= 核数 72）。
3 个 job × 72 rustc，内存 61/62 GB，swap 7/7 跑满，load 236，sshd 抢不到 CPU。

**修法**：
* `capacity` 调到 2；
* `CARGO_BUILD_JOBS` 按内存自适应（`mem_gb / 30`，夹在 1..3），别写死；
* 两台构建机内存差一倍时尤其要自适应，固定值一定会在某一台上出问题。

**代价**：一次手动重启（机器连不上就只能物理重启）。

---

## 2. `mkdir: cannot create directory '/data': Permission denied`

**现象**：job 在 checkout 之后、编译之前立刻失败，只有这一行。

**根因**：重启 runner 时**漏了 `CI_DATA_ROOT` 等变量**，工作流回落到默认的
`/data/nvme1/gitea-ci`，而该机布局是 `~/ci/...`。

**修法**：用 `gitea-ops/runners/start-host-runner.sh`（它强制导出五个 `CI_*`），
或改用 systemd 单元把 `Environment=` 写进去。

---

## 3. 在 job 运行中重启 runner，把 job 打成了「失败」

**现象**：`aarch64-linux-musl` 失败，日志停在编译中间、无 error；
同时 runner 日志里出现 `shutdown initiated, waiting 0s for running jobs`。

**根因**：改配置后直接 `pkill act_runner` 重启，正在跑的 job 被杀。

**修法**：改 `capacity` 前先确认机器空闲（`pgrep -c rustc` 为 0），
或用 `systemctl restart` 并在无 job 时执行。**排查任何"无 error 的失败"时，
先看失败时间点附近有没有 runner 重启记录。**

---

## 4. `unable to update registry crates-io / transfer too slow: 0 bytes in 30s`

**现象**：Linux 目标跑到 30–40 分钟后失败，报错在网络层。

**根因**：该 runner 的 `CARGO_HOME/config.toml` 没有指向 vendor 目录，
cargo 走了 USTC 网络镜像，镜像超时。

**修法**：

```bash
bash scripts/gitea-ci/vendor-deps.sh --install <vendor目录> --cargo-home <CARGO_HOME>
```

工作流已加 preflight：缺 vendor 配置会**立刻**失败并打印这条命令，
不再白跑 40 分钟。

---

## 5. Windows：`系统找不到指定的路径`

两个**互相独立**的原因，都在同一天遇到：

### 5.1 `.cmd` 被写成 LF 换行

Python 改写文件时把 84 行 CRLF 变成了 LF。cmd.exe 对 LF-only 的
`cd /d ... || exit /b 1` 解析异常，只回一句「路径找不到」。

修法：`.gitattributes` 锁 `*.cmd/*.bat/*.ps1 text eol=crlf`；改写脚本时注意
Python 默认文本模式会把 `\r\n` 读成 `\n`，要显式用二进制或 `newline=''`。

### 5.2 `shift` 连带移位 `%0`

```bat
set "TARGET=%~1"
shift                        rem ← 之后 %~dp0 变成第一个参数的路径
cd /d "%~dp0..\..\codex-rs"  rem ← 指向错误目录
```

`act_runner` 传进来的参数是 `x86_64-pc-windows-msvc`（不含分隔符），于是
`%~dp0` 解析成**当前目录**，`cd ..\..\codex-rs` 指向不存在的位置。

修法：把 `%~dp0` 先存进 `SCRIPT_DIR`，再 `shift`。

**排查技巧**：让脚本打印解析后的真实路径（`echo [win-build] cwd=%CD%`），
比对着 cmd 那句「路径找不到」猜要快得多。

---

## 6. `ld: final link failed: memory exhausted`（musl）

**现象**：`aarch64-unknown-linux-musl` 在链接阶段失败，但失败前后 4 分钟采样
都显示还有 43 GB 空闲——不是持续 OOM，是**链接瞬间的峰值**。

**根因**：musl-cross 带的 GNU ld 2.37 默认把每个输入文件的符号表都留在内存里，
链接这个体积的二进制时峰值极高。

**修法**：给 musl 目标加

```
-C link-arg=-Wl,--no-keep-memory -C link-arg=-Wl,--reduce-memory-overheads
```

**换 lld 走不通**（别浪费时间）：该 gcc driver 只在自身工具链内查找 `ld`，
`-fuse-ld=lld` 报 `cannot find 'ld'`；`-B/usr/bin` 会让它误用宿主 `as`
（`/usr/bin/as: unrecognized option '-EL'`）。

---

## 7. runner 在线、机器完全空闲，但 job 永远排队

**现象**：`.23` 一直空转（load 0.9、0 个 rustc），所有 Linux 构建都挤在另一台。

**根因**：`.23` 的 runner 注册标签是 **`ubuntu-latest`**（容器模式），
而工作流的 Linux 构建要求 **`linux-host`**。标签不匹配 → 永远接不到。

**修法**：在该机用 `--labels linux-host:host` 再注册一个宿主机模式 runner，
并用 `gitea-ops/runners/start-host-runner.sh` 启动、配 systemd 保活。

**排查**：

```bash
python3 -c "import json;print(json.load(open('<runner目录>/.runner'))['labels'])"
```

---

## 8. GitHub Actions 的 mac job 慢到 5 小时

**现象**：同一次发布、同一 commit，`aarch64-apple-darwin` 33 分钟，
`x86_64-apple-darwin` **5 小时 01 分**；两者编同样多的 crate、都是冷缓存。

**根因**：不是「x86 交叉慢」。日志显示 x86_64 那条在 `19:13 → 21:22`、
`21:30 → 23:46` 各有约 2 小时**完全静默**（没有 Compiling 输出），
且下一轮 `aarch64` 也跑了 4 小时——是 GitHub mac runner 的抖动/争抢，与架构无关。

**结论**：自建 mac mini（M4）上两个目标各 **22–23 分钟**，因此 darwin 目标
由自建 Gitea 承担，GitHub 只做备用。

---

## 9. 汇总：新机器上最容易犯的五个错

1. runner 标签写错（`linux-host` vs `ubuntu-latest`，`host` 后缀漏了）；
2. 启动 runner 时漏 `CI_*` 变量（非默认布局的机器必踩）；
3. `capacity` × cargo 并行度超过内存承受能力；
4. 在 job 运行中重启 runner；
5. Windows 脚本的换行符与 `shift`/`%~dp0`。
