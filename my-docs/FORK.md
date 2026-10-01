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

设计：`my-docs/anthropic-hosted-tools/`。

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
（正整数）→ 桥线 `max_tokens`（Anthropic 必填字段由此覆盖默认 16384；Chat 可选）；
Responses 直传按原文发送不受影响。远程 thread config 尚无此字段线格式（转 None）。

**桥性能/安全项**（2026-10-01 起）：
- 连接复用：同静态头的请求共享 reqwest 连接池（按 protocol+头指纹进程级缓存，
  免每轮 TLS 握手；鉴权头始终按轮注入，custom CA 路径仍逐轮构建）。
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
| Anthropic 固定 `max_tokens=16384`，长输出/重思考模型命中即整轮报错 | `codex-rust-rig-bridge/src/client.rs:120`；与官方 `response.incomplete` 行为一致（对齐而非缺陷） | backlog（可配置化） |
| 无 Anthropic prompt caching（`cache_control`） | 桥内零出现（grep 实证） | backlog |
| 每轮新建 reqwest client（每轮 TLS 握手） | `stream.rs`/`responses.rs` 每次 `http_client(...)` | backlog |
| 跨轮 web_search 检索上下文丢失：`WebSearchCall` 条目无结果字段，桥回放时丢弃（无悬空块风险，assistant 文本保留） | `protocol/src/models.rs:1190-1203`、`request_messages.rs:207-219`；两轮复核定案"丢弃正确" | **phase-3 增强**：新增结果持久化字段 + 成对回放 `server_tool_use`/`web_search_tool_result` |
| 病态网关"HTTP 200 + 非 SSE JSON 错误体"两线均 EOF 丢体 | `sse.rs` EOF 路径；三家目标厂商 live 未见此行为 | 已知边界（可选加固：EOF 时附 body 摘要） |
| 测试序列化兜底 `unwrap_or_default()` 仅在工具 schema >128 层嵌套时可达（静默零工具） | `request_tools.rs:41-47`（serde_json 递归深度限制，实测无现实 schema 可触发） | P3 nit（debug_assert） |

阶段 D（搜索结果持久化/pause_turn/混合轮）已立项：Spec 见
`phase-d-spec.md`，Plan 见 `phase-d-plan.md`（2026-10-01）。
