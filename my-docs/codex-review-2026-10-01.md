# 上游合并后审查、修复与 Claude 开发任务

日期：2026-10-01。起始分支 `test`，HEAD `f3fe255ef`，起始工作树干净。

审查结论：A/B 的主要链路已接通，官方合并保留了 Rig Responses 同协议发送、ratio 传递和 CODEX 模型参数隔离。阶段 D 已实现初版，但尚不满足完整保真续接、来源隔离、混合工具轮和恢复侧硬上限的验收要求。本文件登记源码问题、本轮小修复、剩余任务和验证证据。

## 审查范围与依据

- A/B 对照 `b9200b1fe..a1d519778`；上游合并 `1f9841304` 的两父为 `a1d519778` 和 `67727e7cf`，后者包含此次同步的官方 315 个提交。
- C/D 对照 `1f9841304..f3fe255ef`。检查 fork 受影响路径及冲突解决，不宣称逐行审查了所有官方提交。
- 复核既有交接文档、Phase D Spec/Plan、当前源码、测试断言及已有工件口径。以下旧坐标均以审查 HEAD 为准，修复后行号可能移动。
- 官方 [Server tools](https://platform.claude.com/docs/en/agents-and-tools/tool-use/server-tools) 要求暂停时原样回传 assistant 内容，混合轮按 id 跨响应配对，并先返回客户端工具结果。现有重建路径不能据此认定完整兼容。
- [Web search](https://platform.claude.com/docs/en/agents-and-tools/tool-use/web-search-tool) 的结果和引用有独立结构；保存搜索结果块不能替代引用持久化。

## 已直接修复的问题

这些是本次工作树修改，尚未提交。验证结果见后面的证据表。

| 编号 | 级别 | 审查坐标与触发条件 | 本轮修复 |
|---|---|---|---|
| F01 | P1 | `codex-rs/codex-rust-rig-bridge/src/client.rs:121`：缓存键包含每轮 metadata、trace 和 gateway header；缓存永久增长并保留旧凭据，非 ASCII 值还被同一占位字符串折叠 | 共享池仅按三种协议有界保存无请求头客户端；所有 gateway/metadata 头按请求注入，保留 SDK 显式头优先级；自定义 CA 仍走原有专用构建 |
| F02 | P1 | `codex-rs/app-server-daemon/src/backend/pid_start.rs:91`：直接 daemon/updater 启动绕过 TUI exclusion，继承全部 NUWAX 变量 | 两种 detached child 均清除四个 NUWAX 变量和四个 CODEX 模型 seed；直接 standalone app-server 仍读取自身启动环境 |
| F03 | P1 | `codex-rs/cli/src/doctor/model_routing.rs:112`：把 URL authority 当 host，输出 user:password | 使用已有 URL parser，仅输出 scheme、host、port，覆盖 IPv6 和非法 URL |
| F04 | P2 | `codex-rs/cli/src/doctor.rs:639`：doctor 未采用 NUWAX seeds，报告与 exec/TUI 不一致 | doctor 专用加载路径复用同一 seed 解析与优先级；其他 cloud 命令沿用既有入口 |
| F05 | P2 | `codex-rs/utils/cli/src/nuwax_env.rs:92,139,184`：CLI 模型已覆盖环境值，仍校验未采用的空白/非 Unicode MODEL | 只校验实际采用的环境模型 |
| F06 | P2 | `codex-rs/utils/cli/src/nuwax_env.rs:83`：重复 `-c model_provider` 判断首值，配置加载采用末值 | 按最终显式 provider 判断是否采用环境组 |
| F07 | P2 | `codex-rs/cli/src/doctor/model_routing.rs:86`：打印原始 compact 字段，Total scope 未反映运行时 90% 上限 | 按实际 scope 计算：Total 使用模型 accessor；BodyAfterPrefix 优先配置 absolute。另展示完整 usable window，不混淆两种上限 |
| F08 | P1 | `codex-rs/codex-rust-rig-bridge/src/request_messages.rs:147,207,249,298`：Function/Custom/Agent assistant 未计数，搜索遇到新 user 时还会附到旧 assistant；search-only 的空 assistant 又在 SDK 非空校验前失败 | 补齐计数；跨 user/tool-result 的搜索创建新 assistant，通过 SDK 真实 server_tool_use anchor 后在 transport 替换为原始块；关闭回放时清理空消息，不伪造文本 |
| F09 | P1 | `codex-rs/codex-rust-rig-bridge/src/responses.rs:204`、`codex-rs/core/src/client.rs:907`：Anthropic 搜索载荷进入 Rig/native Responses，发送 fork 专属字段和厂商块 | 两层都只清理请求副本的 wire_blocks，保持原始历史；真实 HTTP 回归已在修复前复现 |
| F10 | P1 | `codex-rs/codex-rust-rig-bridge/src/convert_response.rs:214`：pause flush 丢 suffix，HashMap 展平打乱工具顺序 | 暂停与普通 final 共用有序 flush；暂停仍不产生 Completed |
| F11 | P1 | `codex-rs/core/src/context_manager/history.rs:1151`：只计 search action，不计实际回放的原始块 | 将 wire_blocks 的 JSON 字节纳入既有估算，避免几 KB 结果被算作短 query |
| F12 | 测试 | `codex-rs/codex-rust-rig-bridge/tests/wire/responses_regression_tests.rs:252`：请求头超时测试强制读取完整上传，取消上传时自身 panic；外层 2s 还包含客户端初始化 | 服务器仅保持已接受连接、不发响应头；仍断言 100ms 内层 timeout 类型，外层守护允许初始化耗时 |

新增/增强回归包括：三协议逐轮认证与 metadata、公开 Rig 请求的 Responses 投影、Core 请求副本保持、真实 resume 后 wire 清理且 rollout 原前缀保持、20 组 assistant 归属组合、暂停生命周期顺序、搜索载荷估算、NUWAX 模型覆盖/重复 provider、daemon/updater 环境清理及 doctor 脱敏/有效配置。

独立复查还核对 doctor 的普通 `--oss` 意图（未显式 local provider 时也须忽略 NUWAX 组）和 scope 区别：100k window + absolute 120k 时 Total 的压缩阈值为 90k，BodyAfterPrefix 的 scope 阈值为 120k，完整窗口上限独立生效。

本轮格式化后的 Rust 代码/测试合计约 1149 changed lines（含三个新测试文件），需要按五个可独立审查的批次暂存：①启动与诊断；②连接池与请求头/超时测试；③Responses 请求投影；④搜索归属/事件 flush/估算；⑤SDK search-only anchor 与关闭回放。共享文件用分块暂存，按依赖先①②③④再⑤；每批保持可构建并附对应证据。不要将文档、所有修复和后续 D 重构混为单个大型提交；不重写已有提交历史。

## 仍需开发或验收的问题

### R1 临时 provider 的配置隔离与 fail-fast

1. **[P1] 同名配置仍可混入旧认证。** `codex-rs/utils/cli/src/nuwax_env.rs:132` 只检查 exact `-c model_providers.nuwax_env`。普通配置、profile、项目配置经 `codex-rs/config/src/merge.rs:139` 递归合并，旧 http_headers/env_http_headers/auth/aws 可能保留并发送到新 endpoint。必须在有效配置加载阶段检查临时 provider 的来源冲突，明确报错；不能仅追加几个覆盖字段或静默清空其他用户配置。
2. **[P2] URL 检查不是正规解析。** `codex-rs/utils/cli/src/nuwax_env.rs:225` 接受 `http://@` 等无 host authority。使用正规的 URL 类型，启动时拒绝无 host、非法端口/IPv6等；错误不得包含凭据。依赖变化需要同步 Cargo/Bazel 锁。
3. **验收缺口：** doctor 尚缺逐字段来源说明；两个客户端共享 daemon 的隔离仍需实际双客户端/子进程验证，不能仅以白名单和投影代码存在替代。

另有继承的 OSS 诊断限制：`codex-rs/cli/src/doctor.rs:697` 对未指定 local provider 的普通 --oss 没有采用 oss_provider/默认 OSS 模型；TUI/exec 会进一步解析本地 provider。当前修复保证该模式忽略 NUWAX 组，尚不保证它的诊断路由与实际 OSS 启动完全一致。后续复用无副作用的选择逻辑，并明确本地发现/交互的边界。

R1 为下一批优先任务。回归需通过实际 ConfigBuilder 和私有临时 HOME 覆盖配置文件/profile/项目/`-c` 四种来源，证明冲突时零请求、原配置未被改写。

### R2 历史载荷的来源与恢复侧边界

4. **[P1] Anthropic→Anthropic 缺少来源判定。** `codex-rs/codex-rust-rig-bridge/src/request_messages.rs:127` 只检查目标协议。GLM 切到另一 endpoint/provider/model 会发送旧网关 raw block/密文。应使用与现有 reasoning_source 一致的来源身份或明确的能力策略；未知旧来源采取保守降级，不猜测密文可移植性。
5. **[P1] provenance 尚未参与生产投影。** `codex-rs/core/src/context_manager/history.rs:581` 将 envelope 转为裸 item，`codex-rs/core/src/session/model_history.rs:79` 仅负责写 provenance。当前没有生产投影消费者。保存 metadata、在测试中断言它存在，不能视为该需求完成。
6. **[P1，新增 >1K token 路径需 P0 人工复审] 64 对与载荷上限不成立。** `codex-rs/codex-rust-rig-bridge/src/convert_request.rs:169` 限制 assistant 分组数量；`request_messages.rs:134` 将同一 assistant 的所有对扩展成一组。单组 65 对会通过。40,960-byte 检查只在事件发射侧，旧 rollout/导入载荷未校验大小、块数、id/shape。bytes/4 是估算，不能称精确 token 硬上限。

先独立写 Spec/Plan 决定版本化载体、来源、未知来源降级、真正的 pair/byte 上限和错误语义，再实现 Tasks。所有新建、加载、resume、fork、切 provider/model 路径均执行同一验证；密文不能截断、拼造或跨来源重用。硬上限必须在分组前按真实 pair 执行，并验证超限后的 assistant 不为空、不留下悬空 call/result。

### R3 pause_turn、混合轮与引用

7. **[P1] 暂停内容不是原样回传。** `codex-rs/codex-rust-rig-bridge/src/hosted_tools.rs:395` 只重建文本和搜索对，`stream.rs:500` 将其追加后继续。thinking/signature、redacted_thinking、citations、其他 server/client 块及原始交错顺序会丢失。F10 修复了对 Core 的事件 flush，未修复内部 continuation body。
8. **[P1] 未决搜索结果无法跨响应配对。** `codex-rs/codex-rust-rig-bridge/src/hosted_tools.rs:315` 仅遍历当前响应的 uses。混合轮的下一响应只有旧 server id 的结果时，会直接丢结果；第一响应的 call 永远是 in_progress。
9. **[P2] 引用未保存或映射。** `codex-rs/codex-rust-rig-bridge/src/convert_response.rs:115` 只读取 Text.text；SDK Text 附带的 citations 元数据未保留。保存搜索 result 块不能恢复逐文本引用。

R3 必须保留全部原始 assistant content block 的顺序和不透明字段，恢复 input_json/text/thinking/signature 增量的完整终态。混合轮按照 server id 跨响应关联，先返回全部 client tool_result，保持同一 tools 数组。继续使用追加存储；补发完成结果可采用新的追加条目与请求时投影，不能修改旧 rollout。

至少验证：thinking+签名→搜索→结果→带引用文本→pause；仅 thinking 的 pause；重复 pause 与次数上限；server/client 并行→client result→server result；多个乱序 id；缺失/未知/篡改块；取消与错误；保存后 resume/fork。对 continuation content 与 tools 做深度相等，禁止用 contains 或只检查几个 id 代替。

### R4 流取消与远程配置

10. **[P1/P2] drop 后仍可能执行续接或保持请求。** `codex-rs/codex-rust-rig-bridge/src/stream.rs:303` 等待下一 SDK event 时未监听输出 receiver 关闭；`:498` 等 pump 后，即使 Core 已取消也可能启动 continuation。设计清晰的取消所有权，drop/interrupt 时终止 pump、header 等待和递归续接；不以 idle timeout 代替取消。
11. **[P2] remote thread config 丢扩展字段。** `codex-rs/config/src/thread_config/remote.rs:206,241` 对 max_output_tokens、hosted_results_replay 转为 None/不写出。需要远程协议可表达这两个配置，或明确拒绝/报告不支持，尤其不能丢 replay=false 回退策略。预算限制虽已有登记，回退开关也必须登记。
12. **已知功能边界：** provider max_output_tokens 当前只作用于 Chat/Anthropic；Responses passthrough 明确忽略它。输出预算、reasoning、structured output、usage/cache、重试/限流等应整理为三协议字段契约，逐项决定保留/映射/拒绝/省略，避免功能开关的跨协议语义不一致。

同一字段矩阵还需核对 `codex-rs/model-provider/src/provider.rs:485`：cached_web_search 当前只与 native_transport 绑定，Rig Responses 也会禁用 cached 默认模式。应明确按协议和厂商能力选择，不能仅用桥名称代表全部能力；本轮未改变该默认搜索策略。

取消回归应在服务器保持连接且没有下一模型事件时 drop stream，并证明 socket/任务迅速释放；还需证明取消后不出现第二请求、工具不重复执行。HTTP pool 改动的 loopback 只证明本轮头/认证保真，不能替代取消与长会话稳定性验收。

### R5 schema、工件与 CI

13. **[P2] app-server schema 过期。** `codex-rs/protocol/src/models.rs:1208` 增加 wire_blocks，`codex-rs/app-server-protocol/schema/typescript/ResponseItem.ts:25` 及 JSON fixtures 没有该字段。rawResponseItem/completed 也是公共集成面。按最终载体生成 stable/experimental schema，检查 TS wire 名及 JSON，运行协议包测试。
14. **[P2] 失败现场落盘不完整。** `codex-rs/live-tests/src/bridge_turns.rs:22,36` timeout/错误在 persist_lines 前 panic；`binary_turns.rs:422` 子进程 timeout 早于 stdout/stderr 写入。失败也需要目录、manifest、部分事件/字节、退出码和 stdout/stderr；上传目录存在不代表现场完整。
15. **[P2] manifest 无构建 receipt。** `codex-rs/live-tests/src/artifacts.rs:29,43` 使用运行时 HEAD，缺构建时 source SHA/features/platform 的可核验绑定。binary hash 只识别产物；mtime 和运行时 HEAD 不能证明构建来源。B3 的 5913e929f、D 的 201a72d6e+dirty 证据不能直接算当前最终树验收。
16. **[P2] D 的真实验收不足。** `codex-rs/live-tests/src/binary_turns.rs:518,544` 只计搜索 completed 增长；临时 home 返回后删除，没有保留 rollout，也没有捕获 turn2 body 并比对第一轮原始结果。现有 Responses resume/function 测试不能替代 hosted 保存→关闭→resume/fork/切 provider 的测试。
17. **[P2] 断言强度不足。** `codex-rs/codex-rust-rig-bridge/tests/wire/pause_turn_tests.rs:131` 的 contains 不证明 verbatim；`src/hosted_tools_tests.rs:255` 用解析器输出与自身输出比较，需独立确认 query/input。该 fixture 本身转义正确，不应误报为解析错误。
18. **验证缺口：** `.github/workflows/live-tests.yml:76` 已加入所选厂商五项配置预检，但其他厂商/GenAI 场景仍可运行时 return Ok 计 PASS。分别记录注册、选择、实际执行、完成断言、跳过、失败、超时和重试。没有真实 CI run 不能宣称 Linux 已覆盖。
19. **构建与文档缺口：** 两桥/live-tests 没有 BUILD.bazel，Core Bazel 未证明 fork feature 可见；MODULE lock 已变更但尚无本轮生成验证。fork 删除了 upstream Bazel/Cargo CI，`.github/workflows/README.md:10` 仍介绍不存在的工作流。先建立实际可跑的 fork Cargo 离线 PR 门禁，再明确 Bazel 支持范围，更新 README。

MiMo/Step 不支持当前 hosted 声明的证据可以保留；GLM 搜索矩阵单列合理。三家 live 未产生 pause_turn，仍为 not-run。不要通过修改基线或改名，将厂商失败、配置缺失或未执行包装为通过。

## 变更规模审查

以下是历史提交的可审查性问题，不要求重写或回退历史。后续按真实依赖分批，每个复杂逻辑批次尽量 <500 行，非机械批次 <800 行。

| 编号 | 级别 | 提交与范围 | 后续拆分依据 |
|---|---|---|---|
| S01 | P2 | `3ca07d64c` 1414 changed lines；约 1080 为代码/测试，入口 hosted_tools.rs:61、transport.rs:140 | 等价 transport 抽取、搜索约束、tool_choice 分开 |
| S02 | P2 | `cd6a6e74b` 3159 行、28 文件，入口 live-tests/lib.rs:40、artifacts.rs:15、core/config/mod.rs:1679 | ratio 最小独立批约 172 行；随后 daemon、capability gating、live 机械提取、证据增强分开 |
| S03 | P2 | `5913e929f` 1334 行；扣 docs 和 348 行 snapshot 后仍 921 行，入口 nuwax_env.rs:67、doctor/model_routing.rs:12 | atomic group、model-only、exec、TUI/app-server、doctor 分批；snapshot 归对应行为 |
| S04 | P2 | `201a72d6e` 1585 行；Rust 1486 行，stream.rs 单文件 diff 663 行 | pump 等价抽取→载体/采集→投影→暂停续接；最后一批依赖前面的载体与投影 |

`2ae4a90f2` 扣文档后约 390 行代码/测试，`f3fe255ef` 114 行，不按规模报缺陷。两段 fork delta 共 51/56 个文件，包含机械提取；纯上游 315 提交不计入这些功能规模数字。

## 验证记录

所有命令 cwd 为 `codex-rs`（根 justfile 会切到此目录），统一环境：

```sh
export CARGO_TARGET_DIR=/tmp/codex-stability-20260930-target
export CARGO_BUILD_JOBS=4
```

Core/doctor/daemon 首轮定向的精确命令：

```sh
just test -p codex-core -p codex-cli -p codex-app-server-daemon \
  --features codex-core/rust-rig --offline --retries 0 --test-threads 4 \
  -E 'test(client_bridge_tests) | test(web_search_wire_blocks_increase) | test(rig_responses_bridge) | test(doctor) | test(detached_children_do_not_capture_client_model_seeds) | test(nuwax_env)'
```

不修改用户全局配置和凭据，不删除既有 target。使用 just test，测试阶段在 fix/fmt 前完成。首轮误将 -j 和 --test-threads 同时传入 nextest，参数校验退出 2；修正命令后才执行下列基线，参数错误不计测试结果。

| 验证 | 命令/日志 | 结果 |
|---|---|---|
| 修复前基线 | `just test -p codex-rust-rig-bridge -p codex-model-provider-info -p codex-models-manager -p codex-utils-cli --offline --retries 0 --test-threads 4`；`/tmp/codex-review-20261001-baseline.log` | exit 100；282 run，281 pass，1 请求头超时用例失败，1 skipped |
| F09 修复前复现 | 增强真实 HTTP 断言后，`just test -p codex-rust-rig-bridge --offline --retries 0 --test wire -E 'test(responses_wire_projects_chat_history_envelopes)'`；`/tmp/codex-review-20261001-projection-red.log` | exit 100；1 run/1 fail，实际请求包含 wire_blocks |
| 桥/API/配置第一轮修复验证 | `just test -p codex-rust-rig-bridge -p codex-api -p codex-model-provider-info -p codex-models-manager -p codex-utils-cli --offline --retries 0 --test-threads 4`；`/tmp/codex-review-20261001-bridge-config.log` | exit 0；492 run/492 pass，1 skipped |
| 桥/API/配置最终验证（含 SDK search-only） | 同上加 `--locked`；`/tmp/codex-review-20261001-bridge-final.log` | exit 0；496 run/496 pass，1 skipped |
| Core/doctor/daemon 首轮定向 | 上方精确命令；`/tmp/codex-review-20261001-core-startup.log` | exit 100；151 run，149 pass，2 fail；5388 未选/跳过。daemon 被沙箱 ps 权限阻断，resume 的新断言遗漏 Core 动态 turn metadata |
| daemon 隔离复验 | `just test -p codex-app-server-daemon --offline --locked --retries 0 --test-threads 1 -E 'test(detached_children_do_not_capture_client_model_seeds)'`；`/tmp/codex-review-20261001-daemon-isolation.log` | 在允许 ps 的执行环境复验；exit 0，1 run/1 pass，71 未选/跳过 |
| resume 断言修正复验 | `just test -p codex-core --features rust-rig,rust-genai --offline --locked --retries 0 --test all -E 'test(responses_bridge_resumes_history_without_backfilling_provenance)'`；`/tmp/codex-review-20261001-resume.log` | exit 0；1 run/1 pass，2246 未选/跳过。只从比较用 JSON 副本去除动态 metadata，完整 saved/resumed envelope 与原 rollout 前缀比较仍保留 |
| scoped fix | `just fix -p codex-rust-rig-bridge -p codex-utils-cli -p codex-app-server-daemon -p codex-cli -p codex-core --features codex-core/rust-rig --offline --locked`；`/tmp/codex-review-20261001-fix.log` | exit 0；自动 async fn 建议破坏递归 Send 推断，rustfix 自动回滚。保留显式 Send 并补理由注释；手动做等价布尔化简/去冗余 clone、测试 unwrap→expect；删除两个既有未用 import |
| 桥最终 Clippy | `just clippy -p codex-rust-rig-bridge --offline --locked`；`/tmp/codex-review-20261001-clippy-final.log` | exit 0，无 warning/error；不将其称为完整 workspace Clippy |
| fmt/diff-check | 首次 `just fmt` 的 uv 全局缓存写入被沙箱阻断；随后 `UV_CACHE_DIR=/tmp/codex-review-20261001-uv-cache just fmt`；`/tmp/codex-review-20261001-fmt-final.log` | fmt exit 0；git diff --check exit 0；保留首次失败记录 |

未运行：完整 workspace、Linux/Windows、真实厂商请求、CI dispatch、npm 安装/发布。本轮 HTTP 验证使用本地服务器。旧 live 结果保留其原 source/binary 身份，不回填为本轮真实请求证据。

最终测试范围为 496 + 149 + 1 + 1 = **647 个通过项**，不把前面的基线、红灯复现和重复通过批次再累加。过滤未选、框架 skip 与运行时 return 的口径保持区分；不能以此证明完整 workspace 或 D 的未实现矩阵。

验证顺序为 tests → scoped fix → 等价 lint 修整 → 桥 Clippy / fmt → diff-check 和源码复查。遵守 AGENTS.md，fix/fmt 后未重跑测试；以上通过项对应测试时源码，后续只有格式、注释、等价化简与测试错误信息调整。Core 的两处 expect 修改和未用 import 删除经过源码复核，未宣称重新执行完整 Core Clippy。

## 下一轮 Tasks 完成标准

- [x] T01 R1：真实配置来源冲突（用户文件/profile/`-c` 子键；项目来源经核实被上游结构性忽略并断言该边界）与 URL 正规校验；零请求、不改原配置；doctor 逐字段来源与 plain --oss 边界行；双客户端隔离子进程测试。证据与偏差见 `r1-temp-provider-isolation.md`。
- [x] T02 R2：版本化 envelope + 来源门禁 + 请求侧真实 pair/byte/形状上限（`eff9c9d6c`）；旧格式与跨来源保守降级；实现与证据见 `r2-r3-hosted-fidelity-{spec,plan,tasks}.md`。
- [x] T03 R3：全块原样暂停续接（含 thinking/signature/citations/交错，`e9e31e2f7`）；跨响应配对与追加完成条目 + 投影去重（`18e97a41a`）；引用持久化进 envelope（`407d5b2de`）；深比较 wire 回归与 core rollout 持久化（B5）。核心级 resume 当前覆盖 Responses 场景的 envelope 持久化；Anthropic 核心级全链路由桥 wire 测试覆盖。
- [ ] T04 R4：取消关闭 socket/pump/递归续接；远程配置保留预算与 replay=false；三协议字段契约和厂商能力矩阵。
- [ ] T05 R5：schema/Bazel 与 CI 范围准确；失败现场和 rollout 保留；构建 receipt 绑定 source/features/platform/binary；实际执行和跳过分开统计。
- [ ] T06 相关离线回归完成后，最小 GLM hosted live 与最终工件验收；其余厂商/平台无配置或未运行时登记 not-run；更新本文和 Phase D 状态。

## Claude 下一轮执行提示词

```text
在 /Users/soddy/Documents/git-rust-work/fork-codex 工作。先读 AGENTS.md、my-docs/codex-review-2026-10-01.md、原稳定性交接和 phase-d-spec/plan，核对 HEAD 与本轮未提交修复，保留并适配这些改动。

先执行 R1 的临时 provider 来源冲突隔离与正规 URL 校验，完成真实配置加载、零请求和双客户端隔离回归；然后按 R2→R3 推进 hosted 历史来源/硬上限、全部原始 assistant 块暂停续接、跨响应配对和引用。大改动先形成独立 Spec、Plan、Tasks，再逐批实现；本轮 Codex 修复不代表 D 完整验收。R4/R5 中依赖这些载体的取消、远程配置、schema 和测试证据配套完成。

每批补可复现行为回归，检查真实 HTTP body/headers、完整事件、保存恢复和 rollout 原前缀。使用 just test、隔离 target、构建 receipt，完成相关离线回归后做最小 GLM hosted live；MiMo/Step/官方厂商按实际能力和已有凭据登记，缺配置或未触发 pause 均为未验证。完整 workspace 按 AGENTS.md 单独确认。

保持 Rig Responses 同协议发送、显式 wire_api、类型层投影、追加历史；不伪造密文，不静默放宽工具约束，不用 PASS 总数或 contains 代替验收。同步 schema/Bazel 数据和必要锁文件，遵守 fix/fmt 的仓库顺序。

持续更新每批 Tasks：文件、行为、精确命令与退出码、实际断言/跳过/失败数、source/features/binary hash 与工件路径。每批复杂逻辑 <500 行，独立可构建；完成后说明剩余边界。提交仅按明确用户授权，禁止自动 push、发 PR 或发布。
```
