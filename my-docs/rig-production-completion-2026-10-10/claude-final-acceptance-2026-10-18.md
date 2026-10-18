# 最终测试验收（2026-10-18，Claude 轮）

> 判决：**核心接入开发收口。** Linux/容器 + ACP 自定义模型（NUWAX 四环境变量 → rig 三协议）
> 主路径可以日常使用。本轮修复了 CI 首轮全量跑出的 4 个真实跨平台问题；剩余项均不阻断核心目标。

## 0. 仓库终态

| 仓 | 分支 | 起点本轮推进到 | 本轮提交 | 状态 |
|---|---|---|---|---|
| Codex | codex/rig-044-20261014（worktree；== test@60956fb10 起点） | `60956fb10 → <本文档提交>` | `a0b28cba2`（macOS 测试构建 libc）、`659770c14`（guardian 预热合同）、`35658f7be`（bounded 停机可恢复）、`1f7904544`（astra 快照）+ 本文档 | clean，未 push |
| ACP | dev | `854f2c3 → 0d4c12d` | `7bfb58a`（连续工作流 live 场景）、`0d4c12d`（纯 cyan 夹具） | clean（vendor/ untracked 照旧），未 push |

主 checkout 占用 `test` 分支（同为 `60956fb10`），本轮在 worktree 分支提交；推送后 fast-forward：
`git -C /Volumes/soddy/git_workspace/fork-nuwax-codex fetch && git -C … checkout test && git -C … merge --ff-only codex/rig-044-20261014`

## 1. 起点核对

- 两仓与所述 SHA 一致、干净（ACP vendor/ 未跟踪保留）。
- CI run #33（39081573074，test@60956fb10，00:07Z 触发）实测：bazel bridge ✔；
  macos ✘（编译错误）、ubuntu ✘（13 个测试）、windows ✘（约 20 个）——逐家族归因见 §3。

## 2. 本轮验证总表（源码/二进制对应）

Linux 实机 swufe@192.168.32.163。**最终源码（含全部修复）**重建的 release 二进制：
codex `e51de4385cea78fac4b325d29bfa139a50b9e4351a95efae6f3b607316cb19fb`、
codex-exec `87046f35ebc116a2be390980c273342b4d871131207b386c93c673e1496de4b6`
（`cargo build --release -p codex-cli -p codex-exec --features codex-core/rust-rig`，工具链 1.95.0-x86_64）。
早段验证（§4 之前）使用同源码 60956fb10 构建：codex `c0856255…e6e0`、codex-exec `2a95c03e…1fba`。

| 门 | 二进制 | 结果 |
|---|---|---|
| ACP 全量 Vitest | c0856255 | 839 通过 / 32 既有 skip / 0 失败 |
| ACP 四 TS 检查域 | c0856255 | 通过 |
| test:acp-validation / test:live-budget | c0856255 | 37/37、12/12 |
| 容器三 wire 6 场景 ×3 | c0856255 与 **e51de4385** 各跑一遍 | 各 18/18（bootstrap/env-passthrough/typed-image/cancel/compaction/cold-resume） |
| 容器三 wire 18 压力 ×3 | 同上 | 各 54/54（verifiedImage 4/wire、probeFailures 0、mockOpenSocketsAfter 0） |
| 容器双沙箱 exec_command 探针 | c0856255 | read-only 拒写(NOT-WRITTEN，turn rc=0)；FullAccess marker=shell-sandbox-ok |
| 容器 --network none 辅助外呼失败探针 | c0856255 | turn 亚秒完成、pong 正确、无孤儿进程 |
| restored-usage（warm load/冷恢复用量） | c0856255 与 **e51de4385**（含对抗环境） | 均 used=5/size=258400 三点一致，failures=[] |
| hostile 宿主环境（NUWAX_*=/MODEL_PROVIDER/CODEX_CONFIG 指向伪端点） | 同上 | 仍全绿（离线驱动隔离生效） |
| 真实 Token Plan（live-smallbatch 12+1 场景） | c0856255 | 详见 §4；上游 attempts 96/96 用尽（产品修复触及的 guardian 预热与 bounded 停机路径在 live 配置中未启用，且已由上述最终二进制的容器门+restored-usage 复验） |
| 凭据扫描（两 live 根 + 调试日志） | — | 89606 文件 0 命中（原文+base64）；.env.live 已删 |

修复后二进制对受影响路径的复验：容器门（SHA 强校验）与 restored-usage 均以 **e51de4385** 重跑全绿。

## 3. CI 失败家族归因与修复（同根因合并）

### 3.1 已修复（含红→绿证据）

| 家族 | 平台 | 定性 | 修复 | 红→绿 |
|---|---|---|---|---|
| core-plugins 测试目标编译失败（E0433 `libc`） | macOS | `a55cdd0fc` 把 libc 放在 Linux-only target 依赖，unix 门控的 startup-sync 测试在 macOS 引用 libc | `a0b28cba2`：加 `[target.'cfg(unix)'.dev-dependencies] libc` | 红：CI 编译 3 错；绿：mac 编译过 + 该测试 PASS |
| guardian `model_guardian_policy_controls_cua`×3 + `resumed_thread_does_not_wait_for_guardian_websocket_warmup`（30s 握手超时） | ubuntu+windows | **上轮 a6d34d279 产品缺陷**：Luna 池对 `supports_websockets=false` 完全跳过预热；上游合同是"guardian 通道独立于该开关，仍要后台握手、lease 即时 HTTP 回退" | `659770c14` 撤回跳过；Codex 轮反向回归测试改写为真实不变量（预热有界 + 两次分类走 HTTP） | 红：盒上 4×30.5s 超时；绿：10/10（4.6s）。pool_pressure 保持绿（其真修是测试配置 snapshot+lag32） |
| `resume_agent_from_rollout_*`×3（InvalidRequest "thread manager is shutting down"） | ubuntu+windows | **上轮 b45fea55e 产品缺陷**：把 bounded 停机做成永久栅栏；上游合同是可复用停机、之后可 resume | `35658f7be` 栅栏移回 `wait_for_shutdown_bound_tasks_bounded`（退出路径）；入口门+注册复查保留 | 红：盒上 3×1s 确定性失败；绿：6/6。b45fea55e 自身栅栏回归测试 3/3 仍绿 |
| `astra_plugin_policy_limits_visible_skills`（insta 快照） | ubuntu+windows | **合并解析错误**：481a3f485 把快照留在 fork 侧（含已不存在的 ego-browser 等夹具） | `1f7904544` 恢复 cb03f28a4 的上游快照 | 红：分支红/上游绿同机对照；绿：分支 PASS |

### 3.2 环境性 / 继承性（登记，不计产品缺陷）

| 家族 | 平台 | 定性依据 |
|---|---|---|
| `permission_request_hook_allows_exec_command...` | ubuntu CI | 盒上同源码 PASS（CI-only） |
| `host_roots::repo_ancestry...` | ubuntu CI | 上游树与分支树在盒上均 PASS（CI-only） |
| `shell_snapshot`（insta ""/blocked_descriptor） | ubuntu CI | **上游 cb03f28a4 在盒上同样失败**（继承/环境） |
| `root_metadata_symlinks...` | ubuntu CI | CI runner 无系统 bwrap 且无 bundled 资源（探针日志为证） |
| unified_exec legacy ×4、cur detect ×4、uds ACL ×2、approvals_selection_popup_snapshot、conpty ×2、profile_cleanup、external_agent_config、subagent_notifications、mcp_image_tool_payload_round_trips、memory_exit_contract、astra 之外的 insta 若干 | windows CI | Windows 专属兼容尾项（上轮 CI 账本已有真机基线差分记录的延续）；不影响 Linux 主路径 |

### 3.3 开放项（优先级序；均不阻断核心目标）

1. **`uploaded_original_image_history_compacts`（guardian_context_budget）= fork 真回归**：
   上游 cb03f28a4 盒上 PASS / 分支 FAIL（CI 双平台 + 盒确定性复现，`compact_requests.len()` 0≠1）。
   初步机理：fork 的审查者图片改为附件引用传输（4a0dd66d5 attachment preparation 一系），
   审查输入不再因内联原图超窗 → 不触发压缩服务；与上游 b974893c9 的"内联原图超预算必须压缩"
   语义分歧。需裁决：审查者请求是否必须内联 original-detail 图，或上游测试期望需按附件语义演进。
   不得单方面改断言，故登记。
2. CI 全绿仍差：macos 分片（前轮账本建议）、windows 尾项、§3.2 环境项。
3. 上轮遗留：elevated runner pipe 15s 握手、guardian-v2 组内并发时间敏感。

## 4. 真实模型最终验收（预算 96/96 用尽，账本见 live-run*/request-ledger.jsonl）

- run1（12 场景，21 attempts）：11 过 / 1 败——`vision-direct` 2/3（image2 答 turquoise）。
  定性：**夹具歧义**（"cyan" 实为 RGB(30,200,210) 本身就是 turquoise 系）；修复 = 纯色 (0,255,255)
  （断言不放宽、提示词不含色词的既有合同保留）。新样本重跑 7 attempts 全对（3/3）。
- 补跑段（--resume）：`acp-smoke` 两连败（post-compact 答 "live-ok" 而非 "post-compact-ok"，
  run1 同二进制曾过；mock 侧压缩续话 3 wire 确定性全绿）→ 定性为 GLM-Flash 对压缩后精确串
  指令跟随的**厂商能力波动**，非适配缺陷；首败日志保留。压缩/续话机制由 mock 门与 workflow
  行为学步骤覆盖。
- **acp-workflow（新增连续工作流场景，本轮主验证）**：中文对话→文件写/读→shell→MCP 截图
  （真值账本 + 无 base64 文本泄漏）→流中取消→续话 六步实跑通过；/compact 起被共享预算 402
  拒绝（96/96 用尽）——错误以文本清晰上抛、无挂起（失败形态本身为证）。压缩/冷恢复/孤儿进程
  链由容器门（S5/S6×3 wire + 压力 cold=2/wire）与 restored-usage 以**最终二进制**复验。
- 网关合同自证：预算不可重置（75→67 拒绝）、EADDRINUSE 无、denied=2 全部如实计数。

## 5. 用户启动方式（不变，见 quickstart）

```bash
export NUWAX_BASE_URL=… NUWAX_WIRE_API=chat|responses|anthropic NUWAX_MODEL=… NUWAX_API_KEY=…
export CODEX_PATH=/path/to/codex-rs/target/release/codex   # 本轮最终 SHA 见 §2，勿用 npm 附带旧版
cd /Volumes/soddy/git_workspace/fork-codex-acp-ts && node --import tsx src/index.ts
```

四变量成组提供；显式 CLI/provider 覆盖优先于环境变量。调试日志（APP_SERVER_LOGS）经本轮
89606 文件扫描确认不含凭据。

## 6. 执行轨迹备注

- 镜像事故（如实记录）：一次 rsync 以 codex-rs 为 cwd 执行，把 crate 平铺到 /tmp/codex-fa 顶层，
  与旧布局混合；期间 cargo 回退到顶层 workspace（恰好含当期修复，F1/F2 验证有效），已恢复
  正确布局并从最终源码重建。期间发现 astra 快照拷贝路径错误并纠正后复验。
- mac 本地因 v8 ptrcomp_sandbox 预编译资产缺失无法构建 core 测试目标；盒上以
  openai/codex release 的配对资产（校验和核对通过）补齐后复跑。
