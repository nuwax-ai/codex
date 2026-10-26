# FORK.md — nuwax-codex 与官方 codex 的逻辑差异

维护说明：本文是 fork 行为差异的权威索引。改动 fork 行为时同步更新本文与
`codex-review-prompt.md`（§二 补丁面清单）。2026-10-22 的上游同步目标为
`main@342ed69a1`，Rust `1.99.1`、V8 `152.2.0`，rig 升至 `0.45.0`（2026-10-26，用户解除
0.44 冻结；桥机械适配 Model 构造器/Arc 消息与非穷尽错误，行为合同不变）；合并冲突已解，
main集成与晚间批次已推送，复审输入origin/test=850a87390；备用genai现为0.7.0。
不同源码的实网、fixture与CI证据分开，不宣称全部厂商/全部平台已通过。
当前优先级与本轮验证快照见 [ROADMAP.md](ROADMAP.md)，
历史执行证据见 [最新验收台账](rig-production-completion-2026-10-10/claude-rig-final-acceptance-2026-10-18.md)。
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
环境变量启动指南与模板见[docs/nuwax-harness](../docs/nuwax-harness/README.md)。

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
  放宽为 live）；无对应物的工具仍丢弃并告警。新工具 = 加表项。
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
[最新台账 §2、§4](rig-production-completion-2026-10-10/claude-rig-final-acceptance-2026-10-18.md) 为准。

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
| usage/缓存计数 | 同协议解码，保留字段 presence | 保留缺失/部分/显式零及可用 cache 计数 | 保留 start/delta、cache 和显式零语义，避免重复加计 |
| 重试/限流 | 受 provider 配置及 Core 采样策略约束 | 桥传输/流重试与 Core 采样分层；无统一总尝试数保证 | 同 Chat |
| web_search 模式 | 原样（含 cached/indexed） | 丢弃声明（无 hosted 概念） | 仅 live；cached/indexed fail-fast |
| `parallel_tool_calls` | 原样 | 原样 | hosted-only 时还原 tool_choice 并关 parallel |

`cached_web_search` 能力当前按 `native_transport` 绑定（model-provider
`capabilities.rs`）：native Responses为true，可保留cached；Rig Responses桥为false，
Core默认Cached据此降级Disabled。手工构造Responses请求的同协议字段透传是另一层，
不应与Core默认能力混同。Chat/Anthropic桥收到无法表达的cached/indexed请求时fail-fast。按协议×厂商
的精细能力矩阵仍为登记项（R5 追踪），本轮未改默认搜索策略。



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
[字段与图片说明](rig-production-completion-2026-10-10/rig-field-image-closure-2026-10-18.md)
和最新台账。

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
code-mode-host、rg）。先前fork npm包只发单二进制，自启所需布局不完整；v0.18.14补入Code Mode host，
但仍不宣称具备上游完整daemon bundle（如独立rg）
。fork 在
`tui/src/daemon_startup.rs::exclusion` 排除 NpmNuwax 安装方式（安装方式参数
注入，hermetic 可测），TUI 走内嵌 app-server；显式 `app-server daemon` 命令不受影响。

## 7. fork 专属字段与环境变量总表

**config.toml 键**（官方 schema 不认识，只写 `~/.nuwax-harness`）：
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
2026-10-20 增加 `NUWAX_BRIDGE`（值域同 provider 的 `experimental_bridge`：
`rig`|`native`|`genai`）：种子层随之写入 `experimental_bridge`，让官方 GPT 经中转
端点时用上游原生 Responses 传输而不是 rig 桥；`native` 必须配
`NUWAX_WIRE_API=responses`（chat/anthropic组合明确报错，不静默忽略），未知值与孤立设置同样 fail-fast 报变量名。未设置时仍是 fork 默认 `rig`，
daemon 子进程同样剥离该变量；doctor 的 `config.model_routing` 收据相应显示
`bridge: native transport` 并列出 `experimental bridge override:`。
多进程应分别设置独立 `CODEX_HOME`；参数表、启动示例、远程与会话
边界见 [容器多进程审查](container-multiprocess-env-review-2026-10-03.md)。
（ACP-TS 侧另有 `CODEX_BASE_URL`/`CODEX_API_PROTOCOL`/`CODEX_WIRE_API`/
`INITIAL_AGENT_MODE` 等，见该仓库。）
ACP的CODEX_*入口显式选择custom-gateway，现支持独立`CODEX_BRIDGE=rig|native|genai`
映射；NUWAX_BRIDGE不覆盖这个显式provider。native仅允许OpenAI Responses，非法组合
启动失败。环境网关已授权，无需显式authenticate；显式providers/set或任何authenticate
请求按既有优先级替换/清除该环境配置。隐藏标题线程复制完整活动provider定义，防止
遗漏bridge/headers/query或回退到固定官方模型。28/28子进程fixture与GLM原生实网4/4
见[收尾记录](rig-production-completion-2026-10-10/codex-core-closure-2026-10-22.md)。

## 8. 版本与发布

正式版本 `v0.18.14` → `f82730975` 已发布（2026-10-23）：
[Release39792664843](https://github.com/nuwax-ai/codex/actions/runs/39792664843) attempt1、10/10 jobs success，
GitHub七个平台归档公开、OSS7/7、npm `nuwax-codex@0.18.14` / latest 已实际查询与隔离安装。
Mac ARM默认/显式home、Linux/Windows显式CODEX_HOME的实际初始化与metadata通过，正式Mac host stdio1/1、
正式制品ACP四组28/28；Intel CI制品经Apple Silicon上的Rosetta实际运行验证，不冒称物理Intel宿主。
`v0.18.13` 的前一 Release 已主动取消、从未发布 npm，原 tag 保留不改写。
macOS 使用当前标准 runner，musl 编译并行限制为 2；Rust/V8 继续从仓库固定版本与
可信配对清单读取。完整制品、首败与执行边界见
[发布记录](rig-production-completion-2026-10-10/nuwax-harness-release-0.18.14.md)。
Release成功不改写旧完整Core CI三条nextest红，平台限制继续按§10维护，主线已收口。

- workspace 版本恒为占位 `0.0.0`；npm 版本号来自 git tag（`v*` → release.yml，
  7 平台构建 → 阿里云 OSS 二进制 → npm publish；cargo bin 名保持官方 `codex`，
  npm 打包时主程序改名 `nuwax-harness`，保留旧命令兼容入口，postinstall 从 OSS 下载平台二进制到
  `~/.nuwax-harness-cache`）。
- 主程序 `--version`/`-V` 只输出版本号本身（如 `0.18.14`），不带官方的
  `codex-cli` 前缀；tarball 安装脚本 `install.sh` 的版本探测兼容新旧两种输出。
- live-tests crate（`LIVE_VENDORS` + `.env.local` 凭据）按 厂商×桥×场景 矩阵跑
  真实请求；compact 场景已入 exec 矩阵（三线）。

## 9. 差异未及之处

不能再把沙箱、审批、MCP、TUI 等整体标为与官方完全一致：后续已有后台模型选择、
退出清理、图片入口以及Linux沙箱deny等修复。self/thread-self/net家族deny现已
明确Fatal（不再静默失效），并不承诺可运行地拒绝任意per-pid视角。显式/dev分支
的初始缺席不等于不可重建，晚间复审的隔离修正与限定能力见最新复审记录。
当前补丁与未闭合安全边界
见 [ROADMAP.md](ROADMAP.md) 和最新台账；补丁索引仍见 `codex-review-prompt.md` §二。设计文档见
`my-docs/nuwax-home/`、`my-docs/anthropic-hosted-tools/`、
`my-docs/rig-responses-phase1|2/`。

## 10. 已知限制与历史修复

| 局限 | 坐标/依据 | 状态 |
|---|---|---|
| 显式 `:danger-full-access` 命令曾被同 profile ID 的 Ask preset 抢先匹配，跳过确认 | `tui/src/chatwidget/permissions_menu.rs`；`15283cc98` | 已修：profile-id 入口选 canonical full-access，显式 on-request 与两 reviewer 仍先确认；Ask 菜单既有语义保留，权限模块 49/49 |
| Windows legacy 沙箱 workspace-write **不约束既有文件删除**（内核 WRITE_RESTRICTED 不覆盖 DELETE 权位，diag1-7 证据链见台账 §6.57；显式 deny-write 路径有 DENY ACE、删除受保护）。运行时降级有 WARNING 日志；完整删除约束需 elevated 后端 | `windows-sandbox-rs/src/unified_exec/backends/legacy.rs`、台账 §6.57 | 用户决策 B+警告（2026-10-22），上游继承问题不再扩fork补丁面 |
| Chat/Anthropic 输出预算可通过 provider.max_output_tokens 配置；Anthropic 未配置时仍默认 16384，Responses typed HTTP/WS、Rig passthrough 与预算已采用该字段 | `codex-rust-rig-bridge/src/stream.rs`、`client.rs` | 三协议已接线；验收见 `provider-request-controls-2026-10-03/tasks.md` |
| 无 Anthropic prompt caching（`cache_control`） | 桥内零出现（grep 实证）；rig 0.45 的 Messages wire 已带 `prompt_caching`/`static_prefix_cache_ttl` 原语，桥仍未启用 | backlog（0.45 起具备实施条件，需 provider 级设计与实网验收） |
| 普通 TLS 客户端复用且请求头按轮注入；自定义 CA 仍专用构建 | `codex-rust-rig-bridge/src/client.rs`、`transport.rs` | 取消与连续工作流已有定向/实网证据；本次 main 合并后重新验收受影响部分 |
| 搜索来源身份、恢复上限、混合轮配对已入库；厂商能力仍分别受限 | `hosted_replay.rs`、`hosted_replay_budget.rs`、`model_output_projection.rs` | 离线 wire 与真实厂商证据分开，不能把 GLM 搜索支持扩大至 MiMo/Step |
| 病态网关"HTTP 200 + 非 SSE JSON 错误体"两线均 EOF 丢体 | `sse.rs` EOF 路径；三家目标厂商 live 未见此行为 | 已知边界（可选加固：EOF 时附 body 摘要） |
| raw 工具定义转换失败或整数超出精确可表示范围 | `request_tools.rs::request_tools` | 已改为 `InvalidRequest`，不再 panic/清空工具表/近似化整数；不是仍待修的 nit |

阶段 D（搜索结果持久化/pause_turn/混合轮）已立项：Spec 见
`phase-d-spec.md`，Plan 见 `phase-d-plan.md`（2026-10-01）。
