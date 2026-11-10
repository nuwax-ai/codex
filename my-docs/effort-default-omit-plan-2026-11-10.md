# thinking / reasoning_effort 默认不传 — 实施方案

| 项 | 值 |
|---|---|
| 日期 | 2026-11-10 |
| 状态 | **已实施并本地验证**（2026-11-10；核实结论见 §11，实施差异见 §12） |
| 作者 | soddygo（DSH 会话） |
| 涉及仓库 | `fork-nuwax-codex`（引擎）、`fork-codex-acp-ts`（适配器）、`nuwax-client`（客户端清理） |
| 关联 | 图片治理方案（P0/P1）、413 日志与 32MiB→512MB 同窗口期；本文档只管 effort/thinking |

---

## 1. 背景

### 1.1 事故

2026-11-10 本地测试，provider 配智谱官方 OpenAI 协议 + `glm-5.3-flash`，请求直接失败：

> `the effort tiers declared for model glm-5.3-flash (vendor catalog overlay; provider may
> override or set ignore_catalog_effort_levels = true) do not include medium; this model
> takes low, high, max`

（来源：`nuwax-client/logs/electron-dev.log` 13:32 段；同形态报错在 UI 截图中复现。）

### 1.2 根因链（已代码核实）

1. 智谱 GLM-5.3 家族只接受 `low/high/max` 三档，**没有 `medium`**（官方文档，见 §2）；
2. 引擎的档位门（vendor catalog overlay 声明表）正确识别并 fail-fast——**门本身没错**；
3. 但请求里被塞进了 `medium`：注入者是 **ACP 适配器的 `fallbackEffort()` 硬编码兜底
   `"medium"`**（`fork-codex-acp-ts/src/CodexAcpClient.ts:1283`：env → 声明 levels[0] →
   `"medium"`）。适配器把 medium 写进 session 配置，引擎请求即带 `reasoning.effort=medium`，
   发送前被门禁拒。
4. 当前 workaround：客户端对 GLM-5.3 播种 `NUWAX_MODEL_REASONING_EFFORT=low`
   （`nuwax-client/.../acp/acpClient.ts:989`（anthropic 分支）与 `:1014`（openai 分支））。
   它能修好错误，但只是把"猜错的值"换成另一个猜的值，且策略散落在客户端按模型名判断。

### 1.3 产品诉求（用户原话归纳）

- **用户没有设置、环境变量/后端也没有设置时，默认不传** thinking 开关与 effort 档位，
  由模型厂商按自己的默认决定（智谱官方对 GLM-5.3 的默认是 `max`）；
- 用户显式设置时照发，且非法组合 fail-fast 点名；
- 不引入"客户端按模型名猜档位"这类第二策略源。

### 1.4 为什么值得做

- 结构性消除 medium 类错误（不依赖客户端播种）；
- 线上网关/模型持续增加，"引擎猜一个默认档"的模式不可持续；
- 与图片治理同窗口期发布引擎新版本，顺带合并评审。

---

## 2. 官方协议依据（链接 + 原文摘录）

> 评审 agent 请以这些链接的**当前内容**为准复核；若文档已变更，以变更后语义重审本方案。

### 2.1 OpenAI

| 主题 | 结论 | 链接 |
|---|---|---|
| Chat Completions / Responses 的 effort 参数 | **非必填**；省略 = 模型默认。Responses 指南原文："如果省略 `reasoning.effort`，GPT-5.6 在两种模式下都默认使用 `medium`" | [对话状态指南（api-mode=chat）](https://developers.openai.com/zh-Hans/api/docs/guides/conversation-state?api-mode=chat) ／ [推理指南](https://developers.openai.com/zh-Hans/api/docs/guides/reasoning) |
| effort 取值域 | 因模型而异：`none/minimal/low/medium/high/xhigh/max`；`none` 在部分模型被拒（GPT-6 Astra 传 none 返回 400） | [推理指南](https://developers.openai.com/zh-Hans/api/docs/guides/reasoning) |
| chat 无服务端状态 | "每个文本生成请求都是独立且无状态的"，多轮需手动回传全部 messages（图片治理方案的背景，此处仅引用） | [对话状态指南](https://developers.openai.com/zh-Hans/api/docs/guides/conversation-state?api-mode=chat) |
| 请求体上限 | 单请求 payload ≤ 512 MB、≤1500 张图（32MiB 闸门整改的依据，另行方案） | [视觉指南](https://developers.openai.com/zh-Hans/api/docs/guides/images-vision?api-mode=chat) |

### 2.2 Anthropic

| 主题 | 结论 | 链接 |
|---|---|---|
| thinking 默认值 | **现代模型默认开启**。thinking 总览原文："**Messages API: On by default.** `thinking: {"type": "disabled"}` turns it off on models that accept that value. Where it is off by default, set `thinking` to turn it on." | [thinking 总览](https://docs.anthropic.com/en/docs/build-with-claude/thinking) |
| 手动模式已废弃 | "Extended thinking (`thinking.type: "enabled"` with `budget_tokens`) is deprecated. Claude 4.7 and later models do not support it and reject requests that use it, returning a 400 error." | [extended thinking](https://docs.anthropic.com/en/docs/build-with-claude/extended-thinking) |
| effort 档位 | 新模型 `output_config.effort`，省略 = 模型默认（Sonnet 4.6 默认 high、Opus/Haiku 5.5 默认 medium） | [thinking 总览](https://docs.anthropic.com/en/docs/build-with-claude/thinking) |
| disabled 的支持度 | 多数新模型（Fable/Mythos/Opus 5.5 等）**拒绝** `disabled`；Sonnet 5.5 最低档是 `between_tools` | [thinking 总览](https://docs.anthropic.com/en/docs/build-with-claude/thinking) |

**对本方案的含义**：Anthropic 线上"省略 thinking = 开（现代模型）"，与 OpenAI 系一致；
"必须显式传 enabled"是 2024 时代语义且在现代模型上会 400——**两轴都不要按 wire 一刀切**。

### 2.3 智谱（GLM）

| 主题 | 结论 | 链接 |
|---|---|---|
| GLM-5.3 思考参数 | "会始终启用思考功能……并不再支持禁用思考功能"；`thinking.type` 仅支持 `enabled`；`reasoning_effort` 取值 `low/high/max`，**默认 `max`**；用 `disabled` 请求将失败 | [GLM-5.3 模型文档](https://docs.bigmodel.cn/cn/guide/models/text/glm-5.3) |
| 官方 Codex 集成配置 | `wire_api` 必须 `responses`；官方 models.json 给 GLM-5.3 配 `default_reasoning_level: "max"`、levels `low/high/max` | [智谱 Codex 接入文档](https://docs.bigmodel.cn/cn/coding-plan/tool/codex) |

---

## 3. 现状代码事实（均已核实，file:line 可复查）

### 3.1 引擎已经原生支持"不传"

| 事实 | 位置 |
|---|---|
| chat wire：`if let Some(effort)` 才插入 `reasoning_effort` 键——None 时键直接省略 | `codex-rs/codex-rust-rig-bridge/src/convert_request.rs:138` |
| anthropic wire：effort 走 rig 类型化通道 `rig_reasoning(...)`，None → 不发 `output_config.effort`/`thinking` | `codex-rs/codex-rust-rig-bridge/src/convert_request.rs:213` |
| 档位门对 None **放行**（`let Some(effort) = requested else { return Ok(()) }`） | `codex-rs/codex-rust-rig-bridge/src/reasoning_effort.rs:128` |
| 档位门对显式 `none/off`（关闭思考）且网关"始终思考"时报错点名——fail-fast 机制已存在 | `codex-rs/codex-rust-rig-bridge/src/reasoning_effort.rs:143` |
| "none/off 不是档位而是开关轴"的两轴模型已是显式设计 | `codex-rs/codex-rust-rig-bridge/src/reasoning_effort.rs:170` |
| 档位表来源：provider 未声明时读 vendor catalog overlay 的 `levels` | `codex-rs/codex-rust-rig-bridge/src/stream.rs:464` |
| 发送前已记录 `has_reasoning_effort`（debug 日志，排障可用） | `codex-rs/codex-rust-rig-bridge/src/stream.rs:437` |
| 解析链：`effective_reasoning_effort = 显式设置.or(model_info.default_reasoning_level)`；**目录模型的 `default_reasoning_level` 本来就是 None** | `codex-rs/core/src/session/step_settings.rs:84` ／ `codex-rs/models-manager/src/vendor_catalog.rs:404` |
| 配置字段本就是可选：`model_reasoning_effort: Option<ReasoningEffort>` | `codex-rs/core/src/config/mod.rs:1148` |
| `ReasoningEffort` 枚举含 `None`（=关闭思考）与 `Custom`；`Medium` 是 `#[default]`（仅内置 OpenAI 模型路径使用） | `codex-rs/protocol/src/openai_models.rs:73-103` |
| rig-core 0.45 anthropic 映射：Effort → `output_config.effort`（+adaptive 模型的 `thinking:{type:adaptive}`）；Budget → `thinking:{enabled,budget_tokens}` | `~/.cargo/registry/.../rig-core-0.45.0/src/providers/anthropic/options.rs:60-90` |

### 3.2 medium 的注入点与死数据

| 事实 | 位置 |
|---|---|
| **medium 注入者**：适配器 `fallbackEffort()` = env → 声明 levels[0] → 硬编码 `"medium"`；`createModelId` 在模型不在列表时走它 | `fork-codex-acp-ts/src/CodexAcpClient.ts:1283`（`fallbackEffort`）、`:1296`（`createModelId`） |
| env 读取遵循 `NUWAX_`/`CODEX_` 别名约定 | `fork-codex-acp-ts/src/CodexEnvConfig.ts:294` |
| overlay 的 `default` 字段**已被解析但无任何消费者**（`effort_table_for` 只取 `levels`）——是死数据 | `codex-rs/models-manager/src/vendor_catalog.rs:162` ／ `codex-rs/models-manager/vendor/overlay.toml:12` |
| overlay 现值：`glm-5.3-flash` levels `[low,high,max]`、default `high`（探测值）、alias → `zai-coding-plan` | `codex-rs/models-manager/vendor/overlay.toml:12-22` |
| **约束**：overlay `default` **不得**接进 `model_info.default_reasoning_level`——那会进入 `effective_reasoning_effort` 变成发送值，违背"默认不传"。它的合法用途只有展示/探测记录 | 推导自 `step_settings.rs:84` |
| session 身份串强制 `model[effort]` 格式（无 effort 抛错）——"不传"时身份串需要单独设计 | `fork-codex-acp-ts/src/ModelId.ts:23` |
| 客户端当前 workaround（本方案要清理的两处） | `nuwax-client/nuwa-electron-shell/crates/agent-electron-client/src/main/services/engines/acp/acpClient.ts:989`、`:1014` |

---

## 4. 方案

### 4.1 适配器（主改，`fork-codex-acp-ts`，版本 1.2.32）

| # | 改动 | 验收标准 |
|---|---|---|
| A1 | `fallbackEffort()` 删除硬编码 `"medium"`：env → 声明 levels[0] → **undefined** | 未知模型无任何 env 时，session 配置不带 effort；请求体无 `reasoning_effort` |
| A2 | 身份串与发送值解耦：effort 未定时 `currentModelId` 用哨兵 `model[default]`（UI 渲染"厂商默认"）；`setSessionModel`/`fromString` 收到 `[default]` 映射回"未指定"，**永不把哨兵当档位下发**；`applyModelAndEffort` 的 effort 参数与身份串解耦 | `[default]` round-trip 不回灌；切模型/续会话不 invent 档位 |
| A3 | 显式关闭的 fail-fast：`isThinkingDisabled → none` 保留；目标模型不支持关闭（引擎模型列表无 none 档，如 GLM-5.3 家族）时启动/切档即报错点名"该模型始终思考，不支持关闭" | 不再把 `none` 发出去吃网关 400 |

### 4.2 引擎（小改 + 锁定测试，`fork-nuwax-codex`，版本 0.18.36）

| # | 改动 | 验收标准 |
|---|---|---|
| B1 | 补回归测试：effort=None → chat wire 无 `reasoning_effort` 键、anthropic wire 无 `output_config.effort`/`thinking`、档位门放行 | 新测试锁定现状，防回归 |
| B2 | overlay `default` 字段加注释："仅展示/探测值；禁止接入 `default_reasoning_level`（会变成发送值）" | 注释存在；无行为变化 |
| B3 | 显式 `none` + always-thinking 模型的 fail-fast 文案补一条测试（点名模型） | 文案含模型 slug |

### 4.3 客户端（清理，`nuwax-client`）

| # | 改动 | 说明 |
|---|---|---|
| C1 | 删除 `acpClient.ts:989`、`:1014` 两处 GLM-5.3 → `low` 播种 | medium bug 已结构性修复；保留"后端显式下发优先"逻辑。低成本默认若要保留，应由**后端统一下发**（单一策略源），不在客户端按模型名猜 |

### 4.4 行为总览（改完后）

```
用户/环境变量/后端均未设置
  → 两个 wire 都不传 reasoning_effort / thinking
  → GLM-5.3-flash 按其默认（探测 high）、glm-5.3 = max、现代 Claude = 自适应开
  → UI 显示「厂商默认」；身份串 model[default]
显式设置（含 none/off）
  → 照发；档位门校验；none 遇 always-thinking 模型 fail-fast 点名
后端/环境变量显式设置
  → 最高优先，照发（现有逻辑不变）
```

---

## 5. 测试矩阵

| 层 | 用例 | 期望 |
|---|---|---|
| 适配器单测 | 未知模型、无 env | 身份串无 invented effort；session config 不带 effort |
| 适配器单测 | `glm-5.3-flash`、无 env | 同上；无 medium |
| 适配器单测 | `[default]` 身份串 round-trip（setSessionModel 再取回） | 不回灌档位；状态不变 |
| 适配器单测 | env `NUWAX_MODEL_REASONING_EFFORT=low` | 照发 low |
| 适配器单测 | 显式关闭 + GLM-5.3 | 报错点名"始终思考" |
| 引擎单测 | effort=None，chat wire | 请求体无 `reasoning_effort` 键 |
| 引擎单测 | effort=None，anthropic wire | 无 `output_config.effort`、无 `thinking` |
| 引擎单测 | effort=Some(none) + GLM-5.3 档位表 | fail-fast，文案含 slug 与档位表 |
| 引擎既有 | `compact::tests`、`app-server-protocol`、schema fixtures | 全绿（防波及其他链） |
| 端到端（本地） | `glm-5.3-flash` chat 线裸配（无任何 effort env） | 日志确认请求体无 effort 键；GLM 按默认执行；无 medium 报错 |
| 端到端（本地） | 同一会话中途切换模型（deepseek-flash ↔ glm-5.3-flash） | 无 medium 报错、无"建议切回"类警告（该警告已按产品改为 log-only） |

---

## 6. 待决策项（需产品拍板）

| # | 决策 | 选项 | 建议 |
|---|---|---|---|
| D1 | 不传后的成本：flash 跑探测默认 `high`、glm-5.3 跑 `max`，compaction/guardian 等内部轮次也吃高档 | (a) 接受先观测 (b) 内部轮次单独降 `low` | (a)——内部轮次占比小，少一个旋钮少一份维护 |
| D2 | 低成本默认的归属：C1 移除播种后是否还要"默认低档" | (a) 接受厂商默认 (b) 后端统一下发 low | (b) 若需要——单一策略源，客户端不再按模型名猜 |
| D3 | 发布节奏：与图片治理（P0/P1 + 413 日志 + 32MiB→512MB）同版 0.18.36，还是本方案先发 | 同版 / 先发本方案 | **先发本方案**——改动小、风险低、可独立验证；图片组单独一版 |

---

## 7. 发布计划

| 步骤 | 内容 |
|---|---|
| 1 | 引擎 `0.18.36`：B1/B2/B3 + 测试 + FORK.md §5 + NUWAX-HARNESS.md |
| 2 | 适配器 `1.2.32`：A1/A2/A3 + 测试（**必须与引擎配套发布**：单发任一边都会回到 medium 或丢身份） |
| 3 | 客户端：C1 清理，随后续 beta（不阻塞引擎/适配器） |
| 4 | 本地验收（§5 端到端两行）通过后合入 `test` 分支打 tag |

回滚：引擎/适配器各为独立版本，回退 tag 即可；客户端 C1 是删除性改动， revert 单 commit。

---

## 8. 风险

| 风险 | 缓解 |
|---|---|
| 厂商默认档位变化导致行为漂移（同一 slug 不同网关默认不同） | UI 显示"厂商默认（探测值 X）"而非虚构档位；探测值记录在 overlay 注释 |
| 成本上升（D1） | 观测一周；必要时内部轮次降档 |
| 旧世代 Claude（≤4.6，省略=关闭思考）不在本产品模型范围 | 若将来支持，在 overlay 加 per-model thinking 能力声明，不做全局特例 |
| `[default]` 哨森被误当档位下发 | A2 的 round-trip 测试 + `applyModelAndEffort` 解耦审查 |

---

## 9. 给评审 agent 的核实清单

请逐条独立核实（不采信本文档结论），每项给出"属实/不属实/部分属实 + 证据"：

1. **协议层**：打开 §2 全部链接，确认当前文档仍支持"effort/thinking 非必填、省略=模型默认"；特别注意 Anthropic thinking 总览的 "On by default" 与 manual mode deprecated 两条是否仍成立。
2. **medium 注入点**：读 `fork-codex-acp-ts/src/CodexAcpClient.ts:1283`（`fallbackEffort`）与 `:1296`（`createModelId`），确认"无 env 无声明时兜底 medium"是唯一/主要注入路径；若还存在其它注入点（如引擎内置表、app-server 序列化），请指出。
3. **引擎省略路径**：读 `convert_request.rs:138` 与 `:213`、`reasoning_effort.rs:120-152`，确认 effort=None 时两个 wire 均不发键、档位门放行、显式 none 有 fail-fast。
4. **解析链**：读 `step_settings.rs:84` 与 `vendor_catalog.rs:404`，确认目录模型未设置时 `effective_reasoning_effort` 为 None（即引擎本来就会省略）。
5. **死数据与约束**：读 `vendor_catalog.rs:162` 与 `overlay.toml`，确认 `default` 字段无消费者；并评估"接入 `default_reasoning_level`"是否确实会使其变成发送值（若是，B2 的注释约束成立）。
6. **身份约束**：读 `ModelId.ts`，确认 `model[effort]` 格式约束与 A2 哨兵方案的必要性；检查 `setSessionModel`/`session/new`/resume 三条路径对哨兵的处理是否都被 A2 覆盖。
7. **客户端 workaround**：读 `nuwax-client/.../acpClient.ts:989`、`:1014`，确认删除后 GLM-5.3 在两协议分支都不再被播种 low，且无其它客户端侧 effort 注入。
8. **测试充分性**：审查 §5 矩阵是否覆盖"省略/显式/哨兵/回灌/跨模型切换"五条路径；指出遗漏用例。
9. **发布配套**：确认 A 组（适配器）与 B 组（引擎）是否必须同版本发布，还是可解耦。

---

## 10. 附：本方案明确**不**包含

- 图片 JPEG 归一 / 旧图降级（P0/P1，另案）；
- 413 可观测日志与字节观测上限（另案）；
- 32MiB → 512MB 闸门整改（另案）；
- 模型切换 resume 警告的用户可见化（已改为 log-only，另行提交）。


---

## 11. 独立核实结论（2026-11-10，按 §9 清单逐条）

| # | 项 | 结论 |
|---|---|---|
| 1 | 协议层 | **属实**。三份文档当前内容均支持"非必填、省略=模型默认"；Anthropic "On by default" 与 manual mode deprecated（Claude 4.7+ 返回 400）均仍成立 |
| 2 | medium 注入点 | **部分属实**：适配器是主路径且事故链完整复现（`session/load` → `createModelId(null)` → `fallbackEffort()=medium` → `applyCollaborationMode` → `threadSettingsUpdate(medium)` → 档位门拒）；**但漏了引擎侧第二条**：`multi_agents/spawn.rs` 的 `unwrap_or_default()`（`ReasoningEffort` 的 `#[default]` 即 `Medium`） |
| 3 | 引擎省略路径 | **属实**（三处 file:line 精确） |
| 4 | 解析链 | **属实**且更强：全仓无任何 `default_reasoning_level: Some(...)`，catalog.json 无该字段 |
| 5 | 死数据与约束 | **属实**（`effort_table_for(...).map(levels)`；接入 `default_reasoning_level` 确实会变发送值） |
| 6 | 身份约束 | **部分属实**：除文档三条路径外还有**第四条** `createCodexCollaborationMode`——而那正是事故路径；A2 原改动清单漏了该文件 |
| 7 | 客户端 workaround | **属实**（:989/:1014 行号准确） |
| 8 | 测试矩阵 | 缺 4 条（见 §12 已补） |
| 9 | 发布配套 | **属实**：必须配套才生效 |

## 12. 实施记录与对方案的修订

**新增 A4（计划外发现）**：引擎模型列表对无默认档模型报 `defaultReasoningEffort:
"none"` 占位（`openai_models.rs:1099` 的 `unwrap_or(ReasoningEffort::None)`）。适配器
`unstable_setSessionModel` 收到裸 model id 时原本会用这个占位当档位 → 身份串
`[none]` → 恢复重放把 `none` 发进引擎 → GLM-5.3 报"始终思考不能关闭"。实施：裸 id
与 `[default]` 一律解析为哨兵，`createModelId` 不再使用 `defaultReasoningEffort`。

**B4（计划外发现）**：`spawn.rs` 三处 `unwrap_or_default()` 注入 `Medium`，改为
`Option` 端到端透传，并提取纯函数 `effective_spawn_effort` + 3 个单测。

**测试补充（对应 §11 第 8 条）**：适配器 `effort-default-sentinel.test.ts` 8 例
（哨兵 round-trip、裸 id、collaboration 映射 null、createModelId 不发明档位）；
引擎 `unset_reasoning_effort_is_omitted_on_both_wires` 2 例；spawn 3 例。

**本地 e2e（2026-11-10，真实密钥）**：

| 配置 | 结果 |
|---|---|
| GLM-5.3-flash × Responses / Chat / Anthropic 三协议、无 effort | 全部成功；日志 `reasoning_effort=default` |
| mimo-v2.6-flash × Anthropic | 成功 |
| step-5-preview / step-3.7-flash × Anthropic / Chat | 成功 |
| ACP `session/new` → `session/load`（事故路径）→ prompt | `currentModelId=glm-5.3-flash[default]`，resume 后正常，**无 medium 错误** |
| 显式 `NUWAX_MODEL_REASONING_EFFORT=high` | 身份串 `[high]`，env 优先于哨兵 |
| 显式 `medium`（chat 线） | 档位门拒，原文案复现 |
| 显式 `none`（GLM 始终思考） | 拒，点名模型与档位表 |

**发现的网关行为差异（记录，非本方案问题）**：档位门只挂 chat wire；GLM 的
Responses 端点对 `medium` 宽容（实测通过），chat 端点拒绝。
