# Codex 复审包：最终验收轮（2026-10-17/18）

给 Codex 的复审输入。本轮 16+1 笔提交（Codex 10 / ACP 6），全部本地提交未 push。
自查已完成一轮（逐笔 diff 复核），发现并修复 1 处调试残留（`ed25d2e78`）。

## 仓库状态

| 仓 | 分支 | HEAD | 基线 | 状态 |
|---|---|---|---|---|
| Codex | codex/rig-044-20261014 | `ed25d2e78` | `e5b0eaa0a`（复审轮顶） | clean |
| ACP | dev | `d39acbf` | `6dec6c8`（复审轮顶） | clean（vendor/ untracked 照旧） |

Linux 最终二进制（源码 `a6d34d279` 时点；其后提交均为测试/Windows 专属/文档）：
codex `856a0a4a…77939`、codex-exec `846e1d63…c03ee`、codex-app-server `ab92bf54…40bd4`、
test_stdio_server `1b9cbad4…d9299`。

## Codex 提交（按序）

| 提交 | 类型 | 内容 | 验证 | 复审要点 |
|---|---|---|---|---|
| `a6d34d279` | 产品 | Luna pool `start_refill` 前置 `supports_websockets` 检查：false 时跳过 prewarm（避免失败 upgrade 触发 5min CONNECT_COOLDOWN 饥饿） | 红 3/17 POST→绿 3.6s；guardian 单测绿 | **行为变化**：HTTP-only provider 不再尝试预热。若上游认为应保留"尝试后回退"语义请指出 |
| `0d0559de4` | 测试 | analytics 场景按请求身份路由（guardian 标记 + 因果序列 + 命令历史识别），断言按 turn/thread 配对 | mac 4/4 连续；**Linux 见开放项 1** | P1 continuation 截流的 `web_search` 匹配在 Linux 有竞态（见开放项） |
| `6d3a414ea` | 产品 | TUI 不再从全层 `config.personality` 转发 opt-out（User/Project 层是 daemon 已读的共享配置；SessionFlags 层已覆盖 CLI 场景） | mac+Windows 同红→双绿；tui config_overrides 4/4 | 删除的 `personality_opt_out_only` 无其它调用者（已核）。**语义变化**：remote overrides 不再携带共享配置层的 personality——如上游认为应转发请指出 |
| `c0f5b7bda` | 测试 | approvals_popup 接受 Windows 的 OpenWindowsSandboxEnablePrompt 完成形态（携带同 preset） | Win+mac 2/2 | 保留 on-request/never 断言，未放宽 |
| `a63c08f0f` | 测试 | ConPTY ctrl-c：build≥26100 断言降级行为（pty 仍收输入、terminate 仍可用）；<26100 保持原断言 | Windows 验证机 PASS | 三层实证（产品 API/GenerateConsoleCtrlEvent/裸 FFI）；`windows_build_number` 改 pub(crate)、`psuedocon` mod 改 pub(crate) |
| `6556e609c` | 产品 | command-runner helper 缺失时 fail-fast（列搜索位置），不再裸名 fallback；测试 guard 从 cargo target 布置 runner | Windows：error 2 消除、runner 启动 | **行为变化**：异常布局下从"可能 PATH 命中"变为硬错误。runner pipe 握手超时仍开放 |
| `a5f25ac17` | 测试 | 两个预存 clippy 测试构建问题（文档注释空行、dead helper） | clippy 4 crate -D warnings 过 | 无行为影响 |
| `fbd1d6243` | CI | nextest 给 analytics 场景 4×1m 慢窗（默认 2×30s 在慢机先杀） | — | 与既有 packaged-daemon 等 override 同惯例 |
| `df3c9fe31`/`859f51090` | 文档 | CI 账本、执行记录 | — | — |
| `ed25d2e78` | 修复 | 删除 `a6d34d279` 误提交的诊断 eprintln | build 过 | 自查发现 |

## ACP 提交（按序）

| 提交 | 类型 | 内容 | 验证 | 复审要点 |
|---|---|---|---|---|
| `31d1d99` | 测试基建 | mock chat 响应补 usage-only 尾帧（include_usage 契约；input4/output1/total5） | mock 单测 + restored-usage 红转绿 | 之前 responses/anthropic 有、chat 缺——所有历史 chat-wire 验证从未产生过 usage |
| `8939859` | **产品** | warm（同进程）reload 无 app-server 快照重放时，继承被替换 SessionState 的 usage 账本并按同形 usage_update 发布；抽出共用 `publishSessionUsageUpdate` | restored-usage：used=5 三点一致 + 后续轮持续；npm 823 过 | 边界自查：serializeSessionOpen 串行保护；首次 load/fresh-process 跳过；冷路径快照优先（lastTokenUsage null 判定在 applyRestored 之后）。**继承源是内存账本**——warm 路径 app-server 不重放，这是唯一恢复源，语义自洽 |
| `5869e00` | 测试基建 | mock 增加 exec_command 脚本触发（真实 shell 工具往返） | 容器双沙箱探针 | cmd 按 schema 为 string |
| `0e0dcc3` | 测试基建 | 三处锚定排除 `<environment_context>`（跨日注入会遮蔽真指令）；image label 检测移到 tail | 单测 24/24 + 保存的失败请求回放（image/red 对真值）；**见下方矛盾标注** | `labelInCurrentTail` 使 image 分支触发面略扩大（tail 含 label 即触发），有 stale/missing 兜底 |
| `73ac0b8` | 测试基建 | mock 请求预算 160→200（18 轮表+偶发重试贴线） | 长跑 | 防失控语义不变 |
| `d39acbf` | 测试基建 | 长跑每批随机 mock 端口（固定 47713 连跑数十批后 EADDRINUSE） | **94 批全绿** | — |

## 必须知道的证据边界（如实声明）

1. **`0e0dcc3` 与长跑绿收据的版本差**：94 批/5076 轮/4h16m 的绿收据用的是**不含**该修复的
   mock（`5869e00` 版）——当时把 compact 500 误归因于该修复而回滚，后证实 500 真因是端口
   EADDRINUSE；最终绿跑时间窗不跨 16:00 UTC，未触发跨日缺陷。**该修复的证据是单测+失败
   请求回放，未经长跑二次实跑**。下轮长跑跨日窗口可补。
2. 长跑首启曾因我与容器门/离线/live 同机并发争用失败一批（mock hang 探针超时）——调度
   失误已记录，隔离后 94 批全绿。

## 门与收据（详情见执行记录 §3.5）

容器 18 场景 + 54 stress + 双沙箱探针；真实 API 12/12（21/72 attempts、GLM/MIMO/STEP）；
4h 长跑 5076 轮零失败；clippy/fmt/bazel；core-plugins 480、memories 288、exec 合同 4、
restored-usage 数值验证。凭据 Linux 副本已删、live 目录扫描零命中。

## 开放项（优先级序）

1. **analytics 测试 Linux 收据**（取证完整）：continuation 截流的 `web_search` 匹配竞态吞
   parent-second/third 主 review（速度决定；mac 快时常躲过）。已证：真正的"对 web search 的
   续 review"不存在（guardian 不对 review 自身 search 续审批）。已试反证：`web_search=disabled`
   使 scripted search 无法执行；`outcome+enum` 截流误吞 v1 review。方向：等待循环改数
   sequence 消费（精确）+ 以请求体可区分特征路由分类与 review（需先 dump 两者 schema 差异）
2. elevated runner pipe 15s 握手超时（helper 布置后 runner 启动、确定性复现）
3. guardian-v2 组内并发时间敏感（预存；单测全绿）
4. macOS CI 分片（workflow 改动，建议在 CI 账本）
5. workspace 全量离线未跑（分轨道覆盖）
