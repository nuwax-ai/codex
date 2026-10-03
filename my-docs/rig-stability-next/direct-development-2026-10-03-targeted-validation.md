# 2026-10-03 定向验证记录

## 源码与范围

- 检查点 HEAD：`20898140f`，工作树含本轮未提交实现与测试修复。
- 编译器失败修复：Core 两个 sibling 测试文件显式导入 `pretty_assertions::assert_eq`，解除九处宏解析歧义。
- 仅定向包和过滤器；未运行完整 workspace 测试。fmt/fix 和最终 live acceptance 由主任务最后统一处理。
- 实际结果在命令结束后填写；构建失败不计为测试通过。

## Core / AppServer / Exec

cwd：`codex-rs`

```sh
CARGO_TARGET_DIR=/tmp/codex-stability-20260930-target CARGO_BUILD_JOBS=4 just test \
  -p codex-core -p codex-app-server -p codex-exec \
  -E 'test(rig_anthropic_hosted_tools) | test(rig_responses_bridge) | test(model_output_projection) | test(stream_events_utils) | test(nuwax_) | test(build_receipt) | test(listen_off_honors_persisted_remote_control_enable) | test(config_manager)' \
  --offline --locked --retries 0 --test-threads 2
```

- 第三轮：编译失败（exit 101），九处 sibling `assert_eq` 宏歧义，日志 `/tmp/codex-oct03-core-app-exec3.log`。
- 第四轮：编译成功，nextest exit 100；114 实际运行、112 通过、2 失败、6888 按过滤器排除。run ID `7d871014-f308-4c33-ac99-900eae42add9`；日志 `/tmp/codex-oct03-core-app-exec4.log`。
- 两项失败分别为：嵌套 Seatbelt 的 `sandbox_apply: Operation not permitted`（工具 exit 71，未开始）；凭据轮换 fixture 在 text start frame 直接填正文而没有 text delta，typed stream 没有产出对应可见 Message。
- 修复仅将该正文 fixture 改为 empty start + text delta，完整下一请求 messages、签名/密文/引用、轮换投影和 append-only 断言全部保留。
- 独立本机允许执行环境复跑这两项：编译成功、nextest exit 0，2/2 通过、7000 按过滤器排除；run ID `712ed503-a842-4331-9933-3ddde8fcc3ca`，日志 `/tmp/codex-oct03-core-tool-credential-unsandbox.log`。这属于不同执行环境的明确复验，不能写成第四轮单次 114/114。

独立复验命令（通过工具的 `require_escalated` 在外层 Codex sandbox 之外执行，未手工改写 sandbox 环境变量）：

```sh
CARGO_TARGET_DIR=/tmp/codex-stability-20260930-target CARGO_BUILD_JOBS=4 just test \
  -p codex-core -p codex-app-server -p codex-exec \
  -E 'test(anthropic_interrupt_during_tool_runs_it_once) | test(anthropic_actual_credentials_keep_signed_replay_and_rotation_drops_only_opaque)' \
  --offline --locked --retries 0 --test-threads 2
```

## Bridge / API / Config / History / Live 支撑 / Model Provider

cwd：`codex-rs`

```sh
CARGO_TARGET_DIR=/tmp/codex-stability-20260930-target CARGO_BUILD_JOBS=4 just test \
  -p codex-rust-rig-bridge -p codex-api -p codex-config -p codex-history \
  -p codex-live-tests -p codex-model-provider \
  -E 'not binary(exec_live) & not binary(bridge_live)' \
  --offline --locked --retries 0 --test-threads 2
```

- 编译成功，nextest exit 0；964 实际运行、964 通过、0 跳过（1 slow），run ID `09785b59-42a6-4ede-944d-1813027e52d7`。
- 日志 `/tmp/codex-oct03-l1-final-validation.log`；按 binary 过滤器排除 `exec_live` 与 `bridge_live` 两个真实 provider suites。
- 当前源码最后一次变更仅为 Core 的文本流测试 fixture 修复；此轮未修改生产源文件、fmt 或 fix。

## 证据边界

- 通过 nextest 过滤器选择、实际执行、assert 到 provider request / item / history、跳过以及失败，分别记录。
- nested Seatbelt 工具场景必须使用独立允许执行的本机环境验证，不能用零请求或工具未启动证明取消成功。
- 本地 loopback / 离线回归不会替代真实 MiMo/GLM 网关、CI、Bazel 或跨平台验收。

## 实际日志计数

### core_app_exec4

| 包 | 状态 | 数量 |
|---|---|---:|
| codex-app-server | PASS | 67 |
| codex-core | FAIL | 2 |
| codex-core | PASS | 36 |
| codex-exec | PASS | 9 |

实际出现 PASS/FAIL 的 binary IDs：5 个（按 binary + test name 去重，末尾失败摘要不重复计数）。

- `codex-app-server`
- `codex-app-server::all`
- `codex-core`
- `codex-core::all`
- `codex-exec::all`

### core_unsandbox

| 包 | 状态 | 数量 |
|---|---|---:|
| codex-core | PASS | 2 |

实际出现 PASS/FAIL 的 binary IDs：1 个（按 binary + test name 去重，末尾失败摘要不重复计数）。

- `codex-core::all`

### l1

| 包 | 状态 | 数量 |
|---|---|---:|
| codex-api | PASS | 209 |
| codex-config | PASS | 367 |
| codex-history | PASS | 35 |
| codex-live-tests | PASS | 27 |
| codex-model-provider | PASS | 103 |
| codex-rust-rig-bridge | PASS | 223 |

实际出现 PASS/FAIL 的 binary IDs：12 个（按 binary + test name 去重，末尾失败摘要不重复计数）。

- `codex-api`
- `codex-api::clients`
- `codex-api::models_integration`
- `codex-api::realtime_websocket_e2e`
- `codex-api::realtime_websocket_tls`
- `codex-api::sse_end_to_end`
- `codex-config`
- `codex-history`
- `codex-live-tests`
- `codex-model-provider`
- `codex-rust-rig-bridge`
- `codex-rust-rig-bridge::wire`


## 第一轮真实厂商验证（修复后还需定向复验）

- 命令：`env CARGO_BIN_EXE_codex-exec=/tmp/codex-stability-20260930-target/debug/codex-exec LIVE_VENDORS=mimo,glm CARGO_TARGET_DIR=/tmp/codex-stability-20260930-target CARGO_BUILD_JOBS=4 just test -p codex-live-tests --test exec_live -E 'test(/^(mimo|glm)_(chat_rig|anthropic_rig|responses_rig_default)$/) | test(glm_websearch_anthropic_rig)' --offline --locked --retries 0 --test-threads 1`。
- run ID `8d63092c-73ed-44a3-9d67-a8f2643f911a`；日志 `/tmp/codex-oct03-live-final.log`；nextest exit 100。7 selected/executed，5 PASS、1 FAIL、1 TIMEOUT；另 24 项按过滤器排除，未执行。
- 通过：GLM 三协议 marker，MiMo Chat/Responses marker。每项实际工具 exit 0 + marker 验证，源码/包级配置收据通过；依赖 feature 图仍 unknown。
- GLM websearch FAIL：第一轮真实上游返回 3 个 `web_search_prime`，input 为 `search_query`，匹配 result 内容为合法 string；生产桥支持该 tool family，harness 却硬编码 `web_search`。保留的三次请求中后两次已原样重放这三个调用对。此项不能登记成功，正在修复校验器并补回归。
- MiMo Anthropic TIMEOUT：不是网络不可达。原临时 home `.tmpcEFwKE` 有真实 `exec_command` marker（exit 0），02:21:20.963Z assistant、02:21:21.492Z task_complete、02:21:21.683Z last_message。外层 nextest 到约 02:22:21Z 才终止。manifest 在场景开始约 178s 后落盘、首请求再约 114s 后；505,179,560 bytes debug executable 的 opt0 software SHA 是已确认的性能热点；现场全部额外延迟尚未在隔离 benchmark 中复现，不能全部归因于 hash。runner 在 posthash 前未保存 pipes，外层终止造成 artifacts 不完整。
- 根因修复：保持完整前后 SHA 校验；优化 dev SHA backend、先保存已完成 pipes/exit；两轮 compact/search 的外层预算覆盖 300s × 2 内层 turn timeout + drain/preflight，不再用单轮 360s。

### 实际出站字段核对

安全汇总脚本 `/tmp/codex-oct03-audit-captures.py` 对上述七个目录验证 POST、原始 JSON 可解析、stream=true、manifest model 一致、无 headers/URL userinfo、query values 全脱敏；capture 为最终发送前 attempt，不宣称厂商必然收到每次 attempt。

| 厂商 | 路径（实际请求） | attempts | 观察到的续轮字段 |
|---|---|---:|---|
| GLM Anthropic | `/api/anthropic/v1/messages` | 2 | 第二 body: thinking+非空 signature、tool_use、tool_result 各 1；max_tokens=16384 |
| GLM Chat | `/api/coding/paas/v4/chat/completions` | 2 | function tool call 与 tool result |
| GLM Responses | `/api/v1/responses` | 2 | reasoning、function_call、function_call_output |
| MiMo Anthropic | `/anthropic/v1/messages` | 2 | thinking、tool_use、tool_result；本次无 signature，nextest 仍 TIMEOUT |
| MiMo Chat | `/v1/chat/completions` | 2 | function tool call 与 tool result |
| MiMo Responses | `/v1/responses` | 2 | reasoning、function_call、function_call_output |

此处核对出站协议和字段存在；签名/密文/引用全部字节保真由严格 mock/wire/Core 用例另行证明，未观察到的真实厂商 capability 不能冒充通过。

### SHA 性能复核

只优化 sha2 的 package dev opt-level，流式算法、64 KiB buffer、SHA-256 和所有前后检查保持不变。隔离 probe 使用实际 live fingerprint 对应 rLib、同 rustc 1.95 和真实 binary prefix：

| 测量 | 耗时 |
|---|---:|
| opt0，8 MiB（二次） | 0.289623 / 0.291247s |
| caller opt0、sha2 opt3，8 MiB（二次） | 0.025121 / 0.026411s |
| sha2 opt3，505,179,560 bytes 完整 binary | 1.927752s |
| sha2 opt3，8639 source inputs / 85,935,702 bytes | 2.399428s |

8 MiB 摘要相等，完整 binary 摘要与 Python hashlib SHA256 相等。约 11.3 倍是本机隔离吞吐对照，不能据此推定所有平台或将现场全部 preflight 延迟解释成 SHA。日志：`/private/tmp/codex-oct03-sha-probe/{actual-live-debug-results.json,optimized-results.json}`。

## 最后源码冻结后的整合与隔离复验

整合：9 个明确包（Core/AppServer/Exec/API/Config/History/Live/ModelProvider/Rig）；Core/AppServer/Exec 使用下面筛选，其余包排除真实厂商 binaries。命令环境沿用 `/tmp/codex-stability-20260930-target`、build jobs=4、offline/locked、retries=0。

```text
(package(codex-core) & (test(rig_anthropic_hosted_tools) | test(rig_responses_bridge) | test(model_output_projection) | test(stream_events_utils)))
| (package(codex-app-server) & (test(nuwax_) | test(listen_off_honors_persisted_remote_control_enable) | test(config_manager)))
| (package(codex-exec) & test(build_receipt))
| (not package(codex-core) & not package(codex-app-server) & not package(codex-exec) & not binary(exec_live) & not binary(bridge_live))
```

- 本机允许执行环境，`--test-threads 2`；run ID `ce32e024-289c-42e4-9d69-7b045bda513f`。
- `/tmp/codex-oct03-final-related-validation.log`，exit 100；1083 run / 1075 PASS / 8 FAIL / 6898 filter-excluded。没有 nextest TIMEOUT。
- 新增 pipe cap 5 项全部通过（包含真实 subprocess 提前停止、自然非零 exit 主错误）；新增 auth normalization 3 unit + 3 wire 通过；signed-query Core/API 回归通过。
- 8 FAIL：2 Core citation 用例的 10s common event wait Elapsed；6 model catalog 请求在 5s 总预算中 timeout，一项明确 0 HTTP 请求。认证 matcher 已排除：Core Mock 只检查 POST；同路径的 identity/mixed/pause/credential 其余用例通过。

同一包/feature 选择图下仅调整 nextest 测试并发为 1，未修改源码、mock 条件或超时阈值：

```text
test(anthropic_cited_replay_keeps_repeated_text_citation_owner)
| test(anthropic_cited_answer_appears_once_with_citations)
| (package(codex-model-provider) & test(models_endpoint))
```

- `/tmp/codex-oct03-timeout-isolation.log`，run ID `b0489293-0720-472b-862b-48c83a1a608c`，exit 0。14 run / 14 PASS / 8082 filter-excluded。涵盖全部 8 个失败案例及其它目录检查。
- 同类目录请求在并发轮耗约 6.4–7.8s；单线程约 3.5–4.5s。5s 预算包含 transport/client 构建与 HTTP。并发计时敏感得到复验支持，尚未将全部底层 IO 原因归到具体目录或证书库。
- 不能写成单次 1083/1083。最终覆盖为整合通过 1075 项 + 失败项在隔离轮通过；同时保留本机并发计时风险。

| 包 | 整合 PASS | 整合 FAIL | 隔离 PASS |
|---|---:|---:|---:|
| API | 209 | 0 | — |
| AppServer | 67 | 0 | — |
| Config | 367 | 0 | — |
| Core（筛选） | 33 | 2 | 2 |
| Exec（收据筛选） | 3 | 0 | — |
| History | 35 | 0 | — |
| Live 支撑 | 35 | 0 | — |
| Model Provider | 97 | 6 | 12 |
| Rig bridge unit+wire | 229 | 0 | — |

此轮以后仅 API auth kind 的文档注释补充 `credentialInstance`，不改运行逻辑。fresh exec build/最终真实请求和工具收尾另列。

## 新 exec 下的真实请求第二轮

- fresh Exec build：`/tmp/codex-oct03-final-source-exec-build.log`，exit 0，13m19s；receipt `/tmp/codex-oct03-final-source-exec-receipt.json`。源码内容摘要 `d96aa78cfadcfda8c5c4127353fb6f7b6de22bed2f04a098fc9225a1f489e5e1`，HEAD/target/profile 与前述一致，dependency features unknown。
- `/tmp/codex-oct03-final-live-two.log`：2 run / 1 PASS / 1 FAIL / 29 filter-excluded，exit 100。
- MiMo Anthropic PASS 24.603s；artifact `logs/live-mimo/mimo-anthropic-rig-1791001062025874000-33403`。真实工具完成和 marker、正常回答、两次 HTTP attempt；源码/包配置收据通过。
- GLM websearch FAIL 49.954s；artifact `logs/live-glm/glm-websearch-anthropic-rig-1791001012028909000-24759`。第一轮一对合法 prime search/result，第二轮只有 reasoning/回答，没有新搜索调用；不是跨 response call-ID 去重误计。原 matched-pair 断言正确拒绝。
- 两次请求仍声明 hosted web search，tool_choice=auto，两个 CLI 均 exit 0。首轮普通 assistant 的 969 UTF-8 bytes / 633 chars 与第二请求历史逐字一致。新进程 credentialInstance 改变，两个旧 opaque carriers 按规范降级；这不算字段误删。
- 没有观察到新 search，因此不能说第二轮服务端搜索已验证，更不能用 curl 或回答非空替代。仅强化 harness 的第二提示为必须先新服务端搜索、禁止历史/shell 替代，保留严格 pair 断言，计划一次限定复验。

## lint 收尾前补修

只读 scoped Clippy：`/tmp/codex-oct03-scoped-clippy-readonly.log`，exit 101。1 个 denied redundant_clone（测试 fixture 最后一次 clone），6 处 warning（question-mark、range pattern、collapsible-if、let-and-return）。已手工做等价修改；`/tmp/codex-oct03-lint-delta-tests.log` 正在相关逻辑验证。此后才执行最终 required scoped fix/fmt，不在最终 fix/fmt 后重跑测试。

### lint 等价修改后的回归

- `/tmp/codex-oct03-lint-delta-tests.log`，run ID `82c74fd5-2e75-4590-a80b-f0b8c6a8980c`，exit 100：80 run / 77 PASS / 3 FAIL / 5167 filter-excluded。新增过滤器含 model_output_projection/startup_prewarm、live 支撑全 lib、Rig request_capture/wire_budget/auth_normalization；保持原 9 个包的构建图。
- 三项失败均为隔离 target 缺少 `test_stdio_server`，进产品逻辑之前明确报 binary 不存在；不是字段转换或 WebSocket 断言失败。
- 补构建 `codex-rmcp-client` 的 `test_stdio_server`（exit 0），再仅复跑三个 prewarm 场景；`/tmp/codex-oct03-finalizer-helper-rerun.log`，run ID `9e970efb-cec6-4b69-b590-379fccb945f8`，exit 0，3/3 PASS / 8093 filter-excluded，42.088s。
- 源码、模拟响应、原连接/复用断言未改；两轮证据合并覆盖这 80 项，不写成单次 80/80。

## 强化指令后的唯一 GLM 复验

- fresh Exec build exit 0（4m34s），本轮 pre-fmt source SHA `18afe06a6c5a15504455d138f7a5ab98290c8a9773e6af276edc6cb42de5ba39`。
- 单项 `glm_websearch_anthropic_rig`，LIVE_VENDORS=glm，retries=0 / threads=1，仍通过 `just test`。日志 `/tmp/codex-oct03-finalizer-glm-live.log`。
- run ID `c68299d4-a1de-47e7-a17f-d09262e49d69`，exit 0；1 selected / 1 PASS / 30 filter-excluded，69.950s。
- 工件 `logs/live-glm/glm-websearch-anthropic-rig-1791003794045654000-32760`，receipt/2 HTTP attempts/两个 exit 0/rollout/search-evidence 均留存。
- 独立工件检查：turn1=1 matched completed call/result，turn2=3 新增 matched completed call/result，总计 4；ID 严格对应，prime/search_query/string result 保留真实 shape。
- 首轮普通正文在第二 body 逐 UTF-8 bytes 保留：659 chars / 1041 bytes。冷进程旧 opaque 未发送，如规范。
- citations=0/0，citation_capability_asserted=false；不能升级为真实加密引用字段验收，也不能称跨冷进程 opaque replay 验收。JSON 中 wire_asserted=false 继续保留，单独列明这次人工结构/字节核对。
- 先前 auto 不搜索 FAIL 完整保留；强化提示提高本次调用选择概率，不提供未来所有模型确定性服从保证。没有无限重试或放宽 pair 断言。

所有测试结束后才执行 required scoped fix/fmt。最终源内容若因格式化变化，将只重建/读取 receipt 证明最终编译；不在 final fix/fmt 后重跑测试，亦不将 pre-fmt 真实请求等同 final SHA 的实测。

## 最终工具和源码绑定

- required scoped `just fix`：`/tmp/codex-oct03-finalizer-scoped-fix.log`，exit 0（27m24s）。两测试文件的三处等价 lint 修正，补丁 `/tmp/codex-oct03-finalizer-fix.patch` 已人工核对。
- `just fmt`：`/tmp/codex-oct03-finalizer-fmt.log`，exit 0。70 Rust 文件字节差异为格式化及上述测试等价修正；递归 pause future 的 `impl Future + Send` 和 `manual_async_fn` allow 保留。
- 最终 `cargo build -p codex-exec --bin codex-exec --offline --locked`：`/tmp/codex-oct03-finalizer-final-exec-build.log`，exit 0，8m34s。
- 最终收据 `/tmp/codex-oct03-finalizer-final-receipt.json`：完整 HEAD `20898140f2b3638aac6ab328b24d2f604871bfc8`，aarch64-apple-darwin/debug，source SHA `78b80bbe020225678b7c6c8e4bec81a98741b0a6ddb6974eb147eeeb6b69bd4e` 与当前 source digest 一致。
- 最终 binary 505,311,368 bytes，SHA256 `90b1ed802a3545a3c2a6f5185f29d03a4f708c632cb8bb833927fbdbbf2b0738`。依赖 feature 图仍 unknown，非认证供应链证明。
- `git diff --check` exit 0，未 stage/commit/push。Cargo.lock 已刷新；`just bazel-lock-update` 本轮第三次 `/tmp/codex-oct03-bazel-lock3.log` exit 0，MODULE.bazel.lock 无 drift。同步 lock 不等于 Bazel build 通过。
- 按 AGENTS，final fix/fmt 后没有 test/live。源码编译、之前 mock/unit/live、跨平台/CI/release 分别陈述，不能将 pre-fmt `18afe06a…5ba39` 实测挪到 final formatted SHA。

完整执行汇总临时文件：`/tmp/codex-oct03-finalizer-evidence.json`。长期阅读以本仓库本文和 spec/plan/tasks 为准。
