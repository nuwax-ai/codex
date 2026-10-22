# 当前收尾路线图（2026-10-22 晚复审）

目标：通过环境变量使用国内/自定义模型，支持 OpenAI Chat、OpenAI Responses、
Anthropic Messages，并保留绕过 rig 的上游原生 Responses 入口。
日常路径是 Linux/容器 app-server + ACP；Windows 已按用户决定纳入支持范围，
其 legacy 删除限制按“B+警告”接受，不重新立项重写 Windows 沙箱。

源码差异索引：[FORK.md](FORK.md)。连续事实台账：
[§6.55 起](rig-production-completion-2026-10-10/claude-rig-final-acceptance-2026-10-18.md)。
本轮复审：[晚间复审](rig-production-completion-2026-10-10/codex-review-2026-10-22-late-audit.md)。
此表取代旧路线图中“尚未push/进行中/待拍板”等过期状态，历史收据仍保留。

## 1. 已完成的核心能力

| 使用路径 | 配置 | 状态 |
| --- | --- | --- |
| 国内/自定义 OpenAI Chat | `NUWAX_WIRE_API=chat`，默认 rig | 已实现，有真实工具/图片/取消/恢复证据 |
| 国内/自定义 Anthropic | `NUWAX_WIRE_API=anthropic`，默认 rig | 已实现，实际能力仍取决于模型/网关 |
| 自定义 Responses 经 rig | `NUWAX_WIRE_API=responses`，默认 rig | 已实现，同协议透传及 cap/usage 合同在册 |
| 官方兼容 GPT/中转站走原生 Responses | `NUWAX_WIRE_API=responses` + `NUWAX_BRIDGE=native` | 已实现；真实子进程 fixture 已证明绕过桥，不是待开发开关 |

四条路径共同配置 `NUWAX_BASE_URL`、`NUWAX_API_KEY`、`NUWAX_MODEL`；
原生模式只允许 Responses，`native + chat/anthropic` 启动失败，不静默忽略。
显式 CLI/config provider 优先于 NUWAX 临时 provider；ACP 自有 CODEX_* 入口边界见下。

main@342ed69a1 已通过 merge a5b25b043 集成并随晚间批次推送。
复审输入 origin/test=850a87390；Rust1.99.1、V8152.2.0、rig0.44.0、genai0.7.0。
验收期间冻结这些版本，不再同步新的 main 或升级 rig。

已完成证据包括：三协议实网9用例、ACP工作流12/12、rcoder容器3/3；
这些二进制绑定 a5b25b043，不扩大为后续全部提交/全部厂商均已实网。
前轮1350协议等合同、365运行时/存储、晚间215定向合同分别以各自源码/参数为准。
215轮使用 justfile 的Unix 8MiB测试栈，不能把标准配方复跑写成默认2MiB也全过。

## 2. 当前必须闭合的有限尾项

1. **本轮缺陷修正已阶段完成。** doctor桥名、V8共享缓存/路径、备用genai query/header、
   Rust1.99 CI lint、TUI旧测试接口，以及/dev隔离退化均有定向结果；139/139合同、
   doctor五案、TUI兼容选集、Linux真实helper、Windows原生类型检查见晚间复审记录。
   TUI额外七例首轮6过1EOF超时，补取消完成握手后该单例通过；首红和调度长尾边界
   保留，下一次CI仍观察，不把一次通过当成所有负载上界证明。阶段提交不自动push。
2. **修复提交的CI收据。** run39761410573 head=c17b8675b：Ubuntu/macOS在
   Clippy阶段因fetch_update弃用失败；Windows在TUI测试编译阶段报7处接口遗漏，
   三个平台workspace测试均未执行。Bazel bridge gate通过。不能按旧红名单宣称通过。新修复的完整CI需后续推送触发；本轮只本地commit。
3. **实际GPT/中转端点的native短验收。** 已有fixture证明开关与出站请求；现有实网
   Responses主角是GLM且默认rig，没有本轮官方GPT/native中转实网收据。
   选定真实端点和相应凭据后，少量文本+工具+取消/恢复验收即可，不重做整个模型矩阵。
4. **ACP自有环境入口的native选择。** ACP的CODEX_BASE_URL/CODEX_WIRE_API会
   显式选择custom-gateway，目前EnvGatewayConfig没有experimental_bridge字段。
   此时单独设置NUWAX_BRIDGE不能覆盖显式provider；若正式部署用此入口，补桥选择
   映射与端到端对照是一个小任务。Codex内核的NUWAX原生入口已经完成，二者分开。

## 3. 构建与平台边界

- **#24已完成主要配方**：预取脚本、清单校验、Linux断网暖缓存release构建8m28s、
  Windows目标的archive/binding校验与env落盘均已有收据。action共用脚本是可选维护项，
  不作为模型接入收口门。本轮补共享缓存并发、相对路径及shell导出回归，8/8通过。
- **#53已结束**：旧head29e7e59d5的M1 mac job119602236907于05:48:01Z成功。
  不因旧“执行次数0”再发一手，不把旧成功用于新main。
- **Linux O2/O3/O4**：O3仅在fresh netns+fresh proc时按窄静态名单判缺席。
  d88ffb3d4已把/proc/self、thread-self、net及后代deny统一改为明确Fatal；
  这关闭的是静默失效，不能称实现了任意per-pid路径的可运行拒绝。
  O2晚轮只证明初始缺席，复审发现可重建、挂载覆盖与宿主devpts实例可见，现已
  以恶意重建及hostPTY sentinel生产helper探针复验修正。仅特殊deny分支禁用新PTY，
  普通私有PTY与授权shm写入仍可用；不能沿用“241/241所以安全等价”的旧理由。
- **Windows**：proxy/TLS两包及本轮TUI测试接口定向编译已通过；UDS实机10/10，runner需后续CI。
  三个legacy慢腿实机亚秒级通过。write-delete限制已接受并加WARNING，测试保持红，
  不放宽断言；elevated runner pipe没有解决，但按用户范围不继续扩修。禁止Windows全workspace测试。
- 发布/tag/npm未授权；不为收尾新增Files/PDF、多模态输出或泛化厂商私有API支持。

## 4. 收工条件

核心模型能力无需再大规模开发。关闭本轮确定的回归，取得修后CI与实际部署入口的
短验收，保留明确的Windows/协议限制，即可进入正常使用与按复现修bug的维护阶段。
不把缓存优化、尚无需求的供应商扩展或已接受的平台局限重新包装为一长串必做任务。
