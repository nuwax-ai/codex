# Claude 稳定收口执行记录（2026-10-16 轮）

执行者 Claude Code；范围 = [十项收口计划](claude-stability-closure-plan-2026-10-16.md)。
起点：Codex `test` `2f2ddc157`（CI run `38824922110`），ACP dev `c9e0ab2`。
本轮不 push、不手动 dispatch、不发布、不调用付费 API；冻结 main/Rig/V8。

## 0. 逐项判决（executed / fixed / blocked）

| # | 任务 | 判决 | 摘要 |
|---|---|---|---|
| 1 | 当前 SHA CI + Linux 主路径 | executed | CI headSha 核对=2f2ddc157；ubuntu/windows 失败 lane 首败已收（见 §1）；Linux 原生定向复验 6 轨（bridge 303/303、core-family 52/52、utils-truncation 61/61、app-server 修复族、env、replay，见 §2） |
| 2 | ACP 六场景 × 三 wire = 18 | executed | **Linux 18/18**（codex CLI `4c241421…`，mock 17/17 合同）；**macOS 交叉 18/18**（CLI `3fe7388…`）；期间修正 mock 三处 wire 形状认知（namespace 广告、tool_result 嵌套图、output 数组图） |
| 3 | Code Mode 三输出合同 | executed | 3 个新测试过真 host：plain return 无内容、base64 text 不猜图、超限图片合并省略标记（`284709eab`） |
| 4 | 图片生命周期缺口 | executed | 见 §4（子代理补 kill 冷恢复、fork 双 wire、TurnComplete error、JPEG/WebP、remote compact append-only） |
| 5 | cap/usage/取消公共链路 | executed | 跨 wire 取消 4 测试（`486d9e2a2`）+ cap 温继续/半截参数/bridge cut（`c09328aae`）+ WS cap/usage/cancel 5 测试（`1cc718dff`）；无产品缺陷 |
| 6 | thread/read 重连分页 | executed | 见 §6（冷重启身份一致、call/result 分页边界 + ACP 消费侧） |
| 7 | 50 轮压测 | executed | 51 轮（3×17 固定表）三 wire；收据 §7 |
| 8 | 记忆巩固正常退出 | fixed | 见 §8（Shutdown 与真实错误分离 + ownership 校验） |
| 9 | Windows 原生 | executed | 当前 SHA 原生 MSVC 复验 + 19:58:39Z ABORT 先行分诊；收据 §9 |
| 10 | 产物与最小使用清单 | executed | [nuwax-minimal-usage-2026-10-16.md](nuwax-minimal-usage-2026-10-16.md) + 本文件 + `artifacts-2026-10-16/`（18 行场景 ×2 平台、压测 3 wire 工件） |

## 最终判决

**Linux/容器主门（app-server + ACP dev + 第三方 provider，mock 合同）全绿**：
18/18 场景 ×2 平台、51/51 压测轮、定向复验仅剩 3 类已探针定案的环境性失败
（bwrap×3、模型目录外呼×1、（Windows 侧同 cloud-bundle））。真实产品缺陷
本轮修复 3 个（记忆退出 `b45cfeab6`、ACP 图片失败详情与恢复用量 `1669028`），
新增强回归 40+ 条。**无阻断项：可进入真实服务小批验收**（真实厂商/视觉
按约定另取授权；Step 端点只有 Chat/Anthropic 形状，Responses 真实服务
验证待端点提供）。

交 Codex 的非阻断清单：
1. cloud-bundle 挂起加固（模型目录外呼有界超时/不阻塞 initialize）；
2. CI ubuntu 新增 3 失败分诊（hooks×2、guardian_subagent cancel 竞态）+ 11 慢性；
3. CI windows 20 个新形态（unified_exec/uds/pty/cur 检测/TUI ABORT）；
4. macos lane 连续被取消的基建问题；
5. AppArmor/userns 受限主机上的 bwrap 依赖测试策略（探针收据在 §2）。

## 1. CI run 38824922110（headSha 已核 = 2f2ddc157）

- bazel bridge gate（ubuntu）：✅ success。
- nextest ubuntu：❌ 22,841 run / 22,827 pass / 13 FAIL + 1 TIMEOUT（48 skip）。
- nextest windows：❌ 完整跑完（无磁盘中断）：27 个唯一非过（对比上轮中断前 94）。
- macos：**cancelled**（连续第二 run 被取消，无结论不引用；22K 全仓在 mac
  runner 上疑似超时/取消——lane 基建问题交 Codex 决策，非测试结论）。

**上轮修复全部在 CI 复验转绿（ubuntu resolved 15）**：截断簇
（code_mode nested×5、truncation×3、step_settings、large_explicit_budget）、
turn_cap_usage、fs_watch×4、retained_tool_output SIGABRT。Windows resolved 87
（含上一轮 19:58:39Z 的 `retained_tool_output_survives_compaction_and_resume`
ABORT 与全部 sqlite/code14 簇——磁盘隔离 `8d5650e3d` 生效）。

**ubuntu 当前 14 个非过分类**：
- 慢性（上轮同败，11）：account external_auth discovery TIMEOUT、
  analytics guardian_review_turns_and_tools、guardian_context_budget 的
  uploaded_original_image_history_compacts、astra_plugin_policy、
  shell_snapshot blocked_descriptor_path、guardian-v2 pool_pressure +
  unusable_parent_checkpoint、linux-sandbox root_metadata_symlinks、
  network-proxy unapproved_hosts_dns、skills host_roots repo_ancestry、
  tui config_overrides_forward。
- 新增（3，待 Codex 分诊，均非主门路径）：
  1. `hooks::permission_request_hook_allows_exec_command_without_user_approval`
     — "approved command should remove marker file"（0.9s，CI runner 环境敏感？）；
  2. `hooks_executor::executor_browser_and_computer_use_cleanup_hooks_use_separate_mcp_routes::subagent_enabled`；
  3. `guardian_subagent_authorization::…code_mode_user_message_flat_cancelled_before_confirmation`
     — 取消未确认回合时 tokio "context being shutdown" panic + deadline（10.5s，
     疑异步 teardown 竞态）。

**windows 当前 27 个非过（7 慢性 + 20 新形态）**：windows-sandbox unified_exec×4、
uds ACL×2、pty conpty、external-agent-migration cur 检测×4、TUI ABORT×3、
TUI fail×3、plugin_list、external_agent_config、config_overrides 等——
全部为 Windows 扩展支持门项（独立缺陷/环境待复验），不阻断 Linux 主门；
原生机定向复验见 §9。

## 2. Linux 原生定向复验（192.168.32.163，源 2f2ddc157 tar + git init，
   RUSTUP_TOOLCHAIN=1.95.0、RUST_MIN_STACK=8MiB、NEXTEST_PROFILE=local、
   test_stdio_server/codex-code-mode-host/codex-exec/codex CLI 预建）

| 轨道 | 结果 |
|---|---|
| bridge 全 crate | 303/303 |
| core rig 家族（含 anthropic_interrupt 夹具、cap、图片、20 截图） | 52/52 |
| utils output-truncation + image | 61/61（1 slow） |
| app-server 修复族（turn_cap_usage/fs_watch/thread_read/thread_resume） | 111 run：108 过；3 失败 = 2×bwrap（同下环境性）+ 1×模型目录外呼挂起（见下） |
| env 三件套 | 61/62（1 = environment_permissions_follow_configuration_ownership，已用独立探针校准为环境性，见下） |
| live replay（mimo,glm,step） | 196/196 |

**cloud-bundle 失败定性（第 3 个 app-server 失败）**：
`thread_{resume,fork}_surfaces_cloud_config_bundle_load_errors` 在本机确定性
挂 10s deadline，但 CI ubuntu 1.4s 通过、mac 通过。探针：initialize 期间
app-server 对 `199.59.148.6:443` 停在 **SYN-SENT**——本机 DNS 被拦截
（api.openai.com 等解析到 Facebook 段假地址），默认模型目录端点外呼连到
黑洞 IP。定性：**环境触发（拦截式 DNS）+ 产品加固项**——MessageProcessor
构建即启动 models refresh worker，外呼无短超时，离线或出口过滤部署下
initialize 会长时间挂起（NUWAX 目标环境相关）。交 Codex：给目录外呼加
有界超时或使 initialize 不等待它；不为绕过而改测试。

**AppArmor/userns 校准（环境性定论，带探针收据）**：该测试在 2f2ddc157
仍以 `bwrap: loopback: Failed RTM_NEWADDR: Operation not permitted` 失败。
独立探针：裸 `bwrap --unshare-net` 在该机同样 RTM_NEWADDR EPERM（与 codex
版本无关）；`unshare -Ur` uid_map EPERM；机器 **2026-10-15 10:47 重启**，
`kernel.apparmor_restrict_unprivileged_userns = 1`（Ubuntu 默认限制）。
上轮"4ca0e4fa0 绿 → 8e7c5904b 红"的分界与重启时间吻合：绿跑在重启前、
红跑在重启后，非产品回归。该测试需要 bwrap/userns 可用主机（或容器内
root/userns 路径）才能执行；不作为当前 SHA 的产品失败记账。

## 3. （并入 §0/§7）

## 4. 图片生命周期缺口（commit `0b5c0cdb7`，62/62 全绿，无产品缺陷）

1. **kill 模式冷恢复**：`{chat,anthropic,responses}_image_history_survives_kill_resume`
   ——不经 Op::Shutdown 直接 drop 全部句柄（rollout 逐行 flush 已证），断言
   thread-writer 锁释放后 resume 精确回放 typed image 载荷 + call/result 对。
   首跑 responses 红（夹具算术：第 3 个请求才是 resumed），修正后三 wire 绿。
2. **Chat/Responses fork**：`chat/responses_fork_replays_image_history`
   （Legacy fork 模式，精确载荷 + 工具对）。首跑即绿。
3. **热继续载荷**：`image_resume_cell` 各 wire 断言 `bodies[1]` typed image
   与产出载荷逐字节相等。绿。
4. **TurnComplete error 检查**：compact_remote/view_image/image_rollout/compact
   四套件的等待处全部补 `error.is_none()`（common/lib.rs 未动，爆炸半径已核）。
5. **remote compact append-only**：4 个 test_case 均断言逐阶段 rollout 快照
   字节前缀 + 真实增长。绿。
6. **JPEG/WebP E2E**：`image_formats_pass_through_typed_parts`
   {wire}×{jpeg,webp} 6 格：typed media type + 源字节精确 passthrough。绿。

## 5. cap/usage/取消公共链路

- **跨 wire 取消合同（`486d9e2a2`，4 测试）**：chat/responses 的
  before-headers（socket 释放 + 同 client 立即 follow-up）与 mid-delta
  （消费过增量后 TurnAborted、绝无假 TurnComplete、恰 2 连接——重试预算
  非零使该断言有效）。
- **cap 后温继续（`c09328aae`）**：turn_cap_usage 两参数化用例扩展——cap 后
  同线程成功 follow-up 安装自身 usage 为 last、total 严格增长越过
  previous+14、冷恢复回放温继续态；core 侧 resume follow-up 补第二
  TokenUsageRecord 断言（total=70+21）。绿 2/2，无产品缺陷。
- **Responses 半截工具参数 E2E（同提交）**：
  `responses_cap_with_half_streamed_tool_arguments_never_executes`——
  function_call arguments 半途截断 + incomplete 终态走 assert_cap_terminal
  全合同（恰一次失败、不执行、无假 Completed、重试预算 2 下恰 1 POST、
  截断调用不入历史）。绿。
- **bridge PendingTools::cut 单测（新 response_tools_tests.rs，5 测试）**：
  cut 完成无错但 finish 带 cut 标记、final 不能复活、与正常工具共存、
  有原生 item 的非法参数是 shape error 而非 cut、无参数片段也带标记。绿 5/5。
- **Responses WS cap/usage/cancel 家族（`1cc718dff`，5 测试）**：真 WS 客户端
  E2E——cap 终态 usage presence 四变体（complete/显式零/部分/缺失）与 SSE
  矩阵逐字节同语义（共享解码器）、恰一次失败无重采样；mid-stream
  Op::Interrupt → TurnAborted 无假 Complete、aborted socket 退役、立即
  follow-up 重连完成、两连接共 3 消息。首跑红为夹具（prewarm 语义），修后绿。
  顺带钉住两条行为事实（非缺陷）：abort 后重连不再 prewarm；WS 与 SSE 的
  cap/usage 语义一致。

## 6. thread/read 重连分页与 ACP 消费（commit `930d7367b` + ACP `1669028`/`44791bb`）

- **冷重启身份一致**：`cold_reconnect_replays_identical_history_across_read_
  resume_and_pagination`——3 回合线程（成功命令工具、provider 失败回合、文本）
  跨 热全读/limit=1 分页合并/热 attach/冷重启 四视图身份、顺序、失败态、
  条目完全一致。首跑红为夹具过严（live 历史把结果嵌进 CommandExecution）。
- **call/result 分页边界**：`thread_items_list_limit_one_pages_slice_between_
  call_and_result_items`——Asc/Desc limit=1 每页 ≤1、call/result 相邻不被拆散、
  合并重建精确序列；含 in-flight McpToolCall 边界。
- **ACP 两个真实产品缺陷（已修，`1669028`）**：
  1. `CodexToolCallMapper` 丢弃 `ImageGenerationItem.failure`——失败的工具
     调用在 ACP 侧看不到失败详情；现 content 带失败文本、rawOutput 带结构化
     failure。
  2. loadSession 后的 `thread/tokenUsage/updated` 恢复回放被丢（handler 仅在
     prompt 活跃时注册）——违反 app-server"客户端需在下一回合前拿到恢复用量"
     合同；现 client 缓冲无 handler 线程的最新恢复通知、late 订阅者 drain、
     装载后发 usage_update。
- ACP 测试 14/14（targeted），全套 806 pass/32 skip；typecheck 干净。

## 7. 51 轮压测（Linux，codex CLI `4c241421…`，ACP dev `be81fe1`）

**51/51 执行，exit 0**：三 wire 各 17 轮固定表（4 文本、3 真工具图、
2 cancel-before-headers、2 cancel-mid-stream、1 cancel-during-tool、
2 compact+continuation、2 cold-restart），合计取消 15（三阶段均覆盖）、
工具图 12、compact 6、冷恢复 6。每 wire：mockOpenSocketsAfter=0、
shutdown 10–20ms、shot-server 子进程全部退出（poll ≤10s）。
逐轮收据（HTTP delta、openSockets、RSS/FD 序列）在 Linux 机
`/tmp/acp-val-stress.*/stress-results.json`。
压测器自身的四处夹具修复（force 锚定真实指令、冷恢复重挂 MCP、
stats/pgrep 自匹配、bash3.2 兼容）随 ACP `be81fe1` 入库；每个修复有
对应失败轮次证据（tool 预算循环、not advertised、假泄漏、mac 单 wire 中断）。

## 8. 后台记忆巩固正常退出（P2，commit `b45cfeab6`）

- 首红回归：`phase2_releases_claim_promptly_after_normal_agent_shutdown`
  （旧码 `SkippedRetryUnavailable`——一小时不可再领）。
- 修复：`release_global_phase2_job(token, retry_delay)` 严格 ownership 校验
  （`status='running' AND ownership_token=?`），不清 watermark/失败预算；
  `AgentStatus::Shutdown` → `canceled_agent_shutdown`（独立指标标签，不记
  失败），释放延迟 5s（原 3600s）；agent 未真正停止则保留 lease；
  SIGKILL 仍走 lease 到期恢复；无新常驻服务。
- 新 API 回归：stale token 不能释放新 owner；真实失败仍消耗预算 + backoff；
  立即可重领。收据：memories-write 57/57、state 231/231、fmt/clippy 干净。

## 9. Windows 原生（192.168.32.235，MSVC，源 2f2ddc157 tar）

- 磁盘：C: 162G 可用（对比上轮 173G）；测试专属临时目录 /c/tmp/codex-stab-2f2ddc157。
- **先行分诊闭环**：`retained_tool_output_survives_compaction_and_resume`
  （上轮 19:58:39Z、早于磁盘满的 ABORT）当前 SHA 原生 **PASS 1/1（6.4s）**，
  CI ubuntu 本 run 也 PASS——该 ABORT 不能再用磁盘满或环境解释为遗留缺陷，
  按"当前 SHA 已不复现"关闭；上轮 ABORT 首败证据保留在旧账本。
- bridge：301/301。
- 其后 core-family（含本轮新 rig_interrupt）→ utils-truncation →
  app-server 修复族 → env 三件套，收据补录。
- CI windows lane（windows-latest）完整跑完 27 个唯一非过（7 慢性 + 20 新形态，
  见 §1），与原生机结果分列。

**当前 SHA 原生机收据（进行中，逐轨补录）**：

| 轨道 | 结果 |
|---|---|
| build-fixtures（test_stdio_server） | exit 0 |
| abort-triage（retained_tool_output…） | 1/1 PASS |
| bridge | 301/301 |
| core-family（52 选择器同 Linux 口径） | 52/52 |
| utils output-truncation + image | exit 0 |
| app-server 修复族 | 108/109（唯一失败 = cloud-bundle，5.5s，与 Linux 同一外呼挂起定性——两台验机出口均受限，CI 通过；4 个 leaky 通过单列） |
| env 三件套 | 59/59 |

全部轨道 2026-10-16 10:15 结束；磁盘前后记录在 reval-all.log（C: 162G 起，
测试专属 /c/tmp/codex-stab-2f2ddc157，未触碰其他目录）。

## 10. 本轮提交（两仓）

| 仓 | 提交 | 内容 |
|---|---|---|
| codex（codex/rig-044-20261014） | `284709eab` | test(core): code-mode 三输出合同（真 host） |
| codex | `486d9e2a2` | test(core): chat/responses 跨 wire 取消合同 |
| codex | `930d7367b` | test(app-server): 冷重启历史身份一致 + call/result 分页边界 |
| codex | `0b5c0cdb7` | test(core): 图片生命周期六缺口（kill 恢复/fork 双 wire/热载荷/error 检查/append-only/JPEG+WebP） |
| codex | `b45cfeab6` | fix(memories): 正常 Shutdown 干净释放 consolidation claim（ownership 校验 + 5s 重领） |
| codex | `c09328aae` | test: cap 温继续 + Responses 半截工具参数 + bridge cut 单测 |
| codex | `1cc718dff` | test(core): Responses WS 的 cap/usage presence/interrupt 家族 |
| codex | `845151c70` | test(core): 新图片生命周期 helper 的 clippy -D warnings 修复 |
| ACP dev | `f8f0b3f` | 三 wire 六场景驱动 + mock + 机器可读场景表 |
| ACP dev | `f8088ef` | wire 形状修正（namespace/tool_result/output）+ 51 轮压测驱动 |
| ACP dev | `1669028` | fix(acp): 图片生成失败详情 + loadSession 恢复用量（两个真缺陷） |
| ACP dev | `44791bb` | test(acp): 上述两缺陷的回归 |
| ACP dev | `be81fe1` | 压测器加固至 51/51 干净收据（force 锚定/冷恢复重挂 MCP/probe 自匹配/bash3.2） |

构建收据（源 2f2ddc157，--locked，V8 150.4.0 artifact 对按仓 manifest 校验）：
Linux codex CLI `4c241421…`、host `9a601dde…`、exec `ac3e7b49…`、
app-server `aea2b9aa…`、fixture `82542cff…`；macOS codex CLI `3fe7388…`、
host `a22c2ae2…`、exec `14429f8d…`、app-server `0c20b872…`、
fixture `0b7abb3d…`。
