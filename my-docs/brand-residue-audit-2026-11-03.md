# 品牌残留审计与修复清单(2026-11-03)

> 交接文档:供执行 agent 按 §2 规范修复 §3/§4 清单;§5 不改、§6 需用户决策、§7 验收。
> 上游三波清扫(`ea4e330460`、`25343cac5d`、`3d70e6fc0c`)之后的**全量复扫**结果。

## 0. 审计口径与方法

- **要改的面**只有两类:①用户可见(TUI 渲染文本、CLI 输出/帮助/错误、命令示例);②模型可见(随请求发给 LLM 的文本)。
- 审计工具:[scripts/brand_audit.py](../scripts/brand_audit.py) —— 剥离 `#[cfg(test)]` 模块、`#[test]` 函数与注释后,扫描字符串字面量中的 codex(大小写不敏感)。用法:
  ```bash
  python3 scripts/brand_audit.py codex-rs/tui/src codex-rs/cli/src codex-rs/exec/src codex-rs/core/src ...
  ```
- 全量原始输出(1164 行,含不改项)存档:[brand-residue-audit-2026-11-03.raw.txt](brand-residue-audit-2026-11-03.raw.txt)。修复完成后复扫 diff 应只剩 §5/§6 项。
- **更正一处早前误报**:`"Ask Codex to do anything"` 在生产代码中为 **0 处**——全部位于 `#[cfg(test)]` 夹具内;生产占位符已是 `"Ask nuwax-harness to do anything"`(`tui/src/chatwidget.rs:2101`)。测试夹具按 FORK.md §10 约定**不动**。

## 1. 替换规范

| 场景 | 替换为 | 参照 |
|---|---|---|
| 句中产品自指(小写 codex 作主语) | `nuwax-harness` | `tui/src/chatwidget.rs:2101` |
| 句首/独立产品名 Codex | `Nuwax Harness` | TUI 标题既有清扫风格 |
| `Codex CLI` | `Nuwax Harness CLI` | |
| 命令示例 `codex <sub>` | `nuwax-harness <sub>` | `cli/src/main.rs:540` 已是范例 |
| 导出文件名 `codex-session*.md` | `nuwax-session*.md` | |
| 导出标题 `# Codex conversation` | `# Nuwax Harness conversation` | |
| `the Codex sandbox / Codex host` | `the nuwax-harness sandbox / nuwax-harness host` | |

**替换红线(不许动)**:

- 模型名:`gpt-*-codex`、`codex-mini-latest`、`gpt-live-1-codex`、`codex-auto-*`(auto 档位 id)
- URL(全部):`github.com/openai/codex`、`chatgpt.com/codex`、`developers.openai.com/codex`、`persistent.oaistatic.com`、`go/codex-*`
- 环境变量名:`CODEX_*`(上游契约,fork 兼容接受;报错里引用变量名是事实陈述,保留)
- CLI flag 名:`--codex-home`
- 二进制/包名:`codex.exe`、`codex-linux-sandbox`、`codex-code-mode-host`、`@openai/codex`、`bin/codex`、`codex.sock`
- 内部标识与协议键:`codex_tui`/`codex_app`/`codex_apps` namespace、`codexDelegation`、`<codex_delegation>`、`__CODEX_SNAPSHOT_*`、`__codex_shell_script__`、`codex/encryptedContent`、`codex/imageDetail`、`codex_approval_*` 等 mcp meta 键、`codex-project-roots://`、`codex://` scheme、`x-codex-*` 请求头、`codex-rig-*`、`codex-world-state-fragment-*`、`Product` 枚举值(`protocol/src/protocol.rs:4185+`)
- 遥测事件名:`codex.*` 全族
- 产品专名/生态文案:`Codex Cloud/Desktop/Apps`、`Codex.app`、`com.openai.codex`、`OpenAI.Codex_*` AppID、`"Codex account"`(OpenAI 账户生态)、`ChatGPT backend auth`、`%ProgramData%\OpenAI\Codex`
- 路径保护项:`.codex`(`protocol/src/permissions.rs` PROTECTED_METADATA)、`dotCodexFolder`
- 测试夹具与 insta 快照里的占位串(约 200+ 处;生产串改后需翻新的快照,用窄过滤器逐个接受并目视核对,禁用 `INSTA_FORCE_UPDATE`)

## 2. P0 — 模型可见(随请求发给 LLM),必修

| 位置 | 现文 | 改法 |
|---|---|---|
| `core/src/tools/handlers/dynamic.rs:109` | `Tools provided by the current Codex thread.`(ToolSearch 描述) | `Tools provided by the current nuwax-harness thread.` |
| `tui/src/thread_transcript.rs:293` | `Sent by Codex from task {id}\n{prompt}`(进入 transcript 回放→模型上下文) | `Sent by nuwax-harness from task ...` |
| `tui/src/chatwidget/replay.rs:452` | 同上 | 同上 |
| `tui/src/goal_files.rs:18` | `GOAL_FILE_PREFIX = "Read the Codex goal objective file at "`(目标文件注入对话) | `Read the Nuwax Harness goal objective file at ` |

另:`core/src/unified_exec/process_manager.rs:107`(网络拒绝错误,工具输出回给模型)已列入 P1,注意它同时是模型可见。

## 3. P1 — 用户可见,必修

### 3.1 TUI(`codex-rs/tui/src`)

**app/**:
- `agents_overview.rs:118`(`Open \`codex agents\` ...`)、`agents_overview_new.rs:181,184`
- `background_requests.rs:1038,1043,1048,1049,1071`(plugins 消息)
- `config_panel.rs:144`、`config_persistence.rs:527`
- `daemon_menu.rs:58,111,139,175,176`
- `event_dispatch.rs:2544,3092`(`Run \`codex agents\` ...`)
- `exit_summary.rs:35,52`(**功能性修复**:`vec!["codex".to_string(), ...]` 重连命令,fork 用户没有 `codex` 命令,改 `nuwax-harness`)、`:141,146`(`codex resume {}` 提示)
- `input.rs:285,536,541,546`(ESC 菜单三项)
- `managed_worktree_creation.rs:28,264`、`platform_actions.rs:168,224`、`reconnect.rs:560`
- `server_version_notice.rs:64`、`startup_prompts.rs:138,148`
- `thread_goal_actions.rs:20`(`Run \`codex\` to start ...`)
- `transcript_export.rs:228,290`(`# Codex conversation`)
- `tui_mode_picker.rs:24`、`user_verification_errors.rs:15,28,31,56`

**chatwidget/**:
- `connectors.rs:307`、`notifications.rs:51`(`Codex wants to edit {}` 桌面通知)
- `permission_popups.rs:449,460`、`permissions_menu.rs:15`(full-access 警告)
- `plugin_catalog.rs:391`、`rate_limits.rs:554`(`continue using codex`)
- `transcript_export.rs:50,51`(文件名 `codex-session*.md` → `nuwax-session*.md`)
- `tui_mode_picker.rs:18`、`turn_runtime.rs:385,491,496`(负载/额度消息)

**bottom_pane/**:
- `approval_overlay.rs:932,1048`(`No, and tell Codex what to do differently`)
- `experimental_features_view.rs:107,317`、`hooks_browser_view.rs:705`
- `memories_settings_view.rs:125,140`(:36 的 URL 保留)
- `status_line_setup.rs:195`、`title_setup.rs:102,126`

**其余**:
- `external_agent_config_migration/model.rs:59`、`render.rs:87,92`
- `history_cell/approvals.rs:61,80,95,106,125,144,153,168,174,197,208,225,253,286,318`(UI 片段 `" codex to run "`、`" codex network access to "`、`" before codex could apply "` 等,改 `nuwax-harness`)
- `history_cell/session.rs:226,236`
- `ide_context/ipc.rs:26,30,72,75,92,96,109,112,115`(:23 进程名、:185,195 pipe 名保留)
- `keymap_setup/debug.rs:25,26,67`、`keymap_setup/picker.rs:326`
- `model_migration.rs:92`、`permission_discovery.rs:168`、`daemon_recovery.rs:193`
- `onboarding/auth.rs:441,446,625,646,685`(:626 URL 保留)、`onboarding/auth/headless_chatgpt_login.rs:129`、`onboarding/bedrock.rs:375,377,479`
- `pets/image_protocol.rs:67,70,76`、`pets/catalog.rs:29`(若决策 #4 选择保留 pets 功能则改文案,否则随禁用跳过)
- `session_archive_commands.rs:87`、`session_queue_commands.rs:34,37`、`session_resume.rs:61`
- `slash_command.rs:95,98,110,117,149,159`(斜杠命令描述)
- `startup_orchestration.rs:126`、`status/card.rs:748`(`run codex login ...`)、`status/remote_connection.rs:60`

### 3.2 CLI(`codex-rs/cli/src`)

- `bin/logs_client.rs:18`(`Tail Codex logs ...`;:17 `codex-state-logs` DB 名保留)
- `daemon_install.rs:26`、`exec_server_command.rs:186,420`(命令示例部分;`CODEX_ACCESS_TOKEN` 变量名保留)
- `login.rs:123,284,292`
- `main.rs:814,824,829,871,884,896,904,1097,1128,1146,1151,1160,1832,2013,2280,2285,2315,2320,2328,2423,2515,2538,2561,2630`
- `mcp_cmd.rs:101,495,784,1124`(:563,643 `"Codex account"` 产品专名保留)
- `migrate_nfs_db.rs:63,102,106`、`migrate_rollouts.rs:338`、`queue_cmd.rs:46`
- `plugin_cmd.rs:52,84,85,103,104,122,123`(usage 示例)
- `sandbox_setup.rs:56,102`(`--codex-home` flag 名保留)、`sandbox_uninstall.rs:8,19`
- `state_db_recovery.rs:33,34,48,50,70,71,77,78`
- `doctor.rs:458,555,735,1404,1419,1422,1430,2628,2630,2635`(:1055,1062,1073 `PATH codex entries` 标签归 P2;:1214 二进制探测名保留)
- `doctor/background.rs:100`、`doctor/filesystem_paths.rs:111`、`doctor/git.rs:77`、`doctor/network.rs:111`、`doctor/runtime.rs:102`、`doctor/output.rs:82,467`、`doctor/thread_inventory.rs:195,198,214,432`
- `doctor/desktop.rs:305`、`doctor/desktop/macos_security.rs:147,165,183`、`doctor/desktop/windows_security.rs:202,204,205,210`(AppID/进程名/签名主体保留,只改句子自指)
- `doctor/sandbox.rs:45,283,299,306,311`
- `doctor/security.rs:69-120`:整段面向 IT 管理员的端点排除建议——自指 `Codex`→`Nuwax Harness`,但二进制名(`codex.exe`、`codex-command-runner.exe` 等)与签名主体保留;若嫌语义拧巴可整体降级为决策 #10 处理

### 3.3 exec(`codex-rs/exec/src`)

- `cli.rs:13`(usage 行 `codex exec [OPTIONS] ...` → `nuwax-harness exec ...`)
- `event_processor_with_human_output.rs:221`(`OpenAI Codex v{VERSION}\n--------` 启动横幅 → `Nuwax Harness v{VERSION}`)
- `lib.rs:295,329,332`(错误消息;:272,399,1225 为日志归 P2)

### 3.4 app-server / app-server-daemon(ACP 客户端可见)

- `app-server/src/request_processors/catalog_processor.rs:211`(`Restart Codex to apply them`)
- `app-server/src/request_processors/account_processor/enterprise_login.rs:203,211`
- (account/rate-limit/mcp 的 `"codex account authentication required ..."` 族为 OpenAI 账户生态文案,保留)
- `app-server-daemon/src/lib.rs:333,496,625,676,794,864,1081`、`migration.rs:169,176`、`prepare_install.rs:77,149`、`windows.rs:135`(命令示例 `codex app-server daemon ...` → `nuwax-harness ...`;`CodexCLI.app`/`bin/codex` 二进制名保留)
- `app-server-daemon/src/managed_install.rs:146,163,169,177,254`、`backend/pid.rs:204`、`backend/pid_start.rs:166`、`lib.rs:389,401,1033,1046`、`migration.rs:34`(`has no Codex home` 错误族)

### 3.5 core / protocol / config / login

- `core/src/config/mod.rs:4436`、`core/src/config/permissions.rs:857,872,875`(配置错误/警告)
- `core/src/tasks/mod.rs:409`、`core/src/tasks/user_shell.rs:154`
- `core/src/unified_exec/process_manager.rs:107`(网络代理拒绝错误;**模型+用户双可见**)
- `protocol/src/error.rs:199,818`(升级/额度文案)、`protocol/src/models.rs:1813,1916,1973`(附件读取错误)
- `config/src/strict_config.rs:293`、`config/src/requirements_exec_policy.rs:82`
- `login/src/device_code_auth.rs:86`、`login/src/server.rs:480,951,973,974,976,977,985`(登录失败页文案)、`login/src/success_page.rs:28`(裸 `"codex"`,修前先核对上下文)

## 4. P2 — 半可见(日志、文件名、标签),可选

- 日志消息:`exec/src/lib.rs:272,399,1225`、`core/src/exec.rs:674`、`core/src/session/handlers.rs:355`、`core/src/mcp_tool_call.rs:877`、`core/src/thread_manager.rs:735,775`、`app-server/.../apps_processor.rs:163`、`core/src/tools/handlers/request_plugin_install.rs:150`
- 用户主目录可见的文件名:`tui/src/lib.rs:295`(`codex-tui.log`)、`cli/src/login.rs:86`(`codex-login.log`)、`app-server/.../feedback_processor.rs:205`(`codex-logs-legacy.log`)、`tui/.../feedback_view.rs:255`(`codex-logs.log`)、`cli/doctor/updates.rs:133`(`codex-doctor/`)
- doctor 输出标签:`doctor/output/detail.rs:234,273,275`(`PATH codex entries`)
- `config/src/loader/mod.rs:221`、`config/src/loader/layer_io.rs:86`(构建/托管配置错误,路径事实保留)
- `tui/src/external_editor.rs:185,187`(旧 `.codex` 默认路径探测;§2 已声明不迁移,若确认死代码可另开清理)

## 5. 明确不改(已在 §1 红线列出,此处给理由)

- **内部标识/协议键/遥测**:改了会破坏 TUI↔core↔MCP 协议、历史会话回放与统计连续性,且用户/模型都看不到。
- **环境变量名 `CODEX_*`**:上游契约;fork 文档(NUWAX-HARNESS.md)已并列 `NUWAX_*`。报错中引用变量名是事实。
- **URL/安装源**:事实链接;更新流问题见决策 #6。
- **产品专名**:`Codex account`、`Codex Cloud/Desktop/Apps`、ChatGPT 生态——指 OpenAI 的产品,不是我们自指(FORK.md §10 既有约定)。
- **测试夹具/快照**:不用户可见,改了徒增快照翻新。

## 6. 需决策项(不要在本次批量修,逐项问用户)

1. **`<codex_internal_context>` 标签族**(`core/src/context/internal_model_context.rs:10-11`、`user_goal.rs:87-88`)——随请求发给模型;改名需两侧同步 + 旧 rollout 回放兼容。建议:另立专项。
2. **模型可见的工具名/服务器名**:`mcp__codex_apps__*`(`core/src/tools/user_messaging.rs:95-98`)、`mcp_servers.codex_tui`(`tui/src/dynamic_tools.rs:76` 等)——改名是协议变更。建议:专项评估。
3. **模型输出指令协议**:`:codex-followup`、`codex-file-citation`、`::codex-inline-vis`(`tui/src/markdown_render/inline_directives.rs`、`inline_visualization.rs:29`)——渲染器解析模型输出;改名需同步提示词与历史兼容。
4. **pets 功能**(内置宠物名 "The original Codex companion"、从 oaistatic 拉资源)——建议 fork 直接禁用 `/pets` 或仅改宠物名。
5. **tooltips 上游拉取**(`tui/src/tooltips.rs:17`)——从 openai/codex GitHub 拉 announcement_tip.toml,会给 fork 用户推官方公告;建议禁用拉取(品牌之外的功能问题)。
6. **自更新流**(`tui/src/update_action.rs`、`update_prompt.rs`、`history_cell/notices.rs`)——指向 `@openai/codex`/官方安装脚本;版本 0.0.0 是否触发需验证;若可触发,正解是禁用更新提示而非改文案。
7. **OSC `app=codex`**(`tui/src/terminal_program_status.rs:250`)——终端集成契约,建议保留。
8. **shell 注入变量 `CODEX_SESSION_ID` 等**(`protocol/src/shell_environment.rs`)——`env` 可见但为上游契约(影响 hooks/脚本),建议保留。
9. **originator/User-Agent**(`login/src/auth/default_client.rs:42,145` `codex_cli_rs`、`"Codex "` 前缀)——仅 OpenAI 认证线使用,vendor(rig 桥)不带;建议保留。
10. **doctor 安全建议整段**(`cli/src/doctor/security.rs:69-120`)——本意是官方桌面应用的端点排除指引;fork 场景相关性存疑,可低优先整体重写或保留。

## 7. 验收标准

1. `cargo nextest` 受影响套件全绿(至少:tui、exec、cli doctor、core config、app-server);Mac 上跑 core 集成测试带 `RUST_MIN_STACK=33554432`。
2. insta 快照:仅接受因本清单字符串变化产生的 diff,窄过滤器逐个过,目视核对(AGENTS.md 规矩)。
3. 复扫归零:
   ```bash
   python3 scripts/brand_audit.py codex-rs/{tui,cli,exec,core,protocol,app-server,app-server-daemon,login,config}/src > /tmp/after.txt
   diff <(sort /tmp/after.txt) <(sort my-docs/brand-residue-audit-2026-11-03.raw.txt 中对应 crate 部分)
   ```
   差集应只减不增,且减少项全部对应 §2/§3/§4 的修改。
4. 完成后:FORK.md §10 追加一行本轮清扫记录(含保留清单增补);台账登记(my-docs)。
