# claude-rig-full-validation.md — 桥相关全量验证记录（2026-09-29）

> 来源：三轮缺陷复核（`fork-defect-review-2026-09-29*.md`、`fork-fix-plan-verification-2026-09-29.md`）
> 的 Batch 3 关闭前置。统一隔离 `CARGO_TARGET_DIR=/tmp/codex-nuwax-home-target`、
> 仓库 `just test`（nextest）。本机 macOS aarch64。

## 1. 桥相关五包全量（含本轮全部新测试）

命令：`just test -p codex-rust-rig-bridge -p codex-api -p codex-config
-p codex-models-manager -p codex-utils-cli --offline --retries 0`

结果：**750 项：749 通过 / 1 超时 / 1 跳过**。

唯一失败 `responses_time_out_when_server_never_sends_headers`（既有负载竞态：
测试支撑代码在客户端按设计超时断开时断言"必须读到完整请求"，读 0 字节即 panic；
隔离复跑恒通过——2026-09-28 起多轮记录在案，与本轮改动无关）。
`/tmp/val-bridge.log`。

## 2. core（rust-rig 特性集）

命令：`just test -p codex-core --offline --retries 0 --features rust-rig`
（含本轮新增的 `bridge_chat_compact_recovers_from_context_window_rejection`）

结果：**4712 项：4479 通过 / 225 失败 / 8 超时 / 27 跳过**（2940s，
`/tmp/val-core-rigrig.log`）。

失败聚类与 2026-09-28 全量基线**完全一致**（`my-docs/nuwax-home/tasks.md` T2.3
已逐类归因）：code_mode 簇 146+、realtime_conversation 22、remote_env 14、
scenarios 12、guardian_* 20、hooks/mcp_optional/unified_exec/client 等——均为
`codex-code-mode-host` 本机构建缺失（rusty_v8 归档 404）或其负载竞态；
`suite::compact` 全部通过（含新回归测试）。新增的桥线端到端测试在 `suite::
rig_responses_bridge` 下**通过**。

## 3. 本轮新测试证据（全部通过）

| 测试 | 层 | 结果 |
|---|---|---|
| `bridge_chat_compact_recovers_from_context_window_rejection`（新） | core 桥线端到端 | ✅ 13.3s；无 rust-rig feature 时 0 匹配跳过 ✅ |
| `anthropic_replay_drops_web_search_call_history`（新） | 桥 wire | ✅ 断言回放不含 server_tool_use/结果块/载荷泄漏 |
| `glm_websearch_anthropic_rig`（新） | live（真实 GLM） | ✅ 双轮搜索完成——网关接受丢弃历史的回放 |
| `auto_compact_recovers_from_http_context_window_rejection_by_trimming` | core native（v0.18.10 已有） | ✅ |

## 4. 已知环境限制（非回归）

- `codex-code-mode-host` 依赖的 rusty_v8 `ptrcomp_sandbox` aarch64-darwin 预编译
  归档 GitHub 404，本机无法构建 → core 全量中依赖 code-mode 的用例在本机必然失败
  （2026-09-28 全量基线已逐类归因，属构建环境缺失）。本记录以 rust-rig 特性集 +
  桥五包覆盖替代"完整 workspace"，CI（Linux）覆盖其余。
- `responses_time_out_when_server_never_sends_headers` 负载竞态（见 §1）。

## 5. 结论

三轮复核"桥无正确性缺陷"的结论现在有：桥五包 749/750（唯一失败为既有竞态）、
core rust-rig 集（§2）、四项新测试（含真实网关 live）背书。
