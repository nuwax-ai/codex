> 2026-10-18 Codex复审更正：下表是历史分诊。原ConPTY版本门控已撤回，Windows中断/runner握手仍是兼容性尾项；analytics原web_search字符串路由有误，本轮已用当前planned action与实际review握手修正。四分钟宽限已移除，不能再以该宽限作为解决路由错误的证据。

# CI 平台账本（最终验收轮，2026-10-17）

本轮在候选分支 `codex/rig-044-20261014`（当前 `6556e609c` + 后续）上对 CI 失败家族逐项归因并修复。
所有定性均有同 SHA 真机复现或红绿收据支持；未用删测试/放宽断言换绿。

## ubuntu

| CI 失败项 | 定性 | 处置与证据 |
|---|---|---|
| guardian pool_pressure（ubuntu+windows 双红） | **产品缺陷**：ConnectionPool 的 prewarm 不检查 `supports_websockets`，HTTP-only provider 上失败的 upgrade 触发 CONNECT_COOLDOWN(5min) 饥饿，分类流量 30s 内仅 3/17 到达 | 产品修复 `a6d34d279`：`start_refill` 尊重能力合同。红收据（修复前）：3/17 POST + upgrade 撞纯 HTTP mock；绿收据：pool_pressure 3.6s 通过 |
| guardian analytics `guardian_review_turns_and_tools_reach_analytics`（慢性） | **测试缺陷**：固定请求序列假设 3 请求=3 review，review 内 web_search 触发续采样致序列错位、身份断言错配 | 重设计 `0d0559de4`：请求按身份路由、断言按 turn/thread 配对。红收据：thread_id 错配 panic；绿收据：4/4 连续通过 |
| ubuntu hooks（前轮已处理） | 前轮 `203679e02` DNS fixture + 字面量已修 | 本轮 CI 对应应绿 |

## windows

| CI 失败项 | 定性 | 处置与证据 |
|---|---|---|
| unified_exec ×4 / cur ×4 | CI runner 特有（真机全过，上轮同 SHA 基线差分） | 无需代码改动 |
| TUI config_overrides personality | **产品缺陷（跨平台）**：TUI 从全层 config.personality 转发 opt-out，User config.toml 的值泄漏进 remote overrides | 修复 `6d3a414ea`。mac+Windows 同红（首迭代断言失败）→ 双平台绿 |
| TUI approvals_popup navigation | **测试平台假设**：Windows 上确认 preset 先走 OpenWindowsSandboxEnablePrompt（合法安全流程） | 修复 `c0f5b7bda`：接受两种完成形态、保留 on-request/never 断言。Windows 2/2 绿 |
| conpty_ctrl_c_interrupts | **OS 能力边界（实证）**：Win11 24H2+（build 26100+）ConPTY 不再把输入管道的 0x03 翻译为 ctrl-c | `a63c08f0f`：build 门控 + 降级行为断言（pty 仍收输入、terminate 仍可用）。基线证据：build 26200.9711 产品 API 层 0x03 不中断、GenerateConsoleCtrlEvent 到不了 ConPTY console、裸 FFI 探针连文本都不达（仅 shipped 管道拓扑有意义） |
| elevated_non_tty error 2 | **产品报错缺陷 + 测试环境缺 helper**：helper 缺失 fallback 裸名 → 沙箱用户 PATH 解析不到 → 误导性 error 2 | 修复 `6556e609c`：fail-fast 明确报缺失文件与搜索位置；测试 guard 自动从 cargo target 布置 runner。Windows 收据：error 2 消除、runner 启动。**开放**：runner 启动后 15s pipe 未连（确定性，待下轮） |

## macOS 180 分钟超时

CI macOS job 在旧基线跑满 180min 超时。本轮无法取得 CI 时间线（不 dispatch），以本地同树耗时为代理：
- guardian-v2 全 crate（-j2）：~186s；单测试 3-20s；组内并发有预存时间敏感失败（connection_pool 组 7 并发 4 红，单测全绿，基线复核过非本轮引入）
- core 家族需 `-j 1`（本机负载干扰已知）
建议分片（保留全部覆盖，不删测试）：
1. 按crate 分片：guardian-v2、tui、core、app-server、exec 各自 job（guardian-v2 + app-server 单独化即去掉最大两块）
2. core 内部按测试二进制再分（tests/suite 与单元分开）
3. 或对既有 job 提高 CI runner 并发度并保留 nextest profile=ci 的重试=0 语义（当前慢主要来自串行子进程合同类）

## 本轮未处理的 CI 项

- analytics/analytics 家族的 ubuntu CI 复跑结果需 CI 实测确认（本地 4/4）
- macOS 分片需要仓库 workflow 改动（本轮不 push，交 Codex 复审后统一上）
