# 实施计划

按 A→B→C→D 开发，每批生产逻辑尽量低于 500 changed lines；超过时按真实依赖继续拆分。每批完成代码、相关测试、scoped fix、fmt 和记录后再进入下一批。

## A：统一 query 与 header scope

1. `codex-api/src/model_source.rs` 改成 endpoint-v2：仅规范化 origin/path，不 hash 任意 query 值；保留 userinfo 拒绝。在 codex-api 提供小型共享 scope 策略，替换 Core 的第二份凭据 denylist。
2. 所有非空 URL/config query 都视为私有 scope。`core/src/model_output_projection.rs` 必须把它作为 selected override：有 immutable snapshot 才进入现有随机 credentialInstance，完整 URI/query 已在缓存里有界比较；不可证明则 unknown/selector 不回放 opaque。这样 routing 值轮换同样隔离，而非仅删 query 后仍用 anonymous/account 身份。
3. 从实际 options.extra_headers 向 request_source 传递每轮 scope。先列出当前三个请求路径的头和覆盖顺序；仅静态规范/telemetry 白名单可忽略，其他头与 provider headers/auth snapshot 按真实 wire 的覆盖顺序合并。
4. 为未知 query 名、重复 query、空值、query_params、账号登录、无 key、header rotation 添加 API/核心集成回归。比较真实请求与持久化元数据，确保 opaque 降级、可见历史保留、旧 rollout 不改写。
5. `reasoning_source` 旧 wrapper 改 unbound，保留签名并记录原外部调用方的行为变化；生产 Core 继续使用有 scope 的入口。别用新测试专门检查已删除的 legacy-unscoped 字面量。

## B：session command ownership

先阅读 `session_archive_commands.rs`、`session_queue_commands.rs`、`named_session_lookup.rs` 与服务端 queue/list 处理器。N4 原报告的“queue 一定 provider missing”已撤销。

1. 行政/queue 名字查询明确传空 provider filter（跨 provider），检验现有 lookup 是否已完整分页和拒绝歧义；不全局改变 thread/list 的 None 语义。
2. 共享 daemon 仍拥有 queue writer。NUWAX group 不通过 queue RPC 切换该会话 provider；已存在 UUID queue 不因 client provider 不同而额外配置模型。冷执行/恢复的缺失临时 provider 必须明确提示需要可用 owner 配置，不能把 enqueue 成功等同模型执行成功。
3. 非 queue 的 embedded 启动可以使用 seeds，但统一由同一解析器产生，ConfigBuilder 和 bootstrap loader 使用相同 EnvSeed，install-method 排除与交互入口一致。避免在 lib.rs 新增大型编排代码，抽私有 startup policy 模块。
4. 测试 daemon 默认 openai + NUWAX 会话、UUID/名字、loaded/unloaded、同名跨 provider、daemon 不存在、显式 remote、完整/损坏/被显式 provider 屏蔽的环境组。验证无第二 writer、无 config 写入、无凭据污染。
5. 补同 provider echo 的 JSON-RPC resume 测试，断言返回模型和实际 HTTP model；无 override 对照恢复磁盘模型。

## C：direct env 请求控制

扩展 `utils/cli::NuwaxEnvInput` 原始 OsString 字段与组验证，解析到 provider 既有 retry/timeout 字段。同步 `app-server-daemon` 子进程剥离列表、CLI/TUI/exec/app-server 的 EnvSeed 接线和容器说明。

验收至少包含：0 次重试仍发 1 次、1 次重试 wire/capture=2、SSE 建立后不握手重放、取消 Retry-After 无后续 attempt、短 idle 超时、多个并发进程的独立环境/配置，以及显式 CLI/provider 优先级。不要只测试 parser 对象。

## D：稳定性门禁

- 在本地验证指南中写明 `just test -p codex-core --features rust-rig`，并检查过滤命令实际匹配数。现有 `.github/workflows/fork-cargo-pr.yml:72,96` 已显式设置 `codex-core/rust-rig`；保留该 gate，不重复添加或自动 dispatch。
- live marker/websearch 后对最终 capture 增加期望 path/model/cap 的比较，出具字段级 evidence。捕获的 attempt 不是服务端收件证明；不记录 auth header 值。
- 对 Responses incomplete、Chat finish_reason=length、Anthropic stop_reason=max_tokens 添加公共请求路径 mock，核对输出/usage/错误/重采样次数，按实际产品语义处理缺口。
- app-server 冷启动失败先在低负载复现，分开 spawn-to-ready 与 RPC 等待证据；不单凭 diff 无交集证明不是回归，不扩大 deadline 制造绿灯。
- 只在离线 gate 通过且需要验证厂商差异时执行已有授权的少量 MiMo/GLM 请求；Step/压力/加密引用矩阵逐项登记能力与调用成本，另行确认扩展范围。

## 命令约定

使用隔离 CARGO_TARGET_DIR、just test、--retries 0；Core 单包显式 rust-rig；不例行 --all-features。测试在最终 fix/fmt 前；改 Cargo 依赖时 bazel-lock-update，改 ConfigToml 时 write-config-schema。完整 workspace 需按 AGENTS 单独确认。日志同时保留首轮失败和定向复验，不能求和成唯一用例总数。
