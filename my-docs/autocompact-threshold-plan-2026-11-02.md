# 自动压缩阈值默认 90%→80% 与显式值解封 — 实施方案(Plan)

日期:2026-11-02 · 状态:**已实施**(用户拍板 2026-11-02,方案 B;随 v0.18.25 发布)
关联:overflow-autocompact-plan-2026-11-02.md(§3.1/§3.2 两面修复已实施)、
vendor-model-catalog-plan-2026-11-02.md §4(窗口优先级哲学同构)、台账 §6.73

---

## 1. 背景与分析

### 1.1 现状:90% 是"默认值"也是"封顶"

自动压缩阈值的唯一推导点在 `protocol/src/openai_models.rs:529`:

```rust
pub fn auto_compact_token_limit(&self) -> Option<i64> {
    let context_limit = resolved_context_window().map(|w| w * 9 / 10);   // 90%
    let config_limit = self.auto_compact_token_limit;                     // 显式值
    if let Some(context_limit) = context_limit {
        return Some(config_limit.map_or(context_limit, |l| min(l, context_limit)));
    }
    config_limit
}
```

三层语义:

| 配置状态 | 生效阈值 | 说明 |
|---|---|---|
| 未配置 | 90% × 窗口(派生) | fallback 与未带显式值的目录模型 |
| 显式绝对值/ratio(≤90%) | 显式值 | `NUWAX_AUTO_COMPACT_TOKEN_LIMIT` / `model_auto_compact_ratio`(ratio 在 models-manager 层先折算成绝对值再进来) |
| **显式值 > 90%** | **被静默钳到 90%** | **暗病**:用户设 950k 实际生效 900k,无任何提示,违反 fail-fast |

### 1.2 为什么 80% 更好(两个实测依据)

1. **三方网关的输出预留吃掉了余量**。2026-11-02 GLM 探针:窗口 1M,网关在
   ~950-980k 处拒绝(服务端为输出留了预留)。90% 触发线(900k)距拒绝线只剩
   50-80k——一轮较大的工具输出/图片/思考累积即可穿过,落到事后恢复路径
   (当轮失败、压缩、重试)。80%(800k)把余量翻倍到 ~150-180k,把"事后救"
   压回偶发。
2. **压缩请求自身的存活**。压缩请求携带全量历史,触发时 usage≈阈值。梯子实测
   (§6.73)中 900k 触发的压缩请求**自己**被网关 1261 拒绝,靠 trim 丢最老条目
   自愈("compacting; removing oldest history item")——过程正确但浪费且丢史。
   80% 触发时压缩请求 ~800k,一次通过率显著提高。

代价:压缩频率从每 0.9W 一次变每 0.8W 一次(**+12.5%**),每次压缩损失部分
早期细节、多一次窗口读成本。对 flash 级价格与摘要保要点的现状,可接受。

### 1.3 方案取舍

- **方案 A(最简)**:派生 `9/10 → 8/10`,保留 `min()` 封顶语义。
  问题:官方目录模型的显式值(多为 ~90%)会被一并压到 80%——改了 upstream
  对 gpt-\* 的既定压缩点,违背"不削弱官方目录"取向。
- **方案 B(推荐)**:**派生默认降为 80% + 显式值权威不封顶**。
  - 无显式值:90% → 80%(三方模型,fork 主场景);
  - 有显式值(目录或用户):直接生效,**不再被 90% 封顶**——官方目录模型
    显式值全部 ≤90%,**行为零变化**;用户 >90% 的配置从"静默钳"变"生效",
    顺手修掉 §1.1 的暗病;
  - 与 vendor 目录 Spec §4 的"用户显式优先、告警不钳制"哲学同构,目录落地后
    无需二次返工。

## 2. 改动内容

### 2.1 核心:`protocol/src/openai_models.rs::auto_compact_token_limit`

```rust
pub fn auto_compact_token_limit(&self) -> Option<i64> {
    // Fork (nuwax-codex): the derived default is 80% of the resolved window,
    // not upstream's 90%. Third-party gateways reserve part of the window for
    // output (probe 2026-11-02: a 1M-window gateway rejected at ~950-980k),
    // and the compaction request itself carries the full history, so a 90%
    // trigger leaves too little room for either. An explicit limit — catalog
    // or user configuration — is authoritative and no longer capped by the
    // derived ratio: clamping it silently contradicted fail-fast, and every
    // catalog value is ≤90% anyway, so official models keep their behavior.
    match self.auto_compact_token_limit {
        Some(explicit) => Some(explicit),
        None => self.resolved_context_window().map(|w| (w * 8) / 10),
    }
}
```

### 2.2 护栏告警(显式值高到可能永不触发常规压缩时)

显式值 ≥ `resolved_context_window × effective_context_window_percent`
(即越过 full-window 强制线,如 1M 窗口的 950k)时,models-manager 覆盖点
`warn!` 一次:点名配置值、窗口、生效的强制线,提示"常规压缩将不会先于
full-window 兜底触发"。不钳制、不报错,只可观测。

### 2.3 涉及文件

| 文件 | 改动 |
|---|---|
| `protocol/src/openai_models.rs` | §2.1(推导点,~10 行含注释) |
| `models-manager/src/model_info.rs` | §2.2 告警(~15 行) |
| `protocol` 单测 | §3.1 |
| `FORK.md` §10 | 新行:默认 80% + 显式解封,含两个实测依据 |
| `NUWAX-HARNESS.md` §4 | 默认策略 90%→80%;显式值不再被 90% 封顶(用户可见:>90% 的配置从此生效) |
| ACP `usage-guide.md` | §1.3/§2.3 窗口/压缩行同步一句 |

## 3. 测试计划(cargo nextest)

1. **单测(核心三态)**:`auto_compact_token_limit()`——
   - 无显式值 + 窗口 1M → 800_000(新默认);
   - 显式 360k(≤90%)→ 360_000(与旧行为一致);
   - 显式 950k(>90%)→ 950_000(旧行为 900_000,新行为不封顶——**断言旧值
     不再出现**);
   - 窗口未知 + 显式值 → 显式值(维持)。
2. **告警单测**:显式 960k + 1M 窗口 → 触发 §2.2 告警(断言日志或抽成纯函数
   断言返回值)。
3. **既有测试梳理**:grep 依赖 90% 派生的断言(如 `Some(360_000)` 这类恰好等于
   90% 的用例),逐个按新语义更新并目视核对;显式值用例(200_000 等)不受影响。
4. **集成回归**:compact 套件 `-j 8` 全量(104+ 用例)确认无意外翻转。
5. **e2e 真网(发版前)**:复用 §6.73 的 GLM 多轮梯子——预期压缩在 ~800k 触发、
   **压缩请求一次通过**(输出中不再出现 "removing oldest history item")、
   全程零 1261。

## 4. 兼容性

- 显式配置用户(值 ≤90%):行为不变;
- 未配置用户:压缩点 90%→80%(更安全方向;会话细节保留略减,文档明示);
- 显式 >90% 用户:从"静默钳到 90%"变"按配置生效"——严格更符合配置预期;
- 官方 gpt-\* 目录模型:零变化(显式值均 ≤90%);
- 与已实施的溢出两面修复的关系:本变更把**常规**压缩点提前,让 §3.1 事后
  恢复与 trim 自愈从"经常兜底"回到"偶发兜底";pending 投影(full-window 线)
  不受影响。

## 5. 发布

改动小,建议并入待发的 v0.18.25(其 release run 尚未定稿重发;与遥测修复
同批),一次发版携带:溢出两面修复 + 品牌清扫 + 遥测默认关 + 更新通道收敛 +
本阈值调整。

## 6. 非目标

- `effective_context_window_percent`(fallback 95%)与 full-window 强制线的调整
  ——那是另一条线(网关预留的精确刻画),留给 vendor 目录 Spec 的 overlay
  按厂商配置;
- post-turn 异步压缩阈值百分比的行为(其相对基准是压缩上限,随本变更自动
  前移,无需单独处理);
- ratio 默认值改动(`NUWAX_AUTO_COMPACT_RATIO` 未设时才走派生,派生即本变更)。
