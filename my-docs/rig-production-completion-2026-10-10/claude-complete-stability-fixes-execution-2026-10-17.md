# Claude 全量稳定修复执行记录（2026-10-17 轮）

执行者 Claude Code；范围 = [十二项收口提示词](claude-complete-stability-fixes-2026-10-16.md)，
输入 = [Codex 复审](codex-closure-review-2026-10-16.md)（5 产品缺陷 + 4 验收缺口 + 最终验证/真实 API/分诊）。
起点：Codex 候选分支 `codex/rig-044-20261014` `535882fef`（本轮推进至 `e235002d0`），
ACP dev `be81fe1`（推进至 `63a58d6`）。本轮不 push、不手动 dispatch、不发布。
产品代码止于 `73c1828b5`（其后两笔均为纯测试提交），平台二进制即按该树构建；
`e235002d0` 只改测试夹具。

## 0. 逐项判决（fixed / executed / blocked）

| # | 任务 | 判决 | 摘要 |
|---|---|---|---|
| 1 | 正常退出有界等待记忆 claim 清理 | **fixed** | `0ab3ad112`：管道任务 shutdown-bound 跟踪 + agent 处理内联；app-server 关闭序列在线程全停后有界等待（10s，lease 兜底不变）；3 个真实子进程合同测试（1s 写锁竞争下 pending、错误预算保留、长锁下有界退出留 lease） |
| 2 | 真实错误不被 Shutdown 覆盖 | **fixed** | 同提交：`loop_agent` 改 watch 订阅 + `Errored` 粘性（后到 `Shutdown` 不可改判，后到 `Completed` 仍可翻案）；子进程合同断言 `error/last_error=failed_agent/retry_remaining=2/+3600s` |
| 3 | 已有 handler 的 reload 丢恢复 usage | **fixed** | ACP `056fc0c`：按 load 显式 arm 的 per-load capture 取代全局无 handler 缓冲；armed 期间通知归 load 所有（抑制旧 handler 写旧状态） |
| 4 | 恢复 usage 缓存全局无界 | **fixed** | 同上：仅显式 arm 产生条目（TitleGenerator 线程永不入表）；cap 64 只驱逐已结算项；load 失败/线程关闭/dispose 清理；100 次创建关闭后 0 条目 |
| 5 | 恢复 usage 发送的异步拒绝 | **fixed** | ACP `c3d2278`：best-effort + catch + 日志（含会话 id），load 响应与内存态保持一致；transport 拒绝回归：无未处理 rejection、load 成功 |
| 6 | 每轮工具图验真实 typed 出站 | **executed** | ACP `9028555`：shot-server zlib 合成 6 色随机 PNG + ground truth 落盘；mock 解码当前轮 tail 的 typed 图（三 wire 原生形状）并按解码色作答；缺图/旧图/坏图显式失败；表格 18 轮/wire 含 4 已验证图轮（3 wire = 12），cancel-during-tool 不计；更正旧"12 图轮"收据（实为 9） |
| 7 | 取消严格命中阶段 + 无额外主请求 | **executed** | Codex `73c1828b5`：MidDelta 先等客户端真实消费 `AgentMessageContentDelta` 再 Interrupt（复用 WS 家族握手）；ACP stress：ready 超时即失败、三阶段前置条件必须实证（请求到达/流 delta/工具 start）、取消后主请求数 0、title/guardian 独立计数 |
| 8 | 零残留探针测真实对象、失效显式失败 | **executed** | ACP `af0e33d`：stats/pgrep//proc 探针失败进 `probeFailures` 并使 run FAIL；每轮 PID 树（wrapper + 后代 + lstart 防 PID 复用）关闭后 ≤10s 归零核查；全机 pgrep 移除；模拟探针故障自检模式（stats/pgrep）exit 1 实证有牙 |
| 9 | 冷重连/分页比较完整 payload | **executed** | `73c1828b5`：canonical JSON 全 payload 比较（仅 startedAt/completedAt/durationMs/itemsView 归一化，逐项理由写在 helper）；夹具扩 cap 截断轮；视图含热读/attach 后读/冷读/fork/limit=1 分页合并；旧格式最小字段 rollout 读取降级测试；"kill resume" 更名为 drop-without-shutdown 并注明非 SIGKILL。残留：v2 级 McpToolCall-图像 canonical 比较（图像字节往返已由 core rollout 测试与 ACP 冷恢复图轮覆盖） |
| 10 | 最终源码离线三协议验收 | **executed** | 源 `73c1828b5` dirty=0、rustc 1.95.0、V8 150.4.0 配对工件按仓内可信 manifest 双层校验；macOS 18/18 场景 + 54/54 压测（逐 wire verifiedImage=4、probeFailures=0、openSocketsAfter=0）；Linux/Windows 原生轨道见 §3 |
| 11 | 有限额真实模型小批验收 | **executed** | 预算 48 attempts（含重试/辅助）、输出 2048、输入 ≤32K；NUWAX_* env 全程决定 endpoint/model/wire/key（本地 sink 预检证实）；主路径（GLM chat）：文本/真实 shell 工具（磁盘验证）/截图（MCP 真图+颜色 ground truth）/中途取消/冷恢复召回；三 wire 基础（GLM chat、STEP anthropic、GLM responses）+ 备用切换 MIMO；3 张随机色/数字/位置合成图 vision（GLM-5.3-Flash 官方原生多模态）；ACP smoke（prompt→/compact→续轮→冷 load）。实际消耗见 §5 |
| 12 | 外呼挂起 / CI / 平台分诊 | **executed** | 外呼根因定案并修复（`c36eac709`：回退保留配置的 chatgpt_base_url + 启动 residency 同步 5s 有界；计数拒绝代理回归在 CI 也可判别）；macos lane 连续"取消"= 两次均 181 分钟撞 fork-cargo-pr.yml:37 的 180 分钟 job 超时（非测试问题，处置交 Codex）；CI ubuntu 新增 3 失败本机全过（7/7，环境敏感）；其余分诊见 §6 |

## 1. 产品修复（4 个，全部带真实复现 → 修复 → 回归闭环）

### 1.1 记忆 claim 清理 + 错误覆盖（Codex items 1+2，`0ab3ad112`）

- 复现（Codex 提供的外部 oracle，本轮复跑确认修复）：`MEMORY_REVIEW_LOCK_SECONDS=1` →
  修复前 job 停留 `running`（1 小时 lease）；修复后 1.9s 退出、`pending`、retry_at=+5s。
  `MEMORY_REVIEW_ERROR=1` → 修复前被覆盖成 clean cancel（pending、无 last_error、5s retry）；
  修复后 `error` + `last_error=failed_agent` + retry_remaining 3→2 + 3600s backoff。
- 机制：`ThreadManager` 新增 shutdown-bound 任务跟踪器（tokio-util TaskTracker）；
  `start_memories_startup_task` 的管道任务改为 tracked spawn；`agent::handle` 内联执行
  （不再二次 detach）；app-server `shutdown_threads()` 在线程全停后有界等待 tracked 任务
  （10s；超时放行，lease 到期仍是恢复兜底——长锁探针实测 5.19s 有界退出、job 留
  `running` + 未来 lease）。`loop_agent` 改为 `subscribe_status()` 订阅 + 粘性错误
  （1s 轮询窗口内被 Shutdown 覆盖的 Errored 不再丢失；成功重试仍可翻案）。
- 回归：`exec/tests/suite/memory_exit_contract.rs` 三个真实子进程合同
  （本地 mock vendor + 独立进程查 jobs 表）：锁竞争释放 / 错误预算与 backoff /
  长锁有界退出留 lease。3/3 绿（mac）；Linux 轨道绿（§3）。

### 1.2 ACP 恢复 usage 三缺陷（Codex items 3+4+5，`056fc0c`+`c3d2278`）

- Codex 三个复现脚本全部翻转：REVIEW repro 由红转绿；100 次创建/关闭后孤儿条目
  100→0；transport 拒绝后 Node 不再未处理拒绝退出（exit 0、错误恰好记日志一次）。
- 设计：per-load 显式 arm/settle/clear 的 capture（复用 `takePendingRestoredTokenUsage`
  语义）；armed 期间 `thread/tokenUsage/updated` 归 load 所有并抑制投递给旧 handler；
  通知顺序事实（thread_processor.rs:4424-4442 响应先行）支撑 install 点消费 + 迟到窗口
  （下次 handler 注册冲刷/线程关闭/dispose/cap）。cap 64 只驱逐已结算、绝不动 in-flight。
- 回归：新 `CodexAppServerClient.test.ts` 8 测试 + `token-usage-events.test.ts` 3 测试；
  定向 20/20、全套 817 pass/32 skip、typecheck 干净。

### 1.3 启动回退把 bundle loader 重指向生产（item 12 外呼根因，`c36eac709`）

- 根因链（子代理全文核对源码 + 双机探针定案）：启动配置加载失败 →
  `load_default_config()` 丢弃用户 `chatgpt_base_url` → 第二次
  `replace_cloud_config_bundle_loader` 指向硬编码生产 `https://chatgpt.com/backend-api/` →
  启动期 `sync_default_client_residency_requirement().await` 成为唯一 20s 界的外呼。
  验机 DNS 黑洞下 initialize 超 10s deadline；CI 通网时静默外呼生产。
  （此前"SYN-SENT=模型目录刷新"的猜测不成立：models refresh worker 是后台的、5s 界。）
- 修复：回退时保留 bootstrap 的 `chatgpt_base_url`；启动 residency 同步 5s 有界 + warn。
- 回归：`startup_config_fallback_keeps_bundle_on_configured_base_url`——HTTPS 经计数拒绝
  代理（loopback mock 走 NO_PROXY 豁免）断言零生产外呼 + 有界 initialize + resume 仍按
  配置 URL 报 401 错误。联网机器上旧代码也红（回退后的第二拉取直连生产）。

## 2. 验收器加固（items 6/7/8/9）

- 图片逐轮验证（ACP `9028555`）：mock 在当前轮 tail 中按 wire 原生形状提取 typed 图，
  纯 JS PNG 解码（filter 0-4、CRC 校验）后按平均像素分类颜色作答；driver 用
  shot-server 的 ground truth（`SHOT_TRUTH_PATH`）核对每轮答案；缺图/字节重复（旧图）/
  解码失败均为显式失败；单测 24/24（含负向合同）。
- 取消阶段握手：见 §0.7；压测三阶段前置实证 + 取消后主请求 0 + 辅助请求
  （title/guardian）独立计数入 stats。
- 探针有牙：`STRESS_SIMULATE_PROBE_FAILURE=stats|pgrep` 自检模式实测 exit 1
  （否则 18/18 全过的运行也被判失败）——证明合同无法被探针故障静默放绿。
- 压测表格升级为每 wire 18 轮（54 总；4 已验证图轮/wire=12 图轮，满足 ≥12；
  修正旧执行记录的"12 图轮"错误计数——旧工件实际 9）。

## 3. 最终源码离线验收（item 10）

源 `73c1828b5`（dirty=0），rustc 1.95.0，V8 150.4.0 ptrcomp_sandbox 配对工件
（openai/codex release tag，双层 sha256 对 `third_party/v8/rusty_v8_150_4_0_release_manifests.sha256` 校验）。

二进制收据（sha256 前 8 位）：

| 二进制 | macOS | Linux | Windows |
|---|---|---|---|
| codex CLI | fd7223f0 | 33a3edfe | （构建中/见 §3.2） |
| codex-exec | 31948863 | eb5cd946 | … |
| codex-app-server | 67068012 | 3999488 | … |
| codex-code-mode-host | d08f739b | 17522ab8 | n/a |
| test_stdio_server | f1022b5d | fb0a5a13 | … |

### 3.1 mac 主流程（全绿）

- 18/18 场景 ×3 wire（CLI fd7223f0，summary-18.json）。
- 54/54 压测：每 wire 18/18、verifiedImage=4、cancels=5、probeFailures=0、
  mockOpenSocketsAfter=0。

### 3.2 Linux 原生（192.168.32.163，72 核）

轨道与收据（final-tests.log + linux-acp.log）：

| 轨道 | 结果 |
|---|---|
| exec memory_exit_contract（真实子进程） | **3/3** |
| memories-write + state | 288/288 |
| bridge | **308/308** |
| core rig/interrupt 家族 | 155/157；2 失败（interrupt_long_running_tool…/timed_out_interrupt_hook…）在干净 2f2ddc157 同机同样失败——验机预存环境性 |
| app-server read/resume/fallback 家族 | 14/14（cold-reconnect 首跑红：测试夹具缺陷，`e235002d0` 修复后过，见 §3.4） |
| 18 场景 ×3 wire（Linux CLI 33a3edfe） | **18/18**（/tmp/acp-val-wires.LDdbl6） |
| 54 轮压测 ×3 wire | **54/54**（每 wire 18/18、verifiedImage=4、probeFailures=0）；首轮 2 wire 各 1 轮冷重启红＝PID 树合同抓到更新检查 `git ls-remote`（github 黑洞、产品 30s 界内）——ACP `63a58d6` 按类分窗后全绿，产品界本身已正确 |

### 3.3 Windows 原生（192.168.32.235，MSVC）

| 轨道 | 结果 |
|---|---|
| bridge | **306/306** |
| exec memory_exit_contract | **3/3**（首跑 1 红＝父进程读表时序窗，`e235002d0` 改类别判别后过） |
| app-server read/resume/fallback 家族 | 12/12（cold-reconnect 同夹具修复后过） |

### 3.4 既有套件回归（最终树）

- memories-write + state：288/288（-j 2）。
- rig_interrupt（严格握手后）：4/4；image_history（更名后）：3/3。
- thread_read：cold-reconnect canonical 5 视图 + legacy 读取 2/2；fallback 回归 1/1。
- **跨平台夹具修复（`e235002d0`）**：cold-reconnect 的工具轮此前只在 macOS 恰好可行
  （readOnly 线程沙箱恰放行 echo；Linux/Windows 不放行）——改为 command_exec 套件同款
  DangerFullAccess；memory-exit 的 retry 窗按子进程时钟读表时序改为类别判别。修复后
  三平台全绿。
- thread_manager 家族：-j 1 42/42（并行下的 start_thread_keeps_internal_threads_hidden
  为已知负载干扰：单跑 3s 绿、family 并行红、干净 HEAD 同红——非本轮回归）。
- message_processor::thread_lifecycle 4 测试：干净 HEAD 本机同 TIMEOUT（预存环境性，
  CI ubuntu 通过）——非本轮回归，交 Codex 处置。

## 4. 真实容器主门

app-runtime 镜像（node 22 + glibc 2.41，nuwax 私有 registry）内挂载最终树运行
（container-gate.log）：

- 6 场景 chat wire：**通过（rc=0）**。
- 18 轮压测 chat wire：**18/18**（verifiedImage=4、probeFailures=0、openSocketsAfter=0）。

在跑容器未受影响；镜像内未安装 rust 工具链，故容器门为 ACP+二进制层（非源码构建层）。

## 5. 真实 API 小批（item 11）

预算 48 attempts / 输出 2048 / 输入 ≤32K；凭据仅经 `.env.local`（gitignored，
用户 2026-10-12 授权）进子进程 env。两轮启动失败均发生在 env 构造期
（mac bash 3.2 无 `${var^^}`），未发生真实请求；cancel 的 exec+SIGINT 形态在
非交互后台停滞后改走 ACP `session/cancel`（notification）路径。

**最终 12/12 全过（summary: /tmp/acp-live-val.RxD7nf/summary.json，账面 44/48，
tokens in=162,092 / out=1,733；单请求输入 8.8–13K < 32K，输出由
NUWAX_MAX_OUTPUT_TOKENS=2048 硬限）：**

| 流程 | 结果 |
|---|---|
| env 决定性（本地 sink，零真实请求） | PASS：NUWAX_BASE_URL/WIRE_API 命中 /v1/chat/completions |
| GLM chat / STEP anthropic / GLM responses 基础 | PASS ×3（pong） |
| 备用 provider 切换 MIMO chat | PASS |
| 主路径文本（GLM chat） | PASS |
| 冷恢复召回（跨进程密语 LIVE-KIWANO-7） | PASS |
| 真实 shell 工具（磁盘验证 tool-marker.txt=marlin） | PASS |
| 取消（ACP session/cancel，真实流 delta 后） | PASS：stopReason=cancelled |
| 截图（MCP shot-server 真图→模型命名真色） | PASS：'red'（vision 经工具证实） |
| 直接 vision（3 张随机色/数字/位置合成图，view_image，GLM anthropic） | PASS 3/3（seed 919222744；提示词不含答案） |
| ACP smoke（prompt→/compact→续轮→冷 session/load） | PASS |

未消耗的授权项：真实 Responses 端点的 STEP 验证（Step 未提供该端点，不外推）；
guardian/标题等辅助路径的真实单独验证（离线 mock 已覆盖路由）。GLM-5.3-Flash
多模态能力依据官方/社区文档（智谱发布说明、ModelScope 模型页）先查后用。

## 6. CI / 平台分诊（item 12）

- **macos lane 连续取消根因**：两次 run 的 macos job 分别运行 3h1m4s / 3h1m18s，
  撞 fork-cargo-pr.yml:37 的 `timeout-minutes: 180` 被 GH 标记 cancelled——22K 测试在
  macos runner 超 3 小时。非测试内容问题；提高该 job 超时/分片/取舍由 Codex 决策
  （本轮不擅改 CI）。
- CI ubuntu 新增 3 失败（hooks×2、guardian_subagent cancel 竞态）：本机 7/7 全过，
  定性 CI runner 环境敏感；未盲改产品码。
- Windows 20 新形态 / 11 慢性 ubuntu：维持上轮清单；本轮 Windows 原生主子集复验见 §3.3。
- AppArmor/userns：维持上轮探针结论（受限主机 bwrap 依赖测试需容器/root 路径），
  本轮未改测试跳过。

## 7. 本轮提交（两仓）

| 仓 | 提交 | 内容 |
|---|---|---|
| codex | `0ab3ad112` | fix(memories): 有界等待 claim 清理 + 粘性终态（items 1+2，含 3 子进程合同） |
| codex | `c36eac709` | fix(app-server): 启动回退保留配置 bundle base + 启动同步 5s 有界（item 12） |
| codex | `73c1828b5` | test: 严格中断握手 + 全 payload 历史读比较 + 旧格式读取 + drop 更名（items 7/9） |
| ACP | `056fc0c` | fix(acp): per-load 恢复 usage capture（items 3+4） |
| ACP | `c3d2278` | fix(acp): 恢复 usage_update best-effort catch（item 5） |
| ACP | `6e4dc6b` | test(acp): reload/拒绝回归 |
| ACP | `9028555` | test(acp-validation): 工具图轮端到端 ground truth 验证（item 6） |
| ACP | `af0e33d` | test(acp-validation): 探针有牙 + PID 树残留核查（item 8） |
| ACP | `03171b0` | test(live-validation): 真实厂商小批验收 harness（item 11） |
| ACP | `5f66052` | test(live-validation): 修 usage 解析 + ACP 路径实网取消 |
| ACP | `63a58d6` | test(acp-validation): 残留窗按更新检查类分窗（Linux 冷重启红定案） |
| ACP | `0ef32fe` | test(acp-validation): 宽限后收割本轮自有的更新检查孤儿进程 |
| codex | `e235002d0` | test: 子进程/canonical 合同的跨平台夹具修复（沙箱 + 时序窗） |

工件（my-docs/.../artifacts-2026-10-17/，均无凭据/无端点泄漏）：
live-smallbatch-summary.json、mac/linux summary-18.json、mac 三 wire 压测
（fd7223f0）、linux 三 wire 压测（33a3edfe）、压测器自检期间 vendor 二进制参照
（3ce16ae8）。容器收据在验机 /tmp/container-gate.log。

（外加最终文档提交：本文件 + 提示词 + Codex 复审归档。）

## 8. 交 Codex 事项（非阻断）

0. **更新检查孤儿子进程**（Linux 冷重启红定案时发现的产品加固项）：app-server 在自身
   30s 界到期前被杀时，其 `git ls-remote`（curated plugins 更新检查）成为孤儿——唯一会
   杀它的进程已死，黑洞网络上可悬挂分钟级。建议给更新检查子进程加父进程死亡联动
   （Linux PDEATHSIG 等价/Child guard on drop）。压测侧已按类分窗 + 宽限后收割
   （`63a58d6`+`0ef32fe`），产品界本身（startup_sync.rs 30s kill+wait）在进程存活时正确。
1. macos lane 180 分钟 job 超时的处置（超时/分片/取舍）。
2. v2 级 McpToolCall-图像 canonical 比较（本轮图像字节往返由 core rollout 测试 +
   ACP 冷恢复图轮覆盖）。
3. CI ubuntu 3 新失败（本机绿）与 message_processor::thread_lifecycle 本机预存 TIMEOUT
   的环境定性复核。
4. 真实 API 剩余预算与未尽项（见 §5 填充）。

## 9. 第二轮继续开发（同日追加：发现问题→修复）

用户指示"继续开发，发现修复的问题"后的追加工作，起点 `dd7221e7f`：

| # | 发现 | 处置 | 提交 |
|---|---|---|---|
| 1 | 更新检查孤儿子进程（§8.0 的产品加固项）：app-server 被杀早于自身 30s 界时 `git ls-remote` 成孤儿，黑洞网络悬挂分钟级 | **fixed**：`git_command` 加 PDEATHSIG pre_exec（复用 core/spawn.rs 与 utils-pty 的既有 helper，Linux 生效）；Linux 子进程回归红验证（无修复 6.3s 红 / 有修复绿） | `4097b2d57` |
| 2 | item 9 残留：v2 级 McpToolCall-图像 canonical 比较缺失 | **executed**：image_scenario 工具图像在热读/limit=1 分页/冷重启/fork 四视图 canonical 全等 + base64 点检；开发中两次红（REPL-only js 工具、空参反序列化失败）证明有牙 | `93cd60142` |
| 3 | "CI 慢性"4 项在本机 mac 全部复现（此前两轮被误标为 CI 环境性） | 2 修复 + 1 深诊断 + 1 同根因：见下 | `203679e02` |
| 4 | Windows CI 20 个新形态首批复现（旧树=CI 同 SHA） | unified_exec×4 与 cur 检测×4 在真机全过（CI runner 特有）；`elevated_non_tty_cmd_forwards_env_output_and_exit` FAIL+LEAK 为真机可复现新项（36s，待分诊） | 记录见 §6 |

慢性 4 项明细：
- `unapproved_hosts_do_not_trigger_dns`：**测试缺陷**——共享策略 helper 装的公网 DNS fixture
  按设计优先于 lookup 参数，第二段"Approved 解析私网必须拦"从未真正走到。测试改为安装
  私网解析 fixture，真正驱动审批后私网复查分支。
- `unusable_parent_checkpoint_fails_closed`：**字面量过期**——枚举早已细分 serialization/
  oversized/unusable，兄弟测试已用新值。对齐。
- `guardian_review_turns_and_tools_reach_analytics`：**场景过时（未修，交 Codex）**——固定
  请求序列假设"3 个 guardian 请求=3 个 review"，但 web-search 的 review 会续采样（一个
  review 多个请求），实测只有 2 个 review 线程、序列错位级联。需要场景级重设计（按去重
  线程收集 + 按线程配对状态 + 非到达序断言；本轮已验证嵌套 turn-metadata 提取与排序比较
  均必要但不充分）。
- `pool_pressure_cancels_the_oldest`：同 websockets 根因待修——guardian 采样器在 provider
  默认 `supports_websockets=true` 下走 WS upgrade 撞纯 POST mock（3 次 upgrade 后放弃，
  17 个分类未发生）。兄弟文件 decisions_lifecycle_tests.rs:34 的 `supports_websockets =
  false` 是既有惯例，该测试漏配。**注意**：这暴露一个待答复的产品问题——WS upgrade 失败
  （mock 返回非 101）后未见回退 SSE POST，是否应有回退需 Codex 定夺。

## 10. 第二轮追加（续）：Windows 新形态深分诊与修复

在验机（真机）复现 CI windows 的 20 个新形态（旧树=CI 同 SHA 2f2ddc157）：

| 组 | 真机结果 | 定性 |
|---|---|---|
| unified_exec ×4（CI 红） | **全过**（16/17；唯一红是另一项 `elevated_non_tty_cmd…` FAIL+LEAK 36s） | CI runner 特有 |
| uds/socket ACL ×2 + private_directory ACL | **10/10 全过** | CI runner 特有（ACL 语义/账户差异） |
| cur 检测 ×4 | **165/165 全过** | CI runner 特有 |
| `conpty_ctrl_c_interrupts_powershell_foreground_child` | **3/3 确定性红**（10.8s） | 真缺陷：Ctrl-C(0x03) 写入 conpty 后 `ping -t` 未被中断，后续 `cmd /C ver` 永不执行；输出全为中文 ping 回复（第一段 127.0.0.1 等待可命中，非 locale 问题）。输入层 `WindowsTtyInputNormalizer` 0x03 透传正确、`conpty_delivers_input_to_foreground_children` 过——问题在控制台事件路由。需交互式 Windows 调试（建议探针：裸 cmd.exe 替代 powershell 复现；检查近期 console 抑制类提交 fbc169827/1a89aec96 对 PTY 子进程控制台挂接的影响） |
| TUI `kitty_local_file_pet_image` | 确定性红（0.03s） | **已修 `26af4d0fb`**：路径 base64 对齐碰撞——canonicalized 路径长度使 "frame.png" 落 3 字节边界时，路径自身编码含内容标记 "cG5n"；改断言 `t=d,`（inline 类型）缺席。mac 验证过；win 复核因验机宕机未完成（见下） |
| TUI `approvals_popup_navigation_skips_disabled` | 确定性红（0.45s） | 真缺陷/待深查：Enter 后未派发任何 `OverrideTurnContext`（"enter should select an enabled preset"）。交 Codex（TUI 内部） |
| TUI `config_overrides_forward_explicit_summary_verbosity…` | 确定性红（0.08s） | 同上交 Codex |

**环境事件**：Windows 验机（192.168.32.235）在两棵验证树的 target 目录填满 C: 后失联
（ping 100% 丢包），后续 win 侧复核中断。恢复后需清理 `/c/tmp/codex-stab-final` 与
`/c/tmp/codex-stab-2f2ddc157` 的 target（或整树）再继续。

慢性修复的 Windows 交叉验证已在新树完成（triage2 A1：network-proxy + guardian-v2 2/2 过）。
