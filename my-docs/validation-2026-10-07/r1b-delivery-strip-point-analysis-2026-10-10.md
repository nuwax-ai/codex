# R1b 投递链静态追踪：guardian 复核 compaction item 被剥的确切位置（2026-10-10）

任务来源：`pkg3-failure-signatures-worklist.md` R1b 节（2026-10-09 判别定案后的下一步）。
方法：只读静态追踪，不改代码。所有行号基于当前工作区（branch `test`，含未提交改动前后的 guardian 链文件均为已提交状态）。

---

## 0. 结论（TL;DR）

**剥离点不在 guardian 侧，也不在会话种子/池路由，而在 fork 新增的"出站请求不透明载荷投影"：**

- 文件：`codex-rs/core/src/model_output_projection.rs`
- 函数：`project_input`（179-261 行）
- 精确剥离臂：**217-222 行** `ResponseItem::Compaction { .. } => { if !opaque_compatible { dropped += 1; } opaque_compatible }` —— provenance 不匹配时该 item 从出站请求 input 中被**整项移除**。
- 调用点（三条出站路径全部投影）：
  - `codex-rs/core/src/client.rs:1768-1773`（`stream_model_bridge`，1702 行起）
  - `codex-rs/core/src/client.rs:1962-1967`（`stream_responses_api`，1888 行起；**测试实际走这条**，R1 修复后 fixture 钉了 `experimental_bridge = "native"`）
  - `codex-rs/core/src/client.rs:2158-2163`（`stream_responses_websocket`，2105 行起）

**根因一句话**：复核线程（模型 `codex-auto-review`）重播父线程（父模型）产出的加密 checkpoint 时，`project_input` 要求 item 的 envelope 元数据 `model_output_provenance` 与**当前请求**的 provenance **全字段相等**（`ModelOutputProvenance` 派生 `PartialEq`，含 `model`，`codex-rs/history/src/lib.rs:139-164`）。父产 checkpoint 的 provenance（父模型 + 无 guardian 头）与复核请求的 provenance（复核模型 + `x-codex-guardian: reviewer` 头改变 auth 域）**必然不等** → `compatible=false` → `opaque_compatible=false` → Compaction item 被丢弃。

该投影是 **fork 独有代码**：`model_output_projection.rs` 由 fork 提交 `ead6f2bca`（"feat: checkpoint Rig stability and provider request controls"，2026-10-03）新增，`61e331fd9`（"fix: bind model replay to actual credential scope"）加固；`origin/main` 上无此文件。时间线与 10-07 验证轮出现 R1b 13 项失败吻合。

---

## 1. 完整投递链静态追踪（文件:行号）

以下按执行顺序列出 sync 复核从入口到 HTTP 请求的每一跳，并标注每跳对 compaction item 的处理。

### 1.1 入口与准备

| 步骤 | 位置 | 说明 |
|---|---|---|
| 1 | `core/src/guardian/review_request.rs:196-205` `attempt()` → `run_guardian_review_session_before_deadline` | 每次尝试用**活历史**重建（review.rs:221 `parent_history: session.clone_history().await`），不是过期快照 |
| 2 | `core/src/guardian/review.rs:215-235` 组装 `GuardianReviewSessionParams` | `parent_history` 为压缩后父历史（`parent_input_types` 证据与之相符） |
| 3 | `core/src/guardian/review_session_setup.rs:19-56` `PreparedGuardianContext::prepare` | 计算 `context_policy`（ThreadOwned / LegacyWithCheckpointReuse / Legacy / Independent）与 `parent_compaction` |
| 4 | `core/src/guardian/review_session_context.rs:54-84` `parent_compaction()` | `CompactionCheckpoint::latest(history.annotated_items())`（`codex-rs/history/src/compaction_checkpoint.rs:19-31`）；ThreadOwned 与 LegacyWithCheckpointReuse 都会返回 envelope；usable 校验 46-60 行。**此步在测试场景返回 Some**（父历史含 compaction，10-09 已证） |
| 5 | `review_session_setup.rs:36-46` 组装 reuse key | ThreadOwned/LegacyWithCheckpointReuse 的 `parent_history_version` 用真实版本号（`review_session.rs:216-223`）；压缩经 `replace_compacted` 必然 +1（`context_manager/history.rs:718`） |

### 1.2 池路由（snapshot vs trunk 两分支的重播方式）

`ext/guardian-reviewer/src/pool.rs` `review()`（173-289 行）：

| 分支 | 条件 | 传给 spawn 的 snapshot | 历史重播方式 |
|---|---|---|---|
| **trunk 复用** | key 匹配且锁可得（245-255 行） | — | **不重播**：直接在既有 trunk 会话上追加本轮 turn；trunk 历史只有它自己积累的复核对话（`ConversationState::commit_snapshot`，`review_session.rs:922-933`） |
| **trunk 替换** | key 不匹配且锁可得（190-195 行 `state.take()` → 199-229 行） | `None` | 新 trunk；`thread_options(None)` → 种子 = `parent_compaction` envelope（见 1.3） |
| **ephemeral（上下文失配）** | key 不匹配且锁被占（242-244 行） | `None` | 同上，仅种子 |
| **ephemeral（busy fork）** | key 匹配但锁被占（245-254 行） | `trunk.session.snapshot()` | `thread_options(Some(snapshot))` → 种子 = snapshot 的 `initial_history`（`review_session.rs:918-920`/929-932，来源 `session.guardian_fork_history()`，`core/src/session/guardian_checkpoint.rs:11-42`，含 `RolloutItem::Compacted` 包装的 replacement_history） |

测试场景（每次压缩后 `parent_history_version` 变化）走的是 **trunk 替换**分支：旧 trunk 被丢弃、新 trunk 以 `parent_compaction` envelope 为种子。R1b 观察到的"3 条 message"与新 trunk 的预期内容一致（历史里只有 checkpoint envelope + 本轮 final 化的复核上下文消息），说明**路由分支本身按设计工作，种子已进入会话**——问题在更下游。

### 1.3 种子 → 会话历史

| 步骤 | 位置 | 说明 |
|---|---|---|
| 6 | `review_session_setup.rs:90-97` `thread_options` | snapshot=None 时 `initial_history = InitialHistory::Forked(vec![RolloutItem::ResponseItem(envelope)])`；envelope 带 item+metadata（fork 提交 `e9996566b` 保留 producer 元数据） |
| 7 | `ext/guardian-v2/src/sync_reviewer/mod.rs:62-156` spawn 闭包 | `session_source = Internal(Guardian)`、`thread_source = GuardianReview`、`SessionIsolation::Isolated`，`start_thread_until` |
| 8 | `core/src/thread_manager/managed.rs:26-108` → `thread_manager.rs:1109-1155` `start_thread_inner` → `spawn_thread`（2041 行起，2286 行 `conversation_history: initial_history` 原样传入） | 无过滤。注意 2292-2299 行把 Guardian source 重映射为 `SubAgent(Other("guardian"))`（复核线程确实是 subagent） |
| 9 | `core/src/session/session.rs:1942` `record_initial_history(initial_history)` | 无条件调用 |
| 10 | `core/src/session/mod.rs:1623-1627`（Forked 臂）→ `apply_rollout_reconstruction`（1704-1801 行）→ `reconstruct_history_from_rollout`（`core/src/session/rollout_reconstruction.rs:169-536`） | 种子是 `RolloutItem::ResponseItem`（非 `RolloutItem::Compacted`），`select_input_compaction`（61-88 行）返回 None → 全量重播；420-425 行 `replay_annotated_item` |
| 11 | `core/src/context_manager/history.rs:512-531` `replay_annotated_item` → `record_item_with_metadata`（533-576 行） | `is_api_message`（998-1021 行）对 `ResponseItem::Compaction`/`ContextCompaction` 返回 **true** → envelope（含 metadata）原样 push 进 `self.items`。**复核会话历史确实包含 checkpoint envelope** |
| 12 | `session/mod.rs:1758-1785` 安装历史 | `replace_annotated_history(..., HistoryReplacement::Reset)`（`core/src/state/session.rs:186-209`）→ `replace_annotated`（`context_manager/history.rs:669-683`）无过滤 |

结论：**种子链路（步骤 6-12）完整，没有任何 subagent 专属的历史剥离**。worklist 中"internal_model_context 对 subagent 剥离加密 item"的嫌疑可排除：`core/src/context/internal_model_context.rs` 只是文本 fragment 封装（`<codex_internal_context>` 标记），不触碰 ResponseItem 级过滤；`is_api_message`/`normalize_history`（`history.rs:933-947`，只做 call/output 配对与音视频裁剪）均保留 Compaction。

### 1.4 会话历史 → 请求 input（剥离发生处）

| 步骤 | 位置 | 说明 |
|---|---|---|
| 13 | `core/src/guardian/review_session.rs:673-718` | `PendingReviewContext` 注入 + `start_review_turn` 提交 turn（transcript 经 `input_budget::finalize` 展开为 turn input，`core/src/guardian/input_budget.rs:81-265`，203-249 行把 marker 替换为完整上下文消息——这些是 Message item） |
| 14 | `core/src/session/turn.rs:515-521` | `sampling_request_input = history.for_prompt_annotated(...)` = **[checkpoint envelope, …turn 消息]**（`history.rs:589-595`，normalize 不删 Compaction） |
| 15 | `core/src/session/turn.rs:1665` | `input_provenance = sources_for_input(&prompt_input)`（`model_output_projection.rs:156-175`）：`"cmp_repository"` → envelope.metadata.model_output_provenance（= **父模型压缩请求**的 provenance，`compact_remote_v2_attempt.rs:130` 捕获、`compact_remote_v2.rs:324-329` 附到 compaction envelope——`build_v2_compacted_history` 538 行保证 compaction item 在最后） |
| 16 | `core/src/client.rs:1947-1955` | `producing_source = request_source(..., model_info.slug /* = "codex-auto-review" */)` —— **复核请求自己的 provenance** |
| 17 | `core/src/client.rs:1962-1967` → `model_output_projection.rs:190-222` | `compatible = target_known && sources[id] == Some(target)`；`ModelOutputProvenance` 全字段相等（含 `model`）→ **false**；Compaction 臂返回 `opaque_compatible`=false → **item 被移除**，并输出 warn 日志 `"Dropped incompatible opaque model history from the outbound request"`（256-260 行） |

**这就是 sync review 请求 input 只剩 3 条 message 的原因**：剩下的 3 条是步骤 13 注入的复核上下文 Message（`project_input` 的 Message 臂恒为 true，240-253 行），而历史里唯一的 seed item（Compaction）在第 17 步被丢弃。

---

## 2. 交叉验证（为什么各观察面互相吻合）

| 观察 | 解释 |
|---|---|
| 父请求 input 含 compaction（`parent_input_types=[message, compaction, …]`） | 父的 turn 请求与压缩请求**同模型、同 provider、同端点、同 auth、无 guardian 头** → provenance 相等 → `compatible=true` → 保留 |
| async（Luna）input 含 checkpoint（compatible 用例通过） | Luna sampler **自建** `ResponsesApiRequest`（`ext/guardian-v2/src/async_scorer/sampler.rs:230-252`），把 `parent_compaction` 直接 push 进 input（186-188 行），有自己的传输（`sampler/execution.rs`），**完全不经过 `client.rs`/`project_input`** |
| sync input 只有 3 条 message | 见 1.4 步骤 17 |
| e9996566b（保留 producer 元数据）修复后仍缺 item | 修复前：种子 envelope 无 metadata → `sources.get(id)=None` ≠ `Some(target)` → 丢弃；修复后：`Some(父prov)` ≠ `Some(复核prov)` → 丢弃。**两种情况都在同一点被剥**，只是失败原因从"缺元数据"变成"元数据不匹配"。这精确解释 worklist 的"首次修复不足、投递在更下游被截" |
| 已回退的"fork snapshot 追加 envelope"尝试同样无效 | 该修复只覆盖 busy-fork 分支的种子；种子进入历史后同样在步骤 17 被剥 |

补充一个使不等**更加必然**的细节：复核请求带 `x-codex-guardian: reviewer` 头（`sync_reviewer/mod.rs:98-104` 注入 `CodexResponsesHeaders`；native 路径在 `client.rs:1946` 把它并入 `options.extra_headers` 后才计算 `request_source`）。`x-codex-guardian` 不在 benign 头白名单（`codex-rs/codex-api/src/model_source.rs:57-91`）→ 复核请求的 `auth_domain` 走 credentialInstance 分支（`model_output_projection.rs:108-125`），父压缩请求仅在有匹配账户凭据时走 account 分支（95-107 行）；其他来源还可能为 anonymous/credentialInstance/缺证来源。本案需要分别核对 **model 与 auth_domain 的实际差异**——即便只放宽 model 比较也修不好（见 §4）。

---

## 3. 嫌疑排除记录（对照 worklist 的三个候选）

1. ~~"复核线程作为 subagent 的请求组装剥离"（internal_model_context / subagent 过滤）~~ — 排除。subagent 身份不影响历史记录与 `for_prompt_annotated`；剥离与 subagent 无关，任何"重播他模型产出的加密 item"的请求（无论是否 subagent）都会被 `project_input` 剥。
2. ~~"走 trunk 复用分支根本不重播历史"~~ — 排除为本案主因。压缩使 `history_version` +1（`replace_compacted`，`history.rs:718`），ThreadOwned/LegacyWithCheckpointReuse 的 key 必然失配 → 走 trunk 替换分支且带种子。trunk 复用分支确实不重播（那是设计行为，靠 `input_budget` 的完整 transcript 交付），但它不是本案路径。
3. ~~"种子未生效 / CompactionCheckpoint::latest 返回 None"~~ — 排除（10-09 已证候选 B：父历史含 item；本链 1.1-1.3 复核种子到历史全程无丢失）。

---

## 4. 建议的最小修复方案（不实施）

设计约束：`project_input` 的目的是跨协议/跨凭证 resume 时**不盲目重放不透明载荷**（fork 的既定设计，`61e331fd9`）。而 guardian 复核是**有意**跨模型重放父 checkpoint 的唯一合法场景（上游语义：`review_session_context.rs:74-76` 注释"sync reviewer 可跨 comp_hash 消费 checkpoint，由后端校验载荷，复核错误仍 fail-closed"）。因此修复应把这个**显式授权**从 guardian 种子一路带到投影点，而不是全局放宽比较。

推荐方案（改动面最小、语义显式）：

1. 在可信运行时为本次 guardian 请求创建有限 replay grant，绑定确切 checkpoint 身份、producer 来源、目标 reviewer 的 endpoint/auth/wire/模型变化范围以及会话/请求生命周期；不得从可编辑的持久 envelope bool 自行授予权限。
2. sources_for_input 保留普通 provenance；grant 经可信调用链传给投影，仅对绑定的 checkpoint 和目标请求生效。禁止“任意 scope 可重放”哨兵；普通 resume、未知来源、跨 endpoint/凭据/wire 仍按原隔离规则处理。
3. project_input 的 Compaction/ContextCompaction 分支仅在完整 grant 校验通过时保留。Reasoning 的 encrypted_content 规则不放松；不能将 guardian header 加入全局 benign 白名单来绕过认证域变化。
4. 回归须覆盖实际 guardian 阳性与普通 resume/endpoint/auth/wire/未知来源阴性，关联同一 history/checkpoint/attempt 并断言最终 wire；13 个历史身份逐个复验，不能预先宣布转绿。方案尚未实施，完整约束见 ../rig-production-completion-2026-10-10/plan.md。

不推荐的替代（列出以备评审）：
- 全局把 Compaction 的比较降为"provider+endpoint+auth 忽略 model"：**不够**——`x-codex-guardian` 头使 auth_domain 也不同（§2），还得再把该头加入 benign 白名单，改动面反而更大且弱化了凭证隔离设计。
- 在 `stream_responses_api` 等三处为 reviewer 特判跳过投影：三处调用点 + `is_guardian_reviewer` 判定扩散，违背"最小 footprint"。

另注：修复后 `guardian_checkpoint_tests.rs`（`core/src/session/guardian_checkpoint.rs` 配套）与 `review_session_tests.rs` 中 e9996566b 增加的用例语义不变（它们验证种子与元数据保留，不经过投影）。

---

## 5. 附 A：池路由明细（worklist 任务 2(b) 的直接答案）

见 §1.2 表格。补充两点：

- `reuse_key` 的回退（`review_session_setup.rs:64-73`）：非 ThreadOwned 且 `parent_compaction.is_none()` 时沿用旧 trunk 的 `parent_history_version`——为的是"无可解密摘要时，既有 reviewer 可能持有父历史仅存的授权"。本案（有 checkpoint）不触发。
- ephemeral 的 busy-fork snapshot（`guardian_checkpoint.rs:11-42`）以 `RolloutItem::Compacted`（带 replacement_history=复核自身历史、guardian_history、window 元数据）为种子，经 `select_input_compaction`（`rollout_reconstruction.rs:61-88`）作为重播边界——这条路径的 Compacted item 是**复核自己的**，其 replacement_history 里的父 checkpoint envelope（若有）同样在投影处受同样约束。

## 6. 附 B：R1b 待做① —— Luna post-answer 二次采样的触发条件（代码事实）

**触发点不存在"答题钩子"**：二次采样不是由用户提交答案直接触发的，而是由**下一个被打分工具真正开始执行**触发：

- 唯一入口：`ext/guardian-v2/src/async_scorer/extension.rs:172-175` `on_tool_start` → `score_tool`（`observation.rs:39-364`），由 core 在**工具开始执行时**派发：MCP 工具 `core/src/tools/handlers/mcp.rs:221`、内建工具 `core/src/tools/registry.rs:681`（`notify_tool_start`，`core/src/tools/lifecycle.rs:21-60`）——都在**审批通过之后**。
- 测试中的两次采样：Luna #0 = `request_user_input` 调用执行时的分类；Luna #1（"post-answer"）= 答案落盘后 `inspect-1` 开始执行时的分类（此时 retained context 已含 VerifiedAnswer → transcript 渲染出 `>>> TRUSTED USER ANSWERS START`）。

**on_tool_start 是该采样路径的触发入口，不是请求必发的充分条件。** 答案后没有相应工具开始可解释无采样；工具已开始后也可能早退。实际 TIMEOUT/竞态仍待受控时序验证，候选路径如下：

1. 审批链把工具拒掉（不执行 → 无 tool start）：
   - `ext/guardian-v2/src/async_scorer/approval.rs:129-255` `cached_evidence`：缓存放行（232-241 行）→ 执行；`MissingScore`/`StaleScore`（lag > `max_tool_call_lag`，218-227 行）/`ElevatedRisk`（243-245 行）/`AuthorizationChanged`（233-238 行）→ 转同步复核（122 行）；**同步复核 deny 或失败 → 工具不执行**。
2. `score_tool` 自身的早退（有 tool start 但无请求）：
   - `observation.rs:45-53`：sampler/config/progress 缺失（桥接 provider 会移除，`extension.rs:76-93`——R1 已证的场景）；
   - `observation.rs:222-249`：`select_parent_compaction` 报错（Unusable/Oversized/RequiresSync）→ 直接 return，**不发任何 Luna 请求**（压缩后不兼容 → 设计上"无 checkpoint 不做 async 兜底"）；
   - `sampler.rs:168-172`（IncompatibleCompaction）、`sampler.rs:227-229`（InputTooLarge 转 sync）；
   - `sampler/execution.rs:159-175` + `retry_after_failure`（45-110 行，重试上限 `MAX_SAMPLING_RETRIES=2`）：连接租约失败耗尽 → fail-closed，请求可能从未上线。
3. **竞态候选（与 worklist 记录吻合）**：`classification.rs:315-319`——Luna #0 的（被 mock 门住的）响应在答题后放行时，`ScoreAuthorization::current` 复查发现授权已变 → `Superseded`，分数**不发布**（321/333-335 行）。发布规则本身在 `score.rs:101-121`（严格更新者胜）与 123-142 行（fail-closed 平局获胜）。这些只影响"审批走哪条路"，不直接决定请求是否发出，但会改变同步复核的介入与结果，间接决定工具是否执行。
4. 分数缓存复用语义（`approval.rs:215-216` 注释）：最新线程分数可在 lag 限额内跨工具类别、且在当前动作的 async 分数**仍在途时**复用——若命中缓存放行，工具执行、#1 仍会发出；若缓存判 ElevatedRisk/Stale 走同步且同步拒掉，则 #1 永不发出。

对挂起态（TIMEOUT 60s，采样栈显示 app-server stdin `read()` 空闲）的最小解释：**turn 已完成但答案之后没有被打分工具执行**（即 inspect 调用被审批链拒绝或未发生）。要在下一次复现时定案，需在挂起时补采 `review_requests.len()` 与 `parent_requests.len()`（本测试的诊断增强只打了 sync_input 与 parent_input_types，恰好缺这两个计数）。

## 7. 附 C：与上游对齐注记（worklist 待做③相关）

`project_input` 整套投影为 fork 独有（origin/main 无 `model_output_projection.rs`）。上游语义中复核请求**不做**跨模型不透明载荷剥离（checkpoint 由 `parent_compaction()` 选择、后端校验）。因此 R1b 13 项是 fork 的 Rig/凭证域加固工作对 guardian 复核场景的**回归**，不是上游缺陷；修复方向应保持 fork 的隔离设计并给 guardian 显式通道（§4），而非删除投影。
