# Codex 独立复审交接书（2026-10-10/11）

本文件是给下一任审查者（Codex）的完整交接。审查对象：origin/test `877b02758..5637a6566`（53 个提交，已全部推送，工作区干净）。逐项证据在 `results.md`（批 A–S）与 `current-test-ledger.tsv`（126 行）。

## 审查范围与主张（按包）

### 包3 稳定性（已完成项）

| 主张 | 提交 | 声明的证据 | 建议复核点 |
|---|---|---|---|
| T03+T04 native-root 键/缓存 | `489c43c85` | 12 单测+3 真实握手，http-client 141/141×3 | 键语义 vs rustls-native-certs 0.8.3 `CertPaths::from_env`（var_os+split_paths）；测试实例隔离是否真不碰全局 |
| T05+T06 WS 脱敏+env https 代理 | `f3f86e926` | 24/24；三协议 CONNECT 矩阵+SOCKS5+脱敏子进程 | 脱敏是否覆盖所有出口（重点：`responses_websocket.rs` 的错误路径）；`tungstenite_env_https_proxy` 的恢复顺序是否严格复刻 SDK `get_first_env` |
| T07-1 专职线程 loader | `0b2ecbbbc` | 7/7 loader 单测 | unsafe Send/Sync 论证（CFDictionary 不可变拷贝）；毒化恢复策略是否会在用户进程 panic |
| T07-2 matcher 向量 | `5ac058769` | 15 向量（对齐 hyper-util 自身测试） | 向量与 hyper-util 0.1.20 源码逐条对照（重点：空 scheme 变量=未设语义、CGI、NO_PROXY IP/CIDR） |
| T07-3 ResolvedDefault opt-in | `23d8d3e6f` 前身 `5ac058769` | 8 路由向量+11 案例双轨 wire 等价 | 双轨的参考 client 是否真是未优化默认（reqwest::Client::builder().build()）；SOCKS 委托是否漏了 `socks4a`；**默认迁移未做（留裁决）** |
| T11 R1b | `9d307f72d` | 13 身份 16/16 | Compaction scope 级契约放宽的安全性：Reasoning/WebSearch 仍严格？跨 kind 轮换仍拒？grant 是否真不落 rollout |
| T12 Luna 终局 | `081b6b0b6` | 6 新用例+矩阵映射 | 矩阵映射表（agent 报告）与实际覆盖是否吻合 |
| T16 D2 | `5667058bc` | api 214/214、bridge 618/618 | Chat 迟到 usage chunk（V-D2-1）仍未测；Anthropic 零值保留是否真在 wire 层验证 |
| T17 ledger | `69c0ea00e` | 4 单测+39 家族 -j2 绿 | 替换语义的跨 turn 重放分支（net 进新 turn）是否符合 D4 决策文档 |
| T18 两段 | `d51d5ef4d`/`6b45debc4` | bridge 252/252、cap 7/7、cap_partial 7/7 | **已登记未竟**：turn-error 发射点+usage 携带（stash 尝试已回退，原因写明）；集成测试里 CapPartial 事件断言已撤回（发射未落地）——不要把 CapPartial 类型的存在当成功能已完成 |

### CI / T02（完成：DoD 达成）

- 修复链 `a2f8e8d84`→`eaecf3805`→`23d8d3e6f`→`e02dc3771`，6 轮真实运行（38045982072…38065399719）。
- bazel gate 连续三次全绿；三 nextest 车道完整跑 21.5k 测试。
- **剩余失败终局定性**（results.md 批 S）：ubuntu 389/406=GitHub runner 禁嵌套网络命名空间（`bwrap: loopback: RTM_NEWADDR EPERM`，装包解决不了，需自托管 runner=T32 环境）；macOS 217=并发固有边界（本地安静窗 5633/5642）。**未加 skip 求绿**。

## 已知边界（审查时不要误判为缺陷）

1. 119 账本终态：PASS 98 / FIXED-linux 3 / BOUNDARY 23（串行绿/并发败，负载 7–9 复验仍 34 败，定性并发固有）/ FAIL-linux 2（V8 配对工件+慢机窗口）。0 个无解释失败。
2. BOUNDARY 的 23 个身份不因"CI 上也失败"而改变定性——CI 2-vCPU 放大同机制。
3. V8：pin 150.4.0 的 `ptrcomp_sandbox` 预编译归档从未发布（上游 2e32d9589 开 sandbox）；本地能构建纯靠 build script 缓存。车道已按缺工件模式排除 code-mode-runtime/host/v8-poc，V8 pin 按禁令未动。
4. tui 45 身份的关闭依赖两件事：37 个 run3 全量 + 8 个 elapsed-frame 修复（`94ab4f533`，零 accepted 快照改动）。
5. R11 栈溢出为 pre-merge 潜伏（非回归），`just test` 8MiB 下 0 abort；直跑 cargo 必带 `RUST_MIN_STACK=8388608`。
6. 一个 pre-existing `.snap.new`（10-01 的 astra_kickoff_remote_compaction_windows）在 .gitignore 覆盖内，非本批产物。

## 验证环境声明

- 本机 macOS（arm64），同期其他用户负载 7–152 剧烈波动；所有"全绿"都注明了窗口与并发度，负载敏感失败均给出隔离复验。
- 未做：push 以外的发布、live 付费请求、Windows/Linux 本地原生验证（CI 已代跑编译+测试）、旧机工件联结（需旧机导出）。
- 本批所有 scoped 测试在最终 fix/fmt 之后未重跑（AGENTS 流程）；fmt/`git diff --check` 每批已过。

## 建议的审查入口

1. `git log --oneline 877b02758..5637a6566` 通读 53 个提交信息（每个含证据行）。
2. results.md 批 A–S 与 ledger 逐条抽查 5–10 行对源码。
3. 重点攻击面：`model_output_projection.rs` 的 scope 级 Compaction 契约（T11 核心语义变更）、`chain_carries_tls_error`（23d8d3e6f）、`default_proxy_matcher.rs` 与 hyper-util 的逐字段等价、CI workflow 的排除清单是否与注释理由一致。
