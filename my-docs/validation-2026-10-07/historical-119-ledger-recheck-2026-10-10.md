# 历史账目 119 独立复算核对（2026-10-10）

性质：**只读文档核对**。本文件不修改任何代码与其他文档；唯一产出即本文件。所有"复算"均指基于本机标注文件 `my-docs/validation-2026-10-06-round2/historical-failures-per-item-labels.md`（下称"标注文件"，396 行 = 表头 13 行 + 表格 2 行 + **数据 383 行**，编号 #1–#383 连续无缺）的逐行统计，以及本机 JSON 工件与文档陈述的交叉。**旧机卷 `/Volumes/soddygo` 当前未挂载**，所有 tsv/日志级身份联结在本机不可执行，相关项明确标注。

核对输入（本机实际读取）：

| 输入 | 位置 | 状态 |
|---|---|---|
| 标注文件 | `my-docs/validation-2026-10-06-round2/historical-failures-per-item-labels.md` | 本机，已全文逐行解析 |
| 待复核清单 | `my-docs/codex-pending-review-2026-10-09.md`（第 10 项 + 日志计数节） | 本机 |
| 数字口径 | `my-docs/validation-2026-10-07/pkg3-failure-signatures-worklist.md` §6 及各根因节 | 本机 |
| JSON 工件 | `my-docs/validation-2026-10-07/*.json`（5 份） | 本机 |
| 归因说明 | `my-docs/validation-2026-10-06-round2/historical-failure-attribution.md` | 本机（佐证引用） |
| 10-10 修复批 | `my-docs/claude-p1-fixes-2026-10-10.md` | 本机（exec-server 本机全量佐证） |
| still249.tsv / 各验证日志 | `/Volumes/soddygo/git-workspace/codex-tmp/...` | **不在本机（卷未挂载）** |

---

## 1. 断言 1 核对：249 → 124（115+9） → 15 → 119

### 1.1 基数与四态（标注文件"10-07 复验"列逐行统计）

| 复验列状态 | 行数复算 | 文档声称 | 判定 |
|---|---:|---:|---|
| 本次复跑仍败（threads=2/retries0），即 t2 仍败 | **249** | 249 | 一致 |
| threads=2 复跑通过 | 89 | 89 | 一致 |
| 全套重载首试即过 | 34 | 34 | 一致 |
| 空闲 18 路复跑恢复 | 11 | 11 | 一致 |
| 合计 | 383 | 383 | 一致 |

旁证：`validation-2026-10-07/historical-383-state-check.json` 的 `historical_statuses`（t2_still_failed 249 / initial_passed 34 / r1_passed 11 / t2_passed 89）与上行复算完全一致。

249 的内部拆分亦复算吻合：未归因 201 + flaky(A/B) 28 + 已确认环境 20 = 249，与标注文件头部"201/20/28"陈述一致。

### 1.2 恢复标签逐类行数（归因列逐行识别）

标签识别规则：`根因R1`/`根因R1已修`、`根因R4已修`、`根因R2已修`、`根因R7已修`、`R10暖态复验通过`、`R6相关切片复验通过`。仍败行中带 10-08/10-09 日期戳但不含上述任一标签者为 **0 行**（无漏识别的第三种措辞）。

| 标签 | 行数复算 | 行号 | 所在复验态 | 文档口径 | 判定 |
|---|---:|---|---|---:|---|
| R1（含已修 2） | **79**（77+2） | #1、#9–#84 区间等 | 全部 t2 仍败 | R1 79 | 一致 |
| R4 已修 | **4** | #109–#112 | 全部 t2 仍败 | R4 4 | 一致 |
| R2 已修 | **22** | #241–#262（全为 `codex-network-proxy`） | 全部 t2 仍败 | R2 22 | 一致 |
| R10 暖态 | **5** | #135（aws-auth）、#235/#237/#238/#239（http-client） | 全部 t2 仍败 | R10 5 | 一致 |
| R6 切片 | **14** | #285–#296、#305、#320（全为 `codex-tui`） | 5 仍败 + 9 已过 | R6 14 | 一致 |
| **10-08 小计** | **124** | | 115 仍败 + 9 已过 | 124 = 115+9 | **一致** |
| R7 已修（10-09） | **15** | #204–#218（全为 `codex-exec-server remote::registration_retry::tests::registration_requires_a_confirmed_conflict_before_replay::*`） | 全部 t2 仍败 | 15 | 一致 |
| **恢复标签总计** | **139** | | 130 在 249 内 + 9 在 89 内 | — | — |

### 1.3 递推复算

| 步骤 | 复算 | 账面 | 判定 |
|---|---|---|---|
| 原 249 中被 10-08 标签覆盖（新增恢复） | 79+4+22+5+5 = **115** | 115 = 83+22+5+5 | 一致 |
| 此前已通过再通过（R6 的 9 行，复验态"threads=2 复跑通过"） | **9** | 9 | 一致 |
| 10-08 后仍开放 | 249−115 = **134** | 134 | 一致 |
| 10-09 R7 转绿 | **15** 行 | 15 | 一致 |
| 当前仍开放 | 134−15 = **119**；独立验算：249−130 = **119**；逐行枚举无标签仍败行 = **119** | 119 | **一致（三条独立途径同值）** |

**结论：断言 1 的全部数字在本机可由标注文件独立复算并完全吻合。** 唯一不能在本机完成的是身份级前提：标注文件的 249 行是否与旧机 `still249.tsv` 逐身份相同（见 §6）。

---

## 2. 断言 1 附带核对：新增恢复 115 vs 重复通过 9 的可支撑性

按 `(binary, test_name)` 解析标注文件（binary 取测试字段首 token；`codex-app-server` 与 `codex-app-server::all` 是不同 nextest 测试二进制，分开计）：

| binary | 标签 | 新增恢复（t2 仍败→标绿） | 重复通过（t2 已过→再过） |
|---|---|---:|---:|
| codex-app-server | R1 | 1（#1） | 0 |
| codex-app-server::all | R1 / R4 | 78 / 4 | 0 / 0 |
| codex-network-proxy | R2 | 22 | 0 |
| codex-http-client | R10 | 4 | 0 |
| codex-aws-auth | R10 | 1 | 0 |
| codex-tui | R6 | 5 | **9** |
| codex-exec-server | R7 | 15 | 0 |
| **合计** | | **115** | **9** |

判定：

- **115/9 的划分可由标注文件本身支撑**：划分依据是"10-07 复验"列（仍败=新增恢复，threads=2 已过=重复通过），R6 的 14 行恰好 split 为 5+9，其余五类标签行全部落在仍败集合。这与 worklist §6"R6 的新增恢复为 5"、§R6"14 行 = 新增恢复 5 + 此前已通过 9"逐字吻合。
- **不可支撑（本机不可复算，需 still249.tsv 及修复批日志）**的部分：
  1. "10-07 复验"列本身是旧机 `ws3-rerun-t2.log` 的转写，其 ground truth 在旧机；
  2. 249 行与 `still249.tsv` 的逐身份等同未联结；
  3. 每个标绿行是否在对应修复批日志（appserver101/、final2/attestation.log、r4-verify*、r2-final3.log、r10-verify4.log、r6-verify3.log、final-verify2.log）中有真实 PASS 行，本机无日志可查。

---

## 3. 断言 2 核对：final19 = core 1 + exec-server 16 + relay 2

`final-verify2.log` 在旧机 `/Volumes/soddygo/git-workspace/codex-tmp/pkg3-r2/`，**本机不可复算其逐行构成**。本机可做的三项检验：

| 检验 | 结果 |
|---|---|
| 算术自洽 | 1+16+2 = 19 = 待复核清单声称的唯一执行数（19 PASS/0 FAIL）✓；而"17+2+1" = 20 ≠ 19，该读法在算术上即不成立 ✓ |
| "registration19"读法 contra 标注文件 | 标注文件中 registration_retry 命名的历史行共 **17 行**：remote::tests 15（#204–#218，R7 标绿）+ relay 2（#223 t2 已过、#224 仍开放）。249 集合内 registration 命名行仅 16（15+relay #224）。即使把 19 行全算 registration，也只有 ≤16 个身份能落回 249；且待复核清单明言"final19 含 guardian 1"——该项为 core guardian（SIGABRT 项，**不在历史 383 清单内**）。故"19 个 registration 恢复"不成立，与断言方向一致 |
| exec16 构成假设（现源旁证） | 本机现源 `codex-rs/exec-server/src/remote/registration_retry_tests.rs` 的 test_case 矩阵共 **16 例**（含历史 15 例名 + `committed_success_body_timeout` 等不在 383 的例），`tests/relay/registration_retry_tests.rs` 恰 2 例。16+2 = 18 与 worklist"registration_retry 家族 18/18"相容；据此最可能构成是 exec16 = 矩阵全 16 例、relay2 = 2 例 relay、core1 = guardian。**此为假设，需 final-verify2.log 逐行确认** |

**遗留疑点（需日志身份才能定案）**：若 relay2 确为 #223+#224，则 relay #224（`registration_retry::shutdown_interrupts_in_flight_registration`，在 249 内且仍标开放）在 final-verify2 已 PASS 却未获转绿标签——119 应为 118；若 relay2 是 #222/#225 或其他，则 119 不变。同理 worklist R7 称"registration **17 项**"而仅 15 行标绿，差的 2 项最可能是 `remote::direct::tests::direct_registration_*`（#201/#202，仍开放）——即 R7 在行级只关闭了 15/17。两项均登记为账目开放差异。

---

## 4. 断言 3 核对：四日志合并 512 个不同身份

四日志均在旧机 `codex-tmp/pkg3-r2/`，**本机不可复算并集**。本机可做的合洽性核算（基于待复核清单给出的分桶数字）：

| 日志 | 唯一执行 | PASS | FAIL | 分桶加和验证 |
|---|---:|---:|---:|---|
| final-verify2 | 19 | 19 | 0 | core1+exec16+relay2 = 19 ✓ |
| dialer-fix-verify | 143 | 139 | 4 | HTTP 111P/4F + ca_env 10P + WS 18P = 143；P=139、F=4 ✓ |
| regression-run | 428 | 419 | 9 | exec16+relay2+watcher29P/1F+AWS1+HTTP53P/7F+network-proxy314P/1F+proxy4 = 428；P=419、F=9 ✓ |
| cache-regression | 143 | 137 | 6 | WS16P/2F + 隐含 HTTP/CA 125 = 143；F=4 TLS+2 WS = 6，与 worklist §R11"首轮 137/143，6 失败=4 已知 TLS 冷态+2 dialer"逐字吻合 ✓ |

并集反推：Σ唯一执行 = 19+143+428+143 = 733，合并 512 ⇒ 净重合 221。已知/可解释重合源：cache-regression 与 dialer-fix-verify 为同一 143 选择（worklist §R11 记为先后两轮）贡献 143；final-verify2 的 exec16+relay2 大概率 ⊆ regression-run 同桶贡献 ~18；regression-run 的 HTTP 60 项与 143 选择的 HTTP 115 项重合约 59–60。143+18+59 ≈ 220–221，**与所需净重合精确相容**。判定：512 与各分桶数字互洽，属"本地旁证通过、日志级不可复算"。

---

## 5. 119 的构成（按根因族）

### 5.1 按 binary 的行数复算（权威，来自标注文件逐行枚举）

| binary | 开放行数 | 行号/说明 |
|---|---:|---|
| codex-app-server | 1 | #2 derive_config 单测 |
| codex-app-server::all | 17 | guardian_v2::history 13（#68–#83 除 #71/#76/#79）+ account 2（#7/#8）+ residency/review 2（#128/#129） |
| codex-core | 1 | #137 managed_network_proxy_decider |
| codex-core::all | 30 | unified_exec_process_events 11、realtime_conversation 10、mcp_startup_grace 3、remote_env 2、tool_parallelism 2、approvals 1、retry_after 1 |
| codex-exec-server | 4 | #199 capability_watchers、#201/#202 direct::tests、#203 noise_tests |
| codex-exec-server::accepted_websocket | 2 | #220/#221 |
| codex-exec-server::relay | 3 | #222、#224、#225 |
| codex-http-client | 6 | #230–#234、#236（route_aware 族） |
| codex-install-context | 1 | #240 brew 环境证据 |
| codex-otel::tests | 6 | #263–#268 otlp_http_loopback 全族 |
| codex-rmcp-client 两个测试二进制 | 2 | #270、#282（均为 retry 类） |
| codex-tui | 45 | R6 未覆盖超时/快照残余（含 chatwidget guardian 快照 5：#367–#371；active_reconnect 快照 1：#297） |
| codex-v8-poc | 1 | #383 |
| **合计** | **119** | 三途径交叉同值（§1.3） |

### 5.2 按根因族（数字来源逐项标注）

| 根因族 | 数量 | 来源 | 判定/备注 |
|---|---:|---|---|
| R1b guardian history resume checkpoint | 13 | **行数复算**（= 文档陈述 13，worklist §R1b/§4.1） | 完全一致；#68–#83 共 16 行，另 3 行（#71/#76/#79）为 R1 标绿 |
| R2 残余（core managed_network_proxy_decider） | 1 | **行数复算**（= 文档陈述 1，worklist §R2"登记开放"） | 一致；#137，环境证据标签 |
| R9 otel/retry（sqlx SQLite 候选） | 6 | **行数复算** = codex-otel 6 行（#263–#268）；文档陈述"otel/retry 家族 6 项" | 数量一致；但 core retry_after #161（worklist 记其在 r10-verify4 仍败、属 R9 候选）与 rmcp retry 2 行（#270/#282，未归因）是否计入 R9 族，文档未逐行落名，**族边界未行级核实** |
| R8 startup handshake 2s 窗 | 9（候补） | 文档陈述 10（§1 签名组），其中 1 项 guardian ws warmup 属 R1 已修 ⇒ 预期残余 9；**行数复算**：开放 exec-server 相关行恰为 9（4+2+3） | 数量吻合但**映射是假设**：9 行与签名组的逐身份对应需 `t2-signatures-249.tsv`；#222/#225 是否属握手族存疑 |
| R7 残余（exec registration 直接相关） | ≥2 | 文档陈述"registration 17 项"，标绿仅 15；**行数复算**：`direct_registration_*` 2 行（#201/#202）+ relay `registration_retry::shutdown` 1 行（#224）在开放集中 | "17"的构成文档未落名；最可能 = 15 remote + 2 direct。行级关闭度 15/17，见 §3 疑点 |
| R10 冷态 TLS（开放） | 6 | **行数复算**：codex-http-client 开放 6 行；文档陈述：暖态恢复 5、另有 3 个 TLS 目标不计历史恢复、dialer 批残 4 已知 TLS 冷态 | 6 行中应含冷态开放项，但 4 个 dialer 失败用例名旧机日志才有，**6↔4 的包含关系本机不可验证** |
| R2 代理族 TIMEOUT（app-server account 2） | 2 | **行数复算**（= 文档陈述，§4.1"R2 代理族 TIMEOUT 2"） | 一致；#7/#8，标注文件未打 R2 标（保持未归因措辞），仅 worklist 归族 |
| app-server 散项 | 3 | **行数复算**（= 文档陈述，§4.1"散项 3"） | 一致；#128 residency websocket、#129 review detached_delivery、#2 derive_config |
| 快照族 | 8（249 口径） | 文档陈述（§1 签名组 snapshot-mismatch 8）；**行数复算**：tui 45 行中可由名认定的快照行 ≥6（#297、#367–#371） | 8 的精确成员本机不可隔离（需签名 tsv） |
| R6 残余（TUI 未覆盖超时/快照） | 45 | **行数复算**；文档无总数（仅"未覆盖的超时与快照差异仍开放"） | 45 为本文件新立的复算基线 |
| core 未归因残余 | 30 | **行数复算**；文档仅按签名组给 249 口径分布 | realtime 10、unified_exec 11 等族归属需签名 tsv |
| 其他散项 | 3 | **行数复算** | #240 install-context（环境证据）、#270/#282 rmcp retry、#383 v8-poc——共 4 行，其中 #240 与上文环境证据口径重叠一次，按行计 119 不重不漏 |

app-server 切片交叉：worklist §4.1 记 appserver101 残余 18 = R1b 13 + R2 代理 2 + 散项 3；**行数复算** codex-app-server(+::all) 开放 = 1+17 = 18，构成逐类吻合（13/2/2+1）。

---

## 6. 差异与疑点汇总

| # | 级别 | 内容 | 定性 |
|---|---|---|---|
| D1 | 无差异 | 断言 1 全链数字（249/124/115/9/15/119、R1 79/R4 4/R2 22/R10 5/R6 14、R1b 13、249=201+20+28、appserver 残余 18=13+2+3）本机复算全部吻合 | 账面算术成立 |
| D2 | 疑点 | relay #224 在 249 内且仍开放；若 final-verify2 的 relay2 含它，则实际开放应为 118 | 需 final-verify2.log 身份 |
| D3 | 疑点 | worklist R7"registration 17 项"vs 标绿 15 行：差 2 项未落名（最可能 #201/#202 direct_registration） | 行级关闭 15/17 |
| D4 | 口径 | "registration_retry 家族 18/18"（16 矩阵+2 relay，现源相容）与历史 15 行、R7"17 项"是三个不同口径的数字，文档间未显式对表 | 建议旧机对表 |
| D5 | 不可复算 | 512 并集、四日志逐行、exec16/relay2/core1 成员、每个标绿行的 PASS 证据 | 旧机日志 |
| D6 | 不可复算 | 249 行 ↔ still249.tsv 身份等同；R8/R9/快照族的逐身份归属 | 旧机 tsv |

---

## 7. 后续最小动作清单

### 7.1 真正独立复算 119 所需输入（均在旧机，挂载 `/Volumes/soddygo` 后）

| 优先 | 文件（旧机路径） | 用途 |
|---|---|---|
| P0 | `/Volumes/soddygo/git-workspace/codex-tmp/pkg3/still249.tsv` | 249 基数身份，与标注文件 249 行做对称差 |
| P0 | `/Volumes/soddygo/git-workspace/codex-tmp/pkg3-r2/final-verify2.log` | 断言 2 构成（exec16/relay2/core1 逐行）+ D2 |
| P0 | `/Volumes/soddygo/git-workspace/codex-tmp/pkg3-r2/{dialer-fix-verify,regression-run,cache-regression}.log` | 断言 3 的 512 并集复算 |
| P1 | `/Volumes/soddygo/git-workspace/codex-tmp/pkg3/t2-signatures-249.tsv`（及 `t2-signatures-groups.md`） | R8/R9/快照/exec-reg 签名族逐身份归属（D4、§5.2 各"假设"项） |
| P1 | 修复批日志：`appserver101/` 批目录、`final2/attestation.log`、`r4-verify*`/`r4-unit2*`、`r2-final3.log`、`r10-verify4.log`、`r6-verify3.log`（codex-tmp 下，具体子目录需旧机定位） | 139 个标绿行逐行 PASS 证据 |
| P2 | `/Volumes/soddygo/git-workspace/codex-tmp/ws3-rerun-t2.log` | "10-07 复验"列 ground truth 溯源 |
| P2 | `extract_signatures.py`、`run_repro.sh`、`run_verify.sh`（pkg3 内） | 复用既有口径，避免重写解析器 |

联结规程（一次性脚本即可）：以 `(binary, test_name)` 为键（注意 `codex-app-server` 与 `codex-app-server::all` 为不同身份；nextest 结果行需去 Summary 重复，按待复核清单口径用"结果序号"），`still249` − ∑(各修复批 PASS 身份) 应恰为 119，并与标注文件 §5.1 的 119 行做对称差输出；再单独打印 final-verify2 的 relay2 两个身份裁决 D2。

### 7.2 本机已完成的替代核验（本文件 §1–§5）

1. 标注文件 383 行全量解析：四态、标签 139 行、115/9 划分、119 三途径同值——全部吻合；
2. `historical-383-state-check.json` 四态交叉吻合；
3. 四日志分桶加和与 512 的净重合反推（~221，与 143+18+59 相容）合洽；
4. 现源 registration_retry 家族规模（16 矩阵 + 2 relay = 18）与"18/18"相容；
5. 本机 10-10 批（`claude-p1-fixes-2026-10-10.md`）已跑 codex-exec-server 全量 616/616（registration 家族 hermetic 夹具后 18/18）——属测试执行佐证而非账目身份联结，不替代 7.1。

**总判定：119 是账面自洽数，本机行级复算成立；身份级（对 still249.tsv 与全部修复日志）确认仍需旧机输入，维持待复核清单第 10 项"不能写独立复算确认"的表述，仅在"标注文件内部算术"这一层将其升级为"已复算吻合"。**
