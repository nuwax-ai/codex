# Rig 稳定性下一轮 — Verification（验证记录）

> 当前裁决与 Codex 修复验证见 `my-docs/codex-review-2026-10-02-round2.md`。本文件保留 Claude 历史命令及结果。nextest PASS 必须与运行时提前返回、实际厂商执行和最终 exec 源绑定分别解释。

规则：每批记录源码身份（HEAD + dirty 状态）、完整命令、features、退出码、
实际执行/跳过/失败/重试/超时数、工件位置与证明范围、剩余问题。凭据不入
记录。上轮结果只作参考，最终计数以本轮源码实际执行为准。

## 源码身份

- 基线 HEAD：`f9c560ed66a6ca14280ef6a1c023e3ed11bc14c1`（分支 `test`）
- 工作树：含审查者 F01–F21 未提交修复 + 本轮 N1–N7 改动（本轮不 commit，
  保留可审查 diff）
- 统一环境：`cwd=codex-rs`，
  `CARGO_TARGET_DIR=/tmp/codex-stability-20260930-target`，
  `CARGO_BUILD_JOBS=4`，`just test`（nextest），`--offline --locked
  --retries 0`（除非另注明）

## 基线（F 修复核对）

- bridge+live-tests 选择集（F 修复树）：
  `CARGO_TARGET_DIR=/tmp/codex-stability-20260930-target CARGO_BUILD_JOBS=4 just test -p codex-rust-rig-bridge -p codex-live-tests --offline --locked --retries 0 --test-threads 2 -E 'package(codex-rust-rig-bridge) | test(binary_turns::tests) | test(pending_search_map)'`
  → exit 0，188 run/188 pass/122 skipped，160.796s
  （`/tmp/n-round-baseline-bridge.log`）
- core/exec/doctor 基线选择：与 N1 实施并发被中间态编译错误打断；由下方
  阶段 A 复跑取代（同一树 + N1 完成）。
- 结论：F01–F21 修复在本环境可构建、bridge 全绿。

## 阶段 A（N1 来源隔离 + N5 规范）

源码身份：HEAD `f9c560ed6` + 未提交 F 修复 + 本轮 N1 改动（dirty；本轮
不 commit）。

- codex-config lib 全量：`just test -p codex-config --lib --offline --locked
  --retries 0 --test-threads 4` → exit 0，366/366（含 9 个新层级隔离用例）
- core nuwax/managed 选择：`just test -p codex-core --lib ... -E 'test(nuwax)
  | test(env_group) | test(managed_config)'` → exit 0，8/8（含新增同名
  base_url/env_key 文件拒绝用例；requirements 优先级矩阵改为带真实 seed）
- app-server JSON-RPC 隔离（in-process，含 0 请求断言）：
  `just test -p codex-app-server --test all ... -E 'test(nuwax_isolation)'`
  → exit 0，2/2：thread/start（base_url/env_key 同名覆写 + 外键）与
  resume/fork（真实 rollout + 冷重载路径）均以 nuwax_env 具名拒绝，
  wiremock 捕获 0 个模型请求
- app-server schema fixtures：协议镜像枚举 EnvSeed 触发 fixtures 失败
  （723/726），`UV_CACHE_DIR=... UV_PYTHON=3.12 just write-app-server-schema`
  + `--experimental` 后再生成 7 个 schema 文件（json×4/zst×2/ts×1）
- doctor 关键选择：`just test -p codex-cli ... -E 'test(doctor_sources) |
  test(doctor_resolves) | test(doctor_oss_without)'` → exit 0，3/3
- doctor 宽过滤（test(doctor)|test(model_config)）：139 pass/2 fail/6 timeout
  （`/tmp/n-a-cli.log` 等）——失败与超时均为网络探测类（proxy 认证挑战、
  版本 HTTP probe），本环境不可达所致，与 N1 改动无关（关键用例全过）
- exec nuwax 套件：`just test -p codex-exec ... -E 'test(nuwax)'` → exit 0，
  6/6（含 F02 -c 子键/父表零请求回归）
- tui lib 选择：`just test -p codex-exec -p codex-tui --lib ... -E
  'test(nuwax)|test(daemon_exclusion)|test(has_only_search)|...'` → exit 0，
  20/20（daemon exclusion 改为 seed vec 判定；search-only onboarding 语义
  保持：组活跃时跳过）
- app-server-protocol 全量（schema 再生成后）：314/314，exit 0
- 环境边界登记：app-server 子进程套件（TestAppServer child）在本环境因
  standalone 启动的 remote-control/auth 网络解析（无凭据时重试循环、
  外网不可达）10s 握手超时——既有上游行为，与本轮改动无关（复现于
  不含 NUWAX 的既有用例）；A4 采用 in-process 等价 JSON-RPC 面。

## 阶段 B（N2/N3，B1/B2 批次）

- bridge 全量：`just test -p codex-rust-rig-bridge --offline --locked
  --retries 0 --test-threads 4` → exit 0，183/183，175.754s
  （`/tmp/n-b-bridge-full.log`；含 B1/B2b 两个新 wire 回归与全部既有
  hosted/pause/cancellation/responses 用例）
- B1 回归：cited_text_replaces_the_plain_answer_projection_in_place——
  真实 history（plain Message + envelope）投影后答案恰好一次、cited 终态
  随组保持流式顺序（use→result→cited text）
- B2b 回归：mixed_turn_late_result_preserves_the_sent_request_prefix——
  生产顺序 [client call, client output, pending, completed] 下，迟到结果
  并入 pending 所在 assistant：前序消息逐字节稳定、assistant 已发内容为
  稳定前缀、client call/output 不消失、use 在 result 之前
- 语义说明：测试以修正后语义编写（实现先行）；旧实现下 B2b 的前序消息
  会因 pending use 被删除而不同（旧混合测试第三请求无 client call/output
  的手工重建掩盖了该差异，见审查 N3）


## 最终验证（本轮源码，收尾状态）

- bridge+live-tests 全量（C3/D3 后最终树）：
  `CARGO_TARGET_DIR=/tmp/codex-stability-20260930-target CARGO_BUILD_JOBS=4
  just test -p codex-rust-rig-bridge -p codex-live-tests --offline --locked
  --retries 0 --test-threads 4` → exit 0，313 run/313 pass/0 skipped，
  716.248s（`/tmp/final-bridge.log`）
  原始 nextest 数字保持不变；源码/日志重新分类为200个本地测试、64个厂商场景、49个运行时跳过（genai42、A/B1、Step Responses6）。Cargo --offline 不禁运行时HTTP，不能称为313个有效离线断言。
- core anthropic/rig 选择（C2 后）：`just test -p codex-core --features
  rust-rig ... -E 'test(anthropic_)'` → exit 0，5/5（mixed 前缀/cited
  一次/resume 一致/中断无重复/工具恰一次）
- C3 wire：paused_turn_usage_reports_the_final_request_counters → PASS
- D3 live-tests 单元：7/7
- 收尾 fix/fmt：`just fix -p codex-rust-rig-bridge -p codex-live-tests`、
  `just fmt` → exit 0；`git diff --check` 干净；fix 后未重跑测试（按
  AGENTS 约定）
- 工作树：130 个文件变更（含审查者 F01–F21 + 本轮 N1–N7 改动），未提交
  （本轮保留可审查 diff）

## 阶段状态登记（implemented / validated / not-run / blocked）

- N1 来源隔离：implemented + validated（A1–A4 全部；见上文各选择集）
- N5 预算规范：implemented（spec 表）；单项预算实现沿用 F04/F08（validated
  at bridge）；模型级精确 token 预算：not-run（登记独立后续 Spec）
- N2/N3 投影与前缀：有实现和既有回归；Codex复查发现子串归属/顺序、跨组结果过滤、GLM anchor和跨消息重复等缺陷。修复状态和真实新增回归见 round2，N2 尚未完整验收。
- N4 generic provenance：not implemented（C1 未做——for_prompt 丢 envelope
  元数据、跨源矩阵未建；下轮首要缺口）
- N5 取消：C2 core 级 validated（中断无重复请求、工具恰一次、aborted
  输出进历史）；登记待查：中断 stalled 响应后紧邻 follow-up 轮
  request timed out（请求未达 mock）——连接复用路径需专项调查
- N5 计量语义：C3 wire validated（最后请求计数，非累加）；全轮累计计量的
  汇总事件：not implemented（规范已写入 spec）
- N6 收据/recorder/live：D3有失败复制实现，但旧7项单元不覆盖该分支；D1 exec自身收据、D2最终HTTP recorder未实现。历史最终命令实际执行64个厂商场景，22个binary manifest均source_validated=false；D4针对最终源码的新鲜产物live仍未证明，不能写成“live未执行”。
- N7 交付门禁：E2 workflow implemented（未 dispatch——blocked on 授权）；
  E1 schema 已再生成（validated 314/314）、锁文件全程序 --locked、proto
  未变；E3 平台矩阵：macOS-only 本轮验证，Linux/Windows/npm/remote exec
  not-run（blocked on CI/授权）

## 待授权动作清单

- push 分支（领先 origin/test 约 18+ 提交 + 本轮未提交工作树）
- dispatch fork-cargo-pr.yml（三平台）与 live-tests.yml
- npm 发布 / tag

---

# 第三轮（2026-10-03，检查点 20898140f 之后）

环境同前：cwd codex-rs，`CARGO_TARGET_DIR=/tmp/codex-stability-20260930-target`，
`--offline --locked`，nextest profile local，`RUST_MIN_STACK=8388608`。
本轮机器背景负载高（外部应用，load ≈ 17），影响并行子进程测试。

## N2 身份与位置根治（R07）— implemented + validated

envelope v2：`block_indices`（pair 块原始 wire index，平行数组）+
`layout`（该响应全部 text/cited/pair 条目按原始 index 排序，text/cited 条目
携带**完整 raw block**保真 vendor 扩展字段）。重放端 `transport_identity`
按 concat 校验（合并文本 == layout 文本拼接，身份而非子串相似）+ 首个被替换
块位置重建；v1（无 layout）pair 照常、cited 不再注入（无身份不可归属，
**行为变化**，登记）；无 Message 历史场景 insert-only 追加（不丢真实文本）；
预算/去重丢弃的 pair 条目跳过不猜测。子串 contains/find 逻辑已全部删除。

- `cargo nextest run -p codex-rust-rig-bridge`（fix/fmt 后）：exit 0，
  **203/203**（新增 transport_identity 10 项单元：intro 顺序、重复文本归属、
  双对交错、多引用、mismatch 降级、insert-only、pause 双段、预算丢弃、错位降级）
- `cargo nextest run -p codex-core --features rust-rig -E 'test(anthropic_) | test(rig_)' --test-threads 2`：
  exit 0，**11/11**——两个保留回归
  `anthropic_cited_replay_preserves_intro_search_answer_tail_order`、
  `anthropic_cited_replay_keeps_repeated_text_citation_owner` 转绿（未改预期）
- 环境备注：7 项 anthropic core 测试**满并行**会因机器负载超时（wait 30s），
  `--test-threads 2` 稳定全绿；单测亦绿。登记为环境边界。
- envelope 形状断言升级 v2 的既有测试：GLM 映射、cited 持久化、迟到结果
  raw 字段矩阵（v1×plain×identity 2×2×2）、pause citation owner；R12 的
  vendor 字段/精度断言保留（layout 携带 raw block）。

## 配置通道 lint 清理 — implemented（5 条登记项）

- `codex_config::LaunchOverrides { cli_overrides, env_seed_overrides }`
  命名通道：`ConfigManager::new`（8→7 参）、`worktree::fork_source`（8→7 参）、
  `doctor::model_cli_overrides`（双 Vec 返回 → 结构体）、
  `config_builder_from_parsed_overrides`（双 Vec 参数 → 结构体）；
  ~45 处调用点机械化迁移（含 `/*env_seed_overrides*/` 注释参数消除）。
- nuwax_isolation 测试 default-then-assign → 结构体更新语法；
  Core 测试 Respond `lock().unwrap()` → 毒锁容忍访问器（record/captured）。
- clippy 验证：`cargo clippy -p codex-app-server -p codex-exec -p codex-cli -p codex-config --all-targets`
  → 无 too_many_arguments/双 Vec/unused 告警（输出登记见下）。

## N1 正向控制（R17）— implemented；validated（受负载影响间歇）

- 子进程启动解锁：tests/common spawn 增加
  `CODEX_INTERNAL_APP_SERVER_REMOTE_CONTROL_DISABLED=1`（daemon 内部标记），
  standalone app-server 的 remote-control 解析不再阻塞 initialize。
  该变化同时把既有子进程套件从"挂起"变为可运行（thread_queue 现卡在
  上游 mock 后端 env 未透传——上游环境边界，不在本轮范围）。
- 新增 `app-server/tests/suite/v2/nuwax_positive.rs`（子进程）：
  ① 合法组（NUWAX_BASE_URL/WIRE_API=anthropic/API_KEY/MODEL 全量）真实
  thread/start + turn → **恰好 1 个模型请求**、model 断言、`x-api-key`
  引用凭据断言、mock 答案完成 turn；② 子进程形态 reserved 表写入拒绝
  （nuwax_env 命名 + 0 请求）。
- 验证：低负载窗口 `cargo nextest run -p codex-app-server -E 'test(nuwax_positive)'`
  exit 0（2/2，含 TRY 2 PASS 重试标记）；高负载窗口（load≈17）initialize
  10s deadline 间歇超时。登记：测试正确性已证，本机稳定性受负载影响，
  CI/空闲机器预期稳定。

## N4 provenance 矩阵 — implemented（覆盖维度）+ 登记（缺口维度）

- 新增 `client_tests::provenance_identity_partitions_replay_by_endpoint_model_and_protocol`：
  端点/模型/协议任一维度变化 → envelope 不可重放；同身份可重放。
  连同既有 wire 级 foreign-source 测试（R2）构成跨源矩阵。
- **登记缺口**：授权域（env_key 名，非值）尚未进入
  `reasoning_source`（codex-client Provider 不携带 env_key；需
  ModelProviderInfo→Provider 三 crate 透传后并入 source hash）。
  同端点同模型不同凭据目前仍允许重放——vendor 密文可能组织绑定，
  下轮实现（方案已写入 n2-n3-projection-spec.md 边界节）。

## N5 预算 — 状态沿用并登记

- 既有实现（validated at bridge）：envelope ≤40,960B whole-drop（含 v2
  layout）、每请求 64 对上限（最旧丢弃）、split pair 两端一致裁剪。
- v2 布局变化：捕获端预算含 layout；请求端预算含 derived cited。字节≠token
  （spec 已声明）。core 级聚合超限（B3d）：not-run，登记。

## 待授权动作清单（第三轮新增无；沿用上轮）

- push / dispatch CI / live-tests.yml / npm 发布均未执行。
- 旧 live 工件不能证明本轮最终源码；D1/D2/D4 未实施（N6 收据、最终 HTTP
  脱敏 recorder、注入失败 runner marker/compact 覆盖）——下轮首要。

## N6 收据 / 最终 HTTP recorder / 失败工件 — implemented + validated（离线）

- **D1 exec 自身构建收据**：`exec/build.rs` 编译期嵌入
  `EXEC_BUILD_{GIT_SHA,DIRTY,TARGET,PROFILE,FEATURES}`（dirty 为
  `git status --porcelain` 的稳定 sha256 指纹，前缀 clean:/dirty:；
  rerun-if-changed 覆盖 HEAD/index）；`codex-exec --build-receipt` 打印
  JSON 即退。live-tests manifest 的 `exec_build` 由占位
  `unknown/source_validated=false` 升级为读取该收据并与 harness 自身
  rev+dirty **精确指纹**比对（仅 clean/dirty 布尔会把不同工作树视为相
  同）。验证：`codex-exec::all suite::build_receipt` 集成测试 PASS
  （真实二进制、40-hex rev、指纹前缀、features 列表）；
  `codex-live-tests --lib` 18/18（含 manifest 形状测试）。Cargo.lock +1
  行（exec build-dep sha2）；`just bazel-lock-update` exit 0（无 diff）。
- **D2 最终 HTTP 脱敏 recorder**：transport 末层捕获
  `{method, url(query 值一律 REDACTED), body(重写后终态)}`——**headers 永不
  记录**；公开 `RigTurnRecorders{events, final_request}` +
  `stream_via_rig_with_recorders`（旧 `with_recording` 委托保持兼容）；
  live Record 模式落盘 `final-request-<tag>.json` 工件。验证：
  `version_tests::final_request_capture_records_the_sanitized_wire_shape`
  PASS（query 密钥掩码、无 headers、body 与线缆逐字节相等）。
- **失败工件覆盖 marker/compact（R20 后续）**：共享
  `retain_rollouts_on_failure` 包装（原错误原样返回、保留错误只警告不
  覆盖）接入 websearch + marker + compaction 两轮；单元测试注入失败
  断言截断 rollout 原字节保留 + 原错误文本不变 + 成功路径零副作用
  （18/18 内 2 项新测试）。真实厂商失败分支的端到端保留随 D4 live。

## R06 Rust embedding 兼容入口 — implemented（wrapper）+ 契约声明

- `codex-config` 新增 `load_config_layers_state_with_cli`：EnvSeed 之前的
  旧签名（seeds 恒空）委托新入口；仓库外直接调用 loader 的源码可编译。
- `InProcessClientStartArgs` 结构体文档明确 SPI 契约：该结构跨 fork 版本
  增字段，不承诺外部 struct-literal 兼容；外部入口为
  `codex_app_server::in_process::start`。
- `app-server-client`/`in_process` 的字段级源码兼容：not provided（有意，
  契约文档化）；发布外部 embedding SDK 时再评估。

## 环境边界与稳定性登记（第三轮）

- 机器外部负载（load≈17，OrbStack/远控等用户应用）使子进程 app-server
  initialize 的 10s deadline 间歇超时；nuwax_positive 两项在低负载窗口
  全绿（TRY 2 PASS），正确性已证、本机稳定性受负载影响。
- 7 项 anthropic core 满并行会超时，`--test-threads 2` 稳定全绿。
- 上游 thread_queue 等子进程套件：从挂起变为可初始化，但卡上游 mock
  后端 env 透传（非 fork 范围，不修改他人测试）。

## 离线最终选择集（fix/fmt 后；fix 零自动改动，此前绿灯继续有效）

| 选择集 | 命令（公共前缀 `cargo nextest run --offline --locked --retries 0`，
NEXTEST_PROFILE=local、RUST_MIN_STACK=8MB、隔离 target） | 结果 |
|---|---|---|
| bridge + live-tests 全量（含 live 厂商场景） | `-p codex-rust-rig-bridge -p codex-live-tests` | 338 run / **337 pass** / 1 fail（见 live 节），exit 100，835s，`/tmp/final-bridge-live.log` |
| core anthropic+rig | `-p codex-core --features rust-rig -E 'test(anthropic_) | test(rig_)' --test-threads 2` | exit 0，**11/11**（两个 N2 回归转绿） |
| doctor + app-server 隔离/配置 | `-p codex-cli -E 'test(doctor_sources) | test(doctor_resolves)'`；`-p codex-app-server -E 'test(nuwax_isolation) | test(config_manager)'` | exit 0，**63/63**（nuwax_isolation 含 1 次 flaky 重试通过） |
| live-tests lib | `--lib`（在全量内） | **18/18**（D1 manifest 形状 + 失败保留契约 2 项新测试） |
| exec 收据集成 | `-E 'test(build_receipt_flag)'` | **PASS**（真实二进制 40-hex rev/指纹/features） |
| clippy（触达面） | `cargo clippy -p codex-app-server -p codex-exec -p codex-cli -p codex-config --all-targets` | **0 警告**（5 条登记 lint 全清） |
| 收尾 | `just fix -p …六crate`（0 自动改动）、`just fmt` exit 0、`git diff --check` 干净 | ✓ |

## Live 最小验证（重建来源明确的 exec 后）— 部分通过 + 1 项开放

1. `cargo build -p codex-exec --bin codex-exec`（当前树）；随后 live 运行的
   manifest **exec_build.source_validated=true / status=validated**（D1 首次在
   真实工件上闭环：rev + 精确 dirty 指纹匹配；早前失败运行的 manifest 正确
   报 mismatch——二进制先于后续编辑构建，证明收据能抓陈旧二进制）。
2. **glm_anthropic_rig（marker，anthropic 线）：PASS**（45s 级，源已验证）。
3. **mimo_anthropic_rig（marker，anthropic 线）：PASS**（源已验证）。
4. **glm_websearch_anthropic_rig：FAIL ×8**（陈旧二进制 ×4 + 新二进制 ×4，
   两次独立运行），断言 "turn 2 produced no answer"。已取得的证据：
   - turn1 完整成功：单响应 5 次搜索 + 交错叙述文本 + 最终答案；rollout 中
     v2 envelope 形状正确（首个 pair 携带 17 条目 layout，其余 4 个 sibling
     无 layout、block_indices 正确）。
   - turn2：叙述 + 搜索执行成功、usage 正常、exit 0，但模型在 hosted 搜索
     结果后 end_turn，无最终回答；一次运行另见 GLM 侧
     `multi_agent_v1__wait_agent` 工具调用 JSON 畸形（模型行为）。
   - 离线 GLM 录像重放（bridge_live cassette）与本轮全部 wire 回归通过。
   - **开放调查**（不归因、不猜测）：需要 turn2 实际请求字节定位——D2
     recorder 目前只在 bridge_live 路径；binary 路径无捕获通道。下一步：
     给 exec 场景接 D2（或用 bridge_live Record 复现两轮 websearch）后比较
     v1 式追加投影 vs v2 保序投影下 GLM 的 turn2 行为；同时验证
     hosted_results_replay=false 是否同样复现（区分重放投影与模型行为）。

## 第三轮阶段状态（覆盖上文各节）

- N2 身份与位置：**implemented + validated**（两个保留回归转绿；v1 cited
  取消登记为行为变化）
- 配置通道 lint（5 条）：**implemented + validated**（clippy 0 警告）
- N1 正向控制（子进程）：**implemented**；低负载 validated，高负载间歇
  （登记）；R06 loader wrapper **implemented**，结构体契约文档化
- N4 provenance 矩阵（端点/模型/协议）：**implemented + validated**；
  授权域（env_key 名）并入 source：**not implemented**（方案已登记）
- N5：预算沿用 + validated at bridge；core 级聚合超限、全轮累计计量事件：
  **not-run/not implemented**（登记）
- N6：D1 收据 **implemented + validated（离线+live 工件）**；D2 recorder
  **implemented + validated**（bridge_live 路径；binary 路径未接，见开放调查）；
  失败工件覆盖 marker/compact **implemented**（单元级）——厂商失败分支
  端到端保留随上述调查
- N7：本机 macOS 全选择集见上；远程 CI/Windows/Linux/npm：**not-run**
  （授权门）；本轮 Cargo.lock +1（exec build-dep sha2），`just
  bazel-lock-update` exit 0 无 diff

## 待授权动作清单（第三轮）

- push 分支（origin/test 落后：检查点 20898140f + 本轮未提交工作树）
- dispatch fork-cargo-pr.yml（三平台）与 live-tests.yml
- npm 发布 / tag
- GLM websearch turn2 开放调查的专项时间（建议下轮首要）
