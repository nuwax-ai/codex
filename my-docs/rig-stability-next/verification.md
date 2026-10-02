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
