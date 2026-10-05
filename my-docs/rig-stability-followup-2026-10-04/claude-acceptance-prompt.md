# Claude Code：补齐未完成验收并修复验证中发现的逻辑问题

## 工作目标与基线

在 `/Users/soddy/Documents/git-rust-work/fork-codex` 继续实际开发和测试，补齐 B2/B3、C2、D2/D3、warm/cold 的未完成验收；对 D4/D5/D6 提供实际证据或明确阻断，不能只写计划或重复旧测试后宣布完成。

已知 HEAD：`11f86b03e6a2a5302c490134226c4b5e60fb1a73`。工作树包含 Codex 尚未提交的身份隔离、公开 Rig scope、queue owner、capture 校验修复和回归测试。上一轮相关批次为 375/375，但这是历史证据，不代替你的新结果。

开始时记录实际 HEAD/分支、`git status --short`、完整 diff 和构建输入摘要。如果 HEAD 已变化，以实际状态记录，不能 checkout/reset 到上面的 SHA。保留现有未提交文件和别人的修改，包括新增的 scope_replay_tests.rs、capture_validation.rs、capture_fields.rs。不要 stash、覆盖或清理 SQLite/tmp/.env.local。

依次阅读：

1. 根及涉及子目录 AGENTS.md。
2. `my-docs/codex-11f86b03e-review-2026-10-04.md`，特别是第 2、4 节。
3. 本目录 `spec.md`、`plan.md`、`tasks.md`。
4. `my-docs/container-multiprocess-env-review-2026-10-03.md` 和当前请求控制/协议字段文档。

复查报告及更新后的 tasks 优先于旧报告的“全部完成”措辞。分批执行下面的顺序，每批生产逻辑尽量低于 500 changed lines；发现确定缺陷直接修复并添加公共路径回归。不能为求绿改 deadline、降低断言或恢复 legacy-unscoped/头部前缀通配/provider 首行否决。

## 第一批：D3，Core 层的输出触顶终止

- 使用 `codex-core` 的公共执行路径和 rust-rig feature，而非只调用 bridge。
- Chat 和 Anthropic 分别设置明确输出 cap、非零 HTTP/stream retries，mock 返回 finish_reason=length / stop_reason=max_tokens；Responses 已有用例作对照。
- 断言实际 wire cap、partial text、唯一 terminal error、零成功 Completed、截断工具不执行、exactly one POST。证明 Core 不重采样，不能用直接 bridge 的一个请求替代。
- 增加 terminal error 后迟到 completed、断流、idle 的负控，旧错误不能被覆盖，不能留下后台重试或继续执行工具。
- 先说明当前 usage/partial item/rollout 的真实行为：cap 时缺少 Done/usage 是此前已有缺口。若补保留策略，必须使用真实计数，明确 UI、持久化历史、内部统计各自去向及兼容性；不得猜 usage 或合成成功 Completed。需要新协议形状时先更新 Spec/Plan，再同步 schema 与消费方测试。

## 第二批：C2，新环境变量的真实进程矩阵

验证 `NUWAX_REQUEST_MAX_RETRIES`、`NUWAX_STREAM_MAX_RETRIES`、`NUWAX_STREAM_IDLE_TIMEOUT_MS` 经生产启动入口落到真实请求行为：

- 三协议握手 retries=0/1/default 的实际 attempt 数；0 仍发送一次。
- 固定 HTTP retries 后，stream retries=0/1/default 的 Core 重采样次数；明确握手和采样是两层预算。
- 短 idle timeout 确实触发；取消 Retry-After/等待 SSE 时关闭请求，不再产生后续 attempt。
- 同一容器或宿主的两个子进程使用独立 CODEX_HOME、有效 SQLite 路径，不同模型、凭据、协议、retry/idle 值；服务端证明无串用，config.toml 不被改写。
- 完整/失活/损坏/非 Unicode/负数/溢出/孤立控制项，以及显式其他 provider 的优先级。遵守 reserved provider 表限制，不为测试放开保留表污染。
- 鉴权仅在本地 mock 服务端断言，不把密钥写入 capture/报告；不得修改测试 runner 的全局环境。

既有 typed-provider wire/cancel/idle 测试只能证明底层行为，不算新环境接线覆盖。相同底层场景可复用 helper，但必须实际启动生产进程消费环境变量。

## 第三批：B2/B3，实际 owner 与行政命令启动

- 在现有 retarget 后 UUID/名字 enqueue 回归上补 loaded NUWAX standalone owner、loaded/unloaded、owner 存在/缺失、daemon/embedded/explicit remote、UUID/名字及同名歧义组合。
- 持有 NUWAX 环境的 standalone app-server 可以是真实 owner；不能根据默认 socket 或创建时 provider 字符串推断它没凭据。
- 有 daemon 时禁止另起 embedded queue writer；验证 writer 数量/实际 RPC 去向，而不只检查枚举返回值。
- enqueue 是提交消息；执行/queue-start/冷 resume 由实际 owner 依据当前配置检查。分别断言 enqueue 和执行结果，不能把 enqueue 成功称为模型执行成功。
- 行政命令补完整/失活/损坏环境组、CLI/provider 优先级、install-method、默认 daemon 存在/不存在、embedded seeds、显式 remote fail-fast 的公共 CLI 启动回归。
- 断言 owner 实际 model/auth、原创建 metadata 与 config 未改写、环境及凭据不成为另一个客户端的 daemon 默认配置。

## 第四批：D2，typed effective cap 和新 live 证据

- 将场景期望的 effective cap 接入 capture 校验：请求明确值优先、provider fallback、未配置/absence，分别对应 max_tokens、max_completion_tokens、max_output_tokens。
- 明确 Anthropic 必需字段及当前默认 cap；核对 reasoning-model 的 Chat 字段转换，不能所有 Chat 都硬断言 max_tokens。
- 每个真实 attempt 校验非空捕获、scheme/authority/协议 path、model、实际 cap 字段。asserted_fields 只列实际断言项；空、缺失、错误字段不能变绿。
- 保留 RawValue 出站字节/数字精度，校验不能对整份请求做解析再序列化。便利 parsed body 无法承载某些合法 raw 数值时，不能把便利视图误当原始请求；必要时从 body_raw 提取所需字段。
- 新逻辑先由离线负控证明。确有厂商差异需要验证时，只做已有授权的少量 MiMo/GLM 请求：密钥由 .env.local 加载，不回显、不落 header 工件。
- 新 live 前重新构建 codex-exec，独立核对当前 HEAD/源码摘要及同一 executable 的 SHA/收据。旧提交前的 7/7、22ff9289… 或上一轮 375/375 不能复用为你的当前源码验收。
- 如果本轮另有提交，提交后的 HEAD/输入集合可能变化；不能继续沿用提交前收据。真实 cap 触顶、加密 citation 与基础 marker 分开计数。

## 第五批：warm/cold、D4、D6

- echo provider 与无 override：cold、loaded、subscribed、running 各有对照，核对返回 model/provider 和实际 HTTP model。
- 保留 live owner 的配置约定；不要为统一 cold/warm 输出强行切换运行中 thread。需要修改产品语义时先记录兼容边界和方案。
- D4 做符合条件的低负载 cold/warm 对照，记录 load、spawn/initialize 时序、旧/新二进制身份、原始失败。没有合格低载窗口就保持未完成，继续其他工作。不要扩大 timeout；_dyld_start 栈或页哈希数量不能单独证明具体耗时根因。
- D6 交付独立 token-aware Spec/Plan、signed/opaque 整体保留与失败策略、缓存/迁移兼容方案，以及受控 tokenizer/boundary 测试。40,960 bytes 和 bytes/4 不是精确 10K-token 证明；未经产品裁决不缩小既有 pause cap、不截断签名。P0 人工复审不能靠普通字节测试关掉。

## D5：门禁与未执行矩阵

分别登记 workspace、Bazel build、Linux/macOS/Windows、容器、远程 CI、Step、实际 live cap 触顶、加密引用、跨进程 opaque。可在现有本地环境运行受控 mock 验证；无法执行则写明缺少什么，不算通过。锁文件同步不等于 Bazel build，macOS 本地测试不等于跨平台。

完整 workspace 按 AGENTS 单独确认；没有授权先完成相关测试，不停止可推进的工作。不要自动 dispatch CI、扩展付费压力矩阵、push 或发布。

## 测试入口、证据与交付

- 使用隔离 CARGO_TARGET_DIR、`just test`、显式非零匹配数、必要时 `--offline --locked --retries 0`。不直接 cargo test，不例行 all-features；Core 单包桥测试显式 `--features rust-rig`。
- 同一阶段尽量保持 package/feature 构建图一致；缺 helper 时按仓库方式构建，不能把缺辅助二进制算产品失败或默默跳过。
- 环境权限限制与逻辑错误分开；记录首轮失败和最小必要复验，不放宽 deadline，不禁用 TLS、不随意更换 V8 pin。需要平台能力时说明具体限制。
- 所有测试安排在最终 scoped just fix/just fmt 之前，最终之后不再重跑测试；说明自动改动范围。变 Cargo 依赖同步 bazel lock；变 ConfigToml/API shapes 同步相应 schema。
- 更新现有 tasks 的每个复选框及证据：HEAD/源码摘要、完整命令、feature/target、selected/executed/asserted/pass/fail/skip/timeout、首轮失败和复验、工件路径、未验边界。不要把不同批次求和成唯一用例总数。
- 输出 `my-docs/claude-acceptance-results-2026-10-04.md`，列实际开发、验证结论、剩余/阻断项和最小下一阶段。已完成必须有功能断言，不能仅以 fmt、编译、静态检查或修改基线作为完成证明。
- 本轮不要自动 commit/push，不改写现有历史，不清理运行时文件。需要时最后给出按真实依赖划分的小批提交建议。
