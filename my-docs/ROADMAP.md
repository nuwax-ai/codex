# 当前收尾路线图（2026-10-22）

核心目标：环境变量选择自定义国内模型，经 rig 的 Chat / Responses / Anthropic
协议在 Linux/容器 app-server + ACP 中使用。主链已有实现与真实工作流证据；
本轮同步上游 main 后应先验证兼容性，不扩大成支持所有厂商私有 API。

源码行为索引：[FORK.md](FORK.md)。历史事实与原始收据：
[验收台账](rig-production-completion-2026-10-10/claude-rig-final-acceptance-2026-10-18.md)。
本文只保留当前优先级；`#24/#42/#53` 沿用台账中的历史任务编号，不重新编号。

## 1. 最高优先：main 集成后的定向验收（核心主链已完成，见 §1.1）

同步目标 `main@342ed69a1`，对应 Rust `1.99.1`、V8 `152.2.0`；rig 保持
`0.44.0`。合并保存在本地 test（merge commit `a5b25b043`，两父提交
`387914de0`/`342ed69a1`），尚未 push。§6.54 完成集成与定向修复后，
§6.55（2026-10-22 下午）完成了冻结二进制的实机/实网/容器验收：

### 1.1 第一批验收结果（台账 §6.55，全绿）

| 证据 | 结果 |
| --- | --- |
| 双平台二进制收据 | mac arm64 与 Linux x86_64 的 codex/codex-exec sha256 均登记；Linux 构建经新预取脚本取 V8 资产 |
| env-determinism（零付费） | Linux 实机 PASS（0 外呼）；rcoder 产品容器内复跑 PASS |
| 三协议实网最小批 | GLM chat / STEP anthropic / GLM responses 三线 + 工具/取消/冷恢复/typed 图片/ACP 冒烟 9 用例 PASS |
| acp-workflow 12 步闭环 | 首败=预算饥饿（30/30 耗尽后 2 denied，账本证据）；同断言重跑 **12/12 PASS**，压缩后 usage 收缩可见 |
| 容器腿 | rcoder 镜像 + 新二进制覆盖挂载，3/3 PASS，收尾无残留进程 |
| 离线补跑 215/215 | NUWAX/hooks/cap/prompt-cache/增量工具套件三次归因（2MiB 栈→SIGABRT、高负载假红、host/helper 二进制未构建）全部转绿，同断言复验；新增 cap×post-model-hook 交界回归 |
| 新 API 字段处置表 | 已入台账 §6.55（hooks/鉴权 Result/Arc 工具/ContentItem/Rollout Box/PromptCacheKey/mailbox 三线分类） |

剩余尾项：Windows proxy/TLS 定向编译（实机探测中）；#24 断网暖缓存完整构建
证明。用户 2026-10-22 追加：genai 备用桥升级 0.7（仅要求编译通过，进行中），
rig 仍冻结 0.44.0 不动。

- 核对冲突解决保留 NUWAX 环境变量路由、rig 三协议、输出 cap、usage presence、
  工具与图片/推理历史、身份隔离及停机清理等 fork 合同。
- 对受影响源码运行定向构建与回归，记录真实源码/工具链/二进制对应关系。
  新 V8 资产已完成本地配对下载与校验；其证据不等于跨平台通用离线配方已完成。
- 不把旧三协议实网、旧 CI 或单一厂商的通过扩大成合并树已全绿；只修有证据的回归。
- 冻结本轮 main 与 rig 版本完成验收，期间不继续追新的上游提交，避免反复移动验收基线。
- 本轮以阶段提交保存；push、额外 CI 或发布不因“更新路线图”自动获授权。

完成条件：冲突解决可解释，受影响合同和定向检查通过，验证缺口明列；不要求先做完
以下全部独立平台或可选扩展。

### 本轮验证快照（阶段集成已完成）

| 证据 | 已确认结果 | 边界 |
| --- | --- | --- |
| 六包协议/桥接等合同 | 1350/1350，通过；另有1个schema专用跳过 | schema由官方稳定/实验两个writer单独验证；跳过不计通过。 |
| rig/genai/http/model四包 + 最终Core/ThreadStore lib Clippy | `-D warnings`通过；最终fmt通过；diff-check仅保留main原样定宽快照例外 | 局部严格检查，不代表workspace零警告。 |
| Core/app-server/exec-server原定向集 | 83/83通过 | 扩选前结果，不能覆盖后续栈失败。 |
| 最终运行时/存储集 | **365/365通过**，retries0、原默认栈 | 包括原88条、turn-context两条及ThreadStore全部275条lib测试；先前SIGABRT首红保留。 |
| 新V8与构建 | 复用官方action脚本并验证清单链，下载/校验152.2.0 archive/binding pair；最终Core/app-server/CLI/live-tests all-targets check通过 | 此检查覆盖最后的stack修复；不是CLI启动端到端或厂商实网收据。 |
| Code Mode真实JS | 2/2通过 | 指本轮实际执行路径，不代表全部工具/多模态扩展均验收。 |
| Linux独立镜像 | sandbox lib168/168、http/model lib271/271通过，均保留源码比对与首败/修复记录 | 各自镜像快照的定向证据，非最终HEAD整体或O2真实沙箱验收。 |
| Bazel | `mod deps --lockfile_mode=error`及V8 module一致性检查通过 | 不扩大为全量Bazel测试通过。 |

最后的stack修复与365条扩选结果见本轮复审记录及精简JSON收据；旧83条的绿、旧实网
和M1不能扩大这一步的范围。§1.1 所列的冻结二进制 ACP/容器/三协议最小工作流
（工具/typed图片、取消、压缩后新指令、冷恢复）已于 §6.55 完成并留有源码/产物
指纹；后续不再重复该轮验收，除非基线再次移动。

## 2. 构建与 CI 的有限尾项

| 项目 | 当前状态 | 下一步与关闭条件 |
| --- | --- | --- |
| #24：可复现的 rusty_v8 构建配方 | `scripts/prefetch_rusty_v8.sh`（提交 5d91ca221）已落地：仓库内主清单校验、共享缓存复用、离线 fail-closed、篡改即删；缓存命中/篡改/错目标三路径实测，Linux 构建实际使用 | 剩余：`unshare -n` 断网暖缓存完整构建收据、Windows 目标验证、（可选）action 改造为共用脚本。旧150.4.0不可替代，不降pin、不跳校验。 |
| #53：两个 macOS bootstrap 测试的120s override | **已收口（2026-10-22）**：M1 唯一一次 rerun 的 mac job `119602236907` 于 05:48:01Z `completed/success`，`Test (workspace)` 步 success——两个 bootstrap 腿在该作业内通过，override（`29e7e59d5` 配置）经真实执行验证 | head 仍为 `29e7e59d5`（旧树），只证明该配置的窗口，不覆盖 main 合并树；合并树 CI 需另行授权 |
| CI证据范围 | attempt 1的Bazel通过，Ubuntu1个已知红、Windows6个已知红；mac容量未接单这一历史已由M1接单推进 | M1 head仍为29e7e59d5，只复验旧树配置，不覆盖main合并树；其余三腿旧完成结果不是新执行。本轮不push，不触发新的合并树CI或发布。 |

#53 的绑定可用已有测试产物与临时配置做确定性核对；不要反复消耗 runner 等一次
随机变慢至 60s 以上才算证明。若监视 CI，必须绑定正确 run/attempt，探针报错与
排队分开，状态不全时不覆盖基线，并设置监视自身的墙钟上限。

## 3. 独立安全语义：#42 / O2 / O3

**2026-10-22 晚间更新：O2/O3 已实现并实机验收（台账 §6.56）**——deny 命中
bwrap 自建 `/dev` 符号链接项时切换显式构建（被拒项缺席即拒绝；`blocked_descriptor_path`
由红转绿，strace 28 次 bwrap execve 零 `/dev/fd`）；宿主专属
`/proc/sys/net/*/conf/<网卡>` 在 fresh netns 中按缺席拒绝（静态窄名单，宁可漏判
维持响亮 abort）。O4 的目录 deny 半边同轮收口：`/proc/net`/`/proc/self/net`
deny 由静默可读改为响亮 Fatal（fail-closed）。已知偏差：`pts` 绑宿主 devpts
（无 CLI 私有实例原语）。
**仍开放**：pid≠1 子进程的 `/proc/self/net` 可读（需 per-pid 枚举/seccomp 决策）、
`/proc/self/environ` 可读边界；判据见
[sandbox-o2-o3-design-2026-10-22.md](sandbox-o2-o3-design-2026-10-22.md)。

以下原始边界描述保留作历史：

这些是已知沙箱语义问题，不归为模型厂商限制或随机 CI 波动，也不应通过删断言或
跳过 deny 让测试变绿。需要单独设计与验收，不在本轮 main 合并中顺手实现。

- **#42 / O2：符号链接路径的可用 mask 原语。** `/dev/fd`、`/dev/core` 的 tmpfs
  mask 经过符号链接解析后目的地缺失，当前 fail-closed abort；`blocked_descriptor_path`
  仍会红。需能力探测与明确降级策略，验证命令能启动且读取被拒绝。
- **O3：新 netns 中不存在的 `/proc/sys/net/*` 目的地。** 仅宿主存在的条目会导致
  启动 abort；不能以“目的地不存在就不 mask”作特例，旧实验已产生 fail-open。
- **已知fail-open不能省略。** `/proc/self/environ` 在已登记deny探针中仍可读；
  `/proc/net` 符号链接的目录deny也存在可读边界。这些与上述fail-closed abort一起
  纳入独立安全设计/修复，不能声称所有`/proc` deny已完成或归为flaky。
  源码、argv、实际读取及启动可用性都要验收。

关闭需要真实支持 userns/bwrap 的 Linux 主机证据；没有能力的主机应产出明确跳过
收据，不能把 skip 视为拒绝合同通过。O1 的已有顺序修复与上述未完成项分开记录。

## 4. Windows 与交付，按使用范围验收

最新完整 Windows 手仍有两条 UDS ACL、三条 legacy 和一条 write-delete 失败，
另有 leaky 观测。按各项已有证据继续处理，不把计数简单写成模型桥回归。
Windows 若作为正式日常路径，需要独立关闭其相关尾项；Linux/ACP 收口无需无限等待。
npm/安装包发布、默认代理迁移、通用 PDF/Files 生命周期及全部输出多模态均不在本轮范围。

## 5. 已完成主链与停止条件

NUWAX 环境变量、rig 三协议、工具/typed 图片、取消、压缩、冷恢复和恢复用量已有
历史定向与真实厂商收据；字段处置中的协议限制仍成立，不承诺任意字段跨协议原样无损。
MCP structuredContent媒体保留、raw工具失败/整数精度防线及原图预算下界已修复，
不要重复开发。main集成后的运行时回归现已通过，冻结后做上述CLI/ACP/容器/三协议
最小验证；构建配方、安全修复和按需Windows尾项分别推进，不追加泛化供应商功能。

达到第 1 节阶段条件即保存集成结果；未完成项保持本表准确状态。只有获得新复现或
明确平台/交付需求时追加工作，不再循环扩展泛化开发清单。
