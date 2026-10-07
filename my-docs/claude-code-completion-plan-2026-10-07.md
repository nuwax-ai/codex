# Claude Code：剩余改造与生产验收总任务（2026-10-07）

本任务覆盖当前项目剩余范围，不等同于一次性大diff。先读AGENTS、最新独立审查、Rig spec/plan/tasks、修订后的cap Step0和D6方案，从实际HEAD及dirty树开始，保留其他人的配置、SQLite、tmp和tracked `.snap.new`，不stash/reset/clean。已有三协议Rig主路径、env启动、owner与可见历史投影已实现；不能因局部mock或基线也失败就宣称生产门禁完成。

按交付边界分 **2个实质功能包 + 6个验收/稳定性包，共8包**。这是工作包数，不是未实现API数，也不能换算成可靠完成百分比。若发布范围明确保留legacy pause byte预算，D6可作为后续增强；在该裁决前仍列全量任务。

## 1. P1：cap partial output / usage完整功能

Step0 D1–D5仍待产品裁决，提案尚非批准。先交具体可review的类型/字段/样例diff和有界策略，再取得方案裁决，之后按Step1–5实施。准备和现状测试可先做，不静默缩小legacy契约。

- 三协议终止帧已知usage在报错前提取，未知≠0；Responses纳入codex-api共用SSE/WebSocket解码，Anthropic聚合message_start/delta，Chat覆盖终止前/后usage顺序。需要独立failure载体，不能合成成功Completed。
- legacy和paginated都保存可见partial，并能读/resume。新增类型须实测旧读取器行为，不假定未知enum自然忽略；不要截断/执行工具参数。
- partial的token、byte、item/总数硬上限、计数来源和超限行为明确决定。新模型可见片段在core/context结构中实现ContextualUserFragment；>1k项P0人工复审，>10k禁止。
- unknown/累计完整性要持久化、可查询和重放。后续成功turn、reconnect、attach、resume不能抹掉未知状态；正确API类型ThreadTokenUsage，与账单ThreadUsage分开；v2字段nullable且同步TS/schema。
- 保留Core Error→TurnComplete(error)、app-server turn/completed(status=failed)、exec turn.failed关闭；partial事件在失败终局前。exec/SDK/rollout/app-server映射与快照全覆盖。

验收：三协议有/无usage、乱序/迟到帧、恰一失败、无重采样、截断工具零执行；双存储模式partial落盘/恢复；未知状态跨后续成功/重连/恢复可见；旧客户端/旧历史兼容。Step6真实Responses触顶另见包7。

## 2. P0裁决：D6 token-aware pause预算

当前40,960-byte兼容边界保持。先核官方计量单位、provider framing、模型/tokenizer版本与全载荷覆盖，交Exact/ProvenUpperBound的可复核证明；API名字、bytes/4或样本匹配不能认证verified。缺证明LegacyBytes/unverified，KeepWholeOrFail，不删opaque块、不截断签名、不改写历史。

启用token硬限会拒绝一些旧可发送内容，必须先做兼容裁决。实现计数适配器及ASCII/CJK/高熵/JSON/引用/签名/framing边界；超限零下一POST，未知模型不能verified。没有证明与人工P0复审不关闭本包。

## 3. P1：workspace失败与时序稳定性

当前不能称383根因全部收口：249历史项低并发仍败，其中201未归因；完整首次日志还漏掉过1个SIGABRT。使用nextest_log_summary.py按(binary,test_name)并包含全部失败终态，分开执行数、身份数、复验状态、根因与回归归属。

- 201未归因逐项建立最小复现、实际错误签名、根因和修复验收；20已证环境问题用可控fixture/依赖注入修复测试环境，不放开产品网络策略或削弱断言；28已有flaky标签不代表当前问题关闭。
- 对guard ephemeral stack overflow做正常栈预算、相同feature图、相同精确集合的交错/同窗实验，定位递归/大future/clone路径；一次单跑PASS只能登记不可稳定复现，不能删除崩溃。
- 优先guardian_v2、TUI snapshots/reconnect、network-proxy、exec-server、MCP grace和重子进程路径。超时定位spawn、bind、ready、initialize、模型请求和退出各阶段。D4采样当前都新spawn，不把后续样本直接称loaded warm；低高load相关性不是因果。
- 同集合基线A/B是诊断辅助，不代替最终门禁全绿；保留首轮失败、同名跨binary条目、信号中止与完整工具命令退出。

相关小批绿后再申请完整workspace，以live排除+retries0运行；每个skip/timeout/abort有明确记录。不放宽deadline或批量接受快照。

## 4. P1：行政/owner完整公共矩阵

现有archive参数单测只验证TuiCli合并，不证明实际daemon选择。补--no-daemon/--strict-config/--profile/--oss的真实CLI→owner/embedded/remote路由，与default/npm/其他install source、daemon存在、active/inactive/incomplete/corrupted group及四命令矩阵组合。

加载owner通过thread/resume真实持有writer锁到卸载；QueueList不是加载。probe要遵守coordination协议、记录真实运行窗口、错误传播并做held/free正校准。保留轮询和PID归属限制；禁止用“没看到”变成任意时刻绝无。补cold/loaded/subscribed/running、owner crash/restart/queue persistence、名字歧义/分页、客户端服务端归属，不启动第二writer。

验收：真实RPC/路由与attempt，锁生命周期、配置字节、SQLite/home独立；损坏组在连接前拒绝、秘密不回显。

## 5. P1：来源/历史矩阵的剩余维度

本轮Anthropic同进程query/header/endpoint旋转应使用固定query-name集合、实际URL/auth/header/model/attempt、signed thinking+search ciphertext+encrypted citation完整正负对照。它不能代替所有协议、signed来源和厂商SDK边界验收。

补Responses/Chat相应public Core路径、credential-only同源正例/负例、不同wire/model/provider切换、legacy rollout旧来源缺证降级、hosted tool continuation/pause顺序、encrypted reasoning RawValue字节/数字精度。跨进程credential-instance变化只证明保守降级，不能独立证明其他旋转维度；可见内容保留，原历史前缀不改写，未知来源不猜戳。

验收：完整块/位置深比、opaque三类保留/消失、实际请求字段和原rollout bytes；不把配置快照变化冒充transport字段已变化。

## 6. P1/P2：Linux/Windows/异OS验收

Windows原生Ctrl-C覆盖Retry-After、首SSE字节等待、流中等待；直到子进程退出无后续attempt。Windows锁语义、home/SQLite和技能根实际验证，不用Unix kill或HOME改写代替。Linux验证同容器双进程预算/凭据/协议隔离；app-server与exec-server异OS配置归属、资源runfiles路径和remote owner路径单独跑。平台不可执行项明确记录，不算pass。

## 7. 需当次授权：真实厂商矩阵

先离线全绿，再凭本机凭据与明确授权执行限定费用/场景矩阵，绑定新source/binary receipt。MiMo/GLM/StepFun支持的三wire与模型实际URL/cap/tool/reasoning/usage/errors验收；Responses live触顶仍缺。厂商不提供端点列协议限制，不伪造支持；旧live工件不移作新提交证据，凭据与原始auth不入报告。

## 8. P1：CI、门禁与发布准备

改完依赖/ConfigToml/API时同步Cargo/Bazel/schema/SDK。Bazel实际public三wire、skills正确`.agents/skills`投毒、new Python socket/parser负控进入门禁；0匹配、缓存复用结果、仅unused codec单测不能替代真实路径。CI必须排除live/禁自动重试。没有授权不dispatch、push或发布；先交可review的CI diff，再在授权平台上执行完整结果。

建立明确release checklist：所选发布范围功能闭环、workspace与跨平台绿/已批准平台限制、真实请求收据、来源/二进制一致、配置与历史兼容、无秘密。只有这些通过才能称稳定生产版本，不用“与基线同败”作为发布依据。

## Claude执行与停止边界

可立即推进包3/4/5的离线复现、修复和测试，及包1/2的具体设计/原型验证；平台具备时包6本地执行。需要产品裁决的语义、真实厂商、完整workspace、CI/推送/发布均在准备好具体review结果后申请对应授权。并行任务按文件所有权拆分，普通diff<800、复杂<500；不能一次把8包塞进大提交。

每批完整命令/feature/target/执行数/skip/timeout/abort、首轮失败与复验、源码/bin身份和脱敏事实；所有相关tests完成后scoped just fix、just fmt、diff-check，之后不再tests。当前授权是完成独立复审与后续安排，未授权发布。不得把未批准提案当完成功能或为求绿增加隐式豁免。
