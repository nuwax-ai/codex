# 模型接入收尾验收（2026-10-22）

用户要求剩余工作较小时直接开发和测试收尾，并选择现有GLM验证原生Responses。
本轮已完成有限的产品改动与部署入口验收，没有新增一轮泛化开发任务。
**核心接入可开始使用；完整跨平台CI尚在运行，不能写成全平台全绿。**

## 源码与提交

- Codex工作副本：`/Volumes/zhitai/git_workspace/fork-nuwax-codex`，`test`。
  代码提交`8ed1cbfbd`已推送（850a87390→8ed1cbfbd），触发
  [run39774635523](https://github.com/nuwax-ai/codex/actions/runs/39774635523)。
- ACP工作副本：`/Volumes/soddy/git_workspace/fork-codex-acp-ts`，`dev`。
  `45c78f1`环境bridge与标题配置修复；`869401b`可复跑验收合同；两笔已推origin/dev。
  不把预存未跟踪vendor/或Python缓存入库。
- main已于前轮以a5b25b043集成342ed69a1，不重复merge或升级依赖。
  冻结Rust1.99.1 / V8152.2.0 / rig0.44.0 / 备用genai0.7.0。
- 本轮Codex后续提交只更新文档和收据，带[skip ci]，不重新触发第二轮完整CI。
  无release、tag或npm发布。

## 修复的实际缺口

1. ACP `CODEX_BASE_URL`入口原先没有桥选择字段。新增`CODEX_BRIDGE=rig|native|genai`，
   映射到thread/start的model_providers配置；native只允许OpenAI Responses，genai只允许
   Chat/Anthropic。缺BASE_URL、未知值、非法组合启动失败，报错不回显敏感输入。
   不设置桥则省略字段、保留Codex默认rig；NUWAX_BRIDGE不覆盖这个显式provider。
2. 真实子进程首轮发现标题隐藏线程丢provider定义：主线程全部成功，唯一titleModelOk=false。
   保留首红，修复两个标题入口，共享选定provider的完整配置快照，带上URL、wire、bridge、
   auth、headers、query；不把主会话MCP等无关配置复制进隐藏线程。自定义配置即使覆盖
   openai这个ID，也使用活动模型；没有覆盖的内置OpenAI/Bedrock保留既有标题模型策略。
3. 验收脚本首因/工件保留：S7工件坏JSON或URL记录失败而非覆盖此前场景结果；
   native收尾统计失败仍关闭Gateway，cleanup/evidence错误另列，不覆盖原失败。
   父环境CODEX_BRIDGE从预算子进程剥离，避免继承其他路由。
4. 实网ACP取消helper原本只kill后exit。现要求真实有身份的流式delta、cancelled、
   同session新随机nonce精确续答，有界确认自身进程树退出后才输出通过。
   不操作无关进程，不用取消成功代替继续与清理成功。

环境网关本来已授权，无需额外authenticate。显式providers/set或任何authenticate
（API-key/ChatGPT/gateway）仍按旧优先级替换/清除环境配置；本轮不更改登录优先级。

## 验收结果

| 范围 | 最终结果 | 证据含义 |
| --- | --- | --- |
| ACP完整Vitest | 870 passed /32 skipped，0 failed，31.52s | 最终产品源码；未新增skip |
| TypeScript | 四个项目均通过 | src、examples、mock、live驱动 |
| mock/routing合同 | 41/41 | 含native空请求/坏工件拒绝、逐字节与重复次数负控 |
| Python合同 | 39/39 | 其中native专门合同10条；不与其相加 |
| budget/workflow合同 | 26/26 | 网关预算及既有工作流 |
| cancel helper合同 | 4/4 | 真实父/孙进程清理、无关进程存活、错误保留、nonce严格性 |
| 实际ACP→Codex→mock HTTP | 28/28 | 下述四组，每组7合同 |
| GLM原生Responses真实API | 4/4，7/16次请求 | 文本、真实工具、冷恢复、ACP取消后继续 |

四个CODEX_*组：Chat/default rig、Anthropic/default rig、Responses/default rig、
Responses/native。均检查实际标题请求、model/auth/path、usage、MCP typed图片、取消、
压缩及冷恢复。三个rig组各9次HTTP/9capture，body_raw与服务端实际body逐字节相等；
native组9次HTTP/0rig capture。合成图片fixture证明结构与生命周期，不代替真实视觉能力。

GLM使用NUWAX原生入口、`GLM-5.3-Flash`，HTTP/stream retries=0，未自动重试场景转绿。
文本1次、工具2次、冷恢复1次、ACP取消后继续3次，共7次实际admitted请求。
Gateway denied=0、active=0且正常退出，rig capture=0；exec三场景有结构化usage，
各owned进程组均已消失。真实Key三个值扫描工件5628文件，命中0。
**officialGptVerified=false**：用户选择GLM，因此没有冒称官方GPT或任意中转端点已测试。

新二进制从8ed1cbfbd clean源码构建，debug aarch64-apple-darwin；exec receipt的
依赖feature图仍标unknown，不扩大收据保证。三个二进制SHA及源码指纹、ACP22个改动
文件SHA、机器可读完整结果见[验收收据](receipts/2026-10-22-core-closure.json)。
mac linker compact-unwind大小警告与原有cli未使用变量警告保留，不冒称构建零warning。
此前Linux/容器/厂商实网记录仍只绑定原有各自SHA，不扩称本轮全平台又跑一遍。

原始工件：
- `/Volumes/zhitai/tmp/codex-final-closure-20261022/`（构建、完整单测、实网）。
- `/Volumes/zhitai/codex-validation/acp-bridge-final-20261022-1f7vk2tn/`（四组最终fixture）。
- `/Volumes/zhitai/codex-validation/acp-bridge-closure-20261022-104t89pg/`（title首红）。

## 剩余工作

只剩本手完整CI终态核对，不是另一轮功能工程。Ubuntu/macOS已通过上一轮首败的Clippy步骤；
Windows尚在二进制构建，TUI测试编译/运行待判，尚无workspace Summary。实际结果按8ed1cbfbd判读：新回归定向修；不以
旧head成功、缓存日志或环境标签替代判决。Windows legacy write-delete仍是用户已接受
的B+警告边界，原断言保留；不为它改写沙箱。个人Windows机不跑全workspace。

换到具体官方GPT/中转站时补该端点短验收即可；兼容性与模型视觉/推理/搜索能力仍需
按实际平台确认。没有继续升级rig、实现通用Files/PDF或厂商私有API的收口要求。
使用方式见[快速开始](nuwax-rig-quickstart-2026-10-18.md)，当前状态见[ROADMAP](../ROADMAP.md)。
