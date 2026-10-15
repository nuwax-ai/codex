# CI 与使用路径验收账本（2026-10-16）

> **Codex 同日复审更正（以此为准）**：截断簇已确认是无条件10K夹断纯文本的跨平台回归，
> 不是已证实的Linux取数分叉；cap累计spend没有丢失，旧测试错误要求覆盖context baseline；
> ACP两项“产品缺陷”已撤回，修正共享mock及通知关联后同二进制六场景通过。
> Windows作业因磁盘耗尽中断，原76只计FAIL并漏掉TIMEOUT/FAIL+LEAK，不能作为完整结论。
> 本轮实际修复、完整统计、原生补录和最终复验见
> [复审与收口记录](codex-stability-review-2026-10-16.md)。以下Round12内容保留历史过程。

Round 12。执行者 Claude Code（本文件）；复审请求直接交 Codex。
范围：`test` 分支 `8e7c5904b`（CI run `38782462743`）→ 本轮新增
`e6ab13a18`（夹具修复）与 ACP dev `2b4ae13`（验收归档）。未 push、
未手动 dispatch、未调用付费 API。

## 1. CI 首败（run 38782462743，headSha 已核 = 8e7c5904b）

| Job | 结论 | 备注 |
|---|---|---|
| bazel bridge gate (ubuntu) | ✅ success | |
| nextest (ubuntu-latest) | ❌ failure | ~25 个唯一失败测试 |
| nextest (windows) | ❌ failure | 76 个唯一失败测试 |
| nextest (macos-latest) | ⚠️ cancelled | 无结论，非绿非红，不能引用 |

对照基线：run `38528171490`（`bc4517dad`，2026-10-14）。注意
`bc4517dad` 早于 main 合并内容（`bc4517dad..8e7c5904b` 共 593 个提交，
含整个上游合并），所以"新增"= 上游合并 ∨ 第 1–11 轮 ∨ Codex 集成
提交，三者的并集；基线绿不能归咎任何单一来源。

- ubuntu：基线 3 个失败（host_roots、shell_snapshot 慢性 + 1 已消失）→
  现在 ~25，净新增 ~22。
- windows：基线 24 → 现在 76；15 个两侧重叠（慢性），61 个新增。
- 各失败的准确平台/断言/行号已存
  `/tmp/rig044-ci-firstfail/{ubuntu,windows}-clean.log`（本地，随会话）。

### 1.1 已定位根因的簇

**A. 原记录称ubuntu截断簇（现已确认跨平台纯函数回归）（truncation×3 + code_mode×5 + guardian_context_budget
+ step_settings 等，~12 个）——首败在 Codex 集成提交内。**
- Linux 主验机（192.168.32.163）复现：`4a0dd66d5` 红、`914a400d7` 绿
  （二分需先 `cargo build -p codex-rmcp-client --bin test_stdio_server`，
  否则 0.05s 假失败污染结果——本轮一度被坑，已更正）。
- 特征：`tool_call_output_configured_limit_chars_type` 得 39,004 字节
  （= 默认 10K token 语义）而非配置的 ~400,000；MCP 变体
  `prepared=output_token_limit Some(30000)` 选择正确、`model_info`
  策略 Tokens(50) 为测试故意设置，最终产物却按默认 10K 截断且尾部带
  `"env":null}` JSON 信封。
- mac 同代码同 feature（带与不带 rust-rig）均绿 → 平台相关分叉。
- 待 Codex 续查方向：`9e4a4276b`（共享硬限）与 `4a0dd66d5` 之间的
  item-wise/formatted 两级截断在 Linux 的取数路径。

**B. `capped_sampling_reports_terminal_usage_over_v2`——旧合同冲突，原“存储缺陷”归因撤回。**
- mac 本地即复现（app-server::all，`-p codex-app-server --features
  codex-core/rust-rig`）；`29d55354c`（活动起点）即红 → 随 main 合并
  进入（上游测试 `e4797d0dc` vs 本仓 SpendingOnly 语义）。
- 机制：`TokenUsageContext::SpendingOnly`（session/mod.rs:5173）持久化
  副本不更新 `last_token_usage`（赋值在 `set_token_info` 之后），终局
  通知从状态读出 last=0 盖掉此前 last=14。
- 修复建议：capped 终态的持久化 info 同步更新 last，或终局通知不回读
  陈旧状态。属独立历史缺陷，本轮未改码。

**C. `anthropic_interrupt_during_tool_runs_it_once` 跨平台夹具——已修
  （本轮 `e6ab13a18`）。** require_escalated + OnRequest + 测试内
  `Op::ExecApproval` 应答握手；命令 Unix `sleep 60` / Windows
  `ping -n 61 127.0.0.1`（cmd/PowerShell/Git Bash 均真实阻塞 ~60s）。
  mac 实测过；Linux/Windows 原生复验见 §3。

### 1.2 待分诊（超出本轮剩余预算，单列）

- fs_watch ×4（ubuntu+windows 双平台新增，0.01–0.03s，panic 在
  app-server/src/fs_watch.rs:237/267/314/351）。
- windows 独有 61 个：doctor、sandbox/junction/elevated 族、sqlite
  runtime 族、`transform_for_direct_spawn_windows`、
  `nuwax_env_startup_reaches_rig_with_tool_round_trip_and_no_leak`
  （env 三件套首次在 windows CI 上跑）等。
- `analytics guardian_review_turns_and_tools`（双平台新增）、
  `history::large_explicit_budget`（双平台新增，0.00s 单测）、
  `astra_plugin_policy` insta 快照漂移（ubuntu）。
- 慢性（基线即红，不新增处理）：host_roots、shell_snapshot×2、
  guardian-v2 pool_pressure/unusable_parent_checkpoint、host_policy
  （windows 慢性；ubuntu 新增？两轮都有，按慢性记）。

### 1.3 环境性（非代码）更正与前车之鉴

- **假爆栈事件（重要）**：裸 `cargo nextest`（无 `RUST_MIN_STACK`）在
  macOS+Linux、1.95/1.99、clean/增量构建下对全部建会话的 core 测试
  确定性 SIGABRT。崩溃栈固定为两层 `Pin<Box<Session::new future>>
  ::poll`。根因：会话 spawn poll 链合法需要 >nextest 默认栈，
  `just test`/CI 一直显式给 8MiB。我的远程脚本漏了该变量 → 第一轮
  Linux 47+22 个"失败"全是环境性，第二轮补齐后几乎全绿。未加栈、
  未改产品码；结论：任何脱离 just 的运行必须 `RUST_MIN_STACK=8388608`。
- **工具链漂移**：rustup 默认从 1.95.0 变 1.99.0；`rust-toolchain.toml`
  只在 `codex-rs/` 下，按 cwd 解析 → cwd 在 codex-rs 外的构建静默用
  1.99.0。CI 用 `dtolnay/rust-toolchain@stable`（=1.99.0）——**CI 与
  仓库 pin 不一致**是结构性风险（fmt/clippy 跨版本漂移、codegen 差
  异），本轮未改 workflow，留给 Codex 决策。
- AppArmor/userns（Linux 主验机）：`unshare -Urn` 仍 `uid_map: EPERM`；
  `environment_permissions_follow_configuration_ownership` 在同机
  `4ca0e4fa0` 绿、`8e7c5904b` 红（bwrap loopback RTM_NEWADDR），
  linux-sandbox 代码范围内零改动——待解释，未归档为环境性。

## 2. ACP 使用路径验收（第二批）

- 归档已入库：ACP dev `2b4ae13`
  （`scripts/acp-validation/`：mock vendor + 录制器、shot-server、
  六场景驱动、runner 含 sha256 收据校验；零凭据扫描通过）。
- 当前二进制 `e6ab13a18`（sha256
  `1b9a2fa74ca39775eafdbf740bc39422c5c75edff26c49285951bf4aba542e55`）
  实测：S1/S2/S4 过；**S3/S5/S6 当时被误判为两个产品缺陷（复审已撤回）**（原版 tmp 驱动共用同一错误mock，不能排除验收器因素）：
  1. MCP 工具在请求 `tools` 列表中以 `mcp__screenshots__take_screenshot`
     广告、服务器 `status=ready`，但执行返回 `unsupported call:` 同名
     ——广告/执行目录不一致。914a400d7 与 e6ab13a18 均复现；10-15 晚
     MiMo 付费轮同工具执行成功 → 回归窗口待二分。
  2. 取消回合后，后续 prompt（/compact、follow-up、召回）从不作为
     user message 出现在模型请求里——vendor 持续收到旧 prompt 作
     last-user（录制 JSONL 直接可见），压缩/召回必然失败。
- Code Mode `image()`/普通 return/`text()` 三分验证：未开始（S3 缺陷
  未修前无法走通图片链路），顺延。
- 真实识图：按约定留待另授权。

## 3. Linux / Windows 原生定向复验

Linux（192.168.32.163，1.95.0，RUST_MIN_STACK=8MiB，`8e7c5904b` 源码
tar + git init + 预建 fixture）：

| 套件 | 结果 |
|---|---|
| bridge 全 crate | 303/303 |
| utils image/truncation + memories-write | 114/114 |
| live replay（mimo,glm,step） | 196/196 |
| env 家族 | 61/62（1 = 上述 bwrap loopback 待解释项） |
| core rig 家族+限额+stress | 51/52（1 = 旧夹具 anthropic_interrupt，e6ab13a18 修复待远端复验） |

Windows（192.168.32.235，原生 MSVC，`e6ab13a18` tar）：fixtures ✓、
bridge exit=0、**core rig 家族+限额+stress 52/52 全过**——含新夹具的
`anthropic_interrupt_during_tool_runs_it_once`（三平台验证闭环：mac ✓
Linux 原生 ✓ Windows 原生 ✓）。utils/env/replay 三套件在本轮收尾时
仍在远程执行，日志在 win 机 `/c/tmp/rig044-val-e6ab13a18/run-*.log`，
下一轮补录。

## 4. 短生命周期 exec 的后台记忆退出行为（第三批，代码级定性）

- 链路：exec 进程退出 → 会话 teardown → 巩固 agent 线程终止 →
  `phase2.rs:471 job::failed(..., "failed_agent")` →
  `mark_global_phase2_job_failed(retry_at = now+3600s)`。
- 定性：**非干净取消**（记失败状态 + 失败计数器），但**不丢工作**
  （input_watermark 保留）且**无硬性重试次数上限**（claim 路径不减
  `retry_remaining`）；代价是 1 小时重试延迟 + 失败指标噪音。SIGKILL
  场景 job 残留 running 态最长占用 lease（3600s）阻塞他进程。
- daemon/ACP（长驻）不受直接影响，延迟后可再 claim。
- 最小修复方向（未实施）：会话终止导致的 agent 中止与真实错误区分
  （cancel 语义 + 短重试延迟或直接释放 claim）；离线回归应断言：
  会话终止 → job 进入取消 flavored 状态且 retry_at ≤ now+小值，而非
  failed_agent+3600。

## 5. 本轮提交

| 仓库 | 提交 | 内容 |
|---|---|---|
| codex（codex/rig-044-20261014） | `e6ab13a18` | anthropic_interrupt 跨平台夹具（独立提交） |
| fork-codex-acp-ts（dev） | `2b4ae13` | 无凭据 ACP mock 验收套件归档 |

## 6. 下一最小批次建议

1. Codex 续查 ubuntu 截断簇（首败 2abe215db..4a0dd66d5 之间；复现命令
   见上）与 fs_watch 双平台新增。
2. 修 ACP 两个产品缺陷（S3 unsupported-call、S5/S6 prompt 丢失）——
   归档 runner 即回归。
3. turn_cap_usage 的 SpendingOnly last 持久化语义修复（B 项）。
4. CI 工具链钉 1.95.0 或显式决策留在 stable（workflow 改动需授权）。
5. Windows 原生流水线收尾 + e6ab13a18 的 anthropic_interrupt 在两台
   原生机复验。
