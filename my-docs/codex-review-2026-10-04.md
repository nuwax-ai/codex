# Codex 独立复查与后续工作（2026-10-04）

基线 HEAD：`ead6f2bcae3e0cac4a8374708e9dc1d1d31296a6`，分支 test。输入是 Claude 的 R1–R4 八文件 diff（+156/−11）、两份交接/结果文档。运行时 SQLite/tmp 和 .env.local 未清理或纳入改动；本轮不提交/push。

## 1. R1–R4 裁决

1. **R3 接受** — `codex-rs/codex-rust-rig-bridge/src/stream.rs:48` 的独立 40,960-byte pause cap 恢复既有行为，不耦合 9,800-byte envelope cap。仍有四次 continuation 上限和最终 wire/context guard。20KB 正向与 41KB fail-closed 测试覆盖有效。
2. **R4 接受** — `codex-rs/codex-rust-rig-bridge/src/transport_identity.rs:125` 在成功重建前补 UNKNOWN-index 外来 call。envelope validator 只接受 UNKNOWN server_tool_use，request_messages 在原 call 存活时去掉克隆，sanitizer 整对过滤。成功 prefix/fallback suffix 是降级位置差别；两者 call 在 result 前，不能把原位置已经缺失的恢复称为精确 wire 顺序保真。
3. **R1 接受，但原测试不足** — `codex-rs/tui/src/session_archive_commands.rs:258` 解析优先级与交互启动同源，在 remote 连接之前拒绝有效本地临时 provider。原测试仅测谓词，删除新 guard 仍会通过。
4. **R2 接受** — `codex-rs/tui/src/lib.rs:1025`、`daemon_startup.rs:47` 由协议常量构造保留键，没有改变配置/RPC/rollout wire 格式。

## 2. 本轮小修复

- 新 `codex-rs/cli/tests/nuwax_session_remote.rs` 实际启动 codex 子进程，覆盖三协议 × queue/archive/unarchive/delete（12 个完整组场景）和四个命令的不完整组场景。临时 CODEX_HOME、子进程 NUWAX 环境、监听 socket 的无连接断言，不修改 runner 环境。错误不得回显模拟 key。
- `codex-rs/codex-rust-rig-bridge/tests/wire/identity_replay_tests.rs` 新公共 wire 回归：原 call 缺失/存活 × layout 完整/缺失，共四场景。通过 envelope→request_messages→sanitizer→真实 HTTP，深比较完整 messages，验证 prefix 和每个 call/result 恰一次。
- 原 Claude 报告修正：标题“未发现 P1”与 R3 矛盾；N4 的 queue 必然 missing-provider 结论不成立；live path/model/auth 被断言的说法与 wire_asserted=false 冲突；测试进程环境约束不能成为不写子进程回归的理由。
- 冷启动证据注明归因边界：采样定位在进入 Rust 之前，但 `_dyld_start`、二进制大小和页哈希数量不足以定量证明签名/动态链接/换页各自耗时。保留失败，后续做低负载冷启动对照，不直接修改 timeout。
- direct-development Spec 补独立 pause cap 和 P0 人工预算复审。40,960/4=10,240 只是估算，不能证明未知 tokenizer 下低于 10K tokens；本轮保持原产品兼容行为。

## 3. 需要继续开发/补测的项

1. **[P2] Query 私密 scope** — `codex-rs/codex-api/src/model_source.rs:13` 仍将未列入 denylist 的 query 值纳入持久化摘要；`sessionkey/sig/hmac` 用作凭据时，保密声明不能成立。API 与 Core 两份名称表也会漂移。需新 source 版本与 conservative scope，不能只补三种名字。见后续 A。
2. **[P2] 每轮 extra headers** — `codex-rs/core/src/model_output_projection.rs:111` 只合并 provider headers/auth snapshot。现有 caller 主要是 metadata/telemetry，不确认当前凭据错配，但未来私有头加入时会与 wire 分叉。完成 shared scope 并补真实请求 rotation 回归。见 A。
3. **[P2] Session 命令归属** — `codex-rs/tui/src/session_archive_commands.rs:294` 的本地路径忽略 seeds；`session_queue_commands.rs:104` 与 archive 名字 lookup 传 provider=None，服务端 `thread_processor.rs:5503` 将其解释为默认 provider。按名字可能看不到另一 provider 会话；UUID queue 不加载 provider，可能成功。仅改 exclusion 会撞上 `session_queue_commands.rs:41` 的禁止第二 writer，需完整 ownership 策略。见 B。
4. **[P2] Core 验证 feature** — `codex-rs/core/tests/suite/rig_anthropic_hosted_tools.rs:928` 等桥测试需 rust-rig；单包默认命令匹配 0 个不能算覆盖。补本地指南/匹配数检查，并补同 provider echo resume；现有 `.github/workflows/fork-cargo-pr.yml:72,96` 已显式开启 feature，不需重复添加。见 B/D。
5. **[P2] Live 字段验收** — `codex-rs/live-tests/src/binary_turns/scenarios.rs:35` 只解析/计数 capture，`:62` 写 wire_asserted=false。后续实际比较 path/model/cap；auth 使用独立离线 wire 证据。见 D。
6. **P0 人工上下文复审项** — `stream.rs:48` 恢复的 pause cap 非 tokenizer 上限证明；设计 token-aware 边界及兼容方案，禁止直接截断签名。此标签来自仓库 context review 规则，不代表本轮引入一个新的 P0 运行时缺陷。

完整功能与验收已拆为 `rig-stability-followup-2026-10-04/{spec,plan,tasks}.md`，可复制提示词在该目录 `claude-prompt.md`。genai cap 保持搁置；N1/N2 是文档/工作流边界，N6/N7/N8 不需在本轮制造行为改动。

## 4. 验证记录

- 独立复跑 Claude 八文件树的 Rig 全 crate：`CARGO_TARGET_DIR=/tmp/codex-review-2026-10-03-target CARGO_BUILD_JOBS=4 just test -p codex-rust-rig-bridge --offline --locked --retries 0 --test-threads 2`，**exit 0，241/241 passed，0 skipped**，36.366s 测试段。日志 `/tmp/codex-oct04-rig-review.log`。
- 加入公共 R4 wire 回归后，同一 Rig 全 crate 命令：**exit 0，242/242 passed，0 skipped**，44.544s 测试段。新增测试包含四个深比较 HTTP 场景。日志 `/tmp/codex-oct04-rig-final-review.log`。
- CLI/TUI/daemon 首轮广选：**exit 100，80 run / 64 pass / 16 fail / 6,153 filtered**，301.845s 测试段，构建 19m29s。16 个失败日志均明确出现 `failed to invoke ps ... Operation not permitted`（4 个 daemon lib、12 个 CLI daemon/worktree）；新增 CLI 两测试在本轮已通过。日志 `/tmp/codex-oct04-cli-tui-review.log`。
- 在允许宿主进程查询的环境，用相同三包图缩小到新增 CLI 回归 + TUI/daemon 相关项：**exit 0，55/55 passed，6,178 filtered**，103.105s 测试段。包含首轮四个 daemon-lib 失败的复验；CLI 两测试实际执行 12 个完整组及 4 个不完整组场景。日志 `/tmp/codex-oct04-cli-tui-host-review.log`。
- 相同三包图，仅复验首轮另十二个 CLI 失败：**exit 0，12/12 passed（6 slow），499 tests/7 binaries filtered**，354.226s 测试段。最慢 packaged double launch 为 164.438s，在仓库原有该测试专用预算内，未改 deadline/retry。日志 `/tmp/codex-oct04-cli-daemon-host-rerun.log`。因此首轮 16 个失败均已在宿主环境定向复验通过；不将这些批次求和为唯一用例总数，也不宣称首轮全绿。
- scoped `just fix -p codex-cli -p codex-tui -p codex-rust-rig-bridge --offline --locked` 首轮 exit 0（12m43s），有一项新测试的 expect_err 告警、无自动改动。将该断言失败路径改为 `.err().context(...)` 返回测试错误后，**终检 exit 0、零告警、无自动改动**（6m10s），日志 `/tmp/codex-oct04-review-final-fix.log`。
- `just fmt` 首轮 exit 1：Rust 已格式化，Python formatter 因 sandbox 拒绝访问现有 uv cache 失败（`/tmp/codex-oct04-review-fmt.log`）；允许宿主缓存访问后相同命令 **exit 0**（`/tmp/codex-oct04-review-host-fmt.log`）。源码摘要对比：收尾只变动新 CLI 测试的失败返回/格式与新 wire 测试格式，Claude 的生产修复字节未改变。按 AGENTS，最终 fix/fmt 后未重跑测试。
- 最终 `git diff --check` exit 0，index 为空，未 commit/push。代码/测试 10 文件、331 changed lines，可按 R4、R3、R1/R2+CLI 回归三组审查；背景报告和后续 Spec/Plan/Tasks 独立作为文档组。
- 独立证据代理核验 Claude 的原日志：Rig 241/241、**定向** TUI/daemon 53/53（5,669 filtered）、live 7/7（24 filtered）。53 不是 TUI/daemon 全套；数字不合并成唯一测试总数。
- Claude live 二进制 SHA256 `93ae5891b262a2e23650110732f32ab5234de92ccfc1d46c9fb988991bc0bd04` 与七个 manifest 一致，505,311,192 bytes；原验证树摘要 `831b07e4b56f300f353246476a82986720add937f1b013ae42bc3e17dfd992c8` 按 Rust 算法独立复算相符（8,648 inputs、1 symlink）。旧 `/tmp/codex-review-digest.py` 的 symlink 分支不完整，不能直接复用；独立复算未依赖它。
- 六个 marker 各有真实 completed command/exit 0；GLM websearch 两轮各有一个匹配 completed search 对及 614/55 字符回答。七份 capture-evidence 均 wire_asserted=false；引用 capability=false。证明最小真实行为，不证明加密引用或 live header/字段逐项验收。
- 上述收据绑定 **Claude 当时的验证树**；本轮新增 Rust 测试已改变 source inputs，旧收据不能用于当前最终树。未重建/重复付费 live，不将旧结果充当本轮新回归的证明。
- 完整 workspace、Bazel build、远程 CI、跨平台/Docker、Step、真实 cap 触顶、跨进程 opaque 与加密 citation 未执行。model_auto_review 的冷启动失败保留 Claude 原记录，不用 diff 无交集替代低负载复验。
