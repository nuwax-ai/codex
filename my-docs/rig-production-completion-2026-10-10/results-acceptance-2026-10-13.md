# 结果：实机验收轮（2026-10-13 晚，收敛到可用）

接续 [results-followup-2026-10-13.md](results-followup-2026-10-13.md)。起点 HEAD `bf591fcf9`（guardian 装箱修复 `695b2a63b` + 复审修正，test 领先 origin 3 提交）。本轮授权：实机验收（Linux/Windows 两台测试机）、离线定向、验收链路、小批真实模型（总上限 24 POST、每场景 ≤6、默认 MiMo flash+Chat）、本地提交不 push。

本轮新增 5 个本地提交（均未 push，CI 待验）：

| 提交 | 内容 |
|---|---|
| `c1236f99c` | **feat(bridge)**: 发送前硬请求预算 `CODEX_RIG_MAX_HTTP_ATTEMPTS`（账本权威、跨进程）+ harness 接线 + 离线验证 2/2 |
| `8b3b2023e` | **test(app-server)**: env 模式压缩+召回验收（离线链路最后缺口） |
| `b5111c9fd` | **fix(chatgpt)**: join! 分支装箱，解 MSVC 布局查询深度溢出（Windows 原生机编译阻断） |
| `8f086f599` | **test(app-server)**: LIVE 门控验收三场景 + wires 审批钉死（Windows 挂起机制修复） |
| （文档） | 本文件 + tasks 标注 |

## 0. 环境与构建收据

| 机器 | 角色 | 环境要点 | 源/构建 |
|---|---|---|---|
| macOS（本机，Darwin 27 arm64） | 开发+离线+live 直连 | rustc 1.95.0（repo pin）/1.99.0 双工具链 | HEAD `8f086f599` 干净树；codex-exec/codex-app-server/codex CLI debug 构建（收据内嵌 git SHA+tree 快照，期间两次正确拒绝编辑前二进制） |
| Windows 11 26200（soddygo@192.168.32.235，原生，非 WSL） | 原生 Windows 验收 | rustc 1.98.1/1.99.0-msvc、MSVC2022、cargo-nextest 0.9.146、V8 用 setup-rusty-v8 同源三重校验（openai/codex release + third_party/v8 清单） | bundle 同步至 `8f086f599`，独立目录 `~/nuwax-acceptance/`，CI 同构命令（`--workspace --exclude codex-voice-host --features codex-core/rust-rig,codex-v8-poc/sandbox`） |
| OrbStack Debian 13 aarch64（`codex-linux`，**--privileged**） | Linux 校准 | bwrap 可用（本轮升级特权后 `--ro-bind / / --proc /proc` OK）、userns OK、双工具链、nextest、strace/inotify | `/src` 同步 `8f086f599`（注意：为取证曾临时插桩，见 §6） |
| Linux 实机 swufe@192.168.32.163 | — | **阻断**：publickey/password 均拒，本机密钥未获授权；.env.local 无 SSH 口令。需要用户提供访问凭据后补原生数据 | 未测（如实登记） |

磁盘处置记录：本机 `/Volumes/soddy` 一度 100%，删除**可再生的** `codex-rs/target`（102G，含全部历史构建产物）；容器删除本轮自建的 1.95 旧 target 卷。用户 bazel 缓存与 Docker 构建缓存未动。

## 1. 硬请求预算闸门（`c1236f99c`）

- 机制：`CODEX_RIG_MAX_HTTP_ATTEMPTS`（0–64，含 0 负控）在 `request_capture::record` 于 `client.send_streaming` **之前**判定——采样/续接/重试全部经此单点，超限即拒、**拒绝的请求从未上线**；跨进程权威=捕获账本（每次 append 重读计数），无捕获文件的预算直接启动失败（不可验证即不允许）。预算错误以 `ApiError::InvalidRequest` 非重试终局呈现。
- harness：`run_nuwax_env_marker_turn(..., http_attempt_budget)` 传入子进程环境（继承值先剥离）；exec_live 三条 env 启动场景钉死每场景 6。
- **离线验证 2/2**（`nuwax_env_budget_offline.rs`，真实 codex-exec+脚本化 loopback）：预算恰够=完整工具往返+账本恰 2；预算少 1=turn 以预算错误终局、账本恰 1、**监听全程开放且服务端连接数停在 1**（连接数是权威，日志行数不作为预算证据）、无 turn.completed 伪装。
- 桥 crate request_capture 18/18（含 4 个新预算单测：越界拒改、跨 recorder 实例、零预算负控、解析边界）。

## 2. 离线验收链路（第三项）

七身份全绿（codex-app-server `nuwax_positive_wires`+live-skip 路径 7/7）：

| 链路环节 | 覆盖（mock wire，Chat/Responses） |
|---|---|
| 启动→turn→无害工具 | chat/responses 工具往返（真实子进程、call ID、工具输出进第二请求、答案含 marker、tools 广告） |
| 取消 | 中断后唯一多余 wire 请求=被中断尝试（零重试预算下断言） |
| 热恢复 / 冷恢复 | 同子进程 resume + 新子进程 cold resume（`8f086f599` 前） |
| **压缩**（本轮补齐） | `env_group_compact_and_recall_keep_the_typed_wire_and_usage_accounting`：learn→thread/compact（同一 typed wire 第 2 交换）→召回含暗号；压缩是 turn，召回须等其完成通知（ActiveTurnNotSteerable 竞态已修）；凭据不落 rollout/config |
| usage | responses 路径 tokenUsage 通知 + chat 路径 usage 帧 |
| 双进程隔离 | 既有 nuwax_isolation/并行进程族（Windows 原生机亦过，§4） |

## 3. ACP 消费链路（15/15，macOS）

驱动：非交互 ACP 客户端（/tmp/acp-acceptance-20261013/driver.mjs，ACP 仓库只读未动）→ `fork-codex-acp-ts` dev `4e1f33b`（v1.2.18，npm 以 `--no-save --package-lock=false` 隔离缓存构建，锁文件不同步问题绕行且未改仓库）→ `CODEX_PATH` 指向本轮新构建 codex CLI → `codex app-server` 继承 `NUWAX_*`（CODEX_HOME 内 config 仅 approval never/danger-full-access，无 provider 表）。

| 阶段 | 断言（全过） |
|---|---|
| initialize/session/new | agentInfo=nuwax ACP 1.2.18；`_auth/status_update` 显示 gateway=Custom model gateway/nuwax_env |
| 工具往返 | 真实 exec_command 执行 marker（tool_call→tool_call_update 含输出）、答案含 marker、turn 采样恰 2 |
| 真实取消 | 慢滴流中 `session/cancel`（通知语义）→ stopReason=cancelled；**服务端连接数不再增长**（取消确认后无新采样） |
| 进程重启恢复 | agent SIGKILL→重启→initialize→session/load 冷恢复→召回含 marker；账本 ≤ 预算 12；turn 采样总数恰 4 |
| 非泄漏 | HOME/工作目录全树扫描 API key 零命中 |

注意：app-server 会话标题生成是独立后台采样（不属 turn 计数），驱动按提示词路由区分——后续 live 断言已按此修正。

## 4. Windows 原生（26 跑/22 过 → 修复后复验）

- **`b5111c9fd` 编译阻断修复**：codex-chatgpt `list_connectors` 的 `tokio::join!` 双分支内联致 MSVC 布局查询深度溢出——1.98.1/1.99.0 双工具链、装箱基线 `0b5cdf8bf` 均复现（非本轮回归；GH windows-latest 可编译，物理机不行），按 #13429 模式装箱两分支后 15.7s 过。CI 若在 runner 上遇同题此修复同样适用。
- 首轮 CI 同构批次（V8 三重校验后）：**26 跑/22 过**。过：**guardian 4MiB 合同（1.99.0-windows 原生，装箱修复第 4 平台证据）**、wires interrupt+双恢复（CI 上超时的身份在原生实机通过）、cross_process_resume 一员、exec nuwax 全环境族（三 wire/重试/并行隔离/fail-fast）。
- **wires 工具往返 2×TIMEOUT 机制钉死（原生 JSON-RPC 全文）**：工具调用真实到达（powershell 包装 echo marker），但默认 `on-request` 审批下 powershell 包装不被自动放行→app-server 发 `item/commandExecution/requestApproval`→测试客户端永不回应→挂起。unix 裸 echo 自动放行故 CI ubuntu/mac 过。修复：`8f086f599` 将 fork 自有 wires/live 套件审批钉死 never（测试主题是 wire 非审批）；复验见 §7。
- `nuwax_env_idle_timeout_closes_a_stalled_stream` FAIL 26.4s、`nuwax_env_parallel_processes_enforce_distinct_idle_budgets` FAIL 26.3s——CI 已登记 Windows 执行环境族的原生同形复现，事件/进程证据在 `win-final-run.log`，独立机制留下一批（不放宽 timeout）。

## 5. 真实模型批次（macOS 直连 HTTPS，恰好用满 24/24）

密钥仅经进程环境（`.env.local` sourced，不进命令行/工件/报告）。全部场景经预算闸门+捕获账本（真实 URL/model/Bearer 逐请求、密钥零出现在账本）。

| 场景 | 结果 | 请求数 |
|---|---|---|
| app-server 工具往返+usage+真实中断+热/冷恢复（跨进程共享账本+预算 6） | **PASS 25.0s** | 5 |
| app-server 压缩+召回（预算 6） | **PASS 28.9s**（首败×2 为测试时序/模型拒答"暗号"，均登记后修正重跑） | 3+2+2（含修正前） |
| app-server 真实触顶（cap 64，预算 2） | **PASS 12.0s**，恰 1 次请求无重采样，cap 终局不伪装 Completed | 1 |
| exec typed-env 工具往返：mimo/glm/step 三厂商 | **PASS 19.1/16.9/11.0s** | 各 2 |

合计 24，达授权上限即停（后续 live 一律不再发）。首败保留：①compact 首跑 `ActiveTurnNotSteerable`（压缩 turn 未完成即召回——测试竞态，已修）；②召回提示词含"暗号"被 MiMo 拒答"I can't reveal secrets"（模型行为，同 Oct-12 空思考类别，改中性措辞后过）；③工具执行断言初版查 turn.completed 的 items summary 视图（不含 commandExecution），改为断言 item/completed 通知。

## 6. Linux 容器（特权升级后）

- **guardian 4MiB：1.99.0-aarch64 Linux 过**（装箱修复平台证据集：macOS 1.95/1.99 + Linux 1.99 + Windows 1.99 原生）。
- **shell_snapshot×4 全部复现**（与 CI 同形：remote_sandbox 0.2s、remote_pipe/tty_recovery 1.5s xx↔xxxx、blocked_descriptor_path 25.2s 超时）。取证进展：捕获成功（file_source=true、状态 1666B）、回放 argv 正确（`bash -pc` + `. /dev/fd/18` 恢复、无 login 标志），但沙箱内执行实际输出=空 stdout + `.bashrc: line 1 EROFS` stderr + **exit 41**（夹具 exec/unset 陷阱）——即沙箱回放仍以某种途径重跑了只读 .bashrc。最后一发插桩（BASH_ENV/完整包装命令）已排队，确切重 source 入口待其落定；修复留下一批（机制登记：与 /dev/fd 探测、注册表脚手架生命周期相关的 bwrap 回放路径）。容器 `/src` 为取证曾临时插桩（DBG-PREPARE 系列 eprintln），非仓库状态。
- **skills /tmp 污染对照**：特权容器干净 /tmp 下 `repo_ancestry_without_project_marker_does_not_walk_parents` **通过**；bwrap 成功运行的本轮全量测试后 /tmp 未再出现 `.git/.agents/.codex` 顶层泄漏（仅注册表 `codex-bwrap-synthetic-mount-targets-0` 与正常临时目录）。旧证据（非特权、bwrap 失败路径）中的 /tmp/.git CREATE 与 scaffold mkdirat 并存——**泄漏与 bwrap 失败/回退路径强相关**的假设增强；GH runner 上该测试失败是否同源（userns 模式差异）需以非 root 用户复刻（容器 testuser 已备，PATH 权限修复后未及重跑）。
- bwrap/userns//proc/工具执行：容器内 `remote_sandboxed_*` 文件系统与进程族 4/4 过、`unshare -Ur` OK、bwrap 挂载 OK；GH 同构非 root 复刻未完成（登记）。

## 7. 残差账目

| 项 | 状态 |
|---|---|
| guardian R11 CI 判决 | 本机 4 平台证据齐；**待 push 后 CI**（run 38358243799 不含修复） |
| Windows idle 族×2 | 原生同形复现+证据留存，机制未定，下一批 |
| shell_snapshot×4（Linux） | 全复现+半程取证（argv/捕获/恢复路径已排除），重 source 入口待最后一发插桩；修复下一批 |
| skills /tmp/.git | 特权+干净 /tmp 通过；泄漏与 bwrap 失败路径相关；GH 非同源未排除，非 root 复刻待续 |
| Linux 实机 | **SSH 凭据阻断**，用户提供后补 |
| 轮转单例/discovery/daemon 族 | 维持登记 |
| codex-chatgpt MSVC 布局 | 已修（`b5111c9fd`）；若 CI runner 未来触同题同修适用 |

## 8. 四问回答（本轮边界内）

1. **哪个精确版本、平台、模型和协议已经可以使用？** `8f086f599`（=bf591fcf9+5 本地提交，未 push）：macOS arm64 上经 NUWAX_* 环境变量 + Rig 的 **Chat 协议**（MiMo mimo-v2.6-flash 已实机全链验证：工具/取消/热冷恢复/压缩/usage/触顶；GLM-5.3-Flash、step-5-preview 的 Chat typed-env 工具往返亦过）+ Anthropic/Responses 协议（mock 全链 + 历史真实收据）；ACP dev 消费链路（CODEX_PATH→新 codex CLI）15/15。Windows 原生：离线三协议 mock 链路 22/26 过（wires 工具往返待修复后复验），live 未做。Linux：容器校准环境可用；原生实机阻断于 SSH。
2. **已修复缺陷与提交？** MSVC 布局溢出 `b5111c9fd`；wires 审批挂起（机制+测试修复）`8f086f599`；预算闸门缺失 `c1236f99c`（本轮授权的前置能力）；压缩验收缺口 `8b3b2023e`+`8f086f599`（时序竞态）。
3. **真正阻断使用的缺陷？** Linux 原生实机访问（凭据）；Windows idle 族×2（影响长流场景稳定性，机制未定）；shell_snapshot×4（Linux 沙箱快照回放，影响 exec-server 沙箱路径非 NUWAX 主链）；无NUWAX 主链阻断项。
4. **最短剩余任务？** ①用户解 Linux SSH→原生 guardian/nuwax/沙箱三件套复验；②push 6+4 提交取 CI 判决（guardian+wires windows+chatgpt）；③shell_snapshot 重 source 入口收口（插桩已排队）；④Windows idle 族机制取证。
