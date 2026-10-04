# FORK.md — nuwax-codex 与官方 codex 的逻辑差异

维护说明：本文是 fork 行为差异的权威索引。改动 fork 行为时同步更新本文与
`codex-review-prompt.md`（§二 补丁面清单）。上游基线：`29f056c26`（2026-09-24）。
npm 包：`nuwax-codex`（二进制名同）；ACP 侧配套包：`@nuwax-ai/nuwax-codex-acp-ts`。

## 1. fork 的使命

让 codex 支持中国厂商（GLM / MiMo / StepFun / DeepSeek / Ollama 等）：他们的
API 是 OpenAI Chat / OpenAI Responses / Anthropic Messages 三种线协议的兼容网关。
fork 通过"桥"（bridge）层做协议转换，官方 codex 只支持第一方 OpenAI。

## 2. 配置目录：分层 CODEX_HOME（v0.18.7+）

| 优先级 | 来源 | 行为 |
|---|---|---|
| 1 | `CODEX_HOME` 环境变量 | 与官方一致（必须存在、canonicalize） |
| 2 | `~/.codex-nuwax` 存在时 | **fork 独有**：config/auth/sessions/log 全隔离 |
| 3 | `~/.codex` | 官方默认（兼容回落） |

官方 codex 完全不感知 `.codex-nuwax`，两边可并存互不干扰。fork 专属配置字段
（`experimental_bearer_token`、`wire_api="anthropic"` 等）只应写进隔离目录——
写进共享 `~/.codex` 会被官方严格校验拒绝（2026-09-28 事故）。设计：
`my-docs/nuwax-home/`。

## 3. 模型协议与路由

### 3.1 wire_api 取值（model_providers 表内字段）

| 值 | 官方 | fork |
|---|---|---|
| `responses` | ✅ | ✅（第三方厂商默认走桥，见 3.3） |
| `chat` | ❌（上游已删） | ✅ fork 重新加回 |
| `anthropic` | ❌ 不存在 | ✅ fork 新增（显式声明，解决 Step 等无 URL 标记网关） |

### 3.2 双桥 + 逃生舱

`experimental_bridge`（fork 字段）：`"rig"`（默认，rig-core =0.42.0）/
`"genai"`（备用，rust-genai 0.6.5）/`"native"`（强制官方原生传输，调试用）。

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
reasoning envelope）→ 原样序列化 → rig `post_sse` + `send_streaming`（建流也受
idle timeout 包裹）→ 原始 SSE 进 codex-api 完整解码器，**严格终止策略**
（`codex-api/src/sse/responses_policy.rs`：顶层 error/failed/incomplete 即停、
畸形帧报错、[DONE] 容忍）。官方原生路径的宽松策略不变。Authorization 在 rig
自动合成 Bearer 后**精确恢复**（api-key-only/无鉴权网关不被改写）。

## 4. hosted（服务端）工具翻译（v0.18.8+）

codex 以 Responses 形态声明 hosted 工具（`{"type":"web_search"}`）；官方在
Chat 转换中直接丢弃。fork 的 `hosted_tools.rs` 翻译表：

- 请求侧：`web_search` → Anthropic `web_search_20250305` 服务端工具（GLM 网关
  实测接受）；`filters.allowed_domains`、`user_location` 等可表达约束随翻译保留
  （域名格式/地点字段前置校验），cached/indexed 模式在发请求前显式报错（不静默
  放宽为 live）；无对应物的工具仍丢弃并告警。新工具 = 加表项。
- hosted-only 请求的 tool_choice：rig 流式路径在 typed 工具为空时丢弃
  tool_choice，transport 注入服务端工具后恢复请求值（none/any/tool 原语义，
  禁并行标志随恢复生效；required/指定工具在无函数工具时前置报错；Chat 线无
  工具时移除悬挂 tool_choice）。
- 默认 fail-closed：`web_search_mode` 默认 cached，而 chat-family 桥线无法表达
  cached → 新增 `ProviderCapabilities.cached_web_search`，默认 cached 在此类线
  上降级为 Disabled（此前字段丢弃翻译会让 GLM 实际执行 live 搜索，与配置模式
  相反）。需要搜索请显式 `web_search = "live"`。
- 响应侧：rig 0.42 公开流式面不暴露 `server_tool_use`，transport 层 tee 原始
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

阶段 D 完整验收和后续任务见 `my-docs/codex-review-2026-10-01.md`；当前不能
宣称跨厂商搜索载荷兼容、完整暂停保真或混合 server/client 轮全部完成。

设计：`my-docs/anthropic-hosted-tools/`。
### 三协议字段契约（2026-10-01，R4）

fork 明确每个跨协议字段在三条线（Responses passthrough / Chat / Anthropic）
上的语义，避免"开关在一条线生效、另一条静默丢失"：

| 字段/能力 | Responses passthrough | Chat 桥 | Anthropic 桥 |
|---|---|---|---|
| `max_output_tokens` | **忽略**（逐字透传，不注入） | 映射 `max_tokens` | 映射 `max_tokens`；未配置默认 16384 |
| `hosted_results_replay` | 不适用（无桥回放） | 不适用 | false 时整体关闭回放（含旧格式） |
| reasoning effort/summary | 原样 | 映射 reasoning_effort（best-effort） | none→`thinking:disabled`；其余映射 effort |
| structured output | 原样 | `output_config.format` 原样 schema | 同 Chat |
| usage/缓存计数 | 原样 | usage 映射（无 cache 计数） | 含 cache 计数（首帧/增量语义） |
| 重试/限流 | codex-api 层 | rig 层 + provider request/stream_max_retries | 同 Chat |
| web_search 模式 | 原样（含 cached/indexed） | 丢弃声明（无 hosted 概念） | 仅 live；cached/indexed fail-fast |
| `parallel_tool_calls` | 原样 | 原样 | hosted-only 时还原 tool_choice 并关 parallel |

`cached_web_search` 能力当前按 `native_transport` 绑定（model-provider
`provider.rs`）：Rig Responses 线同 native 一样默认 Cached→Disabled 由
capability 决定，而 Chat/Anthropic 桥线在请求翻译处 fail-fast。按协议×厂商
的精细能力矩阵仍为登记项（R5 追踪），本轮未改默认搜索策略。



## 5. 上下文压缩（compact）

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
- 环境变量（优先级：显式 `-c` > env > config.toml；非法值带变量名报错）：
  `CODEX_MODEL_CONTEXT_WINDOW` / `CODEX_AUTO_COMPACT_TOKEN_LIMIT` /
  `CODEX_AUTO_COMPACT_RATIO`。同类先行：`CODEX_MODEL_REASONING_EFFORT`。

## 6. daemon：npm 安装不自启（v0.18.8）

官方 TUI 默认自启共享 daemon，需要"完整本地包布局"（codex-package.json、
code-mode-host、rg）。fork 的 npm 包只发单二进制 → 自启必失败
（"this CLI has no complete local package"）。fork 在
`tui/src/daemon_startup.rs::exclusion` 排除 NpmNuwax 安装方式（安装方式参数
注入，hermetic 可测），TUI 走内嵌 app-server；显式 `app-server daemon` 命令不受影响。

## 7. fork 专属字段与环境变量总表

**config.toml 键**（官方 schema 不认识，只写 `~/.codex-nuwax`）：
`model_providers.*.wire_api = "chat"|"anthropic"`、`experimental_bridge`、
`experimental_bearer_token`、`model_auto_compact_ratio`。

**环境变量**：`CODEX_INSTALL_SOURCE=npm_nuwax`（launcher 设置，install-context
识别 NpmNuwax）、`CODEX_MODEL_REASONING_EFFORT`、`CODEX_MODEL_CONTEXT_WINDOW`、
`CODEX_AUTO_COMPACT_TOKEN_LIMIT`、`CODEX_AUTO_COMPACT_RATIO`。

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
多进程应分别设置独立 `CODEX_HOME`；参数表、启动示例、远程与会话
边界见 [容器多进程审查](container-multiprocess-env-review-2026-10-03.md)。
（ACP-TS 侧另有 `CODEX_BASE_URL`/`CODEX_API_PROTOCOL`/`CODEX_WIRE_API`/
`INITIAL_AGENT_MODE` 等，见该仓库。）

## 8. 版本与发布

- workspace 版本恒为占位 `0.0.0`；npm 版本号来自 git tag（`v*` → release.yml，
  7 平台构建 → 阿里云 OSS 二进制 → npm publish；cargo bin 名保持官方 `codex`，
  npm 打包时改名 `nuwax-codex`，postinstall 从 OSS 下载平台二进制到
  `~/.nuwax-codex-cache`）。
- live-tests crate（`LIVE_VENDORS` + `.env.local` 凭据）按 厂商×桥×场景 矩阵跑
  真实请求；compact 场景已入 exec 矩阵（三线）。

## 9. 差异未及之处

除上述外（沙箱、审批、MCP、TUI、profile v2、配置 schema 主体等）行为与官方
一致。fork 补丁面全景见 `codex-review-prompt.md` §二；设计文档见
`my-docs/nuwax-home/`、`my-docs/anthropic-hosted-tools/`、
`my-docs/rig-responses-phase1|2/`。

## 10. 已知局限（非缺陷，已定位或有规划）

| 局限 | 坐标/依据 | 状态 |
|---|---|---|
| Chat/Anthropic 输出预算可通过 provider.max_output_tokens 配置；Anthropic 未配置时仍默认 16384，Responses typed HTTP/WS、Rig passthrough 与预算已采用该字段 | `codex-rust-rig-bridge/src/stream.rs`、`client.rs` | 三协议已接线；验收见 `provider-request-controls-2026-10-03/tasks.md` |
| 无 Anthropic prompt caching（`cache_control`） | 桥内零出现（grep 实证） | backlog |
| 普通 TLS 客户端复用且请求头按轮注入；自定义 CA 仍专用构建 | `codex-rust-rig-bridge/src/client.rs`、`transport.rs` | 已修复全局缓存按每轮 header 永久增长的问题；取消和长会话验收待做 |
| 搜索原始结果的来源身份、恢复上限、混合轮跨响应配对已加入当前工作树；厂商级全场景验收仍未完成 | `hosted_replay.rs`、`hosted_replay_budget.rs`、`model_output_projection.rs` | 定向与真实请求的范围见 `rig-stability-next/direct-development-2026-10-03-targeted-validation.md` |
| 病态网关"HTTP 200 + 非 SSE JSON 错误体"两线均 EOF 丢体 | `sse.rs` EOF 路径；三家目标厂商 live 未见此行为 | 已知边界（可选加固：EOF 时附 body 摘要） |
| 测试序列化兜底 `unwrap_or_default()` 仅在工具 schema >128 层嵌套时可达（静默零工具） | `request_tools.rs:41-47`（serde_json 递归深度限制，实测无现实 schema 可触发） | P3 nit（debug_assert） |

阶段 D（搜索结果持久化/pause_turn/混合轮）已立项：Spec 见
`phase-d-spec.md`，Plan 见 `phase-d-plan.md`（2026-10-01）。
