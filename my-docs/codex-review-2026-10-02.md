# R1–R5 独立复查、修复与后续任务

日期：2026-10-02。起始分支 `test`，HEAD `f9c560ed66a6ca14280ef6a1c023e3ed11bc14c1`，起始工作树干净。
审查范围：上一轮 F 修复已分批提交；本次重点为 `f29e8b83d..f9c560ed6` 的 R1–R5 实现、当前调用链、测试及原始证据。

结论：三协议路由、版本化 envelope、remote config 字段及 schema 已接通，但不能把“各批已提交/既有套件全绿”视作 R1–R5 完整验收。配置来源隔离、普通/混合轮历史投影、generic provenance 和 executable 构建证明仍需继续开发。

## 发现与本轮直接修复

以下原始坐标以审查 HEAD 为准；本轮修复保留在工作树，测试结果见后文。

| 编号 | 级别 | 原始坐标与触发条件 | 本轮处理 |
|---|---|---|---|
| F01 | P1 | `config/src/env_group_isolation.rs:28` 用 merged provider 优先；实际 Core 为 requirements→typed→config，导致选中 provider 与隔离对象不同 | 校验移至最终 provider 选择后，纯函数接收已选择 ID；真实优先级矩阵 |
| F02 | P2 | `exec/src/lib.rs:353`、`tui/src/startup_orchestration.rs:88`、`app-server/src/lib.rs:534` 将整表 seed 放在显式 subkey 后，静默覆盖冲突；doctor 次序相反 | 在 seed 构造前拒绝保留表、子键和含保留 provider 的父表；不改用户文件；实际 binary 零请求回归 |
| F03 | P2 | `cli/src/doctor.rs:657` 用 any() 而非最后一次 CLI 选择判断来源，非保留 CLI provider 也被标为文件/default | 来源行按实际采用的 seed 与最后显式选择推导；不宣称完整 per-field provenance |
| F04 | P0 上下文复审 | `codex-rust-rig-bridge/src/hosted_replay.rs:122`、`hosted_tools.rs:415` 只给 blocks 计数，任意 cited_text 可绕过字节/块类型限制 | 完整 envelope（含 cited_text/source/version）在 anchor 前有 40,960-byte 上限；引用块类型与 citations array 校验；发射侧同样检查 |
| F05 | P1 | `hosted_replay.rs:122` 同组每个 pair 都复制全部 cited_text | 每来源组仅复制一次；普通 Message 与 cited text 的全局重复仍见 N2 |
| F06 | P1 | `request_messages.rs:79,190` 标记 duplicate id 后丢弃所有 completed；foreign/legacy/bad completion 可抑制有效 pending | 先做同源/版本/形状/预算校验，再按具体历史项索引选择首个有效 completed |
| F07 | P1 | `request_messages.rs:218` 在 sanitize 前构造缺 name 的 SDK anchor；result-only 直接报 InvalidRequest，违背 S3 降级 | 共享 validator 在创建 assistant/anchor 前执行；损坏导入不进入 SDK |
| F08 | P1 | `stream.rs:684` pending 配对没有来源、形状或预算检查，并读取 replay=false 前的原输入；旧 call 被重新标为新来源 | 从已验证的实际 replay blocks 和本轮 raw pause 消息建立 pending map，按 call/result 顺序关联实际内容；不反查旧历史或重新标源 |
| F09 | P2 | `hosted_tools.rs:218` 正常 capture 重建 type/id/name/input，丢 vendor 扩展字段 | 保存原始起始块，仅更新 input 增量；真实 capture 回归 |
| F10 | P2 | `hosted_tools.rs:271`、`hosted_replay.rs:318` 未处理 SDK 已支持的 citations_delta，正文与签名初始值也可被空累加器覆盖 | 按 index 收集 citation delta；保留并追加 inline text/thinking/signature；完整对象断言 |
| F11 | P2 | `stream.rs:518` mixed follow-up 的引用固定传空 Vec | 引用交给实际匹配的补发完成条目；引用边界与位置仍需 N2/N3 |
| F12 | P1 | `transport.rs:299` 暂停替换最后一个历史 assistant；thinking-only 后面仍有 user 时覆盖旧回答，连续暂停又可能覆盖先前暂停 | 记录原始 input 边界，转换前只使用原前缀，按顺序追加每次暂停的完整 raw assistant；不替换旧消息 |
| F13 | P1 | `hosted_replay.rs:318,367` 未知 delta、损坏 input、截断 capture 静默降级后仍续接 | 暂停完整重建返回 Result；缺 stop、未知不可重建 delta、损坏 JSON 明确失败，不续发部分消息 |
| F14 | P1 | `stream.rs:600` continuation setup/first-event await 未监听父 receiver 关闭 | setup 和转发均 select 父 channel 关闭；第二请求等响应头时取消的 socket 回归 |
| F15 | P2 | `live-tests/build.rs:11` 只 watch `.git/HEAD`，普通 commit 改 branch ref 不改 HEAD 文本 | 经 Git 解析 HEAD、symbolic branch 和 packed refs 路径，兼容 worktree |
| F16 | P2 | `live-tests/src/artifacts.rs:46` 将 harness 收据与独立 exec hash 混为构建证明 | receipt 明示 HARNESS；exec source/features 为 unknown，不能据此认定新鲜 |
| F17 | P2 | `live-tests/src/binary_turns.rs:605` 忽略 copy 错误，并检查临时原件 contains，而非复制后的真实 envelope | 传播 copy/read/JSON 错误；解析 retained response_item/web_search_call v1 source/blocks |
| F18 | P2 | `binary_turns.rs:454` kill 直接 child 后无限等 pipe EOF；后代持有 pipe 时失败现场永不落盘 | 有界 drain，保留部分 stdout/stderr，未收完明确失败；后代 pipe 离线回归 |
| F19 | P2 | `artifacts.rs:69` 忽略 manifest 写入失败 | 返回 Result，所有调用点传播错误 |
| F20 | 测试 | `pause_turn_tests.rs:231` 对 direct body 的 `body.tools` 做 null==null；`:295` thinking-only 仅绑定 messages | 改为非空真实 tools 深比较；多轮旧历史与连续 thinking pause 的完整 messages 深比较 |
| F21 | P2，本轮再审 | 本轮新增 late-result 补发分支再次解析 pause SSE，而 capture 已给当前首个搜索对保存相同 cited_text；当前 B + 迟到 A 同轮出现时引用重复 | 迟到结果不再二次领取已分配的引用；新增 actual HTTP 回归在修复前准确失败，修复后结果见验证表 |

补充修复：第二次 pause 的迟到结果也从实际 pending map 补发 completed，避免只存 in_progress；raw pause 每个完整 assistant 消息限40,960 bytes，整个 replay+pause 请求调用合计限64，超限明确失败，不截断签名或密文。局部字节上限仍不是精确 token 保证。

F04 控制的是字节和形状，不是精确 token 计数。>1K token 路径已经在本次上下文审查中标为 P0；仍需要下面 N5 的统一模型预算设计，不能把字节/4 宣传为严格 10K token 保证。

本次未提交改动涉及配置/诊断、replay 验证与去重、pause/cancellation、evidence 四类。累计改动已超过单批指导，后续提交应继续细分到复杂批次 <500 changed lines，普通非机械批次 <800。共享文件需要分块暂存，保持每批可构建；目前尚未暂存，不重写已提交历史。

## 仍需 Claude 开发的任务

### N1 真正的临时 provider 来源隔离（P1，先做）

坐标：`codex-rs/config/src/env_group_isolation.rs:44`、`app-server/src/config_manager.rs:446`。
四键白名单只证明键名，不证明来源。standalone app-server 已有合法环境组后，thread/start.config 可覆写 `model_providers.nuwax_env.env_key` 或 base_url，仍通过校验，从而读取另一变量或把 NUWAX_API_KEY 发到新 endpoint。

保留 seed 的来源身份，在配置合并前拒绝文件/profile/managed/CLI/thread 的任何非 seed 贡献，包含同名四键。不要继续扩大黑名单或静默删除配置。增加真实 app-server JSON-RPC 零请求回归：endpoint 覆写、env_key 覆写、线程恢复/fork 覆写、managed 覆写和非 adopted 环境组。普通 --oss 诊断和 endpoint 的精确来源也仍需完善。

### N2 普通轮引用投影与正文位置（P1）

坐标：`convert_response.rs:128`、`transport.rs:346`、`hosted_tools_wire_tests.rs:1034`。
真实完成轮保存 Message 的 plain answer，再把同样文本放进 envelope.cited_text；续轮目前得到 plain answer 与 cited answer 两份。F05 只修 per-pair 复制，没有修这里。现有 wire 测试手工仅回放 WebSearchCall，丢了实际 Message，不能证明完整路径。

确定稳定的 response/block 位置载体，用 cited 终态替换对应文本投影而不是尾部再追加。保存普通 Message 不需要被改写；请求投影应保留旧前缀、文本出现一次、引用关联正确。真实事件累积→下一请求整体 messages 深比较，覆盖多条 search、多文本、未决/完成混合和过滤超预算。

### N3 混合轮保持历史前缀（P1）

坐标：`request_messages.rs:191,239`。
原始顺序 `[client call, pending server call, client output, later result]` 收到完成条目后，投影删除旧 pending，完整 call+result 被搬到后面的 assistant。rollout 追加不代表模型上下文前缀没有变化。当前混合测试手工重建请求，第三请求移除了 client call/output，掩盖了生产顺序。

设计追加 result-only 的投影或带 response boundary 的持久化，使旧 call 留在原位置，迟到 result 留在新响应位置。不得以“去重成功”改变旧请求前缀。需要真实 Core 工具闭环后的 requests[1]/requests[2]、保存恢复和缓存前缀证明。

### N4 generic provenance 与真实 Core 矩阵（P1）

坐标：`core/src/context_manager/history.rs:584`、`session/model_history.rs:79`、`core/tests/suite/rig_responses_bridge.rs:139`。
model_output_provenance 仍只有生产 writer；for_prompt 丢弃 envelope metadata。新 hosted source 约束了这一载荷，却未完成一般 provenance 投影。B5 是 Responses synthetic envelope 的保存/清理，不是 Anthropic capture→save→resume/fork→actual HTTP replay；没有 fork 调用。

让 provenance 进入投影决定，并定义 provider/endpoint/model 与必要授权域的身份边界。补同源、换模型、换协议、换厂商、旧裸数组、unknown version、损坏载荷和 fork 的真实 Core 场景。仍不伪造签名/密文，不改旧 rollout。

### N5 统一完整载荷预算与取消验收（P0/P1）

40,960-byte 是已明确的保守 envelope 策略，不能替代精确 token 硬上限。暂停 raw content 已加入字节/调用数 fail-fast；tee 8 MiB 只是捕获缓冲上限。新增模型可见 raw block 必须有单项和整体请求边界、来源策略与超限行为；无法保真时应明确失败，不截断密文。模型相关预算先独立 Spec/Plan。

桥 drop/socket 回归仍不能替代 Core Op::Interrupt、工具开始/结束、超时/重试和取消后工具不重复执行的集成验证。Windows/Linux 和 remote exec 验证分别登记，不能由 macOS 单测推定。

暂停续接的 usage/request ID 当前明确采用最后请求的 Completed 数据，先前 attempt 的数据并未汇总成最终事件。后续规范需要区分最后请求的上下文占用与全轮累计计量，规定缓存/输出计量及每次请求 ID 的保留方式，避免直接累加所有 input_tokens 后误报上下文大小。此处是仍需明确验收的统计边界。

聚合引用的剩余限制：两个各自合法、约24KB引用的 envelope 归入同一 assistant group 时，sanitizer 将约48KB组引用算给各pair，可能同时降级。引用所有权与位置需随 N2/N3 重构，不能声称只丢超限单对。

### N6 executable 构建收据与真实请求证据（P2）

现在明确区分 harness 与 codex-exec。还需由 **exec 本身**提供 source SHA、dirty tree/源码指纹、features、target/profile 与 binary hash 的可核验绑定，并在执行前验证。运行时 git HEAD、mtime、harness receipt 都不能补足这个关系。

实际旧 GLM 工件 `logs/live-glm/websearch-websearch-anthropic-rig-1790870203146862000-7306/manifest.json` 记录 `6d2d97be6` + dirty tree，exec hash 为 `ecf0f7066f1645786896e88f5cedf614c2dcd5d5786bea50589d1301bc4039fb`，不是已证明的 f9c560ed6 最终 HEAD 构建。其确实证明两轮 exit 0、搜索 completed 和两个 v1 envelope；`/tmp/t06-glm5.log` 为 1 PASS/124 nextest skipped、72.910s。

R5 “出站 body 必须 MITM”不是唯一技术选择。RigHttpClient 已持有最终请求，可以添加测试用、脱敏的 request recorder，捕获 SDK/transport 转换后的 body；凭据/header/query 不进入工件。这能和 Core 保存的原始块深比较。受支持厂商做最小 live，其他厂商能力和 pause 未触发继续标记 not-run。

失败现场仍有缺口：当前 retained rollout 复制只在两轮成功后执行，失败时临时目录中的 rollout 会被清理；stdout/stderr/exit 工件已保留，但还需在失败路径复制部分 rollout。成功路径的至少两个有效 completed pair 也不能代替逐轮来源匹配与实际出站 body 断言。

### N7 CI、Bazel、schema 与交付门禁（验证缺口）

remote proto 21/22 optional 字段及 false/None round trip 未发现新格式问题；JSON/TS/Python 的 wire_blocks schema 已生成。历史日志 `/tmp/r5-proto4.log` 是 6 PASS/309 skipped，`/tmp/r410-full.log` 是 168 bridge PASS，不是本轮执行。

桥/live-tests 仍无 BUILD.bazel，live-tests.yml 未 dispatch，npm 三平台未验。README 如实登记这些边界是必要文档修正，但不能替代实际 PR/offline/feature/platform 门禁。建立可跑 Cargo workflow，提供实际 run 链接；发布另行授权。

## 历史变更规模问题

两项 P2 可维护性问题，不要求重写历史：

- R1 `b00c9ce02` 共 822 行；手写非文档 779 行。URL 校验（`utils/cli/src/nuwax_env.rs:223`）可先独立落 61 行；加载隔离+config/exec 回归 343 行、core 来源矩阵190行、doctor来源142行、daemon44行分批。
- B1 `eff9c9d6c` 共680行。`hosted_replay.rs:37` 来源 envelope/gate 与`:98` pair sanitizer 是不同责任，可按 source+wire 回归→budget/shape/malformed回归拆分；后者消费已验证的 blocks。

其余 B2–B5、R4、R5 批次已符合规模指导。总 delta 3122 行中包含214文档、37生成/lock及两个二进制 schema，不能全部解释成新增 runtime 逻辑。

## 本轮验证证据

统一 cwd=`codex-rs`，`CARGO_TARGET_DIR=/tmp/codex-stability-20260930-target`，`CARGO_BUILD_JOBS=4`；just test、--offline、--locked、--retries 0。

| 验证 | 日志与结果 |
|---|---|
| 原始 HEAD baseline：bridge/config/utils，test-threads=4 | `/tmp/codex-review-20261002-baseline.log`：exit0，578 run/578 pass，1 skipped；没有证明新增缺陷不存在 |
| 修复后的 unit/offline evidence | `just test -p codex-rust-rig-bridge -p codex-config -p codex-utils-cli -p codex-live-tests --lib --offline --locked --retries 0 --test-threads 4`；`/tmp/codex-review-20261002-unit.log`：exit0，530 pass，1 leaky，1 skipped；pipe 测试补 owned descendant teardown 后在下面 bridge/evidence 复验中 PASS，无 LEAK |
| bridge 第一轮 actual HTTP | `just test -p codex-rust-rig-bridge --offline --locked --retries 0 --test-threads 4`；`/tmp/codex-review-20261002-bridge.log`：exit100，179 run，178 pass，1 timeout。初始化超出外层guard且server等未发出的accept；加client warm与accept释放后在下面复验中 PASS |
| bridge/evidence 完整复验（F21 前） | 命令见下方；`/tmp/codex-review-20261002-final-bridge-evidence.log`：exit0，187 run/187 pass，122 skipped，210.592s；无 LEAK/TIMEOUT |
| Core / exec / doctor 定向 | 命令见下方；`/tmp/codex-review-20261002-startup-core-final.log`：exit0，12 run/12 pass，5625 skipped，26.074s；首次编译发现新测试误用了不存在的 mock API，修为 received_requests 后重跑 |
| F21 新增边界回归 | `just test -p codex-rust-rig-bridge --offline --locked --retries 0 --test-threads 1 -E 'test(paused_current_and_late_search_results_have_one_citation_owner)'`；`/tmp/codex-review-20261002-citation-owner-red.log`：exit100，1 run/1 fail，180 skipped，2.079s，失败为重复 cited_text 的完整对象断言；`/tmp/codex-review-20261002-citation-owner-green.log`：exit0，1 run/1 pass，180 skipped，2.098s |
| scoped fix / Clippy | `just fix -p codex-rust-rig-bridge -p codex-config -p codex-utils-cli -p codex-live-tests -p codex-core -p codex-cli -p codex-exec --features codex-core/rust-rig --offline --locked`；`/tmp/codex-review-20261002-fix.log`：exit0。原有 PauseCapture 文档列表告警已修正；`live-tests::spawn_exec_turn` 的存量 9 参数告警仍保留。F21 完成后 `just fix -p codex-rust-rig-bridge --offline --locked`；`/tmp/codex-review-20261002-final-bridge-fix.log`：exit0，无告警、无自动修改 |
| fmt / diff-check | `UV_CACHE_DIR=/tmp/codex-review-20261001-uv-cache just fmt`：exit0；`git diff --check`：exit0。最后格式化未扩散到无关文件；按仓库要求，最终 fix/fmt 后未重跑测试 |

```sh
CARGO_TARGET_DIR=/tmp/codex-stability-20260930-target CARGO_BUILD_JOBS=4 \
just test -p codex-rust-rig-bridge -p codex-live-tests --offline --locked --retries 0 --test-threads 2 \
  -E 'package(codex-rust-rig-bridge) | test(binary_turns::tests) | test(pending_search_map)'

CARGO_TARGET_DIR=/tmp/codex-stability-20260930-target CARGO_BUILD_JOBS=4 \
just test -p codex-core -p codex-cli -p codex-exec --features codex-core/rust-rig --offline --locked --retries 0 --test-threads 2 \
  -E 'test(nuwax_env) | test(doctor_sources) | test(doctor_resolves_environment) | test(responses_bridge_resumes_history)'
```

本轮不自动 commit/push。未运行完整 workspace、Linux/Windows、真实厂商、CI dispatch 和发布；最终测试计数必须按实际执行填写，不能用过去记录回填。AGENTS.md 要求 tests→fix/fmt，之后源码复核而不重跑测试。

## 下一轮 Tasks 与提示词

- [ ] T01 先完成 N1：seed 来源身份与真实 app-server 隔离，不接受键名白名单代替来源；同时先明确 N5 的载荷预算 Spec，不把上下文复审留到交付末尾。
- [ ] T02 N2/N3 合并先写 Spec/Plan，保存 response/block 边界、正文去重、原前缀稳定；再做真实 Core 工具/恢复矩阵。
- [ ] T03 N4/N5：generic provenance 消费、整体载荷预算、取消/不重复工具与跨平台边界。
- [ ] T04 N6：exec 本身的 receipt、新鲜度校验、最终 HTTP recorder；重建明确产物后最小 GLM live。
- [ ] T05 N7：可执行 CI/feature/schema/Bazel 范围与实际证据；同步已完成和仍未验的口径。

```text
请先读 AGENTS.md 和 my-docs/codex-review-2026-10-02.md，核对当前 HEAD 与未提交修复，保留并适配本轮改动。

先修 N1 的真实 seed 来源隔离，并尽早定义 N5 的预算规范；再为 N2/N3 的引用/混合历史边界写独立 Spec、Plan、Tasks 并逐批实现。配套完成 N4–N7。不要继续以键名白名单、手工重建的桥请求、PASS 总数或 harness receipt 代替验收。

每批补真实配置/API/Core/HTTP 回归，深比较实际 requests、事件与原 rollout 前缀。用 just test、隔离 target、exec 构建收据，相关离线验证后再最小 GLM live。缺平台/配置/未触发 pause 标记 not-run。完整 workspace 依 AGENTS.md 单独确认。

持续更新 Tasks 的精确命令、退出码、有效执行/跳过/失败数、工件与构建身份。复杂批次 <500 行，可独立构建；不伪造密文、不重写旧历史、不自动 push 或发布。
```

协议依据：[Anthropic Streaming](https://platform.claude.com/docs/en/build-with-claude/streaming) 与 [Server tools](https://platform.claude.com/docs/en/agents-and-tools/tool-use/server-tools)。内容块按 index 累积至 stop；暂停保留完整内容；mixed server/client 结果按 id 跨响应关联。
