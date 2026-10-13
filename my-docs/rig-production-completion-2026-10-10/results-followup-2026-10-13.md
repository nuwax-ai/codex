# 结果跟进：CI 收敛与批次一（guardian 栈溢出 / Bazel gate / Linux 复现环境）

日期 2026-10-13，分支 `test`。起点远端 `0b5cdf8bf`（codex 独立复审 Oct13 修复已推）。本轮授权边界：批次一的新 SHA CI 归因、最小阻断修复、Linux 复现；不重复复审已修项；无发布/pin 迁移。

## 0. 起点

- 复审修复（`98d59ede8`…`e0dd75647`、`0b5cdf8bf` 文档）已核实生效范围：凭据字节扫描、identity 克隆限界、receipt 转义预算、Bazel rc 冒号路径、工具/usage 假阳性合同、typed live 重试默认。本轮不重做。
- `.bazelrc` 保持复审后的合法 `~/.cache` 默认 + gitignored `user.bazelrc` 显式覆盖；`%{VAR:-default}` 形式确认不会展开，不再使用。

## 1. 新 SHA CI（run `38358243799`，HEAD `0b5cdf8bf`，06:42Z–09:14Z）

| job | 结论 | 明细 |
|---|---|---|
| bazel bridge gate (ubuntu) | **success** | 见 §2 |
| nextest (ubuntu) | failure | 21810 跑/21802 过/**7 败 1 超时**，与 §8.3 残差集完全一致且无新增：discovery 超时（基线族）、guardian SIGABRT 0.558s（§3 本轮修复）、hooks::permission_request_hook（轮转单例）、shell_snapshot×4 + skills×1（§4）、上轮另一轮转单例 hooks_executor 本次通过（证实轮转性） |
| nextest (macos) | failure | 21767 跑/21759 过/**1 败 7 超时**：guardian SIGABRT 0.392s（同 §3）+ app_server_daemon 族 7 超时（基线族，较上轮 +4，族内轮转）|
| nextest (windows) | failure | 20637 跑/20589 过/**27 败 21 超时 198 leaky**；与 run 38286839054 身份级 diff：新增 5/消失 6 全落在已登记轮转单例族（resume/config_rpc/process_exec/tui/file_watcher）；`nuwax_env_idle_timeout_closes_a_stalled_stream` 34.7s FAIL 归入 Windows 执行环境族登记，不放宽、不用 Unix 代验 |

实际是 **Bazel success、三个 nextest failure**，整个 run 为 failure。表中记录的是失败身份的归类；落入既有族不自动证明其根因相同或本轮零回归，新出现的具体身份仍需保留首败和独立对照。

## 2. Bazel bootstrap JoinPathsError 关闭（批次一第 2 项）

run 38358243799 的 gate **success**（06:42–07:08，26 分钟）。冷执行证据：CI runner 无本地盘缓存（`common:ci --disk_cache=` 禁用、无远端端点），日志含 804 行 Compiling/Building/Analyzing、6,018 action 全量真实执行（rig-core 191 文件 22s、codex-rs/config 99 文件 21s 等逐条耗时），三 wire 矩阵（chat/anthropic/responses 端点、retry/idle/interrupt、并行隔离、stream-wait 网关、负控、provider 子键 fail-fast）全部通过。

结论：**新 Linux CI 因果关闭**。旧"rustc 1.95.0 ICE 工具链问题"定性为 rc 表达式制造含冒号 sysroot 路径（复审第 4 项机制），无需 pin 变更。macOS 无缓存冷 bootstrap 已由复审会话实编证明（1177 actions）。本项关闭。

## 3. Guardian SIGABRT（R11）栈压力观测与候选修复：`695b2a63b`

### 观测、候选机制及证据边界

- 溢出线程=测试自建的 **4MiB 专用线程**（上游 #17846/`2bfa62761` 加的包装），ubuntu+macOS 双平台同名线程 0.39–0.56s 崩。
- 上一轮报告本机 1.95.0/1.99.0、两 SHA 在原始4MiB下均通过；这本身不能证明编译器版本就是 CI/local 差异根因。workspace cargo metadata 的特性并集不能替代实际 test binary 的编译图；复审查看与旧 crash basename 相同的 `codex-core-ef8ac5bbb7ed29cb` fingerprint，仅显示 default+rust-rig。旧报告未逐次绑定 compiler/SHA/features/binary UUID，不能据此排除 feature 差异。
- 此前记录 CI `@stable` 浮动到1.99.0、本机默认工具链不同，这是构建条件而非已确认唯一原因。需要相同源码、实际 features、编译参数和原始4MiB预算的对照。
- 前轮缩栈记录：1/2/3MiB下崩溃、4MiB通过，确认受控低栈压力；它不是本机原始4MiB的 CI 同形故障复现，也不能单独给出动态峰值 ~3.1–3.9MiB。
- 前轮反汇编静态帧记录：submission_loop匹配块1036KB、run_turn1184KB、turn_input::handle304+392KB、stream476KB及其它包装帧。数字保持为该次构建的观测，不作为实际动态峰值精确值。互斥 await 分支的 future 布局一般按最大变体占位，不能把“全部分支状态机相加”与 future 构造/移动临时量、poll栈帧或优化器函数内联混同。[上游#13429](https://github.com/openai/codex/pull/13429) 支持在包装链用 heap pointer 替代 inline child future 的修法，但未证明本例 CI 触发条件完全相同。

### 修复（`695b2a63b`，5 文件 +209/−160）

实际 diff 是 **55 个新增 Box::pin 调用点**：handlers28、turn_input8、turn13、regular2、client4；5 文件 +209/-160，共369 changed lines。控制流、参数、await/错误传播、instrument/or_cancel顺序不变；具体 Pin<Box<F>> 不擦除类型，但执行到这些边界会新增 heap 分配/释放，未做性能成本量化。测试文件零改动，常量保持上游4MiB。

### 验证

- 前轮分别记录修复前3MiB失败、修复后3.25MiB通过/2.875MiB失败；两组预算不同、阈值区间重叠，不能据此声称已量化 before/after 栈余量改善。若需要量化，补同一binary条件/相同预算对照并保留收据。前轮静态帧观测保留：submission dispatch1036KB在该调用链消失、handle392→172KB、run_turn本体仍约1MB、stream仍约476KB；不将这些静态数字当成动态峰值。
- 4MiB 合同在 **1.99.0 与 1.95.0 双工具链通过**；guardian 全族、resume/compact/token_usage/turn_input_submission 计 490+ 通过；`sampling_is_ready_for_daemon_recovery` 模块并发失败经基线对照（stash 本修复后同样失败）确认为**预先存在的模块并发脆弱性**，与本修复无关。
- clippy 无告警、`cargo fmt --check` 干净、`git diff --check` 干净；codex-exec 重建后 live-tests 离线套件 **54/54**。
- CI 判决待下次授权 push（本提交不在 run 38358243799 内）。

### Codex 独立复审补证

完整核对55个装箱点未发现业务语义回归；原始4MiB合同在显式 `rust-genai,rust-rig` 下以1.95.0/1.99.0各实跑通过，实际test binary fingerprint/SHA/UUID已留存。现有取消/turn mode/三wire cap/WS fallback/resume/compact/token_usage选集168条完成验证，其中两条CodeMode用例首跑因缺helper失败，原pin双层校验构建helper后2/2通过。未改任何测试栈预算，未将这些本机通过当成新CI判决。详细命令及证据边界见 [codex-box-review-2026-10-13.md](codex-box-review-2026-10-13.md)。

## 4. Linux shell_snapshot / skills（批次一第 4 项）：机制钉死一项，其余留下一批

容器环境：OrbStack Debian 13.7 aarch64（`--security-opt seccomp=unconfined`），源码 tar 拷贝至 `codex-linux-src` 卷（含 .git，tree 干净），rustc 1.95.0+1.99.0、cargo-nextest 0.9.146、registry cache+index、git 依赖缓存齐备。定位为**校准环境**：bwrap 在其中因 `Can't mount proc on /proc: Operation not permitted` 无法运行（无 CAP_SYS_ADMIN），不代验沙箱路径。

### 4.1 skills `repo_ancestry_without_project_marker_does_not_walk_parents`——机制完整钉死

- 容器复现：与 CI 完全同形（实际值多出 `outer/.agents/skills`）。
- 代码路径：`find_project_root` 沿祖先探测默认 marker `.git` → 命中祖先级 `.git` → 把祖先当项目根 → `dirs_between` 纳入父级目录。
- **污染源实证**：容器 `/tmp/.git`（空目录）存在；inotify 捕获其 CREATE 于 exec-server 测试窗口内；strace 捕获沙箱子进程 `mkdirat(fd,".git")`（与 newroot/oldroot/proc/dev/shm 同一脚手架进程）；/tmp 残留同族 `.agents`/`.codex`。沙箱受保护元数据名合成脚手架泄漏到系统临时目录顶层，`TempDir` 在 /tmp 下的测试沿祖先探测即中招。bwrap 失败时脚手架残留（容器）；GH runner 上 bwrap 成功但脚手架生命周期存在可见窗口，且失败自 userns 解锁后持续（解锁前沙箱路径无法真正运行）。
- 修复方向已明确（fixture 隔离或脚手架泄漏源头），**留待下一批**：本轮未定位到创建该顶层目录的确切源码行（合成目标注册表本身路径正确命名空间化；泄漏点在沙箱脚手架创建路径内，需特权校准容器继续追）。

### 4.2 shell_snapshot×4（remote_pipe/tty_recovery、remote_sandbox、blocked_descriptor_path）

CI 失败形态各异且非容器同形：pipe 恢复输出截断（`xx` vs `xxxx`）、remote_sandbox 的 .bashrc 探测写只读文件系统（退出码 41）、blocked_descriptor_path 连接重置后 25s 恢复超时。容器内同族 `remote_sandboxed_*` 因 bwrap proc 挂载限制集体早退（0.077s 环境失败）——**容器不能代验 CI 机制**，需 `--privileged` 校准容器或 Linux VM 后按交接（核对 env/cwd/用户、引号、shell 版本、git 标记、fixture 隔离）逐身份排查。已登记为下一批首要项。

## 5. 环境与资产

- 宿主 `rustup` 新增 1.99.0 工具chain（对齐 CI @stable）；容器同装。
- 宿主 `/Volumes/soddy` 一度 100% 满：清理**可再生的** `codex-rs/target/debug/incremental`（126G）恢复 109Gi；用户管理的 bazel 缓存未动。
- 容器卷：`codex-linux-src`（0b5cdf8bf 干净树）、`codex-linux-target`（1.95.0 旧缓存 15G）、`/target-199`（1.99.0 构建缓存）。
- 本轮提交均本地，push 待用户决定（推送触发 ~2.5h CI，将携带 `695b2a63b` 的 guardian 判决）。

## 6. 残差账目（run 38358243799 后）

| 项 | 状态 |
|---|---|
| Bazel gate（冒号 sysroot） | **已关闭**（§2 冷执行证据） |
| guardian SIGABRT（R11，三平台） | **修复已提交** `695b2a63b`，本机双工具链+阈值证据齐，CI 判决待 push |
| skills repo_ancestry | 机制钉死（/tmp/.git 污染链），修复留下一批 |
| shell_snapshot×4（Linux） | 需特权校准容器，下一批首要 |
| discovery/daemon 超时族 | 基线族维持登记 |
| Windows（nuwax_positive_wires×2 挂起、nuwax_env_idle_timeout 34s FAIL、执行环境族） | 平台独立任务维持登记，需原生证据 |
| 轮转单例（hooks/resume/config_rpc/process_exec/tui/file_watcher） | 轮转族维持登记，未取日志不统称 flaky |
