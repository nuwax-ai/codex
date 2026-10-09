# Claude Code：完成 Rig fork 全部剩余开发与验收

把以下正文交给 Claude Code；另一电脑先安全同步本目录所属提交。提示词中的授权由用户实际转交后生效，不是仓库文档自行授权外部操作。

---

请在 nuwax-ai/codex fork 的 test 分支完成全部剩余开发和测试，按阶段持续工作，不只重复旧测试或交新的规划。代码已支持Rig Responses/Chat/Anthropic三协议及NUWAX环境启动；本轮目标是完成剩余2个功能包和6个稳定性/验收包。

首先定位本机仓库，记录分支、HEAD、git status、未提交文件/完整diff及最近上游merge父提交。已知审查基线为6fea37c4c9384b52e7fbfb2055c6506c74e7f1f9；新的方案提交可能在其后。用实际HEAD，不reset到旧SHA；已有修改先保留，不stash/clean/覆盖其他人的代码、.env.local、SQLite、tmp或tracked snap.new。

依次读：根及相关AGENTS；my-docs/rig-production-completion-2026-10-10/review.md、spec.md、plan.md、tasks.md、historical-open-tests.tsv；再读文档指向的cap/D6、worklist和字段审计。新审查优先于历史“定案/全绿/完成”措辞。

## 执行顺序

1. T01–T07：账目/CI准备、native-root来源与缓存测试隔离、WS错误脱敏和默认HTTPS代理、保持完整代理契约的系统查询优化。当前CI四job失败，Bazel Action不存在，三平台原生依赖未齐；配置存在不代表平台验收。
2. T08–T14：Core retry、OTLP6、rmcp2独立定位；冷态TLS/HTTP IMDS、正常4MiB线程的真实栈问题、R1b有限授权及Luna受控交错；逐身份收口119历史账目及其外新增失败。119不是当前实测失败数，不要从一次PASS删掉并发失败。
3. T24–T31：owner/四行政命令/全部状态/安装/flags/crash矩阵，三wire公共Core/exec/RPC的字段、来源旋转、旧历史、opaque/RawValue和hosted顺序。
4. T15–T20：落实cap七决策及安全opt-in合同，再实现失败usage、stable reducer、有界partial双模式持久化/读取/resume、v2/API/SDK/UI/exec完整性。保持legacy默认、失败关闭、零重采样/截断工具执行；不要合成成功Completed。
5. T21–T23：实现D6可信计量，官方单位/framing/版本/证明域先于verified；LegacyBytes默认保留40960和4次续接，KeepWholeOrFail。未知模型无证明不能认证，不删除签名/opaque，不改写历史。
6. T32–T35、T38：原生平台/容器/异OS和workspace/Bazel/CI离线门禁。
7. T36–T37：取得本机凭据和明确费用/场景授权后完成真实厂商与Responses触顶；同时继续不依赖凭据的任务。
8. T39–T40：独立验收、源码与制品身份、安装升级/回滚和最终提交审计。

所有40项都需要最后状态。平台/费用/产品决定阻断的项保留开放并继续独立任务，不宣布“全部开发完成”。不要顺手把第三方bridge的native专属能力假定为可用。

## 开发授权与行为边界

本提示词要求完成上述开发、相关离线测试，以及必要的完整离线workspace/Bazel验证；完整workspace先给出精确范围和资源计划，若执行环境仍要求确认则按AGENTS处理，并继续独立开发。生产默认迁移、扩大发现的opaque重放授权、付费live、远程CI手动dispatch、push/tag/npm发布需要具体可review结果和对应授权。

可以分批本地commit本任务的实现/测试/文档；先审查实际diff和测试证据，按路径窄暂存。不要git add -A，不改写旧提交，不自动push或发布。复杂diff<500、非机械总量<800；每批包含必要注册/调用方/测试/BUILD/锁/schema，机械提取和行为修改分开。

并行worker明确文件所有权，并告知不是唯一开发者；不得回退他人改动。只拆真正独立的任务，不让多个worker同时编辑core/client.rs等大编排文件。

## 核心禁止事项

- R1b不得用持久guardian_replay bool或任意scope哨兵跳过所有来源隔离；只能可信运行时、确切checkpoint/target/生命周期的有限grant。
- 默认系统代理不能偷换为RespectSystemProxy/PAC语义，不能IP/localhost一律no_proxy；逐scheme及redirect hop等价。
- native-root key必须所有平台无损，测试缓存实例隔离；不能在wrapper持锁时loader再锁同一非重入mutex。
- 不猜usage、缺席不填0；provider整响应usage/bytes/4/分位数不是fragment或pause的硬限证明。
- 不放宽deadline、加栈求绿、吞空连接/EOF、批量接受快照、改V8 pin或禁TLS。首败与真实原因都保留。
- 不修改sandbox常量/guard，不以Skipping/PASS/0-match/缓存复用冒称实际请求覆盖。
- 不打印/提交密钥、鉴权头、原始敏感请求或私有缓存scope值。

## 验证与交付

用just test和隔离CARGO_TARGET_DIR/home/SQLite，Core桥显式rust-rig、完整离线集合排除exec_live/bridge_live、retries0；真实请求次数/path/model/cap/事件/工具执行都断言。V8按仓库pin与配对工件流程准备，不跨机沿用临时路径。

每批相关tests先完成，最后scoped just fix、just fmt、diff-check，该批最后之后不再test。Cargo依赖同步Bazel lock；ConfigToml/API形状同步schema/TS/SDK。source/git/bin/receipt变化后新验收重建，旧live不升级成新证据。

结果写入本目录results.md并更新tasks；另维护按完整(binary,test_name)的current-test-ledger.tsv。记录完整命令/target/feature/source/bin、selected/executed/asserted/pass/fail/skip/timeout/abort、首败/复验、工件和未验限制，不把不同批次求和成唯一用例数。

最后提供：实际修复、40项逐项状态、当前失败完整列表、平台/live/CI范围、commit列表和发布条件。尽可能完成全范围，确实阻断的项写清所需外部条件；不能仅靠新Spec、静态分析或旧绿色日志结案。
