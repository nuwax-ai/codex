# 给 Claude Code：全部已知问题修复和最终稳定验收

请执行本提示词中的全部12项工作，把当前已知的开发和测试缺口一次集中收口。
目标是通过环境变量配置国内或自定义第三方模型，可靠运行 Codex app-server，
并由 ACP dev 完成日常文本、工具、截图、取消、恢复和压缩。需要实际修复、
补回归和完成验收，不能只做分诊、写账本后将可推进工作全部留到下一轮。

## 起点、范围与执行规则

- Codex主仓：`/Volumes/soddy/git_workspace/fork-nuwax-codex`，其test仍是
  `2f2ddc157`。本轮开发必须从下面的已有候选继续：
  `/Volumes/soddy/git_workspace/fork-nuwax-codex/.claude/worktrees/merge-main-20261014`，
  分支`codex/rig-044-20261014`，当前`535882fef`。保留已有9个提交，
  不回退到旧test，不reset/rebase丢弃工作。
- ACP仓：`/Volumes/soddy/git_workspace/fork-codex-acp-ts`，使用dev，当前
  `be81fe1`。保留已有5个本地提交；两个仓库的main均不动。
- 先核对实际status/HEAD，再读本目录的
  `codex-closure-review-2026-10-16.md`、
  `claude-stability-closure-execution-2026-10-16.md`和旧十项计划。
  本提示词和Codex复审修正优先于旧记录中“全部完成、无阻断”的判断。
- 原有未跟踪vendor/是既有缓存，禁止无差别git add或删除；保留并行工作。
  新提示词、复审报告可随本轮最终文档提交。
- 用户先前在对话提供的AGENTS.md已撤销，不重新套用它。遵循当前仓库有效
  约束与实际测试recipe，不把过期要求当作跳过任务或反复索要确认的理由。
- 冻结main同步、Rig/V8升级、新File上传平台和无关架构重构。不改全局
  代理、TLS验证、安全默认值，不操作已取消的artifacts迁移，不删除共享
  target、用户配置或账户数据，不停止验机已有服务。
- 按模块并行分析和修改，避免多个代理改同一文件；共用target的大型Rust
  构建串行安排。尽量使用一致features/profile，先构建必需fixture，再测。
- 每项保留首败，先最小修复再跑原选择器及相邻回归；不能靠skip、吞错误、
  放宽deadline/栈、取消产品硬限、重试直到绿或每轮重启服务来关闭。
- 本轮允许修改两仓、分机制commit；不push、不手动dispatch、不发布/tag/npm。
  完成后交Codex集中复审、合并和推送。最终SHA的自动全量CI待该阶段落地，
  不能以旧run冒充本轮代码已全量通过；等待CI时继续其他独立工作。

## 第一批：5个已确认产品问题

### 1. 正常进程退出必须有界等待记忆claim清理

位置：`memories/write/src/phase2.rs:410,510`。清理仍在detached tokio任务里，
正常exec退出会在SQL release完成前销毁runtime。Codex已用真实子进程复现：
memories_1.sqlite持有1秒普通写锁，前台成功且rc=0，job却仍running、
保留约一小时lease；无锁对照为pending。

追踪清理任务并纳入正常shutdown的有界等待。保留现有正确顺序：先确认
consolidation agent停止，再释放自己的claim；关闭失败或失去ownership不能
错误释放。不能用SIGKILL或强制缩lease替代正常清理。

验收必须用真实子进程：退出后由独立进程查询DB／尝试reclaim。覆盖无锁、
1秒写竞争、正常关闭超时、stale token；保留watermark和失败预算。不能在
被测进程shutdown后额外维持runtime轮询20秒来冒充进程退出合同。

### 2. 真实错误不能被后续Shutdown覆盖成clean cancel

位置：同文件`phase2.rs:423,510`。loop_agent轮询最后AgentStatus；
ShutdownComplete会覆盖Errored。实测memory HTTP400/invalid_prompt，
前台300ms后完成并shutdown，最终却pending、last_error=NULL、5秒retry，
失败预算没有消耗。

保留真实终局结果或失败证据，不能仅用覆盖后的最后状态分类，也不能把
所有Shutdown/Errored/NotFound都当取消。补实际agent错误紧接正常退出的
回归：真实失败仍保留失败指标、预算扣减和既有backoff；纯正常取消不扣。
另验证正常成功紧接退出不丢成功watermark、不重复消费已完成工作。

### 3. 修复已有handler时重复load丢恢复usage

位置：ACP `CodexAcpServer.ts:2004`。`load→prompt→load`后根handler常驻，
恢复通知更新旧SessionState，新安装状态却为null，新增无handler缓存取不到。
Codex独立回归已失败：total应5000，实际null，last/window也丢失。

修正通知所有权／恢复快照捕获与新状态安装的顺序。覆盖首次load、已有
handler的reload、迟到通知、并发generation失效、随后prompt消费；通知
只应用到正确session/generation，不重复、不回退live usage。

### 4. 恢复usage缓存必须有生命周期和全局上界

位置：ACP `CodexAppServerClient.ts:856-862`。目前任何无handler线程都入
Map，包括隐藏标题ephemeral线程。实测关闭100个主session后仍有100项。

限定需要恢复的线程范围，设置合理全局上界，处理关闭、unsubscribe、
ephemeral完成和client dispose。与第3项一起设计，不能通过“所有通知
永远缓存最新一条”修reload而继续放大泄漏。补100次创建/关闭及活跃会话
并存回归，确保不驱逐仍需要的恢复快照、不丢正常订阅事件。

### 5. 处理恢复usage发送的异步拒绝

位置：ACP `CodexAcpServer.ts:2047`的void session.update没有await/catch。
Codex用拒绝的transport调用真实方法，调用同步返回后Node因未处理拒绝退出。

明确采用await的load错误路径，或有日志且受控的best-effort策略；保证state、
响应与通知的合同一致。补transport关闭/notify reject回归，不能留下未处理
Promise rejection或破坏其他会话的退出清理。

## 第二批：4类验收器与证据缺口

### 6. 每轮工具图片必须验证真实typed出站内容

ACP mock当前在tail包含take_screenshot时就答red，stress只检查tool完成和
red；图片被省略也能PASS。每轮关联新的call ID和合成图片，检查当前工具
结果实际进入对应出站typed image字段，不以旧图、提示文字、固定颜色答案
或任意PNG前缀充数。加“image omitted/空图/旧图”的负向mock回归。

保留图像detail、实际字节或受支持重编码后的像素合同、数量／字节／视觉
共享硬限和输出预留；纯文本的显式预算不得再被图片默认值夹断。
51轮改为每wire至少4轮完成工具图片，共至少12轮；cancel-during-tool不计
图片交付。更正旧“工具图12”收据（旧工件实际只有9次完成图像轮）。

### 7. 取消必须严格命中阶段，且无额外主请求

Codex `rig_interrupt.rs`的request_ready在headers/delta前发出；MidDelta
必须先确认客户端消费到本回合AgentMessageContentDelta，再Interrupt。
复用新增WS测试已有的正确模式；保持before-headers的独立握手。

ACP压测的ready超时必须失败，不能resolve继续。分别确认目标请求已到达但
未回headers、目标流已产生delta、工具子进程/真实工具执行已启动。关联
turn与request，取消后不容忍额外一次主请求；辅助title等请求独立分类和
明确计数，不以放宽总数掩盖重采样。三wire、三个取消阶段都实际覆盖。

### 8. 零残留探针必须测真实对象，并对失效显式失败

stats失败=-1、pgrep错误被压成0或-1时不能PASS。probe unavailable应为
FAIL/UNVERIFIED，不能写成零。记录本轮ACP wrapper、真实app-server、
Code Mode host、工具子进程的PID及所属关系，必要时记录启动时间防PID复用。
不得只等待wrapper退出或用全机pgrep替代本轮进程归零。

每个wire在同一长驻服务中执行固定表，三wire合计51轮；只有计划内冷恢复
轮才重启，不要求改造成同进程动态切换三协议。预热后记录RSS、
FD/socket、HTTP attempts和队列，解释正常历史/缓存增长；关闭后核本轮
目标进程和连接归零。模拟探针失败必须使验收失败；不杀其他服务或进程。

### 9. 冷重连／分页必须比较payload，补齐公开消费侧缺口

`thread_read.rs:2822-2830`的fingerprint只比较身份，可能丢工具output／图像
却仍绿。比较持久化ThreadItem完整payload及完整失败信息；只归一化确定
volatile的字段，并逐个说明理由，不能把payload整体删掉再比“相等”。

覆盖热读、完整读、分页合并、attach、关闭进程冷重连、fork，包含成功
工具、cap partial、图像输出和旧无新字段历史。核call/result边界、内容、
顺序、状态及恢复usage。补exec JSON/ACP的消费断言，选定明确旧reader
版本实测能读/降级/拒绝边界。句柄drop并正常teardown与OS进程硬终止
分别命名；不能拿前者宣称SIGKILL恢复已验。

## 第三批：最终验证和待分诊全部推进

### 10. 最终源码的离线三协议、51轮和原生主门

完成修复后构建真实CLI、exec、app-server、codex-code-mode-host及
test_stdio_server；记录源码SHA、dirty、源码指纹、rustc、features、
依赖锁摘要、路径和文件hash。dirty构建须核具体源码指纹，不能仅凭HEAD
判定包含/不含修复，也不能拿旧产物冒充新代码。

统一使用仓库工具链1.95.0和recipe的8MiB栈/local profile；工作目录正确。
V8仅使用当前pin的archive/binding pair并按仓库可信manifest校验，复用CI
setup；不升级pin或退回不受支持的默认下载。缺host会导致整簇unsupported，
先校准fixture，不能算产品失败或提前跳过。

最终树至少完成：相关Rust/TS/Python回归、strict typecheck和scoped lint/fmt；
三wire六场景18/18实际执行；修正后的51/51；Linux原生和真实容器的
app-server+ACP主门；Windows原生对应主子集；Mac交叉主流程。
bootstrap本身无推理时不强迫额外模型请求，其他场景按预定attempt表验证。
SKIP/条件早退/探针不可用均单列，mock识图不算真实视觉。

### 11. 有限额的真实模型小批验收（本提示词授权范围）

允许使用此前提供且当前仍有效、安全保存的测试凭据与现有账户额度，
执行本项真实API请求。禁止把真实密钥写进源码、提交、工件、命令输出或
报告；不购买、充值、升级套餐、开通新服务，也不自动绕到另一个收费平台。
若没有有效凭据或所需模型能力，准确标blocked并继续其他项，不猜密钥。

硬预算：本轮所有真实模型请求合计最多48个attempt（含自动重试、辅助标题、
guardian、memory及WS采样）；每请求输出上限2048 tokens，估算输入不超过
32K tokens。使用隔离home/空的合成工作目录，不上传项目长历史或用户数据。
控制并发、记录实际usage；接近预算停止真实请求并报告剩余项，不私自扩额。
mock的18场景和51轮不计入这个真实API预算，但自身仍需有限请求上限。

先查官方能力/端点文档，优先当前主用模型及协议。至少在预算内完成：
主用路径的文本、真实工具往返、截图、取消、压缩后继续、冷恢复；三wire
各有一个可用真实配置的基础验证；一个备用provider的基本配置切换。
核NUWAX环境变量确实决定endpoint/model/auth，不因真实厂商响应慢而吞错误。

真实视觉使用至少3张不同的合成图（随机颜色/数字/位置组合）；答案不在
文件名、文字提示或工具描述中泄露，不能固定答red就通过。先确认模型
确实支持图像；不支持/缺端点明确标不适用，不能计绿。Step现有配置仅
Chat/Anthropic，未提供Responses端点不外推为永久能力，也不虚构已通过。
预算不足时优先完成主用路径，提供已执行与剩余清单，不重跑付费cassette。

### 12. 外呼挂起、CI与平台遗留分诊及最小修复

先独立复现目录/认证/cloud-bundle外呼故障。用仅localhost的无headers、
慢响应和可控故障，验证initialize、model/list、首轮请求及正常退出是否
有界。区分后台worker与真正阻塞调用；不能只凭SYN-SENT就指定根因。
发现实际阻塞则加恰当超时/取消/后台化与回归，不屏蔽错误、关闭TLS或绕过
受管配置安全检查。验证第三方provider是否意外依赖官方外网。

核实际CI headSha，分列FAIL、FAIL+LEAK、TIMEOUT、ABORT/SIGABRT、LEAK、
SKIP。LEAK通过警告与失败区分，缺Summary/缺结果不能称完整通过。
复用已修复的日志parser，输入需正确去掉GitHub包装时间戳。
处理现有hooks两簇、guardian取消竞态及所有会阻断主路径的真实失败；
核mac连续取消的真实原因，不凭猜测增加job/test deadline。
Windows的sandbox/PTY/UDS/doctor等扩展门逐项判定；当前能最小修复的
实际缺陷本轮处理，有管理员或平台条件的给出具体探针和限制。
AppArmor/userns探针及专属容器校准不能改成测试skip；不擅自修改主机全局
安全策略，缺必要权限准确报告。全上游22K非目标功能不能成为无限前置门。

## 交付标准

1. 两仓分机制小commit、自查；不要把所有产品、测试和工件塞进一个巨提交。
2. 提交真实进程退出／错误覆盖、reload、缓存上界、transport拒绝等新增
   自动回归；临时复现脚本只作起点，不能留在/tmp作为唯一合同。
3. 更正旧执行记录里的错误计数、假绿和未覆盖项，保留首败历史；真实
   服务/mock/旧版本收据分列，不靠PASS总数或基线同败关闭问题。
4. 一页最小启动清单：NUWAX配置占位例子、正确二进制路径、ACP dev指向、
   启停、工具/图片限制、恢复方法、日志/收据和回滚方式；不发布全局安装。
5. 最终给出12项各自fixed/executed/blocked及证据、所有commit和产物身份、
   Linux/容器主门结论、Windows边界、真实API总attempt/usage及剩余预算。
   只有可独立推进部分真正完成，才交Codex复审；外部阻断不能停掉其他项。
6. 明确哪些条件已满足“可日常使用”，哪些仍失败；未完成的硬门不能写成
   “全部收口、无阻断”。不要再另开一批无关升级或架构计划。

## 可复用的本地复现（临时文件失效时按上述场景重建）

- 记忆正常退出：`MEMORY_REVIEW_LOCK_SECONDS=1 python3 /private/tmp/codex_review_memory_exit.py`
- 记忆真实错误后退出：`MEMORY_REVIEW_ERROR=1 python3 /private/tmp/codex_review_memory_exit.py`
- ACP reload：在ACP仓执行`node_modules/.bin/vitest run --config /private/tmp/closure-acp-review.config.mjs --retry 0 -t 'REVIEW repro'`
- ACP缓存：`node --import tsx /private/tmp/closure-acp-buffer-repro.mts`
- ACP拒绝：`node --import tsx /private/tmp/closure-acp-update-rejection.mts`

这些复现只连接localhost；真实API仅允许在第11项预算内单独执行。
