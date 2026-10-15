# 给 Claude Code：Linux／容器 ACP 主路径稳定收口

请从 Codex 本次复审、修复并 push 后的最新 `test` 开始。目标是完成一批
能独立验收、能日常使用的结果：通过环境变量配置第三方模型，启动 Codex
app-server，由 ACP dev 驱动，完成文本、工具、截图、取消、恢复与压缩。
本轮需要实际修复和验收，不能再次只交一份首败账本便停止。

本文件替代旧 `claude-next-ci-validation-2026-10-16.md` 的下一轮安排。
先读本次 Codex 复审提交及收据；旧 `tasks.md` 是较大的历史复合清单，
不能把其中所有开放框都变成本轮发布门，也不要重复已完成的 main 合并、
Rig 0.44 升级、图片入口预算和旧 fixture 转换。

## 工作边界和稳定门

- Codex 主仓：`/Volumes/soddy/git_workspace/fork-nuwax-codex`，分支 `test`。
  开始记录 HEAD、origin/test、工作树、工具链、features、依赖锁和执行
  二进制收据。尊重并行改动，不 reset/stash 他人的工作。
- ACP 仓：`/Volumes/soddy/git_workspace/fork-codex-acp-ts`，使用 `dev`；
  ACP 的 `main` 只同步官方，不能合错目标。允许为验收器修复修改 ACP dev，
  两仓分别提交。官方 Codex／Rig 参考源码只读。
- 用户已撤销先前给出的 AGENTS.md 指令，本轮不复引。测试使用当前仓库
  提供的命令和默认配置，工具链／栈／features 不得因脚本遗漏而悄悄改变。
- 本轮不 push、不手动 dispatch、不发布/tag/npm、不调用真实付费 API。
  完整 workspace 留给自动 CI；本地做与改动有关的充分定向验证。
  真正网络／视觉能力验证待新授权，不能因此阻塞离线修复。
- 冻结上游 main、Rig 和 V8 版本，不扩建新附件/File API 平台、不重做
  guardian 架构，不修改全局代理／信任安全默认值。不操作已取消的 artifacts
  迁移，不删除外接盘、共享target、用户配置和账户数据，不写入真实密钥；测试使用专属端口／
  容器，不停止机器已有服务。
- 首要稳定门是 **Linux／容器 + app-server + ACP dev + 第三方 provider**。
  该路径真实失败必须修好；官方整个 22K workspace、非目标 marketplace、
  实验性 guardian 异步能力、Windows 全部历史问题分别说明，不绑架主门。
  “基线同败”只能说明起源，不能自动宣布可接受。

## 执行方法

下面 10 项按依赖推进，分机制独立 commit。第 1 项先做；第 2 项打好验收
驱动后，第 3–6 项可并行分析；第 7 项使用它们稳定后的版本；第 8、9 项可
独立推进；第 10 项最后生成交付物。每项先查已有测试与本次收据，已有完整
证据就引用并验证当前版本仍通过，只补真正缺口。

每遇红项：保留首败 → 判定被测代码、fixture、机器条件 → 最小修复 → 原
选择器复验，再跑相邻回归。不得靠 skip、吞错误、增 deadline／栈、取消
硬限、关闭 sandbox 安全默认值或重复重试直到绿来关闭。平台不可达时记
明确阻断并继续独立任务；某项难题不能让另外九项只剩“下轮再做”。

### 1. 当前集成 SHA 的 CI 和 Linux 主路径

读取最近自动 CI 的 `headSha`，不能把旧 run `38782462743` 当作新代码
结果。下载原始首败和最终 summary，FAIL／FAIL+LEAK／TIMEOUT／SIGABRT／ABORT／LEAK／
SKIP分列（LEAK是通过但资源泄漏，不能混为FAIL+LEAK），按完整 crate+test 去重，取消 job 单列无结论。Windows 作业若因磁盘写
失败中断，不得把截断日志叫完整套件结果。

在 Linux 原生主验机和一个真实容器记录 kernel、arch、rustc、features、
栈、userns/bwrap、临时目录和磁盘条件；先构建必需 fixture，再运行新
修改家族及当前主路径失败簇。当前 Codex 已修的文本限额／fs_watch／usage
合同等直接复验，不按旧账本重新实现。AppArmor／网络 namespace 故障要
用独立探针校准；不能把真实主路径失败笼统写成“环境性”。

定向构建必须显式准备 `codex-code-mode-host`、`test_stdio_server`、
`codex-exec` 三个可执行文件，不能认为 `-p codex-core --test all` 会替你
构建其他 crate 的程序。启动前按测试的 `codex_utils_cargo_bin::cargo_bin`
定位规则预检：显式 CARGO_BIN_EXE 路径或当前 CARGO_TARGET_DIR 对应的
debug 二进制均需存在、可执行且属于本轮构建；记录实际解析路径/hash。
`CODEX_REPO_ROOT` 指向此次源码，工具链、profile、features、
`RUST_MIN_STACK=8388608` 与当前 `just test` recipe 一致；直接调用 nextest
时显式传递，不沿用 shell 偶然状态。隔离源码复用旧 target 时同样检查。
缺少 host 会使 builder 的可选解析返回 None、Code Mode 降为 Direct，
随后 mock 发出的 custom `exec` 整簇报 `unsupported custom tool call`；
本轮已取得这种 fixture 假红的直接证据，不得当成产品回归或通过跳测关闭。
必须补齐 host 后原选择器复验；若 paired V8 构建条件缺失，保留构建阻断，
不改 V8 pin，不假称 Code Mode 已验证。

验收：当前源码启动的 app-server 实际接到 mock HTTP，请求中 provider、
endpoint、model、wire、工具往返和失败状态可断言；定向主门全绿。CI 其余
失败给出“阻断主门／独立缺陷／环境待复验”具体名单。若 CI 工具链仍漂移，
让主 lane 跟仓库 pin 一致；浮动 stable 兼容验证另列，不悄悄换版本求绿。

### 2. 收紧已归档 ACP 六场景驱动，补齐三 wire

复用 ACP dev `scripts/acp-validation/`，不要复制另一套 tmp 驱动。
保留并扩展本轮已落地的 mock 更正和自动标题活动模型修复：先前 `unsupported call` 与取消后旧 prompt
现象不能预设为 Codex 产品缺陷。mock 必须按当前实际广告的工具目录、
Code Mode 调用路径和结构化消息分段工作，按场景／回合标识选择响应；不能
把控制片段、系统说明或旧工具消息误认成最新 user prompt。验证器本身加
错误 mock 输入和跨回合污染回归，然后再裁定是否有真实产品问题。

把既有 S1–S6 的含义写进机器可读场景表，覆盖启动文本、真实工具往返、
截图、取消、压缩／继续、冷恢复／召回。Chat、Responses、Anthropic 各跑
六场景；仅本地 mock，无真实凭据。每场景保存二进制收据、请求数、关键
事件及结果，响应与断言按结构解析，不能靠日志含某字符串判绿。

验收：**18/18 实际执行**。S1检查实际ACP建会话与MCP初始化，不强迫
无推理的bootstrap额外调用模型；S2–S6检查目标HTTP非零和预定attempt预算。
取消后follow-up确实进入新请求，工具目录和执行使用同一份实际声明。任何提前返回均标
SKIP，不计执行通过；原首败与修正后的 mock 差分保留。

### 3. 真实 Code Mode 的三种输出合同

通过实际 Code Mode runtime 分别执行 `image()`、普通 JS return、显式
`text()`。不要用另一个 helper 单测替代执行链，也不要以 Code Mode 未
真正启用的普通 MCP 回合充数。

验收：`image()` 生成且仅生成一次 typed 图像；普通 return 遵循现有
不主动发内容合同；`text()` 是有界文本，任意 base64 字符串不猜成图片。
在实际出站请求确认 typed 路径、调用/结果配对、detail、payload 大小和
协议转换。同时覆盖错误、取消及一份超限结果，超限不会变成下一轮巨量
JSON。保留 structuredContent 既有优先级；若已有问题，先复现再最小修复。

### 4. 图片生命周期与共享预算的最终缺口

复用既有三 wire 图片历史、20 截图和原图测试，只补缺失：正常桌面
PNG/JPEG/WebP、方向信息、异常 metadata、小 original、多个 original
共享限额、小上下文窗口、未知 File 引用的保守行为。真实图片只用合成
素材，不保存用户截图。

验收：热继续、关闭进程冷恢复、fork、本地 compact、已有受支持 remote
compact 分别检查真正发出的请求。每张合成图身份和工具 call/result 可
追溯；普通采样中的配对正确，压缩摘要与 append-only rollout 各按自身
合同检查。每个 TurnComplete 都检查 error。保留总媒体硬限、字节限额和
输出预留，配置更大的文本额度不能扩大媒体硬限；正常文本额度也不能被
图片默认值错误压回 10K。未知引用／不支持视觉的模型给明确限制，不能
伪称“所有厂商都支持图片”。

### 5. cap／usage／取消的公共链路合同

先引用本轮更正：累计 spending 包含失败采样的真实 usage；`last` 是已
安装模型输出的 context baseline，不能为了通知中显示 cap=14 就全局
改写 `SpendingOnly`，否则恢复／压缩估算会被污染。

验收：三 wire 的 cap 缺失／部分／显式零 usage、迟到终局、工具参数半截、
取消各有公共请求／事件断言；失败恰一次，截断工具不执行，无假 Completed，
无未允许的重采样，取消后无额外 attempt。至少覆盖“正常成功 → cap →
继续／冷恢复”，累计值保留 spend、context baseline 不被 cap 覆盖。
Responses WS 专用 fixture 若仍缺，补真实传输层测试；不把 SSE 共用
parser 说明当成 WS 执行收据。复用 v1 golden 和 v2 replay，验证 usage
presence；不要重新录付费 cassette，也不把 replay PASS 数当实际录制数。

### 6. app-server 公共读取／重连／分页与 ACP dev 兼容

使用公开 JSON-RPC 验证 `thread/read`、附着／恢复、客户端断开重连、分页
历史、fork。场景含普通成功、失败 cap partial、图像工具输出和旧无该字段
历史；分页边界切到 call/result／partial 附近，避免只测第一页。

验收：同一历史在完整读、分页合并、热 attach、冷 reconnect 的身份、
顺序、失败状态和累计 usage 一致，无重复执行工具，无当前字段假造旧
历史证据。补 exec JSON 与 ACP dev 对对应失败的实际消费断言；检查 TS／
SDK 生成与 wire nullable 兼容，只在 API 确实改变时更新 schema。旧 reader
实测选定一个明确已有版本，记录能读／降级／拒绝边界，不凭 serde 推测。
不新增模型可见恢复片段，不把 D6 精确计量／完整历史 40 项的研究性目标
包装为此次主门已经完成。

### 7. 50 轮小压测和资源／取消收尾

在 Linux 持续运行一个 app-server，由归档驱动执行固定种子的 50 轮
离线场景；三 wire 各至少 15 轮，总表分配不少于 10 次取消、10 次工具
图像、5 次 compact、5 次客户端冷恢复（允许一轮覆盖多项）。其中取消
覆盖建连／等头、流中和工具运行三个阶段。

验收：**50/50** 目标场景执行，错误按预期断言。记录逐轮 HTTP attempts、
耗时、RSS、FD／socket、活跃进程和工具子进程；预热与正式采样分开，
解释历史增长和缓存，不拿总 RSS 恒定当唯一合同。请求始终在预算内；
结束并 shutdown 后没有遗留 mock 连接、工具进程、待运行任务或继续增
长的队列，取消后对应 turn 不再发请求。资源若随重复创建/关闭单调增长，
最小复现并修复；不得用每轮重启整套服务抹掉泄漏。

### 8. 后台记忆巩固的正常退出修复（P2）

把正常 `AgentStatus::Shutdown` 与真实 Errored/NotFound 分开，避免退出
一律记 `failed_agent` 并延迟一小时。先写失败回归，再实施带 ownership
校验的取消／释放；可用现有接口则复用，确需 store API 时同步实现和测试。

验收：主进程正常关闭、巩固请求仍挂起时，必须先确认子 agent 停止，再
释放自己的 claim；input/last-success watermark 和失败预算不被取消
消耗，新 worker 可立即或明确的短延时重领。真实失败仍保留失败指标和
正常 backoff；旧 ownership token 不能释放新 worker 的任务。SIGKILL
仍按 lease 恢复，不能虚构清理保证；不引入常驻服务来解决短 exec 生命周期。

### 9. Windows 原生剩余收据和磁盘隔离

本轮Codex已补齐旧远程后三轨收据（utils111、env59、replay189报告数，
详见本轮复审），不再等待旧后台任务；旧SHA只能补旧账，当前改动需当前
SHA原生复验。分别记录 C: 临时目录和实际 workspace/target 所在盘的空
间。把本次测试临时目录放到确有空间的测试专属目录，不删除别人的数据。

验收：原生 MSVC 下构建、bridge、工具取消、图片／cap、env 启动及恢复
主子集实际执行，不能以 WSL 代替。先复验19:58:39Z、早于磁盘满的
`retained_tool_output_survives_compaction_and_resume`原生ABORT，不能一并
归为资源环境；再分诊磁盘污染前已出现的doctor、
sandbox/junction/elevated、spawn 变换真实失败；有能力明确修复的分机制
提交，有管理员／平台要求的准确说明。不要把全部 code14 自动判 SQLite
产品 bug，也不要用后半场 disk full 解释之前的每一处红。Windows 扩展
支持门和 Linux 首要稳定门分列，不许把 Windows 未验写成全平台稳定。

### 10. 可运行二进制、最小使用清单和最终判决

在前述源码稳定后构建 Linux／当前 Mac 验证所需二进制，生成 git SHA、
dirty 状态、源码指纹、rustc、features、依赖锁摘要和文件 hash 收据。
ACP `CODEX_PATH` 必须指向这份产物，验证实际启动的子进程路径／hash；
旧 target 二进制不能因仓库已经 push 就冒称最新版。准备本地可审查
制品和回滚步骤，不替换全局安装、不发布。

交付一页最小使用清单：NUWAX 环境启动、三协议 base URL 形状、模型 ID、
隔离 home、启动／停止、ACP dev 指向方式、工具和截图限制、恢复路径及
日志位置；只用占位密钥和合成数据。能力表将 mock 合同、旧真实服务
收据、当前未授权live分开；现有Step配置只提供Chat/Anthropic端点，
未配置或未支持的Responses不写成跳过通过，也不外推为厂商永久能力结论。

最终交付必须包含：各机制 commit、10 项 executed/fixed/blocked 判决、
Linux 首要稳定门结果、Windows 扩展支持边界、精确选择器与请求数、首败
及更正、仍影响日常使用的 blocker。历史 tasks 仅按证据加状态，不将未
满足复合 DoD 整体勾完。完成代码、自查和离线验证后提交给 Codex 复审；
无阻断时明确写“可进入真实服务小批验收”，不要无边界再列一轮升级计划。
