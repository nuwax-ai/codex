# Claude 验收轮结果(2026-10-05)

执行 `claude-acceptance-prompt.md` 五批开发。基线 HEAD `11f86b03e6a2a5302c490134226c4b5e60fb1a73` + Codex 复查修复(未提交工作树),全部保留并在其上开发;未 commit/push,未清理运行时文件,未放宽任何 deadline,未改 TLS/V8 pin。隔离 `CARGO_TARGET_DIR=/tmp/codex-stability-20261004-target`,`just test` + `-p` 构建限定(不带 `-p` 会把 v8-poc 拉进图,离线下载必败——已记入环境边界)。

最终工作树:34 files changed,+1018/−272(不含 3 个新测试文件与 2 个 D6 文档)。

## 1. 实际开发(功能与修复)

### 确定缺陷修复(产品代码,1 处)
- **迟到 finish_reason 覆盖首终止(真实缺陷,Chat)**:rig 的 Chat 适配器以**最后**见到的 finish_reason 构建终局,恶意/故障网关可在 `length` 终止后补发 `stop` 帧把截断包装成成功(本轮 D3 负控实测复现:`partial cap answerlate smuggled success` 成功完成)。修复于 `codex-rust-rig-bridge/src/sse.rs`:首个携带 finish_reason 的 chunk 之后,再携带 finish_reason 的 chunk 整帧丢弃(warn 日志),usage-only 帧仍放行。桥套件 246/246、最终批 604/604 回归零破坏。
- **曾尝试并在同轮回退**:EOF(无 `[DONE]`)后对已见 finish_reason 合成终局——与既有钉死契约 `error_tests::finish_reason_alone_cannot_hide_truncation_or_late_errors` 冲突(无 [DONE]=未交付终局,必须按截断可重试处理)。按契约回退,只保留首终止优先。

### 新测试资产(全部为公共路径)
| 文件 | 用例 | 覆盖 |
|---|---|---|
| `core/tests/suite/rig_output_cap.rs`(新) | 7 | D3 三协议 Core 级 cap 终止/零重采样/partial/截断工具/迟到帧/停流/双协议截断契约 |
| `exec/tests/suite/nuwax_env_controls.rs`(新) | 9 | C2 全矩阵(见 §3) |
| `cli/tests/queue_owner_matrix.rs`(新) | 3 | B2 env-owner/enqueue-as-submission/同名歧义 |
| `cli/tests/nuwax_session_remote.rs`(+2) | 6 | B3 坏值组/失活组 |
| `live-tests capture_validation.rs`(+CapExpectation) | — | D2 typed cap 校验 + base 含/不含 `/v1` 双形态 path 归一 |
| `live-tests/tests/exec_live.rs`(+2 场景) | 4 | D2 capped live(MiMo/GLM × chat/anthropic) |
| `app-server/tests/suite/v2/thread_resume.rs`(+1) | 1 | warm/running echo 覆盖分歧(见 §5 阻断) |
| D6 文档(新 2 份) | — | token-aware 预算 Spec/Plan |

## 2. 第一批 D3 —— 完成并验证

7/7 PASS(`just test -p codex-core --features rust-rig -E 'test(rig_output_cap)'`,匹配数非零核验):
- **Core 公共路径不重采样**:test_codex 全采样环,request/stream retries 均配 2;Chat/Anthropic/Responses cap 终止后**恰好 1 POST**。
- partial deltas 保留进 TurnComplete 前的流;恰好 1 个错误事件;无成功 AgentMessage;截断工具零执行零完成;wire cap=64(Chat 断言 max_tokens/max_completion_tokens **恰一**——默认测试模型为推理模型,rig 改写为 modern 拼写,与 D2 语义一致)。
- 负控:迟到 `stop` 帧不覆盖(上述修复);Anthropic 缺 message_stop / Chat 缺 `[DONE]` = 截断类可重试,3 attempts 钉契约、不冒领 cap 错误;length 后停流 = idle 预算内(5s≤t<25s)有界失败。
- **usage/partial 策略现状说明**(未新增保留,不合成成功 Completed):cap 终止时桥在 flush pending 输出**之前**返回错误,usage/Done 缺失是 ead6f2bca 前既有行为;ResponseStarted 等流内事件已发出。改此语义需新协议形状 → 先改 Spec/Plan(未做,登记为产品决策项)。

## 3. 第二批 C2 —— 完成并验证

9/9 PASS(`exec/tests/suite/nuwax_env_controls.rs`,全部真实 `codex-exec` 子进程):
- **握手层**:Anthropic+Responses × retries {1→2 POST 恢复, 0→1 POST 失败, default(4)→2}(STREAM 钉 0);矩阵单元并发执行避免冷启动超时。
- **采样层**:截断 Chat 流(200+半截 SSE)× STREAM {0→1, 1→2, default(5)→6} POST,全部最终失败。
- **idle**:1500ms 预算真实触发(elapsed≥1.5s 且 <25s),STREAM=0 时 1 POST。
- **取消**:503+Retry-After:30 期间 SIGINT → 子进程退出、2s 宽限后仍 1 POST(零后续 attempt)。
- **并行双进程**:独立 home、不同模型/凭据/协议/retry/idle,服务端捕获证明无串用(各自 Bearer/x-api-key、各自 attempt 数),config.toml 未改写。
- **坏值矩阵**:负数/非数字/i64 溢出/空白/零 timeout/非 UTF-8(unix)/孤立控制项 → 非零退出、命名变量、不回显密钥、零请求;显式 `-c model_provider=<named>` 覆盖时整组忽略(非法值也不报错)。注:config.toml 内的 model_provider 选择不构成 override,非法控制值仍 fail-fast(与 EnvSeed 优先级一致,解析先于选择)。

## 4. 第三批 B2/B3 —— 完成并验证

3/3 + 2/2 PASS(真实 `codex` 二进制 × 默认 socket daemon):
- **env-holding standalone owner**:daemon 持完整 NUWAX 组,无环境客户端 enqueue → owner 以自身环境执行,wire 断言 model=nuwax-owner-model、Bearer owner-env-key、turn_trigger=queue;config.toml 字节不变。
- **enqueue-as-submission**:owner 缺环境时 enqueue 成功("for thread …"),resume 被拒,submission 留队(count=1),零模型请求,config 不变。
- **同名跨 provider 歧义**:拒绝并列出两个 thread ID,零执行。
- **派发机制**(实现发现):冷线程 `queue/add` 只入队;派发发生在线程 idle(resume/加载)时由 queue 服务 drain——与"enqueue 是提交消息"的语义闭环。
- B3:corrupted 组(fail-fast 命名变量、连接前拒绝)、inactive 组(确实抵达远端连接,反向断言)。
- 剩余:`codex archive` 纳入 daemon_startup.rs 命令表(该矩阵以其他命令覆盖了同一启动策略)。

## 5. 第四批 D2 + 第五批 —— 完成与阻断

**D2 完成**:typed CapExpectation(Explicit/AnthropicDefault/Absent)× 协议字段;asserted_fields 如实记录(含 cap 字段或 `_absent`);离线负控 2/2;Anthropic 必需字段+默认 16384 语义钉死。**live 新证据**:重建 codex-exec 后(历史 Chat receipt 为 source `97720d19…`/binary `c17787a4…`；最终 Anthropic 通过则为另一份 source `b3cd797e…`/binary `c52d6d7c…`，均为 git dirty @11f86b03e 的工作树),`chat_rig_capped`/`anthropic_rig_capped` × MiMo/GLM **4/4 PASS**——真实厂商 wire 断言 `max_tokens=512`;过程修一处:path 校验现归一 base 含 `/v1`(仅追加 `/messages`)与不含两种合法形态(厂商网关即前者)。fmt 后如再跑 live 需重建 exec 更新收据。

**warm/cold(B4 延伸)——历史验证未完成（2026-10-05 Codex 更正）**:冷路径对照(既有用例)当日两次验证 PASS(串行批第 3 位,热身后);新增 loaded/running 覆盖分歧用例逻辑断言曾通过(tuple 断言 PASS;wire 计数出现捕获竞态,已加 5s 收敛轮询),但其完整通过需要 app-server 子进程热身,本机负载 20–65 振荡,`-j1` 批内位置 2 仍冷、位置 3 起才热;与 D4 同根源(10s initialize deadline + 521MB debug Mach-O 冷签名验证)。该历史失败不能全部归因于环境：Codex 后续发现 loaded 计数期待 3 而 helper 只发 1 次 seed、running fixture 挂了两个即时响应，第二个真实 turn 并未被延迟。已改为 2 次请求及明确响应 gate；新结果见 codex-review-push-2026-10-05.md。51/52 的其他通过项不证明此用例无逻辑缺陷。

**D4 未完成**(无低载窗口,负载持续 20+);**D5 登记**:完整 workspace、Bazel build、Linux/Windows、远程 CI、Step、加密引用、跨进程 opaque 回放、live cap **触顶**(本轮是"cap 上线"证据,非"触顶")未执行。**D6 设计交付**(spec+plan 两份;不改现行 cap、不截断签名,P0 复审保持)。

## 6. 最终验证总账(单次同图批,`--offline --locked --retries 0`)

`just test -p codex-api -p codex-rust-rig-bridge -p codex-live-tests -p codex-exec -p codex-cli -p codex-tui -p codex-config -p codex-utils-cli -p codex-core --features codex-core/rust-rig -E '<9 包选择,排除 exec_live/bridge_live>'`
→ **604/604 PASS,11777 skipped**(桥全量、api、live 单测、exec env×2 套件、cli queue/owner/remote、tui 三套件、config、utils-cli、core rig_output_cap/rig_anthropic/model_output_projection)。Codex 复核该历史批有 604 条 PASS，但没有执行 nuwax_session_remote（仅编译并过滤）；不能把 remote 纳入该批覆盖。完整调用参数/-E 未从日志恢复，原占位命令不能作为可复验命令。此批先于最终 `just fix`/`just fmt`(fix:exit 0,自动改动仅 1 处 unused-mut;fmt:exit 0,`git diff --check` 干净;其后未重跑测试,遗留 1 个 codex-exec 测试目标既有 warning)。

分项历史(不与上求和):D3 7/7;C2 9/9;B2 3/3+queue 套件;B3 +2;live capped 4/4;thread_resume 邻组 51/52(唯一失败为已登记 D4 类冷启动)。

## 7. 剩余与最小下一阶段

1. 低载窗口复验 D4 + warm/running 用例(或 CI Linux 跑 app-server 该组)。
2. 产品决策:cap 终止时 usage/Done 保留语义(需 Spec 先行);warm/cold 统一策略;D6 落地。
3. D5 各矩阵按授权单独执行;live cap 触顶专项。
4. 建议小批提交:①sse.rs 首终止修复+桥回归;②core D3 套件;③exec C2 套件;④cli B2/B3;⑤live-tests D2+live 证据;⑥app-server warm 用例;⑦tasks/报告/D6 文档。

## Codex 复查补充（2026-10-05）

历史 capped live 为 4 场景、8 次 pre-send 捕获，跨两份源码，不是同一最终树的 live4。较早 Anthropic 两场景为 field_mismatch、wire_asserted=false，最终通过发生在 path 修订后。已生成可随仓库传递的脱敏摘要 my-docs/live-cap-historical-evidence-2026-10-05.json；不提交原始 logs 或凭据。所有新的修复与验证、剩余矩阵以 codex-review-push-2026-10-05.md 为准。
