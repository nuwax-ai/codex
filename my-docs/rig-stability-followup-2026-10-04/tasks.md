# 执行任务（实施与复查记录）

详细行为见 spec.md，技术路径见 plan.md。Claude 实施轮证据保留在下方；Codex 对提交 `11f86b03e` 的复查修复及剩余验收以 `my-docs/codex-11f86b03e-review-2026-10-04.md` 为准。

## A：身份加固

- [x] A1 记录实际 HEAD/工作树，确认 R1–R4 和 Codex 补测在场；禁止覆盖未提交改动。
- [x] A2 endpoint-v2 不包含 query 值，共享 scope 规则替换两份 denylist。
- [x] A3 非空 query 的全部实际值由私有 credential-instance 隔离，账号/匿名/动态凭据分支无绕过。
- [x] A4 extra headers 参与实际 scope，稳定 telemetry 不破坏每轮 cache，rotation 降级 opaque。
- [x] A5 legacy wrapper fail-closed + API/核心实际请求回归 + 源码兼容说明。

## B：session 命令

- [x] B1 名字跨 provider 查找及歧义/分页回归。
- [ ] B2 部分验收（2026-10-05 新机第二轮；2026-10-06 加固证据，进程级 writer/完整行政矩阵仍开放）：异环境 active client enqueue 由 owner 自身环境执行（客户端侧 mock 零请求、config 不变）；daemon 在默认 socket 时非白名单 override 的 embedded 回落被 second-writer 守卫真实拒绝（队列 0、零请求）；owner 首个模型请求被网关真实门控在途时 enqueue 成功、放门后同一 writer 派发（恰 2 请求、owner 凭据、turn_trigger=queue）；无 owner 冷入队由 embedded writer 持久化（不留 socket、不执行、不改 config），后继持环境 owner resume 派发。剩余边界：embedded/daemon 并存场景的进程级 writer 计数（现以"客户端 mock 零请求 + 全部请求同凭据"为观察面）。
- [ ] B3 新行政验收（Codex 第二轮）：archive/unarchive/delete 的 install-method×daemon×env 八组合已补代码；失败不改路径/字节/config、显式屏蔽、同名歧义与 owner 接收 queue 的精确 ID/内容验证。pgrep 不能证明 writer 生命周期唯一，该项继续开放；实际执行数见当前独立报告。
- [ ] 跨进程 opaque 部分验收（2026-10-06）：exec/tests/suite/nuwax_cross_process_scope.rs——双进程+复制 rollout+query/凭据轮换；实证 credential-instance 为进程内随机身份（跨进程同 key 按设计降级，可见历史保留、rollout 只追加）。
- [ ] B3 部分验收（2026-10-05 新机补齐：四命令 corrupted/inactive 已完成，queue/archive/unarchive/delete 循环、坏值具名不回显、inactive 确实抵达远端连接，见 other-computer-validation-results.md 批次 2；install-method/实际行政启动矩阵仍未完成）：active/incomplete 组 fail-fast（既有）+ 新增 corrupted 值组（命名变量、不回显值、连接前拒绝）与 inactive 组（无 NUWAX 环境时确实抵达远端连接）；显式 CLI `-c model_provider` 覆盖时整组忽略（含非法控制值）。install-method/daemon 存在性分支由 daemon_startup.rs 既有矩阵与 tui 策略单测覆盖；`codex archive` 纳入 daemon_startup 命令表未做（登记为剩余项）。
- [x] B4 同 provider echo resume 与无 override 对照，真实 HTTP model 断言。

## C：容器环境变量

- [x] C1 三个可选 retry/idle 变量的规范、解析、优先级和 daemon 子进程剥离。
- [ ] C2 部分验收（2026-10-05 新机补齐：SSE-wait 期 SIGINT 关闭在途请求（exit=1、无 idle 到期文案、恰一连接）与并行 idle 1500/12000ms 行为差分（各自预算失败、时长分化、独立 model/凭据）已完成，exec/tests/suite/nuwax_env_stream_wait.rs 两用例全绿；Windows 原生取消仍未完成）：三协议握手 retries 1/0/default 实际 attempt 数（Anthropic+Responses 补齐，矩阵并发执行）；STREAM 预算独立验证（截断流 resample 1/2/6 次 POST）；短 idle（1500ms）真实触发且 <25s 有界失败；Retry-After 等待期 SIGINT 后零后续 attempt；并行双进程独立 home/模型/凭据/协议/retry/idle 无串用；负数/非数字/i64 溢出/空白/零 timeout/非 Unicode/孤立控制项全部 fail-fast 命名变量不回显值。
- [x] C3 说明文档列清变量名/单位/0 的含义/覆盖关系，无密钥示例。

## D：证据与门禁

- [x] D1 Core rust-rig 的本地验证示例与实际匹配数断言；核对并保留已开启 feature 的 fork CI，不把 0 匹配算通过。
- [x] D2 完成（2026-10-05）：typed `CapExpectation`（Explicit/AnthropicDefault/Absent）× 协议字段（max_tokens/max_completion_tokens/max_output_tokens，Chat 双拼写恰一）；asserted_fields 如实列出 cap 字段；base 已含 `/v1` 与不含两种 path 形态都归一。离线负控全绿。live 新证据：重建 codex-exec（Chat source 97720d19…/binary c17787a4… 与 Anthropic source b3cd797e…/binary c52d6d7c… 两份历史工作树）后 MiMo/GLM × chat/anthropic capped 4/4 PASS——真实厂商 wire 断言 max_tokens=512。原两轮 7/7 不再作为当前验收依据。
- [x] D3 完成（2026-10-05，core/tests/suite/rig_output_cap.rs 7 用例全绿）：Core 公共路径（test_codex 全采样环、request/stream retries 均为 2）下 Chat/Anthropic/Responses 三协议 cap 终止——恰好 1 POST 证明零重采样；partial deltas 保留、零成功 Completed、截断工具不执行、wire cap 值断言（Chat 双拼写）。负控：迟到 finish_reason=stop 帧不再覆盖 length 终止（本轮修复的真实缺陷：sse.rs 首终止优先+丢弃后续终止帧）；Anthropic/Chat 在缺 message_stop/[DONE] 时按截断类可重试（3 attempts 钉契约，与既有 finish_reason_alone_cannot_hide_truncation 契约一致——曾尝试 EOF 合成终局，因与该契约冲突而回退）；length 后停流走 idle 预算有界失败。usage/Done 缺失维持既有行为（不合成成功 Completed），行为边界在验收报告中说明。
- [ ] D4 部分验收：2026-10-05 新机低载冷/热对照历史 6/6 通过；高载 spawn-to-ready、dyld/签名成本及各 RPC 阶段 profiling 未完成。不得把全部 initialize 失败归因负载，不扩大 deadline。2026-10-06 loaded 无订阅用 unsubscribe ACK + loaded/list 断言证明前置状态。
- [x] Bazel 桥长期门禁（2026-10-06 Claude 轮）：`//:enable_model_bridges` flag + core crate_features select + scripts/bazel_bridge_gate.sh（nuwax_env 三线 19/19 真实执行 + 关桥负控零连接，本机 exit 0）+ just bazel-bridge-gate + fork-cargo-pr.yml job（review diff，未 dispatch）。383 例历史失败仅部分抽样/归因；单跑通过和环境相关性不能代表全部关闭——详见 other-computer-validation-results.md 2026-10-06 节。
- [ ] 本地 compact “加密摘要漏戳”归因已撤销；只生成可见 summary。未知 checkpoint 的 client-wide 来源猜测回填已删除，guardian 原 envelope 来源传递及正/负回放测试保留。deferred_executor 有既有 flake 证据，但负载因果和配对 A/B 尚未完成。
- [x] 配对 A/B 执行与状态登记（2026-10-06 第三轮）：基线 worktree 8017fb76c vs 当前树，同 119 集三轮失败集合一致，20 项附环境证据（fake-ip DNS 18+decider、brew cask 1）；当时通过的用例部分在后续 workspace 复跑再次失败，保留波动及窗口边界。撤回 TLS 首建与无 IMDS 两个不当归因。此项仅完成限定集合配对，不代表全部失败根因关闭。
- [x] 同进程独立旋转矩阵（2026-10-06 第三轮）：identical/same-query 正例+query×3/header/endpoint 独立负例（credential 由既有测试覆盖），全绿；Anthropic thinking 降级整块不回线为既有设计（replay_reasoning 需原签名 envelope）。
- [x] writer 生命周期契约+回归（2026-10-06 第三轮）：观测契约（writer=thread 排他文件锁持有者、50ms 探针、per-home 隔离、不可证明项如实登记）+ queue_writer_lifecycle 回归（enqueue 全程探针、退出后锁空闲、goal/set 真实持锁释放）。
- [ ] cap Step0 决策提案已交付（cap-partial-usage-step0-decisions-2026-10-06.md，D1-D5），过审前不实施。
- [ ] D5 当前边界（2026-10-07 独立复算纠正）：完整 workspace 已执行（retries=0/排除 live：21,576 run=18,512 pass/2,060 fail/1,004 timeout/68 skip；fail 含 SIGABRT 1）。原 3,062 名分类记录漏 SIGABRT 且合并跨 binary 同名，按完整身份应为 **3,064 失败 → 恢复 570 → 再恢复 2,165 → 本次仍败 328 + 未复跑 SIGABRT 1**。历史 383 状态逐项复验一致（本次仍败249/首试通过34/两级复跑通过100），其中 **201 项仍未归因、20 项附环境证据、28 项先前 A/B 通过后再次失败**，根因与门禁保持开放。所选79项未发现持续树差异，不能代表全套零回归或证明负载因果。工具 scripts/nextest_log_summary.py、脱敏聚合 logs/nextest-round3-{audit,paired-audit}.json 与报告纠正节可复核。Responses live 触顶、Linux/Windows、加密引用、跨进程 opaque、远程 CI 仍开放；Bazel skills177/177含投毒HOME复跑为macOS证据，行政archive新增单测为限定字段传递证据，D4测量与归因另由独立复审判断。
- [ ] D6 仅设计、未实施、P0 人工复审开放：以修订后的 d6-pause-budget-token-aware-spec.md / plan.md 为准，LegacyBytes 或完整实际载荷/framing 的 ExactTokens/ProvenUpperBound，KeepWholeOrFail。禁止 bytes/4、经验分位数充当证明；未经产品裁决不启用 token 硬限，不改 40,960-byte fail-fast、不删块或截断签名、不改写旧 rollout。


## 本轮证据（2026-10-04，Claude 实施轮）

| 批次 | HEAD/源码摘要 | 命令/feature/target | selected/executed/asserted | pass/fail/skip/timeout | 首轮失败与复验 | 工件路径 | 未验证边界 |
|---|---|---|---|---|---|---|---|
| A/B/C 联合离线 | ead6f2bca+R1-R4+Codex 测试+本轮 A-D 改动（工作树） | `just test -p codex-rust-rig-bridge -p codex-utils-cli -p codex-config -p codex-app-server-daemon -p codex-api -E 'not binary(exec_live)&not binary(bridge_live)' --offline --locked --retries 0 --test-threads 2`，target=/tmp/codex-stability-20261004-target | 953 run；断言 v2 摘要/私域比较/benign 表/新控制变量/daemon 12 行剥离 | 951 pass / 2 fail / 2 skip | daemon 2 项失败=测试脚本行数计数未随 4 新变量更新（13→12 修正）；修后单跑 PASS | 无独立工件（nextest 摘要） | 完整 workspace 未跑 |
| rig-bridge 全量（含 endpoint-v2 回归） | 同上 | `just test -p codex-rust-rig-bridge …` | 244 run（含 chat/anthropic 触顶 2 新 wire 回归 + query-shape 重写回归） | 244 pass（终轮） | 首轮 1 fail：`reasoning_source_includes_query_routing…` 钉 v1 值分区语义 → 重写为 v2（值不分区、名分区）；二轮 1 fail：`unsuccessful_terminals…` 钉旧聚合 Stream → 按因分类更新 | — | — |
| tui B 批 | 同上 | `just test -p codex-tui -E 'test(named_session_lookup)…'`（实际过滤 named_session_lookup/session_archive/queue_owner） | 12 run：跨 provider All 过滤+歧义、queue owner 预检矩阵（daemon 拒绝/embedded/remote/named 放行） | 12 pass / 0 fail | — | — | — |
| app-server+exec | 同上 | `just test -p codex-app-server -p codex-exec -E '…echo…/nuwax_env/build_receipt' --test-threads 1` | echo resume 双 fixture（bare 恢复 gpt-5.4 + echo 取当前默认 + wire [5.4,5.4,5.2]）；exec 重试 env 三态（1/0/默认×stream=0 → 2/1/2 请求） | echo 1 pass(1 flaky 冷启动)；override 1 pass(1 flaky)；exec retry 1 pass（40.4s）；首轮其余 10 pass | 首轮 3 fail：thread_resume×2=冷启动 initialize（try2 热身后过，载入路径语义分歧已登记）；exec retry=retries=0 被 Core 采样重试掩盖 → 测试固定 NUWAX_STREAM_MAX_RETRIES=0 隔离握手层 | — | — |
| D1 core 单包 | 同上 | `just test -p codex-core --features rust-rig -E 'test(rig_anthropic)|test(rig_responses_bridge)|test(model_output_projection)|test(client)' --retries 0 --test-threads 2` | **258 selected**（非零=feature 生效） | 255 pass / 3 fail（rmcp_client 三项=隔离 target 缺 test_stdio_server 辅助二进制） | 补建 `cargo build -p codex-rmcp-client --bin test_stdio_server` exit 0 后 `test(rmcp_client)` 58/58 pass——两段证据合覆盖 258 | — | — |
| live-tests lib（D2 单测） | 同上 | `just test -p codex-live-tests -E 'not binary(exec_live)&not binary(bridge_live)'` | 35 run；capture 证据断言 wire 字段 | 35 pass / 0 fail | 首两轮 1 fail=unit fixture 期望未随 asserted_fields/field_mismatch 更新 + 假捕获 URL 不匹配预期——改 fixture 后全绿 | — | — |
| 最终 live（收尾后） | fmt 后最终树 | `env CARGO_BIN_EXE_codex-exec=… LIVE_VENDORS=mimo,glm just test -p codex-live-tests --test exec_live -E 'test(/^(mimo|glm)_(chat_rig|anthropic_rig|responses_rig_default)$/)|test(glm_websearch_anthropic_rig)' --retries 0 --test-threads 1` | 7 selected/executed；逐 marker 真实工具闭环 | **7 pass**（run ID 0dcff6f8，308.8s） | — | logs/live-{glm,mimo}/*（本轮） | Step/genai/加密 citation 未跑 |

**D2 live 证据升级（本轮实测）**：manifest `source_validated=true`、`status=source_and_package_validated`、source sha `29f96536…`（与最终树一致）；`request-capture-evidence.json` 首次出现 **`wire_asserted: true`**（asserted_fields=[url_prefix, body.model]），GLM websearch attempts=2。鉴权头仍按规范只由离线 wire 测试证明，不在 live 取证落盘。

行为边界登记：
- **已加载线程的 echo override 保留内存模型**（fresh-load 路径才应用当前默认）：thread_processor 加载路径与已加载 merge 路径不一致；本轮不改生产 merge 逻辑，登记为后续决策项。
- D4 冷启动复验：load≈28 时仍失败（唯一失败项 10.6s initialize deadline）；未出现真低载窗口，D4 保持未完成。
- D5/D6 未动。


## 拆分轮证据（同日追加）

| 批次 | HEAD/源码摘要 | 命令/feature/target | selected/executed/asserted | pass/fail/skip/timeout | 首轮失败与复验 | 工件路径 | 未验证边界 |
|---|---|---|---|---|---|---|---|
| 拆分后 bridge+live lib | 拆分后树 | `just test -p codex-rust-rig-bridge -p codex-live-tests -E 'not binary(exec_live)&not binary(bridge_live)' --retries 0 --test-threads 2` | 279 run | **279 pass / 0 fail**（拆分前后用例总数一致：244+35=279） | — | — | — |
| 拆分后 core 套件 | 同上 | `just test -p codex-core --features rust-rig -E 'test(rig_anthropic_hosted_tools)|test(rig_anthropic_identity)|test(rig_anthropic_credential)'` | 9 run | **9 pass**（1 slow） | — | — | — |
| 拆分后最终 live | fmt 后最终树（source sha `22ff9289…`） | 同前 7 场景命令 | 7 run | **7 pass**（1 slow） | — | logs/live-*（本轮） | 同前 |
| 收尾 | — | scoped `just fix -p codex-rust-rig-bridge -p codex-core -p codex-live-tests` exit 0；`just fmt` exit 0；`git diff --check` 干净 | — | — | — | — | fmt 后未重跑离线测试（live 为源绑定验证非离线回归） |

### 拆分明细（fork 自有大文件 → 模块化，均低于 ~730 行）

- `stream.rs` 851→630：泵（事件转换/hosted 恢复/pause 捕获）抽出为 `stream_pump.rs`（307 行，`PumpContext` 显式传参；`PAUSE_CONTINUATION_LIMIT` 随迁，`map_completion_error` 升 pub(crate)）。
- `tests/wire/hosted_tools_wire_tests.rs` 1687→目录模块 `wire/hosted_tools/`：common（SSE fixtures+capture 回放 harness，pub(crate) 供兄弟模块）/request_translation（声明侧）/item_mapping/replay（持久化回放门控）/pairing（去重/上限/opt-out）/late_results（迟到/混合/引用大场景）。
- `live-tests/src/binary_turns_tests.rs` 963→目录模块 `binary_turns_tests/`：common（FakeRunner+Fixture harness）/retention/runner_scenes/capture_pipes。
- `core/tests/suite/rig_anthropic_hosted_tools.rs` 939→675 + `rig_anthropic_hosted_tools/support.rs`（289：mock 网关 responder/序列/SSE 块构造器，供 identity/credential 子套件复用）。

## 每批证据模板

| 批次 | HEAD/源码摘要 | 命令/feature/target | selected/executed/asserted | pass/fail/skip/timeout | 首轮失败与复验 | 工件路径 | 未验证边界 |
|---|---|---|---|---|---|---|---|
| 待填写 | | | | | | | |

## 完成边界

小批相关测试通过后 scoped just fix、just fmt、diff-check；fmt 后不再重跑测试。只有本批行为及其回归确实完成才勾选。不要自动 commit/push/发布，不清理 SQLite/tmp/.env.local，不盲跑付费矩阵。

## 2026-10-05 Codex 复查补充

- 当前完整结果及跨电脑入口：my-docs/codex-review-push-2026-10-05.md、my-docs/other-computer-handoff-2026-10-05.md。
- 新机（第二台电脑）2026-10-05 独立复验与本轮补齐（B3 四命令、C2 SSE-wait/并行 idle、另一连接 echo 对照、Chat live cap 触顶、D4 低载 6/6、11 场景 live 全绿）：my-docs/other-computer-validation-results.md。
- 同日第二轮（用户授权"macOS 可测全部"）：B2 owner/writer 矩阵补齐（异环境 active client、第二 writer 守卫、running owner 真实 gate 派发、冷入队持久化，7/7）；npm_nuwax install-method 单元格；**登记行为分歧：最后订阅者断开后 loaded-list 仍在但 echo override 已走 fresh-load 路径**（2026-10-06 已确认是明确 cache-entry 契约，补 ACK/loaded 状态断言）；Anthropic live cap 触顶 2/2；workspace 全套首次表征（21,178/21,561，383 例失败/超时尚未全部定性）；**Bazel 首跑抓到并修复三处 fork 构建图缺陷**（两桥 crate 缺 BUILD.bazel、exec build.rs `#[path]` 源缺失、reqwest_rig 重命名 aliases 未接线），`//codex-rs/exec:exec` 与 `//codex-rs/cli:codex` exit 0。详见同上结果文档。
- controls 新文件原为 8 个测试，9/9 计数含原 nuwax_env.rs 的 Chat retry 用例；新增 remote 文件是 4 个测试，报告表旧数 6 不准确。
- 604 历史最终批未执行 nuwax_session_remote；capped live4 来自两份源码，合 8 次捕获。源码/收据/原始失败以可提交脱敏摘要为据。
- D6 已修订：禁止以 bytes/4 或分位数系数关闭 P0，也不能将现行超限 fail-fast 改成未经裁决的 DropOpaque/DropAll。

## 2026-10-06 独立复审与后续任务

用户本轮已授权修复与阶段 commit，不授权 push/发布。新增确认证据及完整命令见 other-computer-validation-results.md 的 2026-10-06 节；后续开发按 ../claude-code-followup-2026-10-06.md 执行。不得将历史测试数、live 收据或 Bazel build 成功替代当前代码的公共请求路径验收。

## 2026-10-06 Codex 第二轮复审

当前基线8017fb76c，阶段保存与实测见 other-computer-validation-results.md 新增第二轮节。后续按 ../claude-code-followup-2026-10-06-round2.md；新增 cap Spec/Plan 与 D6 均未实施，不能勾选产品能力。

## 2026-10-07 完成路线

以最新独立复审和 claude-code-completion-plan-2026-10-07.md为准：2个实质功能（cap、D6）与6个验收/稳定性包。writer loaded-owner正校准已补、Anthropic11场景实际wire仍待本轮执行；旧HOME canary根错误已纠正，D4采样不等于真实warm/根因闭合。未批准cap方案、P0 token证明、201根因、平台/厂商/CI均不勾选。

## 2026-10-07 晚间轮（包3/包1推进）

- 包3进展：249 仍败项签名分组完成（13 组，`my-docs/validation-2026-10-07/pkg3-failure-signatures-worklist.md`）；13/13 失败代表在独立 t1 窗口再次失败，不能据此排除时序/负载因素。R1 机制证据：fork 默认桥接策略把测试 mock provider 判为 bridged → Guardian V2 评分/native Responses WebSocket 被禁用（uses_model_bridge → async_scorer/extension.rs:89 警告）。fixture 钉 `experimental_bridge="native"`（app-server/tests/common/config.rs + attestation.rs raw 配置）后，app-server 切片 101 项累计 **79 恢复/22 开放**（77+attestation×2）；3 金丝雀再次通过。初始验证的 8 PASS = 5 个失败代表恢复 + 3 金丝雀，不是 8 个新增恢复。SIGABRT 项当窗单跑 2/2 PASS，后续模块并发仍失败（见工作清单）。标注文件 77 行恢复+13 行 R1b 开放说明，不能混计。工具/日志：兄弟 codex-tmp/pkg3/。
- 包1：cap partial/usage 具体实施提案交付（`my-docs/cap-partial-usage-implementation-proposal-2026-10-07.md`：真实插入点 diff、CapExhausted 载体、CapPartialEvent/item、ContextualUserFragment 注入、有界策略数值表、D1-a..D4-b 七个决策点），等裁决后按批实施。
- 包6平台确认：OrbStack 在位（orb/orbctl 可用），Linux 侧本地执行可排。

## 2026-10-07 深夜续轮（用户指令"继续开发"）

- **R4 已修（产品代码）**：fork 扩展字段 `provider_id`（load 时填充、参与派生相等）在 `check_thread_model_provider` 的 requirements raw 对比路径未填充 → 组织要求未变也误判 definition_changed、-32600 误拒（上游 #45517 前提破坏）。修复=检查侧非 Bedrock 分支按同键补填。验证：4 集成（enforcement×2+model_list×2）+5 单测+2 相关套件全绿（pkg3/final2/r4-*.log）。app-server 切片累计 **83/101 转绿**。
- **R1b 深挖收窄未收口**：机制链全映射（select_parent_compaction→复核线程 InitialHistory 种子→池路由两分支）；实测 checkpoint item 在 sync review input 完全缺失；两候选（父历史无 item/复用路径无种子）待诊断输出定案。**新发现：修复后该家族双态**——首跑 5.1s 快败断言、后续 3/3 挂起于 index==0 且 Luna 请求数 0（Luna websocket 采样不稳，独立嫌疑）。测试诊断增强已留在 guardian_v2_history_tests.rs。
- **R6 排除桥接家族**：TUI agents_overview 等在 280843aae 已钉 native 仍独立 60s 超时 → 根因独立。
- R2/R7/R9/R10、SIGABRT 栈溢出机制定位、包 4/5/6/8 当窗未动，待下批。

## 2026-10-07 深夜续轮二（用户指令"全部开发完"）

- **R2 已修（22 项）**：fake-ip DNS 依赖注入——策略层 `NetworkProxyState.host_lookup_fixture` + 连接层 `StateDnsResolver`（macOS connector）；`.invalid` 保 NXDOMAIN、`localhost` 保原生双栈、其余主机名固定公网映射；三处测试构建点注入。network-proxy 全 crate 复验 313/314→修复 v6 委托后待 final3 确认；22 行标注已更新。core decider 1 项跨 crate 注入需另设计（开放）。
- **R7 收敛未修**：分段计时+mock 探针证明卡点在 `RouteAwareClientPool` 发送路径（对本地零延迟 mock 间歇 >1s/挂起，connect_timeout 与外层期限放宽到 5s 时 14/16 转绿；不可放宽=削弱断言）。另发现 stalled-body 401/403/404 不从状态行分类（503 正常）。bail 诊断增强已留（`{error:?}`）。
- **R1b 两种失败形态已观察，根因未定**：快败态为 checkpoint 断言（父历史无 item/复用路径无种子两候选）；挂起态的测试诊断停在等待答题后第二次 Luna 采样，sample 捕获子进程 stdin read。该栈可由测试未关 stdin 解释，不能单独定位采样机制或排除 shutdown 问题。临时探针已清。
- **包 4 首切片完成**：`admin_cli_flags_reach_real_config_loading`（PASS）——strict-config 决定格+回退格、profile-v2 文件机制负/正格；实证语义：主配置 `[profiles]` 表=加载失败、未知 provider 恒硬错误。剩余 --oss 格、组状态格、owner crash/restart。
- R9/R10 未动（R10 疑与 R7 同根：codex-http-client 首请求/TLS 初始化）。

## 2026-10-08 凌晨轮（用户指令"R9/R10/R6 + 包 4 剩余格"）

- **R10 暖态目标切片通过，冷态归因开放**：三段计时 execute=8.33s、sample 捕获 `SSLCreateContext`/dispatch_once 栈，支持 TLS 冷态初始化慢路径候选，不能把 execute 全耗时定为平台初始化。测试前本地 native-tls 握手预热与服务器 EOF/read-timeout 处理变更后，`r10-verify4.log` 实际 **9 项=8 PASS（TLS×7+aws-auth×1）+1 core retry telemetry FAIL**，整批不全绿；原 249 中相应 5 个历史项获暖态恢复标签。AWS IMDS 为纯 HTTP，冷态失败根因尚未证实，不能声明生产无缺陷。aws-auth 增 native-tls dev-dep/Cargo.lock；按规则仍需复核 Bazel 锁文件是否需更新。仅 `TlsConnector::new()` 的第一版预热未改善结果。
- **R6 产品补丁与限定切片已验证，语义复审开放**：sample 观察 file-watcher 在 async 线程同步调用 macOS FSEvents `unwatch` 并阻塞，60s 窗口超时；未证明无限阻塞。当前补丁将 `RecommendedWatcher` 移专职线程、async 侧提交命令。验证：file-watcher 22/22、TUI lifecycle TIMEOUT→PASS 19s、**agents_overview 名称过滤切片 71/71**（849.7s：63 项在 `app::agents_overview::tests`，8 项在模块外）；不是模块全集或 TUI 全 crate 门禁。需复审异步 watch 生效、失败簿记、后端积压/阻塞后可用性。R1 fixture 模板行与 TUI 三处显式 experimental_bridge 的重复键已清（realtime_handoff 修复后 PASS）。
- **包 4 --oss 格完成**：`admin_cli_oss_flag_routes_provider_selection`（PASS）——隐式 oss_provider 选择、--local-provider 未知 provider 硬失败/已配置 provider 成功三格。包 4 剩余：组状态格（active/incomplete）、owner crash/restart/队列持久化。
- **R9 卡点候选未修**：sample 捕获 sqlx SQLite `ConnectionWorker::establish` 的 async pending/worker flume recv；R6 补丁后复测仍败 10.463s。sqlx 唤醒/通道语义是候选，尚未定位确切等待原语，也未证明与 R6 不同根因。
- R7（route-aware 纯 HTTP 间歇慢）未动，与 R9 可能同域待查。

## 2026-10-08 独立复审口径纠正

- historical 恢复标签 **124 行 = 原 249 项新增恢复 115 + 此前已通过的 9 项再次通过**；原 249 项仍开放 **134**。124 的分组为 R1 79 + R4 4 + R2 22 + R10 5 + R6 14；R1b 13 项虽带说明仍失败，不能计恢复。R10 标签仅表示预热后的暖态观察。
- SIGABRT 的已证事实为单跑 2/2 PASS、guardian 模块 8 并发下四次 SIGABRT（默认 8MiB 三次、16MiB 一次）。无界递归、任意栈预算均耗尽、任何全套必败均未证实；保留实际失败与根因候选，完整 workspace 门禁仍开放。

## 2026-10-08 Codex 独立复审追记

- R4内建bridge-only归一化/retained endpoint公共3例通过，最终provider enforcement 7/7；R2两族fixture与cfg补齐，network-proxy319/319（macOS）；Linux/Windows实际执行仍待补。
- R6修订为desired/active分离、一个wake、失败重试、独立readiness和changes-only订阅；watcher30/30正常Bazel通过，public fs strict真实通知证据见独立报告。后端永久卡住及平台/cancellation资源验收仍开放。
- R10正常Bazel TLS7/7+AWS1/1；Cargo同暖态8项4PASS/3FAIL/1TIMEOUT。环境差异和冷态根因仍开放，不能按旧暖态标签关闭生产门禁。
- 本轮所有问题/首败/复验/身份、用户授权的阶段commit与push记录：`../codex-independent-review-2026-10-08.md`；另一台电脑工作令：`../other-computer-development-prompt-2026-10-08.md`。此前“不得自动commit/push”指旧轮范围，本轮用户明确授权commit/push；没有授权发布。


## 2026-10-09 轮（10-08 工作令：R7 定案修复、R1b 判别、SIGABRT 定位）

- **R7 已修（产品级，18/18 转绿）**：四层取证链——单例 6/6 复现 → 三段计时（route/permit 瞬时、闭包体 12.28s）→ 活体采样完整栈（reqwest build → hyper_util from_system → **SCDynamicStoreCreateWithOptions → _SC_getApplicationBundleID**）→ minirepro 对照（主线程 7ms = 无 CFRunLoop 辅助线程上的系统代理查询阻塞十余秒）。修复：`resolve_proxy_route`/`resolve_proxy_route_async`（outbound_proxy.rs）对字面量 IP/localhost 目标返回 `Direct`（arm-2 Legacy 臂按 build_route 同步），使构建跳过系统代理加载。同时是产品缺陷修复（macOS 经池访问本地模型服务首请求可被阻塞数秒）。验证：registration_retry 18/18（8.7s，原 1/18）；regression 切片 419/428，9 失败甄别：3=既有 TLS 冷态（与 Codex Cargo 基线一致）、1=watcher 负载 flake（单跑 0.35s 过）、1=cert-classify（补共享 macOS 预热 test_warmup.rs 后过）、1=managed-timeout 断言改 Direct（新语义）、1=legacy-fallback arm-2 修复后过、mitm CA 复跑过。
- **R1b 判别定案（候选 B 证实）**：快败实例 `parent_input_types` 含 compaction item → 父历史有 checkpoint、复核请求缺 → 投递链截断。首次修复尝试（fork snapshot 追加 envelope）不足已回退；下步追 subagent 请求组装剥离点。
- **SIGABRT（R11）栈定位**：溢出线程阻塞于 `rustls_native_certs::load_native_certs → TrustSettings::iter`（每 websocket 连接同步加载钥匙串信任设置）+ 重试链 future 增长；直接 cargo（无 8MiB 注入）时 17 个 guardian 测试 2MiB 瞬时溢出=大 future 直接证据。修复方向：native certs 进程缓存+spawn_blocking；重试包裹加 Box::pin。
- 15 个 guardian 模块 60s 超时（8 并发争用）与 R9 sqlx pending 维持开放。本机新事实：SCDynamicStore/TrustSettings 系统服务调用在本机无 runloop 线程上可阻塞 10s+，所有"本地 mock 间歇慢"族先查此模式。

## 2026-10-09 续轮（缓存修复 + 回归）

- **NATIVE_ROOTS 进程缓存**（custom_ca.rs LazyLock）：websocket 连接免每回钥匙串往返。SIGABRT 30.1→15.2s（仍溢出：增长型栈为主因，TrustSettings 为延迟放大器）；15 并发超时不变。http-client+websocket-client 回归 143 run 137 过；6 失败=4 已知 TLS 冷态+2 新发现 dialer 传输类型断言（本机既有，非缓存致因，开放）。

## 2026-10-09 最终轮（dialer 修复 + 卷恢复后终验）

- **dialer 修复**：R7 直连使 localhost 路由 TransportDefault→Direct → dialer Left→Right → 测试断言失败。修复：Direct 无代理时保持 TcpStream 不装箱直接 `client_async_tls_with_config` → Left 传输（nodelay 检性保留）。139/143（dialer×2 过，仅剩 4 已知 TLS 冷态）。
- **卷断开恢复**：/Volumes/soddygo 曾在测试中弹出（工作树完好，cargo lock 已清）。
- **最终组合验证 19/19 全 PASS（22.9s）**：registration_retry 全族 17 例（0.014-0.565s）+ relay 2 例 + guardian_ephemeral 单线程 14.1s。
- 最终树：10 modified + 1 新文件（http-client 5 + websocket-client 1 + test_warmup + 4 文档）；fix 0 警告、fmt、diff-check 全过；未 commit/push。


## 用户要求先保存WIP：Codex初步复核覆盖说明

当前Claude代码按用户最新指令先commit/push，尚未修复或独立验证。代理短路/redirect、全局证书根缓存、非macOS warmup import有确认问题；R11实际线程固定4MiB，R1b下游剥离与R7因果尚未定案；19组合=core1+exec16+relay2，119仅账面待完整身份复算。请以 `../codex-pending-review-2026-10-09.md` 和 `../other-computer-development-prompt-2026-10-09.md` 为最新交接，不将本页历史“已修/全绿/定案”升级为验收。
