# Rig 稳定性第二版复查与后续任务

日期：2026-10-02。分支 `test`；起始 HEAD 为 `f9c560ed66a6ca14280ef6a1c023e3ed11bc14c1`。本轮审查 Claude 新增的 EnvSeed 通道、N2/N3 投影、Core 验证、失败工件与 CI，以及上一轮保留的 F01–F21 修复。

审查完成时实现留在工作树；用户随后授权将已审查的实现、修复和回归固化为本地检查点提交。本检查点保留两条已确认失败的 N2 回归，不代表完整验收或发布。以下坐标是本轮审查起始树的坐标；修复后可能移动。工作树中的 SQLite、journal、tmp 和凭据文件不属于交付代码，不应暂存。

## 结论

独立 EnvSeed 层可以保留配置来源身份，但迁移遗漏了恢复选择、默认回退和配置写入校验。迟到结果的 result-only 投影没有贯通 sanitizer/SDK，纯结果测试通过不能证明带正文、GLM 或跨消息重复的实际路径。引用位置与归属仍缺少 response/block 身份，不能把文本子串匹配当成 N2 完成。

本轮直接修复局部缺陷，并补回完整请求、取消和错误路径的断言。最终测试结果见后文；没有执行 push、远程 workflow 或发布。

## 逐项审查发现

### 配置与 API

1. **R01 / P1：EnvSeed 不被 TUI 恢复逻辑识别为显式选择。** `codex-rs/tui/src/app/config_persistence.rs:35` 与 `codex-rs/tui/src/app_server_session/provider_selection.rs:13` 只识别 SessionFlags。恢复已保存不同模型/provider 的会话时，本次环境选择会被旧会话覆盖；远程参数和历史过滤也缺少显式 provider。已补 EnvSeed 判断和真实 start/fork/resume 参数回归。本地 embedded fork 不具有“恢复旧 provider”的同类路径，不扩大结论。
2. **R02 / P2：默认启动回退丢失 seed。** `codex-rs/app-server/src/config_manager.rs:352` 的 `load_default_config` 未带独立 seed 通道。合法环境组遇到无效用户配置后可能退到 OpenAI，或因保留 id 无表而失败。已补通道和回退回归。
3. **R03 / P2：config 写入成功后产生不能加载的配置。** `codex-rs/app-server/src/config_manager_service.rs:388` 只做 shape validation，会持久化合法四键的用户 nuwax_env 表，之后来源检查拒绝加载。已在落盘前对 updated stack 做最终选择/来源校验；覆盖字节不变、batch 原子拒绝、requirements 优先级和删除修复。缺少用户文件时预先创建空文件是既有行为，本轮没有重写该持久化机制。
4. **R04 / P2：doctor 把 provider 选择来源当作 endpoint 来源。** `codex-rs/cli/src/doctor/model_routing.rs:208` 仅凭 required_model_provider 就标 managed requirements。已检查被选 provider 的实际 base_url 定义，覆盖只有选择、只有定义、定义无 endpoint 等情况。
5. **R05 / P2：新的稳定 wire enum 会破坏旧严格客户端。** `codex-rs/app-server-protocol/src/protocol/v2/config.rs:102`、`codex-rs/app-server/src/config_layer.rs:28` 会输出 envSeed，旧 Rust/Python 闭合 union 无法解析 config/read，includeLayers=false 的 origins 也受影响。内部 EnvSeed 保留，稳定 API 映射为已有 SessionFlags（启动覆盖）；公共新 variant 已移除，配套重新生成 schema，并增加真实 config/read 的严格旧 enum 解析测试。
6. **R06 / 源码兼容边界：公开 Rust 入口新增必填参数/字段。** `codex-rs/config/src/loader/mod.rs:150`、`codex-rs/app-server-client/src/lib.rs:206`、`codex-rs/app-server/src/in_process.rs:147` 使旧 embedding 调用不再编译。仓库内部调用已迁移，但不等于外部源码兼容。若支持外部 embedding，应补旧 loader wrapper/兼容启动入口；需明确 SPI 契约，不把它与已修 R05 的 wire 兼容混为一谈。

### 模型历史与回放

7. **R07 / P1，仍需开发：引用用首个子串命中归属并改变位置。** `codex-rs/codex-rust-rig-bridge/src/transport.rs:344,361` 将匹配 plain block 的 head/cited/tail 搬到所有 pair 之后；`[intro,use,result,cited,tail]` 可成为 `[use,result,intro,cited,tail]`。较早无引用正文含相同短语时也会被错误领取。已移除新生产 expect；没有继续堆叠启发式。新增两个完整 Core HTTP 回归，必须按 response/block 身份与位置设计修复，不能修改预期迎合现状。
8. **R08 / P1：带正文的迟到结果被 sanitizer 丢弃。** `request_messages.rs:195` 去掉已投影 call，`hosted_replay.rs:225` 却只接受同组 call/result；正文已经创建 assistant 时没有 anchor 旁路，密文和引用丢失。已将 sanitizer 改为跨位置配对，统一预算并重建原位置，导入的裸 result-only envelope 仍拒绝。
9. **R09 / P1：result anchor 绕过配对预算并发送孤立结果。** `request_messages.rs:233` 与 `transport.rs:268` 的不对称清理，让 typed result anchor 在原组被裁剪后继续发送。已改用原合法 server_tool_use 作 SDK anchor，统一清理后只注入获准 raw blocks，并验证 65 个 split pair 的两端一致裁剪。审查子代理同时提出 replay=false 旁路；完整生产路径的 `stream.rs:151` 先清空 wire_blocks，该开关泄漏主张不成立。本轮保留额外 split/opt-out 回归，但不把它登记为已证实旧缺陷。
10. **R10 / P1：GLM assistant tool_result 不是 Rig 允许的 raw anchor。** `request_messages.rs:236` 会在 SDK 转换前失败。已统一使用原合法 call anchor；覆盖 GLM 和 canonical result、有/无正文及扩展字段。
11. **R11 / P1：跨消息非 winner completed / 后续 pending 重发 call。** `request_messages.rs:197` 的 Some(_) => true 保留重复 use，result 却被删除。同组测试被 sanitizer 偶然去重掩盖。已按已投影 id 抑制跨消息重复，保留最早位置和首个有效完成项。
12. **R12 / P2：typed result anchor 丢顶层扩展字段。** 结果组被过滤后，Rig Content 往返取代原 Value，vendor_tag 等丢失。修复随 R08–R10 贯通 raw 注入，并以完整对象断言验证。

上述 source gate 仍只针对 hosted envelope。N4 generic provenance 的实际消费、跨 provider/model/授权域矩阵没有实现；N5 精确 token 与整体模型预算没有实现。接近40,960 bytes 的新模型可见单项继续列为 **P0 人工上下文复审**，字节限制不能证明10K-token硬上限。

### 测试与交付证据

13. **R13 / P1 验证缺口：取消后下一轮失败被移出测试。** `codex-rs/core/tests/suite/rig_anthropic_hosted_tools.rs:513` 只等300ms并数一次请求。原 `/tmp/c2-run2.log:31`、`/tmp/c2-dbg3.log:24` 记录后续轮 timeout，但没有证明其请求进入 transport，不能据此确认共享池坏连接。源码明确 drop 初始 HTTP future，wiremock delay 也不跨锁等待。本轮恢复 raw TCP 强回归，检查首 socket 关闭、立即第二请求、最终 error=None 与助手正文，不重建共享 client 或 reset session 掩盖问题。
14. **R14 / P1：CI 的 -D warnings 遇到已知参数数量告警。** `.github/workflows/fork-cargo-pr.yml:61` 与 `live-tests/src/binary_turns.rs:395`。shared runner 接收独立 launch/artifact 参数，已在该单一函数加带原因的允许，移除无效的其它函数 allow，未放松 CI 的 -D warnings。
15. **R15 / P2：工具“恰好一次”计数只观察首事件。** `rig_anthropic_hosted_tools.rs:567,610` 的 counter 未覆盖 abort/follow-up。已持续统计到后续轮终态。
16. **R16 / P2：引用去重只检查选中的 assistant，接受空 citations。** 同文件`:369,377`。已深比较完整 messages 与完整 citation 字段，并检查全局文本出现次数。
17. **R17 / P2，仍需验证：NUWAX 零请求没有合法 auth 正向控制。** `app-server/tests/suite/v2/nuwax_isolation.rs:189` 的 seed turn 故意缺 key；错误断言证明拒绝配置，但0请求不能证明合法组可发真实模型请求。需补有凭据的 child-process 正向控制及拒绝 start/resume/fork；不以全局环境变更污染并行测试。
18. **R18 / P2：workflow 绕过 just 测试环境且不锁依赖。** `fork-cargo-pr.yml:66,90`。已安装 just/nextest，使用 just test 与 --locked，继承 RUST_MIN_STACK/NEXTEST_PROFILE；Linux bwrap 的 pkg-config/libcap 构建依赖也补了安装步骤。actionlint 本地通过不等于远程运行通过。
19. **R19 / P2：313/313 被误写为全部断言与 live 未执行。** `my-docs/rig-stability-next/verification.md:85,113`。详见下方原始日志重分类；Cargo --offline 仅控制依赖解析，不禁测试 HTTP。
20. **R20 / N6 验证缺口：D3 的7项单元测试不覆盖失败复制。** `live-tests/src/binary_turns_tests.rs:16` 起没有调用 best-effort helper。本轮补截断 JSONL 在临时 home 清理后仍保留原字节的回归，并移除 outcome.expect。它只验证复制 helper，仍需用可注入 runner 验证真实失败分支保留工件且保留原错误；marker/compact 尚无失败 rollout 保留。

## Claude 313 项报告的正确范围

依据 `/tmp/final-bridge.log` 与同批工件：

| 类别 | 数量 |
|---|---:|
| bridge unit / wire | 106 / 78 |
| live-tests 本地 lib / bridge_live helper | 14 / 2 |
| 实际厂商 bridge / binary 场景 | 42 / 22 |
| 提前返回但 nextest 标 PASS | 49 |
| nextest 原始 summary | 313 passed，0 skipped |

因此是200个本地测试、64个厂商场景、49个运行时跳过。跳过为36 bridge genai、6 binary genai、停用 A/B 1项、Step Responses 6项。64是场景数，不是 HTTP 请求数。必须与 nextest 原始统计并列报告。

22个 binary manifest 均记录 exec hash `fa9e6f866238af7dfade38ed5813ae3becc618d612994f6f910228eda917c7a5`、`exec_build.source_validated=false`。GLM websearch 确有两轮成功和两个同源 v1 completed envelope；仍不能证明该 executable 包含本轮最终源码。D1收据、D2最终HTTP recorder、D4新鲜产物live仍开放。

## 变更规模与提交依赖

子代理识别4项P2组织问题：

21. `config/src/loader/mod.rs:150`、`config/env_group_isolation.rs:49` 与跨 crate callers/seed producers 相互依赖。原1–7按文件批次不能分别构建；先来源检查、后迁移二进制还会拒绝合法组。需要增量入口/机械传播阶段，再启用隔离；否则登记原子功能的机械规模例外。本轮 R05 已取消公共 EnvSeed，基础阶段无需再暴露新 wire variant。
22. `request_messages.rs:179,233` 的 result-only 生成与 anchor/sanitizer 支撑必须同批，并带真实 mixed Core 回归，不能把必要生产修复留到后面的测试批。
23. 整文件 `request_messages.rs`、`transport.rs`、`hosted_tools_wire_tests.rs` 原683 changed lines，混合旧 gate/pause 与 N2/N3。应按 envelope gate → 原始暂停/取消 → 迟到结果投影 → 引用身份设计划分实际 hunks，不按文件名宣称小批。
24. 原新 Core test文件613行，fixture/投影/恢复/取消需要按行为拆分。首批98行的 CLI reserved guard + exec零请求回归不依赖EnvSeed，是可先落的完整修复。所有拆批建议均需实际构建验证，未重写既有历史。

## 本轮验证

统一 Rust cwd=`/Users/soddy/Documents/git-rust-work/fork-codex/codex-rs`；`CARGO_TARGET_DIR=/tmp/codex-stability-20260930-target`，`CARGO_BUILD_JOBS=4`，just test、--offline、--locked、--retries 0。本轮厂商请求不在这些选择集中。

- 原始树 baseline：`/tmp/codex-review-20261002b-baseline.log`，exit0，603 run/603 pass/123 nextest skipped，201.798s。选择 bridge/config/utils-cli 全部及 binary_turns 单元；旧套件绿灯未覆盖上述边界。
- actionlint：`/tmp/codex-review-tools/actionlint .github/workflows/fork-cargo-pr.yml`，exit0。
- stable / experimental schema：`just write-app-server-schema` 与 `--experimental`，分别记录在 `/tmp/codex-review-20261002b-schema.log`、`...-schema-experimental.log`；均exit0。Python生成器仍有既有uint格式提示，不将其描述为零警告。
- 首次相关构建：`/tmp/codex-review-20261002b-related.log`，exit101，新增TUI测试模块重名和anyhow→color-eyre转换错误；已修正，未产生测试PASS数。
- 修复后相关选择：`/tmp/codex-review-20261002b-related-final.log`，exit100，958 run / 955 pass / 3 fail / 13230 nextest skipped，606.192s。包含完整bridge/config/utils-cli/protocol、binary_turns单元及定向Core/API/TUI/CLI/exec。
- 两项确定仍失败：`anthropic_cited_replay_preserves_intro_search_answer_tail_order`、`anthropic_cited_replay_keeps_repeated_text_citation_owner`，均为完整messages深比较，证明N2尚未实现。不修改预期或忽略失败。
- 第三项工具取消：隔离backtrace定位到开始前的等待；增强诊断后 `/tmp/codex-review-20261002b-tool-interrupt-output.log` 显示实际tool_result为 `sandbox-exec: sandbox_apply: Operation not permitted`，随后模型已正常完成，原等待掩盖了执行失败。未改沙箱检查；经自动审批，在外层沙箱之外单独复验 `/tmp/codex-review-20261002b-tool-interrupt-unsandboxed.log`，exit0，1 run / 1 pass / 14187 skipped，16.085s。全程计数与后续成功断言保留。
- raw TCP Core中断+socket关闭+立即后续轮成功：PASS，27.793s；不能继续声称已证实共享池坏连接。旧wiremock超时的确切机制尚未重现。
- 新API五项、startup回退、doctor endpoint来源、EnvSeed恢复与remote start/fork/resume、late GLM/canonical结果/扩展字段、跨消息去重、split pair数量限制、失败复制helper均PASS。
- scoped lint：`/tmp/codex-review-20261002b-fix.log`，exit0，25m51s；检查上述10个crate的tests与rust-rig路径。仅自动去掉request_messages_tests中3处冗余clone，源码复核没有生产逻辑自动改动。仍有5条独立告警：ConfigManager::new与exec::fork_source各8参数、doctor双Vec返回类型复杂、旧nuwax_isolation测试先default后赋值，以及新Core测试Respond helper的lock.unwrap。这些应在后续配置通道/测试整理批次解决；不称零告警。
- `UV_CACHE_DIR=/tmp/codex-review-20261001-uv-cache just fmt`：exit0；`git diff --check`：exit0。最终fix/fmt后未重跑测试；没有自动commit/push/CI dispatch或发布。

## 下一轮执行任务与提示词

1. **优先 N2 身份与位置根治。** 保留两个新增失败回归，设计 response/block/message 身份及位置载体；不通过 contains/find 或修改预期宣称完成。覆盖多搜索、多引用、重复正文、前置/后置文本、真实工具闭环、恢复/fork与旧格式投影。
2. **N4 与 N5。** provenance 必须进入投影决定；明确模型/provider/endpoint/授权域边界。设计并实现单项/整体预算、超限策略、上下文与累计计量区别；保持签名/密文完整，不能把字节/4视为严格token上限。
3. **N1 正向运行与 SPI。** 合法环境组用有凭据子进程真实请求作控制；补冷恢复/fork/daemon/托管配置矩阵。明确公开 Rust embedding 兼容契约，必要时补旧 wrapper，不能只更新仓库内调用。用共享命名配置通道减少新增8参数/双Vec接口复杂度，并清理已登记测试lint。
4. **N6 工件和源绑定。** exec自身收据与binary hash/源码指纹绑定；最终HTTP脱敏recorder；失败runner注入覆盖全部binary场景，明确复制错误与原错误。重建确定产物后最小MiMo/GLM live。
5. **N7 验证和拆批。** 默认/rig/native所需路径分别验证；CI、Linux/Windows/remote exec/npm记录实际证据。先完成本地工作，push/dispatch/发布另行授权；按真实依赖拆批，不暂存SQLite/tmp/凭据。

```text
请先读 AGENTS.md、my-docs/codex-review-2026-10-02-round2.md 与 rig-stability-next 的规划和验证记录。保留 Codex 当前未提交修复及新增回归，核对最终测试日志。

首先根治 N2 的 response/block 身份和位置：让 anthropic_cited_replay_preserves_intro_search_answer_tail_order 与 anthropic_cited_replay_keeps_repeated_text_citation_owner 通过，不修改预期、不忽略失败、不继续使用子串猜测归属。随后完成 N4/N5、N1 正向 auth/恢复矩阵与配置通道整理、N6 exec收据/最终请求recorder/失败runner工件、N7本地及平台验证。

每批先明确Spec/Plan/Tasks与依赖，完成真实Core/API/HTTP断言、just test，再scoped fix/fmt。报告实际执行、运行时跳过、nextest跳过、失败、重试、工件与exec来源；不把313 PASS解释成313有效离线断言。没有source receipt时，live不能证明最终源码。

完成全部可执行工作再汇总缺口；不自动commit、push或发布，不覆盖别人的改动，不暂存运行时数据库和密钥。
```

CI工具安装参数核对：[install-action 官方说明](https://github.com/taiki-e/install-action#inputs)。
