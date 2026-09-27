# Claude 增量改动复审（2026-09-27）

## 1. 范围与结论

审查起点：工作树干净，分支 `test`，HEAD `f0444118e5f234b3f3944fe2d50f8de7b7fdd0cb`。

本轮比较 `381fa22f137f6eb86327a823f74a5b02524167c3..f0444118e5f234b3f3944fe2d50f8de7b7fdd0cb`：

- `25f516ce3`：三个 Anthropic live 场景、24 个录制文件、报告补充。
- `f0444118e`：`CODEX_MODEL_REASONING_EFFORT` 环境变量。

两提交没有修改 Rig 生产转换代码。改动共 9,805 行，其中 fixtures 为 9,517 行；实际 Rust 改动为 280 行。复核范围包含这两提交及其配置、daemon、请求续轮调用链，不代表重新验收整个历史 fork 或完成 workspace 测试。

增量审查发现四项需要修改的问题；扩大 TUI 验证时另发现两类既有测试未适配当前 fork。代码与报告修复留在工作树，未提交或推送。测试结果见 §4。

## 2. 发现与修复

### D1 · P2 · 工具失败场景丢失首轮完整历史

原位置：`f0444118e:codex-rs/live-tests/tests/bridge_live.rs:390-405`。

新场景只重建一个 `FunctionCall` 再附加失败结果，遗漏首轮 `Reasoning`、助手文本及工具调用原始 item ID（`FunctionCall.id`，原本保留的 `call_id` 关联不受影响）。三个新 t1 fixture 都有 reasoning，Step 还有助手前言。Anthropic 要求工具续轮完整传回 thinking 块，不能过滤或重建后破坏原始内容。[官方 thinking/tool 工作流](https://platform.claude.com/docs/en/build-with-claude/thinking-tool-workflows)

修复：三个新场景抽到 `live-tests/tests/bridge_live/anthropic_controls.rs`；工具失败路径复用 `append_turn_outputs`，再附加同一 call_id 的 `success: Some(false)` 结果。独立测试通过可注入 runner 调用实际场景，比较整个第二轮 input，覆盖 reasoning、助手文本、工具 ID/参数和内部失败标记。

仅重放 fixture 不能验证此缺陷：`run_turn` 的 replay 分支不比较传入 request。新增回归在修复前失败，明确展示缺失项；历史 fixture 保留为原录制证据，不修改为从未真实执行过的请求。

### D2 · P2 · 环境 effort 阻断共享 daemon，且不能随 daemon 全局继承

原位置：`f0444118e:codex-rs/utils/cli/src/config_override.rs:85-88`；调用方 `tui/src/daemon_startup.rs:59`、`tui/src/session_queue_commands.rs:41`。

环境变量被追加为 CLI override，但 daemon 准入白名单没有 `model_reasoning_effort`。设置该变量后，TUI 不再复用共享服务；已有服务运行时，queue 可能选择 embedded 后报错。使用 `CODEX_MODEL_REASONING_EFFORT=none` 运行原有两项 daemon 策略测试，均可复现失败。

修复包含配对的两处：

1. daemon 允许非空字符串 effort override。effort 已由现有 `config_request_overrides_from_config` 随 thread/start、resume、fork 请求传递；不会作为 daemon feature 启动参数。
2. `app-server-daemon/src/backend/pid_start.rs` 启动共享 app-server 和 updater 时移除该环境变量，防止第一个客户端的选择成为后台服务的长期默认值。直接运行 `codex app-server` 仍可显式使用该变量。

新增隔离子进程测试传入 ambient effort，并通过真正的 PID 启动路径检查 app-server/updater shim 看到的环境；测试进程不调用全局 `set_var`。该启动测试当前为 Unix 覆盖，Windows 实际启动没有在本机执行。

### D3 · P3 · 非 Unicode 环境值被静默当作未设置

原位置：`f0444118e:codex-rs/utils/cli/src/config_override.rs:87`。

`std::env::var(...).ok()` 把 `NotUnicode` 与不存在合并，违背新增配置的 fail-fast 行为。改用 `var_os`，有值时显式校验 Unicode；有效的显式 `-c model_reasoning_effort=...` 仍优先于任何环境值。补充非 Unicode、无效环境值被显式配置覆盖、空白与大小写规范化回归，断言完整结果向量。

### D4 · P2 · 验证结论超出断言和证据

原位置：`f0444118e:my-docs/rig-protocol-audit-2026-09-27.md:282-286`。

- schema 场景只断言非空文本和 Completed/usage，不能证明 schema 被执行或 thinking 不受影响。官方 structured outputs 的语义包括输出遵守 schema；网关接受字段并不足够。[官方 structured outputs](https://platform.claude.com/docs/en/build-with-claude/structured-outputs)
- effort-none 的零可见 reasoning 不能证明没有隐藏推理；有可见 reasoning 则是需要登记的偏离。原固定“77 字符”缺少对应轮次。
- 101/101 是 nextest 登记数，包含运行时跳过；不是 101 个真实厂商场景，也没有证明完整 workspace 通过。

修复报告和场景诊断措辞，保留真实录制与 exec 证据，并明确语义验证和运行参数的边界。

### D5 · P2 · 既有 Responses mock 未明确 native，实际请求走 Chat

首轮完整 TUI 包验证有 22 个测试因此失败。第二轮还从原先超时的生命周期测试中复现了同类问题（`tui/src/app/agents_overview_actions_tests.rs:427`），合计 23 项、11 个测试文件。其他例：`tui/src/app/tests/recap_generation_tests.rs:86`、`tui/src/app_server_session/reasoning_defaults_tests.rs:61`、`tui/tests/suite/focus_palette.rs:117`。

这些自定义 provider 仅设置 `wire_api = "responses"`，未设置 `experimental_bridge`。当前 fork 的 `model-provider-info/src/lib.rs:702` 对自定义 provider 默认启用 Chat bridge；mock 却只接受 `/responses`。preview 日志直接记录了实际 `/v1/chat/completions` POST，mock 匹配次数为零；端到端场景显示 HTTP 404。名称叫 OpenAI 也不等于内建 first-party provider。

修复仅修改这些测试的本地 provider 配置为 `experimental_bridge = "native"`，有内存 `ModelProviderInfo` 的场景同步设 `ChatBridge::Native`。preview 使用已有 `MockResponsesConfig.with_provider_config`，保持压缩和 WebSocket 设置；没有改全局 mock helper 或生产默认路由。这些遗漏早于本轮 base，并非 Claude 最新两个提交新增。

### D6 · P3 · 既有权限菜单测试仍按加入 Ask 前的位置导航

位置：`tui/src/chatwidget/tests/permissions.rs:1018`、`:1062`、`:1383`，及 `approvals_selection_popup.snap`。

Ask preset 早在 `1f1dd2fd6` 加入，最新两个提交未改变菜单。两个 Full Access 测试按一次 Down 实际选中 Ask，因此没有预期的确认事件；返回默认权限的测试少跨过中间选项；旧 popup 快照也没有 Ask。

修复为 End → Enter 选择末尾 Full Access；Home（Windows 再 Down 一次）→ Enter 选择默认权限。只更新已人工核对的现有 Ask 菜单快照，保持其他历史快照。产品菜单和权限行为未改变；Windows 路径未在本机运行。

## 3. 独立核对 Claude 的既有证据

### 3.1 JUnit 与测试注册

本轮测试前保存原 `codex-rs/target/nextest/default/junit.xml` 到 `/tmp/claude-delta-prior-junit.xml`：

- run ID：`8b8326ee-dfb0-4595-a77d-fa9262a12f46`
- 时间：`2026-09-27T13:03:32.952+08:00`
- 101 tests，0 failures/errors/skipped，263.306 秒。
- 分组：`bridge_live` 74、lib 6、`exec_live` 21。

按本轮 Rig-only（`LIVE_INCLUDE_GENAI=0`）口径，从测试注册及运行时 return 分支分解：

| 类别 | 登记数 | 含义 |
|---|---:|---|
| Rig bridge | 36 | 包含三家各一个鉴权负例 |
| exec | 15 | 包含三家 native Responses 场景 |
| 基础设施 | 7 | 6 个 lib 测试和 1 个历史测试 |
| 禁用 GenAI/AB | 43 | 运行时直接返回，JUnit 不显示为 skipped |

JUnit 没有保留成功测试的 stdout，也不包含完整命令、环境和二进制哈希，不能单独证明这些运行条件或零重试。

### 3.2 真实 exec 与录制

`logs/live-{mimo,glm,step}/<marker>/` 中对应时间的 15 份 exec manifest/events 均具有 command_execution 完成事件、exit_code=0、输出中的精确随机 marker，以及 turn.completed。manifest 记录 `git_rev=f0444118e` 和二进制 mtime，但没有二进制哈希/构建来源绑定。因此可以确认这些记录中的工具进程闭环成功，不能用它们证明本轮工作树修复已经跑过真实厂商。

新增 24 个 JSON 文件均可解析、vendor/tag 一致；12 轮事件各有一个 Completed，Added/Done、参数与 reasoning 重组一致。已检查常见凭证格式和含凭证的 URL，未发现。这里不是对任意秘密格式的穷尽性保证。

具体差异：schema 的 MiMo/GLM 输出不是合规 JSON；Step 符合 city1/city2 结构。effort-none 的 Step fixture 有 111 个 reasoning 字符，最后找到的日志有 75 个；MiMo/GLM 未见可见 reasoning。三家工具失败 t2 的原请求元数据都省略了首轮完整输出。

`FunctionCallOutputPayload.success` 是内部字段，普通序列化只写 body；不能从录制请求元数据反推最终 HTTP 的 `is_error`，该字段以 bridge HTTP mock 测试为准。

## 4. 本轮本地验证

所有命令在 `codex-rs` 下通过 `just test` 执行，显式 `--retries 0`。测试日志保存在 `/tmp/claude-delta-*.log`，本节保留命令与结果，临时日志不作为长期仓库产物。

### 4.1 修复前的失败复现

| 缺陷 | 过滤表达式 / 包 | 结果 | run ID | 日志 |
|---|---|---|---|---|
| D1 | `-p codex-live-tests --test bridge_live -E 'test(failed_tool_scenario_preserves_the_complete_first_turn)'` | 1/1 失败，exit 100；完整 input 比较显示遗漏 reasoning、text、item ID | `1e8ba6ba-3c68-4483-b2f9-ee5da4dbd7b8` | `history-before.log` |
| D2 准入 | `-p codex-tui --lib -E 'test(daemon_startup_tests::audited_overrides_allow_daemon_without_allowing_arbitrary_config) \| test(daemon_startup_tests::monorepo_wrapper_overrides_are_eligible_and_select_only_server_features)'` | 2/2 失败，exit 100；额外设置 `CODEX_MODEL_REASONING_EFFORT=none` | `22f23106-f5e1-4c71-a49e-fdcfe5cd2e29` | `daemon-before.log` |
| D2 继承 | `-p codex-app-server-daemon --lib -E 'test(detached_children_do_not_capture_client_effort)'` | 1/1 失败，exit 100；子进程读到 `none` 而不是 unset | `da05b64e-4a27-48f9-a5ec-dadf0f5dbda3` | `child-env-before.log` |

表内命令均附加 `--offline --retries 0`，日志名统一前缀 `/tmp/claude-delta-`。D1 使用 replay 环境，实际缺陷回归由注入 runner 捕获请求，不依赖网络。首次新增 D1 测试编译时发现 live-tests 没有 pretty_assertions 依赖，已改用标准完整对象断言后获得表中有效红测；没有为此修改依赖。

### 4.2 修复后的验证

```sh
LIVE_CASSETTE=replay LIVE_VENDORS=mimo,glm,step LIVE_INCLUDE_GENAI=0 \
  just test -p codex-live-tests --offline --lib --test bridge_live --retries 0
```

结果：81 passed / 2 binaries / 0 runner skipped，exit 0，run ID `1d3ae1a3-b1d6-48f5-af1f-4e3b8eda06cf`，日志 `/tmp/claude-delta-live-replay.log`。实际为 **33 个 Rig 回放 + 8 个基础设施/历史测试 + 40 个运行时 return**（禁用的 GenAI/AB 及 replay 下不执行的鉴权负例），不是 81 个厂商请求。

```sh
just test -p codex-app-server-daemon -p codex-utils-cli --offline --retries 0
```

结果：97 passed / 2 binaries / 2 ignored，exit 0，run ID `59c1db82-e3d9-4343-9d31-0e426d97626a`，日志 `/tmp/claude-delta-daemon-cli-after.log`。分别为 daemon 71 项、CLI 26 项；两个 ignored 项是父测试显式启动的子进程入口，不是缺失覆盖。PID 父测试检查完成 marker，避免 `--exact` 零匹配假绿；非 Unicode 子测试调用公开 `parse_overrides()`，并检查实际执行一项测试。

```sh
just test -p codex-rust-rig-bridge --offline --retries 0
CODEX_MODEL_REASONING_EFFORT=none just test -p codex-tui --lib --offline --retries 0 \
  -E 'test(daemon_startup_tests::) | test(app_server_session::tests::thread_lifecycle_params_forward_config_overrides_and_service_tier)'
```

两命令在下述隔离可执行目录中复验：Rig **81 passed / 2 binaries**，exit 0，run ID `ba0039e9-00a0-46c3-8be1-619ca10567ab`；TUI 定向 **9 passed / 1 binary / 5460 filtered**，exit 0，run ID `f274c5c5-041f-45bd-a062-295c80cb8ced`。日志分别为 `/tmp/claude-delta-rig-isolated.log`、`/tmp/claude-delta-daemon-env-isolated.log`。

### 4.3 本机目录扫描瓶颈与测试位置隔离

未经隔离的 Rig 运行曾出现两个 `stream start deadline` 失败（79 passed / 2 failed，run ID `0e2fc652-2b25-40fe-a09e-b1b5862fc08f`），TUI 定向运行有一个 60 秒超时（8 passed / 1 timed out，run ID `da636478-eba9-4f05-834e-a65bb83ab1af`）。这些初次失败保留在 `/tmp/claude-delta-rig-after.log` 和 `/tmp/claude-delta-daemon-env-after.log`。

进程采样 `/tmp/claude-delta-slow-sample.txt` 显示调用链停在 `reqwest::ClientBuilder::build → hyper_util::proxy::with_system → SCDynamicStore → CFBundle → readdir`。当时 `target/debug/deps` 有 **716,439 项**，单次目录枚举也耗时 15.5 秒。多个 HTTP 测试因此在创建客户端时耗费 30–50 秒，而不是在协议断言处等待。

为保留编译产物，用硬链接把已编译测试程序放入仅有少量文件的临时 `target/debug/deps`，原入口暂时作为转发启动器，仍由 `just test` / nextest 执行。相邻构建资源通过符号链接保持可发现；没有更改测试超时、代理配置、断言或产品源码。隔离后相同 Rig 81 项从 256.2 秒降为 4.3 秒，两项失败转为通过；TUI 定向九项全部通过。

隔离二进制 SHA-256（原文件与硬链接是同一 inode）：

- `codex_tui-3181b88746c09793`：`441a03b5032f319f3134597b93a7fafb125538fa82c2552429cac849619867d3`
- `wire-286276fba7caa139`：`d45344c1b1e55b2b35c8fb55ac26e535466e1fbf492af4d7bfa751eca763b0e2`

最初两个临时转发入口已按记录的 SHA-256 恢复。后续 TUI 使用 nextest 原生的 `--binaries-metadata` 与 `--target-dir-remap` 运行硬链接副本，不再替换原入口。构建及元数据导出命令：

```sh
just test -p codex-tui --offline -E 'none()' --no-tests pass --retries 0
cargo nextest list -p codex-tui --offline --list-type binaries-only --message-format json
```

空过滤仅用于编译，不计入测试通过数。尝试 `just test --no-run` 会与 recipe 固定的 `--no-fail-fast` 冲突，已改用上面的空过滤命令。隔离 target 仅硬链接六个注册测试二进制，其余相邻构建资源指回原路径。两边逐一校验哈希；第二轮元数据及记录在 `/tmp/claude-delta-tui-binaries.json`、`/tmp/claude-delta-tui-final-isolation.json`，其中 TUI lib 哈希为 `014939e1b7879fcd79b2d96fcefa0231e97238fecac22ddbb9d9602da623642a`。

### 4.4 扩展 TUI 全包验证

| 轮次 | 实际结果 | run ID | 日志 |
|---|---|---|---|
| 首轮，默认环境 | 5,487 项 / 3 个实际执行 binary：5,450 passed、30 failed、7 timed out；另 8 skipped；exit 100 | `b28949a8-bc8a-43d1-b175-f02836c94285` | `/tmp/claude-delta-tui-after.log` |
| 第二轮，明确 native、修正菜单测试、隔离测试路径、取消 NO_COLOR | 5,487 项 / 3 个实际执行 binary：5,485 passed、1 failed、1 timed out；另 8 skipped；exit 100 | `0b088778-92e1-47e6-a7fa-51014f6ee154` | `/tmp/claude-delta-tui-final.log` |

nextest 登记六个 binary，其中三个没有测试项；实际执行分布是 lib 5,465 项、`all` 21 项、`manager_dependency_regression` 1 项。这里按实际执行数登记。

第二轮执行仍是 `just test`，附加 `--binaries-metadata /tmp/claude-delta-tui-binaries.json --target-dir-remap <隔离 target> --retries 0`。没有改变断言、超时或重试策略。取消 `NO_COLOR=1` 后，四个 cursor 彩色输出测试全部恢复，未接受其错误颜色快照；旧权限历史快照也随正确导航恢复。

第二轮剩余：生命周期测试原先在慢目录环境下超时，排除该因素后暴露 D5 同类路由遗漏，已补修；统计页测试循环启动 11 个 app-server，在并发运行下达到 60 秒限时。

补修后重新编译 lib，SHA-256 为 `3ccd8603a3c77ddcf8abaf7e87072f95bd92afd8e7fac2d33a60d8dd5bf0e55f`。继续使用相同 nextest 元数据和新隔离 target，附加 `-j 1`：

| 定向范围 / 环境 | 结果 | run ID | 日志 |
|---|---|---|---|
| `test(app::agents_overview::tests::actions::) \| test(analytics::tests::account_layout::)`；取消 NO_COLOR | **19 passed / 1 个实际执行 binary**；5,476 项未选中或 ignored；exit 0，87.259 秒 | `e6e7641c-81ac-4dc7-be23-fdb37e1c3d6d` | `/tmp/claude-delta-tui-followup.log` |
| `test(daemon_startup_tests::) \| test(app_server_session::tests::thread_lifecycle_params_forward_config_overrides_and_service_tier)`；额外 `CODEX_MODEL_REASONING_EFFORT=none` | **9 passed / 1 个实际执行 binary**；5,486 项未选中或 ignored；exit 0，4.641 秒 | `3d767eb9-884e-4077-94b5-ff9b3b1a1e1a` | `/tmp/claude-delta-daemon-env-final.log` |

生命周期测试修复后 11.720 秒通过；统计测试没有改代码或增加超时，低并发下 43.563 秒通过。至此上述全包失败项均有后续通过证据，但**不是最后同一轮全包零失败**：最后一次源码补修后只复验相关模块。

### 4.5 最终源码检查

```sh
just fix -p codex-utils-cli -p codex-app-server-daemon -p codex-live-tests -p codex-tui --offline
just fmt
git diff --check
```

三项均 exit 0。scoped Clippy 用时 8 分 43 秒，日志没有 warning/error，没有自动改动源码；日志在 `/tmp/claude-delta-clippy-fix.log`。格式化只保留本轮相关文件的换行、缩进及尾逗号变化，人工核对没有行为变化；六个无关基线文件（GenAI 五个文件及 `config/src/thread_config.rs`）的自动格式改动已按操作前快照恢复。按照 AGENTS 要求，最终 fix/fmt 后没有重跑测试。

已核对原始 TUI/Rig 二进制入口恢复、隔离副本哈希，以及无遗留 `.snap.new`。没有修改依赖、config schema、生产 Rig 转换代码或权限产品逻辑；新增场景模块与 runner 保持私有范围。最终工作树保留代码、测试和审查报告，未执行 git commit/push。

未启动完整 workspace 测试，也未对修复后的场景发起真实厂商调用。

## 5. 已有语义与后续边界

- effort 环境值走 SessionFlags，沿用显式 `-c` 的 resume 规则：仅覆盖 effort 也会让 model/provider/effort 组使用当前配置。该规则在本轮提交前已存在，并有 TUI 和 app-server 测试；没有在本次局部修复中改成另一套 resume 策略。
- 当前 Rig 生产桥的 Responses/native 边界和缺失能力继续以主审计文档为准。本轮没有新增“任意三协议全字段兼容”的结论。
- 未找到先前提示词要求的 `my-docs/claude-rig-full-validation.md` 或新增完整 workspace 测试证据。
- 本机验证为 macOS；Linux/Windows、真实厂商新录制和完整 workspace 仍应分别登记，不能用本轮局部结果替代。
