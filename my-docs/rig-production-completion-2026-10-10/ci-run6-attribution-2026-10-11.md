# run6 827 个非 PASS 终态逐身份归因（2026-10-11）

数据：run `38065399719`（SHA `e02dc3771`）三平台 nextest 日志（本地 `/tmp/run6logs/`）。方法：解析每个身份的完整失败块；**只用显式证据**分类——panic/assertion 原文、assert_cmd 错误、SIGABRT 信号行；签名聚类 + 噪声不计数。产物：`ci-run6-failure-ledger.tsv`（827 行，新增 category 与首败摘录列，不含日志正文/凭据）。修正前账本的 538+26=564 个未归类降至 154（140 assertion-open + 14 no-evidence，均为待逐测triage，不是已定性）。

## 归因结果（827 = 全部终态）

| 类别 | 数量 | 平台分布 | 机制（证据链） |
|---|---:|---|---|
| code-mode-unavailable-indirect | 415 | u154/m151/w110 | 见下"主根因" |
| assertion-open | 140 | u75/m29/w36 | 真实断言失败，待逐测试 triage（首败摘录在账本） |
| code-mode-host-absent-explicit | 80 | u33/m33/w14 | `could not locate binary "codex-code-mode-host"`（assert_cmd 原文） |
| harness-deadline-explicit | 63 | u29/m9/w25 | 测试harness 的显式 deadline 超时（`timeout waiting for event: Elapsed` / `deadline has elapsed`）；机制显式、环境 vs 缺陷的处置逐测试开放 |
| bwrap-userns-sandbox | 62 | u62 | 显式 bubblewrap/用户命名空间错误（非 RTM_NEWADDR 变体） |
| bwrap-rtm-newaddr | 47 | u47 | 显式 `bwrap: loopback: Failed RTM_NEWADDR: Operation not permitted` |
| no-evidence-unclassified | 14 | u8/w6 | 块内无 panic 无 deadline 文本（多为 setup 阶段夭折），保留待查 |
| stack-overflow | 3 | u1+w1+1 | 同一测试 `guardian_ephemeral_retry_preserves_parallel_trunk_and_fork_history`：ubuntu SIGABRT（`has overflowed its stack`）+ windows 0xc00000fd + 计入 ubuntu SIGABRT 行 |
| windows-powershell-module | 2 | w2 | runner 的 `Get-Acl` PowerShell 模块加载错误（环境性，显式） |
| fail+leak-unclassified | 1 | w1 | FAIL+LEAK 组合态，无 panic 文本 |

另：**Windows 311 个"通过但泄漏"终态行**（nextest LEAK 标记，不在 827 计数内，账本口径仍为非 PASS）——Windows 上测试进程/线程泄漏的独立卫生信号，建议单列跟进。

## 主根因：code-mode host 家族 = 495/827（60%）

**因果链（每一环有证据）**：

1. 上游 `97576b179`（#36217，2026-07-30）"Run code mode exclusively through the standalone host" 删除了进程内 V8 runtime provider，默认 provider 只剩：`Feature::CodeModeHost`（或 `disable_in_process_fallback`）→ 依赖外部 `codex-code-mode-host` 二进制的 ProcessOwned；否则 → `DisabledCodeModeSessionProvider`（`core/src/thread_manager.rs`）。
2. fork 的 CI 车道因 v8 150.4.0 `ptrcomp_sandbox` 预编译归档缺失（既有登记）`--exclude codex-code-mode-host` → 二进制不构建 → 80 个身份显式 `could not locate binary`。
3. 其余 415 个：`Disabled` provider → `code_mode_available=false` → `effective_tool_mode` 静默回退 `ToolMode::Direct`（`core/src/tools/mod.rs:88`）→ registry 无 `exec` custom tool → 模型的 `exec` 调用得到 `unsupported custom tool call: exec`（`core/src/tools/registry.rs:878`）→ 测试断言崩溃（`first cell yielded`、cell ID 缺失、wiremock 校验计数差一、"Script completed" 缺席、analytics deadline 等多种表象，同一根因）。
4. **本地复现**：`code_mode_can_return_exec_command_output` 在本机以与 CI 完全相同的签名失败（`items.len()==1`，内容即 `unsupported custom tool call: exec`）；`code_mode_can_resume_after_set_timeout` 同。即该家族不是 CI 环境问题，是当前树在"host 二进制缺席"配置下的确定性行为。

**处置**：与既有 V8 配对工件登记同一阻塞——修复=工件配对或车道内构建 host（同一缺口）；不能靠 exclude 测试或假 host 求绿。本机可直接用该家族做修复验证（无需 CI）。

## 各类别下一步

- **assertion-open（140）**：唯一需要逐测试工程判断的池子；账本首败摘录可直接当 triage 入口。建议先按 binary 分桶（core 为主）抽样。
- **harness-deadline-explicit（63）**：机制显式（deadline 超时）；逐身份判定 CI 2-vCPU 时序 vs 真缺陷——不贴"并发固有"标签关闭。
- **bwrap（109）**：runner 嵌套命名空间能力；与既有 T32 环境裁决同一事项（自托管/特权 runner 或受控校准）。
- **stack-overflow（3 行，2 个身份）**：需 Linux/Windows 显式 4MiB 线程复现（既有 batch 3 项）。
- **windows-powershell（2）/pass+leak（311）**：Windows 环境卫生项。

## 口径声明

- 分类只升级证据（从未分类→显式机制），未把任何项标成"通过"或关闭验收。
- 408→410 的 ubuntu 差异已对齐（SIGABRT 行此前未入解析集，人工补录）。
- `results.md` 批 S 的"strict missing host 60/60/36"口径被本轮细化：显式 80 + 间接 415 同根因；批 S 的"明确缺host输出块"计数是输出块口径，本账本是身份口径。
