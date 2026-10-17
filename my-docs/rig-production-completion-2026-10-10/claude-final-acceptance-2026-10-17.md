# Claude Code 下一轮：Linux/容器主门验收与剩余缺陷收口

请继续开发，直接修复、测试并按机制 git commit。本轮中心是 fork Codex 通过 rig 接第三方模型，由 ACP dev 供实际使用；优先级是 Linux/容器 app-server，随后补 Windows/macOS 支持证据。

## 先读与定位

1. 读取同目录 `codex-review-fixes-execution-2026-10-17.md`，核对其中提交、实际验证和开放项。
2. Codex 候选工作树：`/Volumes/soddy/git_workspace/fork-nuwax-codex/.claude/worktrees/merge-main-20261014`，分支 `codex/rig-044-20261014`；主工作树 test 仍停在旧基线，请勿把旧源码/旧二进制当最终树。
3. ACP：`/Volumes/soddy/git_workspace/fork-codex-acp-ts`，分支 dev；main 用于同步官方，不在 main 开发。保留现有 untracked vendor，不纳入提交。
4. 使用执行记录中的真实红绿收据。先检查当前树与新提交关系，保留别人未提交工作，避免重复修复。

## 第一批：确认本轮修复形成稳定主门

- Linux 普通生产依赖构建必须通过，不能仅凭 cfg(test) 单测证明。验证 codex-cli、codex-exec、app-server 可启动；核对最终二进制 SHA256 和源码状态。
- 复跑记忆 shutdown 栅栏/发布端终态回归及真实子进程合同，重点覆盖前台先结束、agent 启动中关闭、心跳 SQL 等待期间 Error→Shutdown、1秒锁、长锁保留lease。正常取消保持预算；真实错误扣预算并保留失败 backoff；关闭未确认不得提前释放 claim。核对内外关闭预算，不改模型请求 deadline。
- 复跑 ACP load→prompt→resume→prompt、并发 load/resume、失败释放队列、close/reopen 旧代不能清新代 capture；增加真实 app-server 路径的恢复 usage_update 数值验证（现有真实 smoke 只证明 prompt/compact/continuation/cold-load）。验证 same-process reload 和 fresh-process load，既恢复快照也继续接收下一轮 usage。
- Linux Git 网络 helper 的父死/超时测试须先收到实际 HTTP GET，再杀 owner；普通 transport 全部消失。清理测试残留时先记录失败，不得把清理成功当产品通过。验证参数中空格/特殊字符和可信 PATH/Git 配置策略保持。
- 离线三 wire 场景和压力验收统一用最终二进制。预算闸门、错误颜色、过期残留、取消确认窗口的负向合同也必须通过。

## 第二批：修掉已定位但仍未完成的失败家族

1. **guardian pool_pressure**：核对生产 WS/HTTP 双路径和 prewarm/backoff 设计，用与真实路径匹配的 websocket mock 重新构造测试；分类、并发上限、取消 oldest 的行为必须真实发生。若生产没有遵守 supports_websockets，则先明确配置合同再修生产代码；只改 fixture 不足以掩盖配置合同缺陷。
2. **guardian analytics**：按 review/thread 身份配对请求和状态，允许同一 review 多次采样，避免“3请求=3review”及到达顺序假设；仍要验证真实 analytics 内容和完整生命周期。
3. **Windows 两项 TUI**：修复 config_overrides personality 传递和 approvals_popup Enter 不派发 OverrideTurnContext 的根因；保留有效选择、禁用项与参数转发断言，勿仅改期望值。
4. **Windows ConPTY Ctrl-C**：取得准确 OS build、裸原生 ConPTY 基线与当前产品参数，在 foreground child 真正运行后发送中断。cmd 与 PowerShell 同败只能排除 PowerShell 单因，不能直接定为 OS 限制。查控制台模式/VT输入/进程组/近期 console 变更；确为支持边界时给出实证和清楚的用户行为，不凭猜测跳过测试。
5. **Windows elevated_non_tty**：分别证明可执行文件、cwd、账户/profile 路径、CreateProcessWithLogonW 输入，定位 error2 的具体对象；确有 provisioning/路径产品缺陷则修。保留环境修复步骤和原始首败，不笼统写“机器问题”。
6. **CI 账本**：逐项归因 ubuntu hooks/guardian cancel 及慢性项、Windows 新形态；同 SHA/同机基线差分支持环境定性。macOS 180分钟超时考虑分片/并发/耗时分布，保留全部必需测试覆盖，不以删测试换绿。

## 第三批：一次最终验收并给出可发布判决

- 以 Linux/真实 app-runtime 容器为主门，三 wire 各跑启动、文本、真实 shell、MCP typed 图片、长会话压缩、冷恢复/重连/分页、取消及错误恢复。检查服务关闭后的 socket/PID 树、数据库 claim、用量连续性和日志凭据。默认/受限沙箱与FullAccess分别验证；bwrap/AppArmor受限时给出原生探针和容器权限要求，不能把FullAccess通过外推为默认沙箱也可用。
- 做一次不少于4小时的离线混合场景运行，累计至少200轮，覆盖截图与多次压缩、取消前/中流/工具阶段、断连重连/冷启动。使用可复现 seed 和明确 deadline；首败可保存并单独复现，整轮不得靠自动重试记绿。
- 用户已经明确允许调用现有 Token Plan 模型做必要测试，无需再因“真实API/付费”询问而停工。凭据只读主仓忽略文件 `/Volumes/soddy/git_workspace/fork-nuwax-codex/.env.local`，不要写入源码/文档/命令回显。真实模型均通过 ACP 仓新的 live-validation 共享网关，记录每个真实 HTTP attempt，保留同一 run 的账本和截图 seed；新测试范围可建立合理的新 run 预算。覆盖主用 GLM、备用 MIMO、STEP 已提供的协议；未提供的端点不外推能力。
- 真实截图只评分最终 assistant 答案，随机图明确问“数字与背景颜色”；真值和图片字节不可在重试时换一批。不要用工具文本、文件名或较早回答满足识图断言。
- 在资源合适的机器跑最终源码的完整离线 workspace/平台测试；不混入无上限 live 测试。完成 scoped clippy、fmt、diff-check 和 Bazel lock 检查。把既有失败、环境限制、未执行项分别保留证据。
- 保持默认代理路径、V8 pin与D6人工门的现有边界。
- 本轮不再重复大范围合并 main 或升级 rig；当前迁移已完成阶段验证，优先解决上述确定缺陷。只有阻断本任务的兼容性问题才引入额外依赖改动，并说明原因。

## 交付

逐机制提交到现有逻辑分支，不 push、不发布。交付一份可直接给 Codex 复审的短记录：两仓最终 SHA/干净状态、源码与二进制对应关系、首败→修复→最终测试、真实请求账本、完整开放项。给出明确判决：Linux/容器的 ACP 使用路径能否进入持续使用、Windows/macOS 哪些能力仍受限。

如果某项受外部环境阻断，继续其他独立任务，保留精确复现和恢复条件。交付中区分已修、已执行验证、仍阻断；不要把“已分诊”写成“已完成”，不要只靠测试总数宣称全部收口。
