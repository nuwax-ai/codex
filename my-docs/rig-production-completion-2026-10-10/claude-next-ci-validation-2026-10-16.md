# 给 Claude Code：集成后验收收口

请从最新 `test` 继续，目标是让现有 `NUWAX_* → Codex/Rig → ACP dev`
使用路径可重复验收，先收口失败，不再重复main合并／Rig升级／图片预算改造。

## 起点与规则

- 主仓 `/Volumes/soddy/git_workspace/fork-nuwax-codex`，主用集成分支 `test`。
  开始先核对status、HEAD、origin/test，保留并行改动。本轮集成对象为
  `codex/rig-044-20261014`，交付分支为test；从合入后最新提交继续，不能回到
  `29d55354c`、`914a400d7`，也不要把旧worktree二进制当作test的产物。
- 先读 `AGENTS.md` 及本目录 `codex-integration-review-2026-10-16.md`、
  `image-output-budget-review-fix.md`。旧记录有明确撤回的假通过，请按更正读。
- ACP仓库 `/Volumes/soddy/git_workspace/fork-codex-acp-ts` 使用dev，main
  只同步上游；官方Codex和Rig参考仓库只读。不要改V8 pin、全局代理默认值，
  不操作已取消的artifacts迁移。
- 分机制小提交，每批自查；本轮不push、不手动dispatch、不发布、不跑付费
  API。完整workspace继续以自动CI／另行确认执行，不自行在本机全量跑。

## 第一批：当前推送SHA的CI与跨平台收据

1. 用gh只读找本次push自动触发的Fork Cargo PR，确认headSha等于test最新
   SHA。收取三平台及Bazel gate的首败，不用旧run的绿盖新run的红。
2. 按产品／夹具／机器条件分开定位。新失败必须有准确平台、toolchain、
   features、stack、selector、二进制及源码收据。guardian溢出、超时、
   AppArmor/userns、缺fixture不能只引用旧ledger宣称无关，也不加大栈、
   放宽timeout或早退掩盖。优先修复本轮相关失败，独立历史缺陷单列。
3. Linux `192.168.32.163`主验，Windows `192.168.32.235`用原生二进制；
   仅使用现有安全登录配置，不把凭据写工件。重点复核本轮新模型选择、
   original header估算、共享10K、20图stress、取消／冷恢复／压缩。
   WSL或Mac PASS不能替代Windows原生。不可达如实记未验，继续其他工作。
4. `anthropic_interrupt_during_tool_runs_it_once` 跨平台夹具适配独立提交：
   用确定的执行开始／取消握手，不让Windows停在exec approval，也不能
   靠跳过真实工具执行解决Linux userns问题。优先复用remote-tests帮助器。

第一批通过后提交证据账本：每项executed／skipped／failed及原因，不只写
nextest的PASS总数。修复需push触发新CI时，交Codex复审或另取明确授权。

## 第二批：把一次性ACP／live验证变成可复跑合同

1. 在ACP dev或合适的测试脚本目录归档此前tmp驱动的无凭据版本；禁止
   .env值、真实账户数据和真实截图入库。默认走本地mock，固定CODEX_PATH，
   校验binary receipt／hash后启动。环境传递、截图typed字段、取消、
   /compact、杀进程冷恢复和召回令牌要有机器可检查的断言。
2. 用真实Code Mode runtime分别验证 `image()`、普通JS return、显式
   `text()`：图片保typed；普通返回遵守现有不发内容合同；显式text仍是
   有界文本，不从任意Base64字符串猜图。不改变structuredContent优先规则。
3. 补三wire图片历史的热／冷恢复／local compact缺口，先复用既有测试。
   必须检查TurnComplete.error，捕获实际请求非零命中、每张图片身份、
   call/result和usage。普通采样配对与压缩文本的合同分别验证。
4. 扩展原图边界：正常桌面PNG/JPEG/WebP、orientation、异常metadata、
   多original共用额度、很小窗口、未知File引用保守处理。只用有界header
   信息计费，不恢复original免费，不降detail来绕限。用户消息与工具图
   准备路径分列，完整wire字节与输出预留继续保留。
5. 测试报告补明确的场景执行清单和HTTP attempt数。`84 PASS`含提前返回，
   不是84个付费场景；单attempt cassette不能代替pause/idle/HTTP重试证据。
   保留真实v1 golden；新v2字段缺省兼容，usage presence不许丢。

真实识图只在以后另给vision模型／端点／小批请求预算授权后运行。现有
MiMo红图收据不能代替OCR、多图定位或GLM/Step视觉验收；离线工作不受阻。

## 第三批：只处理仍影响日常使用的尾项

- 定性短生命周期 `codex exec` 退出时后台记忆巩固被杀的行为：区分
  正常取消与failed_agent、是否错误占用重试／任务额度、daemon/ACP是否
  受影响。先加可重复离线回归，再决定最小修复，不引入常驻守护平台。
- 当前模型选择已用实际runtime快照，显式override和OpenAI/Bedrock默认
  优先级已固定。新增consumer复用规则，避免重新从启动Config.model猜。
  guardian-v2固定Luna的既有async路径有bridge gate，需独立能力设计，
  不移除gate声称第三方兼容，不只改政策metadata冒充实际request模型。
- 代理/TLS做与日常启动相关的最小定向验收；每个失败保留真实原因，
  不转为危险的证书／代理全局放宽。

默认使用有界inline图片即可。File API／附件上传是后置opt-in，只有指定
provider支持且用户需要时才设计账户作用域、TTL、删除、取消、冷恢复与
失败回退；不要把它变成当前可用性的前置任务。

每批按tests → scoped just fix → just fmt → diff-check收尾，最终fix/fmt后
不重跑；依赖／配置／API／编译数据变化同步lock/schema/TS/Bazel。最终
报告具体SHA、执行数、首败、修复和未验边界，附下一最小批次即可。
