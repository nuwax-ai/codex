# 下一轮：main 同步、Rig 升级与图片开发的顺序

日期：2026-10-14。用户刚更新 fork 的 main 用于同步官方，要求评估是否整合进业务分支，并考虑升级 Rig。以下是实际分支与源码评估，尚未执行合并、依赖升级或测试。

本轮补充：用户已授权 Codex 复审 shell snapshot 修复并 commit/push。下一轮起点以该阶段 push 后的 `origin/test` 为准，不能回退到本文件统计时的旧 SHA。Codex 已修复 argv[3..] 丢失并补真实执行回归；Bash 长选项顺序是自身规则，原“wrapper长选项缺陷”登记已撤销，不再安排此虚构机制。独立复审结果见 `codex-shell-snapshot-review-2026-10-14.md`。

Codex 产品修复checkpoint：`190a99acc`。macOS相关51/51通过、Linux36/37（已有FD-Deny残败）；原提交与候选同条件full复现同一macOS watcher失败。继续开发前读取复审记录，保留这些已验证修复与未关闭残差。

## 当前基线与实证

- 工作仓库：`/Volumes/soddy/git_workspace/fork-nuwax-codex`。
- 业务分支：`test`，HEAD `7df7dc8f89ab483315dd6dc8ad8fc02b8b5669df`，与 origin/test 对齐。
- 上游同步分支：`main`，HEAD `cb03f28a4034b6ee901885808d27182ac586830a`，与 origin/main 对齐。
- merge-base：`67727e7cf114cf3e1b71db368d74b24e32f6cb12`，2026-09-30。
- `git rev-list --left-right --count test...main`：test 独有327提交，main独有562提交。这是历史身份统计，不表示562个独立缺失功能。
- upstream 自 merge-base 累计：2949文件，+138508/-41025。两侧都改过209个路径。
- 用临时 Git object directory 执行 merge-tree，未修改工作树/index/branch，模拟结果为42个冲突路径，Core17个、workflow7个。临时完整输出 `/private/tmp/codex-merge-preview-wo_2t7ft/merge-tree.txt`；冲突数量随新提交可能变化。
- main 和 test 的 Rust toolchain 都是1.95.0，v8 crate 都是150.4.0；main改变Bazel9.0.0→9.2.0、rmcp3.3.0→3.5.1，仍需校准构建和MCP行为。
- 当前桥接层和live-tests都精确锁rig-core0.42.0。docs.rs 的 latest 当前显示已发布0.44.0：[当前发布API](https://docs.rs/rig-core/latest/rig_core/)。GitHub release列表搜索缓存较旧，不能拿它反证新版本未发布。
- 本地 `/Volumes/soddy/git_workspace/rig` 为0.44.0、HEAD `9e7cb6000`，只是参考源码；不能假定它与发布tarball完全一致。开发时核对registry目标版本/未yank状态、checksum和实际发布API。

## 决策

**应该同步 main；Rig升级有价值，但要作为独立迁移验收。** 推荐顺序：

1. 先完成图片任务文档的第一批：小范围修复桥接层Base64预算误算并固定三协议回归。
2. 独立整合main，保留fork产品合同，完成整合候选验收。
3. 独立迁移至已发布、精确锁定的Rig0.44.0（开始时重新核验版本；更高版本须先重做差异审查）。
4. 在整合后的Core生命周期上完成MCP入口硬界、单工具多图预算及连续截图/提前压缩。

先修的wire guard在fork自有bridge中，适合一个小阶段；完整截图/压缩设计则涉及main已改变的生命周期，先整合可避免重复适配。若Rig迁移扩成多轮，可先交付“合main + 已有Rig0.42 + 图片预算修复”的可用候选，不让追新依赖阻塞主用路径。

这更新此前图片文档的边界：“图片功能批次不同时升级Rig”，不再表示整个下一轮禁止独立升级。三个大动作不能混成一个提交，也不能只依赖编译通过判断行为兼容。

## 给 Claude Code 的执行要求

先读AGENTS、最新results/tasks，以及 `claude-image-budget-followup-2026-10-14.md`。确认实际HEAD和并行改动，保存当前测试身份和首败。中心是Codex；官方和Rig参考仓库只读。所有工作先阶段commit，不push、不dispatch、不发布、不调用付费模型。

### 阶段一：最小图片预算修复

按图片交接文档第一批执行，先得到独立commit和三wire离线回归。保留32MiB wire bytes硬限，区分typed图片视觉估算与实际文本，保留输出reserve、真实文本/工具schema/arguments超限拒绝。不要同时更改Core压缩或Rig版本。

### 阶段二：独立合并 main

在基于业务分支的隔离worktree/`codex/`前缀整合分支中执行真实merge。保留test现有提交，不rebase/force-push/reset/clean；main继续用于同步官方，不承载fork业务修复。最终整合候选通过后，再合入test。

合并时逐机制解决冲突，不批量选ours/theirs。生成文件从合并后的源重新生成；依赖/Bazel锁按工具更新，不手工拼lock。上游同步本身体积很大，单列merge checkpoint与说明；衍生逻辑修复仍尽量每批<500 changed lines。

必须保留并验证：

- NUWAX_* 环境变量seed与配置/CLI优先级；Chat/Anthropic/Responses；默认Rig路由、显式genai/native选择与feature gates。
- 原provider identity/auth域隔离、cap/usage ledger、压缩sampling host key、恢复兼容、TLS分类、代理与请求预算合同。
- Guardian boxing及原始4MiB合同；当前results已记录R11三平台绿，合main后需要重验，不能倒退到删测试/增栈。
- fork CI构建helper、userns/磁盘/Bazel修复和V8工件校验。上游未改v8 pin，不借整合主动升级它。
- 本项目用户已经给出的AGENTS规则和fork工作流意图；审查上游AGENTS/skills删减，不让合并误删本项目仍使用的约束。

**即使没有Git冲突，也必须审查自有bridge：**

1. main改变protocol类型：OutputText新增annotations/logprobs，Message/FunctionCall等有encrypted_content/status，MessagePhase增加PartialAnswer。适配Rig/genai/request转换、response构造、fixture、生命周期；不能只在match加通配符掩盖字段语义。
2. main把base instructions搬进input developer消息，并移除旧ResponsesApiRequest.instructions。三wire验证system/developer映射，既不漏发也不重复，检查incremental tool catalog与Responses Lite。
3. main新增后台压缩和完整context安装/usage更新。后台压缩默认关闭且有remote-v2能力gate；不能擅自对国内Chat/Anthropic开启V2。新流程中的取消、旧/新历史合并、host key及usage需实证。
4. main移除patched-zsh backend；按新执行器适配，不复活被删除的运行时来凑兼容。
5. main新增`truncate_mcp_tool_result`用于分页线程历史等场景。先核调用范围；不能将为UI/RPC输出做的整包JSON文字preview移植成模型截图路径，也不能把这当作已解决视觉预算。

上游可吸收的修复包括b49e2211a（snapshot helper事件乱序）、89c8759d8（子孙进程继承管道drain）、ae7aad586（bwrap多denied-file mask独立FD）、8571b9eaa（compaction完整context安装）、9e03960cf（后台压缩生命周期）。复验具体失败身份后才关项。它们不证明当前4个Linux snapshot残差和skills `/tmp/.git`污染全部关闭。

整合验收：Codex实际构建、三协议env/mock工具往返、cap/usage/old rollout、取消、warm/cold resume、local compact、guardian原4MiB、app-server公共JSON-RPC及ACP dev固定CODEX_PATH消费。Linux优先，Windows原生定向复验；记录源/bin/features/hash。只用just test；完整workspace延续用户选择交CI。通过后给出可用整合checkpoint。

不同checkout的基线/候选验证优先使用独立target目录；若共享缓存，必须核对实际测试身份和source/bin绑定。本轮曾观察到共享target复用了基线测试产物，不能只凭cwd或命令成功认定测试了新代码。

整合后优先重现 `shell_snapshot_concurrent_replays_keep_independent_readers::blocked_descriptor_path`，保存第一次stdout/stderr/退出信号和FD传递证据；这是本轮Linux已登记残败，不因主机制修复而关闭。macOS native watcher 并发超时按独立机制记录、先基线对照；不提高deadline或增加重试来验收。能在小批内定位的真实阻断直接修复，其余给精确复现和下一步，不无界扩张本轮范围。

### 阶段三：Rig 0.42 → 0.44 独立迁移

从已验证整合checkpoint开始，在独立`codex/`前缀迁移分支锁定发布版本。不要用本地path依赖或浮动Git main作为最终依赖；不要引入rig-agent/ECS/新代理循环。Codex仍拥有工具执行和turn循环，bridge继续隔离provider/wire适配。

旧计划“只改一个crate”已经不完全成立：至少涉及`codex-rust-rig-bridge`、`live-tests`、fixture接口、Cargo.lock和MODULE.bazel.lock。评估所有精确0.42依赖，统一升级目标；明确reqwest0.12/0.13类型边界、TLSfeatures与rig-http/rig-reqwest新传输抽象，避免误引入默认大feature矩阵。

迁移重点：

- CompletionRequest/preamble/options与provider options；Message::Assistant→AssistantMessage；CallId/ProviderCallId/ToolName。保留准确工具ID和块顺序。
- StreamedAssistantContent/StreamFinal→新Item/StreamEvent/CompletionResponse；新版首次poll才发送。验证零发送取消、流中取消、迟到usage、末帧/重复终局、工具参数分片和实际请求计数。
- CompletionError→ProviderError，保留status/request ID/body、TLS重试分类、脱敏和错误来源链。
- **Usage变为Option，input已包含cache：**重验缺失/部分/显式零/cache读写/负值/溢出，禁止重复加缓存。不得仅因Option就删除现有Anthropic溢出sanitize。
- hosted replay、pause continuation、thinking/signature/redacted、第三方参数映射与最终wire预算。不能被新版自动行为悄悄多发请求。

**旧会话兼容是必做项：** `reasoning.rs` 的 `codex-rig-reasoning-v1:` 直接存Vec<Rig Reasoning>，0.44类型/序列化已经变化。保留旧版本独立DTO解码和有界迁移，旧rollout不就地改写；签名/redacted/encrypted块及endpoint/model/auth来源隔离不能丢。新版writer是否需要版本号升级要明确定义，不能默认把新类型写进旧v1冒称兼容。旧cassette也不得批量重录以隐藏变化。

至少保存并复验：三wire最终请求体、工具配对、终局/迟到usage/finish reason、输出cap、cancel、retry/idle/HTTP硬预算、代理/TLS、signed reasoning旧会话、resume/compact/token_usage、app-server env和live-tests离线。每个0.42 workaround删除都给等价回归，不用“上游已有”替代验证。

执行just bazel-lock-update，同阶段保存依赖和Bazel锁；必要的BUILD/compile_data更新一并完成。功能迁移按可编译阶段拆解，避免巨型一口气重写；产出独立Rig候选commit和收据。

### 阶段四：继续图片入口与累计预算

按图片交接文档第二、三批执行，但所有行号/生命周期以合main和选定Rig版本后的源码为准。主用模型vision能力与窗口须有证据，unknown metadata不冒充272K/vision实证。文件上传仍为独立后置选配，Rig升级不自动解决视觉context或Files API能力。

## 最终交付

分别报告四阶段commit/checkpoint、merge起点和目标SHA、冲突处理与无冲突语义适配、实际Rig发布版本与features、旧会话兼容证据、已执行测试身份/数量/首败、可用候选和未验限制。优先形成可用的Codex/Rig主链，不把所有上游功能都关闭/打开或全平台历史失败清零当作本轮前置。
