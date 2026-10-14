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

## 残差登记（合并树新败、fork 基线对照通过——真实回归，待下批）

1. **`responses_websocket_creates_on_non_prefix::replaced`**：第二个 WS 请求 `input: []`
   （应全量重发夹具 Compaction 项）。fork 基线同测试过。疑点：WS 增量/投影与无 provenance
   测试 Compaction 的交互；需在合并树复刻 fork 的存活路径。
2. **`guardian_history_uses_deltas::parent_checkpoint`**：审查请求丢失恢复的父检查点
   （wire 无 compaction 项）。探针证据：恢复信封 `env_provenance=false` → `for_checkpoint`
   无法铸 grant → 投影三条件全 false 剔除；target 完整（provider/model/endpoint/auth_domain/kind
   全 Some）、sources 无该 id。fork 基线同测试过 → fork 的恢复物料化给检查点信封挂过
   provenance，main 的 materialization 重构（thread_history_materialization 30+/13-，I/O
   async→sync 重排）之外某处丢失。**未定位到挂点**，下一步：diff fork 恢复链
   （resume/InitialHistory::Resumed → ContextManager 安装）对信封 metadata 的处理。
3. 环境项：`openpty_works_under_real_exec_seatbelt_path`（PATH 解析到 CommandLineTools python3
   xcrun 垫片，Seatbelt 下写 /tmp 缓存被拒）；realtime/tool_parallelism/mcp-grace 为本机负载
   敏感（单独跑全过，realtime 有 2s 握手 deadline）。
4. 快照：guardian_checkpoint_migration、astra_plugin_policy 两张已复核后接受（合并管线真实输出，
   语义完好——检查点存在、审查流完整）。

## 未验边界

- Linux/Windows 实机、ACP dev、Windows 原生、exec-server 全量（含 blocked_descriptor_path
  重现——下一优先项）未跑；CI 未推。
- Rig 0.42.0 保持（阶段三独立迁移）；上游 v8 pin 未动。
- 上游可吸收修复（b49e2211a/89c8759d8/ae7aad586/8571b9eaa/9e03960cf）未逐项复验收账。
