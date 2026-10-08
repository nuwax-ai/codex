# 另一台电脑继续开发提示词（2026-10-08）

以下正文可直接交给 Claude Code。仓库发布分支为 test；本轮 Codex 已审查修复并分阶段保存，代码检查点 `7e46fa22ff2e30272475a2c0d3d8b49bbcc27516`；最终文档提交也应随 test 分支同步。代码阶段、当前测试数与推送目标见 `codex-independent-review-2026-10-08.md`；不要把旧电脑结果当新机通过。

---

请继续开发 nuwax-codex fork，仓库 git@github.com:nuwax-ai/codex.git，分支 test。本机路径先定位并填写，记录当前分支、HEAD、git status和新文件。已有仓库先fetch origin test，核对提交关系；只在可安全快进时同步，不reset/clean/stash，不覆盖配置、SQLite、tmp、.env.local或tracked .snap.new。无仓库可正常clone；不要强制退回旧SHA。

先读根/相关AGENTS、`my-docs/codex-independent-review-2026-10-08.md`、`validation-2026-10-07/pkg3-failure-signatures-worklist.md`、`other-computer-validation-results.md`、`claude-code-completion-plan-2026-10-07.md`、Rig spec/plan/tasks、cap Step0/实施提案与D6文档。以当前源码、Cargo.lock及本机真实执行为准。

第三方 wire_api=responses/chat/anthropic 默认分别经Rig请求 /responses、/chat/completions、/messages；官方OpenAI/Bedrock保留原生边界，native与显式bridge按源码。旧“responses转chat”描述已过时。当前原生mock fixture显式native是恢复上游WS/GuardianV2测试前提，不代表桥路径支持这些原生能力。

当前主线能力基本实现，仍有2个实质功能包和6个验收/稳定性包，按以下顺序推进。每批复杂diff<500、普通<800，必要注册/消费方/fixture/锁/schema同批；并行任务按文件所有权划分。

1. **先收敛包3稳定性与剩余134项。** 原249项仍败集合已在历史限定窗口恢复115，仍134未收口；本轮R10/registration另有失败，134不是当前所有失败的完整总数；124是恢复标签数，其中9项此前已通过，不能写249恢复124。按(binary,test_name)、全部终态包括SIGABRT解析记录。优先R1b13（checkpoint来源/二次采样）、R7 HTTP间歇慢、R9 sqlx establish/waker pending、TUI剩余超时/快照、SIGABRT实际递归/大future定位。R7/R9先分开最小复现与栈证据，确认共同调用链再合并，不能只因都慢就同根。

2. **复核本轮生产修复和平台边界。** R4 provider要求按load-time身份/内置bridge-only归一化，保留loaded线程endpoint，组织transport改变才拒绝；跑公共turn/start/queue与CLI优先级。R6 watcher在专职线程处理backend，desired与active分开、失败重试、单wake合并更新，安装成功以独立readiness信号向缓存订阅发粗粒度失效，changes-only订阅不伪造文件变化；fs/watch异步等实际安装再成功响应，watch失败报错，in-flight unwatch与actual祖先→文件迁移都重新核对ready。验证真实file change、退订/drop、watch失败、阻塞unwatch时Tokio仍响应、模式升降、跨平台；backend若永久卡住仍是退化/资源风险，不能说FSEvents根因已从底层消失。DNS fixture两族受控、localhost原生、prod fixture=None不放松私网策略；Linux/Windows编译和resolver回归要实际跑。

3. **R10冷/暖分开。** native_tls预热只限macOS并一次性、独立localhost peer，不复用应用pool；本轮正常Bazel TLS7/7+AWS1/1，Cargo同暖态切片4PASS/3FAIL/1TIMEOUT；不能只拿green环境关闭差异，更不能算冷首请求达标。AWS real_imds是HTTP，原TLS抓栈不是AWS冷因果证据。用独立进程、相同预算、确切SDK路径捕获未预热首请求（TLS与HTTP/IMDS分别）；不扩大deadline、不吞空连接或削弱断言。

4. **包4行政/owner完整公共矩阵。** profile-v2叠加base，负控先清理base别被其他错误提前挡住；OSS用无效默认+有效显式覆盖证明优先级。补组状态active/inactive/incomplete/corrupted、--no-daemon/strict/profile/oss/local-provider、安装方式/daemon/remote、cold/loaded/subscribed/running、owner crash/restart。writer loaded从resume持锁到unload，probe遵守coordination、ready/window/error propagation；轮询/PID局限保持公开，不以没观测到替代任何时刻唯一。

5. **包5来源/历史。** 完整实际URL/query/header/auth/model/attempt，不把配置快照变化当wire变化。补Responses/Chat、credential-only、跨wire/model/provider与legacy缺来源投影，RawValue字节和数字精度，encrypted reasoning/signed thinking/search ciphertext/citation及pause合法顺序。same-source阳性必须保留，负例visible保留opaque降级，原rollout前缀不改写；未知来源不猜戳。跨进程随机credential-instance不能代替独立旋转。

6. **包1 cap partial/usage具体裁决后实现。** 7个决策点仍未批准，先交可review类型/样例和兼容方案。legacy/paginated都持久化/读取/resume；unknown/completeness跨后续成功/重连/恢复；保留Error→TurnComplete(error)、app-server failed关闭、exec turn.failed；Responses共用SSE/WS解码与Anthropic原帧presence/显式0正确聚合，多response turn稳定键去重。fragment预算按完整render和真实计数/证明上界；provider整response usage、bytes/4不能给片段硬限，缺证明不认证。contextual_user_message matcher/type_markers与授权分类必须覆盖，不能把模型partial当人类授权。截断工具绝不执行、不合成成功Completed、不重采样。

7. **包2 D6仍P0设计。** 40,960-byte兼容边界保持；官方framing/计量/tokenizer版本和全载荷exact/proven upper bound证明先于verified。没有证明LegacyBytes/unverified，KeepWholeOrFail，不截断签名、不删除opaque或重写旧历史。单项2k token的建议仍需>1k P0人工复核；产品兼容裁决后才启用新硬限。若本次发布保留legacy行为，明确将此增强后置，不假称已实现。

8. **包6/7/8验收与发布。** 新机先本地mock，再Linux/Windows（原生Ctrl-C、锁、skills home、异OS app/exec/remote）与完整workspace/CI。真实厂商和Responses live触顶需当次凭据/授权、限场景费用和新source/bin receipt；CI dispatch/push/发布也需对应授权，准备好review结果后请求。协议端点不存在列限制，不伪造支持。完整workspace获授权时显式排除exec_live/bridge_live、retries0；skip/timeout/abort单列。

测试一律just test，Core桥显式rust-rig，隔离CARGO_TARGET_DIR/home/SQLite；网络guard早退的Nextest PASS不是请求验收，打印成功输出并核对Skipping，或用正常Bazel测试环境；不修改sandbox相关常量/guard。V8工件按仓库pin+配对hash准备，不改版本求绿。Bazel非零执行、正确.agents技能根canary、真实公共路径与SDK/schema同步不得遗漏。

每批写基线、完整命令/feature/图、实际执行数、pass/fail/skip/timeout/abort、首轮失败与复验、源码/bin身份和脱敏事实，尤其区分旧机/新机、本地mock/真实厂商、macOS/跨平台、切片/完整workspace/CI。相关tests完成后scoped just fix、just fmt、diff-check，之后不再test。新依赖必须just bazel-lock-update；ConfigToml/API改动同步schema/TS/SDK。

先自主推进已授权的离线复现、修复和验收，以及cap/D6的具体方案准备；真正改变未批准的产品语义或运行需授权的验证前，交具体可审结果再确认。最后分阶段提交建议与剩余任务，不无理由扩大范围或宣称“同基线失败就是生产可发布”。
