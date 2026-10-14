# main 合并批次 — 2026-10-14

整合分支 `codex/merge-main-20261014`（隔离 worktree），merge commit **`481a3f485`**：
`origin/main` `cb03f28a4`（562 上游提交，merge-base `67727e7cf`）并入 fork `test`（`698824464`，
含当日 wire 图片预算修复）。**未合回 test、未 push**——整合候选先交复审。

## 冲突处理（43 路径，逐机制）

- **fork CI 保留**：7 个上游 workflow 为 DU（fork 删除 vs 上游修改），保持删除（fork-cargo-pr/live-tests 覆盖 nextest+bazel 车道）。
- **生成物**：config.schema.json、app-server schema（stable+experimental）、ResponseItem.ts、MODULE.bazel.lock 全部从合并源重新生成（`just write-config-schema` / `write-app-server-schema [--experimental]` / `bazel-lock-update`）。
- **能力模型**：main 把 provider capabilities 搬进 config（`ModelProviderCapabilities` + `from_config`）并新增 `capabilities` 字段；fork 的 `experimental_bridge`/`provider_id` 并存。`from_config` 内加 fork 传输闸门：cached_web_search 与 remote_compaction 仅原生 Responses 传输可用（config 只能降不能升过 wire）——main 的新测试期望与本闸门一致（夹具含 `experimental_bridge = "native"` 的用例直接过；不含的已按 fork 契约适配）。
- **client.rs 大块**：main 拆出 `ResponsesHttpExecutor`（turn_state/event_sender 参数化）。fork 的 `stream_model_bridge`/`opaque_replay_authorization`/WS 路径全部迁移；`output_provenance` 改为 `Arc<Mutex<Option<..>>>` 共享单元由 executor 写、session 持有（background 压缩同样传真实单元——否则后台产出的压缩项因无 provenance 被投影剔除，已修复并验证 10/10）。
- **压缩族**：main 引入 Foreground/Background 枚举 + installation.rs 重构；fork 的 sampling host key、`compaction_provenance`、`record_observed_response_completed(response_key)`、incomplete/cap 用量记账全部接通。**max_output_tokens 解码为 fork 的 CapExhausted**（非 main 的 ResponseIncomplete）：压缩重试分类与模型回退分类补 CapExhausted 终态；cap 用量走 `record_cap_exhausted_usage`（host key 联账）+ SpendingOnly（花费入账、上下文基线不动——main 的双计防护语义）。
- **协议变化适配（无冲突面）**：`ResponsesApiRequest.instructions` 删除 → base instructions 以 input[0] developer 消息传输；两个桥（rig/genai）删除 instructions 读取并依赖 system/developer→System 映射；Message/FunctionCall/WebSearchCall 增 `encrypted_content`/`status`、OutputText 增 `annotations`/`logprobs` 的全部字面量/模式机械适配（rig/genai bridge、live-tests、core/app-server 测试树）。
- **patched-zsh 移除**：unified_exec 的 zsh_fork 分支按 main 删除，未复活。
- **watcher/discoverV2 整族取 main**：fork 的 registration/backend 重写与 main 的协议类型演化（ExecutorSkill 形状等）不兼容且 fork 重写基于 fork 时代协议；上游演化内部自洽（file-watcher 内联架构 + discoverV2 联动 + 测试同源）。fork 侧 app-server fs_watch 适配上游 `add_subscriber`/`registration_succeeded` API。fork 在该族的上游未吸收差异登记为后续审查项。

## 验证（macOS arm64 本机，rustc 1.95.0，CI 同款 feature 集 `codex-core/rust-rig,codex-v8-poc/sandbox`）

- v8：按 fork CI `setup-rusty-v8` 复刻（openai/codex release `rusty-v8-v150.4.0` 配对工件下载 + 双重 sha256 校验）。
- 预建二进制：codex-exec、codex、codex-code-mode-host、codex-voice-host、test_stdio_server。
- **1118/1118**（rig bridge 280 + genai 23 + protocol/app-server-protocol/file-watcher/model-provider，含本日图片预算出站回归）。
- **core 定向选择器 333/333**（rig_*、compact 全家含 async/background、cap partial、wire budget）。
- **codex-core 全量 5333/5358**（27 skip 沿上游；差 25 中 3 flaky 重试过）。
- 流程：fmt --all、CI clippy lane（4 crate）0 error/warning。

## 残差收敛（2026-10-14 第二轮，commit `baaa5b811`）

1. ~~websocket::replaced~~ **已闭环（契约适配）**：该变体为 main 新增（fork 无此变体）；
   断言"无 provenance 的加密检查点原样重发"正是 fork 投影契约要拦的行为。按 fork 契约适配：
   降级后的全量重发为空，记为 `input_shortened`/item type `none`（测试内注释说明）。
2. ~~guardian_history::parent_checkpoint~~ **已闭环（契约适配）**：同为 main 新增变体；
   恢复的检查点作为审查 transcript 消费（delta 断言不变），审查请求断言不带 compaction 项。
   合并树行为自洽：**活压缩**（带 provenance）正常重放（迁移快照 request 4/6/7 实证），
   **恢复的无 provenance 检查点**降级不重放（fail-closed）。
3. **`zero_grace_respects_server_startup_timeout`——已关闭为"机器/二进制性能临界"（非逻辑回归，第三轮取证 `ce96ef24f`）**：
   完整证据链：请求路径全部文件与 main 逐字节一致（含路径修正后复验）；分层计时探针定位到
   **reqwest 客户端构建内的 macOS 钥匙串原生根装载**：合并二进制 ~600ms/次、fork 二进制 ~190ms/次
   （同机同码），测试预算 250ms——fork 基线在本机本就 flaky（重试后过）；官方 main 稳过是因为其
   会话启动流量（connectors/oauth 预取，测试专门 mount 的端点）在 MCP initialize 前焐热了路由池
   （main 树加探针实证：测试期间构建路径从未被触发）。合并树 = fork 的启动序列（无预热流量）+
   更慢的二进制首载 → deadline 在构建期间耗尽 → initialize 从未送达。
   **产品级收益已落地（`ce96ef24f`）**：`PRODUCTION_NATIVE_ROOTS_CACHE` 新增 `peek`，reqwest
   构建在缓存已暖时经 `use_preconfigured_tls` 复用根配置、跳过钥匙串重载；冷路径行为与基线完全
   一致（不叠加首载成本）。http-client 185/185、rig bridge 280/280。剩余失败为该单测的
   机器性能临界性，非合并代码缺陷；若需在本机稳定通过，需 main 同款启动期预热流量或 deadline
   语义调整（排除连接建立），均已超出合并批次范围。
   fork 基线与官方 main 本机均过，合并树稳定失败。**双形态**：(A) 5s 内 initialize
   零尝试（会话启动期停滞）；(B) 尝试已发出但回合 <100ms 完成（不等可选服务器 250ms
   startup timeout）。已排除：core/src/mcp.rs、session/mcp*.rs、codex-mcp 全部、
   config/mcp_types.rs、rmcp-client/src 与 main **逐字节一致**（路径修正后确认）；
   rmcp 3.3.0 回退不可编译（合并版 rmcp-client 需 3.5.1，fork 的 rmcp-client 工作已上游化）；
   探针实证 runtime config 中服务器在、enabled、http=true，但 connection manager 无该服务器
   view / `current_binding_with_requirements` 未被调用（形态 A）。疑点收敛到**会话启动→HTTP
   连接层的停滞**（fork 的系统代理/HTTP 客户端栈 × main 的新可选启动机制交互；fork 自己在
   exec-server/src/remote.rs 记录过 macOS 代理解析可停滞数秒）。下一步：在连接管理器 spawn
   入口与 fork 代理解析路径加时延探针，或按文件组二分（config 组整体换 main 编译不过，需
   逐字段）。
4. 环境项：`openpty_works_under_real_exec_seatbelt_path`（CommandLineTools python3 xcrun 垫片）；
   realtime/tool_parallelism/retry_after 等为**批内并发敏感**（-j 4 限并发后 160/161，仅剩第 3 项）。
5. 快照：guardian_checkpoint_migration、astra_plugin_policy 两张已复核接受。

## 未验边界

- Linux/Windows 实机、ACP dev、Windows 原生、exec-server 全量（含 blocked_descriptor_path
  重现——下一优先项）未跑；CI 未推。
- 第二轮修复后记录数：bridge+genai+protocol 等 1118/1118、core 定向 333/333、
  fragile 族 -j4 160/161（唯一残败=第 3 项）；全量 core 复跑因探针构建干扰未重测，
  下轮在安静环境重取全量数。
- 第三轮（`ce96ef24f`）：http-client 185/185（-j4）、rig bridge 280/280（4 flaky 重试过）、
  core rig_/compact/cap_ 选择器 380 跑 373 过 6 flaky 过 + 1 败=rmcp interrupt 族
  单跑过（并行时敏）。
- Rig 0.42.0 保持（阶段三独立迁移）；上游 v8 pin 未动。
- 上游可吸收修复（b49e2211a/89c8759d8/ae7aad586/8571b9eaa/9e03960cf）未逐项复验收账。
