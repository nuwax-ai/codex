# Codex 复审与核心接入收口（2026-10-18）

## 结论与范围

按用户的核心目标收口：通过 rig 使用自定义国内模型，以 Linux/容器中的 Codex app-server + ACP dev 为主路径。三协议接入、工具、图片、取消、压缩、会话恢复等已有实现；本轮复审修复后可进入日常使用，不再追加泛化功能开发。

兼容 OpenAI Chat Completions、Responses 或 Anthropic Messages 的平台通常只需配置 `NUWAX_BASE_URL`、`NUWAX_WIRE_API`、`NUWAX_MODEL`、`NUWAX_API_KEY`。这不代表任意平台或模型的视觉、工具调用、推理参数、上下文能力完全相同。具体启动方法见 [接入说明](nuwax-rig-quickstart-2026-10-18.md)。

复审输入：Codex `e5b0eaa0a..8d1420ec6`，ACP `6dec6c8..d39acbf`，结合原交接书和执行记录。没有对先前完整 workspace/跨平台全量结果重新作出通过声明。

## 发现并修复

| 提交 | 发现与修复 |
|---|---|
| Codex `998de9bc1` | 停止转发共享配置层 personality 的修复同时漏掉了显式选择 profile 的 personality；将仍然胜出的 profile 选择纳入既有 launch overrides，补 origin 矩阵。 |
| Codex `7d9dc6834` | Windows测试helper在全局锁外复制且复用旧二进制；移入锁内并按内容刷新。恢复ConPTY严格中断断言，撤销没有足够证据的整个Windows版本范围豁免；加强popup反向断言。 |
| Codex `da92ea9ce` | analytics mock用整个请求体的web_search子串分类，导致review误路由/循环；按最新planned action精确匹配、逐项expect、真实通知握手和analytics终态收集。撤销4分钟放宽。新增HTTP-only provider零WebSocket预热回归；原pool产品修复保留。 |
| Codex `309becbe3` | 清理TUI配置迁移留下的未使用import。 |
| ACP `9af0039` | warm load的账本继承没有覆盖warm resume；统一恢复逻辑，显式零值/空窗口快照仍具权威性，只允许同session继承。新增15例回归。 |
| ACP `6b6f677` | mock跨日环境消息、图片标签误判、Chat usage帧等边界；用OS分配端口、PID/源码SHA就绪收据和退出等待防止误复用旧mock。恢复用量按session和句柄验收。 |
| ACP `269c47e` | 开启调试日志时，启动认证配置与原始协议帧会写入API key。启动日志仅保留配置字段名/认证方法，协议流仅记录方向和字节数，URL诊断只记录配置状态；真实index子进程回归证明认证传输不变且日志不含测试凭据。 |
| ACP `854f2c3` | 离线驱动隔离继承的NUWAX变量、MODEL_PROVIDER、CODEX_CONFIG，避免宿主配置把mock请求导向真实平台。 |

没有修改rig版本、模型接口合同或新增供应商专用分支。没有用放宽中断断言、增加重试或拉长analytics期限来转绿。

## 本轮终态验证

Linux镜像目录 `/tmp/codex-fa`，ACP目录 `/tmp/codex-stab-acp`。逐文件SHA核对：14个本轮变化的Codex源码/配置文件、19个ACP运行及驱动文件与本地提交一致。Linux最终重建 `codex-cli` 和 `codex-exec`，开启 `codex-core/rust-rig`，不是沿用旧release二进制。

| 验证 | 结果 |
|---|---|
| Guardian pool/HTTP-only回归与TUI配置、popup定向集 | 7/7，零重试 |
| 真app-server guardian analytics | 1/1，3.163秒，零重试 |
| ACP完整Vitest | 839通过，32个既有skip，零重试 |
| ACP四个TypeScript检查域 | 全通过 |
| ACP mock/stress合同 | 37/37 |
| 真实调用预算与输出解析合同 | 12/12 |
| Guardian定向Clippy（`-p codex-guardian-v2 --tests --features codex-core/rust-rig -- -D warnings`） | 通过；不是全workspace Clippy |
| 最终Linux二进制热/冷恢复用量 | 通过；初始、同进程重载、冷恢复均 used=5、window=258400；继承的冲突provider配置被隔离 |
| 真实Token Plan小批验收 | 6/6：GLM Chat、Step Anthropic、GLM Responses、MiMo Chat、MCP截图识图、ACP会话；真实上游attempts 10/48，拒绝0、残留请求0 |
| Windows helper同长度重建回归 | 原函数2过1败→修复后3/3；Windows原生独立函数测试，不冒称完整sandbox包或提权启动验证 |
| Rust定向格式、两仓diff-check、Bazel lock更新及严格依赖检查 | 通过；无需更改Bazel锁内容 |

机器可读收据：[codex-review-closure-receipt-2026-10-18.json](codex-review-closure-receipt-2026-10-18.json)。其中含实际二进制SHA、文件指纹、真实调用结果及准确的测试范围。最终live工件扫描16799个文件，没有匹配三家真实参考Key的原文或base64；临时凭据文件已删除。待推送两仓提交信息/补丁也完成同样的Key核对。此检查不等于能对任意上游错误字符串作出永不泄漏的保证，也没有删除用户旧日志。

## 修正旧证据表述

- 旧94批/5076轮长跑使用mock `5869e00`，不能记为包含 `0e0dcc3` 跨日修复的最终mock实跑证明；本轮跨日边界用确定性单测覆盖，没有重跑4小时长跑。
- 旧Linux release收据基于 `a6d34d279`，不包含之后TUI修复和调试打印清理；本轮使用重新构建的最终源码，并另存二进制指纹。
- 单台Windows build26200及不完整控制探针不能证明所有build>=26100都不支持ConPTY中断；保留真实失败，不豁免该合同。
- 旧analytics四分钟超时与web_search子串路由不能作为稳定通过依据；已替换为精确路由和通知同步，真实测试约3秒通过。

历史执行记录与CI账本顶部已补充相同更正，保留旧记录用于追溯。

## 收口后的工作边界

本轮源代码保存到Codex `test` 与ACP `dev` 并推送。源码提交不是npm/安装包发布；现有宿主应明确指向本次构建的Codex并使用ACP dev，避免默认依赖仍解析到旧版本。

Windows ConPTY中断、提权runner握手/验机配置、跨平台全量CI和安装包发布仍有兼容或交付尾项。它们不计作Linux/容器ACP自定义模型接入的未完开发，也未被宣称全部通过。用户此前选择本轮只做定向验证，完整workspace验收交给下一轮/CI；本轮未越过该边界。

建议停止连续的大批开发交接，先按接入说明在实际宿主使用；仅对真实使用复现的问题另开修复任务。本轮无需再给Claude一份泛化开发清单。
