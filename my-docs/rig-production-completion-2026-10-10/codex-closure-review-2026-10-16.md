# Codex 十项收口轮复审（2026-10-16）

范围：Codex候选535882fef（2f2ddc157之后9提交），ACP dev be81fe1（c9e0ab2之后5提交）。本轮只复审，未改源码、未commit/push。以下均为P2；不接受“十项全部收口”的验收结论。

## 产品问题

### 1. 正常进程退出不等待memory claim清理

位置：/Volumes/soddy/git_workspace/fork-nuwax-codex/.claude/worktrees/merge-main-20261014/codex-rs/memories/write/src/phase2.rs:510（detached spawn在410）。

新release仍在未注册JoinHandle的tokio::spawn中。shutdown_all_threads_bounded只等thread/session终止，exec返回后runtime会销毁，不保证等到release SQL。给memories_1.sqlite持有1秒普通写锁时，前台正常成功且exec rc=0，DB仍running、lease约一小时；锁释放后复查仍running。无锁对照为pending。新单测shutdown之后继续维持runtime轮询20秒，不能覆盖真实退出。

应追踪并有界等待memory清理完成；测试必须用真实子进程退出，再由另一个进程检查DB和reclaim。

复现（仅localhost、dummy凭据）：
MEMORY_REVIEW_LOCK_SECONDS=1 python3 /private/tmp/codex_review_memory_exit.py
根复核目录：/private/tmp/codex-memory-exit-review-n54m6elr。

### 2. Error→Shutdown被误当干净取消

同文件phase2.rs:423及510。

loop_agent每秒读取最后AgentStatus；ShutdownComplete可覆盖之前Errored。模型HTTP400/invalid_prompt后，前台300ms内正常完成并shutdown，最终job变pending，retry_remaining=3、last_error=NULL、约5秒retry，真实错误的失败预算和backoff被取消掉。新SQL对照测试直接调用failed路径，未覆盖状态覆盖控制流。

应保存真实终局结果/失败，不以可被shutdown覆盖的最后状态判定；加错误后立即退出的子进程回归。

复现：MEMORY_REVIEW_ERROR=1 python3 /private/tmp/codex_review_memory_exit.py
根复核目录：/private/tmp/codex-memory-exit-review-qi1z2ts3。

### 3. 已有handler的同进程reload仍丢恢复usage

位置：/Volumes/soddy/git_workspace/fork-codex-acp-ts/src/CodexAcpServer.ts:2004。

load→prompt会安装session-scoped根handler，prompt结束不移除。再次load的恢复usage被旧handler写入旧SessionState，随后新状态以null安装；新增applyRestoredTokenUsage只能取“无handler”缓存，所以取不到。实测新状态total应5000却为null，last/window一起丢。

应迁移通知所有权或显式捕获本次恢复快照，覆盖已有handler、reload和generation场景，不能只测首次load。

复现：node_modules/.bin/vitest run --config /private/tmp/closure-acp-review.config.mjs --retry 0 -t 'REVIEW repro'
测试文件：/private/tmp/closure-acp-review.test.ts。

### 4. usage缓存全局无界，隐藏标题线程留下孤儿项

位置：/Volumes/soddy/git_workspace/fork-codex-acp-ts/src/CodexAppServerClient.ts:856-862。

无handler的所有线程usage都入Map，包括TitleGenerator创建的ephemeral线程。标题runTurn仅装完成捕获，没有通知handler；关闭主session只清主ID。真实Client+TitleGenerator的mock turn复现：关闭100个主session后，pendingRestoredTokenUsage仍有100项。

限定恢复缓存生命周期，给全局容量上界，并在关闭/unsubscribe/ephemeral完成时清理。不能把“每线程最新一条”当全局有界。

复现：node --import tsx /private/tmp/closure-acp-buffer-repro.mts。

### 5. 恢复usage更新的拒绝未被处理

位置：/Volumes/soddy/git_workspace/fork-codex-acp-ts/src/CodexAcpServer.ts:2047。

void session.update(...)不await、不catch；ACPSessionConnection.update实际await transport.notify。用拒绝的transport调用真实applyRestoredTokenUsage，调用同步返回后Node因未处理拒绝exit1。应await并纳入load错误处理，或明确定义best-effort且catch，避免该异步异常逸出。

复现：node --import tsx /private/tmp/closure-acp-update-rejection.mts。

## 验收假绿与范围

### 6. 重复工具图轮不检查实际typed image

位置：ACP scripts/acp-validation/mock-vendor-server.mjs:233-235；stress-50.ts:295-297。

强制工具轮的tail含take_screenshot即答red；压测只检查tool completed和red，没有检查本轮实际出站图像。Node探针传“合法call/result但内容image omitted”，hasImage=false仍答red。应每轮关联新call ID和typed image，增加缺图负向合同。

实际工件每wire toolImageRounds=3，共9轮完成图片，不是执行记录宣称的12；取消工具轮明确toolDone=false，不能算图像交付。计划要求至少10次，需更正表和重跑。

### 7. MidDelta/工具中取消没有严格阶段握手

Codex core/tests/suite/rig_interrupt.rs:220-233先发request_ready再写headers/delta，:291-296收到ready立刻Interrupt，未等待消费AgentMessageContentDelta。两例可以重复before-headers。WS新增测试有消费delta，应复用该模式。

ACP stress-50.ts:305-320和338-349超时resolve而不fail；before-headers只sleep，during-tool没最终确认已启动，:331还容忍取消后额外一次请求。应阶段ready缺失即失败，按turn关联HTTP attempts，确认取消后零额外主请求并排除独立辅助请求。

### 8. 资源探针失败可使零残留合同通过

ACP stress-50.ts:510-536只等ACP wrapper退出；没有追踪真实app-server PID。stats失败变成-1；pgrep的错误可能被`|| true`压成0，执行异常则返回-1。最后仅>0才失败，探针不可用会PASS。应FAIL或UNVERIFIED，并检查本轮真实app-server/工具PID，不以全机pgrep替代所有权关联。

这不证明本次实际已经泄漏：工件报告0；问题是合同无法可靠检出。

### 9. 冷重连只比身份，未证明条目payload一致

Codex app-server/tests/suite/v2/thread_read.rs:2822-2830比较fingerprints/flattened identity；:3123-3151只留status/error.message/itemkind/id，忽略工具output、图像及完整错误内容。结果校验只做hot_full，cold/full/paged可丢payload而保ID仍绿。应比较持久化ThreadItem完整payload，仅归一化明确volatile字段，并补计划中的cap partial、图像及选定旧reader/exec JSON消费证据。

## 验证边界

- 原ACP相关14/14、mock17/17、typecheck通过；新增独立reload回归失败，不能用旧绿盖掉。
- Memory两个问题均用真实codex-exec子进程复现；现有二进制receipt是2f2ddc157+dirty，包含新memory/state修复。不能仅凭HEAD是2f2就判定不含修复；最终平台二进制仍应以源码指纹核对。
- 本轮没有跑全workspace或真实付费API，没有修改两仓源码、分支或提交。
- SQL ownership/status fencing、watermark保留本身正确；Mapper failure映射、WS等待delta后的取消等新覆盖有价值。
- “kill恢复”通过drop句柄并等待正常teardown，证明无Op::Shutdown的句柄释放恢复，不等同OS SIGKILL。
- 外呼挂起是单列部署风险；仅观察SYN-SENT不能直接证明background model refresh阻塞initialize。应做有界故障回归后判适用环境，不把此未定因果当本轮新增源码问题。
