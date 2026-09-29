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
  （`responses_routes_via_chat_bridge`，fork 关键策略）。

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
  实测接受）；无对应物的工具仍丢弃并告警。新工具 = 加表项。
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

- 配置键 `model_auto_compact_ratio`（0<r≤1）：limit = ratio × context_window；
  绝对值 `model_auto_compact_token_limit` 优先；推导在
  `Config::to_models_manager_config`（两字段保证同源），越界（负数/0/>1）不推导
  ——负 limit 会导致每轮压缩。只设 ratio 不设 window 时，model-info 层按模型
  实际窗口推导（多厂商容器免写死 token 数）。
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
