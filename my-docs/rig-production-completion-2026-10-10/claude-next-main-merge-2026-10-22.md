# 给 Claude Code 的下一轮提示词（2026-10-22）

你接手的是 nuwax Codex fork 的有限收尾工作。用户核心目标是通过环境变量选择国内或
自定义模型，经 rig 支持 Chat / Responses / Anthropic，在 Linux/容器 app-server + ACP
中稳定使用。已有功能不要反复重新立项，不扩成任意厂商私有 API 全兼容工程。

## 先固定坐标与证据

1. 唯一 Codex 仓库 `/Volumes/zhitai/git_workspace/fork-nuwax-codex`，使用 `test`。
   先核对 clean、HEAD、父提交与 origin 关系。最近一笔是 main 集成，父提交应是
   `387914de0` 与 `342ed69a1`。前者包含用户原来六笔未推送文档，不可丢弃。
   `/Volumes/soddy/git_workspace/fork-nuwax-codex` 和它的旧 worktree 禁作最新源码。
2. 先读 `my-docs/ROADMAP.md`、`../../FORK.md`、本目录
   `codex-main-merge-review-2026-10-22.md` 及其 JSON 收据。
   历史台账 `claude-rig-final-acceptance-2026-10-18.md` 从最新 §6.54 接续，
   §7 最新状态优先于旧历史段落；不要拿几周前绿收据覆盖新 main。
3. 上游 Rust1.99.1 / V8152.2.0，rig-core/rig-reqwest=0.44.0，genai0.6.5。
   不降 pin，不单独追新 rig。验收期间冻结这份 main，不再同步下一批上游提交。
   已有本地 V8 配对资产和真实运行证据，详见复审记录。
4. ACP 仓库 `/Volumes/soddy/git_workspace/fork-codex-acp-ts` 的 `dev` 为用户路径，
   `main` 用于同步官方；先核实实际 HEAD，保留已有 untracked vendor/，不重置用户文件。

## 第一批：冻结合并树，关闭核心接入的验收缺口

先做这一批；产品失败给最小复现与修复，再跑同一断言，不扩大为完整 workspace：

- 构建当前提交的 CLI / app-server 二进制并记录 source SHA、dirty 状态、toolchain、
  features、二进制 SHA。不能通过旧 PATH/vendor 二进制拿绿；注意上轮精确过滤错过
  shutdown 模块的教训，保存实际选中用例名单，不只写过滤命令。
- 离线优先验证 NUWAX 环境变量及 config 回退/覆盖、三 wire 工具往返、usage
  missing/partial/explicit zero、cap 错误不重采样、取消、冷恢复/fork/compact。
  main 新增 post-model hooks/mailbox、认证 Result API、Arc 工具容器、ContentItem、Rollout Box
  形状与 prompt-cache key：给这些交界补有意义的回归；已有同等覆盖就复用。
- 至少覆盖一次 cap + hook/interrupt、一次 AdditionalTools 增量工具、一次鉴权错误
  在发网前失败且不发送部分头。确认 ContentItem 渲染/预算与 PromptCacheKey omit/custom
  策略不改变独立 session-id header。新 API 字段逐项分类为保留、显式降级、协议不适用或
  真缺陷；请求抓取做规范化后精确比较，不能把所有未知字段一律忽略。
- 真实 ACP dev + 新二进制，Linux/容器目标路径跑最小完整闭环：初始化→首轮工具→
  取消→继续→冷恢复用量→本地图片识别→压缩后继续。检查 auth、用量、工具失败内容、
  图片/推理历史没有被新接口改动丢失；容器启动外呼与停机有界。
- Token Plan 真实模型调用已获用户授权，凭据只通过既有受保护环境注入，不复制到
  Codex home、日志、manifest、cassette、提交或提示词。用厂商实际支持的端点组合；
  不假设 Step 有 Responses。先离线，再少量 GLM/MiMo/Step 实网；不做长跑刷量。
  对图像不支持、协议字段不支持与桥缺陷分别归因，供应商限制不冒称产品通过。
- 图片必须进 typed image 字段（本地文件可编码 data URL/base64），不能把 base64
  当普通工具文本塞进上下文。复验 MCP image/structuredContent、Code Mode 图片、
  历史冷恢复与压缩，保留当前图数/字节/像素/视觉 token 限额；尺寸超限明确拒绝，
  不截断成损坏图片。不要无需求新增通用 Files/PDF 上传系统。

完成条件：上述当前树路径无未解释的产品失败，字段处置表与工件不泄露凭据，
确定性最小用例复现过的缺陷已修复；主链即可阶段收工，独立平台项另列。

## 第二批：把 #24 的本机突破变成可重复构建入口

复用上游现有 V8 manifest/SHA/host/helper 选择逻辑，提供一个清晰的本地命令，
支持预取、共享缓存复用、校验失败停止，以及明确的离线缺件错误。
优先抽取共享逻辑，避免 action/本地脚本各维护一套安全校验。
验证缓存命中、错 hash/错 binding/错平台拒绝、真正断网的暖缓存构建；
先 macOS 与 Linux，Windows 按实际发布需要补。记录下载阶段与离线阶段的区别。
证明 codex-cli/app_server_daemon 相关目标能在 CI 之外构建/枚举，不以单个 V8 smoke
代替整个目标；不用关闭 TLS、降 pin、假造绑定或复用旧二进制绕过。

## 第三批：真正的 Linux 沙箱缺口，单独修复和验收

这是已有真实产品问题，不是模型 API 缺陷或“环境抖动”：

- #42 / O2：`/dev/fd`、`/dev/core` 等符号链接 deny 导致 bwrap 命令启动前 abort；
  需要能表达同等拒绝合同的 mask 原语与能力探测。
- `/proc/self/environ` 与 `/proc/net` 的 deny 已有可读反例；O3 中新 netns 缺失
  `/proc/sys/net/*` 目标又会 abort。按自指路径/符号链接/新 namespace 分别解释。
- 先整理短设计与真实探针，选保持 deny 强度的实现，支持平台证明命令能运行且读取
  被拒绝；不支持平台给可解释的 fail-closed 错误。不得通过漏掉 mask、删断言或
  跳过失败转绿。read-only carveout / daemon socket mask 不顺手搬迁；它们的语义
  变化须单列，不借本任务把静默无效行为改成无诊断 abort。

使用已授权 Linux 实机，保留 userns/bwrap 能力与 mount/argv/读取结果证据。
单元 168/168 只证明构造合同，不是这些端到端拒绝已通过。

## CI 与 Windows 的边界

- 用户选 M1，已只发出一次：run39633345230 attempt2，mac job119602236907。
  先只读它的最新状态。若已完成，按 Summary 数测试、核对两个 bootstrap 与实际
  override；runner=非空、steps>0 才是执行。它的 head29e7e59d5 仍是旧树，
  不能验新合并。没有用户新授权不得再 rerun/dispatch，也不为了 CI 无意义提交。
- 新 main 的完整 CI、push 与发布不包含在本提示词授权内。先做完本地结果与提交，
  报告明确剩余项；不使用 tip 的 [skip ci] 意外抑制将来真实代码验收。
- Windows 6 个已知尾项（UDS ACL×2、legacy×3、write-delete×1）与 leaky 观测
  独立排期。正式支持 Windows 才把相关项当其发布门；**禁止 Windows 全 workspace**。
  新 main Windows proxy/TLS 分支至少需定向 compile/test，macOS 绿不能代验。

## 收尾交付规则

只修有证据的问题，不增大测试栈/超时、不自动 retries、不吞错误或跳过真实合同。
每批保留首红、修复、同源码同断言的绿收据；失败与取消、跳过分别计数。
定向检查通过后 git commit 阶段保存，更新 ROADMAP / FORK / 台账最新 §6.x，
报告已做、未做、阻断与可选项。每项有关闭条件；主链通过即说可以使用，
不要再生成几十条没有复现的新任务。
