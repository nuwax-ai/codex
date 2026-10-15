# 给 Claude Code：Rig 生命周期证据与图片入口下一轮

请直接执行以下任务，分批验证、分批 git commit 保存，最后自查并交 Codex 复审。

## 目标、起点与边界

核心目标是让 fork Codex 通过 `NUWAX_*` 环境变量和 Rig 使用国内／自定义模型，
长会话中的工具、截图、恢复与压缩可靠。优先 Linux app-server，实际客户端是
`/Volumes/soddy/git_workspace/fork-codex-acp-ts` 的 `dev`；其 main 用来同步上游。

先核对两处 git status/log，保留并行改动：

- 主工作区 `/Volumes/soddy/git_workspace/fork-nuwax-codex` 的 test 在交接时为
  `698824464`，仍 Rig 0.42。不要在这里误以为迁移已生效。
- 当前候选 worktree：
  `/Volumes/soddy/git_workspace/fork-nuwax-codex/.claude/worktrees/merge-main-20261014`，
  分支 `codex/rig-044-20261014`，代码修复检查点 `0b71e182e`（另有后续交接文档提交）。
  从其最新 Codex 复审提交继续，不能从
  `bc4930f27` 丢掉后续修复重新开始。
- main `cb03f28a4` 已通过 `481a3f485` 合并进候选；Rig 已固定 0.44.0，
  图片 Phase A wire 预算修复已在候选中。不要重复 main merge、依赖迁移或
  Base64 预算修复。后续上游有新增提交时独立评估，不混入本批图片逻辑。

先读 `AGENTS.md`、`codex-rig-044-review-2026-10-15.md`、
`rig-044-migration-notes.md`、`claude-image-budget-followup-2026-10-14.md`
第二／三批，以及 `results-wire-image-budget-2026-10-14.md`。上述文档位于
`my-docs/rig-production-completion-2026-10-10/`。

本轮授权离线开发、本机与此前给出的 Linux/Windows 机器定向验证、阶段提交。
不 push、不手动 dispatch、不发布包、不运行付费／真实厂商 API，不复制聊天
中的凭据入库或日志。完整 workspace 继续留到 CI／另行确认。官方 Codex 与
Rig 参考仓库只读；不改 V8 pin，不操作用户已处理的 artifacts 迁移。

## 第一批：把 Rig 0.44 的生命周期证据补准确

1. 先复用已存在的显式 Rig 测试：`core/tests/suite/rig_responses_bridge.rs`
   的 resume、Chat compact 超窗恢复，以及 `rig_anthropic_hosted_tools.rs`、
   `rig_anthropic_identity_tests.rs` 的保存／恢复／fork／来源轮换。列出准确
   nextest 选择器、features、实际执行数与未执行条件。不要重复造现有测试。
2. 之前通用 resume/compact 281 与 env 62 默认走内置 OpenAI，不能冒称走
   Rig。新收据必须显示 `experimental_bridge=rig`、非内置 provider 配置、
   当前候选二进制路径与 hash、三协议实际 loopback endpoint 非零命中；
   不能只看 provider display name 或编译时启用了 rust-rig。
3. 对当前确实未覆盖的三 wire 热恢复／销毁进程后冷恢复／local compact
   做最小补充。保留完整 call/result 配对、usage 与 host key、取消、来源
   切换、v1/v2 reasoning。Anthropic 签名从真实 rollout 读回后第二请求仍
   有效；切 provider 不带旧 opaque 块；旧 rollout 不原地改写。
4. 离线 v2 cassette：用本地 Chat／Anthropic SSE 和实际 recorder 录制单
   attempt，经 `save_rig_event_fixture` 写入临时文件，重新加载后回放，
   对比完整事件、usage、签名／native fingerprint 和工具 ID。只归一化生成
   的 item ID 与测试 endpoint source。保留旧 v1 fixture，新文件独立命名。
   覆盖 reasoning→文本／工具、迟到 usage、cache 与 cap/filter/unknown
   finish；不要手写新格式 fixture 充当“实际录制”。
5. cassette 单 attempt 限制单列。pause 多 attempt 当前不能由共享 recorder
   正确回放；不把旧限制说成迁移新回归，也不静默宣布支持。若本批触及，
   先加明确拒绝或 per-attempt 格式的独立小补丁；HTTP/idle/truncation/
   pause/hosted 注入仍用真实 wire 测试，不靠 cassette PASS 代替。

完成后单独提交验证和必要修复，再进入下一批。

## 第二批：MCP／Code Mode 图片入口硬限（Phase D）

先画清以下实际链路，选择最靠近共同入口的已有边界实施，避免多层传参数：

- `protocol/src/models.rs` 的 `CallToolResult::as_function_call_output_payload` /
  `convert_mcp_content_to_items`，`core/src/mcp_tool_call.rs`，
  `core/src/tools/handlers/{mcp,mcp_resource}.rs`。
- 当前 MCP manager 实际文件是 `codex-mcp/src/connection_manager.rs`。
- Code Mode 的 `code-mode-runtime/src/runtime/{value,callbacks}.rs` 与
  `cell_actor/conversions.rs`；图片准备复用 `core/src/image_preparation.rs`、
  `utils/image` 和 `utils/output-truncation`。以上路径相对 codex-rs。

实现和验收要求：

1. 图片必须保持 typed image，不把任意 JS 值或 resource blob 的 Base64
   当普通文本送给模型。分别验证 MCP image、resource、Code Mode `image()`、
   普通 JS return 与显式 `text()`；不要从任意字符串猜图片，也不擅改普通
   structuredContent 优先语义。混合 content 的现有接口兼容合同要保留。
2. 在解码前限 encoded bytes；解码时限 decoded bytes／pixels／内存；
   重编码后限单图 bytes；另限单工具图片数量、aggregate bytes、metadata
   与序列化总量。用具名 policy／常量和边界错误，先写清额度再实现。现有
   1GiB sanity、2048px 缩放或最终 32MiB 请求限均不能代替入口多重硬限。
3. 文本＋视觉成本共同遵守单条工具输出 10K token 上界，不可文本接近10K
   后再无限加图片。新模型可见 >1K 片段按 AGENTS P0 登记具体格式与上限供
   人工复核；复用 ContextualUserFragment 规则，不先塞大段说明占上下文。
4. 处理 invalid Base64、缺／错 MIME、超大尺寸、解压炸弹、多图和损坏数据。
   模型不支持视觉时返回有界、可理解的错误，不能悄悄删图或输出原始二进制。
   浏览器截图和用户参考图片不要不加区分地裁剪／压缩／淘汰。
5. 三 wire 捕获最终请求，证明图片落入真正 image 字段，正文没有 Base64
   文本副本；保留 call/result 顺序与来源。负控普通 text 中伪 data URL 仍
   计文本；超过上限在 HTTP 派发前拒绝，attempt 与日志策略仍有界。
6. 保留 Phase A 的完整 wire 字节硬限、输出预留、URL/file-id 非零视觉成本。
   独立检查 2MiB request capture 上限与 HTTP attempt 预算耦合：有界诊断
   不能阻止合法大图，也不能无限抬高日志限；发请求前的预算计数不能移后。

先以不改变历史生命周期的入口硬限小批交付，复杂逻辑每批尽量 <500 changed
lines，普通批 <800；不要一次把连续截图压缩和 provider 上传平台塞进来。

## 第三批：连续截图与原生平台验收

1. 至少20次合成截图的真实 Core／app-server 回合，三 wire × 热／冷恢复
   × local compact。提前于压缩请求自身超预算触发正式 compaction 边界，
   不在每次 sampling 临时删除旧图或重写历史。保留必要最新图片、call ID、
   来源／标签与账目，验证压缩后下一请求和冷恢复仍可继续；图片成本与输出
   reserve 一起计量。Guardian 4图／8MiB、remote-v2 retained64K 局部规则
   不能直接冒充普通 Rig 对话策略。
2. Linux `192.168.32.163` 主验；Windows `192.168.32.235` 用原生二进制，
   WSL 结果单列。凭据只用现有安全会话配置，不写交接或仓库。每机固定源码
   SHA、toolchain、features、二进制路径／hash、receipt 与执行数。暂不可达
   记未验并继续其他独立工作，不能用 Mac PASS 代验平台。
3. ACP dev 最小联调固定 `CODEX_PATH` 指向本候选对应二进制，验证环境变量
   传递、截图、取消、冷恢复及压缩。发现 ACP 独立问题再在其 dev 单独提交；
   不切 main，不把“看到了文件”算作“模型识图成功”。
4. 新 guardian 溢出／超时等失败需在相同平台、工具链、features、栈与选择器
   下做候选/基线对照；不要只引用旧 ledger 宣称无关。失败日志保留首败，
   不用加大栈、放宽 timeout 或测试里早退掩盖产品缺陷。

默认继续有界 inline image。厂商 File API／附件上传只作为后置 opt-in 设计，
先核实指定 provider 的真实支持、文件作用域／TTL／删除／冷恢复／换账户、
取消和失败回退；没有支持证据时不盲目新建云存储。真实识图和付费录制待
后续指定 vision 模型与请求预算后单列，不阻塞上述离线开发。

每批按 tests → scoped `just fix` → `just fmt` → diff-check，最终 fix/fmt 后
不重复测试。配置／API／依赖／编译数据变化同步 schema／TS／Bazel。最终
报告提交 SHA、已执行与内部跳过的数量、首次失败、修复、未验边界和下一批。
候选验收后再安排合回 test；本轮不 push，不回滚／强推／重写旧提交。
