# 采样阶段上下文溢出接入自动压缩 — 实施方案(Plan)

日期:2026-11-02 · 状态:待评审(2026-11-02 用户决策:轮前 pending-input 估算并入本次,不延后)
上游文档:FORK.md §10(行为差异)、台账 §6.71/§6.72(effort 档位线)
关联实证:本文件 §1 的两组实验(2026-11-02,本机 nuwax-harness + GLM 实网)

---

## 1. 问题与证据

用户可感知症状:长会话中直接报
`Codex ran out of room in the model's context window. Start a new thread or clear earlier history before retrying.`,
会话废掉,**没有任何自动压缩**——与「溢出应自动压缩续命」的预期不符。

### 1.1 实验实证(2026-11-02,GLM-5.3-Flash,Anthropic 线,窗口配 1M)

每轮注入 ~104 万字符(≈16.3 万 GLM 实际 token)逐步堆叠:

| 累计上下文(GLM 实际 token) | 结果 |
|---|---|
| 17.2 万 | ✅ |
| 81.6 万(第 5 轮后) | ✅ |
| ~98 万(第 6 轮) | ❌ 网关 400:`{"code":"1261","message":"[1261][prompt is too long]"}` |

- **GLM-5.3-Flash 真实有效窗口 ≈ 1M token**(扣输出预留后输入可用 ~95 万);
  `model_context_window = 1000000` 是正确配置。
- 网关拒绝体被 fork 的映射规则识别(`codex-api/src/api_bridge.rs:125-131`,
  marker 表在 `:377`)转为 `CodexErr::ContextWindowExceeded` → 上述硬错误。
- 另一路径:桥层字节预算闸门(`codex-rust-rig-bridge/src/wire_budget.rs:73`,
  `字节/4` 估算 vs 完整窗口)在发送前拒绝,同样映射为该错误(实验 A 第 3 轮触发)。

### 1.2 根因(三层,均已实证)

1. **事前没防住(阈值跳步)**:轮前主动压缩按「上一轮已报用量 vs 窗口 90%」判断
   (turn.rs 循环 + `context_window.rs` 的 `token_limit_reached`)。第 5 轮后用量
   81.6 万 < 90 万 → 不压缩;第 6 轮一次加 16.3 万 + 输出,直接跳到 ~98 万,
   一步跨过 90 万阈值与网关真实上限。检查点在轮前,看不到本轮待加入的输入
   (upstream 自己留了 TODO:`turn.rs:192` 附近 "Estimate pending incoming items")。
2. **事后没救回来(恢复缺接线)**:turn 循环里存在「收到 ContextWindowExceeded →
   自动压缩 → 重试一次」的恢复臂(`turn.rs:837-867`),但守卫要求错误带
   `ExhaustedReviewBudget` 标记——**只有 guardian 本地预算检查会插这个标记**
   (`request_budget.rs:109/:165`);网关映射(`api_bridge.rs:130`)与桥层闸门
   (`wire_budget.rs:104`)都不插,于是落到 `:894` 的通用错误臂 → 硬报错。
   讽刺:`api_bridge.rs:118-124` 的注释明说加映射就是为了让压缩路径能触发——
   意图在,采样阶段的接线没做完。
3. **次生恶化(失败轮残留)**:失败轮的用户输入已先行记录进历史
   (`run_hooks_and_record_inputs`),下一轮历史更大,恶性循环。
   (恢复在同轮内完成后,此问题自然消失,无需单独处理。)

---

## 2. 目标与非目标

**目标(事前 + 事后两面一起修,2026-11-02 用户决策)**:

1. **事后(恢复)**:采样阶段(含工具续步)收到 `ContextWindowExceeded`——无论来自
   网关拒绝、桥层闸门还是 guardian 本地检查——在自动压缩可用时,统一执行
   「压缩一次 → 重试一次」,替代硬报错。
2. **事前(防跳步)**:轮前压缩判断纳入**本轮待加入输入**的估算,防止单轮大输入
   一步跨过 90% 阈值与网关真实上限(§1.2-1 的实测路径)。

**非目标(本次不做)**:
- 桥层字节估算精度提升(重复性内容高估 ~1.5×;两面修复落地后误报可自愈,见 §6);
- 失败轮输入从历史回滚(§1.2-3 随核心修复自然消失);
- 该错误消息的品牌替换("Codex ran out of room" → nuwax-harness,独立小改,
  可随本次或单独做)。

---

## 3. 改动方案

### 3.1 核心改动:`codex-rs/core/src/session/turn.rs`

在 `:837` 的 guardian 恢复臂**之后**、`:868` TurnAborted 臂之前,新增一个平行
match 臂,覆盖「不带 guardian 标记」的采样阶段溢出:

```rust
Err(err)
    if auto_summarization_enabled
        && matches!(err.details(), CodexErrorDetails::ContextWindowExceeded)
        && !guardian_budget_compacted
        && sess
            .services
            .thread_extension_data
            .get::<crate::guardian::ExhaustedReviewBudget>()
            .is_none() =>
{
    // 采样阶段的溢出也可能来自网关拒绝(fork 在 api_bridge 映射厂商
    // 错误体,如 GLM 1261 "prompt is too long")或桥层字节预算闸门
    // (wire_budget),两者都不带 guardian 标记。同样的语义:压缩一次、
    // 重试一次,有界——失败即原样报错,不循环。
    guardian_budget_compacted = true;
    Box::pin(run_auto_compact(
        &sess,
        Arc::clone(&step_context),
        Arc::clone(&step_context),
        &mut client_session,
        CompactionReason::ContextLimit,
        CompactionPhase::MidTurn,
        /*candidate*/ None,
    ))
    .await?;
    can_drain_pending_input = false;
    continue;
}
```

设计要点:

- **设计意图:fork 没有自家模型,网关拒绝是常态错误源,所有来源一律平等。**
  错误来源枚举:guardian 本地预算检查(带标记)、桥层字节闸门(无标记)、网关
  拒绝映射(无标记;官方 SSE 的 `context_length_exceeded` 与三方 HTTP 错误体
  如 GLM 1261 两条线,后者是 fork 自己补的映射)。`api_bridge.rs:118` 的分类
  映射是这项接线的**前半截**(让错误可被识别为溢出),本臂补**后半截**(让识别
  结果触发恢复)。upstream 的"自家检查器带标记才恢复"是它隐含官方模型前提的
  残留,对换模型的 fork 用户不成立——这正是本臂存在的理由。

- **不改原 guardian 臂**:它带着 reviewer 的 fail-closed 语义(插
  `ExhaustedReviewBudget::Compacting`、退休 reviewer)。新臂不插该标记——
  非 guardian 来源没有 reviewer 可退休,插入反而误导。
- **复用 `guardian_budget_compacted` 标志**(`:462` 声明、`:584` 每次成功采样后
  复位):天然获得「每步最多压缩一次」的有界性,两臂互斥(Marker 有→旧臂,
  无→新臂),不会双重压缩。若评审认为标志语义易混淆,可改名
  `overflow_compacted`(纯重命名,行为等价)。
- **前置条件沿用 `auto_summarization_enabled`**(`:189`):`AutoCompactionDisabled`
  标记(如 `new_context_window` 工具设置)或 `TokenBudget` feature 开启时,
  保持现状硬错误——压缩被显式禁用就不该偷偷压。
- **压缩失败即终止**:`run_auto_compact` 的 `?` 传播(含 compact 内部
  `compact.rs:374` 对单条超大历史 trim 到 `turn_input_len<=1` 后的报错),
  与旧臂语义一致;无循环风险。

### 3.2 轮前 pending-input 估算(事前防跳步,upstream TODO 的落地)

现状:`run_pre_sampling_compact`(`turn.rs:1381`)按 `context_window_token_status`
的**已报用量**判断 `token_limit_reached`(:1394),而它在输入记录**之前**被调用
(:203-210)——本轮待加入的用户输入不在判断里,这就是 §1.2-1 的「一步跳过」。
upstream 自己留了 TODO(`:199-202`:_estimate pending incoming items and trigger
compaction preemptively_)。

改动:

1. **估算**:`run_pre_sampling_compact` 增加 `pending_input_tokens: i64` 参数,
   由调用方在 `:203` 处对本轮用户输入的 items 逐项用
   `context_manager::estimate_item_token_count`(guardian 闸门同款估算器,
   图片走同一粗估)求和;文本为空/无 items 时为 0,行为与现状一致。
2. **判定**(实现收敛,2026-11-02 测试反馈后修订):pending **只对齐完整窗口
   上限** `full_context_window_limit`(`ContextWindowTokenStatus::pending_input_
   reaches_limit`),**不叠加**操作者软阈值 `auto_compact_scope_limit`——软阈值
   被单轮跨过只会把压缩延迟到下一轮边界检查(无害),真实窗口被跨过才是网关
   拒绝的致死线;且不设显式软阈值时派生阈值=完整窗口,常见配置两道同时守住。
   软阈值不叠加减去了对既有迷你阈值测试的全量翻新。**不写回报用量**。
   命中即走既有 `run_auto_compact(ContextLimit, PreTurn, ..)`——不新增压缩路径,
   只修正触发条件。窗口未配置(未知模型且未显式设 window)时该机制不生效,
   由 §3.1 事后恢复兜底。
3. **只估用户输入,不估 context 差量**(environment/AGENTS/工具表刷新):
   实测跳步的主项就是用户输入(§1.1 每轮 16.3 万 token);context 差量小且
   在更晚的 `capture_step_context` 才成形,强估会扩大改动面。估算不足的残余
   由 §3.1 的事后恢复兜底——两层互为安全网,这是两面一起做的理由。
4. **语义**:估算只影响「要不要提前压缩」,不会拒绝输入;过量触发最多多压一次
   (费用一次压缩调用 + 摘要损失些许细节),不会恶化正确性。压缩失败走既有
   预轮失败路径(:212-242,输入保全)。
5. **已报用量依赖**:判定基于网关回报的 usage;网关不回报 usage 时该机制与现状
   一样失效(不是回归)。

### 3.3 实现时验证点(不改行为,只确认假设)

1. `continue` 后步循环从**压缩后的历史**重建 prompt,且本轮新输入已在历史中
   (guardian 臂依赖同样语义;实现时以集成测试为准)。
2. `:1818-1821` 的早退臂先执行了 `set_total_tokens_full`(`session/mod.rs:5408`,
   仅置 usage 显示为满窗并发 token 事件)——确认压缩成功重试后,下一个成功响应
   的 usage 会覆盖该显示,不持久化脏值。
3. app-server / ACP 路径走同一 core turn 循环,自动受益;ACP 仓库零代码改动。

### 3.4 涉及文件

| 文件 | 改动 |
|---|---|
| `core/src/session/turn.rs` | §3.1 新增 match 臂(~30-40 行);§3.4 `run_pre_sampling_compact` 传入 pending 估算 + 调用点估算(~30 行) |
| `core/src/session/context_window.rs` | §3.2 `context_window_token_status*` 接受 pending 项并纳入触达判定(~15 行) |
| `core/tests/suite/compact.rs`(或新文件) | 集成测试(§4) |
| `FORK.md` §10 | 新行:上下文溢出两面行为(事前 pending 估算触发预轮压缩;事后采样溢出压缩重试一次) |
| `NUWAX-HARNESS.md` | 该错误排查行语义更新 |
| 台账 §6.73 | 记录根因、实验、修复 |
| `fork-codex-acp-ts/my-docs/usage-guide.md` | 排查行同步 + GLM 窗口示例 128000→1000000(§1.1 实证值) |

---

## 4. 测试计划(cargo nextest;Mac 跑 core 集成需 `RUST_MIN_STACK=33554432`)

**事后恢复(§3.1)**:

1. **主路径(网关拒绝→压缩→重试成功)**:本地 mock Anthropic 网关,按请求体
   大小阈值返回 400 + GLM 1261 体(`{"code":"1261","message":"[1261][prompt is too long]"}`);
   压缩后的请求显著变小 → 200 正常回复。断言:turn 正常完成、恰好发生一次压缩、
   用户收到回复、无错误事件。
2. **桥层闸门路径**:不 mock 网关,而是把 `model_context_window` 配小,让
   `wire_budget` 在发送前拒绝(本地服务器断言零请求);压缩后重试成功。
   ——证明两条来源都接上了同一恢复。
3. **有界性反例**:网关对一切请求(含压缩请求)都回 1261 → 最终错误 surfaced,
   且压缩恰好一次(mock 请求计数断言),无重试风暴。
4. **禁用反例**:注入 `AutoCompactionDisabled`(或开 `TokenBudget` feature)→
   行为与现状一致:硬错误、零压缩。
5. **单测**:match 路由三态(marker 有/无、本步已压缩过)。

**事前防跳步(§3.2)**:

6. **跳步场景**:mock 用量回报把已报用量钉在阈值下(如窗口 1M、用量 81.6 万),
   本轮注入 16.3 万 token 的用户输入(估算后会越限)→ 断言**预轮压缩发生**
   (服务器先收到压缩请求、再收到采样请求),全程无 1261、无错误事件。
7. **不过度触发反例**:已报用量 + pending 仍在阈值内 → 零压缩(防过度积极回归)。
8. **含图输入**:pending 带图片时估算路径不 panic、不全为零(粗估即可)。

**通用**:

9. 既有 `compact.rs:6135` 附近断言如受行为变化影响(无 marker 溢出从报错变压缩
   成功),按新语义更新并目视核对 diff。
10. **真网验收(发版前)**:复用 §1.1 的多轮堆叠脚本(6 轮 × 16.3 万 token)
    对 GLM 实网重放——预期第 5→6 轮之间发生预轮压缩、全部轮次成功,
    不再出现 1261。
11. **压缩请求自身被拒的端到端自愈**:历史超限、且 mock 网关对首个压缩请求
    也回 1261(压缩请求含全量历史,理应更胖)→ 断言 `compact.rs:374` 的
    trim 循环被触发(丢弃最老条目重试)、最终压缩成功、turn 成功。这条链路
    (fork 映射 → upstream trim 自愈)从未在三方场景端到端验证过,本测试钉住。

---

## 5. 发布路径

- harness `v0.18.25`(tag → release.yml → npm);
- ACP `v1.2.25`:仅 pin 升级 0.18.25,无代码变化;
- 均沿用既有发版与隔离冒烟流程(含本轮学到的:验 npm 以 registry 直查为准)。

---

## 6. 风险与兼容

| 风险 | 评估 |
|---|---|
| 每次溢出多一次压缩调用(模型费用) | 仅异常路径发生;压缩本来就在设计内,可接受 |
| **事前估算过度积极**(估算偏高时提前压缩:多一次压缩调用 + 摘要损失些许细节) | 只影响「何时压」不影响正确性;实测主项是用户输入且估算器与 guardian 闸门同源;过度触发反例测试(§4-7)钉住 |
| **事前估算不足**(CJK/重复内容比率偏差、context 差量未估) | 事后恢复(§3.1)兜底——两面互为安全网,这正是并入实施的理由 |
| 桥层估算误报(重复性内容高估 ~1.5×)先触发压缩后仍失败 | 有界:最多多压一次然后原样报错,不劣于现状;估算精度列为独立后续项 |
| 官方 OpenAI 线(native/responses)行为同步变化 | 其 context 错误同样会压缩重试——语义一致化,记入 FORK.md,风险低 |
| upstream 合并冲突 | turn.rs 新增 match 臂 + run_pre_sampling_compact 参数与 context_window.rs 判定扩展;TODO 处正是 upstream 自己想改的地方,冲突可控 |
| 与 guardian 语义互相干扰 | 两臂互斥(marker 有无),复用同一有界标志;不改 guardian fail-closed 路径 |

---

## 7. 后续项(非本次)

1. 桥层 `approx_tokens_from_byte_count` 精度(如对 CJK/重复内容的比率修正),
   或改为可配置估算参数——遵守「不做黑盒兼容」原则,须显式可观测。
2. 错误消息品牌替换("Codex ran out of room" → nuwax-harness),连同技能描述里
   7 处 codex 残留(另案,用户已知晓)。

---

## 8. 工作量估计

- 核心改动:事后恢复臂 ~30-40 行;事前估算 ~45 行(turn.rs + context_window.rs)。
- 测试:~300 行(8 个集成/单测场景)。
- 文档:FORK.md/NUWAX-HARNESS.md/台账/usage-guide 各一小节。
- 节奏:一个代码提交(+测试)→ 全量回归(fmt/clippy/nextest)→ 真网验收
  (§4-10 多轮堆叠)→ 文档提交 → 发版 0.18.25 + ACP 1.2.25 → 隔离冒烟。
