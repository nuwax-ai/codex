# 容器内多进程与环境变量配置审查（2026-10-03）

后续开发：Responses cap 和 Rig HTTP retry 已接线，分批验收及边界见 [请求控制任务记录](provider-request-controls-2026-10-03/tasks.md)。§7 的 65 项是前轮环境隔离验证，不代表本轮新控制逻辑的验收。

## 1. 审查范围与结论

基于 `test` 分支，HEAD `20898140f2b3638aac6ab328b24d2f604871bfc8`，以及当前未提交工作树。
本轮检查 CLI/TUI/exec/standalone app-server 的启动、配置层、daemon、恢复会话和 Rig 出站路径。
此前协议、history projection、真实厂商请求的验证记录见
[direct-development-2026-10-03-targeted-validation.md](rig-stability-next/direct-development-2026-10-03-targeted-validation.md)。

**每个进程可以通过真实启动环境选择厂商、协议、模型和凭据。建议每个 worker 使用独立、绝对路径的 `CODEX_HOME`。**
更换模型环境变量不会自动隔离会话、配置文件、SQLite 或 daemon；共用 home 时这些仍属于同一个状态域。
本轮修复了局部配置缺口，尚不能宣称所有 provider 参数都有环境变量入口、所有协议字段都已覆盖。

## 2. 环境变量支持表

| 变量 | 支持与含义 | 注意事项 |
|---|---|---|
| `NUWAX_BASE_URL` | 临时 provider 的 HTTP(S) base URL | 必须有 host，不能嵌入用户名/密码；填写 base，不是完整 completion endpoint |
| `NUWAX_WIRE_API` | `responses` / `chat` / `anthropic` | 显式选择实际协议；Responses 通过 Rig 的 Responses 路径，当前已非 Responses→Chat |
| `NUWAX_API_KEY` | 临时 provider 凭据 | 配置只存 `env_key="NUWAX_API_KEY"` 引用；三个基础组变量必须齐设 |
| `NUWAX_MODEL` | 模型名称 | 激活 provider 时须设置，或显式传 `-m` / `-c model`；单独设置只改既有 provider 的模型 |
| `NUWAX_MAX_OUTPUT_TOKENS` | **三协议支持**，实际生成 token cap | Anthropic 为 `max_tokens`；Chat 通常为 `max_tokens`，Rig 对部分 OpenAI reasoning 模型转换为 `max_completion_tokens`。正整数、i64 范围；须激活完整组；Responses 为 `max_output_tokens`，包含 reasoning tokens |
| `CODEX_MODEL_REASONING_EFFORT` | 本地 reasoning effort 配置 | 接受 `none/minimal/low/medium/high/xhigh/max/ultra/persistent`；协议映射、模型支持可能有压缩或省略，解析接受不代表厂商支持所有等级 |
| `CODEX_MODEL_CONTEXT_WINDOW` | 本地模型上下文预算 | 正整数；可能受 model catalog 上限约束；不会扩展厂商实际窗口，也不是输出 token 上限 |
| `CODEX_AUTO_COMPACT_TOKEN_LIMIT` | 本地自动压缩绝对阈值 | 正整数；与 ratio 同时配置时绝对阈值优先 |
| `CODEX_AUTO_COMPACT_RATIO` | 本地自动压缩比例 | `(0, 1]`，例如 `0.8`；不是厂商请求字段 |
| `CODEX_HOME` | 配置、会话和默认状态根目录 | **每个 worker 独立**；先创建目录，使用绝对路径 |
| `CODEX_SQLITE_HOME` | 单独指定 SQLite 目录 | 优先级低于已配置的 `sqlite_home`；相对路径按 cwd 解析。只隔离 SQLite 不会隔离会话与 daemon |
| `RUST_LOG` | 日志过滤 | 可按进程分别设置 |
| `CODEX_RIG_REQUEST_CAPTURE_FILE` | 可选 Rig 请求体采集文件 | 每个进程用独立路径；正文可能包含 prompt/工具输入，属于诊断材料 |

### 配置优先级与启动语义

- NUWAX 常规层顺序：文件/profile/project → EnvSeed → 显式 `-c` / typed CLI；legacy managed 层和 requirements 可进一步覆盖或限制最终值。
- CODEX effort/context/compact 环境参数由 `parse_overrides` 注入 SessionFlags；解析时已先检查显式 `-c`，所以同键显式 `-c` 仍优先。它们不是 NUWAX 保留 provider 表的一部分。
- `-m` 优先于 `NUWAX_MODEL`；显式选择另一个 provider 时整个 NUWAX provider 组被忽略，不验证其未使用值。
- `model_providers.nuwax_env` 是保留表，不能在文件或 `-c` 里补 headers/retries 等字段。本轮允许的种子表键为 `name/base_url/wire_api/env_key/max_output_tokens`。
- 参数来自**真实进程环境**。`$CODEX_HOME/.env` 会过滤全部 `CODEX_*` / `NUWAX_*`，不能用该文件注入这些启动参数。
- 环境种子在进程启动时解析。配置 reload 复用原种子；外部修改环境后需要重启进程。
- 临时 provider 种子不自动写入 `config.toml`。用户主动保存 TUI 默认设置、调用配置写 RPC 是另一种显式持久化行为。

源码入口：`utils/cli/src/nuwax_env.rs`、`utils/cli/src/config_override.rs`、
`config/src/config_layer_source.rs`、`core/src/config/mod.rs`、`arg0/src/lib.rs`（均相对 `codex-rs/`）。

## 3. 推荐的同容器启动方式

以下以 `codex exec` 为例；npm 分发的可执行名称是 `nuwax-codex`，参数相同。
URL/模型是用户提供的 plan 配置；具体可用性仍由厂商账号、套餐和模型权限决定。
API key 由容器外注入 `MIMO_API_KEY` / `GLM_API_KEY`，示例不含真实凭据。

```bash
mkdir -p /var/lib/codex/mimo-worker /var/lib/codex/glm-worker

env -u CODEX_SQLITE_HOME CODEX_HOME=/var/lib/codex/mimo-worker \
  NUWAX_BASE_URL=https://token-plan-cn.xiaomimimo.com/anthropic \
  NUWAX_WIRE_API=anthropic \
  NUWAX_MODEL=mimo-v2.6-flash \
  NUWAX_API_KEY="${MIMO_API_KEY:?MIMO_API_KEY is required}" \
  NUWAX_MAX_OUTPUT_TOKENS=4096 \
  CODEX_AUTO_COMPACT_RATIO=0.8 \
  codex exec --json --sandbox workspace-write '处理 worker A 的任务' \
  > /var/lib/codex/mimo-worker/exec.jsonl \
  2> /var/lib/codex/mimo-worker/exec.stderr.log &
mimo_pid=$!

env -u CODEX_SQLITE_HOME CODEX_HOME=/var/lib/codex/glm-worker \
  NUWAX_BASE_URL=https://open.bigmodel.cn/api/coding/paas/v4 \
  NUWAX_WIRE_API=chat \
  NUWAX_MODEL=GLM-5.3-Flash \
  NUWAX_API_KEY="${GLM_API_KEY:?GLM_API_KEY is required}" \
  NUWAX_MAX_OUTPUT_TOKENS=4096 \
  CODEX_AUTO_COMPACT_RATIO=0.8 \
  codex exec --json --sandbox workspace-write '处理 worker B 的任务' \
  > /var/lib/codex/glm-worker/exec.jsonl \
  2> /var/lib/codex/glm-worker/exec.stderr.log &
glm_pid=$!

wait "$mimo_pid"
mimo_status=$?
wait "$glm_pid"
glm_status=$?
printf 'mimo=%s glm=%s\n' "$mimo_status" "$glm_status"
```

这是两个独立进程的配置示例，不是容器生产 supervisor。实际部署还应由进程管理器接收退出码、转发信号并分别保存日志。
如果任务会修改仓库，另给每个 worker 独立 cwd/worktree；`CODEX_HOME` 只隔离 Codex 状态，不隔离任务工作目录。
不要继承指向公共位置的 `CODEX_SQLITE_HOME` 或 `sqlite_home` 配置；不需要单独放数据库时，保持两者未设置，让 SQLite 随各自 home。

### Responses 示例

Responses 已接入同一输出 cap 环境变量：

```bash
mkdir -p /var/lib/codex/mimo-responses
env -u CODEX_SQLITE_HOME \
  CODEX_HOME=/var/lib/codex/mimo-responses \
  NUWAX_BASE_URL=https://token-plan-cn.xiaomimimo.com/v1 \
  NUWAX_WIRE_API=responses \
  NUWAX_MAX_OUTPUT_TOKENS=4096 \
  NUWAX_MODEL=mimo-v2.6-flash \
  NUWAX_API_KEY="${MIMO_API_KEY:?MIMO_API_KEY is required}" \
  codex exec --json --sandbox workspace-write '处理任务'
```

GLM Anthropic base 为 `https://open.bigmodel.cn/api/anthropic`；GLM Responses base 为
`https://open.bigmodel.cn/api/v1`。协议能力和套餐计费需与对应 endpoint 一起确认。
不要把本地 `CODEX_MODEL_CONTEXT_WINDOW` 随意设成大数；按实际模型窗口配置，否则本地预算不能防止厂商 context overflow。

### 诊断和高级配置

在**相同 env / home / cwd** 下运行 `codex doctor --json`，检查 `config.model_routing`。
需要自定义 headers、重试、超时等参数时，使用各 worker home 下的**命名 provider** 配置，凭据通过 `env_key` / `env_http_headers` 引用环境：

```toml
[model_providers.worker_vendor]
name = "worker vendor"
base_url = "https://vendor.example/v1"
wire_api = "chat"
env_key = "NUWAX_WORKER_API_KEY"
max_output_tokens = 4096
request_max_retries = 1
stream_max_retries = 2
stream_idle_timeout_ms = 120000
```

然后传 `-c 'model_provider="worker_vendor"' -m '实际模型名称'`，由环境注入 `NUWAX_WORKER_API_KEY`。
上例 URL/模型是占位。明确选择该 provider 后 NUWAX group 不参与选择。
示例凭据变量采用 `NUWAX_` 前缀，受到 home `.env` 的前缀过滤保护；普通自定义变量不在此保护范围，当前 dotenv 可覆盖同名非保护变量。
可以复制统一模板到各 home；不要把多个可写 `config.toml` 都软链接到同一个文件。
Rig 已采用 `request_max_retries`，可在命名 provider 中设置；它和 `stream_max_retries` 分别计数，组合预算与验收见请求控制文档。

## 4. 进程、daemon 与会话边界

- `exec` 使用进程内 app-server；完整 NUWAX provider 组的 TUI 排除隐式共享 daemon。
- **本轮新增远程 guard**：本地 TUI `--remote` 与完整 NUWAX provider 组同时使用时直接报错。客户端环境不能配置远程 host 的 provider/key；应在实际运行 app-server 的进程上设置。仅 `NUWAX_MODEL` 仍可选远程服务已有模型。
- standalone 服务可运行 `codex app-server --listen stdio://`；每个服务实例独立 env/home。该入口没有 `--profile` 支持，需要 home 配置或 RPC 显式配置。
- `exec resume/fork` 和 TUI 当前配置 override 路径可以选择当前模型/provider。**裸 `thread/resume` RPC 不带覆盖时仍按已有会话元数据恢复**；不能承诺仅改服务启动环境就切换每个旧会话。
- 外部 RPC 客户端要明确切换旧会话，应在 `thread/resume` params 显式传 `model` 与 `modelProvider`；`config` 内配置键仍是 snake_case。
- **本轮修复**：只传 `config.model_provider` 的 resume 请求也会被识别为显式模型路由覆盖，避免新 provider 配旧会话的 model。
- 同 home 的多个 vendor 临时 provider ID 都叫 `nuwax_env`，`resume --last` 可能选中另一个 vendor 的最近会话。使用独立 home 和显式 thread ID。
- 同一 thread 的 writer 有锁，冲突 fail-fast；这不代表跨进程配置写有锁。`config.toml` 原子 rename 仍可能发生读改写丢更新。
- 同容器、同 PID namespace 的独立进程与不同容器共享整个 home 是不同场景。当前 PID record 不记录 namespace，**不支持把包含 daemon runtime 的同一 home 跨 PID namespace 共用**；这是源码发现，未做 Docker 复现。

相关源码：`tui/src/daemon_startup.rs`、`tui/src/startup_orchestration.rs`、
`app-server-daemon/src/backend/pid_start.rs`、`exec/src/lib.rs`、
`app-server/src/request_processors/thread_processor.rs`、`core/src/config/edit.rs`、
`rollout/src/writer_lock.rs`、`app-server-daemon/src/backend/pid_identity.rs`。

## 5. 本轮直接修复

1. 增加 `NUWAX_MAX_OUTPUT_TOKENS`：Chat/Anthropic 可配置实际输出 cap，沿用 Rig 对模型的字段转换；非法、缺组时提前报错；后续阶段已开放 Responses cap。
2. 保留 provider 隔离校验仅允许种子中的新 cap；文件与 CLI 对保留表的污染仍拒绝。
3. daemon 启动时清除新 cap，防止首个客户端值成为共享后台服务默认值。
4. TUI remote + active NUWAX provider fail-fast，防止本地选择与远端实际凭据/endpoint 不一致。
5. app-server resume 识别 `config.model_provider` 显式覆盖，补出站模型回归。

新增/扩展测试分别覆盖 parser、隔离层、daemon 子进程、TUI snapshot、resume 实际 HTTP，以及 Exec env→三种真实 HTTP wire。
本轮没有提交、推送或改动依赖版本；当前工作树还包含上一轮开发变更。

## 6. 功能遗漏与后续优先级

| 优先级 | 缺口 | 实施与验收要求 |
|---|---|---|
| P1（本轮相关验证通过） | Rig transport 已采用 `api_provider.retry` | 建立 SSE 前 HTTP 重试合同；明确与 Core `stream_max_retries` 的总预算、429/5xx/transport、Retry-After、取消；每次尝试采集且不重放已消费 SSE；HTTP 与 sampling 分别计数，最坏组合上限见 spec |
| P1（本轮相关验证通过） | Responses HTTP/WS typed request 已加入 `max_output_tokens`，与预算及 env 同步 | 在类型层贯穿 HTTP/WS/预算及 Rig passthrough，保持 RawValue/原始序列化保真；三路 wire、Core/Exec、真实MiMo/GLM请求通过；当前 Responses 环境 cap 已开放 |
| P2 | 临时 provider 的超时、重试、自定义 headers、query、bridge/replay 等没有直接 env 开关 | 优先定义有限、类型化参数合同；不能任意合并保留表。需要时先使用命名 provider + 环境凭据 |
| P2 | 共用 home 的 `--last` 路由区分与 `config.toml` 跨进程更新 | 如确实要求共享 home，再按完整来源身份过滤最近会话，并实现跨进程文件锁/版本原子比较；当前用独立 home |
| P2 | 跨容器 namespace 的 daemon PID/socket 所有权 | 如果要跨容器共享状态，先分离 runtime 或记录 namespace；遇到外部所有者不得 cleanup/发信号，补 Linux 容器集成验证 |
| P2 | 裸 RPC 旧会话重新指定模型的默认语义 | 当前显式请求覆盖优先、无覆盖保留旧元数据；如要环境自动重定向，应先定义 opt-in 行为及来源投影回归 |

这份表是本轮查到的明确缺口，不是整个官方 Codex + 所有厂商能力的完备清单。
厂商搜索、citation、reasoning、严格 schema、多模态仍需按实际协议与模型分别验收，不能从环境解析成功推导其能力完整。

## 7. 本轮验证

隔离目标目录 `/tmp/codex-stability-20260930-target`，`--offline --locked`，测试使用仓库 `just test` 入口（8 MiB `RUST_MIN_STACK`），串行调度、`--retries 0`。
测试执行需要本机权限以启动 localhost HTTP mock/子进程；没有修改 sandbox 环境变量相关代码。

| 验证 | 实际结果 |
|---|---|
| 当前源代码 fresh `codex-exec` build | exit 0，12m40s |
| utils-cli：NUWAX parser、effort、context/compact | 30/30 PASS |
| config：保留 provider 来源隔离 | 10/10 PASS |
| TUI：remote guard、daemon 选择、snapshot | 14/14 PASS |
| daemon：真实 detached 子进程环境清理 | 2/2 PASS；测试内部调用隔离 helper，不是只检查静态列表 |
| app-server：config-only provider resume | 1/1 PASS；已保存旧选择、恢复响应、新 endpoint 的实际 model 请求均断言 |
| Exec：三协议、输出 cap、非法配置、并发进程 | 8/8 PASS；并发例分别断言 path/auth/model/output cap 和配置全文不变 |
| 合计 | **65 selected / 65 executed / 65 PASS**，0 failed；其余 8098 为过滤未执行，不是通过 |

最终测试日志：`/tmp/codex-oct03-container-env-final-tests.log`，测试阶段 97.487s，命令 exit 0。
Fresh build 日志：`/tmp/codex-oct03-container-env-exec-build.log`。
首轮 55 项中的两项新增 fixture 构造错误（层顺序、假定非持久化事件）已修正；上述最终 65 项包含重新执行的对应回归。

复验 Exec 范围（先从当前树构建二进制，再指定路径，避免拾取旧产物）：

```bash
cd codex-rs
CARGO_TARGET_DIR=/tmp/codex-stability-20260930-target \
  cargo build -p codex-exec --bin codex-exec --offline --locked
env CARGO_BIN_EXE_codex-exec=/tmp/codex-stability-20260930-target/debug/codex-exec \
  CARGO_TARGET_DIR=/tmp/codex-stability-20260930-target \
  just test -p codex-exec --test all -E 'test(nuwax_env)' \
  --offline --locked --retries 0 --test-threads 1
```

最终 `just fix -p codex-utils-cli -p codex-config -p codex-tui -p codex-app-server-daemon -p codex-app-server -p codex-exec -p codex-cli --offline --locked` exit 0（19m38s），无 warning/error/自动修复记录。
`just fmt` exit 0；与 lint 前快照核对，7 个 Rust 文件仅有格式变化，无语义修改；`git diff --check` exit 0。
收尾日志为 `/tmp/codex-oct03-container-env-fix.log` / `/tmp/codex-oct03-container-env-fmt.log`。
按 AGENTS.md，测试安排在最终 fix/fmt 之前，格式化后没有重跑测试；fresh build 与上述测试对应的是格式化前的源内容。
未运行完整 workspace、远程 CI、Linux Docker 或 Windows 矩阵；本轮本地 HTTP mock 验证与上一轮真实厂商验证分别记录。
