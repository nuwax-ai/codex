# Codex 复查交接：Claude 检查点复查修复（2026-10-04）

给 Codex 的独立复查输入。基线：`ead6f2bcae3e0cac4a8374708e9dc1d1d31296a6`（= 检查点提交，工作树干净）。以下 8 文件 +156/−11 是 Claude 复查轮的全部改动，**未 stage/commit**。完整审查报告（发现、证据、未验清单）见 `claude-checkpoint-review-results-2026-10-03.md`；本文只覆盖 diff 本身。

复查命令建议（隔离 target、仓库 just 入口）：

```bash
cd codex-rs
CARGO_TARGET_DIR=<isolated> just test -p codex-rust-rig-bridge --offline --locked --retries 0 --test-threads 2
RUSTY_V8_ARCHIVE=… RUSTY_V8_SRC_BINDING_PATH=… just test -p codex-tui -p codex-app-server-daemon \
  -E 'test(app_server_target) | test(daemon) | test(nuwax)' --offline --locked --retries 0 --test-threads 2
```

我方实测：rig-bridge 241/241（含 2 个新回归）；tui/daemon 53/53；`just fix -p codex-tui -p codex-rust-rig-bridge` 无自动改动；`just fmt` 后 `git diff --check` 干净。

## R3 — P1：pause 续接预算被连带缩小（rig-bridge）

- `src/stream.rs`：新增 `const MAX_PAUSE_CONTENT_BYTES: usize = 40_960;`，`pause_replay` 消息预算检查从 `crate::hosted_replay::MAX_PAIR_BYTES`（本检查点已 40,960→9,800）改为该常量。
- 问题：`MAX_PAIR_BYTES` 收紧是给**持久化 envelope/layout** 的；stream.rs 复用它做**暂停助手消息整条硬失败**预算，导致 >9.8KB 的暂停消息（thinking+签名+文本很常见）整个 turn 报错——检查点前 40,960 内可正常续接。规格未要求缩小 pause 预算。
- 回归：`tests/wire/pause_turn_tests.rs::paused_content_between_envelope_and_pause_budgets_still_continues`（20,000 字节内容 → 续接成功、逐字回放、恰好 2 个 HTTP attempt）；既有 41,000 fail-closed 测试继续钉上界。
- 审查点：40,960 这个数值是恢复检查点前行为（不是新设计）；若你认为 pause 预算也应收紧到 9,800，那是产品决策，需要同步改规格与 fail-closed 测试，不是本修复的隐藏语义。

## R4 — P2：无 wire index 的外来 call 克隆在成功 rebuild 时被吞（rig-bridge）

- `src/transport_identity.rs::rebuild_response`：在遍历 layout 之前，把本响应 ordinal 下 `wire == UNKNOWN_WIRE_INDEX` 的位点块按块序补进 `rebuild` 头部。
- 问题：迟到搜索结果的外来 call 克隆（`call_index=UNKNOWN`，仅当原 call envelope 被预算整体丢弃/缺失时被 request_messages 保留）被 `splice_identity` 标记 handled，但 pair 双射只覆盖有 wire index 的位点——rebuild 成功时该 call 块既不在重建里也不进 fallback/legacy，线上出现孤立 `web_search_tool_result`（tool_use_id 指向不存在的 server_tool_use），违反「每个接受的调用块恰好一次」。
- 回归：`src/transport_identity_tests.rs::successful_rebuild_keeps_unindexed_foreign_call_clones_with_their_result`（断言 [call, text, result]）。
- 审查点：(1) 我选择把克隆放**响应块前部**（call 先于其 result，同消息配对）；fallback 路径（rebuild 失败）仍是响应尾部 [call, result]——两条路径对克隆的相对位置不同但都保配对，你是否接受这个不对称。(2) 克隆块不参与 `pair_coverage` 双射（envelope 校验已保证 UNKNOWN 位点必为 `server_tool_use`，见 `hosted_replay.rs` validated_envelope）。

## R1 — P2：session-command 远程启动绕过 NUWAX fail-fast（tui）

- `src/session_archive_commands.rs::start_app_server_for_session_command` 的 `--remote` early-return 分支（该分支在 `app_server_target_for_launch` 的 guard 之前返回）加入与交互启动**完全相同**的判定：`codex_utils_cli::nuwax_env_overrides(nuwax_env_from_process(), cli.shared.model/-m/--oss, &cli_kv_overrides)` 含保留 provider 表种子即报同文案错误；解析失败也 fail-fast。
- 覆盖 `codex queue/archive/delete/unarchive --remote`（`session_queue_commands.rs` 复用同一入口）。
- 审查点：我只加了 guard，**没有**把 seeds 注入该路径的 ConfigBuilder（不改变本地行为面）；seed-unaware 的 daemon 复用（`config_exclusion`）仍维持原样，登记为 N4 后续项。若你认为 session-command 应完整接入 seeds/exclusion，那超出本修复范围。

## R2 — P2：保留 id 硬编码与协议常量脱钩（tui）

- `src/lib.rs` 新增 `pub(crate) fn nuwax_env_provider_seed_active(&[(String, toml::Value)]) -> bool`：由 `codex_protocol::config_types::NUWAX_ENV_PROVIDER_ID` 构造 `"model_providers.<id>"` 比较。
- 三处共用：`lib.rs::app_server_target_for_launch` 远程 guard、`daemon_startup.rs::exclusion`、R1 的新 guard。替换原来的两处字符串字面量。
- 回归：`src/app_server_target_tests.rs::nuwax_env_provider_seed_active_detects_the_seed_group_regardless_of_site`（正/负/空三态）。
- 审查点：一处 `*key == reserved`（&String 解引用比较）是 fmt 后形态；helper 用 `format!` 构造保留键每次调用一次，调用点频率低（每进程启动一次/每 daemon 判定一次），无性能顾虑。

## 明确未改的相邻项（已在报告 §3 登记，勿误认为遗漏）

- `model_auto_review::thread_resume_and_fork_upgrade_legacy_protected_model_settings` 失败：根因已实测（`sample` 显示子进程卡 `_dyld_start`，521MB debug 二进制 adhoc 签名 126,368 页哈希逐页校验 + 冷换页；mac `DEFAULT_REQUEST_TIMEOUT=10s` vs Windows 25s）。该文件零改动，按「不放宽 deadline 制造绿灯」不修。
- genai 桥不消费 cap、query 凭据 denylist 双份维护、credentialInstance 不含 extra_headers、legacy-unscoped 入口等 N 系列登记项。
- reasoning.rs 投影明文 >9,800 优雅丢弃（有告警、不失败）维持原样。

## 验证映射

| 修复 | 覆盖批次（实测通过） |
|---|---|
| R3 | rig-bridge 241/241（新回归 + 既有 41,000 fail-closed + 全部 pause wire） |
| R4 | 同上（新回归 + 全部 identity/replay wire + core rig_anthropic 套件在批次 2 通过） |
| R1/R2 | tui/daemon 53/53（含快照不变：错误文案与交互路径同串） |
| 修复后 live | MiMo/GLM 三协议 marker + GLM websearch 双轮 7/7（最终树二进制，manifest `source_validated=true`，source digest 与树复算一致 `831b07e4…`） |
