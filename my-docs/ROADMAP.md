# 当前收尾路线图（2026-10-22 收尾验收）

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

## 2. 收尾验收状态

1. **Codex复审修复已推送。** `8ed1cbfbd` 修正provider字段、doctor、V8缓存、
   Linux隔离退化、Rust1.99 lint及TUI旧接口。定向证据见晚间复审记录，不再安排同一轮开发。
2. **ACP部署入口已补齐并验收。** `CODEX_BRIDGE=rig|native|genai` 进入实际provider
   config；默认仍由Codex选择rig。标题隐藏线程遗漏provider定义的真实缺陷已修，
   保留wire/bridge/headers/query及活动模型。四组真实ACP子进程fixture **28/28**，
   包括三协议默认rig及native Responses；每组标题、工具图片、usage、取消、压缩、冷恢复通过。
   三rig组各9条capture与实际HTTP body逐字节相等，native组9次HTTP/0条rig capture。
3. **GLM原生Responses实网已通过。** 用户选择现有GLM；`GLM-5.3-Flash` 四场景
   **4/4**（文本、真实工具、跨进程恢复、ACP取消后继续），共 **7/16** 次请求，
   denied/active/rig capture均为0，所有owned进程清理通过。未把GLM扩大为官方GPT已验。
4. **修后完整CI仍运行。** [run39774635523](https://github.com/nuwax-ai/codex/actions/runs/39774635523)
   绑定`8ed1cbfbd`。Ubuntu/macOS已越过上一轮Clippy首败，三平台进入二进制构建；
   尚无workspace Summary，不能宣称完整CI通过。只按该head实际终态判新回归。

详见[收尾记录](rig-production-completion-2026-10-10/codex-core-closure-2026-10-22.md)。
没有新的模型接入功能阻断，不再给Claude派一轮泛化开发。剩余工作是本手CI终态核对；
若出现新的明确回归则定向修复。未来换成官方GPT或具体中转站时，用该端点补窄验收，
无需为尚未提供的端点继续实现一套传输。

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
- 用户已于2026-10-22授权正式tag/CI/npm发布；本次交付主名nuwax-harness、
  默认数据.nuwax-harness（无旧目录兼容/迁移），npm包名保留nuwax-codex。
  发布前home12/12、npm安装合同12/12、七目标打包合同4/4、改名后ACP14/14通过，
  最终发布状态以tag/Release工作流/npm registry实际结果为准。
  不为收尾新增Files/PDF、多模态输出或泛化厂商私有API支持。

## 4. 收工条件

核心模型能力与实际部署入口短验收已完成，可以开始正常使用。修后完整CI的终态仍需
如实核对；保留明确的Windows/协议限制，后续按真实复现维护。
不把缓存优化、尚无需求的供应商扩展或已接受的平台局限重新包装为一长串必做任务。
