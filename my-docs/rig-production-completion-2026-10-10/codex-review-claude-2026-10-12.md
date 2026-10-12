# Codex 对 Claude 后续批次的复审与修复

日期：2026-10-12。分支 `test`，开始时工作树干净，审查范围 `356052ee2..79ab0ce28`（15 个本地提交）。没有 push、dispatch、付费厂商请求、代理默认迁移、V8 pin 或依赖变更。

用户确认首个验收目标是 Linux/容器中的 Codex app-server，实际入口为相邻 `fork-codex-acp-ts` 的 `dev` 分支。随后再次明确：开发中心放在 Codex 通过 Rig 使用国内模型和自定义第三方平台。本轮只修改 Codex；ACP 仓库仅只读检查。

## 1. 审查发现与处理

以下行号指审查起点 `79ab0ce28`，新增包络及 overflow 回归指本轮文件；不把历史行号冒充修复后的行号。

1. **P1：TLS 回退可能重复已消费的 POST。** `codex-rs/http-client/src/tls_backend_fallback.rs:111` 去掉 `is_connect()` 限制后，连接已建立、服务端已收完整 POST 再发送协议版本告警，也会换后端重发。恢复连接阶段限制，增加真实 TLS 服务端回归，验证只消费一次请求、错误不允许 fallback。
2. **P1：Linux 五个 TLS 失败的归因错误。** 原结果文档 `results-followup-2026-10-12.md:35,42` 将外层 `Kind::Request` 当成 `is_connect()` 返回 false；实际日志源链为 `Connect`，该方法已识别。`SSL_CERT_FILE=/usr/lib/ssl/cert.pem` 使默认构造选了 rustls，遗漏的是 typed `AlertReceived(ProtocolVersion)`，包括 `io::Error` 的内部 payload。已补类型识别并保持证书/timeout 排除及连接限制。Linux 最终仍待新 SHA 的 CI，不能用本机通过预先关项。
3. **P2：EventBytes 裁切误报 FragmentBytes/TurnBytes。** `core/src/session/cap_partial.rs:386` 用序列化裁切后的长度判断 raw 是否容纳完整输入。控制字节用例实际未达到 raw 限额，却报告 `[FragmentBytes, EventBytes]`。改用 raw candidate 判断，回归比较完整 `enforced_caps`。
4. **P2：summary 身份重复计费、提前拒绝。** `core/src/session/cap_partial.rs:251` 用饱和差额增加计数，无法扣除 placeholder 的额外 suffix；而先按两个 section key 入场再扣费，也会提前拒绝预算内 summary。改为入场时直接记实际 `(section key, parent id)`。ASCII 和控制字符 ID 的回归覆盖 7,968 → 8,068 → 8,190 B，再拒绝真正超出 8 KiB 的片段；同时与 serde_json 独立核对转义费用。
5. **P2：完整事件结构包络不足。** 原 `EVENT_ENVELOPE_BYTES=4096` 未覆盖两条允许的 host key 的最坏转义开销。32 片段、256 B 控制字符 host key、完整最大 usage 的实际序列化为 **133,444 B > 131,072 B**。预留改为 8 KiB，事件硬限仍为 128 KiB；新增真实序列化上限回归。正常 host key 为 UUID/序列号构造，此回归同时保证防御性允许输入的合同。
6. **P2：usage 矩阵的一次 POST 断言无效。** `core/tests/suite/rig_output_cap.rs:911,1068` 丢弃真实 RequestLog，五个测试传常量 `1`。现在返回实际请求数并断言，保留非零重试预算。
7. **P2：负值/溢出单测未经过 Rig 反序列化。** `codex-rust-rig-bridge/src/usage_tests.rs:137,159` 只调用 raw accumulator。已补三协议真实 wire 测试：Responses 按字段丢弃非法计数；Chat/Anthropic 的负值会在 Rig unsigned 解码阶段失败，当前合同是有界 stream retry，不能宣称其 wire 路径也按字段容错。超过 i64 的可解码值仍保留其它合法计数，不制造已知零。
8. **P1：Anthropic 大 usage 会触发 Rig 算术溢出。** 新 wire 回归用 start input=4、delta output=`u64::MAX`，实际在 Rig 0.42 的 `providers/anthropic/completion.rs:233` panic，cap 变成错误重采样。已有 SSE 入口先记录原始 usage，再仅限制送入 Rig 的四个相加计数副本；最终仍由原始 accumulator 恢复并饱和累计，诊断保持 presence。边界、cache read/write、start fallback、终局顺序均有回归；未修改依赖。
9. **P2：缺失 Anthropic usage 的覆盖宣称不实。** 原结果文档第 92 行声称已有 truncation 覆盖；原 fixture 实际缺的是 message_stop，两个 usage 都在。已恢复两种缺 usage、但保留 message_stop 的真实 wire 形状，分别断言失败类别、三次有界尝试、零完成/工具/ledger/cap。
10. **P2：remote v2 压缩缺少公共记账回归。** `core/src/compact_remote_v2.rs:412` 有修改，但原 `core/tests/suite/compact.rs:1272` 只选非 OpenAI provider。现在同一用例分别走 local 和 remote v2，验证各两次请求、正确 compaction_trigger、不同持久 host key、provider ID 均 r1，账目 30 + 12 = 42；使用 `build_with_auto_env`。
11. **P2：CI sysctl 写入失败被当作 key 不存在。** `.github/workflows/fork-cargo-pr.yml:89` 对所有写失败输出“从未限制”，可能使 setup 假绿。现在先检查 proc sysctl key，存在时写失败直接失败；不存在只说明设置不可用，实际能力交沙箱测试确认。
12. **证据及规模修正。** `c569e2b38` 是 `+508/-12 = 520` changed lines，超过复杂批 <500 的目标，不能称 508<500。可拆为 capture/protocol/tests 395 行与 ledger/tests 125 行；本轮不重写历史提交。完整范围为 1,476 changed lines（去文档 1,336），应按既有独立阶段评审。原 rig_output_cap 22 个身份应分解为 **15 既有 + 7 新增**；原记录的另一个 predicate 当前静态选 8 个身份，历史“6/6”缺精确 receipt，不能补猜成当前证据。

没有发现新的模型可见 fragment 注入、ToolError 装箱行为回归或 host key 对已有完整对象比较的产品回归。诊断仍默认关闭，partial 不自动注入模型。

## 2. 验证及首败

macOS arm64。测试用 `just test`、locked/offline、nextest retries 0；只访问本地 fixtures，没有使用真实厂商密钥。

- HTTP：`just test -p codex-http-client --locked --offline --retries 0`，**184/184 实跑通过**。新增已消费 POST 和 typed rustls 告警回归均通过；第一个新增服务端 fixture 因 socket nonblocking 继承而首败，按现有 fixture 设 blocking 后通过。
- Core 包络首败：`just test -p codex-core --locked --offline --retries 0 -E 'test(serialized_envelope_covers)'`，实际 1 FAIL，133,444 B。8 KiB 预留后，原旧测试的“5 个完整 chunk”断言需随可用预算改为 4 个；首败保留，再联合验证。
- Core wire 首败：沙箱外并清除**测试子进程**继承的网络禁用标记后，`just test -p codex-core --features rust-rig --locked --offline --retries 0 -E 'test(rig_output_cap) + test(repeated_compaction_response_ids_count_separately)'`，**32 实跑：31 PASS、1 FAIL**，唯一失败为上述 Anthropic 算术 overflow。
- 修复后的联合命令：`env -u CODEX_SANDBOX_NETWORK_DISABLED just test -p codex-core -p codex-rust-rig-bridge --features codex-core/rust-rig --locked --offline --retries 0 -E 'package(codex-rust-rig-bridge) + test(cap_partial) + test(rig_output_cap) + test(suite::compact) + test(state::session) + test(suite::resume) + test(suite::token_usage_rollout) + test(capped_sampling_usage_follows_the_switched_step_model)'`，**432/432 实跑通过**，另 4,891 个身份被 predicate 排除。
- 环境入口：`env -u CODEX_SANDBOX_NETWORK_DISABLED just test -p codex-exec -p codex-app-server --locked --offline --retries 0 -E '(package(codex-exec) & test(nuwax_env)) + (package(codex-app-server) & (test(nuwax_positive) + test(nuwax_isolation)))'`，**24/24 实跑通过**，另 2,020 个身份被 predicate 排除。覆盖 exec 三 wire、坏值、真实重试/idle/取消/双进程隔离，以及 app-server 子进程环境启动、配置保护、重载/start/resume/fork。合计本轮定向实跑 **640 PASS**，不是完整 workspace 验收。
- 前三次 wire 调用（包括仅提升执行权限）仍继承禁网标记，测试早退：其 nextest PASS **不计入功能通过证据**。实际执行以本地服务请求断言及秒级测试收据为准。未改任何产品沙箱常量/逻辑。
- 当前 host 原始日志：`/private/tmp/http-client-tls-review-tests.log`、`/private/tmp/codex-review-cap-envelope-first.log`、`/private/tmp/codex-review-core-wire-first-executed.log`、`/private/tmp/codex-review-final-core-bridge.log`、`/private/tmp/codex-review-env-entrypoints.log`。这些路径只在本机有效；上文保留了可跨电脑读取的命令、数量及首败摘要。

用户明确选择本轮只保存定向结果，完整 workspace 留下一轮/CI；未运行完整 suite。Linux/Windows 原生、新 CI、付费 live、旧 reader、WS cap 专用 fixture、D6/model-visible recovery 仍未验。

最终 scoped `just fix -p codex-core -p codex-rust-rig-bridge --features codex-core/rust-rig --locked --offline` 通过、无告警；HTTP scoped fix 通过（既有 resolved_default_auth fixture 的 type_complexity 告警单列）。Core 最终 lint 前仅把新增压缩测试的两处 unwrap 改为有说明的 expect；按仓库规则最终 fix/fmt 后不重跑测试。userns workflow 只做 Bash 语法检查通过，未在 macOS 冒称校准 Linux 内核能力。最终独立只读审查未发现额外可操作回归。

`just fmt` 最终通过；首次 Python formatter 被沙箱阻止访问已有 uv cache，经正常审批使用该缓存后全流程通过。最终 `git diff --check` 通过，未引入范围外格式化变更。没有运行 Bazel argument-comment lint 或新 CI；不将 scoped Clippy 当作这些门禁的替代。

## 3. 剩余工作如何收敛

本轮代码及测试阶段已保存为本地提交（均 <500 changed lines），代码 tip 为 `8c5d46dab`，随后的文档提交不改运行代码：

| 提交 | 阶段 |
|---|---|
| `a2a2c0662` | TLS 连接门、typed rustls 告警、阻止已消费 POST 重放 |
| `cea93d06e` | cap 身份、raw/escaped 分类及完整事件包络 |
| `8ceb0d8ea` | Anthropic overflow guard、真实 wire 矩阵与请求计数 |
| `4de9ac698` | local/remote v2 压缩 ledger 公共回归 |
| `8c5d46dab` | userns sysctl 写失败不假绿 |

环境变量接线本身已经实现：`utils/cli/src/nuwax_env.rs` 使用一次性启动 seeds、`env_key` 引用凭据、显式 CLI provider/model 优先、坏值 fail-fast。exec 有三 wire、重试、idle、取消、双进程隔离的真实 mock 用例；app-server 有真实子进程环境正例和重载/start/resume/fork 隔离。CLI/exec/app-server 构建均启用 Core 的 rust-rig。不能将 40 项 roadmap 当作 40 个未实现的基本功能。

优先闭环：当前源码二进制 → Linux/容器环境 → NUWAX 环境启动 → Rig 三 wire 离线 fixture → 一个实际使用的 provider/model/wire 的真实验收。包含工具一次往返、取消、失败、重连/两次 resume、compaction/usage、两进程隔离、真实 URL/auth/model/cap/attempt 断言。具体续开发提示词见 `claude-next-rig-env-2026-10-12.md`。

需继续开发：真实复现影响上述路径的错误；现有 live harness 从 config.toml 临时 provider 启动，缺 NUWAX 环境启动模式。应先给现有 subprocess harness 增加 typed 环境模式并离线验证，不再造 provider 路由。

需验证或待 CI：当前 SHA 的 userns/磁盘/TLS 修复；两个三平台历史失败的最小复现（projects lifecycle 顺序、guardian migration history）；当前 Linux build/运行/退出/持久化和 source/bin 一致性；实际使用模型的协议能力。基线同败不等于可忽略，但与 Rig 主路径的关系需分别定位。

独立 roadmap：D6 可证明 tokenizer 预算、partial 模型可见恢复、全套旧客户端降级、完整 SDK/UI 查询面、代理默认迁移、全部行政/异 OS/Windows/macOS 发布矩阵。诊断模式正常工作不需要先实现这些扩展；不得因此宣称这些任务已完成。

还需补一个已有合同边界：旧 rollout 的 `ResponseLedgerKey::of` 和恢复 ledger 仍能 clone 旧的超长 identities。它不是本轮新增回归；后续对恢复后的内存记录做同一身份规范化时，要保持 rollout 原字节和累计 usage，而非重写历史或截断成别名。

## 4. ACP 入口的只读结论

相邻仓库 `dev` 为 `4e1f33b8723b1c35574717cd3cb8d19015e7223d`，工作树干净，未切换/修改分支。`src/CodexJsonRpcConnection.ts:15` 继承 process.env 并启动 app-server。`CODEX_PATH` 可指向当前候选；默认由 `src/resolveCodexBinary.ts:17` 选择安装的 nuwax-codex，依赖声明 `^0.18.12`。本地源码 commit 不自动更新该二进制；验收必须明确路径/版本/hash，不能沿用旧包收据。

需在 ACP 后续单独确认的真实问题（本轮未修改）：

- `src/TokenCount.ts:31` 对部分 usage（input=0、cached=2）会得到负 input；第 14–19/29–34 行未映射 cacheWriteInputTokens。
- `src/CodexAcpClient.ts:434` 环境独立启动时 providers/list 只读 ACP 显式配置，可能把实际自定义路由显示为默认 OpenAI。
- `src/CodexJsonRpcConnection.ts:44` 与 `src/index.ts:92` 在启用日志时记录 raw RPC 和 authentication config；带显式 token 的配置需脱敏实测。这不意味着本轮 NUWAX 环境模式已将 key 写入配置。

先把 Codex 本体通过 Rig 的功能证据闭环，再按实际 ACP 消费的字段补兼容。ACP main 仍用于同步上游，dev 是用户使用分支；不替用户切分支或发布。
