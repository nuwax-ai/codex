# nuwax-harness 内置多厂商与跨厂商模型切换 — 详细设计方案 v3(2026-11-04)

状态: **P1 已实现(2026-11-04),P3/P2/P4 待排期** · 作者: Claude(实测+四项目源码级调研) · 关联: ROADMAP, FORK.md §7.2/§10, NUWAX-HARNESS.md §3
参照源码: `/Volumes/zhitai/git_workspace/{goose, aichat, rig}` + 本仓库
变更: v4 = 落实"**不建新 crate**"决策(用户 2026-11-04 拍板,理由见 §0 表)+ P1 按 §3.1 实现完毕(FORK.md §7.2 为准);v3 = crate 架构决策(§3)+ slash 命令分层(§6);v2 的社区对照并入 §2;v1 简版废弃

---

## ⚡ 交接执行摘要(给实现 agent)

**任务**: 为 nuwax-harness(fork of openai/codex)实现"内置多厂商声明 + /model 跨厂商切换"。分四期,按 P1→P3→P2(→P4 可选)顺序交付。

**已验证的事实基础**(无需重调研,全部 2026-11-03/04 实测):
- 跨协议历史回放内建且安全: glm↔mimo 三链路矩阵(glm-anthropic→mimo-anthropic 669k 历史 / glm-chat→mimo-anthropic / mimo-chat→glm-anthropic)全过,marker 正确回忆;thinking 跨源自动丢弃(rig-bridge reasoning.rs envelope 源哈希比对);图片跨厂商回放无损
- resume-with-overrides 跨厂商可行: `exec resume <glm-id> -c model_provider=mimo -c model=mimo-v2.6-flash 'prompt'` 成功,正确回忆旧标记 RT-42,会话 re-home(state-db provider 标记改写)
- fork 同样可行(无校验)
- 大上下文压缩跨切换可用: 92 万估算触发 pre-turn compaction(6:56),压缩后正确回忆
- `/model` 现状 bug: 自定义 provider 下显示内置 GPT 种子(11 个 gpt-*),Enter 会把坏 slug 写进 config.toml

**P1 实现锚点**(精确到文件):
- 新键 `model_providers.<id>.models: Vec<String>`: `model-provider-info/src/lib.rs`(ModelProviderInfo:213,现有字段 name/base_url/model_catalog_url/env_key/wire_api…)+ `config/src/config_toml.rs`(解析)
- 数据源: `models-manager/src/manager.rs::models_for_entry` 的 Fallback 分支(api_key_discovery_disabled → 现走 bundled models.json 种子)→ 声明存在时以声明列表替换(每 slug 经 `construct_model_info` 取 vendor catalog 元数据)
- picker: TUI `open_model_popup_with_presets`(消费 model/list,接口不变)
- 校验守卫: `/model` Enter 持久化(`persist_default_model_selection` → `build_model_selection_edits` → config/batchWrite)前校验 slug ∈ 声明列表

**P2/P3 实现锚点**:
- 跨厂商切换动作(二选一,建议默认 re-home): `thread/resume {threadId, model, modelProvider}`(ThreadResumeParams 已有字段)或 `thread/fork {threadId, model, modelProvider}`(ThreadForkParams 已有)——**协议原语已实测**
- 只读 mismatch 提示条(tui/src/chatwidget/provider_error.rs ProviderErrorNotice,c/n/Esc 三键)增加"以当前配置模型恢复"动作
- 恢复侧入口: 会话 picker 行内 + 只读提示条(注意只读态 composer 被提示替换,只有 c/n/Esc 可用——新增动作要扩展 key 处理)

**验收标准**:
1. 声明 `models` 的 provider:/model 显示声明列表;未声明:行为不变
2. `/model` Enter 选非声明 slug → 置灰+原因,不写 config
3. `exec resume <glm-id> -c model_provider=mimo -c model=mimo-v2.6-flash '标记回忆'` → 正确回答(自动化到 e2e)
4. 跨厂商切换后压缩轮正常
5. workspace 全量测试过;FORK.md + NUWAX-HARNESS.md 同步

**注意事项**:
- rust-toolchain.toml 钉 1.99.1(合并冲突取 fork 侧)
- 测试跑 workspace 全量(`-p codex-core` 单包不带 rust-rig feature,涉桥测试需 workspace)
- **不建新 crate**(v4 决策,2026-11-04):改动落在 fork 自有/低冲突文件;原"新 crate 放 codex-rs/provider-catalog/"作废,抽取触发 = P4 或第二个 catalog 消费者
- 测试凭据: GLM/MiMo key 已在 ~/.config/nuwax-harness/agent.env(GLM 额度 5h 窗口注意)

---

## 0. TL;DR

| 问题 | 结论 |
|---|---|
| 指定自定义模型 + 恢复指定会话,当前可以吗? | ✅ **可以**(实测): `thread/resume` 带显式 `model`+`modelProvider` 覆盖,旧 glm 会话成功恢复到 mimo 并 re-home;CLI 等价 `resume <id> -c model=… -c model_provider=mimo`。无覆盖的 resume 也解析到当前 config |
| 频繁切换 + 图片 + 大上下文压缩 | ✅ 4 段切换链(glm→mimo→glm→mimo,带图片)上下文零丢失;累计 92 万 / 单轮近满窗(104.7 万)输入均触发 pre-turn 自动压缩(6:56 / 11:40 完成),压缩后正确回忆标记 |
| **新 crate 承载多 provider 模型服务?** | ❌ **v4 推翻 v3:不建**——用户 2026-11-04 拍板。P1 落点全在 fork 自有/低冲突文件(models-manager 全 fork 自有;model-provider-info 低频上游改动;TUI 守卫 10 行),建 crate 只增加间接层。**抽取触发条件**(满足其一再抽): P4 配置向导落地,或出现第二个 catalog 消费者 |
| **slash 命令怎么分层?** | **一个 `/model` 承载"选择"**(当前厂商模型 + 其他厂商分组);"配置新厂商"做成 `/model` 内跳转的**配置向导**(P4),不新增平级 slash 命令 |

分期: P1 声明式 models 键(修数据源 bug,~2-3 天)→ P3 恢复侧入口(~1-2 天)→ P2 /model 跨厂商分组与切换串联(~4-6 天)→ P4 配置向导(可选)。

---

## 1. 背景与实测事实(F1-F9,2026-11-03/04 全部验证)

### 用户诉求
同一个会话不断切换不同厂商/协议的模型;TUI 里方便地配置和使用多厂商。

### 实测事实

| # | 事实 | 证据 |
|---|---|---|
| F1 | `/model` 在自定义 provider 下显示**内置 GPT 种子**(11 个 gpt-*);选中会把坏 slug 持久化进 config.toml | 实测 `model/list`;根因: env-key provider 无 `model_catalog_url` → `api_key_discovery_disabled()=true` → fallback 内置 models.json(models-manager/src/manager.rs `models_for_entry`) |
| F2 | `/model` 不能切 provider: `ThreadSettingsOverrides` 只有 `model` 无 `model_provider`;provider 线程创建时绑定 | protocol/src/protocol.rs;session session.rs `original_config_do_not_use.model_provider_id` |
| F3 | resume 无覆盖跨 provider → `ModelProviderMismatch` → 只读 + c/n/Esc | `load_for_resume`(config_manager.rs)saved vs current 校验 |
| **F4** | **resume 带显式 `model`+`modelProvider` 覆盖 → 成功,会话 re-home**(state-db threads 表 provider 标记 glm→mimo,后续无覆盖 resume 亦正常) | 实测: thread/resume 旧 glm 会话 + mimo 覆盖 ×3 变体全过;threads 表 glm 26→25 |
| **F5** | fork(带/不带覆盖)→ 历史完整拷贝到新线程 + 当前 config,无校验 | fork 走 `load_for_cwd`;实测 exec fork glm→mimo 正确回忆标记 |
| **F6** | **跨协议历史回放内建且安全**: 文本/工具/图片协议中立完整回放;旧模型 reasoning 跨源自动丢弃+warn(envelope 源标记);切回原模型 reasoning 恢复 | reasoning.rs `replay_reasoning`;实测矩阵(§5.1) |
| **F7** | 大上下文自动压缩跨切换可用: 累计估算 > 838,860(80%×1M)→ pre-turn compaction,压缩后正确回忆 | 实测: K1 34 万 → K2 +34 万 → K3 +23 万(≈92 万)> 阈值 → 6:56 完成;**补测: 单轮近满窗输入(1,047,022 est)也触发(压缩 ×2,11:40 完成);86.4 万 est 单轮不触发 → 单轮输入检查按满窗,80% 阈值走累计路径——两条触发路径均实测** |
| **F8** | 图片跨厂商回放无损 | 实测: glm 历史中图片 fork 到 mimo 后准确描述色块图 |
| F9 | 逃生口已存在: `model_catalog_json`、`model_catalog_url`(provider 级)、`NUWAX_MODEL` env(EnvSeed 层,优先级 27 > profile 21 > config 20) | 代码+文档 |

### 实测矩阵(F6 依据)

| 历史来源协议 | 目标协议 | 结果 |
|---|---|---|
| glm anthropic(669k 大历史) | mimo anthropic | ✅ 准确复述旧会话内容 |
| glm OpenAI chat | mimo anthropic | ✅ 标记正确回忆 |
| mimo OpenAI chat | glm anthropic | ✅ 标记正确回忆 |

### 压缩机制说明
- **thinking 为什么不能跨协议**: ①载体互不相通(Anthropic `{text,signature}` 验签 / OpenAI `encrypted_content` 私钥加密 / Chat 纯文本);②密码学绑定(签名防篡改、密文不可解,搬过去直接 400);③语义有害(污染新模型推理分布、白占上下文)——**风险不对称,丢弃是唯一 fail-safe**
- **丢弃的判定**: 每条 thinking 生成时打源 envelope(`codex-rig-reasoning-v2:` + `{protocol, endpoint, model, auth域}` 的 JSON),回放时**逐条比对当前请求的源标识**——无全局状态,与切换频率无关
- **频繁切换行为**: 判定逐条精确;**切回旧模型时它自己的 thinking 恢复回放**(envelope 重匹配);异源 thinking 不占请求体;每次请求每条异源 thinking 一条 warn(有意"make the loss visible")
- **边界**: 工具循环**中途**切换,thinking 丢弃后 tool_use 前无 thinking 块,严格端点可能拒——切换应落在轮间(约束写入文档)

---

## 2. Rust 生态调研(源码级)

### 2.1 goose(Block)— 同生态最直接参照

**Provider 层双轨制**:
- 手写 provider crate(`goose-providers/src/`: anthropic/openai/openai_compatible/openrouter/ollama/azure/databricks/snowflake…)+ `goose-provider-types`(Provider trait、ModelInfo、ConfigKey)
- **声明式厂商定义系统**(`declarative.rs` + `definitions/*.json`): **53 个内置厂商 JSON**(51 openai 兼容 + 2 anthropic,含 alibaba/minimax/moonshot/iflytek 等中国厂商)

**DeclarativeProviderConfig**:
```rust
name, engine(openai|anthropic|ollama_compatible), display_name, description,
api_key_env, base_url, base_path,
models: Vec<ModelInfo>,              // ← 声明式模型列表(轻量条目)
dynamic_models: Option<bool>,        // Some(true)=调 /models;Some(false)=静态列表;None=API 失败回退静态
headers, timeout_seconds, supports_streaming, requires_auth,
catalog_provider_id,
env_vars: Vec<EnvVarConfig{required,secret,primary,description,default}>,
auth: Option<AuthConfig>,            // 命令式凭据刷新(与 api_key_env 互斥)
setup_steps: Vec<String>,            // ← TUI 配置向导步骤文案
toolshim, preserves_thinking, emit_clear_thinking,
setup: Option<ProviderSetupMetadata>,
```

**ModelInfo(轻量条目)**: name / resolved_model / context_limit / input+output+cache token 成本 / currency / supports_cache_control / reasoning / supports_image — **不含任何指令/行为字段**(元数据与行为分离)。

**加载**: `fixed_provider_configs()` 静态加载 53 定义 → `ProviderInventoryEntry`(provider_id/description/default_model/**configured**/**available**/config_keys/setup_steps/models/visible_in_setup/deprecated/replacement…)——这就是 TUI "配置与使用"数据主干。

#### 会话内热切(`goose-cli/src/session/mod.rs::handle_model`,~180 行)
1. `/model` 无参 → 显示当前 provider/model
2. `/model <name>` → 换模型保留 provider;`/model --provider X [model]` → 换 provider(可带模型)
3. 校验链: 注册表存在性 → ACP 拒绝 → `manages_own_context()` 拒绝 → `session_provider()` **构建新 provider 实例(凭据校验在此,失败则"Session continues with current provider"不中断)** → `manages_own_context` 再拒
4. **窗口缩减护栏**: 新模型 context_limit < 当前会话用量 → 黄色警告"建议 /compact"(与我们 80% 阈值机制同思路)
5. `services.switch_provider(session_id, provider, model_config)` → **换会话 provider 实例,消息历史原样保留** → 完成缓存更新 → 提示

**goose 能热切的架构根源**: session = 自有中立消息列表(plain messages),provider = 无状态请求构建器,每请求按当前配置构建 → 换 provider = 换一个 Arc 实例。**没有** resume 校验、reasoning 源绑定、auth 按线程生命周期这些 codex 机制。

#### 护栏 UX 值得逐条对齐
凭据缺失不中断 / 窗口缩减警告建议压缩 / own-context 拒绝 / ACP 拒绝 —— 全部可移植到我们的切换确认框。

### 2.2 aichat(sigoden)— 声明式配置范本

- `config.yaml`: 全局 `model: openai:gpt-4o`;多 provider 声明;`compress_threshold` + `summarize_prompt`(会话压缩)
- **`models.yaml`: per-provider 模型元数据列表**——条目轻量: `{name, max_input_tokens, max_output_tokens, input_price, output_price, supports_vision, supports_function_calling}`(**不含指令/行为**),与"元数据走目录、列表走声明"的分工一致
- 路由语法 `-m provider:model`;`.model` REPL 命令会话内切换
- 20+ provider 实现(`src/provider/`)

### 2.3 rig(0xPlaygrounds)— provider 抽象库(fork 依赖 rig-core 0.44)

- `rig-core/src/providers/`: **25+ 厂商目录**(anthropic/azure/chatgpt/cohere/copilot/deepseek/gemini/groq/huggingface/hyperbolic/llamacpp/minimax/mira/mistral/moonshot/ollama/openai/opencode/openrouter/perplexity/xai/vercel/voyageai/…)
- Provider trait 统一 completion/embedding;**每厂商一个目录**(client+model 列表+类型)
- 印证: Rust 生态多厂商抽象的成熟做法 = **trait + 每厂商模块 + 统一消息类型**;rig 0.44→0.45 的 MSRV 变化正是我们钉版的原因(FORK.md)

### 2.4 codex upstream(本仓库基底)

- `model_providers` 表字段集与 fork 一致(见 §4.1),无 per-provider 模型列表键
- provider 绑定线程;`/model` 列表 = model/list;ChatGPT 后端目录;#4849(profiles CLI 化)进行中
- **与 fork 的差异**: fork 增加了 wire_api(chat/anthropic/responses)+ rig 桥 + vendor catalog 元数据 + models.d——**多协议能力上游没有**,因此上游"provider 绑线程"在 fork 场景下暴露出 §0.2 全部问题

### 2.5 模式结论 → 设计输入

1. **声明式厂商定义(JSON/键)+ 轻量模型列表**是 Rust 生态成熟惯例(goose definitions/、aichat models.yaml)→ 一期直接采用
2. **切换 UX 护栏**(凭据缺失不中断/窗口缩减警告/own-context 拒绝)是社区沉淀,应逐条对齐
3. **codex 的 provider-绑线程**是少数派:切换只能 = 换路由(下轮请求)或换线程;**没有一个参照项目做线程原地 rebuild** → 印证 N1(不做原地热切)
4. goose 的 `setup_steps` + `EnvVarConfig{required,secret,primary}` → P4 配置向导的现成数据结构
5. **厂商目录的组织**:goose 把 53 个厂商做成 `definitions/*.json` 随包分发 + `dynamic_models`/`catalog_provider_id` 支持动态——内置声明 + 可选动态拉取的混合模式

---

## 3. 设计

### 3.1 一期: 声明式模型列表(修 F1)

**配置**(新增,可选键;不设 = 现行为不变):
```toml
[model_providers.mimo]
name        = "Xiaomi MiMo (Anthropic)"
base_url    = "https://token-plan-cn.xiaomimimo.com/anthropic"
env_key     = "MIMO_API_KEY"
wire_api    = "anthropic"
models      = ["mimo-v2.6-flash", "mimo-v2.5"]     # ← 新键: 该 provider 可用的模型 slug 列表
```

**行为**:
- `list_models`(models-manager)在 provider 声明了 `models` 时,**替换** bundled-seed fallback(GPT 种子),列表 = 声明项
- 元数据(context_window/effort 档位)由既有 vendor catalog 按 slug 解析(mimo/glm 已收录,零额外配置);`models.d` 可覆盖
- `/model` picker 显示声明列表;`NUWAX_MODEL` env 继续有效(EnvSeed 层)
- 兼容: 键可选;未声明 provider 行为完全不变(上游兼容零破坏)

**替换方案考虑过并否决**: `model_catalog_json`(已存在但格式重+全局+启动期)、修 discovery(格式/网关差异,黑盒兼容)、自动从 vendor catalog 推导(provider id 无映射)。

### 3.2 二期: 跨厂商切换入口(修 F2 的 UX)

**Picker 分组**:
```
┌─ /model ──────────────────────────────────┐
│ 当前: mimo · mimo-v2.6-flash               │
│ ├─ ✓ mimo-v2.6-flash(当前)                │
│ ├─   mimo-v2.5                            │
│ ├─ 其他厂商 ─────────────────────────      │
│ │   glm · GLM-5.3-Flash                   │
│ │   glm-chat · GLM-5.3-Flash              │
│ │   …(config.model_providers 全量枚举)    │
│ └─   (选择跨厂商模型将切换本会话的厂商)  │
└───────────────────────────────────────────┘
```

**选中跨厂商条目 → 确认框**(一次性说明,可勾"不再提示"):
```
切换到 GLM-5.3-Flash(open.bigmodel.cn)?
· 本会话历史保留,之后的对话由 GLM 处理
· 之前模型的内部思考(thinking)不会带过去(协议/厂商不通用)
· provider 记录将更新为 glm;随时可切回
                                    [切换]  [取消]
```

**实现路径**(两选一,实现时定):
- **路径 a(冷切换,推荐首版)**: TUI 关闭当前线程实例(不删)→ 以 `thread/resume {threadId, model, modelProvider}` 重新挂载(实测可行)。注意热线程需先 unload(已有 `thread/unload` 机制);读写态切换点 = 轮间
- **路径 b(挂载态)**: resume_running_thread 热路径透传 overrides —— 需实现时确认该路径是否校验 saved-provider(未验证,标记为实现风险)

**切换后的线程**: provider/model 标记更新(实测);reasoning 跨源丢弃自动生效;压缩按新模型窗口(80%×1M)。

**护栏**(对齐 goose):
- 凭据不可用(env_key 无值)→ 条目置灰 + tooltip
- 目标窗口 < 当前会话用量 → 确认框黄色警告"建议先 /compact"
- 切换动作失败 → 原会话不动(与 goose 一致)

### 3.3 三期: 恢复侧入口(G3)

- 会话 picker / 只读 mismatch 提示条: 在 `c`(新会话续接)之外增加 "**以当前配置模型恢复**"(= re-home resume,一次点击完成跨厂商续接;提示条文案说明 thinking 丢弃策略)
- `resume` CLI 已天然支持(`-c model= -c model_provider=`),无需改动

### 3.4 明确不做
- 线程原地 provider 热切(= N1)
- thinking 跨模型转换/回放(签名/加密不可搬;语义有害;已内建跨源丢弃)

---

## 4. 兼容性与安全

### 4.1 上游兼容
- `models` 为**新增可选键**(serde default None)→ 上游合并冲突面小;行为差异记 FORK.md
- 不改 `load_for_resume` 校验(安全设计保留)
- picker 数据源改动限定在 api_key_discovery_disabled 分支(上游该分支本就是"无发现"死路)

### 4.2 安全
- 不做黑盒格式适配(厂商目录一律声明式)
- 跨厂商切换不绕过任何校验: 凭据可用性检查前置;失败不中断会话
- thinking 跨源丢弃不变(桥层既有)

### 4.3 明确的行为变化(需 FORK.md 记录)
1. 声明 `models` 的 provider:/model 列表 = 声明项(现状 = GPT 种子)
2. /model Enter 持久化前校验 slug ∈ 声明列表(新增守卫)
3. 跨厂商切换入口(P2/P3,新增 UX)

---

## 5. 测试计划

| 类别 | 用例 |
|---|---|
| 单测 | `models` 键解析(缺字段/空列表/重复 slug fail-fast);list 合并优先级(声明 > catalog > 种子);picker 分组渲染 |
| e2e(自动化现有手工矩阵) | glm-chat→mimo-anthropic、mimo-chat→glm-anthropic、glm-anthropic→mimo-chat、mimo-anthropic→glm-chat;每链路 marker 回忆 + 图片轮 + 压缩轮 |
| UX e2e | picker 跨厂商条目 → 确认框 → re-home/fork;凭据缺失置灰 |
| 回归 | 未声明 `models` 的 provider 行为不变(GPT 种子 + 现有校验) |

已手工验证的基线(本方案的事实来源): §0.2 F4/F5/F6/F7/F8。

---

## 6. 分期与工作量

| 期 | 内容 | 改动面 | 量级 |
|---|---|---|---|
| **P1** | `models` 声明键 + 数据源修复 + picker 校验守卫 | config_toml.rs / model-provider-info / models-manager(models_for_entry)/ tui picker | ✅ 已交付(2026-11-04,见 FORK.md §7.2) |
| **P2** | /model 分组 + 跨厂商条目 + re-home/fork 串联 + 确认框 | goose-cli picker/session + app-server(串联已有原语) | ~4-6 天 |
| **P3** | 恢复侧"以自定义模型恢复"入口 | TUI picker/notice 动作 | ✅ 已交付(2026-11-04,FORK.md §7.3:notice `r` 键 re-home;原"会话 picker 行内"并入 P2 的 picker 分组改造) |
| **P4** | TUI 厂商配置向导(config_keys/setup_steps) | 新 TUI 流 | ~3-5 天(可选) |
| 同步 | FORK.md(行为差异)+ NUWAX-HARNESS.md(键/用法) | 文档 | 随期 |

建议顺序: **P1 → P3 → P2**(P3 一天即可让"跨厂商续接"可用;P2 的切换 UX 随后完善)。

---

## 6.5 slash 命令分层(决策)

**单一 `/model` 命令承载全部三层,不新增平级 slash 命令**:

| 层 | 入口 | 动作 |
|---|---|---|
| 当前厂商模型 | `/model` picker 第一分组 | 选择即切(s=会话级 / Enter=默认)——一期数据源修复后为声明列表 |
| 其他已配置厂商 | `/model` picker "其他厂商"分组(二期) | 选中 → 确认框 → re-home resume 或 fork(待决问题 1) |
| 厂商配置管理 | picker 底部"管理厂商…"入口 | 跳转配置编辑器/向导(P4)——**不在 /model 命令面内新增配置语义** |

**理由**:
1. 用户心智:"换模型"与"换厂商的模型"是同一件事(都用哪个模型),配置新厂商是低频向导流——**选择器高频、向导低频**,goose(`/model --provider` + `goose configure`)与 aichat(`.model` + 配置文件)同为两层结构
2. 命令面最小化:每个平级 slash 都要进命令文档/补全/审计
3. `goose configure` 是独立 CLI 子命令而非 /model 变体——配置流的复杂度不应污染选择器

**CLI 层无新增**: `resume <id> -c model= -c model_provider=`(实测)与 `fork <id> -c …` 已覆盖脚本化场景;`--profile` 覆盖启动期切换。

---

## 7. 待决问题(评审时定)

1. 二期跨厂商默认动作: re-home resume(推荐)还是强制 fork?
2. 一期 `models` 键放 provider 级(推荐,provider 即模型边界)还是全局 `model_catalog_json` 扩展?
3. 未声明 `models` 的 provider,picker 显示什么?(建议: 保持现行为=GPT 种子,但**置灰不可选**或显著标注"未声明,选择可能失败"——避免现在的静默陷阱)
