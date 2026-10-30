# Nuwax Harness：配置与启动指南

本文是 fork 的**使用侧权威文档**：装什么、怎么配自定义模型(临时环境变量或持久
`config.toml`)、`NUWAX_*` 与 `CODEX_*` 两套环境变量怎么用，以及启动命令行、
App Server 或 ACP agent。兼容 OpenAI Chat Completions、OpenAI Responses 和
Anthropic Messages;默认通过 rig。原生 Responses 是可选通路，适合需要上游传输
的 GPT 或兼容中转站。模型仍需具备相应的工具调用或视觉能力，选择协议不会补足
模型自身缺失的能力。

**fork 的行为差异**(协议路由、窗口语义、压缩、品牌)见 [FORK.md](FORK.md);
无凭据环境变量模板见 [nuwax.env.example](nuwax.env.example)(harness CLI)与
[acp.env.example](acp.env.example)(ACP agent)。

## 1. 安装与名称

```bash
npm install -g @nuwax-ai/nuwax-harness@latest
command -v nuwax-harness
nuwax-harness --version
```

### 源码安装(开发与日常 dogfood)

改完代码一条命令更新本地 `nuwax-harness`,不必等 npm 发布:

```bash
cd <仓库>/codex-rs && set -a && source <V8 env 文件> && set +a && \
  CARGO_TARGET_DIR=~/.nuwax-cargo-target \
  cargo install --path cli --bin codex --root ~/.nuwax-cargo --locked --force
ln -sf ~/.nuwax-cargo/bin/codex ~/.cargo/bin/nuwax-harness
```

- 源码构建版本号恒为 `0.0.0`,可用来区分 npm 版;workspace 版本恒占位,
  因此每次重装都需要 `--force`。`CARGO_TARGET_DIR` 固定后后续重装走增量编译。
- V8 env 文件由 `scripts/prefetch_rusty_v8.sh` 生成;不 source 它构建会失败。
- 安装到独立 root 再软链,**不要**把 fork 的 `codex` 装进 `~/.cargo/bin`,
  避免与官方 codex(如 homebrew 的 `/opt/homebrew/bin/codex`)冲突。
- 源码二进制运行 TUI 前需设 `CODEX_INSTALL_SOURCE=npm_nuwax`(可放进下文的
  环境变量文件),否则会尝试自启共享 daemon(需要完整 npm 包布局,必失败);
  设置后走内嵌 app-server,与 npm 制品行为一致。


npm 组织包名是 **`@nuwax-ai/nuwax-harness`**，主命令和发布归档内的主二进制叫
**`nuwax-harness`**（Windows 为 `nuwax-harness.exe`）。该组织包只导出新主命令。
ACP 的 `CODEX_PATH` 使用 `command -v nuwax-harness` 返回的绝对路径。
Rust crate、源码构建目标和 `codex-code-mode-host` 等辅助程序保持内部名称；
发布包会处理主程序改名，请完整安装同一版本，保留其配套辅助程序。

默认数据目录是 **`~/.nuwax-harness`**，保存配置、会话与本地状态；显式
`NUWAX_HOME`（同义别名 `CODEX_HOME`）优先；显式设置的路径须预先 `mkdir -p`
创建，可将它设到外接盘。两者同时设置且值不同会在启动时报错并点名两个变量。
此版本不读取、迁移或删除旧用户级 `.codex-nuwax`、`.nuwax-codex` 或 `.codex` 默认目录。
项目中的 `.codex/` 属于项目级配置/资源入口，仍按既有规则加载，与上述用户级默认目录改名分开。
npm 下载缓存是 **`~/.nuwax-harness-cache`**，用于保存
对应版本和平台的主程序及辅助程序，与会话数据目录分开。

## 2. 直接运行：`NUWAX_*`

这是 CLI、`exec` 和独立 `app-server` 共用的入口。以下以 OpenAI Chat 为例，
把 Base URL 和模型名换成平台实际提供的值：

```bash
export NUWAX_BASE_URL='https://your-provider.example/v1'
export NUWAX_WIRE_API='chat'
export NUWAX_MODEL='your-model-id'
export NUWAX_BRIDGE='rig'
# 从受保护的环境注入 NUWAX_API_KEY；也可在 Bash 中临时输入：
read -r -s -p 'API Key: ' NUWAX_API_KEY
printf '\n'
export NUWAX_API_KEY

# 交互式命令行
nuwax-harness

# 单次执行；在现有 Git 项目中运行
nuwax-harness exec '请只回复 OK'

# 独立 App Server，默认通过 stdio 传输 JSON-RPC
nuwax-harness app-server --stdio
```

`app-server` 需要由客户端完成初始化与会话调用，启动本身不会发送一条模型对话。
若确实需要本机 WebSocket 接入，可用
`nuwax-harness app-server --listen ws://127.0.0.1:4500`。

可复制 [nuwax.env.example](nuwax.env.example) 到仓库外的受保护目录，改好地址与模型后加载：

```bash
mkdir -p "$HOME/.config/nuwax-harness"
cp nuwax.env.example "$HOME/.config/nuwax-harness/agent.env"
chmod 600 "$HOME/.config/nuwax-harness/agent.env"
# 先通过你的密钥管理方式导出 NUWAX_API_KEY，再加载；模板不会保存真实 Key。
. "$HOME/.config/nuwax-harness/agent.env"
mkdir -p "${NUWAX_HOME:-$HOME/.nuwax-harness}"
nuwax-harness doctor --json
nuwax-harness exec '请只回复 OK'
```

环境模板是可信的 **Shell 配置**，不会被程序自动读取，也不是 Docker 或 systemd 的
原始 env-file 格式。只加载自己维护的文件。`$CODEX_HOME/.env` 不能激活 `NUWAX_*`
启动组，变量必须进入启动进程的真实环境。

### 协议与桥选择

| 目标 | `NUWAX_WIRE_API` | `NUWAX_BRIDGE` | Base URL 的含义 |
| --- | --- | --- | --- |
| OpenAI Chat Completions | `chat` | `rig`，或不设 | 平台的 Chat API 基址 |
| OpenAI Responses，经 rig | `responses` | `rig`，或不设 | 平台的 Responses API 基址 |
| Anthropic Messages | `anthropic` | `rig`，或不设 | 平台的 Messages API 基址 |
| 原生 OpenAI Responses | `responses` | `native` | 官方或中转站的 Responses API 基址 |
| genai 备用桥 | `chat` 或 `anthropic` | `genai` | 对应协议基址；不支持 Responses |

使用平台提供的**协议基址**，不要自行附加 `/chat/completions`、`/responses` 或
`/messages`。协议由变量决定，不会从 URL 自动推断。不同协议可能使用完全不同的基址。
例如本项目验收时 GLM 的 Chat 基址为 `https://open.bigmodel.cn/api/coding/paas/v4`，
Responses 为 `https://open.bigmodel.cn/api/v1`，Anthropic 为
`https://open.bigmodel.cn/api/anthropic`；实际以你账号所属套餐提供的地址和模型为准。

切换到原生 Responses：

```bash
export NUWAX_BASE_URL='https://your-responses-provider.example/v1'
export NUWAX_WIRE_API='responses'
export NUWAX_BRIDGE='native'
export NUWAX_MODEL='your-model-id'
# NUWAX_API_KEY 同时换为该端点的 Key。
nuwax-harness doctor --json
```

原生时 doctor 路由会显示 `bridge: native transport`；rig 显示
`bridge: rig (model bridge)`。切回 Chat 或 Anthropic 时，必须将 `NUWAX_BRIDGE`
改回 `rig` 或 `unset NUWAX_BRIDGE`。`native` 与这两种协议组合会报错。
本轮实网原生验收使用 GLM；不能据此宣称每个官方 GPT 或中转站都已逐一实测。

### 直接入口变量表

| 变量 | 用途与约束 |
| --- | --- |
| `NUWAX_BASE_URL` | 完整 HTTP(S) 基址，不能在 URL 用户信息中夹带凭据 |
| `NUWAX_WIRE_API` | `chat`、`responses`、`anthropic`；必须显式设置 |
| `NUWAX_API_KEY` | 该协议端点的 Key，与上述两项成组设置，不允许空值 |
| `NUWAX_MODEL` | 平台实际模型 ID；完整启动组需要它，或用显式 `--model` / `-m` 提供模型 |
| `NUWAX_BRIDGE` | 可选：`rig`、`native`、`genai`；不设时保持本 fork 的 rig 默认值 |
| `NUWAX_MAX_OUTPUT_TOKENS` | 可选正整数，每次生成的输出上限；不是上下文窗口大小 |
| `NUWAX_REQUEST_MAX_RETRIES` | 可选非负整数，HTTP 请求重试次数，`0` 表示本层仅发一次 |
| `NUWAX_STREAM_MAX_RETRIES` | 可选非负整数，流恢复/采样重试预算，`0` 禁用本层重试 |
| `NUWAX_STREAM_IDLE_TIMEOUT_MS` | 可选正整数，流空闲超时，单位毫秒；不是整轮总时限 |
| `CODEX_HOME` | 会话、配置、状态目录；不设时为 `~/.nuwax-harness`，可覆盖为外接盘或持久化卷绝对路径。同义别名 `NUWAX_HOME`（同时设置且值相同视为迁移期写法；值不同则启动报错，两个变量名都会出现在错误里） |

`NUWAX_BRIDGE`、输出上限和重试变量必须随完整启动组使用，孤立设置会报错。
仅单独设置 `NUWAX_MODEL` 可以选择既有 provider 的模型，并不会自动切换端点。
未覆盖时，provider 默认请求重试为 4、流重试为 5、流空闲超时为 300000 毫秒；
请求与流重试是不同层，两者不构成单一总请求上限。重试次数的有效值最多为 100。

配置优先级为：显式 CLI 模型/provider 选择 > 显式 `-c` 对应键 > 本组环境变量 >
配置文件。显式选择其他 provider 时，整组 `NUWAX_*` 不参与该次运行。
临时 provider ID `nuwax_env` 是保留名称，不要在配置文件里手工定义同名 provider。

## 3. 持久配置：config.toml（日常推荐）

环境变量组适合一次性对比；日常长期使用建议把 provider 写进
`~/.nuwax-harness/config.toml`（目录可用 `CODEX_HOME`/`NUWAX_HOME` 改）。
**API Key 不写进配置文件**，用 `env_key` 引用环境变量名。

以 GLM Coding Plan（Anthropic 线，本项目实网验证最充分的线）为例：

```toml
# ~/.nuwax-harness/config.toml
model = "GLM-5.3-Flash"
model_provider = "glm"

# 按平台规格填真实窗口；自定义模型的显式覆盖全额生效（见 §4）
model_context_window = 1000000

[model_providers.glm]
name = "GLM Coding Plan (Anthropic)"
base_url = "https://open.bigmodel.cn/api/anthropic"
env_key = "GLM_API_KEY"          # Key 从该环境变量读取，不落盘
wire_api = "anthropic"           # responses | chat | anthropic
anthropic_prompt_caching = true  # 可选：Anthropic 线 prompt 缓存（opt-in，5 分钟 TTL）
# max_output_tokens = 8192       # 可选：输出上限；Anthropic 线不设时默认 16384
```

Key 放进受保护环境文件（沿用 §2 的 agent.env 模式，`chmod 600`）：

```bash
echo "export GLM_API_KEY='<你的key>'" >> "$HOME/.config/nuwax-harness/agent.env"
```

想对比多条协议线，用 profile 追加 provider：

```toml
[profiles.glm-chat]
model = "GLM-5.3-Flash"
model_provider = "glm-chat"

[model_providers.glm-chat]
name = "GLM Coding Plan (OpenAI Chat)"
base_url = "https://open.bigmodel.cn/api/coding/paas/v4"
env_key = "GLM_API_KEY"
wire_api = "chat"
```

启动：`nuwax-harness --profile glm-chat`，或临时 `nuwax-harness -c model_provider=glm-chat`。
三条线都经 rig 桥（Responses 线为同协议透传）。

provider 常用字段：

| 字段 | 说明 |
| --- | --- |
| `base_url` | 平台**协议基址**，不要自行拼接 `/chat/completions` 等路径 |
| `env_key` | 存 Key 的环境变量名；Key 永不落盘 |
| `wire_api` | `responses` / `chat` / `anthropic`（后两者为 fork 扩展） |
| `anthropic_prompt_caching` | 可选；Anthropic 线 `cache_control` 断点，默认关 |
| `max_output_tokens` | 可选；每次生成的输出上限 |
| `request_max_retries` / `stream_max_retries` / `stream_idle_timeout_ms` | 可选；重试与流空闲超时 |
| `reasoning_effort_levels` | 可选；该网关的模型真实接受的 effort 档位(如 `["low","high","max"]`)，见 §4 |

其他常用顶层键：`model_auto_compact_ratio` / `model_auto_compact_token_limit`（见 §4）、
`[tools] tool_search = false`（MCP 工具全量直出，见 FORK.md §7.1）。

要点：自定义 provider 不弹官方登录界面，Key 在环境变量里即可直接进 TUI/exec；
优先级为显式 CLI（`-m`/`-c`）> 环境种子（`NUWAX_*`/`CODEX_*`）> config.toml；
`nuwax_env` 是保留 provider ID，不要在配置文件里手工定义。验证配置：
`nuwax-harness doctor --json`，看 `config.model_routing` 的 provider/wire/bridge/
effective context window 是否符合预期。

## 4. 上下文、输出与压缩

自定义模型的真实上下文窗口不能靠名称自动可靠推断，应按平台规格设置。
以下数字只展示写法，不能直接当作所有模型的能力：

```bash
export NUWAX_MODEL_CONTEXT_WINDOW='128000'   # 同义 CODEX_MODEL_CONTEXT_WINDOW 永久兼容
export NUWAX_MAX_OUTPUT_TOKENS='4096'
export NUWAX_AUTO_COMPACT_RATIO='0.8'
```

| 变量（`NUWAX_*` 别名同名去前缀） | 用途 |
| --- | --- |
| `NUWAX_MODEL_CONTEXT_WINDOW`（同义 `CODEX_MODEL_CONTEXT_WINDOW`） | 正整数 token 数，告诉本地预算和压缩逻辑模型窗口大小 |
| `NUWAX_AUTO_COMPACT_TOKEN_LIMIT`（同义 `CODEX_AUTO_COMPACT_TOKEN_LIMIT`） | 可选正整数，指定触发自动压缩的 token 阈值 |
| `NUWAX_AUTO_COMPACT_RATIO`（同义 `CODEX_AUTO_COMPACT_RATIO`） | 可选 `(0, 1]` 小数，未设置显式 token 阈值时按有效窗口比例触发压缩 |
| `NUWAX_MODEL_REASONING_EFFORT`（同义 `CODEX_MODEL_REASONING_EFFORT`） | 可选：`none`、`minimal`、`low`、`medium`、`high`、`xhigh`、`max`、`ultra`、`persistent`；具体模型/协议须支持该值 |

窗口与压缩种子的行为（2026-10-28 起）：

- 不在模型目录中的**自定义模型**，`model_context_window` 显式覆盖**全额
  生效**（不再被未知模型 272k 占位上限钳制）：大窗口厂商（如 GLM-5.3-Flash
  的 1M）按真实窗口使用，压缩阈值随真实窗口派生。
- 配大了自愈：桥线厂商的超窗错误被识别为 `ContextWindowExceeded`，自动触发
  "删最旧条目重试"压缩兜底，不会卡死会话；目录内官方模型（gpt-\*）仍按
  目录权威上限钳制。
- `model_context_window` 与 `model_auto_compact_token_limit` 为 0/负数时
  config 加载即报错（`must be a positive token count`），环境变量入口同样拒绝。
- 对应 config.toml 键与上述环境变量同名（`model_context_window`、
  `model_auto_compact_token_limit`、`model_auto_compact_ratio`），优先级为
  显式 `-c` > 环境变量 > config.toml。

这些 `CODEX_*` 也是 Rust 程序自身支持的通用变量，与下节 ACP 专有网关变量含义不同。

### reasoning effort 档位：选模型真实存在的档

`NUWAX_MODEL_REASONING_EFFORT` 接受 `none`/`minimal`/`low`/`medium`/`high`/
`xhigh`/`max`/`ultra`/`persistent`,但**目标模型支不支持是另一回事**。智谱官方文档
([concept-param#reasoning_effort](https://docs.bigmodel.cn/cn/guide/start/concept-param#reasoning_effort))：

| 模型 | 支持档位 | 非支持值的处理 |
|---|---|---|
| GLM-5.3 / GLM-5.3-FLASH / GLM-5.3-FLASHX | **仅 `max` / `high` / `low`** | 文档未说明 |
| GLM-5.2 | `max`/`xhigh`/`high`/`medium`/`low`/`minimal`/`none` | `low`/`medium` → 映射为 `high`;`xhigh` → `max`;`none`/`minimal` → 放弃思考 |

**重要：200 不代表档位被采纳。** 2026-10-29 在官方 Anthropic 端点实测,
`output_config.effort` 传 `low`/`medium`/`high`/`xhigh`/`max`/`minimal`/`ultra`/
`none` **全部返回 200**——包括任何 GLM 档位表里都没有的 `minimal` 和 `ultra`。
也就是说该端点**不校验 effort 取值**：请求被接受，但档位可能被映射、被忽略，
或落到默认档。我们桥把 Codex `medium` 映射为 Anthropic `medium`
(`codex-rust-rig-bridge/src/convert_request.rs:295`),对 GLM-5.3-Flash
**本就不是真实档位**。

因此：**用 GLM-5.3-Flash 时请显式设 `max`/`high`/`low`**,不要依赖默认映射;
换模型前先查它的档位表。这类跨网关差异由环境变量显式控制,引擎不在背后静默改写
(设计决定见 [FORK.md §10](FORK.md))。

### 声明模型真实档位：`NUWAX_REASONING_EFFORT_LEVELS`

上面那个"200 不代表被采纳"的坑，根源是引擎不校验档位。现在可以显式声明：

```bash
# GLM-5.3 / GLM-5.3-FLASH / GLM-5.3-FLASHX：官方文档仅 low / high / max
export NUWAX_REASONING_EFFORT_LEVELS='low,high,max'
```

- 取值是**逗号分隔的 rig 档位词**：`minimal`/`low`/`medium`/`high`/`xhigh`/`max`。
  词表由 rig 的 `Effort` 枚举定义，我们不再自己维护一份；写错的词会在启动后
  第一次请求前就报错，**点名该变量、出错的词、以及 rig 接受的全部档位**。
- 声明之后：选了模型没有的档位(如对 GLM-5.3-Flash 用 `medium`)，以及
  `NUWAX_MODEL_REASONING_EFFORT=none`(这些网关始终思考)，都会在**请求发出前**
  失败，不会再出现"发了 medium、拿到 low 的输出"这种静默偏差。
- **不能写 `none`/`off`**：它们表示"能否关闭思考"，是另一个轴，不是档位；
  写进去会被拒绝并说明原因。
- 不设置时保持宽松——沿用 rig 对未收录模型的默认，不校验档位(即修复前的行为)。
- config.toml 的对应键是 `model_providers.<id>.reasoning_effort_levels`，
  与该环境变量同义；显式 `-c` 优先。空列表与"不设置"含义不同(空列表表示
  "不接受任何档位")，因此空值会被拒绝。
- **该变量必须能附着到一个 provider**：随完整 `NUWAX_` 启动组使用，或配合
  `-c model_provider=<id>`。两者都没有时(例如只靠 config.toml 的
  `model_provider` 选 provider)会在启动时报错并点名该变量——静默丢掉比报错
  更糟，那会让你以为档位在校查，其实没有。用 `--oss` 等显式 provider 选择时
  同理，请把 `reasoning_effort_levels` 直接写进那个 provider 表。

两条经 rig 的线(Chat 与 Anthropic)都受这道门约束，原生 Responses 透传不经过
rig，档位由上游自己校验。两条线的实现方式不同(Anthropic 交给 rig 的类型化通道，
Chat 由桥自己校验)，但对你是同一件事：**配了档位表，选了表里没有的档就在发出前
报错**；没配则保持宽松。原因见 [FORK.md §10](FORK.md)。

### GLM-5.3-Flash 的推荐配置

引擎在 Default 模式**并不发送 effort**：网关模型对内置模型目录是陌生 slug，
`default_reasoning_level` 为 `None`（[model_info.rs:170](codex-rs/models-manager/src/model_info.rs:170)），
effort 整个字段都不上線，档位由网关自己定。真正固定发 `medium` 的是 **TUI 的 Plan
模式预设**（`collaboration_mode_presets.rs` 的 Plan mask 钉死 `medium`，且按上游设计
`model_reasoning_effort` 不影响 Plan 模式——见 `chatwidget/settings.rs` 的
"Plan reasoning is controlled exclusively by the Plan preset"）。

```bash
export NUWAX_REASONING_EFFORT_LEVELS='low,high,max'   # 选错档在发出前就报错
export NUWAX_MODEL_REASONING_EFFORT='high'            # 落在一个真实档位
```

这两个变量在 **ACP 路径**（§5）下已经覆盖 Default 与 Plan 两种模式：适配器的
`CollaborationModeConfig.ts` 自己拼 collaboration mode，effort 取自 `ModelId`，
而 `ModelId` 的 effort 优先取 `NUWAX_MODEL_REASONING_EFFORT`，不经过引擎的 Plan 预设。

**只有直接跑 TUI** 才需要第三处配置，否则一进 Plan 模式档位门就会按 `medium`
拒绝请求（这是 0.18.24 之后的预期行为，修复前它会静默把 `medium` 发出去）：

```toml
# config.toml；该键没有环境变量别名，且只有 TUI 会应用它
plan_mode_reasoning_effort = "high"
```

档位表见[智谱 concept-param#reasoning_effort](https://docs.bigmodel.cn/cn/guide/start/concept-param#reasoning_effort)：
GLM-5.3 全系仅 `max`/`high`/`low`，GLM-5.2 为七档（`low`/`medium` 均映射为 `high`）。

**默认值保持 `medium` 不动**（与上游一致）。改成 `high` 只是把不兼容从 `medium`
挪到另一个词上，还会盖掉上面那道事前警报；`high` 也并非对所有网关都安全
（DeepSeek/MiMo/StepFun/Ollama 的档位表各不相同，本仓库因此不随包内置任何档位
声明）。要对某个部署改默认，用上面的环境变量，不要改引擎。

显式 `-c model_context_window=...` 等对应配置覆盖这些环境种子。
每个 `CODEX_*` 种子都有同义 `NUWAX_*` 别名（如 `NUWAX_MODEL_CONTEXT_WINDOW`）：
仅设其一即生效，`NUWAX_*` 是推荐写法；两者同设且值相同视为迁移期写法，
值不同则启动报错并点名两个变量。
输出、文本和图片都占窗口；增大配置数字不会扩大服务端的实际能力。

## 5. 通过 ACP agent 使用：`CODEX_*`

你目前的使用路径是 `fork-codex-acp-ts` 的 **`dev` 分支**启动本 fork 的 App Server。
以下网关变量应放入 **ACP agent 进程**的环境，不是仅放在其外层交互终端。
**每个 `CODEX_*` 网关变量都有同义 `NUWAX_*` 别名**（`NUWAX_PATH`、`NUWAX_BASE_URL`、
`NUWAX_API_KEY`、`NUWAX_API_PROTOCOL`、`NUWAX_WIRE_API`、`NUWAX_BRIDGE`、
`NUWAX_MODEL`、`NUWAX_MODEL_CONTEXT_WINDOW`、`NUWAX_PROVIDER_ID/NAME`、
`NUWAX_CONFIG`、`NUWAX_LOG_DIR`、`NUWAX_MODEL_REASONING_EFFORT`）：仅设其一即生效，
`NUWAX_*` 是推荐写法；两者同设且值相同视为迁移期写法，值不同则 ACP 启动报错并
点名两个变量。**例外**：`NUWAX_REASONING_EFFORT_LEVELS` 没有 `CODEX_` 同义名
（非上游 codex 变量，见 §4）。
适配器消费掉的别名（BASE_URL/API_PROTOCOL/WIRE_API/API_KEY/BRIDGE/
MAX_OUTPUT_TOKENS）不会透传给 app-server 子进程，因此引擎自身的
`NUWAX_*` 启动组不会看到孤儿变量。无前缀的 `MODEL_PROVIDER`、
`INITIAL_AGENT_MODE`、`NO_BROWSER`、`APP_SERVER_LOGS` 不设别名（非 codex
命名空间）。

```bash
export CODEX_PATH='/absolute/path/to/nuwax-harness'
export CODEX_BASE_URL='https://your-provider.example/v1'
export CODEX_API_PROTOCOL='openai'
export CODEX_WIRE_API='chat'
export NUWAX_BRIDGE='rig'
export NUWAX_MODEL='your-model-id'
# 从受保护环境注入 NUWAX_API_KEY。
export NUWAX_MODEL_CONTEXT_WINDOW='128000'  # 替换为该模型的真实窗口
export INITIAL_AGENT_MODE='workspace-write'

cd /absolute/path/to/fork-codex-acp-ts
node --import tsx src/index.ts
```

`CODEX_PATH` 使用安装后 `command -v nuwax-harness` 返回的绝对路径，或正式归档解压后
主二进制的绝对路径。它指向 **CLI 主程序**，ACP 自己附加 `app-server` 子命令；
不要指向 `codex-exec`、`codex-app-server`，也不要把 `app-server` 写进路径。

ACP 是 stdio 协议服务，由 ACP 宿主配置上述命令、工作目录和环境，然后完成
`initialize`、`session/new` 与 prompt。直接在终端启动只会等待协议输入。
若宿主使用 npm 启动，应写 `npm --silent start`，避免启动横幅污染协议 stdout。

可复制 [acp.env.example](acp.env.example) 到仓库外，修改后用 `. /绝对路径/acp.env`
加载，再执行上述 Node 命令。若宿主独立管理进程，须将相同变量配置进宿主的 agent
环境；当前终端的 export 不会自动更改已运行的宿主。

| 变量 | ACP 网关用途与默认值 |
| --- | --- |
| `CODEX_PATH` | 固定实际运行的 fork CLI，使用绝对路径 |
| `CODEX_BASE_URL` | 设置后启用 ACP 环境网关，默认 provider ID 为 `custom-gateway` |
| `CODEX_API_KEY` | 网关 Key；未设置时可回退到 `OPENAI_API_KEY`，建议只设前者避免串用 |
| `CODEX_API_PROTOCOL` | `openai`（默认）或 `anthropic` |
| `CODEX_WIRE_API` | OpenAI 协议族下为 `responses`（默认）或 `chat`；Anthropic 时无需设置 |
| `CODEX_BRIDGE` | `rig`、`native`、`genai`；不设则让实际 Codex 二进制保留默认桥 |
| `CODEX_MODEL` | 活动网关使用的平台模型 ID |
| `NUWAX_MODEL_CONTEXT_WINDOW`（同义 `CODEX_MODEL_CONTEXT_WINDOW`） | 活动网关的真实上下文窗口，正整数 token 数 |
| `NUWAX_MODEL_REASONING_EFFORT`（同义 `CODEX_MODEL_REASONING_EFFORT`） | 可选：Default 模式使用的 effort 档位；不设则不发送，由网关自定。**不影响 Plan 模式**（见 §4） |
| `NUWAX_REASONING_EFFORT_LEVELS` | 可选：逗号分隔的 rig 档位词，声明后选表外档位在请求发出前报错。**无 `CODEX_` 同义名**（非上游 codex 变量）；适配器只做形状解析，不消费该变量 |
| `CODEX_PROVIDER_ID` / `CODEX_PROVIDER_NAME` | 可选 provider ID / 展示名，不需要时不设 |
| `CODEX_CONFIG` | 可选 JSON 对象，传入会话配置；高级配置需核对网关覆盖优先级 |
| `MODEL_PROVIDER` | 无活动网关时可选的 provider 选择；不要用它与环境网关表达矛盾选择 |
| `INITIAL_AGENT_MODE` | `read-only`、`workspace-write`、`agent`、`agent-full-access`；按实际授权选择 |
| `NO_BROWSER` | 设置后隐藏 ChatGPT 浏览器登录入口，适合无浏览器部署 |
| `APP_SERVER_LOGS` / `CODEX_LOG_DIR` | 可选日志目录，前者优先；日志也需按敏感数据管理 |

ACP 三种常用配置：

| 目标 | `CODEX_API_PROTOCOL` | `CODEX_WIRE_API` | `CODEX_BRIDGE` |
| --- | --- | --- | --- |
| Chat 经 rig | `openai` | `chat` | `rig` |
| Anthropic 经 rig | `anthropic` | `unset CODEX_WIRE_API` | `rig` |
| Responses 经 rig | `openai` | `responses` | `rig` |
| 原生 Responses | `openai` | `responses` | `native` |

同时替换对应基址、Key、模型。Anthropic 入口由 `CODEX_API_PROTOCOL=anthropic` 选择，
**不能**写 `CODEX_WIRE_API=anthropic`。`native` 仅支持 OpenAI Responses；
`genai` 仅支持 Chat/Anthropic。设置 `CODEX_BRIDGE` 却未设置 `CODEX_BASE_URL` 会报错。

环境网关本身已授权，**无需额外发送显式 `authenticate`**。ACP 宿主若发送
`providers/set` 或任何显式 `authenticate`（API-key、ChatGPT、gateway），会按既有优先级
替换或清除环境网关、桥选择与凭据。想继续使用部署环境时，不要再让宿主主动改登录方式。

选择一套入口配置同一次运行：ACP 的 `CODEX_*` 会显式选择 provider，
`NUWAX_BRIDGE` 不能覆盖它；`NUWAX_MAX_OUTPUT_TOKENS` 等也不是这套网关的控制旋钮。
若采用 ACP 透传 `NUWAX_*` 的另一种部署方式，须保持 `CODEX_BASE_URL`、`MODEL_PROVIDER`
及宿主显式 provider/auth 配置不与之冲突；日常使用优先选上面的 ACP `CODEX_*` 入口。

## 6. Linux / 容器与 Windows

Linux 服务和容器同样要求变量进入实际 agent 进程。为独立服务实例设置各自的
`CODEX_HOME`，放在可写的持久化目录；使用不同凭据的实例分别启动，不复用共享 daemon
来承载各自进程环境凭据。NUWAX 启动组激活时会排除共享 daemon 路径。

Docker 已有镜像可显式透传当前环境，例如主程序和配套文件安装在镜像 `/opt/nuwax/`：

```bash
docker run --rm -i \
  --env NUWAX_BASE_URL --env NUWAX_WIRE_API --env NUWAX_MODEL \
  --env NUWAX_API_KEY --env NUWAX_BRIDGE \
  --env CODEX_HOME=/state \
  --mount type=volume,source=nuwax-harness-state,target=/state \
  --entrypoint /opt/nuwax/nuwax-harness \
  your-built-image:your-version app-server --stdio
```

镜像名和主程序位置是示意，替换为你的实际镜像；其他预算变量需要时同样显式透传。
上述 Shell 模板含 `export` 和变量展开，不能直接传给 `docker --env-file` 或 systemd
`EnvironmentFile=`；这两种机制使用各自的 `NAME=value` 文件格式。

Windows PowerShell 的同等写法（这里演示原生 Responses）：

```powershell
$env:NUWAX_BASE_URL = 'https://your-responses-provider.example/v1'
$env:NUWAX_WIRE_API = 'responses'
$env:NUWAX_BRIDGE = 'native'
$env:NUWAX_MODEL = 'your-model-id'
# 通过你的受保护环境提供 $env:NUWAX_API_KEY。
& 'C:\Tools\nuwax-harness\nuwax-harness.exe' exec '请只回复 OK'
```

Windows 的发布主程序名是 `nuwax-harness.exe`；ACP 的 `CODEX_PATH` 相应填该绝对路径。
已接受的 Windows 沙箱/终端平台限制不等于模型接入未实现，验收边界见项目 ROADMAP。

## 7. 图片与排查

本地图片通过结构化图片块发送：Chat 使用 `image_url`，Responses 使用 `input_image`，
Anthropic 使用 `image` 的 Base64 source。无法提供服务端可访问的 URL 时，内联数据是正常
路径；不要把本地 `file://` 路径当成远端可访问 URL，也不要把 Base64 粘成普通提示词文本。
MCP 工具应返回 image 内容块，Code Mode 使用图片输出接口。图片仍消耗视觉 token，
本项目的图片预算与压缩会处理它们，但不能把附件引用理解成零上下文成本。

遇到接入问题，依次核对：

1. `command -v nuwax-harness`、`--version` 和 ACP 的 `CODEX_PATH` 是否指向本轮 fork。
2. Base URL、协议、Key、模型是否属于同一个平台/套餐，是否留有上一套环境变量。
3. `nuwax-harness doctor --json` 的 provider、wire 和 bridge 是否符合选择。
4. 是否有 CLI `-c`、本地配置或 ACP 的显式 provider/auth 选择覆盖了环境入口。
5. 上下文窗口、输出上限和模型实际能力是否一致。

自定义/未知模型收到的身份声明会注入**实际模型名**（"You are the {模型} model
… running in nuwax-harness"），因此询问"你是什么模型"应回答真实型号与
nuwax-harness；目录内官方模型（gpt-\*）保持上游身份声明。

不要用 `set -x`、`env` 全量转储或提交受保护环境文件来排查凭据。
`doctor` 验证配置与路由，不代表实际端点已经完成模型调用；用短文本和真实工具各验一轮。
进一步的实现状态见 [ROADMAP](my-docs/ROADMAP.md)。

## 8. 附录：环境变量速查

**NUWAX_* 直接启动组**（§2；三件套 `NUWAX_BASE_URL`+`NUWAX_WIRE_API`+
`NUWAX_API_KEY` 成组激活临时 provider `nuwax_env`；其余须随组使用）：
`NUWAX_MODEL`、`NUWAX_BRIDGE`、`NUWAX_MAX_OUTPUT_TOKENS`、
`NUWAX_REQUEST_MAX_RETRIES`、`NUWAX_STREAM_MAX_RETRIES`、
`NUWAX_STREAM_IDLE_TIMEOUT_MS`、`NUWAX_REASONING_EFFORT_LEVELS`。

**NUWAX_\* 别名**（每个都有等价 `CODEX_*` 拼写并永久兼容：仅设其一即生效、
同值幂等、异值启动报错点名两个变量）：

- 引擎侧（Rust 二进制读取）：`NUWAX_HOME`、`NUWAX_MODEL_CONTEXT_WINDOW`、
  `NUWAX_AUTO_COMPACT_TOKEN_LIMIT`、`NUWAX_AUTO_COMPACT_RATIO`、
  `NUWAX_MODEL_REASONING_EFFORT`、`NUWAX_TOOL_SEARCH`。
- ACP 适配层（fork-codex-acp-ts 读取）：`NUWAX_PATH`、`NUWAX_BASE_URL`、
  `NUWAX_API_KEY`、`NUWAX_API_PROTOCOL`、`NUWAX_WIRE_API`、`NUWAX_BRIDGE`、
  `NUWAX_MODEL`、`NUWAX_MODEL_CONTEXT_WINDOW`、`NUWAX_PROVIDER_ID`、
  `NUWAX_PROVIDER_NAME`、`NUWAX_CONFIG`、`NUWAX_LOG_DIR`。注意
  `NUWAX_BASE_URL/WIRE_API/API_KEY/BRIDGE` 在两层语义相同（配置活动厂商），
  ACP 消费后会从 app-server 子进程剥离，不会触发引擎侧的孤儿检测。

**不设别名**（非 codex 命名空间或行业标准名）：`OPENAI_API_KEY`、
`MODEL_PROVIDER`、`INITIAL_AGENT_MODE`、`NO_BROWSER`、`APP_SERVER_LOGS`。

launcher/shim 管道变量（`CODEX_INSTALL_SOURCE`、`CODEX_MANAGED_BY_*`）由
程序注入，不是用户配置面，同样不设别名。
