# FORK.md — nuwax-codex 与官方 codex 的逻辑差异

维护说明：本文是 fork 行为差异的权威索引。改动 fork 行为时同步更新本文与
`my-docs/codex-review-prompt.md`（§二 补丁面清单）。2026-10-22 的上游同步目标为
`main@342ed69a1`，Rust `1.99.1`、V8 `152.2.0`，rig 升至 `0.45.0`（2026-10-26，用户解除
0.44 冻结；桥机械适配 Model 构造器/Arc 消息与非穷尽错误，行为合同不变）；合并冲突已解，
main集成与晚间批次已推送，复审输入origin/test=850a87390；备用genai现为0.7.0。
不同源码的实网、fixture与CI证据分开，不宣称全部厂商/全部平台已通过。
当前优先级与本轮验证快照见 [ROADMAP.md](my-docs/ROADMAP.md)，
历史执行证据见 [最新验收台账](my-docs/rig-production-completion-2026-10-10/claude-rig-final-acceptance-2026-10-18.md)。
npm 组织包：`@nuwax-ai/nuwax-harness`；发布主二进制/主命令：`nuwax-harness`。
2026-10-23 改为组织包发布，原生 0.18.14 制品保持不变；不提供旧包卸载或迁移流程。
ACP 侧配套包：`@nuwax-ai/nuwax-codex-acp-ts`。

## 1. fork 的使命

让 codex 支持中国厂商（GLM / MiMo / StepFun / DeepSeek / Ollama 等）：他们的
API 是 OpenAI Chat / OpenAI Responses / Anthropic Messages 三种线协议的兼容网关。
fork 通过 rig 桥统一第三方模型的协议转换、传输与会话生命周期；Responses
同协议接入保留透传路径。模型的工具、视觉、推理和窗口能力仍需逐项配置。

本轮协议/桥接六包合同1350/1350通过（另1个schema专用跳过）、四包Clippy严格检查
及最终运行时/存储365/365已有证据；shutdown/startup暴露的SIGABRT已通过具体future
装箱修复，原默认栈/期限/断言不变。Mac152.2.0配对V8按官方action清单链校验并用于
构建，Code Mode真实JS2/2通过；最终Core/app-server/CLI/live-tests all-targets check
也通过。Linux168/271是各自源码快照的定向结果，不是最终HEAD整体/实网验收。

旧head29e7e59d5的M1 mac job已于05:48:01Z成功。新合并树CI run39761410573
的Ubuntu/macOS在弃用API的严格lint阶段失败，Windows在TUI测试编译阶段失败，
三平台workspace测试未运行，Bazel bridge gate通过；本轮已修API和测试适配，
定向复验及后续CI各自登记，不把旧成功当新树验收。

## 2. 配置目录：Nuwax Harness独立默认（v0.18.14起）

| 优先级 | 来源 | 行为 |
|---|---|---|
| 1 | `CODEX_HOME` 环境变量 | 必须是已存在目录，canonicalize；可选外接盘 |
| 2 | `~/.nuwax-harness` | 默认配置、认证、会话与日志位置；不存在时允许初始化 |

用户明确当前开发阶段无需兼容/迁移旧目录，不再自动回落到`.codex-nuwax`、
`.nuwax-codex`或官方`.codex`，也不删除旧数据。已存在的新路径若是文件/坏symlink则报错，
有效目录symlink保留路径语义。官方Codex不受影响；fork专属配置写在新的隔离目录。
环境变量启动指南与模板见[NUWAX-HARNESS](NUWAX-HARNESS.md)。

## 3. 模型协议与路由

### 3.1 wire_api 取值（model_providers 表内字段）

| 值 | 官方 | fork |
|---|---|---|
| `responses` | ✅ | ✅（第三方厂商默认走桥，见 3.3） |
| `chat` | ❌（上游已删） | ✅ fork 重新加回 |
| `anthropic` | ❌ 不存在 | ✅ fork 新增（显式声明，解决 Step 等无 URL 标记网关） |

### 3.2 双桥 + 逃生舱

`experimental_bridge`（fork 字段）：`"rig"`（默认，rig-core / rig-reqwest `=0.45.0`）/
`"genai"`（备用，rust-genai 0.7.0）/`"native"`（显式选择上游原生Responses传输）。

桥的中立 trait 在 `codex-api/src/bridge.rs`（`ModelBridge` / `ModelWireProtocol`，
含 `ModelBridgeOptions`：extra_headers、协议、idle timeout、x-codex-turn-state 回传）。
core 经 `&dyn ModelBridge` 分派，不感知具体桥；core 无默认桥 feature，由
cli/exec/app-server 二进制启用。

### 3.3 路由策略（core/src/client.rs）

- **Chat / Anthropic wire**：一律走桥（官方原生只认 OpenAI）。
- **Responses wire**：第一方 OpenAI/Bedrock 走官方原生；**第三方厂商默认也走桥**
  （`uses_model_bridge()` 判定 + `dispatch_model_bridge` 分派，fork 关键策略）。

### 3.4 Responses 同协议透传（rig 桥内）

第三方 Responses wire 不做协议转换：请求类型级 clone（仅跨协议历史投影剥
reasoning envelope）→ 序列化并设置有效输出 cap → 构造 HTTP 请求，经 rig
`HttpClientExt::send_streaming`（0.44 起无 provider `post_sse` builder；建流也受
idle timeout 包裹）→ 原始 SSE 进 codex-api 完整解码器，**严格终止策略**
（`codex-api/src/sse/responses_policy.rs`：顶层 error/failed/incomplete 即停、
畸形帧报错、[DONE] 容忍）。官方原生路径的宽松策略不变。Authorization 在 rig
自动合成 Bearer 后**精确恢复**（api-key-only/无鉴权网关不被改写）。

备用genai0.7与主rig不承诺完全等价：Responses不支持；OpenAI端点的path/query已
修正并有真实loopback合同；Anthropic endpoint携带query时明确拒绝，可改用rig，
不能静默吞参数。主rig的严格usage presence与身份合同不能从备用桥编译通过推导。备用桥的header按名称
排除主Authorization、保留同值secondary；不可表达的非文本/同层多值头明确报错，
provider/auth/extra跨层覆盖顺序保留。

## 4. hosted（服务端）工具翻译（v0.18.8+）

codex 以 Responses 形态声明 hosted 工具（`{"type":"web_search"}`）；官方在
Chat 转换中直接丢弃。fork 的 `hosted_tools.rs` 翻译表：

- 请求侧：`web_search` → Anthropic `web_search_20250305` 服务端工具（GLM 网关
  实测接受）；`filters.allowed_domains`、`user_location` 等可表达约束随翻译保留
  （域名格式/地点字段前置校验），cached/indexed 模式在发请求前显式报错（不静默
  放宽为 live）；无对应物的工具仍丢弃并告警。新工具 = 加表项。Chat 线无 hosted
  概念，声明整体丢弃并 warn（`stream.rs`，2026-10-29 起不再静默）。
- hosted-only 请求的 tool_choice：rig 流式路径在 typed 工具为空时丢弃
  tool_choice，transport 注入服务端工具后恢复请求值（none/any/tool 原语义，
  禁并行标志随恢复生效；required/指定工具在无函数工具时前置报错；Chat 线无
  工具时移除悬挂 tool_choice）。
- 默认 fail-closed：`web_search_mode` 默认 cached，而 chat-family 桥线无法表达
  cached → 新增 `ProviderCapabilities.cached_web_search`，默认 cached 在此类线
  上降级为 Disabled（此前字段丢弃翻译会让 GLM 实际执行 live 搜索，与配置模式
  相反）。需要搜索请显式 `web_search = "live"`。
- 响应侧：当前 rig 0.45 转换路径仍借助 transport 层 tee 原始
  SSE（`anthropic_sse_tee`，eventsource-stream 解帧），流泵在 Completed 前回收
  web-search 块 → `WebSearchCall` 条目（兼容 GLM 的 `web_search_prime` 改名、
  `search_query` 字段、非标准 assistant 侧 `tool_result` 块）。
- **结果对持久化与忠实回放（阶段 D1/D2 + R2+R3 复验后落地）**：
  `server_tool_use` 与结果块按 id 配对；未配对=混合轮 pending
  （`in_progress`），后续响应带回的结果按 server id 回配请求历史中的
  pending call，发出**新的追加**完成条目（rollout 只追加），请求构建时
  同 id 以首个 completed 为准去重。载荷以 `WebSearchCall.wire_blocks` 的
  版本化 envelope 持久化：`{"version":1,"source":<凭据无关来源身份>,
  "blocks":[...],"cited_text":[...]}`（空 cited_text 不序列化）。回放要求
  来源身份与当前请求一致——旧版裸数组与跨来源密文保守降级（不回放、不
  截断）；请求侧按**真实对**强制上限：≤64 对/请求（丢最旧）、单对序列化
  ≤40,960 字节（整体丢弃）、id/形状校验，resume/fork/导入同路径执行。
  带 citations 的文本块随 envelope 持久化并按原位回放；provider 级
  `hosted_results_replay=false` 整体关闭回放（含旧格式）。
- **pause_turn 桥内透明续接（阶段 D3 + R3 B2 原样续接）**：每个
  Anthropic attempt 挂 tee；暂停时重组**全部** assistant 块（thinking/
  signature、text+citations、tool_use、server_tool_use、结果块，按原始
  index 顺序，四类增量到终态），续接请求原样替换/追加暂停 assistant 的
  content（深度相等 wire 回归锁定），工具数组不变，上限 4 次，超出明确
  报错。每用户可见 turn 恰一个 Created。
- **流取消（R4）**：消费端 drop 后 pump 立即停止等待模型（select 于
  channel 关闭，非 idle 超时替代），在途请求与 socket 随之释放；已取消
  的 turn 不再启动暂停续接。回归证：drop 后连接快速中止且无第二请求。
- **hosted 搜索厂商能力矩阵**（2026-10-01 live 实测，工件在
  `logs/live-<vendor>/websearch-*`）：GLM 服务端执行 + 回放被接受（PASS）；
  MiMo 把声明的 server tool 回成客户端 tool_use（core 拒绝后模型降级 shell 查
  询，无 hosted 搜索发生）；Step 直接 400 `input_invalid` 拒绝声明（无搜索的
  anthropic 场景 PASS，隔离出变量）。后两者是网关能力边界，websearch live 矩
  阵收敛为 `["glm"]`，网关支持后加回。pause_turn：三家网关均未产生（本轮全
  部 live 日志零命中），覆盖以离线 wire 测试为准。

阶段 D 的早期设计与证据见 `my-docs/codex-review-2026-10-01.md`。暂停保真及混合轮
已有离线 wire 合同，真实厂商支持仍按上述能力边界分别判断；不能把离线通过扩大
成三家网关都真实产生过 pause_turn。当前版本的字段处置和验收范围以
[最新台账 §2、§4](my-docs/rig-production-completion-2026-10-10/claude-rig-final-acceptance-2026-10-18.md) 为准。

设计：`my-docs/anthropic-hosted-tools/`。
### 三协议字段契约（2026-10-22，验收分别绑定源码与二进制）

fork 明确每个跨协议字段在三条线（Responses passthrough / Chat / Anthropic）
上的语义，避免"开关在一条线生效、另一条静默丢失"：

| 字段/能力 | Responses passthrough | Chat 桥 | Anthropic 桥 |
|---|---|---|---|
| `max_output_tokens` | 请求值优先，否则采用 provider 值；两者未设则省略 | 映射 `max_tokens`（Rig 可按模型转为 `max_completion_tokens`） | 映射 `max_tokens`；未配置默认 16384 |
| `hosted_results_replay` | 不适用（无桥回放） | 不适用 | false 时整体关闭回放（含旧格式） |
| reasoning effort/summary | 同协议字段保留 | effort 映射 `reasoning_effort`；summary/context 无等价字段，不转发 | 按 Messages 的 thinking/effort 语义映射；不承诺 OpenAI summary/context |
| structured output | 同协议字段保留 | `response_format.json_schema` 保留 schema/name/strict | `output_config.format` 保留 schema；无 name/strict 等价字段 |
| custom/freeform 工具 | `format` 原样透传 | grammar（syntax+definition）保留到 tool description，区分外层 JSON 参数与 input 原始文本；仅为模型提示，不等价于服务端 grammar 强约束；声明 grammar 但缺 syntax/definition、或空白工具名均 fail-fast | 同 Chat 注入（Messages 线同样无 grammar 通道） |
| namespace 声明 | 原样透传 | 展平子工具名；namespace description 追加到每个子工具 description；缺 name/tools 或空白 namespace 的坏声明 fail-fast | 同 Chat |
| usage/缓存计数 | 同协议解码，保留字段 presence | wire 观察器按 presence 保留缺失/部分/显式零，缺失或 null usage 合法；非法 counter 报错；CapExhausted 与回放携带 `ReportedResponseUsage`；成功终态按 Rig `Usage::is_reported()` 决定 `TokenUsage` 的 Some/None（不是按 usage 对象是否出现），已映射的缺失 token counter 呈现为 0 | message_start/delta 中的 usage 对象必需，内部 counter 可缺席；跨帧累积且显式零覆盖；错误/回放保留 presence，成功终态聚合规则同 Chat |
| 重试/限流 | 受 provider 配置及 Core 采样策略约束 | 桥传输/流重试与 Core 采样分层；无统一总尝试数保证 | 同 Chat |
| web_search 模式 | 原样（含 cached/indexed） | 丢弃声明并 warn（无 hosted 概念，不再静默） | 仅 live；cached/indexed fail-fast |
| `parallel_tool_calls` | 原样 | 原样 | hosted-only 时还原 tool_choice 并关 parallel |

`cached_web_search` 能力当前按 `native_transport` 绑定（model-provider
`capabilities.rs`）：native Responses为true，可保留cached；Rig Responses桥为false，
Core默认Cached据此降级Disabled。手工构造Responses请求的同协议字段透传是另一层，
不应与Core默认能力混同。Chat/Anthropic桥收到无法表达的cached/indexed请求时fail-fast。按协议×厂商
的精细能力矩阵仍为登记项（R5 追踪），本轮未改默认搜索策略。



### apply_patch 对厂商模型开放（2026-11-12 起）

厂商目录与占位模型此前 `apply_patch_tool_type = None`，工具列表里没有
`apply_patch`——agent 只能把补丁塞进 shell heredoc 或 `python3 -c`，那正是
"改错文件/改半截"事故的来源；实测里 agent 甚至先感叹 "apply_patch isn't
available in this harness?"，再靠 shell 里恰好存在的 `apply_patch` 命令绕过。
现在两类模型都取 `ApplyPatchToolType::Freeform`：补丁是纯文本，模型写坏只会得到
一次工具报错，不会损坏文件。官方目录模型维持原判定。

### 图片与附件的边界

MCP typed image、Code Mode `image()` 与用户图片走准备、图文共享预算及最终
wire 检查，再进入 Chat `image_url`、Responses `input_image` 或 Anthropic
base64 `source`。编码是图片载荷，不按普通文本逐字符估算，但仍计视觉 token
与请求字节；把 Base64 塞进 text/JSON 字符串仍按文本处理。工具图有数量/单图/
聚合上限，完整请求有 32 MiB 上限；视觉估计只是跨模型近似，不是厂商 tokenizer。

默认 AttachmentStore 的 `upload()` 不代表已接入远端 Files API；通用 PDF/Files
生命周期及输出多模态不是截图支持的完成前提。`structuredContent` 与同结果
typed 媒体已可并存；原图跨协议转换预算及普通图混合的下界已有定向修复，
其历史验收不代替本次 main 合并后的复验。详见
[字段与图片说明](my-docs/rig-production-completion-2026-10-10/rig-field-image-closure-2026-10-18.md)
和最新台账。

**图片字节预算（2026-11-12 起）**：两级。
- **能力默认值**：目录里查不到的模型按"支持图片 + `detail: original`"处理（此前恒为
  不支持，等于让用户去写一条自己写不了的目录项）。真正决定像素能不能被读的是线路，
  不是这个标记。
- **单图**：`image_byte_budget_bytes` 现在区分"未设置"与"0"。未设置时**由线路决定**：
  只有 Responses 线且模型声明支持 `detail: original` 才不设默认预算；其余（Chat 桥
  一律把 original 改写成 `high`，Anthropic 线根本没有 detail 字段）默认 **2 MiB**，
  因为上游只会按自己的上限重采样，多出的字节纯属白传。设 `0` 是显式关闭，设正整数
  是操作者自己的预算。判定在 `image_preparation::wire_honors_original_detail`。
- **回合**：新增请求级预算 `NUWAX_REQUEST_IMAGE_MAX_BYTES`（默认 **20 MiB**），
  在会话准备完图片后、发请求前执行：只要请求内联图片总量超预算，就按"从最大的一张
  开始对半重压"，直到回到预算内；单张压到 192 KiB 以下就不再压（再小就不是证据了）。
  之前只有"单个工具结果"的 8 张/16 MiB/24 MiB 上限，跨结果聚合毫无约束，最终由
  网关 32 MiB 兜底成 413——那是错误的地方发现错误的事。设 `0` 关闭该 pass；非法值
  告警一次并回落默认。两条边界：① 若这次又降了某张图的分辨率，紧邻其后的
  `<image_resize_notice>` 会追加一行"更正后的尺寸"，否则模型会拿过期数字算截图坐标；
  ② 压到 floor 仍超预算时保留原图并打 `warn`（点名 budget/total），网关仍可能 413，
  但不会再无声无息。

**像素上限与降采样（2026-11-12 起）**：`MAX_PROMPT_IMAGE_PIXELS`（40 MP）只约束
不变换直接发出的图（`PromptImageMode::Original`，本地上传那条路）；两个会重采样的
策略（`HIGH_DETAIL` 2048/2500 patch、`ORIGINAL_DETAIL` 6000/10000 patch）以及工具
输出收缩阶梯，改由 `MAX_PROMPT_IMAGE_DECODE_PIXELS`（80 MP）约束。起因是一个真实
死锁：DPR-3 整页截图 4320×16764≈72 MP，被 40 MP 判超限直接丢弃；而收缩阶梯
`cap_data_url_dimensions` 内部用的正是同一个 40 MP 解码闸，于是"大到无法被
缩小"——最需要降采样的事实上永远解不出来。伴随两处修正：像素超限改用独立的
`ImagePixelsTooLarge` 错误（不再套 `too large (N bytes)` 文案），占位文本给出
真实 `宽×高`、上限和补救方式（降低 DPR / 截更小区域），不再让 agent 去猜一个
并不存在的字节阈值。注意 80 MP 解码约 320 MB 峰值内存，一次性且随即重编码变小。

## 5. 上下文压缩（compact）

**交接摘要的"必须保留"项（2026-11-12 起）**：`templates/compact/prompt.md` 从
四句话扩到六项，显式要求保留"已验证结论及其量测数据""已排除的假设与误报""改过的
文件/命令/环境细节""横跨步骤也不能丢的用户约束"。起因是实测交接后 agent 会重复
报同一个已排除的假缺陷、并把早轮的结论当新发现——摘要缺的不是文采，是可复核的
事实。


### 5.1 超窗错误分类（v0.18.10，修复同事的压缩失败）

官方对 HTTP 400 一律归 InvalidRequest。桥路线厂商以 400/413 body 报"上下文超
限"（`context_length_exceeded` code、Anthropic "prompt is too long" 等标记），
fork 识别为 `ContextWindowExceeded` → core 的"删最旧条目重试"压缩兜底循环与
turn 级超窗处理才会触发（官方代码里这两个机制只认该分类，但桥路径永远到不了）。

### 5.2 ratio 派生阈值 + 环境变量（v0.18.10）

- 配置键 `model_auto_compact_ratio`（0<r≤1）：limit = ratio × **有效**窗口；
  绝对值 `model_auto_compact_token_limit` 优先；**推导唯一归属 models-manager**
  （在 max_context_window 裁剪之后进行——200k 窗口覆盖 + ratio 0.5 在 60k 模型
  上阈值是 30000 而非 100000）；ratio 经 `ModelInfoOverrides` 全链保持（首轮/
  resume/切模型不丢）；越界（负数/0/>1/NaN）或取整为 0 时忽略并告警（0 阈值
  会导致每轮压缩）。三个键均可由环境变量注入并按 thread 投影到共享 daemon。
- `model_context_window` 显式覆盖的钳制语义（2026-10-28 起）：目录内模型仍被
  目录 `max_context_window` 钳制（目录是权威）；**未知模型的 fallback 元数据
  不再钳制**——其合成 272k 上限是占位值而非厂商知识，显式覆盖全额生效并同步
  抬高 max（大窗口厂商如 GLM-5.3-Flash 1M 可按真实窗口使用，压缩阈值随真实
  窗口派生）。配大了自愈：桥线厂商超窗 400 映射 `ContextWindowExceeded` →
  删旧压缩重试；非正数覆盖忽略并告警。判据 `used_fallback_model_metadata`
  （models-manager `model_info.rs::with_config_overrides` 单点）。同轮补齐
  非正数输入的两层拒绝（复查发现）：`model_context_window` 与
  `model_auto_compact_token_limit` 为 0/负时 config 加载 fail-fast
  （TOML/-c 路径，此前仅 env 入有校验；0/负限额会每轮触发压缩——含
  BodyAfterPrefix 作用域直读原始 config 值的路径），models-manager 层再
  兜底丢弃（app-server 直构 ModelsManagerConfig 的路径），丢弃后 ratio
  仍可派生。
- 环境变量（优先级：显式 `-c` > env > config.toml；非法值带变量名报错）：
  `CODEX_MODEL_CONTEXT_WINDOW` / `CODEX_AUTO_COMPACT_TOKEN_LIMIT` /
  `CODEX_AUTO_COMPACT_RATIO`。同类先行：`CODEX_MODEL_REASONING_EFFORT`。

### 5.3 迁移压缩：大窗口会话降级到小窗口模型（v0.18.30 起）

**问题**：会话在 1M 窗口模型上长到 40 万 token 后切到 32K/16K 模型。旧的
"全量历史摘要"请求本身就超窗：实测（本地 llama.cpp 16K）会先被桥的本地预算
闸门拒绝 6 次、再逐条 trim 4 次，最后只给一句 "ran out of room"。摘要也不能
交给旧模型——旧模型可能已经不可用（key 失效/额度耗尽）。

**做法**：不再"整段替换"，而是**隐藏最老的连续区间 + 保留尾部**：

- 记录 `RolloutItem::SurfaceReplacement`（`my-docs` 迁移压缩方案 S1）：以
  `resume_at_item_id`（稳定 `ResponseItem` id，不用下标，跨 checkpoint/回滚
  仍有效）为边界，边界之前的历史由 `summary` 取代；`supersedes` 让新记录吸收
  旧记录（每次迁移只有一条生效）。
- 摘要**只用当前（目标）模型**做，区间有界，所以每个摘要请求都按预算估算
  装入窗口（最终以网关真实 tokenizer 为准；网关拒绝时由采样后的有界溢出
  重试兜底——仅在尚未产生回复/工具副作用时逐条裁剪重试）：
  `budget = 0.8×window − 实测前缀 − 2048`（前缀 = 上轮上报用量 − 历史估算，
  含指令与工具负载），`retain = 0.16×window`，
  `span ≤ min(0.5×window, budget/2)`（后一项是被桥闸门拒绝后补的：闸门按
  "前缀 + 工具 + span" 计价），单次迁移最多 8 段。
- 触发点：`turn.rs` 在常规 pre-sampling compact **之前**；
- 回放：`rollout_reconstruction` 在重建完历史后按边界裁剪——必须放在重建之后，
  因为旧的 `Compacted` checkpoint 会用 `replacement_history` 整体替换历史，
  边界条目可能在那份快照里而不在 suffix 中（曾因此导致"跳过整段 suffix 但
  checkpoint 的大历史仍在"）；边界找不到时保留全量历史并告警，不静默丢内容。
- 失败语义：摘要失败只 `warn` 并回退旧路径；不会静默截断历史。

**实测（32K 本地 llama.cpp + 2B 模型）**：两段摘要把 26,565 → 23,674 token 压到
预算 22,855 内，本轮正常出结果；后续续聊回放后请求 17.5K token，不再重复压缩。

**极限场景（80 万 token → 32K 窗口，回归测试
`compact::tests::migration_compact_extreme_800k_history_converges_into_32k_window`）**：
单次迁移 pass 最多摘 `max_ranges`(8) 段，只能移走约 7.2 万 token，**一个 pass 装不下**。
`run_turn` 内**循环执行迁移 pass 直到历史入预算**（2026-11-06 第二轮审查起，此前
是每 turn 一次、约 11 个 turn 收敛）：默认配置 806,379 → 15,308 token 需 ~11 个
pass / 79 次摘要请求，**单个 turn 内完成**；`max_ranges=64` 时 3 个 pass。turn 级
硬上限 32 个 pass 防病理空转，摘要失败仍只 warn 回退。

**审查加固（2026-11-06，codex 两轮独立审查）**：
- **旧迁移不覆盖新 checkpoint**：回放只在**有效 Compacted checkpoint 之后**的
  记录里选生效的 `SurfaceReplacement`——checkpoint 的 `replacement_history`
  是权威面，其之前的迁移记录已被吸收，再套用旧边界会删掉新 checkpoint 特意
  保留的边界前内容（回归 `an_older_surface_replacement_cannot_override_a_
  newer_checkpoint`）。
- **摘要输出上限与预留一致**：`RESERVED_OUTPUT_TOKENS`(2048) 不再只是预算减
  法——`Prompt.max_output_tokens` 新增逐请求覆盖，摘要请求在 wire 上真实携带
  2048 上限（native Responses 与 rig 桥同享该字段），provider 的大输出配置
  不再能把 input+output 顶出目标窗口。回归：极限测试逐请求断言
  `max_output_tokens == 2048` 且解压后 `input(4B/token) + 2048 ≤ window`。
- **切点联合约束**：`select_compaction_region` 的边界必须落在带 rollout id 的
  条目上。前移被单调性禁止（尾部保留与 span 上限都停在各自最大合法位置，
  end+1 必破其一）；**无 id 切点向后回退**到最近的"带 id 且配对安全"边界
  （缩小区间必然满足 span/尾部约束）。不存在任何合法边界才返回 None，绝不
  跳到孤立工具结果。另：固定前缀吃满预算（budget ≤ 0）时明确跳过，不再退化
  成 1-token span。
- **未收敛不得落入旧 trim 路径**（第二轮 P1）：迁移 pass 耗尽 `max_ranges` 而
  历史仍超预算时，若继续走常规压缩/推理，旧 trim 循环会**未经摘要地删除最老
  条目**——内容从未进入任何成功请求即消失（实测丢 14/64 段）。修复即上述
  turn 内循环：收敛前不把超窗历史交给会丢未摘要前缀的 fallback。

**端到端 turn 级验证**（`migration_compact_end_to_end_turn_completes_and_keeps_
recent_context`）：对 32K"网关"（mock 按同口径拒收超窗请求）跑完整 `run_turn`。
断言**覆盖不变量**：离开上下文的每段原始历史，必须先进入过至少一个被网关
**接受**的摘要请求（旧 trim 删除的内容只会出现在被拒请求里，过不了该断言）；
最终推理在窗内完成、保留尾部 recent fact、最老 filler 已被摘要。两个前置条件：
TokenBudget 特性会关闭溢出恢复臂（测试显式禁用）；OpenAI 第一方 provider 默认
`remote_compaction=V2`，32K 本地网关场景必须显式 `Unsupported`（桥路径 provider
本就解析为 Unsupported）。

**降级裁剪（2026-11-08 方案，`my-docs/migration-compaction-downshift-plan-2026-11-08.md`）**：
迁移结果拆为 `MigrationOutcome`（NoMigrationNeeded / Converged / StillOverBudget /
CannotSummarize / Cancelled），run_turn 重复 pass（每轮上限 32）后仍超预算、无合法
切点或摘要失败时，走 `degrade_history_to_target_window`：按目标窗口的请求预算
（窗口 − 2048 输出预留 − 指令估算 − max(2048, 窗口/8) 工具/重注入余量 − 本轮输入）
从尾部向前保留连续历史，切点配对安全、落稳定 id（全放不下允许空尾部），持久化
`SurfaceReplacementReason::WindowShrinkOmitted`（空 summary = 无摘要省略，回放按同
一边界裁剪且不种植消息；空边界 id = 隐藏全部），内存投影与记录一致，实际省略时
发一次用户提示。原始 rollout 不动。必要内容（指令+工具+本轮输入+预留）本身超窗
时保留本轮输入并报可行动错误；用户取消立即停（已提交记录仍可回放）。缩窗路径
不依赖旧模型：迁移处理过的 turn 跳过 `maybe_run_previous_model_inline_compact` 的
旧模型摘要入口，也不把未收敛历史交给旧全量 trim 循环。`retain_ratio = 0` 时选择器
从最后一个实际条目回退找切点（此前在 `end == len` 直接放弃）。模型可见历史与原始
记录的区别：裁剪只作用于模型可见面，rollout 永远完整。

**配置键**（`config.toml` 顶层，缺省即下列默认值；越界 fail-fast 并点名键）：

| 键 | 默认 | 含义 |
|---|---|---|
| `migration_compact_ratio` | `0.8` | 历史+固定前缀可占窗口的比例（预算） |
| `migration_compact_retain_ratio` | `0.16` | 原样保留的尾部占比 |
| `migration_compact_max_span_ratio` | `0.5` | 单次摘要请求占窗口上限 |
| `migration_compact_max_ranges` | `8` | 单次迁移最多摘要段数（1–64） |

TUI 进度与 ACP 回执沿用既有 `ContextCompaction` 条目（`compaction_update`），
未新增协议字段。

### 5.4 摘要装不下时的自我修复 + 品牌（2026-11-09 起）

**问题（事故）**：同事的 ACP 定时任务会话从 2026-11-08 18:03 起每小时都以
`Codex ran out of room ...` 失败，耗时恒 1.0–1.3 s、`usage:null`；客户端把该
文本当 `ASSISTANT` 消息写回，下一小时再注入，历史只增不减。根因不是"没有默认
阈值"（默认 80% 一直在生效，见 §5.2 与 `protocol/src/fork_compact.rs`），而是
**摘要请求本身必须装进窗口**：历史一旦越线，整段摘要请求同样被拒；失败路径
又不落盘（`remove_first_item` 只改内存 clone），进程退出后 rollout 原样，下次
resume 在同一状态上再失败——永久原地打转。

**行为**（`my-docs/compact-unrecoverable-selfheal-plan-2026-11-09.md`）：

- **不变量**：任何一轮开始前，历史必须可发送。轮前压缩（`run_pre_sampling_compact`）
  与中途恢复臂遇到 `ContextWindowExceeded` 时不再把错误抛给用户，而是转
  `degrade_history_to_target_window`：按目标窗口预算从尾部保留连续历史，落
  `SurfaceReplacementReason::Overflow`（本 fork 新增用途），并发出一次
  `EventMsg::Warning`（"could not summarize … omitted ~N tokens"）。
  必要内容（指令+工具+本轮输入+预留）本身超窗才返回可行动的 `Fatal`。
- **落盘是硬要求**：降级必须持久化 `SurfaceReplacement`（回放按同一边界裁剪），
  否则 resume 会把超窗历史装回来——这是事故的机制性根因。
- **压缩内部有界补救**：`ContextWindowExceeded` 时先做一次确定性裁剪
  （`prune_oversized_tool_results`：把超长文本工具输出裁成头/尾窗口加标记，无
  模型调用、不动消息与配对），仍超再逐条删最旧（`remove_first_item`，保留
  call/output 配对；`build_compacted_history` 会重新注入 base instructions）。
  删最旧最多 `MAX_OVERFLOW_TRIM_ROUNDS = 2` 轮，之后交还上面的降级链——避免
  "删到装下为止"的无界重试（每轮都是一次已被网关拒绝的请求）。
- **路径区分**：降级提示按 `DegradeReason` 分文案（`ModelDownshift` 保持原文案，
  `CompactionOverflow` 点名 nuwax-harness 与省略量）。
- **中途前台检查点**：`async_compaction` 只在宿主远程压缩 V2 可用时启动，第三方
  网关没有中途压缩点，长工具循环只能等网关拒绝后靠 `fork_overflow_recoverable`
  补救一次。现在每步采样后（`needs_follow_up` 且活跃上下文达到
  `model_mid_turn_compact_threshold_percent`，默认 90、0 关闭、每轮至多 2 次）执行
  前台 `run_auto_compact(MidTurn)`；摘要装不下同样转上面的降级链并 warn。
- **观测上限（2026-11-09）**：采样请求被网关以超窗拒绝时，把当时的活跃上下文
  记入 `ObservedContextCeiling`（仅内存、只降不升），后续
  `context_window_token_status` 把有效窗口钳到该值的 95%，让下一次检查点提前
  压缩。针对"网关实际拒绝点低于宣称窗口"和图片/工具负载被本地低估两种情形；
  resume 后重新学习，不进 rollout。
- **回放语义（2026-11-09 复审修复）**：空 `resume_at_item_id` 的"空尾部"记录不再按
  当前历史的末尾截断——回放改为定位**记录之后第一条 item**，因此裁剪之后新增的
  用户消息/回复不会在 resume 时被一起隐藏（回归
  `a_no_summary_trim_with_later_turns_keeps_them_on_replay`）。
- **轮内降级保护当前指令**：`DegradeProtection::CurrentTurnInput` 保证边界不越过
  最新的 user 消息；若保护后仍装不进预算，返回 `NecessaryContentExceedsWindow`，
  不会出现"重试成功但任务指令已丢"。
- **每步只压一次**：中途检查点执行后重算 `context_window_token_status`，旧的
  rollover 分支不再用压缩前的 `token_limit_reached` 二次压缩。
- **观测上限按路由隔离，且只影响调度**：`ObservedContextCeiling` 记录 model +
  provider，只对同一路由生效；它只参与"该不该压缩"的判定
  （`observed_window_limit`），裁剪预算仍用宣称窗口
  （`full_context_window_limit`），所以一次图片密集请求被拒不会让后续纯文本轮次
  被误判成"必要内容都装不下"。
- **两种"装不下"分流（2026-11-09）**：`NecessaryContentExceedsWindow`（指令+工具+
  本轮输入本身超窗）返回 `CodexErr::InvalidPrompt`，协议映射为
  `CodexErrorInfo::InvalidPrompt`；与"会话上下文耗尽"（`ContextWindowExceeded`）
  形成两个机器判据。此前是 `Fatal → Other`，客户端会误判成可重试的 provider 故障。
- **复审加固（2026-11-09 第二轮）**：① 空尾部裁剪的回放按"记录之后第一条 item"
  定位，若其后的轮次被回滚（id 不再出现在重建历史里）则隐藏全部，不会把被裁掉的
  旧历史放回来；② 轮内降级保护当前指令时，先对保留尾部做一次确定性工具输出裁剪
  （头 4k/尾 1k 字符），一句短指令 + 超长工具输出不再被判成"输入过大"；③ 被网关
  拒绝过的那次重试，其裁剪预算取 `min(宣称窗口, 已观测拒绝点)`，避免原样重发；
  ④ 中途检查点执行后，同一 step 的 rollover 不再二次压缩，且
  `NecessaryContentExceedsWindow` 会明确报成 `InvalidPrompt` 而不是被吞掉。
- **复审加固（2026-11-09 第三轮）**：① 工具输出剪枝随 `SurfaceReplacement`
  持久化（`pruned_tool_outputs`，按 `call_id` 记录头/尾窗口），resume 后回放同样的
  模型可见面，不再恢复完整大输出；② 轮内降级保护"本轮第一条 user 消息"
  （turn_id 归属），追加指令不会挤掉原始任务；③ 裁剪预算与"必要输入超窗"判据
  拆分（前者可含已观测拒绝点，后者只用宣称窗口）；④ 本步已前台压缩时不再取走
  异步候选。
- **复审加固（2026-11-09 第四轮）**：① 回放把**所有**未失效 `SurfaceReplacement` 的
  `pruned_tool_outputs` 合并施加（后写覆盖同 call_id），连续两次降级各剪不同输出时
  resume 不再恢复第一个的完整长度；② 轮内降级的工具输出剪枝升级为预算驱动——固定
  头/尾窗口不够时，循环把"当前最大的输出"窗口减半（至多
  `MAX_PRUNE_ESCALATION_ROUNDS = 6` 轮），多条工具输出合计超窗不再误报"输入过大"。
- **复审加固（2026-11-09 第五轮）**：① 回放按**记录顺序**施加全部剪枝窗口（同
  call_id 可多条、逐条演化），与实时路径共用同一投影函数，连续收缩后 live 与
  resume 的模型可见文本逐字节一致；② 预算驱动剪枝覆盖 `CustomToolCallOutput`，
  上限提到 32 轮并带"无进展即停"检查。
- **复审加固（2026-11-10 第六轮）**：① 轮内剪枝的重试目标改为
  `min(宣称窗口, 观测拒绝点)`（此前用宣称窗口，被拒后可能原样重发）；到 stub 的
  输出标记跳过、继续剪下一条，不再整体退出。② `app-server` v2 新增
  `CodexErrorInfo::InputTooLarge`（wire `inputTooLarge`），harness 的
  `InvalidPrompt`（必要内容超窗）不再翻成 `Other`，客户端经适配器得到
  `input_too_large`（category `request`、actions `[]`），端到端贯通。
- **会话不弃（同日起）**：产品目标是"一个会话一直用、可自由换模型"，所以超窗的
  终态提示不再让用户"开新线程"——`ContextWindowExceeded` 文案改为
  "the conversation is kept; retry, or switch to a model with a larger context
  window"，ACP 侧 `context_exhausted` 的 action 从 `new_session` 改为 `retry`；
  降级警告也改为"本地记录完整，模型需要该细节时换更大窗口的模型"。会话内存中的
  裁剪由 `SurfaceReplacement` 持久化，跨模型继续可用。
- **品牌**：`CodexErrorDetails::ContextWindowExceeded` 的文案改为
  `nuwax-harness ran out of room in the model's context window. ...`。
  机器判据只能是 `codex_error_info == "context_window_exceeded"`；任何按
  ASCII 前缀匹配 "Codex ran out of room" 的客户端（iOS 输入上限分类器）必须
  同批次改判该字段。


### 5.6 thinking / reasoning_effort 默认不传（2026-11-10 起）

**背景**：厂商档位表互不相同（GLM-5.3 只 `low/high/max` 且**不能关闭思考**、官方
默认 `max`；OpenAI GPT-5.x 有 `medium`；现代 Claude 默认开思考），而 ACP 适配器
曾对"无显式档位"硬编码兜底 `medium`，写进 thread settings 后 GLM-5.3 每请求必被
档位门拒。协议侧三家官方文档一致：**该字段非必填，省略 = 模型厂商默认**
（OpenAI 推理指南、Anthropic thinking 总览、GLM-5.3 模型文档）。

**契约**：

- **未设置（用户/环境变量/后端都没给）→ 两个 wire 都不发** `reasoning_effort` /
  `thinking`，厂商按自己的默认执行。适配器身份串用哨兵 `model[default]`（UI 显示
  "Vendor default"），`ModelId.fromString` 也接受无括号的裸 model id（映射为哨兵）——
  裸 id 此前会抛错，使客户端的模型同步静默失效。
- **哨兵永不作为档位下发**：`createCodexCollaborationMode`（resume 与模式切换的
  thread settings 写入路径，即事故路径）把哨兵映射为 `null`。
- **显式设置照发**，档位门校验；`none`（关闭思考）遇"始终思考"网关 fail-fast 并
  点名模型与应设的变量（适配器 `applyModelAndEffort` + 引擎档位门双侧）。
- **后端/环境变量显式设置优先级最高**（`NUWAX_MODEL_REASONING_EFFORT` /
  `CODEX_MODEL_REASONING_EFFORT`）。
- 客户端**不再按模型名播种**档位（删除两处 GLM-5.3→low 播种）；确需低成本默认由
  后端统一下发，单一策略源。

**引擎侧既有行为（本轮加测试锁定，无行为变更）**：`convert_request.rs` chat wire
`if let Some(effort)` 才插键、anthropic wire 走 rig 类型化通道（`rig_reasoning(None)`
返回 `None`）；档位门对 `None` 放行。目录模型 `default_reasoning_level` 全为
`None`——引擎本来就不会发明档位；注入者只有适配器（已修）与
`multi_agents/spawn.rs` 的 `unwrap_or_default()`（= `Medium`，已改为 Option 透传，
子智能体不带 effort 时不再注入）。

**overlay `default` 字段是探测记录而非请求值**：`effort_table_for` 只取 `levels`；
**禁止**把 `default` 接进 `default_reasoning_level`（那会进入
`effective_reasoning_effort` 变成发送值，违背"默认不传"）。

**已知边界**：档位门（catalog overlay 档位表）只挂在 **chat wire**；Responses 线
由网关自行校验（实测 GLM Responses 端点对 `medium` 宽容、chat 端点拒绝）。引擎
模型列表对无默认档的模型报 `defaultReasoningEffort: "none"` 占位，适配器已显式
将其视为"未指定"而非档位。

**审批默认问用户、不问模型（2026-11-10，适配器侧）**：上游 ACP 适配器的默认
session mode 是 `agent`（"Auto review"：`approvalPolicy=on-request` +
`approvalsReviewer=auto_review`），每个需要审批的动作都交给引擎的 guardian
reviewer——**一次模型调用**——由它代替用户批准。nuwax-client 自己就有 ask/yolo
权限协调器（yolo 本地自动放行、ask 弹窗），被 guardian 抢先后再也收不到
`session/request_permission`（实测 842 次 guardian、0 次权限请求，ask 模式形同
虚设）。fork 把默认 mode 改为 `workspace-write`（**沙箱与原默认完全相同**，只是
审查者从模型换成用户）；`agent` 仍可在 mode 列表里显式选。引擎侧
`approvals_reviewer` 配置默认仍是 `user`，TUI 一向如此、不受影响。

**压缩工具调用一定有终态（2026-11-13）**：ACP 用 item id 收尾工具调用，所以任何
`ItemStarted` 都必须配一次 `ItemCompleted`。此前的两类漏口径：

- **id 不同**：迁移压缩的 `finish_announcement` 另建了一个 item 去发完成，客户端因此
  留下一个永远 in_progress 的 `Compact conversation`，旁边多一条没有 start 的
  completed（`core/src/compact.rs` 的 `MigrationAnnouncement` 已修）。
- **缺完成**：主压缩路径的取消/超限/重试耗尽、`compact_remote_v2` 与 installation 的
  `Err` 早退都只发过 start。现在 `run_compact_task_inner_impl` 由外层 wrapper 统一补
  completed，`remote_v2`/installation 在 `Err` 前补。

适配器侧（`fork-codex-acp-ts` 1.2.36 起）对 ACP v1 客户端补了兜底：登记已 announce 的
压缩 id，完成事件 id 对不上时按 announce 的 id 收尾；turn 结束时仍未终结的压缩补终态
（interrupted→failed+取消文案）。终态带英文结果文本：`Context compaction completed.` /
`Context compaction failed.` / `Context compaction cancelled.`（ACP 没有 cancelled 状态，
取消也发 failed，用文案区分）。

**workspace-write 默认放行出站网络（2026-11-13）**：上游 `SandboxPolicy::WorkspaceWrite`
的 `network_access` 缺省 `false`，在本地 CLI/客户端部署里等于每次 `curl`、包安装都要
走一轮审批。fork 把**缺省值**改成 `true`，`config.toml` 里不写
`[sandbox_workspace_write]` 表时也按放行构造 `PermissionProfile`；写盘边界不变（仍只
允许工作区与显式 `writable_roots`）。收紧仍是一行配置：
`[sandbox_workspace_write] network_access = false`，客户端显式发 `false` 同样生效；
`read-only` / `danger-full-access` 不受影响。落点：`protocol/src/protocol.rs`（serde
default）、`config/src/types.rs`、`config/src/config_toml.rs`（无表路径）、app-server
v2 的 `SandboxWorkspaceWrite` 与 `SandboxPolicy`（线格式默认值同步，否则客户端省略字段
时会重新收紧），以及随之重生成的 `core/config.schema.json` 与
`app-server-protocol/schema/**`。

**超大截图先缩小再丢弃（2026-11-10）**：工具输出的图片入口界（单张 16 MiB
编码后、合计 24 MiB、`detail: Original` 按 16×16 patch 计费至 10000 token）此前
是**纯丢弃**——一张 12 MB 的 Retina 全页截图（base64 16.35 MB）会整张变成一行
"[omitted 1 image(s): ...]" marker，模型完全看不到画面（用户 agent 实测反馈）。
现在入口前加三级收缩：**① 像素**——超视觉预算的图先把最长边压到
`longest_edge_for_visual_budget`（10000 token 预算 → 约 1430px，只降尺寸不降质量，
因为 UI 文字在小图上比低画质上更可读）；**② 单张字节**——JPEG 质量阶梯
85→75→65，再不够才走尺寸阶梯 2048→1600→1280→1024；**③ 合计字节**——从最旧的图
开始收缩到剩余预算（最新的图是本轮证据，优先保像素）。仍放不下的才走原丢弃路径
（含 marker）。无损优先：alpha 全 255 的截图打平转 JPEG（视觉无损），真半透明保
PNG。无法解码的 payload 原样透传、交给入口丢弃。

**resume 的覆盖边界：与用量同源，且不跨越表面重写（2026-11-11 三次修正）**

这一项改了三轮，前两轮都不够正确，记录清楚免得再踩：

1. 第一版把**全部重载条目**标成已覆盖 → 漏算上报之后追加、随后被中断的条目。
2. 第二版数"最后一次 `TokenCount` 之前的 `ResponseItem` 条数" → 两个缺陷：(a) 用量查找
   会跳过 `info: None`，边界却取最后一个任意 `TokenCount`，两者可能指向**不同事件**；
   (b) 该计数取自**原始 rollout**，而 `Compacted`/`SurfaceReplacement` 会重写它前面的
   表面（100 条压成 5 条再追加 2 条 → 原始计数 100、重载后 7，`min(100,7)=7` 把两条
   未上报的也标成已覆盖）。
3. 现行：`last_token_usage_with_coverage` 一次性返回**同一个事件**的用量与边界；
   若该事件**之后**有任何表面重写，覆盖数直接记 0（宁可在估算上保守，也不能把真实条目
   藏起来——少算只会提前压缩，多算会漏掉内容）；否则只数**最后一次重写之后**到该事件
   之间的 `ResponseItem`。调用方再 clamp 到重载条目数。
   `token_usage_coverage_tracks_the_same_report_and_respects_rewrites` 覆盖四种组合。

4. 第四版（现行）：不再枚举改写类型，改用**顺序无关的不变式**——只有当"rollout 里的
   `ResponseItem` 总数 == 重载后的历史条目数"（重建无损）时才信任记录的位置，否则覆盖数
   记 0。列举法已经漏过一次 `ThreadRolledBack`（两轮已上报 → 回滚第二轮 → 追加一条未上报
   输入：重载成 2 条，新输入被算作已覆盖）。比较两个总数能覆盖压缩、表面替换、回滚、
   丢弃孤儿等全部情形，无需逐个识别。

5. 第五版（现行）：总数相等**也不够**——"回滚删一条 + 子智能体消息加一条"能让总数不变
   而位置全移。现在对**已知会改变表面位置的条目**（`Compacted`、`SurfaceReplacement`、
   `InterAgentCommunication`、`EventMsg::ThreadRolledBack`）直接否决覆盖数，再叠加
   总数相等作为兜底；任一命中即 0。

代价：被压缩/回滚的会话在 resume 后退化成"整段重估"，可能重新出现提前压缩。彻底修法
是在**回放过程中**同时推导边界（回放是逆序的，需要给 `RolloutReconstruction` 增加
"有效用量 + 对应重建位置"的字段），作为独立提案推进。

**重启后立刻压缩的真因：恢复用量时丢了边界（2026-11-11 复盘修复）**：
`get_total_token_usage()` = 上次服务端用量 + 未上报条目的估算。恢复会话时只调了
`set_token_info`，没设 `last_server_reported_usage_item_count`，边界停在 0——于是
**整个重载历史被再估算一遍叠加上去**，14.8 万 token 的会话看起来像几十万，
`full_context_window_limit_reached` 在第一条消息就为真。实测：7 次引擎重启，
每次 **+0.9 秒**都出现一次 Compact conversation。改为 `set_token_info_with_boundary`
（覆盖数 = 重载条目数），并加回归测试
`resuming_anchors_the_token_count_to_the_recorded_usage`（断开边界时断言"必然多算"）。

同批代码审查修复：

- **用户上传的图片绝不被剪枝**：`prune_stale_images` 原先把 `Message` 里的图片也计入
  候选，用户传三张会有一张被换成占位符——那是用户自己的输入；且 Message 没有 call id，
  剪了无法写进 `SurfaceReplacement`，导致实时与重载不一致。现在只剪工具输出。
- **413 在 token 合规时也真的剪**：`degrade_history_to_target_window` 原先
  `total <= history_budget` 就直接返回"没裁"，于是 body 超限的重试原样再发一次 → 再
  413 → 终态。现在 `CompactionOverflow` 会走完确定性剪枝（旧图 + 超大工具输出），并
  照常记录回放。
- **迁移压缩不再抢跑**：它原先每轮按 `ratio*窗口`（≈80%）无条件跑，绕过惰性轮前判断。
  现在只在**换模型**或**真的装不下**时执行。
- **单图合规时仍做合计收缩**：`shrink_oversized_tool_output_images` 原先"每张都合规"
  就 return，多张图合计超限时进不到 Pass 2，只能被入口整张丢弃。
- **fail-fast 认环境授权头**：`has_authorization_header` 只看静态 `http_headers`，
  漏了 `env_http_headers.Authorization`——合法配置会被误拒。
- **配置 schema 补 `image_byte_budget_bytes`**（`config_schema_matches_fixture` 此前
  实红）。
- **图片字节预算改为随调用传递**：它原本是进程级 `AtomicU32`，每次配置加载写一次——
  两个会话配不同预算会互相覆盖，失败的加载也会改写。现在预算作为参数穿过
  `prepare_response_items → prepare_message_content / prepare_tool_output_content →
  prepare_image`，由调用方从**会话自己的 config** 读取（`image_byte_budget(&config)`），
  全局单元与其 setter 已删除。guardian 审阅图走 `resize_image(..., None)`（不设预算）。

**压缩惰性化：不在每轮开头按 80% 线压缩（2026-11-11）**：`run_pre_sampling_compact`
此前只要 `token_limit_reached`（自动压缩额度，未显式配置时=窗口的 80%）就压缩。
结果是一个停在 80% 出头的会话，**每发一条新消息都先压一次**——UI 上表现为上一轮
回答后面冒出一块 "Compact conversation"、用户要等几分钟才看到自己的问题被处理。
改为只在**真的装不下**时压缩：`full_context_window_limit_reached`（窗口有效上限，
含观测拒绝点）、本轮待加入输入让总账越线，或**显式配置的**自动压缩额度被击穿。
最后一条是关键区分：派生出来的 80% 是调度启发式（该忽略），而
`model_auto_compact_token_limit` 或目录声明值是运维的**意图**（该遵守）——
`ContextWindowTokenStatus.auto_compact_limit_is_explicit` 记住这个来源。其余场景由
既有机制兜底——轮内检查点（自己的 90% 线）会在工具循环里拦住漂移，
`fork_overflow_recoverable` 在网关真拒绝时反应式自愈。

**413 走完整自愈链 + 压缩剪掉保留尾部的旧截图（2026-11-11）**：两处实战缺陷。

- ① **413 在压缩路径上变成终态**：采样 413 → 触发压缩 → 压缩请求本身也带全量
  历史 → 又 413 → 三个"压缩失败"分支只认 `ContextWindowExceeded`，`RequestTooLarge`
  被当未知错误直接返回，于是**从未走到降级链**，用户只看到终态错误。三处判断
  （轮前、轮内、guardian 待处理输入）都改为同时接受 `RequestTooLarge`。
- ② **截图密集任务无限压缩**：正常摘要压缩只切前缀，保留尾部按 `retain_ratio`
  （0.16）想留 30k，但当前轮全是浏览器截图、自己就 100k+，结果压缩后仍贴近 80%
  触发线，下一轮立刻再压——UI 上就是连续三条 "Compact conversation"、数分钟没有
  回复。修复：摘要成功后对**保留尾部**同样做确定性剪枝（旧图换占位、保留最新两张），
  并把剪枝窗口写进 `SurfaceReplacement.pruned_tool_outputs`，回放逐字节一致。

**未配置时的 fail-fast（2026-11-10）**：`to_api_provider` 在"无 base_url、无 auth
mode、provider 无 key、headers 无 Authorization、无 aws/auth.command/gateway_oauth、
环境无 OpenAI key"时直接报错点名配置项，不再发出必然 401 的请求——内置默认域名
因此不会出现在用户可见的错误里（产品考量：避免用户对国外域名产生数据泄露疑虑）。
显式选择官方 OpenAI（配 key 或 ChatGPT 登录）完全不受影响；AWS / `auth.command` /
`gateway_oauth` 等凭据在别处的 provider 也不受影响。

**已知限制（2026-11-09，观察中）**：guardian reviewer 自己的回合超窗时仍按
fail-closed 失败并丢弃 reviewer 会话（`review_session_setup` 只在 Reusable 时
保存 checkpoint）。这不影响用户会话：下一次审批会从干净会话重建。长会话里可能
出现"审批周期性失败"的抖动，先只收集结构化日志（窗口/证据 token/丢弃原因），
再决定是否做 reviewer 有界化。详见方案文档 §12。

## 6. daemon：npm 安装不自启（v0.18.8）

官方 TUI 默认自启共享 daemon，需要"完整本地包布局"（codex-package.json、
code-mode-host、rg）。先前fork npm包只发单二进制，自启所需布局不完整；v0.18.14补入Code Mode host，
但仍不宣称具备上游完整daemon bundle（如独立rg）
。fork 在
`tui/src/daemon_startup.rs::exclusion` 排除 NpmNuwax 安装方式（安装方式参数
注入，hermetic 可测），TUI 走内嵌 app-server；显式 `app-server daemon` 命令不受影响。

## 7. fork 专属字段与环境变量总表

**config.toml 键**（官方 schema 不认识，只写 `~/.nuwax-harness`）：
`model_providers.*.wire_api = "chat"|"anthropic"`、`experimental_bridge`、
`experimental_bearer_token`、`model_auto_compact_ratio`、
`model_providers.*.anthropic_prompt_caching`（2026-10-26 起：Anthropic 线 opt-in
cache_control 断点，rig 0.45 原语，5m TTL；远程 thread config 无该字段线格式，
同步时丢弃、恢复为 None，同 `max_output_tokens` 的既有边界）、
`[tools] tool_search = false`（2026-10-26 起：MCP 工具全量直出开关，见 §7.1）、
`model_providers.*.models`（2026-11-04 起：声明式模型列表，见 §7.2）。

**环境变量**：`CODEX_INSTALL_SOURCE=npm_nuwax`（launcher 设置，install-context
识别 NpmNuwax）、`CODEX_MODEL_REASONING_EFFORT`、`CODEX_MODEL_CONTEXT_WINDOW`、
`CODEX_AUTO_COMPACT_TOKEN_LIMIT`、`CODEX_AUTO_COMPACT_RATIO`、
`CODEX_TOOL_SEARCH=false|true`（2026-10-26 起：种 `[tools] tool_search`，见 §7.1）。
**NUWAX_ 前缀别名**（2026-10-28 起）：用户可设的 `CODEX_*` 变量均接受同义
`NUWAX_*`——`NUWAX_HOME`、`NUWAX_MODEL_REASONING_EFFORT`、
`NUWAX_MODEL_CONTEXT_WINDOW`、`NUWAX_AUTO_COMPACT_TOKEN_LIMIT`、
`NUWAX_AUTO_COMPACT_RATIO`、`NUWAX_TOOL_SEARCH`。解析规则统一在
`utils/home-dir/src/env_alias.rs`（NUWAX 优先；同值幂等；异值 fail-fast 报双
变量名——脑裂配置不得静默选边）；值为 OsString 直通，Unicode 校验留在各消费点
既有边界（种子层显式 `-c` 优先检查仍先于 env 校验，非 Unicode 值仅在真正被
使用时报错；home 解析沿用忽略非 Unicode 值的旧行为）；`find_codex_home`、全部 CODEX_HOME 散读点
（config symlink/doctor disk/daemon background_command/Windows provisioning）、
种子层（utils/cli）与 daemon 子进程剥离名单（NUWAX 种子随 CODEX 种子一并剥）
均已接线；launcher 注入的 `CODEX_INSTALL_SOURCE`、包管理器 shim 的
`CODEX_MANAGED_BY_*` 与程序间传信变量不加别名（非用户配置面）。

**图片请求预算（2026-11-12 起）**：`NUWAX_REQUEST_IMAGE_MAX_BYTES`（字节数，
默认 20 MiB）限制一次请求里内联图片的总量，超了从最大的一张开始降采样，见 §4
"图片字节预算"。`NUWAX_IMAGE_MAX_BYTES`/`image_byte_budget_bytes` 仍是单图预算：
未设置为"按模型自动"，`0` 为关闭。

**OpenAI 出站总闸（2026-11-12 起）**：`NUWAX_OPENAI_EGRESS`（别名
`CODEX_OPENAI_EGRESS`，`NUWAX_` 优先）默认**关闭**。fork 面向的是经自有网关访问
厂商模型的部署，"抓包看到 chatgpt.com"或"日志里出现本机 hostname +
installation_id"是信任事故，不是文案问题，因此上游那些会自行联网的启动行为统一
过这道闸：
- app-server remote control：向 `https://chatgpt.com/backend-api/` 开 websocket，
  并把 `installation_id` 与本机 `server_name` 写进日志；未登录时会以约 1 Hz
  循环解析偏好（日志实测 1500+ 行/天）。
- 插件启动任务：`chatgpt.com/backend-api/plugins/*`（featured、目录、已安装态
  同步）与 curated 同步（`api.github.com`、`github.com/openai/plugins.git`、
  `chatgpt.com/backend-api/plugins/export/curated`）。其中 featured 请求在
  API-key 鉴权下**不带认证头也照发**，必然 401。
- 落点：`AppServerRuntimeOptions::default()` 取 `PluginStartupTasks::Skip` 与
  `RemoteControlStartupMode::DisabledEphemeral`，`codex`/`codex app-server` 两个
  二进制仅在闸开时才改回 `Start`/`ResolvePersisted`。判定点
  `codex-http-client/src/openai_egress.rs`。
设为 `1`（`true`/`yes`/`on` 同义）恢复上游行为；无法识别的值按关闭处理并告警
一次（不静默选边）。显式 `--remote-control` 仍然生效——那是操作者主动要求。
Analytics 不在此闸内：仍由 `[analytics] enabled` / `--analytics-default-enabled`
控制（默认关），开启后同样指向 `chatgpt.com/backend-api/codex/analytics-events`，
需要彻底禁网时两处都要关。

**provider 级输出预算**（2026-10-01 起）：`model_providers.*.max_output_tokens`
（正整数）→ 桥线输出 cap（Anthropic `max_tokens` 覆盖默认 16384；Chat 可选，
Rig 对部分 OpenAI reasoning 模型转换为 `max_completion_tokens`）；
Responses typed HTTP/WS 请求采用 `max_output_tokens`，未配置时省略字段。远程 thread config 尚无此字段线格式（转 None）。

**桥性能/安全项**（2026-10-01 起）：
- 连接复用：每个 protocol 最多一个不保存请求头的 reqwest 连接池（进程内共三个，
  请求头和鉴权按轮注入；custom CA 路径仍逐轮构建）。
- tee 有界：Anthropic wire tee 与 cassette 录制缓冲上限 8 MiB，溢出停采并告警
  （流本身不受影响；块恢复只解析完整帧，截断尾不产生半块）；未声明 hosted 工具
  时 tee 整体跳过。

**NUWAX 启动组**（2026-10-01 起；来自真实进程环境，`$CODEX_HOME/.env` 不可注入）：
`NUWAX_BASE_URL`+`NUWAX_WIRE_API`（responses|chat|anthropic）+`NUWAX_API_KEY`
三者齐设激活本次运行的临时 provider `nuwax_env`（凭据仅以
`env_key="NUWAX_API_KEY"` 引用，不落 config/rollout/诊断），`NUWAX_MODEL` 可单
独选既有 provider 的模型。优先级：typed CLI（-m/--oss）> 显式 `-c` > 本组 >
config 文件；显式选其他 provider 时整组忽略。CLI/TUI/exec/standalone
app-server 同语义接线；组激活时共享 daemon 被排除（每进程凭据），doctor
`--json` 新增 `config.model_routing` 检查。部分设置/非法值 fail-fast 报变量名。
2026-10-03 增加 `NUWAX_MAX_OUTPUT_TOKENS`，三协议均可用；Responses 使用
`max_output_tokens`。2026-10-04 增加 `NUWAX_REQUEST_MAX_RETRIES`（0=只发一次）、
`NUWAX_STREAM_MAX_RETRIES`、`NUWAX_STREAM_IDLE_TIMEOUT_MS`（毫秒）：与组同激活、
同优先级（typed CLI > 显式 `-c` > env），孤立设置 fail-fast，daemon 子进程启动时
剥离。握手重试与 Core 采样重试分开计数，无单一总预算。
2026-10-20 增加 `NUWAX_BRIDGE`（值域同 provider 的 `experimental_bridge`：
`rig`|`native`|`genai`）：种子层随之写入 `experimental_bridge`，让官方 GPT 经中转
端点时用上游原生 Responses 传输而不是 rig 桥；`native` 必须配
`NUWAX_WIRE_API=responses`（chat/anthropic组合明确报错，不静默忽略），未知值与孤立设置同样 fail-fast 报变量名。未设置时仍是 fork 默认 `rig`，
daemon 子进程同样剥离该变量；doctor 的 `config.model_routing` 收据相应显示
`bridge: native transport` 并列出 `experimental bridge override:`。
多进程应分别设置独立 `CODEX_HOME`；参数表、启动示例、远程与会话
边界见 [容器多进程审查](my-docs/container-multiprocess-env-review-2026-10-03.md)。
（ACP-TS 侧另有 `CODEX_BASE_URL`/`CODEX_API_PROTOCOL`/`CODEX_WIRE_API`/
`INITIAL_AGENT_MODE` 等，见该仓库。）
ACP的CODEX_*入口显式选择custom-gateway；自 2026-10-28（fork-codex-acp-ts
`7e65bde`）每个网关 CODEX_* 变量均接受同义 NUWAX_* 别名（解析规则同引擎
env_alias：NUWAX 优先/同值幂等/异值报双变量名；错误不回显值），消费掉的
别名从 app-server 子进程环境剥离，故引擎 NUWAX_* 启动组不受孤儿影响——
原先"NUWAX_BRIDGE 刻意不做别名"的规则由此废止并为测试锁定新语义。native仅允许OpenAI Responses，非法组合
启动失败。环境网关已授权，无需显式authenticate；显式providers/set或任何authenticate
请求按既有优先级替换/清除该环境配置。隐藏标题线程复制完整活动provider定义，防止
遗漏bridge/headers/query或回退到固定官方模型。28/28子进程fixture与GLM原生实网4/4
见[收尾记录](my-docs/rig-production-completion-2026-10-10/codex-core-closure-2026-10-22.md)。

### 7.1 `[tools] tool_search`——MCP 工具全量直出开关（2026-10-26）

**背景**：官方 codex 对支持 Responses search-tool 能力的模型默认启用 MCP 懒加载——
MCP 工具注册为 Deferred、不进首个请求的工具列表，模型经 `tool_search`（BM25 检索，
默认 limit 8）按需取回 schema；另有连接级懒启动（`LazyWhenCached`，仅 SubAgent）。
自托管/第三方网关常需工具全量直出（网关不识别 `tool_search` 线格式、或部署侧要
确定性工具表），官方无任何开关（`features.tool_search` 已 `Stage::Removed`，无
环境变量），fork 增设配置键。

**用法**（config.toml 或 profile）：

```toml
[tools]
tool_search = false   # MCP 工具在首个请求全量直出，不注册 tool_search
```

`true`/不设 = 官方默认（跟随模型目录的 `supports_search_tool`）。profile 同样生效：
`[profiles.<name>.tools] tool_search = false`。环境变量
`CODEX_TOOL_SEARCH=false|true`（2026-10-26 起，与其他 `CODEX_*` 种子同优先级：
显式 `-c tools.tool_search=…` > env > config.toml；仅接受大小写不敏感的
`true`/`false`，其他值带变量名 fail-fast；daemon 子进程启动时剥离）。

**实现**（`8c6a60193`，与官方差异的完整链路）：

- 唯一判据：`ModelInfo.supports_search_tool`。core `spec_plan.rs` 的 5 个分支
  （MCP 注册 Deferred/Direct、exposure 策略裁剪、`tool_search` 注册、deferred
  指引文案、扩展工具 exposure）全部由它派生，翻转它即可整体关闭懒加载，无旁路。
- 单点收口：models-manager `model_info.rs::with_config_overrides`——远程目录与
  fallback 两条模型元数据路径的公共终点（`construct_model_info`），未来上游新增
  的 `supports_search_tool` 分支自动跟随。只允许强制 OFF（`Some(false)` 才翻转），
  不会给不支持 wire 特性的模型强制 ON。
- 管线：`[tools].tool_search` → `Config.tool_search_enabled`（默认 true）→
  `ModelsManagerConfig.tool_search_enabled: Option<bool>`（禁用时 `Some(false)`，
  否则 `None`）→ `ModelInfoOverrides` 双向携带（首轮/resume/切模型不丢，同
  `model_auto_compact_ratio` 的既有约束）。所有生产 `get_model_info` 调用点
  （review/child/guardian/doctor 等 11 处）均经 `to_models_manager_config()`，
  子代理/审查会话自动继承。
- schema：fork 的 `core/config.schema.json` 已含该键（官方 schema 无）；顺带修复
  了 fixture 此前漏随 `anthropic_prompt_caching` 更新的 drift。
- 测试：收口单测（强制 OFF/OFF-only/None 不动）、`ModelInfoOverrides` round-trip
  端到端（StaticModelsManager 解析后 `supports_search_tool=false`）、config 解析
  传播三例；行为层由既有 `suite::search_tool` 覆盖（`supports_search_tool=false`
  → 无 `tool_search`、MCP Direct 路径已验证）。

### 7.2 `model_providers.*.models`——声明式模型列表（2026-11-04）

**背景**：env-key 自定义 provider 无 `model_catalog_url` 时，`/model` 列表落入
bundled GPT 种子（11 个 gpt-\*，与该 provider 毫无关系），Enter 会把种子 slug
写进 config.toml。内置厂商目录（models.dev 快照）只有按 slug 的元数据，没有
"该 provider 服务哪些模型"的映射，无法自动推导——列表必须由用户声明。

**行为契约**：
- 声明 `models = [...]` 后，该 provider 的全部列表 surface（`/model`、
  model/list、get_remote_models）恰好给出声明 slugs（保持声明顺序），
  同时替换 bundled 种子、缓存快照与线上 catalog——显式声明最高。
  实现：`SelectedCatalog::Declared`（models-manager），per-slug 携带真实 origin。
- 元数据按 slug 仍走 `vendor_catalog::resolve`：命中 → `ModelOrigin::Vendor`，
  fork 窗口覆盖策略走"显式全额生效"路径（与未声明时 `-m GLM-5.3-Flash`
  的既有行为一致，声明不改变窗口语义）；未命中 → 占位元数据（272k 窗口）
  + 一次性告警提示设置 `model_context_window`。
- 占位元数据 turn 内告警彻底降级（2026-11-07）：占位元数据
  （`ModelOrigin::Synthetic`）原本每个 turn 弹用户可见警告 "Model metadata
  for `<slug>` not found…"。三方网关普遍使用带日期的快照 slug（如
  `...-1017`），任何目录快照都不可能收录——这是文档化状态而非异常，
  **一律只写 `tracing::warn` 日志，不再发 turn 警告事件**（0.18.30 曾按
  "是否命中 provider `models` 声明"分流，未命中的仍可见；终端用户无行动
  价值，ACP 客户端会把它渲染进聊天流）。实现：`core/src/session/
  turn_context.rs::maybe_emit_model_warnings_for_turn`；测试：remote_models
  套件 `fallback_model_metadata_is_never_a_user_visible_warning`（声明与
  未声明都 0 条事件）。ACP 侧另有兜底过滤（`CodexEventHandler::
  createWarningEvent` 丢弃该模式并写日志），覆盖旧引擎。
- 声明治理列表、不治理选择：`-m` / `NUWAX_MODEL` 选声明外的 slug 照常解析
  （走同一 vendor 目录兜底）——声明不是白名单闸门。
- `/model` Enter 持久化守卫（tui model_defaults）：激活 provider 声明了
  `models` 时，非声明 slug 拒绝写入 config 并报错点名 provider；
  未声明 provider 的持久化行为不变。
- 校验 fail-fast（`ModelProviderInfo::validate`）：空列表、空/空白 slug、
  重复 slug（大小写不敏感）在 config 加载即报错。
- 优先级：`model_catalog_json`（StaticModelsManager，最重逃逸口）> `models`
  声明 > `model_catalog_url` 发现 > bundled 种子。
- 远程 thread config 线格式新增 `models`（repeated string，tag 27；空 = 未
  声明），from/to proto 双向携带——顺带修复了 `.proto` 文件中合并残留的
  重复字段块（experimental_bridge/max_output_tokens/hosted_results_replay
  与上游 project_id/location/credential_file 的 20/21/22 号冲突；vendored
  生成代码本身无此问题）。
- 内置 key（如 ollama）的 Occupied 合并例外在 `experimental_bridge` 之外
  现也携带 `models`（本地模型策展进 `/model`）。

**测试**：声明解析/round-trip、校验 fail-fast、merge 携带
（model-provider-info）；声明列表替换种子/缓存/远端 + Vendor origin 与窗口
策略保持 + 声明外 slug 逃逸（models-manager）。

**2026-11-05 审查修复**：`model_providers.openai` 配 `models` 由静默忽略改
为 fail-fast 报错；声明存在时跳过 catalog 网络刷新（原"拉了再丢"+ 假错
误日志）；远程 thread config 的 proto 路径补空 slug 校验；Declared 查找改
大小写不敏感（与 TUI 持久化门对齐，防大小写变体静默降级占位元数据）；
Declared 目录在 `with_declared_models` 一次性物化缓存（原每轮 turn 全量重
建，每 slug 含一次 ~20KB 指令模板渲染）。

### 7.3 多厂商会话切换——恢复侧入口（2026-11-04，P3+P2 核心）

提示条新增 **`r` = resume with current model**：以当前
config 的 `model`+`model_provider` 显式覆盖重新 `thread/resume`（协议原语
2026-11-03 实测：跨厂商 resume 成功、线程 re-home、后续无覆盖 resume 亦正常）。
（2026-11-05 起提示条只在"provider 已不再配置"或策略冲突时出现，见本节末"路由不阻塞"。）

- **适用变体**：`ModelProviderMismatch` 与 `ModelProviderNotConfigured`
  （当前配置能接手即能 re-home）；`PolicyConflict` 不给该键（组织要求的
  provider 不是用户能绕的）。
- **实现**：`ResumeSource::ActiveWithCurrentModel`（config 刷新同 `Active`，
  仅 model_settings 强制 `OverrideFromCurrentConfig`）→
  `resume_target_session_with_source`；提示条 footer 条件渲染 `r` 快捷键。
- **与 `c` 的区别**：`c` 开新会话、把旧转录当文件引用；`r` 保留同一会话与
  全部历史，thinking 跨源按既有 envelope 机制丢弃。
- **CLI 等价**：`resume <id> -c model=… -c model_provider=…`（早已支持）。
- 快照锁定：`provider_errors`（mismatch/未配置带 `r`，policy 不带）。

**`/model` picker 跨厂商条目（P2 核心，2026-11-04）**：All-models 列表在当前
provider 的模型之后追加"其他厂商"行——数据源是各 provider 声明的
`models` 列表（§7.2；未声明 `models` 的 provider 不出现——没有真实列表可
展示）。每行带 provider 显示名（category tag/description），选中（二次确
认，footer 说明"历史保留、旧模型 thinking 不跨厂商"）即以
`ResumeModelSettings::Explicit { model, model_provider }` 走同一 re-home
resume;`env_key` 声明了但环境变量为空 → 行置灰并提示 `set $KEY …`;
运行中的轮次不热切换：记录最后一次选择，收到该轮 `turn/completed`（正常完成或停止完成）后自动切换；重复选择覆盖待切换项。等待期间暂停后续输入自动发送，切换完成后恢复；离开该会话取消待切换项。同 provider 的后续选择沿用官方设置更新路径并覆盖此前的跨厂商待切换意图。`nuwax_env` 保留 ID 不列出。切换是会话级的,
不写 config.toml;失败的 resume 按既有错误路径提示，原会话不动。

**同一会话切换修复（2026-11-04）**：显式跨厂商选择不再被当前线程的
`Already viewing` 导航去重拦截；恢复前先解除本客户端订阅，使 app-server
能够重建空闲线程并应用路由覆盖（其他客户端仍订阅或线程忙时，不强制接管）。
恢复结果必须匹配所选 model/provider，否则明确报切换未生效；成功后同步
TUI 的完整 provider 配置与模型默认值，保留同一线程、历史和输入草稿，
不改磁盘默认厂商。恢复失败会尝试重新订阅原线程并显示错误。
回归覆盖 MiMo→Step→MiMo 的 resume 参数、返回 provider、界面当前模型、
草稿和历史保留，以及执行中多次选择、正常完成/停止后仅应用最后一项。
协议测试与真实 app-server 集成测试分开；本机真实集成曾阻塞于 macOS
FSEvents 注册（首次 resume 阶段），不以协议模拟通过宣称真实集成已通过。
**未开始会话的切换（2026-11-05，实测修复）**：刚启动、还没有任何 turn 的会话
点跨厂商模型会失败——`thread/resume` 只能从**已落盘**的 rollout 重建，新会话
（rollout 路径已分配、文件尚未生成）会被 app-server 以
`no rollout found for thread id …` 拒绝，TUI 显示
`Failed to switch model: thread/resume failed … (code -32600)`。空会话没有需要
保留的内容，所以切换改为**用目标路由新开一个会话**：`thread/read` 先确认当前
会话没有 turn（读取失败同样视为"未开始"），再走
`AppServerSession::start_thread_on_route`——它在 `thread/start` 参数里显式带上
`model`+`model_provider`，因为 `thread_start_params_from_config` 只转发由
session/env/profile 层选中的 provider，纯 config.toml 默认值会被传成"无偏好"、
由服务端按自己的默认值落线（实测会把 step 落回 mimo）。草稿随
`restore_input_after_model_switch` 带过去；有 turn 的会话仍走原 resume 路径，
并保留"rollout 被删时按同一错误回退到新会话"的兜底。
**路由必须同时进会话层**：`thread/start` 的类型化 `modelProvider` 参数不进线程的
config layer stack，而 turn 准入用 `load_latest_config_with_session_layers` 重放
层栈——只传类型化参数时首条消息会被判成 `ModelProviderMismatch`（UI 显示
"read-only / Continue in a new chat"）。所以起线程时把 `model`+`model_provider`
一并写进 `params.config`（会话级覆盖）；回归断言了 `thread/start` 的
`config.model_provider` 与 `modelProvider` 两个位置都带目标路由，并**实际发起一次
`turn/start`** 验证切换后的会话可直接继续对话（该检查正是只读条的来源）。
目标是"同一会话内跨厂商切换后继续使用"，因此切换成功即用会话层路由，
不出现只读提示。
**路由不阻塞（2026-11-05 用户决策，取代此前的只读策略）**：打开/续接一个 provider 与当前
配置不同的会话时**不再设为只读**。会话保留自己创建时的 provider——`load_for_resume` 把它
镜像进会话层，保证真正加载出来的 config 用它（此前只是放行，config 仍跟随磁盘默认，等于
静默换厂商）；准入检查与续接校验在 provider **仍然配置**时降级为 `tracing::info`
（记录 `thread_provider`/`config_provider`）并继续，不再返回 `ModelProviderMismatch`。
只有两种情况仍会拦：该 provider **已不再配置**（`ModelProviderNotConfigured`）与组织
策略要求的 provider 冲突（`ModelProviderPolicyConflict`）——这两种 UI 才出现带 `r`/`c`/`n`
的提示条。`ModelProviderMismatch` 变体仅为协议兼容保留，当前无生产路径产生它。
上游断言"默认选择变化必须拒绝既有会话输入"的用例按新语义重写
（`model_provider_enforcement` 的 default 分支、`thread_resume_recovers_after_provider_configuration_is_restored`）。

**跟进项已闭环（2026-11-04 晚，三层根因）**：真集成变体失败并非"rejoin 顺序"
本身，而是一整条链：①`load_for_resume` 与 turn 准入检查同源的磁盘漂移比较
把显式跨厂商 resume 全部拒掉（P0 的孪生 bug）——现改为**客户端显式指定
provider 时该指定即本次 resume 的权威选择**，只跳过磁盘比较，策略/存在性
校验保留，无显式指定时只读保护不变；②resume 成功后未把有效路由写回存储
——现在 resume 响应组装前把 `model_provider`+`model` 直接 upsert 进
durable thread store（manager 的 loaded 路径只进 staging，物化前读不到）；
③Legacy 合并读（`read_thread.rs`）用 rollout 线程替换 sqlite 线程时回填了
model 却没回填 provider——现补"sqlite 观测 provider 非空且 ≠ 存储默认时
覆盖 rollout 头"（默认值填充的修复行仍让位 rollout，上游测试语义保留；
**已知边角**：re-home 到恰好等于存储默认 provider 的会话读侧仍显示旧头）。
真集成变体已恢复（取消 `#[ignore]`）。

**2026-11-05 审查修复批**（10 角度代码审查 + 逐条验证后落地）：
①`load_for_resume` 把显式 typesafe `model`/`modelProvider` 镜像进 request
overrides，使其落成 SessionFlags 层——修复跨厂商切换 resume 成功后**首次
turn 准入仍被拒**（typesafe 载体对准入重建不可见；回归测试
`rehome_admission_probe` 锁定全链）；②继承路径的磁盘加载错误恢复 fail-fast
传播（原 `.ok()` 静默跳过漂移检查）；③**行为变化记录**：profile/env-seed
设置的普通 resume 因总携带显式 modelProvider，现在会**静默 re-home** 并落
章（原合并后行为是只读提示）；无 sqlite 元数据的线程仍走漂移检查；④排队
切换：被后续选择（含限流降级提示）取消时现在显式提示、widget 替换路径统
一清理待切项（防跨会话还原抑制标志）；中止轮（无 TurnCompleted 通知面）
的搁浅为已知残留，导航/再选择可解锁。

**P2 剩余（后续）**：目标窗口 < 当前会话用量时的"建议先 /compact"黄色警
告（需要 TUI 侧 token 用量接线；确认交互沿用现有行内二次确认）。P4 配置
向导已落地为 §7.5。

### 7.4 终端状态协议应用标识（2026-11-04）

TUI 发出的 OSC 7501 生命周期状态使用 `app=nuwax-harness`（原 `app=codex`），
包括 idle/working/blocked/done/error；状态名、阻塞原因、清理序列和 iTerm2
OSC 21337 字段不变。快照覆盖全部状态。该改动不重命名 Cargo bin/crate，
也不保证 IDE 按进程名生成的终端标签随之变化；Qoder 标签仍需实际验证。

### 7.5 TUI 厂商配置向导（2026-11-04，P4）

`/model` 的 All-models picker 底部新增 **"Manage providers…"** 行（列表非空
即出现，`tui/src/chatwidget/model_popups.rs`），发出 `AppEvent::OpenProviderWizard`
进入链式弹窗向导：`tui/src/chatwidget/provider_wizard.rs` 渲染各步，
`tui/src/app/provider_wizard.rs` 逐步校验推进（一步一个 `AppEvent` 变体）。

- **步骤 1 预设选择**：8 个精选预设——`glm` / `glm-chat` / `mimo` /
  `mimo-chat` / `deepseek` / `moonshotai` / `stepfun` / `minimax`（GLM/MiMo
  各含 anthropic 与 chat 双入口）+ "Custom…"。**预设仅做 UI 预填**：id/名称/
  base_url/wire_api/凭据（`api_key` 或 `env_key`）/models 皆可编辑，不参与请求行为——引擎侧模型
  元数据仍按 §7.2 走 bundled vendor catalog 的 per-slug 解析。已配置的 id
  在行上标注 "already configured — saving overwrites these keys"。
- **预设路径**：凭据步骤（见下）→ models（逗号分隔，可编辑预填）→ 预览。
- **Custom 路径**：provider id（仅小写字母/数字/连字符；`nuwax_env` 保留，
  拒绝）→ 显示名（留空默认 = id）→ base URL（必须 `http://` 或 `https://`
  开头）→ wire_api picker（`anthropic`/`chat`/`responses` 各带一行说明）→
  凭据步骤 → models → 预览。
- **凭据步骤（§7.6 起重构）**：三选一——**直接 API Key（默认）**、环境变量
  （高级）、无鉴权。选直接键时进入专用隐藏输入框（`render_ref_masked('*')`），
  粘贴的 key 只存在于向导意图中直到确认写入；选环境变量才走
  UPPER_SNAKE_CASE 变量名步骤（留空 = 本地免鉴权网关）。
- **预览步**：逐字展示将写入的 `[model_providers.<id>]` TOML 块（直接键只显示
  圆点状态行，绝不显示值）；覆盖已有 provider 时注明只覆盖所列键、其余保留；
  环境变量方式提示 `export $KEY` 并重启 TUI，直接键方式保存即生效、无需重启；
  "Save provider" 需显式二次确认（`require_explicit_confirmation`）才落盘。
- **写入路径**：`config/batchWrite`（`persist_model_defaults`），每字段一条
  key-path edit——`model_providers.<id>.{name,base_url,env_key,api_key,wire_api,models}`
  （`MergeStrategy::Replace`；`api_key` 为 §7.6 的三态写：缺省 Keep / null Clear /
  值 Set，设置主凭据时同批删除其它主凭据字段）——同 provider 表下未列出的
  其它键原样保留。models 空列表行内拒绝（至少一个 slug），与 §7.2 的加载期
  fail-fast 呼应。
- **安全红线**：key 值不得进入提示、错误、日志、预览 TOML 块或公开配置投影
  （`config/read` 层视图替换为布尔）；只有用户级配置文件本身以 0600 持有明文
  （§7.6）；`nuwax_env` 为保留 id 不允许使用。
- **与 §7.2/§7.3 衔接**：保存后该 provider 声明的 `models` 进入 `/model`
  （P1），All-models 出现其跨厂商行并可会话级切换/re-home（P2/P3）；向导
  本身不改当前会话与磁盘默认厂商。

### 7.6 直接 API Key 凭据模式（2026-11-05）

用户决策:允许真实 key 明文保存在用户级 config.toml（`model_providers.<id>.api_key`），
环境变量降为高级选项（my-docs/direct-provider-api-key-2026-11-05/）。

- **鉴权**：`ModelProviderInfo.api_key: Option<RedactedString>`（Deserialize 接受、
  Serialize 跳过——公开投影永不携带）；`api_key()` 显式键优先、无回退；
  `has_static_api_key_source()` 统一四处散落的显式鉴权判定；与
  env_key/experimental_bearer_token/auth/aws 互斥，加载即 fail-fast（只报字段名）。
  三协议 loopback 实测鉴权头（responses/chat=Bearer、anthropic=x-api-key），双
  provider 不串 key（`direct_api_key` 测试族）。
- **写入**：`modelProvider/config/write` 扩展 wire_api/models/api_key 三态
  （缺省=Keep、null=Clear、值=Set）；设置主凭据同批原子删除其它主凭据字段；
  空白 key、空 models、非法 wire_api、来源冲突写入即拒且不回显值；
  `GatewayAuth::DirectApiKey` 摘要只报已配置状态。
- **脱敏边界**：config/read 的层视图把 api_key 值替换为布尔（`false` 才可能被写回
  磁盘，掩码字符串永不落盘）；`write_atomically` 在 Unix 上对配置文件与临时文件
  强制 0600；错误消息/Debug 不回显。
- **轮换**：加载线程的 resume 现比较 provider 定义（含 api_key）——同名换 key 触发
  重建（空闲、无其它订阅者时），下一轮用新 key；执行中轮次保持旧快照（既有边界）。
- **TUI**：向导凭据步骤三选一（直接键/环境变量/无鉴权，默认直接键）；隐藏输入
  （`render_ref_masked('*')`）粘贴 key；key 只存在于向导意图直至确认写入；预览显示
  圆点状态行；保存后刷新内存配置、无需 export/重启。
- **远程 proto 不携带 key**（thread_config 无该字段——各后端读自己的用户配置）。

**2026-11-05 第六轮审查修复**：①刷新"已生效"判据改为**双条件**——`widget_generation`
计数器只在真正替换 widget 的单一交换点自增（此前在 resume 包装函数入口自增，终端
清除失败时会给出假"已生效"），且必须 `result.is_ok()`；两个保存路径（执行中排队
与应用前空闲态）都据此判定，失败保留抑制与待刷新意图并明确提示，不再静默。
②类型错误值脱敏**收口到两处 Display**（`format_config_error` 与
`ConfigLoadError::Display`，后者经 `io::Error::to_string()` 进入 app-server 的
"invalid configuration" 告警），匹配规则改为"值位置内、受引号包裹的第一个片段"：
兼容 serde 对字符串的**双引号**渲染与 `unknown variant` 形态，且不再误伤
`expected one of \`a\`, \`b\`` 这类期望值；`diagnostics` 10 例单测 + 一处
`replace_chat_widget` 计数器语义测试锁定。③顺带恢复 clippy 闸门：本批范围内
`config`（`remote.rs` 的 `then_some`、`ConfigLoadError` Display 路径）、
`model-provider-info`（测试里的冗余 clone）、`app-server`（provider 写入的两处
`redundant_closure`/`needless_borrow`）与 `tui`（`expect_used`、`needless_late_init`、
`print_stderr`）的 deny 级 lint 全部清零；并移除模型切换测试里遗留的 `eprintln!`
计时输出（`codex-tui` 现 `cargo clippy --all-targets` 通过）。④补齐**第三处脱敏缺口**
（2026-11-05 复查实测发现）：exec/TUI 启动走的是"合并后 TOML 反序列化"路径，
`deserialize_config_toml_with_base` 直接把裸 `toml::de::Error` 塞进 `io::Error`，
`downcast_ref::<ConfigLoadError>()` 失败后走 `eprintln!("...{err}")` 兜底，实测把
`api_key = 987654` 原样打印。现由 `codex_config::RedactedConfigError` 包装该路径
（Display 即脱敏文本），exec/TUI 兜底分支再叠加 `redact_config_error_text`，
日志 `Failed to deserialize overridden config` 也随之脱敏；`diagnostics` 补 1 例，
共 11 例。

## 8. 版本与发布

正式版本 `v0.18.14` → `f82730975` 已发布（2026-10-23）：
[Release39792664843](https://github.com/nuwax-ai/codex/actions/runs/39792664843) attempt1、10/10 jobs success，
GitHub七个平台归档公开、OSS7/7、npm `nuwax-codex@0.18.14` / latest 已实际查询与隔离安装。
Mac ARM默认/显式home、Linux/Windows显式CODEX_HOME的实际初始化与metadata通过，正式Mac host stdio1/1、
正式制品ACP四组28/28；Intel CI制品经Apple Silicon上的Rosetta实际运行验证，不冒称物理Intel宿主。
`v0.18.13` 的前一 Release 已主动取消、从未发布 npm，原 tag 保留不改写。
macOS 使用当前标准 runner，musl 编译并行限制为 2；Rust/V8 继续从仓库固定版本与
可信配对清单读取。完整制品、首败与执行边界见
[发布记录](my-docs/rig-production-completion-2026-10-10/nuwax-harness-release-0.18.14.md)。
Release成功不改写旧完整Core CI三条nextest红，平台限制继续按§10维护，主线已收口。

**发布执行者：Gitea 优先（2026-11-13 起）**。GitHub 那条链的 `aarch64-apple-darwin`
在冷缓存 runner 上撞到单 job 6 小时上限被取消（run 43257256775），Release 与 npm
发布被跳过；同一天把 Gitea 侧补齐成可以独立发 npm：

- 构建矩阵仍是 6 个目标（2 darwin / 3 Linux / x86_64 Windows），每个 build job 在打完
  包后**同时**上传 S3（`ci-gitea/<version>/<target>/`，给内部下载用）与**阿里云 OSS**
  （`nuwax-codex/v<version>/nuwax-codex-<version>-<target>.<ext>`，npm postinstall 的
  唯一来源，布局见 `npm/bin/runtime.js`）。
- 新增 `publish-npm` job（needs `build` + `verify-artifacts`）：先逐个 HEAD 校验 OSS 上
  6 个平台归档都是 200，再 `npm/publish/update-base-package.sh <version>` + `npm publish
  --access public --registry=https://registry.npmjs.org`。顺序不可反：npm 同版本不可重发，
  先发布会留下装不上的版本。
- 需要的仓库 secrets（已写入 Gitea）：`OSS_ACCESS_KEY_ID`、`OSS_ACCESS_KEY_SECRET`、
  `NPM_TOKEN`（新版 90 天有效，过期需换）；S3 那套 `NUWAX_UPLOAD_*` 仍用于内部归档。
- **已知缺口**：`aarch64-pc-windows-msvc` 在 Gitea 侧关闭（`windows-host` 没装 VS
  ARM64 工具链），GitHub 才有。publish 校验 6/6 即可发布，但 Windows-on-ARM 用户会
  404；补齐工具链后把该矩阵项打开即回到 7/7。
- 并发度按宿主内存派生，但**必须按 OS 读内存**：Linux 读 `/proc/meminfo`、Darwin 用
  `sysctl -n hw.memsize`（2026-11-13 之前只写了前者，macOS job 在 `set -e` 下直接
  `awk: can't open file /proc/meminfo` 失败）。

- workspace 版本恒为占位 `0.0.0`；npm 版本号来自 git tag（`v*` → release.yml，
  7 平台构建 → 阿里云 OSS 二进制 → npm publish；cargo bin 名保持官方 `codex`，
  npm 打包时主程序改名 `nuwax-harness`，postinstall 从 OSS 下载平台二进制到
  `~/.nuwax-harness-cache`）。旧命令 `nuwax-codex` 仅由 npm 层 JS 转发
  （`bin/nuwax-codex.js`，`package.json` bin 注册）提供兼容入口；
  **发布归档与平台分包不再附带 `nuwax-codex` 二进制别名**（2026-11-06 起——
  此前 zip 无硬链接语义，同一主二进制被完整复制两份，Windows 包白占 ~121 MB；
  tar.gz 侧原以 tar hardlink 免费携带，一并移除以统一语义）。下游按文件名解析
  二进制的脚本须改用 `nuwax-harness`（ACP `CODEX_PATH` 用
  `command -v nuwax-harness`）。
- 主程序 `--version`/`-V` 只输出版本号本身（如 `0.18.14`），不带官方的
  `codex-cli` 前缀；tarball 安装脚本 `install.sh` 的版本探测兼容新旧两种输出。
  实现方式是 `disable_version_flag` + 手动 `version` 字段（打印
  `CARGO_PKG_VERSION`），而**不是** clap 的自动版本标志——后者渲染
  `<crate name> <version>`，即 `codex-cli 0.0.0`。该实现曾在 2026-11-03 的
  529 提交合并中丢失（合并只留下 `version,`），2026-11-05 已按 `v0.18.19` 的
  原样恢复并加解析回归；源码构建因此显示裸 `0.0.0`，npm 制品显示裸 `0.18.x`。
- live-tests crate（`LIVE_VENDORS` + `.env.local` 凭据）按 厂商×桥×场景 矩阵跑
  真实请求；compact 场景已入 exec 矩阵（三线）。
- **用户可见文案改名须同批次协同客户端**（2026-11-09 起，§5.4）：错误 prose
  可以改，但机器判据只能是 `codex_error_info`；任何按字符串前缀匹配的客户端
  （如 iOS 输入上限分类器）必须同步改为读该字段，否则改名即漏判。

## 9. 差异未及之处

不能再把沙箱、审批、MCP、TUI 等整体标为与官方完全一致：后续已有后台模型选择、
退出清理、图片入口以及Linux沙箱deny等修复。self/thread-self/net家族deny现已
明确Fatal（不再静默失效），并不承诺可运行地拒绝任意per-pid视角。显式/dev分支
的初始缺席不等于不可重建，晚间复审的隔离修正与限定能力见最新复审记录。
当前补丁与未闭合安全边界
见 [ROADMAP.md](my-docs/ROADMAP.md) 和最新台账；补丁索引仍见 `my-docs/codex-review-prompt.md` §二。设计文档见
`my-docs/nuwax-home/`、`my-docs/anthropic-hosted-tools/`、
`my-docs/rig-responses-phase1|2/`。

## 10. 已知限制与历史修复

| 局限 | 坐标/依据 | 状态 |
|---|---|---|
| 显式 `:danger-full-access` 命令曾被同 profile ID 的 Ask preset 抢先匹配，跳过确认 | `tui/src/chatwidget/permissions_menu.rs`；`15283cc98` | 已修：profile-id 入口选 canonical full-access，显式 on-request 与两 reviewer 仍先确认；Ask 菜单既有语义保留，权限模块 49/49 |
| Windows legacy 沙箱 workspace-write **不约束既有文件删除**（内核 WRITE_RESTRICTED 不覆盖 DELETE 权位，diag1-7 证据链见台账 §6.57；显式 deny-write 路径有 DENY ACE、删除受保护）。运行时降级有 WARNING 日志；完整删除约束需 elevated 后端 | `windows-sandbox-rs/src/unified_exec/backends/legacy.rs`、台账 §6.57 | 用户决策 B+警告（2026-10-22），上游继承问题不再扩fork补丁面 |
| Chat/Anthropic 输出预算可通过 provider.max_output_tokens 配置；Anthropic 未配置时仍默认 16384，Responses typed HTTP/WS、Rig passthrough 与预算已采用该字段 | `codex-rust-rig-bridge/src/stream.rs`、`client.rs` | 三协议已接线；验收见 `provider-request-controls-2026-10-03/tasks.md` |
| 无 Anthropic prompt caching（`cache_control`） | 已实施（2026-10-26）：provider 级 `anthropic_prompt_caching` opt-in；rig 0.45 手动断点（末工具/system 尾/末消息，ephemeral 5m）+ 桥的 agent 断点（多轮时标记上一轮结束处，`CachePoint::Messages(倒数第二)`）；默认关、字节不变 | **GLM 实网已验证**：两轮会话 turn2 命中 11,008 cached tokens（修复 agent 断点前为 0）；其他网关按矩阵待验 |
| 普通 TLS 客户端复用且请求头按轮注入；自定义 CA 仍专用构建 | `codex-rust-rig-bridge/src/client.rs`、`transport.rs` | 取消与连续工作流已有定向/实网证据；本次 main 合并后重新验收受影响部分 |
| 搜索来源身份、恢复上限、混合轮配对已入库；厂商能力仍分别受限 | `hosted_replay.rs`、`hosted_replay_budget.rs`、`model_output_projection.rs` | 离线 wire 与真实厂商证据分开，不能把 GLM 搜索支持扩大至 MiMo/Step |
| 病态网关"HTTP 200 + 非 SSE JSON 错误体"两线均 EOF 丢体 | `sse.rs` EOF 路径；三家目标厂商 live 未见此行为 | 已知边界（可选加固：EOF 时附 body 摘要） |
| **Anthropic 线 `effort=none` 时发送 `thinking:{"type":"disabled"}`**：Anthropic 现行契约在 Opus 5.5 / Sonnet 5.5 上对该值**一律 400**（Opus 5 仅 effort≤high 接受），GLM 网关有同类规则（`1210 该模型始终思考，不支持关闭思考；请使用 low、high 或 max`） | 原坐标 `transport.rs:254` / `stream.rs:397` / `convert_request.rs:295` 均已随下一条修复移除；现由 `reasoning_effort.rs` 走 rig 类型化通道，`can_disable=false` 时关闭思考在本地即被拒 | **用户决策：不在引擎里静默改写**（2026-10-29）。正解是按文档**省略 `thinking` 并把 effort 降到 `low`**，但这类跨网关差异**应由环境变量显式设置**（计划 `NUWAX_ANTHROPIC_THINKING_MODE`，取值 `omit`/`disabled`，未实现），而不是桥在背后黑盒兼容——静默改写会让"用户配了什么"与"线上发了什么"不一致，问题只能从抓包反推 |
| **GLM 的 effort 档位，与"200 不等于被采纳"**：官方文档 (concept-param#reasoning_effort) 写明 GLM-5.3 / GLM-5.3-FLASH / GLM-5.3-FLASHX **仅支持 max / high / low**;GLM-5.2 支持七档，且 low 与 medium 映射为 high、xhigh 映射为 max、none 与 minimal 放弃思考。我们桥把 Codex medium 映射为 Anthropic medium(convert_request.rs:295)，对 GLM-5.3-Flash **不是真实档位** | convert_request.rs:295;NUWAX-HARNESS.md 档位表 | 实测(2026-10-29，官方 Anthropic 端点):output_config.effort 取 low/medium/high/xhigh/max/minimal/ultra/none **全部 200**，含任何 GLM 档位表里都没有的 minimal 与 ultra——**该端点不校验 effort 取值**，所以 200 只说明请求被接受、**不说明档位被采纳**。1210 因此无法用我们的请求形态在该端点复现(疑为代理上游不同)，但"发了 medium 却拿到 high 或默认档"这种**静默语义偏差是真实存在的**:请用 NUWAX_MODEL_REASONING_EFFORT 选模型真实存在的档位。**已修(2026-10-29)**:effort 改走 rig 的类型化通道(见下条)，不再裸注入 body |
| **effort 曾绕过 rig 的校验通道，裸注入 body**：rig 对 `reasoning` 有完整的按模型校验（`ReasoningSupport::Listed { levels, can_disable }`，默认策略`OnUnsupported::Error`，在编码请求时就拒绝），但桥为了控制线值把 `output_config.effort` 与 `thinking:{"type":"disabled"}` 直接插进序列化后的 body（`transport.rs`），类型化通道一个字段都没设——等于把 rig 这门关整个绕掉。于是"模型不接受的档位"与"不能关闭思考的模型被关思考"都不会在本地失败，只能靠网关自觉；而 GLM 的 Anthropic 端点不校验 effort（见上一条），medium 被静默按 low 执行，排查只能靠抓包 | `codex-rust-rig-bridge/src/reasoning_effort.rs`（新增）、`convert_request.rs`（改设 `completion.options.reasoning`）、`client.rs::build_anthropic_model`（按 provider 声明的档位绑 `ModelSpec`）、`transport.rs`（删除两处裸注入）、`utils/cli/src/nuwax_env.rs`（`NUWAX_REASONING_EFFORT_LEVELS`） | **已修（2026-10-29）**：effort 走 rig 的类型化通道，provider 可用 `NUWAX_REASONING_EFFORT_LEVELS`（或 `model_providers.<id>.reasoning_effort_levels`）声明模型真实档位；档位词表直接用 rig 的 `Effort` 反序列化器读，不自己维护副本（rig 自己读厂商目录也是这么做的，`rig-core/src/catalog/row.rs`）。声明后，选了模型没有的档位、或对始终思考的模型关思考，都在**请求发出前**失败（测试断言本地服务器一个请求都没收到）。不声明则保持修复前的宽松行为。注意绑 `ModelSpec` 会让 rig 把它当能力权威：其默认 `input.image=false` 会丢图片，但 Anthropic 线的图片命名规则为 `None`（`is_none_or` 短路），spec 的 modalities 不被读取——已用测试钉住（带图请求在绑定后仍完整到达）|
| **Chat 线的 effort 仍是裸注入，档位校验由桥自己做**：`wire_api = "chat"` 的 `reasoning_effort` 仍作为 raw body key 发出，没有走 rig 的类型化通道  | `codex-rust-rig-bridge/src/convert_request.rs`（Chat 分支的 `params.insert("reasoning_effort", …)`）、`client.rs::build_chat_model`（facts 保持不绑定）、`reasoning_effort.rs::check_declared`（桥自己的档位门）、`stream.rs`（解析一次，两条线共用）  | **已修（2026-10-30）**，但** deliberately 不用 rig 的类型化通道**：rig 的 `Rewrite::OutputCapRename`（`completion/options/merge.rs`）在 `reasons(facts, model) == Some(true)` 时把 `max_tokens` 改名为 OpenAI 专有的 `max_completion_tokens`，而同一个 `reasons()` 正是类型化通道打开的——rig 把"要不要校验 effort"和"输出上限拼哪个字段"耦合在了一起。兼容网关（GLM/MiMo 等）不认 `max_completion_tokens`，配好的输出上限会**静默失效**；而未绑定 facts 时 `reasons()` 回落到按模型名判断，对网关模型返回 `None` 从而不改名——rig 正是为此把该重写限定到模型（"this same wire reaches compatible servers"）。只要给 Chat 绑 spec，改名必然触发。**所以档位门由桥自己下**：规则本来就是我们的（档位表是我们的配置），词表仍然是 rig 的（过 `Effort` 反序列化器读），拒绝点仍然在请求构建之前。已用测试断言：声明档位后 Chat 选 `medium` 会被拒且本地服务器一个请求都没收到，而合法档位的 body 与未声明时逐字节一致 |
| **上下文窗口溢出曾直接硬报错，不自动压缩（长会话废掉）**：采样阶段收到 `ContextWindowExceeded`——三方网关的拒绝体（GLM `1261 prompt is too long`，fork 在 `api_bridge.rs` 映射）或桥层字节预算闸门（`wire_budget.rs`）——turn 循环里本有"压缩一次→重试一次"的恢复臂，但其守卫要求 guardian 本地检查才插的 `ExhaustedReviewBudget` 标记，两条三方常态来源都不带 → 落入通用错误臂硬报错。事前同样有缺口：轮前压缩判断只看**上一轮已报用量 vs 阈值**，看不到本轮待加入输入，单轮大输入（实测每轮 16.3 万 token 对 10 万余量）一步跨过阈值与网关真实上限（实测 GLM-5.3-Flash：81.5 万 token 通过、约 98 万被拒，真实窗口 ~1M）。fork 没有自家模型，网关拒绝是常态错误源，upstream 的"自家标记才恢复"是其官方模型前提的残留 | `core/src/session/turn.rs`（新增恢复臂：无 marker 的采样阶段溢出同样压缩一次重试一次，复用既有每步一次的有界标志；`run_pre_sampling_compact` 接入 pending 估算）、`core/src/session/context_window.rs`（`pending_input_reaches_limit`：待加入输入**只对齐完整窗口上限**，不叠加操作者软阈值——软阈值跨过只会把压缩延迟到下一轮边界检查，真实窗口才是会致死的线；未显式设软阈值时派生阈值=完整窗口，两道同时守住）、`core/src/context_manager/history.rs`（`estimate_user_input_token_count`，guardian 闸门同款粗估，图片按 resized 地板） | **已修（2026-11-02，两面）**：①事后——网关拒绝/桥层闸门/guardian 本地三类来源一律触发"压缩一次、重试一次"（有界：每步一次，压缩失败即原样报错；压缩被禁用时维持硬报错）；②事前——轮前判定把本轮输入的逐项估算计入完整窗口，防单轮跳步（窗口未配置时该机制不生效，由①兜底）。压缩请求自身被拒由 upstream 的 trim 循环（`compact.rs` 溢出→丢最老一条→重试）自愈，已有 HTTP 体场景测试钉住。测试：3 个集成场景（拒一次→压缩→重试成功；持续拒绝→有界报错；pending 触发预轮压缩先于采样）+ 判定/估算单测；发版前另以多轮堆叠脚本对 GLM 真网验收 |
| **自动压缩派生阈值 90%→80%,显式值不再被 90% 封顶**：upstream 的派生默认(90%×窗口)同时充当一切显式值的封顶(`min(显式, 90%×窗口)`)——三方网关场景两处不合适:①网关为输出留预留(实测 GLM 1M 窗口在 ~95-98 万拒),90% 触发线距拒绝线只剩 5 万余量,一轮大输出即穿;②压缩请求自身带全量历史,90% 触发时压缩请求贴近网关上限,实测被 1261 拒、靠 trim 丢史自愈。另外用户设 950k 会被静默钳到 900k(违反 fail-fast) | `protocol/src/openai_models.rs::auto_compact_token_limit`(派生 8/10;显式值权威不封顶)、`models-manager/src/model_info.rs`(显式值越过 full-window 线时告警不钳制) | **已修(2026-11-02,方案 B)**:未配置的模型(fallback/无显式值目录模型)按 80%×窗口触发;显式值(目录或用户)直接生效——官方目录模型的显式值均 ≤90%,**行为零变化**;>90% 的显式配置从静默钳制变为生效+必要时告警。显式值 ≥ 窗口×有效百分比(full-window 线)时 warn 一次提示"常规压缩不会先于兜底触发"。测试:单测四态(80% 派生/≤90% 不变/>90% 解封/窗口未知)+ 更新一个依赖旧封顶的断言;compact 全套回归 |
| **厂商模型元数据目录(models.dev 快照 + overlay + models.d)**:三方模型此前走 fallback 占位(272k 窗口/95%/Text 工具面),十余处行为与目录模型不一致(轮内转向拒、审阅预算钳 128k、图片不入审阅等)。Spec 见 `my-docs/vendor-model-catalog-plan-2026-11-02.md`(2026-11-02 四项决策:逐字段深合并/仅实证档位/维持隐藏/200KB) | `models-manager/vendor/catalog.json`(白名单 15 厂商快照,`scripts/refresh_vendor_catalog.py` 幂等生成+锚点校验,运行时零网络)、`vendor_catalog.rs`(解析/小写索引/同 slug 消解:别名>套餐>最大窗口+告警)、`model_info.rs::vendor_model_info`(命中=目录待遇,`used_fallback=false`)、`MetadataSource` 枚举与窗口四态(vendor/fallback 显式全额+向上告警;官方维持 upstream 钳制)、`core/config::load_user_models_overlay`(`$NUWAX_HOME/models.d/*.toml` 逐字段深合并、fail-fast 点名)、桥内档位目录默认(未声明 `reasoning_effort_levels` 时回落目录表;逃生口 `model_providers.<id>.ignore_catalog_effort_levels`,默认 false 不序列化)、doctor 窗口行标来源 | **已实施(2026-11-02,T1-T5;e2e 实证)**:零配置 GLM-5.3-Flash(用户拼写大小写同命中)→ 目录 1M 窗口与 80% 阈值;`medium` 发送前被目录档位门拒(原需手配 NUWAX_REASONING_EFFORT_LEVELS);逃生口回退宽松;models.d 坏文件 fail-fast;快照 165KB。已知边界:NUWAX_* 环境组的逃生口暂无 env 种子(用 config.toml provider 或 provider 档位显式配置),models.d 的窗口/百分比经 models-manager 生效;**models.d 档位表当前不进请求期档位门**(桥只读 committed overlay,等价做法是用 provider 级 `reasoning_effort_levels` 显式配置)——已记 T7 跟进。T6 剩:全量 §9 组合验收与文档 |
| **品牌残留清扫与旁路功能禁用(2026-11-03 审计)**:另一 agent 全量审计(`my-docs/brand-residue-audit-2026-11-03.md`)后三批清理——①P0 模型可见(ToolSearch 描述/transcript 任务前缀/目标文件前缀/网络代理拒绝);②P1 用户可见 ~100 文件(exec 横幅/TUI 命令与提示/CLI 帮助/doctor/login/core/protocol/config;含 `exit_summary` 重连命令 `codex`→`nuwax-harness` 的功能性修复);③P2 日志与主目录可见文件名。**决策项落地**:#4 pets 禁用(命令提示禁用 + CDN 拉取 bail);#5 官方公告 tip 拉取禁用(prewarm 直接置 None,不再访问 raw.githubusercontent.com/openai/codex);#6 TUI 自更新提示禁用(原本引导装 @openai/codex);ChatGPT Plus 升级 URL 从用户错误中移除。**MCP 名称替换**(无历史兼容负担,用户确认):`codex_apps`→`nuwax_apps`(常量 `CODEX_APPS_MCP_SERVER_NAME`→`NUWAX_APPS_MCP_SERVER_NAME`,`mcp__codex_apps__*`→`mcp__nuwax_apps__*`,128 文件)、`codex_tui`→`nuwax_tui`(MCP namespace)、`list_mcp_resources`/`list_mcp_resource_templates` 默认 server `codex`→`nuwax-harness`、rate-limit `limit_id` 同步。红线全保:模型名/URL(拉取禁用处的常量除外)/`CODEX_*` 环境变量/`codex.*` 遥测族/二进制名/`<codex_internal_context>` 标签族/`:codex-followup` 等输出协议(专项待定) | `tui/src/tooltips.rs`(公告禁用)、`tui/src/chatwidget/slash_dispatch.rs`(pets 禁用)、`tui/src/pets/asset_pack.rs`(CDN bail)、`tui/src/update_prompt.rs`(更新提示禁用)、`codex-mcp/src/mcp/mod.rs`(服务器名常量)、`core/src/tools/handlers/mcp_resource/*`(默认 server 名)、以及品牌批量清单(见审计文档 §3) | **已实施(2026-11-03)**;复扫 tui/cli/exec/core 归零(审计脚本 `scripts/brand_audit.py`);workspace --all-targets 0 错误;测试断言同步(device_code 警告、usage-limit、strict_config);`codex.*` 遥测事件名/tracing 字段在批量误伤后逐一还原。**仍未动**(专项):`<codex_internal_context>` 标签、`:codex-followup`/`codex-file-citation` 输出协议、`CODEX_SESSION_ID` shell 变量、originator/UA(OpenAI 认证线) |
| **origin/main 大合并(529 提交,2026-11-03)后 fork 集成点重接线**:合并取上游结构性重构(`ModelOrigin`/`CatalogModel` 取代 `used_fallback_model_metadata`/`MetadataSource` 字段、Prompt 按值、Cow 输出截断等),fork 逻辑随之迁移——`MetadataSource` 枚举删除,fork 的 vendor 来源并入 `ModelOrigin::Vendor { provider }` 常驻 provenance(该枚举因此去掉 `Copy`);`with_config_overrides` 恢复窗口四态/ratio 派生/绝对值告警/tool_search 三态(签名加 origin 参数);`construct_model_info` 恢复 vendor 目录插入(官方目录之下、fallback 之上);`core/config` 恢复 `models.d` 加载、`model_auto_compact_ratio`/`tool_search_enabled`/`user_overlay` 三字段与 `to_models_manager_config` 填充;`turn.rs` 恢复溢出双向修复(`estimate_pending_input_tokens` 轮前臂 + `fork_overflow_recoverable` 采样后臂,与 upstream 新增的 guardian 预算臂互补);`is_first_party`/`provider_id` 盖章恢复(改在 `merge_configured_model_providers` 终点盖,避开 GCP override 的 `default()` 比较陷阱),`deserialize_model_providers` 恢复 `ignore_catalog_effort_levels` 逃生舱提取;`local_model_messages(slug)` 身份渲染三件套恢复;品牌批量替换以三方补丁重放找回被合并覆盖的 ~130 处(命令引用/自称/`codex_apps`→`nuwax_apps` 全树改名/`bin_name`),ChatGPT Plus 升级 URL 二次移除;telemetry 零旁路(statsig→none、DEFAULT_ANALYTICS_ENABLED=false)与 pets/公告/自更新禁用经核验在合并中存活 | `protocol/src/openai_models.rs`、`models-manager/src/model_info.rs`+`manager.rs`+`vendor_catalog.rs`、`core/src/session/turn.rs`+`config/mod.rs`、`model-provider-info/src/lib.rs`+`merge_configured_model_providers`、`config/src/config_toml.rs`、`app-server/src/config_manager.rs`、品牌重放清单(89 文件) | **已重接线(2026-11-03)**;另:合并曾把 `rust-toolchain.toml` 解析为上游的 1.95.0,**已回钉 fork 的 1.99.1**(rig-core 0.45 的 MSRV 是 1.97.1,上游不用 rig 可以钉 1.95.0,fork 不行——此文件冲突永远取 fork 侧)。编译 `--workspace --all-targets` 0 错误;回归:workspace 全量 24772/24810 过,余 38 失败逐一核验——5 个截断测试按 fork 图片管线适配后 31/31 绿;tui 2 个 ABRT 加大栈即过(已知 Mac 栈环境);`resume_lists_all_providers`(fork provider 过滤)经 pre-merge 对照树复跑**同样失败**,属本机环境预存在(PTY+daemon);voice-host 3 个真实 UDP/TCP 协商测试本机网络环境(代理)超时,单跑亦挂;v8-poc 2 个中 1 个 pre-merge 同红、1 个为上游钉 git fork rusty_v8 与 fork 钉 crates.io 152.2.0 的绑定差异。追加修复:上游版 `BearerAuthProvider` 丢了 `immutable_credential_headers` 快照 → 投影作用域从 credentialInstance 退化到 config-selector → 压缩检查点跨轮被丢(compact_remote 图片计费/skill_catalog/code_mode 等 5 测试红),已按 fork 版恢复并与上游 fedramp 检测合并;原生 Responses HTTP 路径补回 `output_provenance` 写入;strict_config 增加 model_providers 未知键走查器(serde_ignored 看不穿 raw-table 拦截)。注意:`-p codex-core` 单包跑测试不带 `rust-rig` feature,涉桥测试需 workspace 跑(特性统一)。上游新功能面(529 提交)的**旁路**审计已于 2026-11-09 完成(见下行),品牌审计仍待做 |
| **529 合并新旁路面审计(2026-11-09,内网政企部署验收)**:验收标准"除用户配置的模型网关外零外联",逐面核验:①模型目录发现:`models_endpoint.rs` 仅当 `base_url.is_none()` 且无 `model_catalog_url` 时才改指 `chatgpt.com/backend-api/codex`;vendor 恒有网关 base_url → `/models` 发现打自家网关;vendor 目录 `models-manager/vendor/catalog.json` 为 `include_str!` 内嵌,运行时零网络。②AnalyticsEventsClient(`analytics/src/client.rs`,POST `{chatgpt_backend}/codex/analytics-events/events`)三层门:`[analytics] enabled=false` → `AnalyticsEventsClient::disabled`;API-key 认证仅保留带 plugin_id 的插件族事件(纯 vendor CLI 为空集);非 codex-backend 认证直接 return——**2026-11-09 起改为 opt-in**:`core/src/config/mod.rs` 未配置时解析为 `Some(false)`(原为 None→客户端驻留靠 auth 门兜底),session.rs 即构造 `AnalyticsEventsClient::disabled`,TUI `terminal_size_telemetry`(上游未配置时默认开启)一并归 off;显式 `enabled=true` 仍可开启。③statsig 内置导出器:`otel/src/config.rs::resolve_exporter` 无条件 Statsig→None(端点与 key 不携带);上游 TUI 侧 `default_analytics_enabled=true`(`startup_orchestration.rs:729`,upstream 提交 a0bed4be21)被此兜住,exec 侧本为 false。④guardian:审查模型经 `reachable_review_model_id` 非 first-party 回落活跃模型→自家网关(与会话同通道);guardian-v2 异步打分 websocket 有 `uses_codex_backend` 门。⑤agent-identity/runtime-agent-instance(auth.openai.com JWKS+注册):`runtime_agent_instances` feature 默认 false(UnderDevelopment)。⑥remote_control websocket:仅 app-server 显式启动模式,CLI/TUI/ACP 默认不启。⑦realtime/voice 走 experimental 配置覆盖或 OpenAI 实时端点,仅用户主动触发,vendor 部署不配置即不可达;pets/公告(raw.githubusercontent.com)/自更新提示已于 2026-11-03 禁用 | `otel/src/config.rs`、`analytics/src/client.rs(send_track_events)`、`models-manager/src/manager.rs(should_refresh_models/supports_api_key_discovery)`、`model-provider/src/models_endpoint.rs`、`core/src/otel_init.rs`、`features/src/lib.rs` | **旁路审计完成(2026-11-09,本行)**:未发现 vendor 默认路径到 OpenAI 的旁路;同日 analytics 改为配置层 opt-in 默认关(`core/src/config/mod.rs`,测试 `load_config_analytics_is_opt_in_default_off`;按 upstream 旧默认断言的 9 个测试——mcp_auth_elicitation×6、plugins×2、tui terminal_size×1——fixture 已显式 opt-in/更新期望,analytics 过滤集 188 测试串行全过);品牌面审计仍未做;内网验收纵深兜底:出口防火墙仅放行网关域名 |
| **529 合并带入的 turn 准入检查曾拒绝一切会话级路由的网关会话**:`check_thread_model_provider_for_input`(app-server)每轮 turn 把线程 provider 与 `load_latest_config`(纯磁盘+进程 CLI)比较,会话级选择(thread/start 的 `modelProvider`+provider 定义、resume 覆盖、§7.3 re-home)不在其中 → ACP 网关会话(custom-gateway vs 磁盘 openai)首条 prompt 即 `ModelProviderMismatch`,适配器 0.18.28 集成测试全灭(0.18.26 无此检查) | `app-server/src/config_manager.rs::check_thread_model_provider_for_input`:改用 `load_latest_config_with_session_layers(current.config_layer_stack, cwd)`——会话层(SessionFlags/EnvSeed)随线程重放,会话级路由即线程的权威路由;磁盘继承(无会话选择)的线程仍做磁盘漂移检查;managed-requirements 校验原样保留;profile(用户层)选择仍按进程可见性拒 | **已修(2026-11-04)**;测试:`input_provider_check` 拆三态(explicit 会话覆盖随线程过/profile 仍拒)+ 新增 ACP 形态用例(请求覆盖带 provider 定义、磁盘 openai → 准入过;磁盘继承后磁盘改选 → 拒);enforcement 套件 mismatch 两测过。附带发现(既有未修):suite `builtin_bridge_requirements…lmstudio` 被保留 ID 校验拒;`malformed_system_defaults…` 超时(HEAD 同样) |
| raw 工具定义转换失败或整数超出精确可表示范围 | `request_tools.rs::request_tools` | 已改为 `InvalidRequest`，不再 panic/清空工具表/近似化整数；不是仍待修的 nit |
| TUI 标题与 fallback 指令身份显示 "OpenAI Codex"、指令声明"OpenAI 主导"（厂商模型自述会沿用该身份幻觉成 GPT） | `tui/src/history_cell/session.rs`、`models-manager/prompt.md`、`protocol/src/prompts/base_instructions/default.md` | 已修（2026-10-28，两轮清查）：fallback 模板身份行渲染实际 slug（"You are the {slug} model ... in nuwax-harness"，`{{model}}` 占位符 + `local_model_messages(slug)`）；protocol 静态默认同步中性化；快照归一化改 `is_fallback_model_instructions`（拒绝花括号，防未替换占位符自匹配）；目录内 gpt-\* 保持上游指令。第二轮遗漏清查补齐：TUI 品牌/输入占位符（"Ask nuwax-harness to do anything"）、**exec 回复标签**、CLI `bin_name`/usage/补全名与全部子命令帮助文案（含 `mcp add` 帮助里 `~/.codex`→`~/.nuwax-harness` 路径纠错）、**模型可见字符串**（`USER_MESSAGE_BEGIN` 权威常量"## My request for nuwax-harness:"、task_mentions 标题/引用块、dynamic_tools 工具描述、memories 整合提示词自指）、错误/警告消息中的自指与可操作命令引用（`codex doctor/unarchive/logout/fork/app`→`nuwax-harness ...`）。有意保留：目录 gpt-\* 指令、Codex Cloud/Desktop/Apps 产品专名、realtime/guardian 上游专用提示词（vendor 不走或涉安全语义，登记待评估）、TUI daemon 版本通知（fork 内嵌模式不可达）、memories 中 `api.openai.org` 脱敏示例（数据必须保留）、约 200 处测试夹具内占位符字符串（不用户可见，改动徒增快照翻新） |

阶段 D（搜索结果持久化/pause_turn/混合轮）已立项：Spec 见
`my-docs/phase-d-spec.md`，Plan 见 `my-docs/phase-d-plan.md`（2026-10-01）。
