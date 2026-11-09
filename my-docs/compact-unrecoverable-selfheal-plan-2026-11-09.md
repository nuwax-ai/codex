# 上下文不可压缩即会话报废 — 自我修复实施方案(Plan)

日期:2026-11-09 · 状态:**P0-A/C/B 已实施，定向测试通过** · 触发事件:同事 Nuwax 客户端 3.1.1-beta.11
(nuwax-harness-acp-ts 1.2.29 / nuwax-harness 0.18.32)

关联:[overflow-autocompact-plan-2026-11-02.md](overflow-autocompact-plan-2026-11-02.md)(阈值跳步 + 事后恢复)、
[autocompact-threshold-plan-2026-11-02.md](autocompact-threshold-plan-2026-11-02.md)(默认 90%→80%)、
[migration-compaction-downshift-plan-2026-11-08.md](migration-compaction-downshift-plan-2026-11-08.md)(有界摘要)、
FORK.md §5 / §10

---

## 1. 问题与证据

### 1.1 用户可见症状

```
Codex ran out of room in the model's context window.
Start a new thread or clear earlier history before retrying.
```

定时任务会话(ACP session `01a1b0f4…`)在 2026-11-08 18:03 → 11-09 13:03 的**每一次**
小时级运行都以同一句失败,耗时稳定 1.0–1.3 s,`usage:null`;客户端把该文本当作
`ASSISTANT` 消息写回会话,下一小时连同样本一起再注入,历史只增不减。

### 1.2 已确认的四个事实(源码 + 日志)

| # | 事实 | 依据 |
|---|---|---|
| F1 | 自动压缩阈值有默认值:**80% × resolved_context_window**,无需环境变量 | `protocol/src/fork_compact.rs:18`;日志 10:17 `used:164119` 后 16 ms 出现 `Compact conversation` |
| F2 | 有效窗口 = 95% × 200000 = **190000**,硬墙;阈值 160000 | `protocol/src/openai_models.rs:412`;`core/src/session/context_window.rs:115`;`usage_update size:190000` |
| F3 | `NUWAX_AUTO_COMPACT_RATIO` / `NUWAX_AUTO_COMPACT_TOKEN_LIMIT` / `NUWAX_MODEL_CONTEXT_WINDOW` 别名**已存在且 ACP 路径可达** | `utils/cli/src/config_override.rs:91-100,143-146`;`app-server/src/lib.rs:550` 走同一 `parse_overrides()`;适配器 `CONSUMED_NUWAX_ENV_VARS`(`CodexEnvConfig.ts:261`)不含这三个,不会被 strip |
| F4 | 压缩失败路径**不落盘**:`remove_first_item()` 只改内存 clone,只有成功才 `replace_compacted_history` | `core/src/compact.rs:386-402`;`core/src/session/mod.rs:4653` |

### 1.3 根因(三个设计缺口,互相放大)

1. **缺口 A — 放弃后不落盘,于是永远原地失败。** 失败轮既不写 checkpoint 也不回滚,
   进程退出后 rollout 原样;下次 `loadSession` 装回同一条历史,在同一个点上再失败。
   F4 是这个循环的机制。
2. **缺口 B — 压缩请求必须"自己装得下"。** 历史越线后,摘要请求同样越线;兜底只能
   `remove_first_item()` 逐条删(删最旧、不删最贵、不摘要),删到剩 1 条仍超就原样
   抛回同一个错误 → 死锁。
3. **缺口 C — 触发点靠检查点 + 估算,一步跳跃会漏判。** 工具循环中间没有前台检查点
   (`async_compaction` 需宿主远程压缩 V2);图片/大输出 token 为粗估。151058 + 两张
   `view_image` + 1277 行 HTML 输出即可一步跨过 160000 与 190000。

> F1/F3 说明"没配环境变量/没有默认值"**不是**病因,修方案不应退回调参。

---

## 2. 目标与非目标

**目标**

1. **不变量**:任何一轮开始时,历史必须能被发送;装不进时自动降级并**落盘**,
   而不是报错。降级只在"确实装不下"时发生,一次 turn 至多一次。
2. **压缩链不再一步到死**:全量摘要 → 有界摘要 → 按成本裁剪到预算,三步之内必然收敛。
3. **错误文案与结构化信息正确**:品牌 `nuwax-harness`;能自愈时不出错;真不可恢复时
   说清原因与动作;ACP 侧带 `codexErrorInfo`,客户端可编程处理(自动换会话)。
4. **环境变量别名成文并回归覆盖**(F3 已实现,补测试与文档一致性)。

**非目标**

- 不改变默认阈值语义(80%,F1),也不在本次引入自适应 ratio;
- 不重做 migration compaction(downshift 路径已可用);
- 不改客户端 UI;客户端"超窗自动开新会话"作为跨仓协同项单独跟踪(§8.3)。

---

## 3. 方案总览

```
run_turn 轮首 / resume 之后
        │
        ▼
┌─────────────────────────────┐
│ ensure_history_fits()       │  ← 新增(P0-B):不变量闸门
│  active < full_window ? 通过 │
└──────────┬──────────────────┘
           │ 超窗
           ▼
  ① run_auto_compact(全量摘要)          —— 现有路径
  ② run_migration_compact_task(有界 span) —— 已存在,改为 fallback
  ③ degrade_history_to_target_window      —— 已存在,改为 fallback
           │
           ▼
  必须 replace_compacted_history 落盘 + EventMsg::Warning
           │
           ▼
       继续本轮(不再 return Err)
```

只有"配置非法"(窗口 ≤ 0、输出预算 ≥ 窗口、窗口未知却要求裁剪)才允许 `Err`,
因为那不是运行期可恢复状态(Fail-Fast 的边界)。

---

## 4. 补丁清单

### P0-A 品牌与错误语义

**落点**

- `codex-rs/protocol/src/error.rs:127-131`

```rust
// before
// The iOS input-limit classifier matches this message's ASCII prefix.
#[error("Codex ran out of room in the model's context window. Start a new thread or clear earlier history before retrying.")]
ContextWindowExceeded,

// after
/// Machine clients must match `codex_error_info == "context_window_exceeded"`,
/// never this prose; the prose is for humans and may be reworded.
#[error("nuwax-harness ran out of room in the model's context window. Start a new thread or clear earlier history before retrying.")]
ContextWindowExceeded,
```

**连带工作**

1. 全仓检索 `"Codex ran out of room"`(测试快照 `core/tests/suite/compact.rs:5962`、
   适配器 `failuresShownAsMessage` 相关断言)同步改名,避免只改一半。
2. 该注释声称"iOS 输入上限分类器匹配 ASCII 前缀"。**同一发布批次**必须让客户端
   改判 `codexErrorInfo`(`protocol/src/error.rs:520` 已映射为
   `CodexErrorInfo::ContextWindowExceeded`),否则 iOS 侧会漏判。这条要作为发布
   checklist 项写进 FORK.md §8。
3. 新增第二档文案(降级成功,不再是错误):

```rust
// EventMsg::Warning
"nuwax-harness discarded the oldest {shadowed} tokens of history to fit the model's context window."
```

**验收**:`grep -rn "Codex ran out of room"` 仅剩历史文档;快照测试更新;
`codex_error_info` 在 ACP `session_failure` 里可见。

### P0-B 新增不变量:ensure_history_fits

**落点**:`codex-rs/core/src/session/turn.rs`(新函数,放在 `run_pre_sampling_compact`
附近),调用点两处:

- `run_turn` 开头,migration block(`turn.rs:244`)之后、`run_pre_sampling_compact`
  (`turn.rs:1621`)之前;
- resume 恢复历史之后:`core/src/session/mod.rs:1772` / `:1820`。

**语义**

```rust
/// fork: 每轮/每次 resume 保证历史可发送。降级必须落 checkpoint,
/// 否则下次 resume 会把同一条超窗历史装回来(见 2026-11-09 事故 F4)。
pub(crate) async fn ensure_history_fits(
    sess: &Arc<Session>,
    step_context: &Arc<StepContext>,
    client_session: &mut ModelClientSession,
    pending_input_tokens: i64,
    cancellation: &CancellationToken,
) -> CodexResult<FitOutcome> {
    let status = context_window_token_status(sess, &step_context.turn).await;
    let Some(full) = status.full_context_window_limit.filter(|w| *w > 0) else {
        return Ok(FitOutcome::NotNeeded);           // 窗口未知 = 无从判断,不裁剪
    };
    if status.active_context_tokens < full
        && !status.pending_input_reaches_limit(pending_input_tokens)
    {
        return Ok(FitOutcome::NotNeeded);
    }

    // ① 全量摘要(现有)
    if let Ok(_) = run_auto_compact(..., CompactionPhase::PreTurn, ...).await {
        return Ok(FitOutcome::Summarized);
    }
    // ② 有界 span 摘要(已有实现)
    let outcome = run_migration_compact_task(sess.clone(), step_context.turn.clone(), cancellation).await;
    if matches!(outcome, MigrationOutcome::Converged) {
        return Ok(FitOutcome::Migrated);
    }
    // ③ 按成本裁剪到预算(已有实现)
    let degraded = degrade_history_to_target_window(sess, &step_context.turn, pending_input_tokens, cancellation).await?;
    // ④ 落盘 + 告知客户端
    persist_degraded_history(sess, step_context, &degraded).await?;
    warn_degraded(sess, step_context, degraded.shadowed_tokens).await;
    Ok(FitOutcome::Degraded { shadowed_tokens: degraded.shadowed_tokens })
}
```

**边界**

- 一次性:同一 turn 内用 `extension_data` 标记,避免每步重试裁剪;
- 只在轮首/resume 生效,**不进工具执行中途**(中途仍走 `fork_overflow_recoverable`
  的一次性补救,避免打断 tool_call/output 配对);
- `pending_input_tokens` 复用 `estimate_pending_input_tokens`(`turn.rs:1603`);
- 降级后必须让本轮继续,而不是 `return Err`。

**验收**

- 单测:构造 800k 历史 + 32k 窗口 + 摘要请求必超窗的 mock,断言第一个 turn
  **成功**、rollout 追加一条 Compacted checkpoint、发出 Warning;
- 集成:`resume` 超窗 rollout(可用固定 fixture)后第一个 turn 成功。

### P0-C 压缩内部的裁剪策略

**落点**:`codex-rs/core/src/compact.rs:386-402`

**现状问题**:`remove_first_item()` 删最旧、逐条试;遇到"一条巨大图片 + 一堆小消息"
效率为 0,且从未落盘。

**改法**

```rust
Err(e) if matches!(e.details(), CodexErrorDetails::ContextWindowExceeded) => {
    // 1) 先按"单项成本"排序,一次删到预算内(图片/大工具输出优先)
    let dropped = drop_most_expensive_items(&mut history, /*until*/ budget);
    if dropped > 0 { retries = 0; continue; }
    // 2) 已经无法再删(只剩必需前缀/最近一条) → 交给 P0-B 的降级链,
    //    不再把同一个错误抛给用户
    return Err(CodexErr::ContextWindowExceeded); // 由 ensure_history_fits 捕获并降级
}
```

- `drop_most_expensive_items` 复用 `context_manager::estimate_item_token_count`;
- 只删"可重放的输入项",不删最新一条 user 输入与 summary 前缀;
- 裁剪结果不在这里落盘,统一由 P0-B 的 `persist_degraded_history` 负责(单点)。

**注意**:`compact.rs` 同时承担"手动 /compact",手动场景失败**可以**报错,但文案
要走 P0-A 的新文案,且不能在 ACP 里伪装成 assistant 消息。

### P0-D 环境变量别名(F3 已实现,本次只补护栏)

1. 回归测试:`utils/cli/src/env_context_seeds_tests.rs` 已有 4 个 case,补一条
   **别名同时存在且取值一致**、一条**别名冲突报错点名两个变量**。
2. ACP 路径回归:在 `app-server` 的启动测试中断言
   `NUWAX_AUTO_COMPACT_RATIO` 经 `CliConfigOverrides::parse_overrides`
   进入层叠配置(app-server 已走同一函数,补断言防回归)。
3. 文档:NUWAX-HARNESS.md 环境变量表已列别名,补一句"`CODEX_*` 为兼容名,
   `NUWAX_*` 优先;两者同时设置且不一致时报错"。

### P1-A 工具循环中途的前台压缩检查点（2026-11-09 已实施）

**落点**:`core/src/session/turn.rs:929-940`

现在前台压缩被 `!needs_follow_up` 挡住,工具循环中间只有"撞墙后补一次"。

**改法**:每次 sampling 结束后(工具已执行完、下一次 sampling 前),条件改为

```rust
let mid_turn_due = token_status.token_limit_reached
    || token_status.active_context_tokens.saturating_mul(100)
        >= limit.saturating_mul(i64::from(turn_context.config.mid_turn_compact_percent));
```

新增配置 `mid_turn_compact_percent`(默认 85,`0` = 关闭),仅当
`needs_follow_up` 时生效;非 `needs_follow_up` 仍走现有的轮末路径。压缩放在
现有 `async_compaction::maybe_start` 的同一个检查点,不新开时机。

**风险**:中途压缩会改变 assistant/tool 配对的历史结构 —— 必须复用现有
`replace_compacted_history`,它已经把 tool 调用与输出一起纳入 replacement history,
不要自己拼历史。

### P1-B 预算估算修正(图片/大输出)（2026-11-09 已实施：观测上限）

1. 图片:在 `wire_images::canonical_vision_floor` 之上,对**工具返回的图片**追加
   固定下限(建议 4k/张,可由 `vision_token_floor` 配置),避免粗估漏判;
2. 大输出:工具输出落历史前已有 truncation policy,检查是否与 `estimate_item_token_count`
   使用同一口径(否则"截断了但按未截断算"或反之);
3. **网关拒绝学习（已实施）**：采样被拒后把活跃上下文记入
   `ObservedContextCeiling`（thread 级、仅内存、只降不升），
   `context_window_token_status` 把有效窗口钳到 `rejected × 95%`，下一次检查点
   自然提前压缩。单测 `observed_ceiling_clamps_and_keeps_a_margin`。
   不做"图片固定下限"：缺乏各厂商视觉计费实证，贸然抬高会过度压缩；观测上限
   用的是真实拒绝点，无需猜。

---

## 5. 数据契约

- 降级复用 `RolloutItem::Compacted` + `SurfaceReplacement` 语义:
  `replace_compacted_history(..., CompactedHistoryInstallation::Complete(metadata))`;
- metadata 里新增可选字段 `degraded: { shadowed_tokens, reason }`(serde default,
  旧 rollout 可读);
- 回放时不需要新逻辑:裁剪后的 replacement history 已是权威面;
- **禁止**在降级时只改内存(这是 2026-11-09 事故的根因)。

---

## 6. 测试矩阵

| 层 | 用例 | 断言 |
|---|---|---|
| compact 单测 | 800k 历史 + 32k 窗口,摘要请求必 400 | 不返回 Err;产生 checkpoint;Warning 带 shadowed_tokens |
| session 单测 | resume 一条超窗 rollout | 第一个 turn 成功;token 状态回落到窗口内 |
| compact 单测 | 一条 5MB 图片 + 200 条小消息 | 一次裁剪后入预算,且保留最近 user 输入 |
| turn 集成 | 工具循环中 active 跨过 mid_turn 阈值 | 出现 `Compact conversation`,turn 不中断 |
| protocol 快照 | `ContextWindowExceeded` 文案 | 含 `nuwax-harness`,不含 `Codex`;`codex_error_info` 不变 |
| cli | `NUWAX_AUTO_COMPACT_RATIO` / `CODEX_...` | 别名生效;冲突报错点名两变量 |
| 回归 | `cargo nextest` 全量(含 `compact.rs` 现有 insta 快照) | 仅预期的快照变更 |

Mac 上跑 core 集成测试记得 `RUST_MIN_STACK=33554432`;insta 用窄过滤器逐个接受。

---

## 7. 文档同步

- FORK.md §5 新增小节「§5.4 不可压缩时的自我修复(2026-11-09)」:不变量、三级降级、
  落盘要求、错误文案与 `codex_error_info` 契约;
- FORK.md §8 发布 checklist 增一条:文案变更必须同步客户端判据(§4 P0-A-2);
- NUWAX-HARNESS.md:环境变量别名优先级与冲突语义;
- 本文件状态从"待评审"改为"已实施"并记录版本号。

---

## 8. 提交拆分与顺序

1. **commit 1(P0-A)**:品牌文案 + 测试快照 + 注释改写。低风险,可先合。
2. **commit 2(P0-C)**:按成本裁剪 + 移除"删到 1 条就抛错"。
3. **commit 3(P0-B)**:`ensure_history_fits` + 轮首/resume 两个调用点 + 落盘 + Warning。
   ← 治愈"每小时原地失败"的关键提交
4. **commit 4(P0-D)**:环境变量别名回归 + 文档。
5. **commit 5(P1-A)**:中途检查点(可单独回滚,配置默认关闭更保守)。
6. **commit 6(P1-B)**:图片下限/口径统一。

### 8.1 风险与回滚

| 风险 | 缓解 |
|---|---|
| 降级误删重要历史 | 只在 `active >= full_window` 时触发;Warning 显式告知 token 数;一次 turn 一次 |
| 中途压缩破坏 tool 配对 | 只走 `replace_compacted_history`;P1-A 默认阈值保守(85%) |
| 文案变更导致客户端漏判 | 同批次改客户端判据;`codex_error_info` 保持稳定 |
| resume 阶段裁剪与 SDK 语义冲突 | 只在 ACP/app-server 恢复路径启用;TUI 恢复保持现状(可后续放开) |

### 8.2 跨仓协同(不在本仓实施,需开单)

- `fork-codex-acp-ts`:把 `ContextWindowExceeded` 走 `session_failure`
  (`category=context_exhausted`,建议动作 `new_session`),不要只发
  `agent_message_chunk` 文本;
- `nuwax-client`:收到 `context_exhausted` 自动开新会话;把
  `CODEX_MODEL_CONTEXT_WINDOW = "200000"` 改为按平台模型元数据下发,并可下发
  `NUWAX_AUTO_COMPACT_RATIO`;
- 客户端 `<context-message>` 回放:剔除失败消息、限制回放条数(否则一次失败会被
  永久放大,见事故 §1.1)。

### 8.3 事故止血(与代码无关)

- 已卡死的会话(`01a1b0f4`、`01a1b0c7`)必须开新 thread,旧 rollout 修不动;
- 过渡期可先设 `NUWAX_AUTO_COMPACT_RATIO=0.6` 降低再次撞墙概率。

---

## 9. 实施记录（2026-11-09，P0-A / P0-C / P0-B）

| 项 | 落点 | 说明 |
|---|---|---|
| P0-A 品牌 | `protocol/src/error.rs` | 文案改为 `nuwax-harness ran out of room ...`；注释改为"机器判据只能是 `codex_error_info`"，并在 `FORK.md §5.4/§8` 记入发布协同要求 |
| P0-C 压缩内裁剪 | `core/src/compact.rs`、`session/compaction_prune.rs`（复用既有死代码） | 溢出时先 `prune_oversized_tool_results`（确定性的头/尾窗口裁剪，无模型调用、不动消息结构）；仍超再逐条删最旧，最多 `MAX_OVERFLOW_TRIM_ROUNDS = 2` 轮，然后交还降级链。**放弃"按 25% 批量丢"**：`local_compaction_retries_preserve_instructions_and_tool_metadata` 证明批量删会吃掉仍需保留的工具调用/输出对，改为有界的逐条删 |
| P0-B 自愈 | `core/src/session/turn.rs`、`core/src/compact.rs` | 轮前压缩与中途恢复臂遇到 `ContextWindowExceeded` 时转 `degrade_history_to_target_window`（新增 `DegradeReason`），落 `SurfaceReplacementReason::Overflow` 并提示一次；仅"必要内容本身超窗"才返回可行动 `Fatal` |
| 测试 | `core/src/context_manager/history_tests.rs`、`core/tests/suite/compact.rs` | `remove_oldest_fraction_for_compaction_*` 两条单测（批量丢最旧 + 永不动最后一条）；原 `snapshot_request_shape_pre_turn_compaction_context_window_exceeded` 改为断言"压缩失败后本轮继续、不再报错"，并同步快照头部文案 |

**测试取舍**：原计划的"降级必须真的裁掉历史并警告"集成测试（构造超预算历史 →
断言 Warning 文案）暂时移出：单条 item 在记录时按 `truncation_policy`（默认
10k bytes）截断，要靠"多条大消息累积"才能稳定超过预算，而这让 fixture 体积/耗时
不可接受（实测 12×60k 字符的用例单轮 ~50s 且事件超时）。`shadowed_tokens > 0`
分支目前由既有迁移压缩用例覆盖（同一实现、仅 `DegradeReason` 不同文案），
后续用"小窗口 + 受控工具输出"重做该用例。
| 文档 | `FORK.md §5.4/§8`、`NUWAX-HARNESS.md` | 行为差异、发布协同、别名冲突语义 |

未做（后续迭代）：P1-A 中途检查点（`mid_turn_compact_percent`）、P1-B 图片 token 下限、
ACP 结构化 `session_failure`（跨仓）、失败压缩条目的 UI 收尾（`ContextCompaction`
completed 事件在失败路径仍未发出，属既有小瑕疵）。

### 9.1 定向测试结果（2026-11-09，本机 aarch64-darwin）

| 命令 | 结果 |
|---|---|
| `cargo check -p codex-core` | 通过（仅既有 dead-code 警告） |
| `cargo nextest run -p codex-core -E 'test(compact::tests)'` | 30/30 通过（含 `local_compaction_retries_preserve_instructions_and_tool_metadata` 两例；单跑复验稳定） |
| `cargo nextest run -p codex-core --test all -E 'test(pre_turn_compaction)' --test-threads 1` | 3/3 通过（含改写后的 overflow 快照） |
| `rustfmt --edition 2024 --check`（改动文件） | 干净 |

注意：该测试套件在并行跑时偶发 10s 事件超时（与本改动无关，未改动的同类用例同样复现），
定向复验请用 `--test-threads 1`；Mac 上需 `RUST_MIN_STACK=33554432`。


---

## 10. 复审修复（2026-11-09 晚，codex review）

| 发现 | 结论 | 修复 |
|---|---|---|
| P1 空尾部裁剪在重载时隐藏后续消息 | 成立 | `rollout_reconstruction` 对空 boundary 改为"定位记录之后第一条 item"；新增回归 `a_no_summary_trim_with_later_turns_keeps_them_on_replay` |
| P1 轮内降级可能裁掉当前用户指令 | 成立 | 新增 `DegradeProtection::CurrentTurnInput`：边界不越过最新 user 消息；保护后仍超预算则 `NecessaryContentExceedsWindow` |
| P2 一次采样可能连做两次轮内压缩 | 成立 | 检查点后重算 token status，旧 rollover 分支不再用过期阈值 |
| P2 观测上限未按路由隔离 | 成立 | `ObservedContextCeiling` 加 `model`/`provider`；新增 `observed_window_limit` 只用于调度，裁剪预算仍用宣称窗口，避免一次图片密集请求把后续轮次误判为"必要内容超窗" |
| P2 ACP 失败信号未接到客户端终态 | 成立 | ① 客户端在 `new_session` 或 `category=request` 时 `throw`；② "必要内容超窗"改用已存在的 `CodexErrorInfo::InvalidPrompt`（独立 wire tag，无需改 schema），适配器新增 `input_too_large` 失败种类（category=request、actions=[]），legacy `_meta` 也透出 |
| P2 配置 schema 未更新 | 成立 | `cargo run -p codex-config-schema --bin codex-write-config-schema` 重新生成，`config_schema_matches_fixture` 通过 |

**客户端命名**：`DEFAULT_CODEX_MODEL_CONTEXT_WINDOW` → `DEFAULT_NUWAX_MODEL_CONTEXT_WINDOW`；
客户端下发的环境变量改用 `NUWAX_MODEL/BASE_URL/API_KEY/PROVIDER_ID/WIRE_API/
DISABLE_THINKING/MODEL_CONTEXT_WINDOW`（适配器与 harness 均支持同义别名），日志里
不再打印 `CODEX_*`。`CODEX_HOME`/`CODEX_SQLITE_HOME`/`CODEX_LOG_DIR` 因无同义别名
暂留，目录名 `.codex` 的改名单独评估。
