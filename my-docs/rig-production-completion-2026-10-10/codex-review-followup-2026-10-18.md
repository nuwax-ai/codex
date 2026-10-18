# 最终验收后的 Codex 复审（2026-10-18）

复审输入为 Codex `60956fb10..b514cd6cd` 和 ACP `854f2c3..0d4c12d`，并直接核对 CI run `39098893052` 的原始失败日志。目标仍为 Linux/容器 + ACP + rig 自定义模型接入，不扩展新功能。

## 已确认的问题与修复

1. **终态停机漏掉晚到线程。** reusable stop 捕获快照后仍允许内部线程注册；随后只 close tracker 并等待，不能停止已经注册的晚到线程。终态 drain 现在先 close，再做最终线程关闭扫描，最后等待后台清理，所有步骤共用原有总超时；普通 bounded stop 仍允许恢复。
2. **旧停机任务误删同 ID 新实例。** A已停、B仍在关闭时，A可以恢复出新运行时；旧快照最后按ID移除会误删新A。现在只有registry仍指向快照中的同一个Arc实例才移除。新增三个真实生命周期回归，覆盖上述两个时序及正常stop/resume；原准入栅栏回归保留。
3. **图片失败是旧夹具前提失效。** fork 对工具original图保留内联尺寸，1×1图片约一个patch，不能沿用上传后opaque File预留10k的预期。改用2560×2048的可解码原图，视觉成本5120 tokens，低于工具入口限额、高于4500 reviewer窗口；仍要求恰好一次压缩、超大输出替换标记和正确续轮，并明确验证未上传。另对user Original File独立验证10000 token计费、超窗拒绝及预算耗尽标记。没有回退图片入口限制或修改生产上传策略。
4. **CI夹具隔离和前提问题。** 两个hooks测试使用同一全局marker，改成各自TempDir并正确引用命令路径；host_roots无marker测试用fixture范围的metadata文件系统，避免宿主或其他测试的祖先.git改变前提；root_metadata为合成namespace补上与实际`--ro-bind /usr /usr`一致的显式只读边界，保留生产可信PATH检查和原拒绝断言。
5. **Windows夹具问题。** MCP图片回放把Windows路径原文拼进TOML，改用TOML字符串序列化。记忆清理blocked/transient writer测试缺少“锁已获取”的握手，允许子进程先退出、清理已成功却断言仍running；现在只有BEGIN IMMEDIATE成功后才放行前台响应。没有放宽lease、失败预算、退出超时断言。
6. **ACP连续工作流的验收缺口。** 按当前prompt重置流式信号；截图比对唯一真值、同tool的typed image及成功终态；文件读用未透露给模型的nonce，旧marker先清理；压缩先捕获usage游标并检查原生完成事件；冷恢复要求恰好一次、used/window与停止前一致；mac/Linux进程探针失败必须失败，按PID及启动时间核对两个adapter的残留。短对话不保证压缩后厂商reported usage严格下降，工作流记录数值变化并要求真实压缩完成与新usage；超窗压缩的严格合同由Core定向测试保留。
7. **真实调用预算的宿主覆盖隔离。** 共享live provider环境清除显式MODEL_PROVIDER、CODEX_CONFIG及已知CODEX provider/model覆盖，保留选定二进制路径和网关认证，防止宿主配置把真实调用绕开共享预算。补离线负例。
8. **Chat多段纯文本的实网兼容缺口。** 真实GLM Chat在压缩后反复回复旧`live-ok`。rollout中的最新user及摘要顺序正确，桥每次重建请求；布尔wire探针确认最新`post-compact-ok`确实进入供应商的user消息，原始供应商回复仍为旧串。rig 0.44会将连续user合成text数组；临时网关仅把严格纯text数组按序用双换行连接成字符串后，4次真实请求的smoke对照通过。这是格式兼容证据，不记作正式产品验收。产品桥接层现采用该Chat纯文本形式；图片/混合块、扩展键/未知块/空数组及其他协议保持原形，预算检查在改写后执行，并更新Content-Length。新增6个真实HTTP回归，其中一个走Codex请求转换→实际Rig SDK→localhost，旧实现因数组而失败，修复后通过。没有厂商硬编码、额外配置旋钮或rig版本变更。

验收器自查也保留：新增检查曾漏读MCP的`rawOutput.result.content`，并误把已启用的原生`compaction_update`当成legacy tool_call；分别通过真实mapper与生产compaction tracker/fixture补回归后修正。没有因此改动ACP产品输出或将失败当成模型不支持。

Claude本轮的Unix libc测试依赖、Astra上游快照对齐及撤销guardian预热跳过均核对通过。Luna审查通道的后台握手独立于普通provider WebSocket开关，分类应在无可用socket时即时HTTP回退。

## 验证记录

两处停机回归在旧实现分别因同ID新实例丢失、晚到线程未停而失败；Chat出站回归在旧实现因实际SDK发送数组而失败。最终树验证结果：

| 验证 | 结果 |
| --- | --- |
| Core定向生命周期/预算 | 19/19 |
| 原生Core/exec/app-server集成 | 18/18 |
| host_roots（主动注入祖先.git） | 红→绿，9/9 |
| root_metadata（真实Linux嵌套沙箱） | 红→绿，1/1 |
| protocol + bridge（含6个新HTTP合同） | 555/555，无跳过 |
| 离线单元及选定Rig回放 | 95/95 |
| ACP Node / Python合同 | 25/25、11/11 |
| 类型检查 | 四域全部通过 |
| Core lib / bridge all-targets Clippy | `-D warnings`通过 |
| Bazel严格锁检查 / 变更文件格式及diff检查 | 通过，锁文件无变动 |

最终Linux二进制从修复树重建，Codex的9085个已跟踪源码文件、ACP的1319个运行/驱动相关已跟踪文件与本地逐文件哈希一致。二进制哈希、源码清单指纹和脱敏结果见同目录[codex-review-followup-receipt-2026-10-18.json](codex-review-followup-receipt-2026-10-18.json)。CLI编译另有既存`network_policy`未使用警告，不将定向Clippy结果表述为整个workspace无警告。

**正式实网验收通过。** 显式移除临时wire探针和改写环境，仅使用产品代码，GLM Chat的ACP连续工作流12/12步通过：中文对话、文件写入/隐藏nonce读取、shell、真实MCP截图识色、流中取消、取消后续话、原生压缩完成、压缩后工具调用、无孤儿停机、冷恢复usage及恢复后工具调用。压缩前后reported usage为10373→7583，窗口258400；冷恢复恰好一次且数值一致。独立smoke同时通过，压缩后正确回复新的`post-compact-ok`。最终两个场景共22次真实上游请求（18+4）。

本轮共享预算未重置：含失败诊断、验收器修正和4次临时格式对照，共59/96次，预算拒绝0、残留活动请求0。前37次不计为最终产品验收；正式验收为第38–59次，临时网关改写已关闭。收据保留各阶段版本、结果和用途，不能将控制实验的通过替代产品通过。11058个真实工件及待提交历史经三个已提供密钥的原文/base64比对，未检出匹配；临时凭据文件已删除。此结果针对这些已知密钥，不宣称通用秘密检测器能发现所有形式的秘密。

旧150个工件不需要伪造更新或重录：Chat/Anthropic回放读取中间rig-events，不匹配最终HTTP请求，旧final-request是历史审计记录；因此回放绿不能单独证明这次wire修复。新HTTP回归才覆盖该边界。镜像不含Git元数据且测试产生的嵌套.git会误导自动fixture根发现，首次回放因此读到重复codex-rs路径；验证时使用已有LIVE_FIXTURE_DIR显式指向真实fixture目录，不删除或伪造Git标记。

补测试过程如实记录：新预算回归曾因误将CodexErr常量当枚举模式而编译失败，随后因固定4500窗口连空基线都放不下而失败；已改用details类型检查，并根据实际固定请求开销构造“空请求可容纳、10k图片必超窗”的窗口。日志保留，这两项不记作旧产品回归首败。

## 边界

不宣称这次已取得跨平台全量CI全绿。没有手动dispatch、默认代理迁移或发布安装包。原始CI日志与旧执行记录保留，错误归因在原文顶部更正。本轮先本地保存修复和定向验证；旧CI仍属于fee6cda7d，不代表本轮新修复树。

## 提交与收口判断

Codex代码/测试提交为`4d9c62ae6`（最终停机栅栏和实例身份）、`14a19eefb`（图片预算合同）、`170de0c0e`（CI夹具和清理握手）、`9b51c6289`（Chat纯文本格式兼容）；ACP驱动提交为`564b890`、`c2a8ca2`、`958b63f`，ACP产品源码本轮未改。

以“Linux/容器中通过ACP及rig接入自定义国内模型”为核心目标，本轮已完成发现问题的修复和最终真实工作流验收，可以进入日常使用，不再安排扩大范围的新功能开发。剩余Windows专属失败与完整跨平台CI仍属已登记验证尾项；没有将本轮单一真实GLM Chat验收扩大成所有厂商/协议均已重跑。后续若有具体使用失败，再以可复现问题安排修复。
