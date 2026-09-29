# fork 缺陷修复 — 实施方案（2026-09-29，交 Claude Code 执行）

> 执行者：Claude Code。复核者：用户 + Qoder（原分析者）。
> 前置结论：两轮独立复核已定案——**B 组 5 条代码缺陷 0 条成立**（详见
> `fork-defect-review-2026-09-29-rebuttal.md` 与 `-round2.md`）。本方案只落地
> 已确认的 **文档(A)/仓库卫生(C)/测试缺口(N1,B2残留)/P3 nit** 四类工作。
>
> 执行纪律（AGENTS.md）：改完跑 `just fmt`（在 `codex-rs/` 下）；测试用
> `just test -p <crate>`，**不要直接 `cargo test`**；大改后 `just fix -p <crate>`；
> 动 core/common/protocol 后跑全量 `just test` 前**先问用户**。文件差异一律用
> `/usr/bin/diff`（shell 里 `diff` 被别名劫持会静默吞输出，两轮复核都踩过）。
> 提交按文件名分组；push 被拒用 `git pull`/merge，**不要 rebase**（共享分支）。

---

## ⚠️ 先纠正前两份报告里的锚点错误（执行前必读；本节已经第三轮核实修订）

> 修订记录（fork-fix-plan-verification-2026-09-29.md）：本节原文对 round2 的
> "纠正 1"本身有误，已按实测撤回——日志串真实存在；"纠正 2"成立并已扩大清单。

1. **N1 测试的"证明走桥"方法**：round2 引用的日志串
   `"Dispatching chat stream via rig"` **真实存在**（
   `codex-rs/codex-rust-rig-bridge/src/stream.rs:169`，live stderr 已实证可达），
   round2 锚点并非臆造。但 core 集成测试 harness 不捕获 tracing 输出，**suite 测试
   里断言日志串不可行**——证明"走了桥"改用**结构性证据**（见 Batch 2 / N1）；
   live/exec 场景（harness 捕获 stderr）可选断言该日志串。真实分派链是
   `core/src/client.rs` 调 `dispatch_model_bridge` → `RigModelBridge.stream(...)`。

2. **A1 错名 4 处跨 3 文档，旧符号共 6 处（第三轮扩大）**：
   - `responses_routes_via_chat_bridge`：`FORK.md:50`、`codex-review-prompt.md:37`
     与 `:97`、`rig-bridge-implementation-plan.md:140`。
   - `codex-review-prompt.md`：`:37` 的 `stream_chat_api()`、`dispatch_chat_bridge()`、
     `&dyn ChatModelBridge`、`responses_routes_via_chat_bridge()` 四个死/旧符号；
     `:38` 的 `ChatModelBridge`/`ChatWireProtocol`/`chat_wire_protocol()`；
     `:58` 的 `ChatModelBridge`；`:59` 的 `dispatch_chat_bridge`/`&dyn ChatModelBridge`。
     真实符号：`stream_model_bridge` / `dispatch_model_bridge` / `&dyn ModelBridge` /
     `ModelWireProtocol`（`codex-api/src/bridge.rs`）。
   - 桥实现结构体真名 `RigModelBridge`。**不存在**名为
     `responses_routes_via_chat_bridge` 的谓词函数；路由判定内联在
   `Codex::stream`（`client.rs` ~2400）里的 `info.uses_model_bridge()`。
   - `rig-bridge-implementation-plan.md:138`"桥自动丢弃宿主工具"亦为过时结论
     （现状是 hosted 工具翻译表），由 1.3 废止戳覆盖。

> 执行每个 Batch 前，先 `grep`/打开坐标确认现状与本文一致；若代码已变（用户在并行
> 改动），停下报告，不要盲改。

---

## Batch 1 — P2 文档 + 仓库卫生（低风险，第一个提交）

### 1.1 A1 错名/旧符号改正（4 文件）
- `my-docs/FORK.md:50`：`responses_routes_via_chat_bridge` → 改为引用真实机制
  （`uses_model_bridge()` 判定 + `dispatch_model_bridge` 分派）。
- `my-docs/codex-review-prompt.md:37` 与 `:97`：同上；并把 `dispatch_chat_bridge()`→
  `dispatch_model_bridge()`、`&dyn ChatModelBridge`→`&dyn ModelBridge`。
- `my-docs/rig-bridge-implementation-plan.md:140`：改函数名引用；并在文件顶部加一行
  废止戳（见 1.3）。
- 验收：`grep -rn "responses_routes_via_chat_bridge\|dispatch_chat_bridge\|ChatModelBridge\|ChatWireProtocol\|RigChatBridge" my-docs/` 仅剩废止戳/历史说明处，无"现状描述"误用。

### 1.2 FORK.md 补"已知局限"章节（新增 §10）
在 FORK.md 末尾（§9 后）新增 §10，登记（每条一句话 + 坐标）：
- Anthropic 固定 `DEFAULT_ANTHROPIC_MAX_TOKENS=16384`（`client.rs:120`）——长输出/重思考
  模型偏小；命中即整轮报错（与 native `response.incomplete` 行为一致，非缺陷，是产品局限）。
- 无 Anthropic prompt caching（`cache_control` 桥内零出现）。
- 每轮新建 reqwest 0.13 client（`stream.rs`/`responses.rs` 每次 `http_client(...)`）——每轮 TLS 握手。
- 跨轮 web_search 检索上下文丢失：`WebSearchCall`（`protocol/src/models.rs:1190-1203`）
  无结果字段，桥丢弃历史（`request_messages.rs:207-219`）；忠实多轮回放需 phase-3（见 Batch 5）。
- 病态网关"HTTP 200 + 非 SSE JSON 错误体"两线都会 EOF 丢体（三家目标厂商 live 未见此行为，记边界）。

### 1.3 旧计划文档盖废止戳
- `rig-bridge-implementation-plan.md` 顶部加：`> ⚠️ 已被 FORK.md 取代（§8 Responses→Chat 转换、§12.4 ChatModelBridge 均为旧设计；现状见 FORK.md §3.3/§3.4）。保留作历史。`
- 验收：读者不会再把该文档当现状权威。

### 1.4 C1 删顶层游离副本（单独提交，删前必须 diff）
- 先 `/usr/bin/diff codex-rust-rig-bridge/src/convert_response.rs codex-rs/codex-rust-rig-bridge/src/convert_response.rs`，
  **确认顶层 330 行是早期原型（旧 PendingRigMessage），真实 crate 295 行是重构后版本**——
  即"顶层落后、无真实桥缺失代码"。若 diff 结果与此相反（顶层有真实桥没有的逻辑），**停手报告**。
- 确认无引用：`grep -rn "codex-rust-rig-bridge" justfile MODULE.bazel* .github/ codex-rs/Cargo.toml`
  ——workspace member 只指向 `codex-rs/codex-rust-rig-bridge`（`codex-rs/Cargo.toml:147/182`），顶层目录无任何构建引用。
- `git rm -r codex-rust-rig-bridge/`（顶层）。验收：`cargo check`（或 `just` 对应）不受影响；顶层目录消失。

**Batch 1 提交分组**：① 文档改正(1.1-1.3) 一个提交；② 删顶层副本(1.4) 一个提交。

---

## Batch 2 — P2 测试缺口（第二个提交，本方案最高价值项）

### 2.1 N1：桥线"400 超窗 → ContextWindowExceeded → trim 重试"端到端
- **位置**：`core/tests/suite/rig_responses_bridge.rs`（已 `#![cfg(feature="rust-rig")]`、
  已手写自定义 provider、已用 `responses::mount_response_sequence`/`mount_sse_once` +
  `request.path()`/`body_json()` 断言——直接复用其样板，**不要新造证明机制**）。
- **新增测试** `bridge_chat_compact_recovers_from_context_window_rejection`：
  1. provider：`wire_api = Chat`、`provider_id = Some("mock-chat")`（非 first-party）、
     `base_url` 指向 mock server → `uses_model_bridge()=true`。
  2. **走桥的结构性证明**（替代臆造日志串）：chat-wire provider **没有 native 传输可回落**
     （`dispatch_model_bridge` 注释 + `client.rs:2393-2400` 无桥 feature 时直接 Fatal）。
     断言 mock server 收到的是 **chat-completions 形态**请求（路径/body 形状按
     `rig_responses_bridge.rs` 既有断言风格），且测试在 `rust-rig` feature 下运行——
     二者共同证明经过桥。可选再加 `wire_api=Anthropic` 变体覆盖 `with_terminal_check` 与 400 共存。
  3. mock 序列：`[SSE 轮1(usage 高), SSE 轮2(更高), 400 JSON(context_length_exceeded), SSE 摘要, SSE 轮3]`。
  4. 断言：请求总数符合预期；出现一个 400；**400 后的重试请求 input 条目数严格少于 400 那次**；
     最终轮正常完成；rollout 含 compacted 记录。
- 验收：`just test -p codex-core --test suite rig_responses_bridge`（或仓库等价命令）绿；
  该测试在 `--no-default-features`（无 rust-rig）下被 cfg 跳过而非失败。

### 2.2 B2 残留：跨轮 web_search live 场景
- 在 live-tests crate 增加"多轮 web_search"场景（厂商×桥矩阵），**断言当前行为**：
  第二轮请求**不含**上一轮 server_tool_use/web_search_tool_result 块（即丢弃是当前的预期行为），
  并注释指向 phase-3 增强。目的：把"已知局限"钉成回归基线，而非留空白。
- 验收：live 场景跑通并落 JSONL；若需真实凭据，标 `LIVE_VENDORS` 门控，CI 无凭据时跳过。

**Batch 2 提交**：测试代码一个提交。

---

## Batch 3 — 经验验证缺口收口（关闭前置条件）

> 两轮均为静态读码；`claude-rig-full-validation.md` 从未产出。在宣称"fork 无正确性缺陷"前补此步。

- 跑桥相关 crate 全量：`just test -p codex-rust-rig-bridge`，再 `just test -p codex-core`
  （core 全量前**先问用户**，因 AGENTS.md 要求）。
- 已知障碍：`codex-code-mode-host` 依赖的 rusty_v8 归档本机 404 无法构建，会导致 core 里
  ~200 个依赖 code-mode 的用例失败（见 `nuwax-home/tasks.md T2.3`，与本改动无关）。
  处理：**排除 code-mode-host 依赖子集**跑，或如实记录"这些失败是预存在的构建环境缺失，非回归"。
- 产出 `my-docs/claude-rig-full-validation.md`：命令、通过/失败/跳过计数、失败逐类归因、
  与基线对照。**或**（若无法跑全量）在 FORK.md §10 显式写明"结论基于静态复核，全量经验验证受
  code-mode-host 构建缺失阻塞，待补"。二选一，不许留空。

---

## Batch 4 — P3 代码 nit（可选，最后，单独提交；无正确性影响）

- `request_tools.rs:41-47`：`unwrap_or_default()` 前加 `debug_assert!`（或失败 warn），
  防 >128 层嵌套 schema 静默零工具。不改正常路径行为。
- `reasoning.rs:44-49`：`first_mut` 不匹配（契约外"complete 后同 id delta"）时加一条 `debug!` 日志，便于排查。
- `transport.rs` `with_terminal_check` / `sse.rs`：零 SSE 帧即 EOF 时，把原始 body 前 N 字节附进错误信息（4b 加固，可选）。
- `stream.rs`：无 hosted 工具声明时跳过 anthropic tee，省一次全量拷贝（性能 nit，**非泄漏**，可不做）。
- 验收：`just test -p codex-rust-rig-bridge` 仍绿；`just fix -p codex-rust-rig-bridge` 零告警；`just fmt`。

---

## Batch 5 — phase-3 增强登记（仅文档，不实现）

- 在 FORK.md §4（hosted 工具）或 §10 登记规划项："多轮 web_search 忠实回放 = 新增
  `WebSearchCall` 结果持久化字段 + 成对回放 server_tool_use/web_search_tool_result"。
  明确这是**增强**，非缺陷修复；当前丢弃行为正确。

---

## 执行顺序与交付

1 → 2 → 3 → 4（可选）；Batch 5 随 Batch 1 文档一起改。
每个 Batch 完成后：跑该 Batch 的验收 + `just fmt`，按文件名分组提交，**不自动 push**（等用户确认）。
全部完成后回报：每批的 diff 摘要、测试结果、Batch 3 的 validation 记录或显式声明。

## 关闭标准（全部满足才算完）

- [ ] A1 错名 4 处 + 旧符号 2 处全改，grep 无残留误用
- [ ] FORK.md §10 已知局限补齐；旧 plan 盖废止戳
- [ ] 顶层 `codex-rust-rig-bridge/` 已删（删前 /usr/bin/diff 确认方向）
- [ ] N1 桥线端到端测试落地并绿（用结构性证明，非日志串）
- [ ] 跨轮 web_search live 场景钉成回归基线
- [ ] `claude-rig-full-validation.md` 产出 **或** FORK.md 显式声明验证受阻
- [ ] P3 nit（可选）落地或显式跳过
- [ ] phase-3 增强已登记
