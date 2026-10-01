# Codex fork 协议稳定性与环境变量启动开发交接

本文交给 Claude Code 执行开发与测试。目标是修复当前已经定位的问题，并让用户通过进程环境变量稳定启动使用 OpenAI Responses、OpenAI Chat Completions 或 Anthropic Messages 的模型会话。

**本轮执行范围为阶段 A 和 B，以及验证它们所需的测试和 CI 基础。** 阶段 C、D、E 是后续路线图，不在本轮同时展开。完成 A 后继续 B；每个批次独立验证和记录，最终如实报告完成范围。

日期：2026-09-30。审查基线：分支 `test`，HEAD `b9200b1fe`；与上次交付 `9113bff63` 相比，15 个提交、51 个文件。开始工作时重新检查 HEAD 和工作树；若已有并行修改，保留并适配，不覆盖或回退。

本文件将 Spec、Plan、Tasks 分为三个层级。Spec 规定目标与边界；Plan 规定实现归属和批次；Tasks 记录行动与证据。2026-09-30 的审查为静态读码、官方协议文档核对和既有日志检查，**没有重新运行测试**。本文中的旧测试数字不能作为新实现的通过证据。

## 一 Spec 功能范围与验收契约

### 1 保持已有协议架构

- `wire_api = "responses"` 的 Rig 路径使用 Rig OpenAI Responses 客户端发送 `/responses`；不得改回 Responses→Chat 转换。
- `wire_api = "chat"` 明确使用 Chat Completions；`wire_api = "anthropic"` 明确使用 Messages。URL 不覆盖显式协议选择。
- 第一方 OpenAI 和 Bedrock 的既有 native 路径保留。第三方默认 Rig；GenAI 保留为已有备用配置，不作为本轮扩展主体。
- 保持锁定的 `rig-core = "=0.42.0"`。本轮不升级 SDK，不引入本地 Rig path 依赖。
- Responses 请求继续采用类型层复制和有限投影，保留 `Arc<RawValue>`、字段顺序及大整数；禁止全量 JSON Value 往返。
- 历史存储保持追加语义。请求投影不修改原始 rollout，不伪造搜索结果、签名或密文。
- 错误后不得产生成功 Completed；取消、EOF、超时和终止错误必须有明确状态。不要增加会与 Core 重试相乘的重试层。

### 2 阶段 A 必须处理的审查发现

以下坐标以审查 HEAD 为准。执行前确认符号与调用链；只有源码或复现证据才能推翻某项结论，并将证据写入 Tasks。

| 编号 | 级别 | 问题和触发条件 | 源码入口 |
|---|---|---|---|
| F01 | P1 | Anthropic `web_search` 翻译只留下 type/name，丢掉 cached/indexed、域名白名单、地点等约束；用户选择可能被静默放宽 | `codex-rs/codex-rust-rig-bridge/src/hosted_tools.rs:29`；`core/src/tools/hosted_spec.rs:15` |
| F02 | P2 | 只设置 `model_auto_compact_ratio` 时，`ModelInfoOverrides` 不保存 ratio，正常 turn 从第一轮起就丢配置；resume、切模型同样受影响 | `codex-rs/core/src/session/step_settings.rs:182,208`；`session/turn_context.rs:1171` |
| F03 | P2 | Config 在模型最大窗口裁剪前把 ratio 转为 absolute，后续模型层因此不再按有效窗口计算 | `codex-rs/core/src/config/mod.rs:1662`；`models-manager/src/model_info.rs:20` |
| F04 | P2 | 新增的三个 context env seed 不在 daemon 准入表及 thread 配置投影中，阻止复用；已有 daemon 时 queue 可能报错。只加白名单会变成静默丢配置 | `codex-rs/tui/src/daemon_startup.rs:74`；`app_server_session.rs:1795`；`session_queue_commands.rs:41` |
| F05 | P2 | 共享 daemon/updater 启动只清除 effort，三个新 env 会被继承并固化为其他客户端的后台默认 | `codex-rs/app-server-daemon/src/backend/pid_start.rs:91` |
| F06 | P2 | 只有 hosted 工具时，Rig 因 typed tools 为空删除 tool_choice；transport 后置加入工具却未恢复选择及禁并行设置 | `codex-rs/codex-rust-rig-bridge/src/transport.rs:140,191` |
| F07 | P2 | 双轮搜索 live 仅断言非空回答，不能证明搜索执行或第二轮请求形态 | `codex-rs/live-tests/src/lib.rs:938,957` |
| F08 | P2 | 搜索工件固定目录且无 manifest，会覆盖或被 index-logs 忽略；二进制 mtime 与当前工作树 HEAD 不能证明构建来源 | `codex-rs/live-tests/src/lib.rs:469,917,1271`；`src/bin/index-logs.rs:48` |
| F09 | P2 | 全量报告把全部失败归为 V8/负载，证据不足；少量基线对照不能证明整个失败集合一致 | `my-docs/claude-rig-full-validation.md:27` |
| F10 | 验证缺口 | 没有 Core 全量 Linux 工作流依据；默认 live 未配置 MiMo Responses URL；运行时 return 被计为 PASS | `.github/workflows/live-tests.yml:47`；`my-docs/claude-rig-full-validation.md:48` |
| F11 | 文档 | 审查提示词仍声称 Chat 按 URL 选 Anthropic、Responses 经桥转 Chat；live 注释也有旧描述 | `my-docs/codex-review-prompt.md:53,56`；`codex-rs/live-tests/src/lib.rs:217` |
| F12 | 维护 | live-tests 的 lib.rs 从 1091 增至 1362 行，继续混合执行、场景和证据管理不利于扩展 | `codex-rs/live-tests/src/lib.rs:701,903` |

F06 的范围是桥公开请求接口；当前 Core 在 `core/src/client.rs:1028` 固定 `tool_choice="auto"`，不要将它表述为所有普通 CLI 调用都会发生的问题。

F03 的具体回归例：配置窗口 200000、ratio=0.5，模型 max_context_window=60000，应先得到有效窗口 60000，再得阈值 30000。当前先得 100000，随后默认上限可能将其限制为 54000，语义错误。

### 3 搜索约束修复契约

- 对 Anthropic 可表达的字段正确转换，至少包括 `filters.allowed_domains`→`allowed_domains`、`user_location`，并核对官方输入类型和厂商能力。
- cached、indexed 以及其他无法等价表达的模式必须显式处理。本轮默认采用请求前的清晰错误，不得悄悄改成 live 搜索；错误应说明用户可选择已支持的 live 模式。
- 不因 hosted 工具后置注入而改变 `none`、`required`、指定工具或禁并行的有效含义。组合不受目标协议支持时明确报错，不默默转 auto。
- 搜索 disabled 必须不发送搜索工具。函数工具与 hosted 工具混合时保持二者约束。
- 本轮保留已经声明的“搜索历史项成对丢弃”边界，不合成空结果，不顺便设计完整结果持久化。

### 4 压缩与 daemon 修复契约

- 显式 absolute limit 与 ratio 是两种独立配置；继续保持 absolute 优先，包括 absolute 来自配置文件、ratio 来自 env 的情况，并在文档中举例。
- ratio 保留至实际模型窗口确定后，由模型元数据层统一派生；Core 和 CLI 不各自复制阈值算法。
- 新建、首轮、后续 turn、resume、fork 和切模型都必须保留同一配置语义。
- 检查零值、负数、NaN、无穷、极小比例导致取整为零等边界，避免合法输入组合产生每轮压缩。非法配置应有可诊断结果。
- 三个 context 配置按 thread/start、resume、fork 传递；daemon 准入、参数投影、共享后台 env 清理必须配套修改。
- 后台 AppServer 和 updater 均不得继承前台的四个模型 seed。直接执行 standalone `app-server` 时读取调用者 env 的现有语义保留。
- 两个客户端同时连接同一个 daemon，使用不同窗口/ratio/effort 时不得相互污染。

### 5 阶段 B 环境变量启动契约

本节是**待实现设计**，这些新增变量在审查 HEAD 尚不可用。采用 `NUWAX_` 命名，避免误用现有 `CODEX_API_KEY` 的认证含义，或把 ACP-TS 的变量当作 Rust CLI 已支持的入口。

| 变量 | 语义 |
|---|---|
| `NUWAX_MODEL` | 本次启动选用的模型，映射既有 model 配置 |
| `NUWAX_BASE_URL` | 临时自定义 provider 的绝对 HTTP/HTTPS Base URL |
| `NUWAX_WIRE_API` | 严格枚举 `responses`、`chat`、`anthropic` |
| `NUWAX_API_KEY` | 临时 provider 的凭据，由既有 auth/header 管线处理 |
| `CODEX_MODEL_REASONING_EFFORT` | 保留已有名称和兼容语义 |
| `CODEX_MODEL_CONTEXT_WINDOW` | 保留已有名称，配合阶段 A 修复 |
| `CODEX_AUTO_COMPACT_TOKEN_LIMIT` | 保留已有名称，绝对阈值优先 |
| `CODEX_AUTO_COMPACT_RATIO` | 保留已有名称，按有效模型窗口计算 |
| `CODEX_HOME` | 保持已有显式目录语义；不自动迁移或混合旧目录 |

#### 5.1 临时 provider 模式

1. BASE_URL、WIRE_API、API_KEY 是完整的一组配置：三者均未设置时不激活；只设置部分时 fail fast。MODEL 可以单独用于选择已有 provider 的模型；激活临时 provider 时，必须从 NUWAX_MODEL 或显式 CLI 模型参数得到明确模型。
2. 为该组创建仅本次运行有效的 provider，建议保留 ID `nuwax_env`。它是自定义 provider，默认走 Rig；不按显示名称或 URL 冒充第一方。
3. 配置只保存 `env_key="NUWAX_API_KEY"` 等凭据引用；不把真实 key 写入 `-c`、config.toml、rollout、manifest 或诊断输出。
4. 若已有配置定义同名保留 ID，明确报告冲突；不要混入旧 provider 的 bearer、OAuth、AWS 或其他认证字段。
5. 无鉴权本地服务继续使用既有 provider 配置；本轮临时 provider 模式要求 key，不扩展新的无鉴权开关。
6. 原有配置文件、profile 和 `-c model_providers...` 继续有效；不引入另一套 provider 存储。

#### 5.2 优先级与输入校验

- 在管理策略允许范围内：显式 CLI 模型参数及 `-c` 高于新 env；env 高于已选 profile 与普通配置中的对应值；其余配置层沿用现有顺序。
- 显式 CLI 选择另一个 provider 时，忽略整组临时 provider env，包括凭据；不得只覆盖 endpoint 却继续使用另一 provider 的 key。显式选择 `nuwax_env` 时使用该完整组。
- CLI 明确覆盖的模型值不再由 env 覆盖。未采用的 env 值不能因单独校验而使有效的 CLI 配置失败。
- 对最终采用的字段检查空白、非 Unicode、未知协议、无效 URL 和缺失凭据；错误只包含字段名及安全上下文，不包含 key。
- 这些变量来自**真实进程环境**。自动读取 `$CODEX_HOME/.env` 时不允许注入或覆盖上述 NUWAX 启动变量；保留现有 CODEX 前缀过滤和其他 dotenv 兼容行为。不要扩大 dotenv 的可信范围。
- 程序读取、解析一次得到明确的有效配置；各模块不反复读取进程 env，更不在多线程运行期改全局 env。

#### 5.3 启动入口和后台边界

| 入口 | 本轮必须支持的行为 |
|---|---|
| CLI、TUI、exec | 相同 env 得到相同模型、协议和请求；TUI 临时 provider 可使用嵌入式后端 |
| 直接 standalone app-server | 从其启动进程 env 解析，作用域为该实例；客户端显式 thread 参数遵循现有优先级 |
| 共享 daemon | 阶段 A 的非秘密模型参数逐 thread 传递，不能污染全局默认 |
| 临时 provider 与共享 daemon | 若既有安全凭据通道能支持，复用它并验证隔离；否则明确使用嵌入式后端，诊断说明原因，不往 thread 配置塞明文 key |
| queue 等必须依赖共享服务的操作 | 对未支持的临时 provider 组合明确报能力错误；普通已配置 provider 配合四个模型 seed 必须正常复用 daemon |

完整的“任意前台临时凭据跨共享 daemon 传递”不列为本轮必须开发的新系统。不得为了入口一致而引入秘密落盘或首客户端污染；需要新凭据通道时登记为后续设计。

扩展现有 `doctor --json`，展示有效 model、provider ID、wire、bridge、脱敏 endpoint、各配置来源、压缩阈值和后端选择原因。保留已有 JSON 字段兼容性，不新建重复诊断命令。

实现后的使用示例，当前版本不可直接使用：

```sh
mkdir -p "$HOME/.codex-nuwax"
export CODEX_HOME="$HOME/.codex-nuwax"
export NUWAX_MODEL="YOUR_MODEL"
export NUWAX_BASE_URL="https://YOUR_PROVIDER_BASE_URL"
export NUWAX_WIRE_API="anthropic"
export NUWAX_API_KEY="$YOUR_PROVIDER_API_KEY"
export CODEX_AUTO_COMPACT_RATIO="0.8"
nuwax-codex
```

分别将协议设为 chat、responses，并使用对应正确 Base URL，即构成其余两种协议的启动场景。Base URL 和模型由实际厂商配置提供，不用 URL 推断协议。

### 6 本轮排除的功能

- 通用双向历史投影、跨厂商密文兼容和存储迁移。
- Anthropic 搜索结果与引用完整持久化、pause_turn 自动续接。
- Prompt caching、连接池、通用能力注册系统、额外厂商 SDK 接入。
- 重写 Core 主流程、去掉 GenAI、改变第一方 native/Bedrock 行为。
- push、创建 PR、发布 npm、改正式凭据或全局用户配置。

这些项目在后续路线图中有位置。本轮修复和测试可以暴露其边界，但不能据此宣称已完成。

## 二 Plan 实施归属与批次

### 7 代码归属

优先复用现有 crate 和类型。新 env 解析放在 `utils/cli` 的独立私有模块；沿既有配置层传递，避免继续扩大 `config_override.rs`。provider 的有效配置和认证继续由现有 provider 管线处理。压缩 ratio 的派生归属 `models-manager`，Core 负责传递。

hosted 请求/响应适配继续位于 rig bridge。测试执行器、产物清单和 live 场景从 `live-tests/src/lib.rs` 提取到私有模块，只重导出实际集成测试使用的入口。不要新增一个包办所有功能的管理器。

遵守 SOLID、Fail Fast 和仓库 AGENTS.md。生产路径不新增 unwrap/expect 或 unsafe；不修改禁止触碰的 sandbox 环境变量机制。测试优先比较完整对象和真实出站字节。

### 8 分批实施顺序

| 批次 | 实施内容 | 必需验收 |
|---|---|---|
| A1 | F01、F06：搜索约束转换和 hosted-only 工具选择 | wire 原始请求断言；cached/indexed 明确拒绝；disabled、域名、地点、混合工具、none/required/指定工具/并行控制矩阵 |
| A2 | F02、F03：ratio 全链传递与单处派生 | Config→有效窗口→实际首轮和恢复后的模型阈值；切模型；absolute 优先；数值边界 |
| A3 | F04、F05：daemon 准入、thread 参数、后台 env 隔离 | queue 复用；start/resume/fork 请求；两个客户端不同参数；AppServer/updater 子进程隔离 |
| A4a | F12：机械提取 live 执行和工件模块 | 对照提取前后代码，行为保持；与逻辑修复分开 |
| A4b | F07、F08：搜索断言、唯一工件目录、manifest、构建来源 | 搜索缺失必须失败；重试不覆盖；index 可收录；过期或不明来源二进制不能生成验收成功 |
| A4c | F09、F10、F11：纠正文档与最小 CI 保障 | 已知失败逐项登记；有效执行与跳过分开；配置必需项缺失失败；修正路由说明 |
| B1 | 新 env 解析、配置优先级、临时 provider 构造 | 三协议配置矩阵，冲突/非法/非 Unicode/缺值，认证不混用，不写真实 key |
| B2 | 各入口接线与 doctor 信息 | CLI/TUI/exec/standalone app-server 一致；共享 daemon 作用域符合 Spec，旧启动方式不回归 |
| B3 | 阶段 A/B 最终集成与真实请求验证 | 同一构建产物、三协议本地 HTTP、MiMo/GLM 最小 live、准确汇总和剩余边界 |

阶段 A 存在未处理的行为回归时先修复，再进入 B。维护性提取不与协议或配置修复混在一个大 diff。复杂逻辑批次尽量小于 500 changed lines，普通非机械批次不超过 800；必要时按依赖拆分，保持每一批可构建。

本提示不要求自动 commit。先形成可审查的分批变更及证据；仅在用户明确授权时按文件窄范围提交。已有用户对提交的直接授权仍有效，但不包含 push 或发布授权。

### 9 测试分层

| 层 | 重点 | 判定方式 |
|---|---|---|
| 配置单元与子进程 | env/CLI/profile/home、非法输入、父子进程隔离 | 可重复，避免并发修改进程全局 env |
| 三协议 HTTP loopback | headers/body、RawValue、auth、字段映射、SSE 分片、终止与错误 | 比较真实出站请求和事件；不只比较模型文本 |
| Core 集成 | 超窗→trim→继续、工具后压缩、ratio-only、resume/fork、切模型、daemon 参数 | 使用实际配置加载和 session，不只直接构造底层 config 对象 |
| 二进制 | CLI/TUI/exec/app-server、临时 HOME、env-only 启动、npm 后端选择 | 绑定明确的构建和运行产物 |
| 真实厂商 | 现有 MiMo/GLM 配置能支持的三种 wire | 文本、工具闭环、reasoning、搜索和续轮按能力分别验收 |
| CI 与平台 | PR 离线回归、相关 feature 组合、三平台启动、nightly live | 工作流实际运行结果与只新增 YAML 分别记录 |

网络故障矩阵至少保留：401 启动错误、429/Retry-After、5xx、响应头前超时、流中错误、EOF、idle timeout、取消。验证失败后不产生 Completed，工具不会因错误处理而重复执行。

现有 Chat/Anthropic cassette 多从 Rig 中间事件进入转换器，不能当作完整 HTTP 覆盖。阶段 A/B 新增或修改的行为必须有对应 loopback 证据；全面补齐其余 cassette 在阶段 C 实施。

真实搜索测试必须要求实际搜索事件。若场景要求两轮都搜索，两轮均应出现完成的搜索事件；若只验“第一轮搜索后可续聊”，明确改名并只宣称该范围。历史出站形态由 wire 捕获证明，不能根据提示词或回答反推。

### 10 测试命令与执行纪律

阅读当前 AGENTS.md 和 justfile。使用足够空间的隔离 target，并记录路径；不要删除已有 target 或运行中的构建产物。以下示例从 `codex-rs` 执行，依赖已缓存时可附加 `--offline`。

```sh
export CARGO_TARGET_DIR=/tmp/codex-stability-20260930-target

# 协议修改相关包
just test -p codex-rust-rig-bridge -p codex-api --retries 0

# 配置修改相关包
just test -p codex-config -p codex-models-manager -p codex-utils-cli --retries 0

# 既有 Core 桥和压缩场景；新增回归必须另加入实际选择列表
just test -p codex-core --features rust-rig --test all \
  -E 'test(rig_responses_bridge) | test(compact)' --retries 0

# 无桥 feature 的明确失败行为
just test -p codex-core --no-default-features --lib \
  -E 'test(missing_bridge_features_reject_before_native_responses_dispatch)' --retries 0

# 重建 live 使用的实际二进制
cargo build --locked -p codex-exec --bin codex-exec
```

- daemon、TUI、home-dir、CLI、app-server 等受影响包也必须运行对应回归。实施者在 Tasks 写出精确命令和新增测试名，不能只运行上面的示例便称覆盖完整。
- 测试使用 `just test`；禁止直接 cargo test 或用裸 nextest 绕过仓库栈大小等默认值。列表查询可使用 nextest list；选择结果必须非零且包含新回归。
- 不默认使用 `--all-features`。分别验证需要的 `rust-rig`、无桥以及受影响的已有备用桥编译/契约。
- 完整 workspace 的 `just test` 依 AGENTS.md 单独确认；不因本文件而假定已授权。相关包及定向回归可先完成。
- 新增静态 include 文件、依赖或配置字段时，同步处理对应 Bazel 数据、锁文件或 schema。配置改变后运行 `just write-config-schema`。
- 完成测试后执行受影响包的 scoped `just fix -p ...`、`just fmt` 和 `git diff --check`。按 AGENTS.md 不在 fix/fmt 后重跑测试；报告准确记录验证顺序与自动修改内容。若 lint 引入语义变化，不得仍声称其已被先前测试验证，应明确记录并按仓库规则安排独立验证批次。
- 不杀 Rust 进程规避锁等待；不要把构建等待当测试超时。

### 11 真实请求和证据规则

- 仅使用已有 gitignored 凭据和用户允许的厂商；不把 key 打印到聊天、日志、fixtures 或文档。不新增购买、账户配置或付费服务。
- 先跑无网络回归，再用最小场景验证改动。MiMo/GLM 为当前优先；Step、官方 OpenAI 和官方 Anthropic 缺少配置或额度时，登记未验证，不冒充通过。
- provider 与 protocol 是两层维度：通过某兼容网关不能证明官方厂商或其他网关全部兼容。
- 必需用例在执行前校验所需 endpoint/model/凭据来源。非支持协议明确标为 unsupported；配置缺失标为 not-run。不得在必需场景中 return Ok 后计作有效通过。
- 区分 registered、selected、executed、asserted、skipped、failed、timed_out、retried。若框架无法直接提供 asserted，使用场景执行清单或显式完成标记，不解析一个总 PASS 数代替。
- 每次运行和每次重试有唯一目录；失败也保留工件。manifest 记录 source SHA、dirty 状态、构建 feature、平台、实际二进制 SHA256、场景及模型/协议，不记录凭据。
- 通过构建 receipt 把源版本、features 和二进制 hash 绑定，或使用等价可核验机制。运行时的 git HEAD 加 mtime 不足以证明构建来源。
- 所有子进程使用被记录的同一个绝对二进制路径。stdout/stderr/退出码/请求数量/关键事件与 manifest 一起索引。
- 重试后通过必须单列 flaky，不能覆盖第一次失败记录。故障分类需有证据，不能统一归为网络、V8 或负载。

### 12 既有验证证据的正确解释

2026-09-29 日志曾记录以下结果；本轮开始后必须重新生成对应新实现的证据。

| 旧证据 | 可确认的范围 | 不能据此宣称 |
|---|---|---|
| `/tmp/val-bridge.log` | 750 executed：749 pass、1 timeout，另 1 skipped | 桥相关全绿 |
| `/tmp/val-core-rigrig.log` | 4712 executed：4479 pass、225 fail、8 timeout，另 27 skipped | 全部失败均为环境原因 |
| `/tmp/nuwax-batch-baseline.log` | 56 项选择集的基线；部分 registry/Azure 断言也失败 | 整个 4712 项集合的完整基线对照 |
| GLM websearch 工件 | 第一轮存在搜索，第二轮文本续聊完成 | 双轮均完成搜索 |
| compact 工件 | 有 7 个 provider/protocol 组合的两轮完成记录，manifest 标记旧 SHA | 当前 HEAD 全矩阵通过 |

已有失败要列明准确测试名、失败现象、基线是否复现、原因证据和状态。预存在缺陷与本批回归分开，但都不能计入绿色验收。`CI Linux 覆盖其余` 必须给出真实 workflow/run 证据。

### 13 后续路线图

| 阶段 | 范围 | 开始与完成条件 |
|---|---|---|
| C 协议契约完善 | 字段矩阵、输出预算可配置、structured output、工具和 reasoning/usage、完整三协议 HTTP replay | A/B 完成；每项能力有字段策略和正反例；未支持内容明确反馈 |
| D 会话与 hosted 完整流程 | provenance 参与投影、搜索真实结果/引用、原始块顺序、pause_turn、混合 server/client tools、保存恢复 | 先独立 Spec/Plan；历史格式和大小上限经过审查；不伪造密文或结果 |
| E 性能与发布 | HTTP 连接复用、tee 有界缓存、取消释放、长会话稳定性、三平台 npm 安装、上游同步与发布回归 | 以观测数据验收；仅定义 CI 不算已跑；发布另行授权 |

D 中新增模型可见内容须遵守 context 规则：有硬上限，单项超过 1k tokens 按仓库要求进行额外审查，不能引入无限搜索结果。tee 会随请求释放不等于内存有界，需设置容量与溢出策略。

Anthropic citations、pause_turn、搜索结果回放目前属于已经登记的缺口。A 的有限修复完成后，仍不得宣传完整多轮 hosted 支持。Prompt caching 和长输出预算也应按厂商能力单独验收。

## 三 Tasks 执行清单与结果记录

### 14 本轮清单

- [x] T00 记录起始 SHA、dirty 状态、依赖版本、适用 AGENTS；确认本文件坐标和源码一致。（起始 `b9200b1fe` 干净；F01–F12 全部坐标逐一核对属实；rig-core =0.42.0 registry 源核对）
- [x] T01 为 F01/F06 增加可复现回归，完成 A1，保留修复前失败和修复后通过证据。（见 §15 批次 A1）
- [x] T02 为 ratio-only、窗口裁剪、首轮/resume/切模型和数值边界增加回归，完成 A2。（见 §15 批次 A2）
- [x] T03 配套修复 daemon 准入、thread 参数与后台 env，完成 A3 的两个客户端隔离验证。（机制级验证见 §15 批次 A3；未建双进程专用测试）
- [x] T04 机械提取 live runner/工件/场景模块，记录行为对照，完成 A4a。（字节级 diff + 84/84 复跑）
- [x] T05 修正搜索有效执行断言、唯一工件和构建来源，完成 A4b。（离线单测 10/10；真实验证留 B3）
- [x] T06 修正文档现状、基线失败口径、live 必需配置与执行统计；增加相关离线 CI 任务，完成 A4c。（workflow 改动未运行，登记 not-run）
- [x] T07 实现本文件的 NUWAX env 契约，覆盖所有优先级/冲突/缺值/认证隔离规则，完成 B1。（12 项矩阵测试，43/43）
- [x] T08 接通 CLI/TUI/exec/standalone app-server，明确共享 daemon 限制并完善 doctor，完成 B2。（三入口接线 + 排除分支 + doctor 新检查 + 二进制层三协议 4/4）
- [x] T09 验证旧 config/profile/native/GenAI 配置兼容，生成必要 schema，检查新增文件的 Bazel 可见性。（既有 config_tests/套件全绿覆盖旧配置路径；本轮未新增 ConfigToml 字段故无需 schema 重生成；sha2 依赖变更经 `just bazel-lock-update` 核实 MODULE.bazel.lock 无漂移；无新增静态 include）
- [x] T10 重建并绑定测试二进制，跑本地三协议及最小 MiMo/GLM live，完成 B3。（见 §15 批次 B3）
- [x] T11 区分实际运行与未运行的平台、厂商和 CI；登记全部剩余失败及能力边界。（见 §15 批次 B3 与下方最终边界清单）
- [x] T12 完成 scoped fix、fmt、diff-check 后独立源码质量复查；检查日志、fixture、暂存候选中没有凭据。（fix 4 处自动修复 + fmt + diff-check；凭据扫描零命中）
- [x] T13 更新 FORK.md、审查入口和本文执行记录；交付改动摘要、精确命令、结果与下一阶段建议。（FORK.md 增补本轮差异；摘要见最终回复）

清单只有在对应实现和证据齐全时勾选。缺少凭据、平台或构建依赖的测试标记 not-run，不补填通过。全部 T 项完成仅代表 A/B 的约定范围完成，不代表 C/D/E 完成。

### 15 每批记录模板

在本节后追加记录，保留失败经过；不要改写历史数字使其看起来全绿。

```text
批次：
对应发现或需求：
开始与结束 source SHA，dirty 状态：
改动文件与行为：
新增和修改的回归测试名：
修复前失败的命令、现象和退出码：
修复后验证的完整命令、cwd、非秘密环境开关：
target 目录、构建 features、binary 绝对路径与 SHA256：
selected / executed / asserted / skipped / failed / timed_out / retried：
有效厂商、协议、模型、请求数量：
原始日志、manifest、脱敏请求/回执路径：
未运行项与原因：
fix/fmt 结果、后续源码复查：
遗留项及下一步：
```

#### 批次 A1（F01+F06：搜索约束转换与 hosted-only 工具选择）

- 开始 SHA `b9200b1fe`（=审查基线，工作树干净）；结束 SHA 见 A1 提交（未 push）。
- 改动：`hosted_tools.rs`（web_search 翻译保留 `filters.allowed_domains`→`allowed_domains`、`user_location` 透传并校验 approximate/至少一字段/域名格式；cached/indexed 模式请求前显式报错；search_context_size/search_content_types 无对应字段 warn 后丢弃；新增 `translate_anthropic_server_tools` 批量入口）；`convert_request.rs`（`validate_tool_choice`：required/指定工具在无函数工具时 InvalidRequest；`anthropic_tool_choice` wire 映射 auto/none/any/tool）；`transport.rs`（body 改写抽出为纯函数 `ChatFamilyRewrite`；hosted-only 时恢复 tool_choice 并让 disable_parallel_tool_use 生效；Chat 无工具时移除悬挂 tool_choice）；`stream.rs`/`responses.rs` 接线。
- 新增回归：桥 lib 12 项（`cached_mode_is_rejected…`、`indexed_mode_is_rejected…`、`allowed_domains_and_user_location_cross…`、`malformed_domain_entries…`、`user_location_without_any_field…`、`batch_translation_fails_fast…`、`batch_translation_drops_unknown…`、`anthropic_tool_choice_maps_the_wire_shapes`、`required_or_specific_choice_without_function_tools_is_rejected`、`hosted_only_anthropic_request_translates_and_restores_choice`、transport_rewrite 6 项）；wire loopback 5 项（`hosted_web_search_constraints_reach_the_anthropic_wire`、`hosted_only_request_restores_tool_choice_and_parallel_control`、`tool_choice_none_is_restored_on_the_wire`、`cached_web_search_mode_fails_before_the_request`、`required_tool_choice_with_hosted_only_tools_fails_before_the_request`）。
- 修复前失败（stash src 后仅保留新 wire 测试）：`just test -p codex-rust-rig-bridge --offline --retries 0 -E 'test(hosted_web_search_constraints_reach)|test(hosted_only_request_restores)|test(tool_choice_none_is_restored)|test(cached_web_search_mode_fails)|test(required_tool_choice_with_hosted_only)'` → **5 run / 0 pass / 5 fail，退出码 100**（约束与 tool_choice 断言失败；cached/required 场景请求照发→空闲超时，证明旧代码会发出被放宽的请求）。日志 `/tmp/a1-prefix-repro.log`。
- 修复后：同选择器全绿；全包 `just test -p codex-rust-rig-bridge --offline --retries 0` → **135/135 pass**（基线 118，+17）；协议包组合 `just test -p codex-rust-rig-bridge -p codex-api --offline --retries 0` → **333/333 pass**。日志 `/tmp/a1-postfix-full.log`、`/tmp/a1-protocol-packages.log`。cwd `codex-rs`，`CARGO_TARGET_DIR=/tmp/codex-stability-20260930-target`，无秘密 env。
- executed=350（5+135+333 去重后全量 333+135 中 wire/lib 重叠，以日志为准），asserted=333+135 全部断言真实出站 JSON/事件；skipped=0；failed=0；timed_out=0；retried=0（--retries 0）。
- 真实厂商请求：本批未运行（A1 为离线行为修复；live 验证在 B3 统一执行，避免重复计费场景）。
- fix/fmt：`just fix -p codex-rust-rig-bridge`（1 处自动修复：transport.rs）；`just fmt` 后丢弃两份与本批无关文件的纯 rustfmt 漂移重排（`core/src/config/config_tests.rs`、`core/tests/suite/rig_responses_bridge.rs`，仅换行无语义；上一批次先例）。fix/fmt 后按仓库规则未重跑测试。
- 遗留：core 套件 `rig_responses_bridge|compact` 选择集回归在 A2 结束后与 A2 改动一起跑（两者都触及请求构建路径）；`anthropic_server_tool` 返回 `Result` 是 crate 内 API，core 不直接调用，无破坏面。

#### 批次 A2（F02+F03：ratio 全链传递与单处派生）

- 开始 `3ca07d64c`（A1 后）；改动：`Config::to_models_manager_config` 移除本层 ratio→absolute 派生（F03 根因：派生发生在 max_context_window 裁剪前，200k 窗口 + ratio 0.5 在 60k 模型上会冻结成 100000 而非 30000）；`ModelInfoOverrides` 新增 `auto_compact_ratio` 字段并在 From/models_manager_config 双向保持（F02：首轮/resume/切模型经该往返重建模型信息）；模型层 `with_config_overrides` 保留唯一派生点并新增零值下限（极小比例取整为 0 时 warn 并忽略，避免每轮压缩）。
- 新增/重写回归：`to_models_manager_config_passes_the_ratio_through_for_model_layer_derivation`（重写自旧 Config 派生测试；含 F03 200k 形状与 NaN 透传断言）、`ratio_only_compact_threshold_survives_override_resolution`（Config→overrides→切模型解析，断言有效窗口 60000→阈值 30000）、`tiny_auto_compact_ratio_deriving_zero_keeps_the_model_limit`。
- 修复前失败（stash 三个 src + 引用新字段的测试文件）：`tiny_auto_compact_ratio_deriving_zero…` → FAIL（旧代码派生出 Some(0)）；`to_models_manager_config_passes_the_ratio_through…` → FAIL（旧代码派生 Some(48000)/Some(160000)）；均退出码 100，日志 `/tmp/a2-prefix-repro.log`。
- 修复后：`just test -p codex-config -p codex-models-manager -p codex-utils-cli --offline --retries 0` → **439 run：439 pass / 1 skip**；core lib 选择集（4 项含新回归与既有 model_resolution）→ **4/4 pass**。日志 `/tmp/a2-config-packages.log`、`/tmp/a2-core-lib.log`。cwd `codex-rs`，同上隔离 target。
- 语义边界：absolute 优先（配置文件 absolute + env ratio 组合按既有 -c>env>config 顺序解析后同层绝对值优先，模型层 `if let Some(absolute)` 先行）；NaN/负数/0/1.5 在模型层统一 warn+忽略；Config 层不再有第二套阈值算法。

#### 批次 A3（F04+F05：daemon 准入、thread 投影、后台 env 隔离）

- 改动：`tui/src/daemon_startup.rs` 准入表新增 `model_context_window`/`model_auto_compact_token_limit`（正整数）与 `model_auto_compact_ratio`（(0,1] 浮点，整数 1 协变）三键；`tui/src/app_server_session.rs` 的 `config_request_overrides_from_config` 在 start/resume/fork 共享的投影中补三个数值键（置于字符串 `insert` 闭包最后使用之后，避免双借用）；`app-server-daemon/src/backend/pid_start.rs` 对 AppServer 与 updater 两类子进程统一 `env_remove` 四个模型种子（effort + 三 context）。
- queue 坐标核实：`thread/queue/add` 仅对已存在线程追加消息（参数无 config），其受影响面只是 daemon 准入（env 种子不再把客户端挤到 embedded 后端从而触发"daemon 已存在却用 embedded"报错）——白名单修复即闭环，未改 queue 本身。
- 新增/扩展回归：`audited_overrides_allow_daemon_without_allowing_arbitrary_config`（+10 用例：合法三键放行、非法值/字符串/越界拒绝）；`thread_lifecycle_params_forward_config_overrides_and_service_tier`（+三键期望值，start/resume/fork 三形态）；`detached_children_do_not_capture_client_effort` → `detached_children_do_not_capture_client_model_seeds`（子进程 shim 记录四变量，断言全部 unset，覆盖 app-server 与 updater）。
- 修复前失败：stash 后 `audited_overrides…` 与 `detached_children_do_not_capture_client_model_seeds` → **2 FAIL / 1 pass（旧行测试）/ 退出码 100**（`/tmp/a3-prefix-repro.log`）；投影坐标用"临时禁用投影块 + 保留新测试"复现 → FAIL 退出码 100（`/tmp/a3-prefix-projection.log`）。三个坐标均有行为级失败证据。
- 修复后：`just test -p codex-tui -p codex-app-server-daemon --offline --retries 0 -E 'test(audited_overrides_allow_daemon) | test(thread_lifecycle_params_forward_config_overrides) | test(detached_children_do_not_capture_client_model_seeds) | test(daemon_eligibility_preserves)'` → **4 run / 4 pass / 0 fail**（`/tmp/a3-postfix.log`）。
- 两客户端隔离的机制保证：per-thread 投影（各客户端线程请求各带各值）+ 后台 env 清除（daemon 默认不被任何客户端种子污染）；未另建双进程专用测试。

#### 批次 A4a（F12：live 执行/工件/场景机械提取）

- 改动：`live-tests/src/lib.rs`（1362 行）按层拆为 `env.rs`（vendor/endpoint/auth）、`bridge_turns.rs`（L1 桥级）、`binary_turns.rs`（L3 二进制级）、`assertions.rs`、`error_probe.rs`、`artifacts.rs`；lib.rs 保留全部原 pub 面（26 个集成测试引用项逐一再导出）+ `pub(crate)` 三处跨模块管道（persist_lines/write_manifest/prune_artifacts）。各模块 `use super::*` 复用原命名空间。
- 机械等价证据：六个模块体与原文件对应行区间 **diff 逐字节一致**（仅上述三处可见性标注差异）；`cargo check --tests --bins` 零警告。
- 行为对照：`binary(bridge_live) | config:: | cassette::` → 首轮 91 run：90 pass / 1 fail（`step_rig_parallel_tools`，并行负载下 2.3s 失败；隔离复跑 7.9s pass，整批复跑 **84/84 pass**，非本批引入，登记为既有并行 flake）。
- 未运行：exec_live 真实请求场景（需绑定构建的二进制与凭据，B3 统一执行）。

#### 批次 A4b（F07+F08：搜索断言、唯一工件、manifest、构建来源）

- 改动：`run_websearch_turns` 工件目录改为每次运行唯一（`websearch-{protocol}-{nonce}-{pid}`）并写 manifest、纳入 prune；新增 `completed_web_search_calls` rollout 解析器——turn1 后断言 ≥1 个 completed `web_search_call`、turn2 后断言计数增长（非空回答不再单独算作搜索证据，F07）；`write_manifest` 新增 `git_dirty`、`binary_path`、`binary_sha256`（sha2 流式计算，运行时 rev+mtime 不能证明构建来源，F08）；`index-logs` 表格补 dirty 与 binary sha256 前 12 位；websearch 运行自此可被索引。依赖：live-tests 增加 `sha2`（workspace 已有 0.10，仅 Cargo.lock 成员依赖表变化）。
- 新增单元测试：`completed_web_search_calls_counts_only_completed_items`、`completed_web_search_calls_sums_across_rollout_files`、`completed_web_search_calls_on_an_empty_home_is_zero`（离线可验证部分）。真实验证在 B3。
- 修复前状态（无法用测试复现的运行时证据缺口即缺陷本身）：固定目录覆盖 + 无 manifest（index-logs 不收录）+ 仅断言非空回答——已由代码对照确认，行为修复的证据在 B3 真实运行时补齐。
- A4b 离线验证：`just test -p codex-live-tests --offline --retries 0 -E 'test(binary_turns::tests) | test(config::) | test(cassette::)'` → **10/10 pass**。

#### 阶段 C/E 提前完成项（2026-10-01 第二轮）

用户指示"未完成的、可以做的，继续做"。按 §13 逐项评估后落地三件（其余受阻项见后）：

- **C：输出预算可配置**：`model_providers.*.max_output_tokens`（正整数，schema 已重生成）→ `ModelProviderInfo` → `codex_api::Provider` → 桥在 convert 后覆盖 `CompletionRequest.max_tokens`（Anthropic 覆盖默认 16384；Chat 可选携带）；Responses 直传不受影响（verbatim）；远程 thread config 尚无对应 proto 字段（转 None，注释登记）。wire 测试 3 项（两协议到达 body、无预算保持默认）+ 解析测试 2 断言。
- **E：tee 有界缓存**：`tee_wire_bytes` 上限 8 MiB，溢出停采+告警，流不受影响（块恢复只解析完整帧，截断尾不产生半块——安全降级路径分析见 transport.rs 注释）；单测断言 12 MiB 输入全部透传且缓冲触顶。**未声明 hosted 工具时 tee 整体跳过**（stream.rs 重排翻译在前，pump 对 None 已优雅处理）。
- **E：HTTP 连接复用**：`http_client` 进程级共享池（key=protocol+静态头指纹；鉴权头从不到客户端默认——按轮注入，共享安全；custom CA 路径保持逐轮构建避免对不可哈希 TLS 配置做键）。`RigProtocol` 补 derive(Hash)。单测断言同头同客户端（Arc::ptr_eq）、异头异客户端。
- **D：立项文档**：`phase-d-spec.md`（目标/非目标/数据模型硬上限/失败矩阵/验收契约）+ `phase-d-plan.md`（D1 采集→D2 回放→D3 pause_turn/混合轮→D4 上限与 provenance，含决策点与回退开关）——满足 §13 D 行"先独立 Spec/Plan"的开始条件。
- **live-tests.yml 预检**：脚本逻辑本地正反例验证（缺 RESPONSES_URL 被正确点名）；真实 dispatch 仍需 push 后进行。

验证（最终树，fix/fmt 后复跑）：桥+provider-info 全量、`max_output`/`budget`/`wire_tee`/`shared_http_client` 选择集、exec `nuwax_env` 二进制 4/4（重建二进制，鉴权穿过共享池）、真实 GLM chat 一轮（池 + 真网关 TLS）"收到" exit 0。工作区 check 0 错误。既有 flake：`responses_time_out_when_server_never_sends_headers` 单次超时（多轮在案）。

**合并遗留修正**：本轮全量跑暴露 `test_amazon_bedrock_providers_add_mantle_client_agent_header` 与 `test_create_amazon_bedrock_runtime_provider` 失败——1f9841304 冲突解决时保留的 fork 行 `http_headers = None` 与上游新 mantle 头测试冲突。已修正：保留 fork 的 runtime provider_id 钉住，恢复上游保留 mantle 头的行为（两测试期望同步）。provider-info 35/35；桥+provider-info 最终 174/175（唯一失败为 `responses_time_out_when_server_never_sends_headers` 既有竞态，隔离 0.7s 通过，2026-09-28 起多轮在案）。

**仍受阻/未做**：push+CI dispatch、发版（未授权）；D 实施本体（Spec/Plan 待评审）；MiMo/Step 的 NUWAX live 全矩阵与 pause_turn 真实验证（依赖 D3 或网关支持）；E 剩余项（取消释放、长会话稳定性、三平台安装回归——需发布周期）。

#### 合并 origin/main（2026-10-01，提交 1f9841304）

用户同步官方代码到 main 后（本地 main 落后 origin/main 315 提交，实际目标是 origin/main `67727e7cf`）。`git merge-tree` 预演 9 个冲突，实际解决：bazel.yml 保持 fork 删除；codex-api sse 三处（fork 公开模块+策略 与 main 的 responses_error 抽取并集，错误臂 Strict‖FlexUnavailable 双早退）；model-provider-info 两处（fork 的 bedrock runtime id 固定）；tui 投影（fork 数值种子 + main origins 门控并存）；compact 测试（fork 回归 + main program_tests 模块）；Cargo.lock 取 main 重解 + sha2 边；schema 重生成。上游 API 适配 3 处：ResponseStream.interrupt（两桥 None）、include_internal_metadata 替换目标检查、测试字面量补字段。本机环境事项：brew 安装 gstreamer（voice-host）；v8 系成员仍不可本机构建（既有 rusty_v8 限制）。合并后回归：工作区 check 绿（除 v8 系）；桥+api 338/338；core 选择集 153/165（12 失败=已知 code_mode 簇，与合并前一致）；tui 选择集 120 通过+4 负载 flake（空闲复跑通过、纯 origin/main 亦通过）；exec nuwax 4/4（重建二进制）；live 离线 93-94/94（每轮单个不同的负载 flake，隔离均过）。

#### 批次 B3（最终集成与真实请求验证）

- 构建绑定：`cargo build --locked -p codex-exec --bin codex-exec --offline`（cwd `codex-rs`，target `/tmp/codex-stability-20260930-target`），binary 绝对路径 `/tmp/codex-stability-20260930-target/debug/codex-exec`，SHA256 `581400825c6e03ea528bfeeebee81d7d693e10a71c0d1e2933743c01bda03529`，源 `5913e929f`+websearch 场景修正（后续提交 `351ea7770`，仅 live-tests crate，不影响该二进制）。
- 三协议本地 HTTP（同一构建产物）：`just test -p codex-exec --offline --retries 0 -E 'test(nuwax_env)'` → **4/4 pass**（`/tmp/b3-exec-final.log`；path/Bearer/x-api-key/body-model 断言 + 部分组零请求）。
- 真实网关：GLM websearch（anthropic 线，A1 约束翻译 + A4b rollout 断言）`just test -p codex-live-tests --offline --retries 0 -E 'test(glm_websearch_anthropic_rig)'` → **PASS 83.8s**（`/tmp/b3-glm-websearch.log`）；NUWAX 组最小 live：GLM chat/anthropic/responses + MiMo chat 各一轮 `只回复两个字：收到` → **4/4 exit=0 均回复"收到"**（`/tmp/b3-*.out|err`，凭据仅经环境变量注入，未回显；产物 `/tmp/b3-*`）。MiMo anthropic/responses 与 Step 未在本轮 NUWAX live 中运行（既有 live 套件覆盖其协议矩阵；NUWAX 组入口与协议无关，按最小原则未重复消耗配额）。
- 顺带修复：websearch 场景需显式 `web_search = "live"`（fail-closed 默认的必然结果），提交 `351ea7770`。
- fix/fmt：`just fix` 全部触及 crate（nuwax_env.rs 3 处、exec/lib.rs 1 处自动修复）+ `just fmt` + `git diff --check` 通过；fmt 后按 B3 重建重跑上述二进制层测试（绑定最终源）；另修复一个**既有**的 upstream 同步测试字面量缺口（app-server thread_processor_tests 缺 fork 字段，非本轮引入）。未在 fix/fmt 后重跑已验证的其余套件。
- 凭据扫描：新增/修改文档与测试中无任何密钥子串（`7274b51*`/`QFEbxeJq*` 零命中）。

#### 批次 A4c（F09+F10+F11：文档现状、基线口径、live 必需配置）

- F11：`my-docs/codex-review-prompt.md` 分派策略改为显式 wire_api 语义（Chat 不按 URL 推断；Responses 同协议直传；hosted 工具两线行为与 cached/indexed 报错）。live `LiveWire` 注释核实为准确（其描述的是测试 harness 显式调用 `RigProtocol::from_base_url`，非生产路由），未改。
- F09：`my-docs/claude-rig-full-validation.md` §2 归因口径改为"按名称聚类一致 + 已逐类举证的簇 + 未逐项复跑核实的推断"，并显式登记"CI Linux workflow run 链接缺失"。
- F10：`.github/workflows/live-tests.yml` 补 `LIVE_MIMO_RESPONSES_URL`（本地 .env.local 同值，token-plan 网关同基址）；preflight 必需键加入 RESPONSES_URL——所选厂商缺 Responses 配置直接构建失败，运行时静默跳过不再可能计为绿。新增/修改的 workflow **未实际运行**（需 push 后 manual dispatch，未获授权；登记为 not-run）。

#### 批次 B1（NUWAX env 解析、优先级、临时 provider 构造）

- 新模块 `utils/cli/src/nuwax_env.rs`（私有，lib.rs 再导出 3 个入口）：`NUWAX_BASE_URL`+`NUWAX_WIRE_API`+`NUWAX_API_KEY` 三者同设才激活；部分设置 fail-fast 报缺失变量名（不含值）；`NUWAX_MODEL` 可单独选已有 provider 模型；激活时构造 `model_providers.nuwax_env` 种子（name/base_url/wire_api/**env_key="NUWAX_API_KEY"**——凭据仅引用不落值）+ `model_provider="nuwax_env"` + `model`。校验：wire 严格枚举、URL 绝对 http(s) 带主机、空白/非 Unicode 拒绝、保留 id 冲突硬错。优先级落点：typed CLI（-m/--oss）> 显式 -c > 本组 env > config 文件（复用既有 ConfigOverrides 合并序，`model.or(cfg.model)` 与 `required_model_provider().or(typed).or(cfg)` 已核实）；typed/-c 显式选其他 provider 时整组忽略且**不校验**（未采用值不使有效配置失败）；显式选 `nuwax_env` 时使用完整组。
- dotenv 边界：`arg0` 的 `.env` 过滤前缀从 `CODEX_` 扩为 `CODEX_`+`NUWAX_`（§5.2：启动组只来自真实进程环境）。
- 测试 `nuwax_env_tests.rs` 12 项矩阵：三协议种子形状+凭据不回显、model-only、部分组报缺失名、无模型报错、typed/-c 满足模型要求、typed/-c 选其他 provider 忽略+免校验、-c 选保留 id 用全组、非法值/空白/非 Unicode 报字段名、保留 id 冲突、-c model 优先。`just test -p codex-utils-cli --offline --retries 0` → **43/43 pass**（+1 既有 skip）。

#### 批次 B2（各入口接线与 doctor）

- 接线：`exec/src/lib.rs`（typed model_cli_arg + oss/oss_provider）、`tui/src/startup_orchestration.rs`（cli.shared.model + oss/oss_provider，TUI 全入口共享）、`app-server/src/lib.rs`（standalone：cli_model/cli_provider 均 None——线程参数保持既有优先级）。种子并入 `cli_kv_overrides` 后进入既有 ConfigBuilder。
- daemon 边界：`daemon_startup.rs` 准入表新增分支——kv 含 `model_providers.nuwax_env` → Some("NUWAX environment provider (per-run credentials)")，强制嵌入式后端并给出诊断（§5.3：临时凭据不跨共享 daemon，不塞明文 key 进 thread 配置；queue 对此组合报能力错误的路径由既有"embedded+daemon 并存"检查承担）。
- doctor：新检查 `config.model_routing`（`cli/src/doctor/model_routing.rs`）——model、provider id、wire api、bridge/native、脱敏 endpoint（scheme://host，剥 path/query）、experimental 覆盖、窗口/绝对阈值/ratio 配置、有效窗口与有效压缩阈值（经 models-manager 派生）、后端选择说明；JSON 形状仅增 check id，字段兼容。
- 连带修复（测试暴露的真实缺陷）：默认 `web_search_mode=Cached` 在 chat-family 桥线不可表达，A1 的显式报错会使**默认配置**的 anthropic 会话失败。新增 `ProviderCapabilities.cached_web_search`（默认 true；ConfiguredModelProvider 按非 native 传输置 false，Bedrock 字面量同步）+ `resolve_web_search_mode_for_turn` 规则：preferred=Cached 且线不支持 → **Disabled（fail-closed）**，显式 Live 不受影响；桥内 cached/indexed 显式报错保留为纵深防御。语义权衡记录：fork 此前默认 cached 工具经字段丢弃翻译在 anthropic 线实际执行 live 搜索——查询泄漏到外网与配置模式相反，本身就是缺陷；现默认无托管搜索，需要搜索显式 `web_search="live"`。
- 验证：tui 选择集 **3/3**（含 NUWAX 排除用例，`/tmp/b2-tui.log`）；**二进制层端到端 4/4**（`exec/tests/suite/nuwax_env.rs`：三协议真实 codex-exec + loopback，断言实际 path/Bearer/x-api-key 凭据/body model + 部分组 fail-fast 零请求，`/tmp/b2-exec-nuwax4.log`，中途两轮失败为共享 `mount_sse_once_match` 硬编码 `/responses` 路径的测试基建伪影与本轮 fail-closed 修复，均已解决）。doctor/capability 回归：`just test -p codex-cli -p codex-model-provider --offline --retries 0 -E 'test(doctor) | test(capabilit)'` → **148/148 pass**（doctor 快照不受新增 check 影响，`/tmp/b2-doctor-cap2.log`）；core 解析回归 `test(web_search_mode_for_turn)` → **9/9 pass**（含新增 fail-closed 用例）。

### 16 开始工作前的阅读顺序

1. 根 AGENTS.md、相关子目录 AGENTS.md 和本文件。
2. `my-docs/FORK.md`、`my-docs/rig-responses-review-2026-09-28.md`、`my-docs/field-mapping-audit.md`。
3. `my-docs/anthropic-hosted-tools/`、`my-docs/nuwax-home/`、`my-docs/claude-rig-full-validation.md`。
4. 按本轮修改实际需要阅读源码和锁定 Rig registry 源码；历史 rebuttal 只是待核对证据，不是免审结论。

协议核对使用官方原始文档。当前设计依据：

- [OpenAI 流式响应](https://developers.openai.com/api/docs/guides/streaming-responses)：Responses 采用具名语义事件，测试要分别覆盖正常完成和错误事件。
- [Anthropic Web search](https://platform.claude.com/docs/en/agents-and-tools/tool-use/web-search-tool)：核对 allowed_domains、user_location、搜索结果和结构化引用字段。不能只保留工具名称便声称约束保持。
- [Anthropic Server tools](https://platform.claude.com/docs/en/agents-and-tools/tool-use/server-tools)：后续 pause_turn 设计需要按原始状态续接并限制续接次数，不能以重试原始 prompt 代替。

### 17 Claude Code 执行提示词

```text
请在 /Users/soddy/Documents/git-rust-work/fork-codex 开发并测试。

先读 AGENTS.md 和 my-docs/claude-protocol-stability-handoff-2026-09-30.md，核对当前 HEAD/工作树；保留并行修改。以交接文档的 Spec 为契约、Plan 为批次顺序、Tasks 为证据清单。

本轮执行阶段 A 和 B：先修复已确认的搜索约束、压缩 ratio 传播/计算、daemon 环境隔离和测试证据问题，再实现文档规定的 NUWAX 环境变量启动及各入口接线。按 A1→A2→A3→A4→B1→B2→B3 推进，每批完成回归后继续。C/D/E 仅登记后续路线图，不在本轮展开。

每个行为修复先补能复现缺陷的测试；测试必须检查实际配置、HTTP 请求、事件及历史，不能以非空回答或 nextest 总 PASS 数冒充有效验证。使用 just test、隔离 target 和同一批可追溯二进制，先离线和定向集成，再最小 MiMo/GLM 真实请求。凭据只读已有私有配置，禁止输出或提交。缺少平台、凭据或构建依赖就记录未验证，不伪造通过。

保持 Responses 经 Rig 同协议发送、显式 wire_api、历史追加及类型层投影；不升级 Rig、不新增不必要的 Core 大模块、不改变第一方 native/Bedrock。完整 workspace 测试依 AGENTS.md 单独确认，相关测试先完成。

全过程更新本文 Tasks 和每批证据，同步 FORK.md 与过时审查入口。结束时给出修改清单、精确测试命令/退出码/有效执行与跳过数、真实请求与工件路径、未完成边界。遵守 fix/fmt 后不重跑测试的仓库顺序，并复查最终源码。不自动 push、发 PR 或发布；提交按已有明确用户授权处理。
```
