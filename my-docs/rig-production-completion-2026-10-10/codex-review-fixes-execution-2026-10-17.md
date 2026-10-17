# Codex 复审修复执行记录（2026-10-17）

本轮输入为 Claude 13 笔 Codex / 10 笔 ACP 提交与 Codex 复审确认的 6 处产品缺口、4 处验收缺口。用户授权直接修复并本地提交，也已明确允许调用现有 Token Plan 模型测试。本轮没有 push。

## 1. 提交与位置

| 仓库 | 分支 | 本轮提交 | 内容 |
|---|---|---|---|
| Codex | codex/rig-044-20261014 | a55cdd0fc | Linux 普通依赖修复、startup Git/transport 进程组监督、真实 GET 后父死与超时回归 |
| Codex | codex/rig-044-20261014 | b45fea55e | 发布端保留终态、关闭注册栅栏、claim 取消/失败/关闭未确认分类、关闭预算及回归 |
| Codex | codex/rig-044-20261014 | df13b04d4 | 终态锁先释放再await；spawn/queue错误载荷装箱；最终lint收尾 |
| ACP | dev | c2acd9c | load/resume 按会话与关闭代串行、恢复 usage 消费、capture owner 与回归 |
| ACP | dev | 6dec6c8 | 共享真实请求网关、严格图片/最终答案、取消窗口、残留失败、标准验证入口 |

Codex 工作树 `/Volumes/soddy/git_workspace/fork-nuwax-codex/.claude/worktrees/merge-main-20261014`；ACP `/Volumes/soddy/git_workspace/fork-codex-acp-ts`。主工作树 test 仍是旧基线 `2f2ddc157`，本轮保存于候选分支。ACP 既有 untracked `vendor/` 未纳入提交。

## 2. 产品修复

1. **Linux 正式构建**：libc / codex-utils-pty 从 dev-only 移为 Linux 正常依赖。普通 core-plugins library、CLI、exec 构建通过；cfg(test) 不再掩盖生产依赖缺失。
2. **Git helper 生命周期**：Linux 使用固定 `/bin/sh` 监督脚本，参数始终通过位置参数传递，保留 cwd 与环境 set/remove；独立进程组在 owner 死亡时由 TERM trap 清组，超时直接杀组。回归先接收实际 HTTP GET，再杀 owner，覆盖已启动的 remote-http helper。保证普通同进程组 transport，未宣称主动 setsid 脱组 daemon 也被覆盖。
3. **记忆终态**：Session 发布端保留本轮 Completed/Errored，Running 清除上一轮结果，Shutdown 不覆盖。memory 从发布端保留的本轮终态快照（进程内）恢复分类，watch 仍明确是可合并的 latest-value 订阅。真实错误不再被正常退出改成短重试取消。
4. **关闭栅栏**：ThreadManager 在注册表同锁下关闭启动准入；启动中的 session 最终注册再次核对，拒绝后关闭未注册 session；关闭后后台 future 不再 detach。phase2 对明确取消释放 owned claim；真实启动错误仍扣预算；已启动但关闭未确认继续保留 lease。
5. **ACP 恢复**：同 session/close generation 的完整 load/resume 串行，失败 finally 释放队列，关闭后的旧队列不能复活会话；resume 安装主动消费恢复 usage。capture owner 防止旧请求 settle/clear 新代状态。64 项准确称为 settled 窗口的软目标。

关闭预算明确调整：processor 由外层5秒改为25秒以覆盖内部 thread10秒+tracked cleanup10秒及前置drain余量；outbound仍5秒、analytics仍25秒，ACK总界55秒。模型请求 deadline 没有修改。正常退出实测仍在约1秒内；长锁按合同有界退出并保留 lease。

## 3. 验收器修复与开发中首败

- **每次真实 HTTP attempt 先扣账再出站**：所有 exec/vision/ACP 路径共用网关，包含重试/工具续轮/辅助采样；账本持久化、同账本单 owner，残留锁保守拒绝接管，重定向不透传 Location。journal 只写安全计数/协议/厂商/状态，不写凭据或正文。
- **严格图片评分**：要求 exec成功、turn完成、工具成功、typed image、唯一真值与最终assistant颜色均成立；`screenshot captured` 的 red 子串不能满足答案。
- **取消/残留**：发取消通知前保存请求游标，覆盖通知至响应确认窗口；先保存超时残留失败，再清理，清理成功不翻绿。
- **标准入口**：Vitest 与 node:test 分开，保留实际存在的 next-preview-version Vitest suite；npm test 移除默认重试；typecheck 覆盖两组脚本。
- **实网首败1**：Anthropic 网关只接/messages，而真实rig会发/v1/messages。首轮0上游请求即404；按rig基址归一规则修复，补 bare/v1/完整endpoint 的真Codex本地sink与单测后实网通过。
- **实网首败2**：ACP helper 把告警/压缩通知与模型文本混拼，尽管各RPC正常仍严格字面失败。按messageId和final phase收集答案，保留严格比较，补纯本地注入回归及真ACP本地smoke后通过。
- **实网首败3**：随机图白字/彩色背景，但旧问题只问color；第三张答digit=7 color=white（另外两张正确）。明确问背景色，保留 seed438073469 及原三张PNG字节，再测3/3通过；仍要求全部正确。重测缺损fixture会失败，不重新随机生成。原失败图与输出在Linux `/tmp/codex-oct17-live-review/first-red-vision/`。
- **编译首败**：新增core测试glob导入使assert_eq宏歧义；显式导入pretty_assertions后4/4通过。没有改断言或重试转绿。
- **lint收尾**：新异步状态发布使原match临时MutexGuard跨await，拆成独立语句先取出terminal_error并释放guard；另将既有ThreadSpawnError::Backend和QueueServiceError::CoreSubmissionError装箱并保持From/错误显示与映射语义。队列错误的Rust载荷现为Box，未承诺source动态downcast类型不变；仓内消费点不依赖该类型，wire/schema未变。

## 4. 验证收据

| 项目 | 实际结果 |
|---|---|
| Linux 普通 core-plugins library / CLI / exec build | 通过 |
| Linux startup_sync | 25/25，包括GET握手父死/超时helper清理 |
| Core 发布端终态/关闭栅栏 | 初次4/4；lint收尾后含终态发送/多代理spawn的最终18/18，零重试 |
| exec 真实 memory_exit_contract | 4/4，零重试，含快速前台退出/写锁/失败预算/长锁lease |
| memories-write + state | 288/288，test-threads=2，零重试 |
| app-server关闭预算（paused clock） | 1/1 |
| ACP 标准 npm test | 823通过 / 32既有skip，63套通过，退出0，默认零重试 |
| ACP typecheck | src/examples/acp-validation/live-validation四组通过 |
| Node mock/压力合同 | 28/28 |
| Node 网关/最终答案合同 | 9/9 + 3/3 |
| Python live合同 | 10/10 |
| Linux ACP离线原生三wire | Chat/Responses/Anthropic各6/6，合计18/18 |
| Linux三wire压力 | 各18/18，合计54/54；每wire verifiedImage4、probeFailures0、socket残留0 |
| scoped Clippy | Linux core-plugins/core/memories-write/app-server，lib、rust-rig、-D warnings通过 |
| 改动Rust格式/diff-check | 通过 |
| Bazel lock update/check | 通过，无lock内容变更；check显式使用Python3.14，系统Python3.9无法解释现有wrapper类型注解 |

### 独立真实子进程 oracle

- 心跳90秒处遇3秒DB写锁，期间真实400错误再Shutdown：93.487秒rc0，最终 `error / retry_remaining=2 / last_error=failed_agent / +3600s`。
- 取消人工“等memory请求”门的快速前台退出：0.782秒rc0，最终pending、无lease、预算3。
- 1秒普通写锁：1.717秒rc0，pending、无lease、预算3。

这些是localhost dummy模型的真实exec进程测试；没有使用禁网环境变量将测试跳过。完整行见 [memory-oracles.json](artifacts-codex-review-2026-10-17/memory-oracles.json)。

### 真实 Token Plan

最终12/12：本地env决定性预检、GLM Chat、STEP Anthropic、GLM Responses、MIMO切换、文本、冷恢复召回、真实shell落盘、流中取消、MCP截图、三张随机数字/背景色图、ACP prompt/compact/continuation/cold-load。

共享账本总计 **33次真实上游attempt**（包含首败及修复后复测），预算96，denied0、active0。最终随机图3/3，seed与PNG字节经独立比较不变。final summary里的每case数只代表最后一次区间，不能相加冒充整个历史总数；observedExecUsage也不是所有ACP/辅助路径的完整token账。

凭据仅从主仓忽略的.env.local读入网关；Linux临时副本0600。扫描本次live目录49,546个文件，供应商凭据匹配0；测试后已删除本轮创建的远程凭据副本。journal/summary入库，不入库原始home、图片base64或凭据。

收据：[最终结果](artifacts-codex-review-2026-10-17/live-summary.json)、[逐请求账本](artifacts-codex-review-2026-10-17/live-request-ledger.jsonl)、[含首败的case历史](artifacts-codex-review-2026-10-17/live-case-history.jsonl)、[压力汇总](artifacts-codex-review-2026-10-17/stress-summary.json)。

## 5. 源码与二进制对应

Linux验证副本 `/tmp/codex-stab-final` 沿用此前候选源码。模型/压力验证检查点b45fea55e的17个改动Rust/Cargo文件与本地逐个SHA256比较一致，见 [检查点收据](artifacts-codex-review-2026-10-17/source-receipt.json)。lint收尾df13b04d4后共20个改动文件再次比对一致，见 [最终源码收据](artifacts-codex-review-2026-10-17/final-source-receipt.json)。该副本无Git元数据，不冒称自动收据是最终commit的clean工作树。

- 下列live二进制属于b45fea55e产品验证检查点；随后df13b04d4只收紧锁作用域及装箱错误载荷，已重新通过最终Core18测试和四crate Clippy，未重跑整组真实API。
- CLI SHA256 `fac79c7cf93512c141a5d7ee73fe44f7f76b333d43f4825eaf820dd2c71573a6`。
- exec SHA256 `0b94c41983be41dbd376884d02e84b4ad2f7ffe4bc158e1d23f4baf310f2dee8`。
- Linux rustc1.95.0，Node24.21.0；源码/功能改动没有涉及V8 pin、provider schema或默认代理迁移。
- 本轮没有重新安装覆盖日常二进制，也没有把候选分支合并到主工作树test。
- live exec使用隔离临时home/work和FullAccess；通过证明模型/工具主链，不外推默认沙箱或容器权限已经验收。

Linux验机缺Clippy组件，rustup按当前安装清单补组件会panic回滚。本轮校验已有缓存SHA256 `ac779bc9839dd47180806b133e4e2563c4a34716284cd5b8fede8ef289f452ca` 后，仅将Clippy1.95解压到 `/tmp/codex-oct17-clippy`，库链接指向既有1.95工具链；没有修改全局安装清单。后续验机命令请显式使用已有toolchain/bin，Clippy加此前临时bin并设置 `RUSTUP_TOOLCHAIN=1.95.0-x86_64-unknown-linux-gnu`，避免重复触发该环境故障。

## 6. 剩余范围与下一轮

本轮6产品+4验收缺口已落修复并完成上述验证。尚未执行完整workspace/Windows/macOS全量、4小时长跑、真实容器镜像发布或真实恢复usage数值端到端验收。真实ACP smoke的冷load只证明RPC/生命周期成功，恢复usage的两处缺陷本轮以专项产品回归证明。

仍开放：guardian pool_pressure与analytics fixture/合同、Windows两个TUI、ConPTY/沙箱error2的源码与原生基线归因、CI平台账本与macOS分片、长会话/容器最终主门。不能仅凭cmd也失败或error2就把相应项定为“非产品缺陷”。

下一轮完整任务：[claude-final-acceptance-2026-10-17.md](claude-final-acceptance-2026-10-17.md)。目标是关闭确定遗留并给Linux/容器主路径可持续使用的清楚判决；不再借大范围依赖升级扩张范围。
