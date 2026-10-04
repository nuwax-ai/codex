# 执行任务（未开始）

这些是后续开发任务，不是本轮已验证结果。详细行为见 spec.md，技术路径见 plan.md。

## A：身份加固

- [x] A1 记录实际 HEAD/工作树，确认 R1–R4 和 Codex 补测在场；禁止覆盖未提交改动。
- [x] A2 endpoint-v2 不包含 query 值，共享 scope 规则替换两份 denylist。
- [x] A3 非空 query 的全部实际值由私有 credential-instance 隔离，账号/匿名/动态凭据分支无绕过。
- [x] A4 extra headers 参与实际 scope，稳定 telemetry 不破坏每轮 cache，rotation 降级 opaque。
- [x] A5 legacy wrapper fail-closed + API/核心实际请求回归 + 源码兼容说明。

## B：session 命令

- [x] B1 名字跨 provider 查找及歧义/分页回归。
- [x] B2 queue owner/writer 策略，loaded/unloaded、UUID/名字、daemon/embedded 矩阵。
- [x] B3 行政命令 seeds 和 install-method 一致性，明确远端责任边界。
- [x] B4 同 provider echo resume 与无 override 对照，真实 HTTP model 断言。

## C：容器环境变量

- [x] C1 三个可选 retry/idle 变量的规范、解析、优先级和 daemon 子进程剥离。
- [x] C2 HTTP attempt/capture、Core retry、取消、idle timeout 和并发进程的公共路径测试。
- [x] C3 说明文档列清变量名/单位/0 的含义/覆盖关系，无密钥示例。

## D：证据与门禁

- [x] D1 Core rust-rig 的本地验证示例与实际匹配数断言；核对并保留已开启 feature 的 fork CI，不把 0 匹配算通过。
- [x] D2 live 最终 capture 真正断言 path/model/cap；鉴权保留离线 wire 证据。
- [x] D3 三协议输出触顶 mock，usage/partial output/终止/重采样次数明确。
- [ ] D4 冷启动 timeout 低负载复验；原始失败单独保留，不放宽 deadline。（本轮 load≈28 复验仍失败，真低载窗口未出现，保持未完成）
- [ ] D5 独立登记未执行矩阵：workspace、Bazel build、Linux/Windows、远程 CI、Step、加密引用、跨进程回放、实际 live cap 触顶。
- [ ] D6 pause P0 人工预算复审：提出 token-aware 和兼容方案，未经产品裁决不改既有 cap。


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
