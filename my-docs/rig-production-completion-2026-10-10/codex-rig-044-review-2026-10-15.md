# Rig 0.44 迁移：Codex 复审与阶段收据

**2026-10-16更正：下表Core cap 30/30因禁网跳过宏提前return，撤回为未执行，
不能算通过证据。原日志30项共0.212s。本轮实际执行与后续修复收据见
[集成复审](codex-integration-review-2026-10-16.md)，以该记录为准。**

日期：2026-10-15。审查范围为 `5b6b51457..bc4930f27` 的迁移及其测试证据，
并修复下列问题。工作目录是
`/Volumes/soddy/git_workspace/fork-nuwax-codex/.claude/worktrees/merge-main-20261014`，
分支 `codex/rig-044-20261014`。主工作区 `test` 仍为 `698824464`（Rig 0.42），
本次修复只提交到候选分支，不 push，不合回，不调用厂商付费 API。

代码检查点（各批 <500 changed lines；测试针对组合后的最终修复树执行）：

| 提交 | 范围 |
|---|---|
| `88ed4a5ec` | 无 rust-rig 构建过滤 v2 信封 |
| `afa0828b9` | reasoning 生命周期及上下文终态 |
| `05b191f37` | legacy reasoning／cassette usage／message_id |
| `508335917` | Chat origin、native 指纹与 hosted 位置 |
| `0b71e182e` | typed 工具结果、实际 wire 回归与图片夹具计时 |

## 1. 发现的问题及修复

以下行号以本次修复后文件为准；旧行为可在 `bc4930f27` 对照。

1. **P1：Chat reasoning 的迟到 End 被当作非法重开。**
   `codex-rs/codex-rust-rig-bridge/src/convert_response.rs:270`。
   Rig 0.44 的 Chat reasoning 在文本／工具开始后才发 End；原实现见到文本或
   工具就关闭 reasoning，稍后 End 报 `Rig reasoning changed after its item was
   completed`，正常思考模型无法完成回合。现在跟踪 Start→End，等权威 native
   终值确认；工具后缀预留 reasoning Done 位置，保持 reasoning→tool→Completed。
   实际 Chat SSE 覆盖 reasoning→答案、reasoning→工具两条路径。

2. **P1：Chat 第二次请求丢 reasoning_content。**
   `codex-rs/codex-rust-rig-bridge/src/convert_request.rs:195`、
   `src/request_messages.rs:185`（同 crate）。原实现只给 Anthropic 历史挂 origin，
   Chat 被 SDK 当跨模型内容降级为普通文本。现两 wire 都声明相符 origin；旧的
   无 native Chat 推理在支持该协议的自定义模型别名上仍回放到 reasoning_content。
   测试走首个真实 SSE→ResponseItem→JSON 序列化往返→第二个真实请求，保留工具
   call ID，另覆盖 app-server 公共接口的 reasoning→执行工具→答案。

3. **P2：Anthropic 上下文耗尽误判为输出 cap。**
   `codex-rs/codex-rust-rig-bridge/src/stream_pump.rs:225`。
   SDK 把 `model_context_window_exceeded` 和 `max_tokens` 都折叠成 Length；
   现从已有 raw SSE tee 恢复原终态，返回 ContextWindowExceeded，避免按增大
   输出 cap 处理。真实 SSE 验证错误分类，且不发工具 Done 或成功 Completed。

4. **P2：旧 Chat cassette 缓存输入重复计数。**
   `codex-rs/codex-rust-rig-bridge/src/cassette.rs:343`。
   0.42 Chat input 已含 cache，原统一迁移又加一次。现在按 provider 区分：
   Anthropic 加缓存并重派生 total；Chat 保留 input 和非零原 total。
   真实旧 fixture 的 131 input／128 cache／51 output 保持 131／182，另覆盖
   缺省 total、Anthropic 包含式输入及饱和算术。

5. **P2：旧 Anthropic cassette 丢 message_id。**
   `codex-rs/codex-rust-rig-bridge/src/cassette.rs:306`。
   旧 Final 的 ID 在 message_id，迁移只读 response_id。补兼容回退，并比较完整
   Completed（ID、usage、end_turn），不把无 ID 的完成误当有效历史关联证据。

6. **P2：v1 reasoning 多块签名与不透明内容损坏。**
   `codex-rs/codex-rust-rig-bridge/src/reasoning.rs:80`、`src/cassette.rs:204`。
   原先把后续文本并入首段签名，忽略后续 redacted/encrypted。现在每个旧 part
   独立转换，签名只配自己的文本；不透明块保留 native payload 和有效 canonical
   fingerprint，summary 不新投影到可见思考。cassette 的 reasoning/tool part
   共用唯一编号，避免串块。保留旧 rollout，仅读侧转换；完整 wire 和事件回归。

7. **P2：重包 v2 native 会使过期签名重新有效。**
   `codex-rs/codex-rust-rig-bridge/src/request_messages.rs:188`。
   原实现读 native.item 后再次 with_native，以编辑后的 canonical 内容重算
   fingerprint，SDK 无法识别原签名已经失配。现在 v2 保留原 fingerprint；仅
   真正旧 v1 在转换时构造 native。回归修改 canonical thought，断言旧签名不
   上 wire、修改后的文本保留。

8. **P2：空白 assistant 被 SDK 丢弃，hosted replay 索引错位。**
   `codex-rs/codex-rust-rig-bridge/src/request_messages.rs:135`。
   桥按适配前位置编号，SDK 删除空白 turn 后可能丢搜索块或拼进下一 turn。
   现在先按 SDK is_blank 过滤，再计算 turn／segment／块数；两组真实搜索
   call/result 整体对比，覆盖空串和空白字符。

9. **P2：SDK 重命名重复 call ID 后工具失败标志丢失或串位。**
   `codex-rs/codex-rust-rig-bridge/src/request_content.rs:51`、`src/transport.rs:293`。
   原 typed ToolResult 一律 is_error=false，随后按旧 ID map 补写，重名 c1→c2
   后无法对应。失败标志现随 typed result 流动；旧 metadata 只补缺失的显式
   false，不覆盖 true 或跨 ID 补 true。重复 ID 的败／败、败／成、成／败三种
   完整请求，以及原单 ID 成功／未知语义均覆盖。

10. **P2：不启用 rust-rig 的构建会泄出 v2 桥信封到原生 Responses。**
    `codex-rs/core/src/client.rs:3573`。无 feature 镜像仅识别 v1，v2 被当原生
    encrypted_content。补 v2 过滤；无 rust-rig 实编实测保留真正原生密文，
    剔除两代桥信封，同时确保原历史不被改写。

另修两处测试夹具问题：图片预算 loopback 在同步 TLS／默认客户端初始化前
就开始 5 秒 accept 倒计时，冷钥匙串导致假超时；现将准备工作移到倒计时前，
不延长 deadline、不加重试。app-server 冷恢复夹具只查看 user 的首个文本块，
遗漏 environment_context 后的真实 prompt；现遍历全部文本块，保留取消和
两次恢复的完整断言。两处旧 Core 测试显式空 BaseInstructions，隔离 main
新默认 developer 前缀，保留完整请求比较。

## 2. 实际验证

环境：macOS arm64、仓库 pin rustc 1.95.0、nextest local、offline、retries=0。
只连接本机 mock 服务。以下测试在最终 scoped fix/fmt 之前完成；之后不再重复
测试。首败与修复中间结果均保留于 `/private/tmp`，不把最后一次绿覆盖成首跑绿。

| 范围 | 结果 | 命令关键参数 |
|---|---|---|
| Bridge 全 crate | 295/295，0 skipped | `just test -p codex-rust-rig-bridge --locked --offline --retries 0 --test-threads 8` |
| Core 无 rust-rig 投影 | 3/3 | `just test -p codex-core --lib --no-default-features --locked --offline --retries 0 -E 'test(client::tests::bridge_tests)'` |
| Core cap 含原生对照 | 撤回：该轮30项被提前跳过 | `just test -p codex-core --features rust-rig,rust-genai --test all --locked --offline --retries 0 --test-threads 4 -E 'test(rig_output_cap)'` |
| app-server 公开 JSON-RPC 环境路径 | 9/9 | `just test -p codex-app-server --test all --locked --offline --retries 0 --test-threads 2 -E 'test(suite::v2::nuwax_positive::) + test(suite::v2::nuwax_positive_wires::) + test(suite::v2::nuwax_isolation::)'` |
| env 启动／恰好预算／差一预算 | 3/3 | `just test -p codex-live-tests --test nuwax_env_offline --test nuwax_env_budget_offline --locked --offline --retries 0 --test-threads 1` |
| 旧 cassette replay | nextest 84 PASS，含内部跳过 | `LIVE_CASSETTE=replay LIVE_VENDORS=mimo,glm,step just test -p codex-live-tests --test bridge_live --locked --offline --retries 0 --test-threads 4` |

- Bridge 基线 283 跑 279 过／4 图片夹具假超时；首批新增的 8 条语义回归
  修复前全部失败（reasoning lifecycle 2、Chat replay 2、cache 1、context 1、
  hosted/error 2），见 `codex-rig044-review-red.log`。无 feature 的 v2 泄出
  回归也先失败，见 `codex-rig044-review-core-red.log`。
- 后补的 legacy multipart／编辑 v2 签名回归尝试 RED 时先撞并行修改中的
  private Vec API 编译过渡，不冒称已实跑 RED；最后 295 全套实际覆盖。
- app-server 首次 8/9（prompt 首块夹具问题），修复后在最终重建的 CLI 上
  9/9；env 首次 0/3 是 receipt 正确拒绝过期二进制，重建后 3/3。
- 最终日志：`codex-rig044-review-{bridge-final,core-final,core-cap,replay}.log`、
  `codex-rig044-review-appserver-verified.log`、
  `codex-rig044-review-env-offline-final.log`（均在 `/private/tmp`）。
- 测试时二进制由 `cargo build --manifest-path codex-rs/Cargo.toml -p codex-cli
  -p codex-exec --locked --offline` 重建；测试源为 bc4930f27 + dirty 修复，
  不是已提交最终 SHA。测试 patch 为 `/private/tmp/codex-rig044-review-tested.patch`，
  SHA256 `865d957f7973cd5ec48ddc0b0cda78584bf105ff858033bc544ece9be2baa288`。
  CLI SHA256 `0e549836eda555673405cdcf08cb82aeb96400e5404bd4b7a45b0b0d3810dbb4`；
  exec SHA256 `f5317cf8c74417d3619db7bc2ad650c848400d20b6e010e0dbeebc2171843243`。
  后续按新提交运行时应重新构建并核 receipt，不能用这些旧收据冒充最终 SHA。
- 收尾 `just fix -p codex-rust-rig-bridge -p codex-core -p codex-app-server
  --features codex-core/rust-rig,codex-core/rust-genai --locked --offline` 成功；
  不是零告警：既有 ToolCallError 大 enum、live 测试 unused helper 仍在。
  工具自动改的 4 处既有无关样式已还原，不混入这轮行为修复。
  `just fmt`、`git diff --check` 成功。对比测试 patch 与最终源码 patch，
  仅格式化差异（含 live-tests 一处 import 重排），无新增行为改动；未在最终
  fix/fmt 后重跑测试。日志为 `/private/tmp/codex-rig044-review-{fix,fmt}.log`。

## 3. 证据更正与未完成项

1. 第四轮 281 resume/compact 测试默认 builder 使用内置 OpenAI provider_id，
   默认走 native。仅 feature 开启或 provider 显示名变化不证明 Rig 路由。
   62 env 是 Core 家族，也不等于 app-server API。原数量保留为普通回归，
   “只剩付费 record”的完成判断撤回。
2. 仓库已有显式 Rig Responses resume、Chat compact 超窗、Anthropic
   save/resume/fork 和来源／凭据轮换测试。下一轮先在当前候选明确执行并列
   出站收据，再补图片历史的三 wire × 冷／热恢复 × local compact 缺口，
   不重复造已有测试。此次 9 个 app-server 用例覆盖 Chat 冷恢复、Chat
   compact/recall 等指定路径，不宣称完整三 wire 生命周期矩阵。
3. 旧 replay 绕过新 SDK wire decoder／pump，部分测试内部 early-Ok。
   84／188 都不能冒称对应数量的协议场景真正执行；真实 loopback SSE 回归
   和新离线 v2 recorder→磁盘→加载→回放的证据须分别列出。
4. 当前 Rig-event cassette 仅将单 attempt 作为有效证据。跨 pause 复用
   recorder 会覆盖 terminal、复用 Part，导致错误或混合事件；0.42 也不支持
   成功回放这个路径。本轮不扩大修复。pause／hosted 用真实 wire 测试；
   将来需独立限制不支持的录制或设计 per-attempt 格式。
5. 已删除无效 Anthropic 私有 context 指纹镜像。当前 adaptive／非绑定模型
   的可达路径不比较它，旧测试不足以证明该算法。未来在 adapt 前启用显式
   thinking 模式必须重审；v2 native 自身 fingerprint 验证则保留且已回归。
6. 未运行当前候选的 Linux／Windows 原生、全量 CI／workspace、ACP dev 或
   真实厂商 API／视觉识别。旧 guardian／超时 ledger 不能直接把当前失败
   归作基线；需同 SHA 对照条件与明确工具链／栈／feature 收据。
7. 图片 Phase A（wire Base64 预算修复）已在候选内；MCP／Code Mode 入口
   的硬限和连续截图压缩仍是下一开发批次，不因升级 Rig 自动完成。

下一轮按 [开发提示词](claude-next-rig-images-2026-10-15.md) 分批推进。
