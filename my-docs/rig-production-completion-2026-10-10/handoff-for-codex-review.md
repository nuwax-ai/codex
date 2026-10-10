# Codex 独立复审交接书（2026-10-10/11，账目修正版）

本次独立复审范围为 `303ad8960..070098774`，提交基线为 `070098774`。原交接使用的 `877b02758..5637a6566` 是较早范围。当前工作区有独立复审修正，不能沿用“工作区干净”或把旧收据当作修正后的验收。证据入口为 `results.md`（批A–S及修正状态表）和以下账目文件；本轮没有运行新测试或dispatch CI。

## 历史实现收据与未验边界

| 项 | 历史提交/收据 | 本次复审后的证据边界 |
|---|---|---|
| T03/T04 native-root键/缓存 | `489c43c85`；http-client本机作用域收据 | Windows真实A→B未完成；本轮修正未验 |
| T05/T06 WS脱敏/HTTPS代理 | `f3f86e926`；24/24 | 需要保留上层错误出口与平台覆盖边界；本轮修正未验 |
| T07 loader/matcher/ResolvedDefault | `0b2ecbbbc`/`5ac058769`/`5753a3505`；loader/向量/双轨收据 | opt-in存在，默认迁移未裁决；CFRunLoop因果及默认生产路径未关闭；本轮修正未验 |
| T11 R1b | `9d307f72d`；旧13身份16/16 | 旧收据不能验收本轮授权/投影修正 |
| T12 Luna终局 | `081b6b0b6`；六个新增fixture | 有请求/终局断言，不扩大为全部公共入口已验 |
| T16 D2 | `5667058bc`；api/bridge历史收据 | Chat迟到usage、host key及错误侧usage闭环开放 |
| T17 reducer | `69c0ea00e`；直接SessionState单测/既有usage家族 | 重复ID/跨turn/恢复ledger后重报缺公共Responses→Core→rollout集成证明 |
| T18协议/预算地基 | `d51d5ef4d`/`6b45debc4`；旧cap/预算测试 | 旧cap断言允许final AgentMessage为None，未证明durable partial。普通Done尝试已在本轮撤回，零普通完成合同恢复但未测试；实际发射/持久化/读侧/resume仍开放 |

OTLP fixture从ReqwestDefault换成RespectSystemProxy后的pair通过只证明替代策略。生产默认factory仍使用ReqwestDefault，不能称默认生产路径已根治。

## CI / T02 / T38

已有run **38065399719** 的测试SHA为 **`e02dc3771710c0a4e6fd8b3dd125715b08c39b55`**，event=push。四job都进入build/test，Bazel gate成功，三个nextest车道失败。T02只完成启动/构建入口验收；T38完整门禁未达成。

三车道执行的是workflow排除后的集合，nextest实际使用profile local及retries=0；150分钟是job总预算，单测试deadline未随之扩大。

| 平台 | 执行 | PASS | Summary FAIL桶 | TIMEOUT | SKIP |
|---|---:|---:|---:|---:|---:|
| Ubuntu | 21561 | 21151 | 408 | 2 | 47 |
| macOS | 21514 | 21292 | 217 | 5 | 68 |
| Windows | 20400 | 20205 | 176 | 19 | 138 |

Summary FAIL桶含Ubuntu的1个SIGABRT、Windows的1个ABORT和1个FAIL+LEAK。`ci-run6-failure-ledger.tsv`完整登记827个非PASS终态：Ubuntu410、macOS222、Windows195；不包含stdout或凭据。

原“Ubuntu389/406都是sandbox、macOS217都是并发固有边界、剩余失败已终局分类”结论撤回：

- 明确缺code-mode-host的输出块至少Ubuntu60/macOS60/Windows36。排除host crate后，依赖该二进制的app-server/Core测试仍被选择，不能归为并发失败。
- Ubuntu105个失败块明确出现bwrap loopback RTM_NEWADDR EPERM。这证明该运行操作被拒绝；未证明389项同因、唯一原因是NET_ADMIN或必须换runner。其他身份保留unclassified。
- ledger旧23个BOUNDARY身份在此run macOS全部PASS。旧本机窗口保留；不能用它们解释217个CI失败。
- Ubuntu/Windows均有 `codex-core guardian::tests::guardian_ephemeral_retry_preserves_parallel_trunk_and_fork_history` abort；Windows明确是0xc00000fd stack overflow。旧macOS选集“8MiB下0 abort”不能外推跨平台已关闭。

## 账目口径

`current-test-ledger.tsv`保留126条记录及旧窗口状态：98 PASS / 3 FIXED-linux / 23 BOUNDARY / 2 FAIL-linux。**这是记录行数，不是126个实际身份。**

`canonical-ledger-map.tsv`逐行标明119 historical-identity、3 alias、2 summary-not-identity、2 new-identity；去掉别名和摘要后有121个完整canonical身份。两个家族摘要未展开，不推算其新增身份数。

119历史tuple登记齐全，其旧窗口状态为95 PASS / 1 FIXED-linux / 23 BOUNDARY。23项另有 `ci_run6_macos_state=PASS`，保留旧窗口且明确追加来源；不能将全部126行总计称为119历史终态。TSV占位 `-` 表示没有在该列登记结果/身份，不表示SKIP或通过。

三组别名：review_start少 `suite::` 前缀、unavailable_commands旧嵌套路径、thread_title旧模块路径。它们通过map关联到实际身份，不再额外计数。TLS fallback五项和analytics两项的行仅是家族摘要。

## 下一步及验证边界

1. 补齐V8/host配对工件依赖，查明Linux namespace拒绝配置和跨平台stack overflow，再按CI完整身份逐项归因；未归因项保持开放。
2. 补T17重复快照/恢复ledger和T18失败partial的公共读写/resume集成证明；不把普通Done或类型存在当作durable完成。
3. 完成T07/T08默认生产路径、冷态/并发原预算验证，再推进原计划的D6、owner、来源/历史、原生/异OS、live和发布准备。

本机为macOS arm64；Windows/Linux本地原生、旧机工件联结、live付费和发布验收仍缺。CI已经跑过编译/测试，但三车道均红，且不覆盖本轮未提交修正。

只读证据：`gh run view 38065399719 --repo nuwax-ai/codex --json headSha,conclusion,event,jobs,url`；已有 `/tmp/run6logs/1_nextest (ubuntu-latest).txt`、`2_nextest (macos-latest).txt`、`3_nextest (windows).txt`。原始日志不进入仓库；仓库CI ledger只存身份/状态/类别。本轮没有新测试、dispatch、commit或发布。
