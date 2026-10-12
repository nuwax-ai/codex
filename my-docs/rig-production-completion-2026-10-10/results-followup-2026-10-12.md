# 结果：Claude 跨电脑后续批次（2026-10-12）

工作分支 `test`。起始 HEAD `356052ee2`（与 `origin/test` 一致，checkpoint 确认包含，无需 fast-forward）。工作树起始干净；本轮全部改动为本地小批提交，**未 push**（远程 push/手动 dispatch 按交接需明确授权）。

本轮提交（按序）：

| 提交 | 内容 |
|---|---|
| `c569e2b38` | D1-d cap-partial 身份/序列化总界（实现+测试，508 行） |
| `732ec0d77` | Spec/裁决文档登记 D1-d |
| `ccee53c48` | ToolError Codex 装箱 + record_token_usage typed 参数 |
| `3f9376bf6` | V-D2-1 Chat 迟到 usage/primary chunk wire 测试 |
| `1c245ec56` | 裁决文档关闭 V-D2-1 |
| `dc34e0814` | 负值/溢出 usage 计数语义单测 |
| `edde3a18a` | CI：Linux 放开 unprivileged userns |
| `a0765f355` | CI：Linux 释放预装工具链磁盘 |
| `d1e1374f1` | tls fallback 分类器按链内标记判定（修 ubuntu 5 失败） |
| `e85c66883` | Responses 路由 cap-usage presence 矩阵（缺失/部分/显式零） |
| `9a7cd984b` | Anthropic 路由 cap-usage 矩阵（start/delta 累积、显式零穿透） |

## 1. 当前 SHA 的 CI（run `38195171566`，HEAD `356052ee2`）

四 job：`bazel bridge gate (ubuntu)` **success**（该门首次全绿，V8 配对工件/setup 修复生效）；`nextest (ubuntu-latest)` **failure**（完整日志已取回并逐行归因，见下）；windows/macos nextest 本文档写作时仍在运行，完成后补记。

### ubuntu 失败归因（21757 测试身份，197 失败）

| 簇 | 身份数 | 根因（日志实证） | 处置 |
|---|---|---|---|
| codex-core 套件（approvals/network_approval/unified_exec/workspace_roots/request_permissions/guardian 等） | 89 | 沙箱路径依赖 bwrap，`bwrap: setting up uid map: Permission denied` | CI 修复 `edde3a18a` |
| exec-server file_system_unix / capability_discovery / file_stream | 52 | 同上 | 同上 |
| codex-app-server（command_exec/analytics/executor_skills 等） | 24 | 日志含 `Codex's Linux sandbox uses bubblewrap and needs access to create user namespaces` | 同上 |
| codex-linux-sandbox landlock 套件 | 22 | 同上 | 同上 |
| codex-cli sandbox_* | 3 | 同上 | 同上 |
| codex-http-client tls_fallback_tests | 5 | ubuntu 上协议版本告警以 `kind: Request`（经 hyper_util）浮现，`is_connect()` 门槛拦截了合法 rustls 回退（run6 账目同一形态） | 产品修复 `d1e1374f1` |
| codex-cli app_server_daemon | 2 | `No space left on device`（runner 磁盘耗尽） | CI 修复 `a0765f355` |

三项修复均待下一次 push 自动触发的 CI 验证；本轮不将其记为已验证。190 个沙箱身份的最终裁决仍依赖修复后 CI 的真实结果，不因根因明确而预先记绿。

### tls 修复的边界

`d1e1374f1` 依据：run6 账目与本次日志一致显示同一告警在 ubuntu 为 `kind: Request`、macOS 本地为 Connect；分类器改为按源链内平台标记判定（超时仍不回退、证书类仍主动排除）。本机 macOS `codex-http-client` 182/182 通过；Linux 确认待 CI。

## 2. D1-d：cap-partial 身份与序列化总界（第二批优先项）

语义先落 Spec（`cap-decisions-2026-10-10.md` D1-d 行 + `spec.md` S3 增补），再实现（`c569e2b38`）：

- 每身份串 ≤256 B：capture 的 item/call/summary 键与 usage ledger 的 provider response ID。超限整串丢弃、绝不截断成存储键（同前缀不同 ID 不可能别名）。
- 事件身份元数据总和 ≤8 KiB；事件 JSON 序列化 ≤128 KiB（serde_json 转义逐段增量记账 + 4 KiB 结构包络，包络按 32 片段最坏结构开销 ~3.6 KiB 定值）。
- 可观测性：`CapBudgetKind` 新增 `IdentityBytes`/`MetadataBytes`/`EventBytes`；usage ledger 超限退化 keyless-additive，warn 只记长度不记内容。
- 裁切都在借用切片上进行；summary 组合键先验证 item id ≤256 才 format；`finish()` 对 host 键防御性检查（超限输出空串不截断）。
- 工具 identity、授权及 wire 事件（RawResponseCompleted 等）上的 provider id 不变。

测试（`cap_partial_tests.rs` 19/19，其中 9 个新增）：巨型 ID、重复巨型 ID 只记一次、同前缀不同巨型 ID 无别名、恰好 256 B 多字节 ID 保留、summary 巨型 item id、组合键超限退化为整 item 捕获、元数据总限关闭、控制字符 6 倍转义使序列化预算先于 raw 预算收紧（真实 `serde_json::to_string` 断言 ≤128 KiB）、escaped 记账与 serde_json 逐字节等价、host 键超限输出空串。ledger 侧（`state/session_tests.rs` 15/15，2 个新增）：超限 provider ID 退化 keyless-additive 且空串存储、同前缀巨型 ID 均不入账、host key join 不受 provider ID 长度影响、预算内 ID 保留 legacy replace 语义。

首次失败记录：无（本批一次通过）。clippy `--fix` 自动应用 1 处等价方法引用改写（map_or 闭包→`String::len`），其后未重跑测试，由编译/lint 验证。

## 3. 两条结构告警（上轮复审遗留）

`ccee53c48`：

- `ToolError::Codex(CodexErr)` 240 B → `Box<CodexErr>`。错误类别/所有权/匹配语义不变（全部匹配点自动解引用，构造点 `Box::new` 包裹；`unix_escalation` 的 `err.into()` 依赖 `Box<T>→T` 仍成立）。约 20 个使用点逐一核对。
- `record_token_usage` 8 参数 → `TokenUsageReport<'a>` 单一 typed 值；测试侧 `LedgerFixture` 一行式调用。

验证：`cargo clippy --tests` 两条告警 0 出现、无新告警。相关面：state::session 15/15、token_usage_rollout 3/3、approvals/network_approval/unified_exec/guardian/escalation 选集 442/443、tools::events/mcp_tool_call/write_stdin/process_manager/orchestrator 选集 156 通过 + 16 失败（全部为下述既有环境簇）。**不将装箱宣称为跨平台栈修复**：R11/T10 仍开放，待原生平台复现。

### 本机既有失败归因（非本轮引入，基线复现）

在基线 `356052ee2`（临时 worktree）复跑确认同样失败：

- `suite::code_mode::code_mode_can_apply_patch_via_nested_tool`：断言 1≠2（`code_mode.rs:6766`），本地 macOS 稳定复现，与本轮改动无关；属 code_mode 历史族，待独立归因。
- rmcp/truncation/code_mode 多数集成身份：`could not locate binary "test_stdio_server"`——`-p codex-core` 选择性运行不构建其它 crate 的 bin，属本地运行方式限制，非产品失败；workspace 完整运行（CI）不受影响。

## 4. 三协议 usage 闭环补充

- **V-D2-1 关闭**（`3f9376bf6`）：`chat_cap_usage_only_chunk_after_length_finish_is_retained` 证明 finish 后、`[DONE]` 前的 usage-only chunk 完整进入 cap 终局报告（6/64/0/70 presence、显式零保留、未报缺席、恒 Incomplete）；`chat_late_primary_chunk_after_length_finish_is_dropped` 证明首终局后迟到 primary-choice chunk 丢弃不进可见 partial。两者均在非零 retry 预算下断言单次 POST。rig_output_cap 套件最终 22/22。
- **负值/溢出**（`dc34e0814`）：Chat 负值/溢出计数在该 usage 对象内缺席（同对象合法计数仍上报）；Anthropic 负值 delta 保留上一有效值。桥 crate 261/261。
- **Responses 矩阵**（`e85c66883`）：终局无 usage → 不落账不造假报告；部分计数 → ledger 缺席计零（设计如此）、诊断保 presence 且 Incomplete；显式零 → ledger 与诊断都保留为已报计数。三例首跑即过。
- **Anthropic 矩阵**（`9a7cd984b`）：start(input)+delta(output) 累积进 ledger 与诊断（total 恒缺席、Incomplete）；终局 delta 显式零 output 穿透 rig 0.42 零→缺失归一化，端到端保留 Some(0)。
- **Anthropic 首败记录（保留）**：构造矩阵时两次首败——rig 的 Anthropic 反序列化要求 message_start 与终局 message_delta 都带 usage（start 还要求 output_tokens 字段）；缺任一则 JsonError → 流按可重试截断失败。这不是缺陷而是 rig 的严格性与既有截断分类的正确行为；相应非法形状由既有 truncation 测试覆盖，矩阵测试改为合法形状。

## 5. 验证记录汇总（macOS arm64，本机）

- `just test -p codex-protocol --locked --offline`：367/367。
- `just test -p codex-core cap_partial / 'state::session' / suite::token_usage_rollout / suite::resume / suite::compact`：19/19、15/15、3/3、9/9、88/88。
- `just test -p codex-core --features rust-rig -E 'test(rig_output_cap)'`：22/22（17 既有 + 5 新增：Chat 迟到 usage/primary 两例 + Responses 矩阵三例；Anthropic 矩阵两例计入，总数 22）。
- `just test -p codex-core --features rust-rig -E 'test(anthropic_cap_) + test(responses_cap_)'`：6/6。
- `just test -p codex-core capped_sampling_usage_follows_the_switched_step_model`：1/1。
- `just test -p codex-rust-rig-bridge --locked --offline`：261/261。
- `just test -p codex-http-client --locked --offline`：182/182（分类器修改后）。
- 每批后 scoped `just fix -p ...`、`just fmt`、`git diff --check` 均通过；最终 fix/fmt 后未重跑测试。

## 6. 开放项与下一步

1. **CI 待验**：userns/磁盘/分类器三修复待下次 push 的 CI 实证；windows/macos nextest 结果待补记。
2. **模型可见恢复/D6（P0 门）**：未动，维持上轮结论（无计量证明仅诊断）。
3. **app-server thread/read/attach/重连/分页、exec JSON/SDK/UI、旧 reader 实测**：未动（下批）。
4. **WS cap fixture**：Responses WS 端点复用同一 `ResponsesStreamEvent` 解析与 incomplete/usage 处理（`responses_websocket.rs:739` 起源码证实），usage 语义与 SSE 同路径；但 WS 传输层的 cap 终局专用 fixture（WS mock 服务端 → CapExhausted → ledger 全链）仍未建，保持开放。
5. code_mode `can_apply_patch_via_nested_tool` 本机失败需独立最小复现归因（1≠2 断言）。
6. 原 T21–T40 未变；本批不勾任何 checkbox。

## 7. 授权边界遵守情况

无付费厂商请求、无远程 push、无手动 dispatch、无默认代理迁移、无 V8 pin 变更；API key/凭据未入库。基线 worktree 两次（归因用）已清理。
