# Claude Code 后续任务：第二轮独立复审后（2026-10-06）

从仓库 `/Volumes/soddygo/git-workspace/fork-nuwax-codex` 的实际最新 HEAD/worktree 开始。审查起点为 8017fb76c；最新阶段提交与独立证据见 `other-computer-validation-results.md` 的 Codex 第二轮节。先读 AGENTS、Rig spec/plan/tasks 和修订后的 cap Spec/Plan，不退回旧 SHA，不 stash/reset/clean，不清理配置、SQLite、tmp 或 tracked `.snap.new`。

本轮实际保留的模型生产修复为：bridge 调已有 outbound ID preparation、default namespace 不进 flat fallback、Guardian 传递原 checkpoint envelope metadata。本地 compact 产出可见 summary，不存在本轮声称的 encrypted 漏戳；最近请求不能为未知旧 opaque 证明来源，client-wide 回填已删除。不得重新引入该兜底来求测试通过。

## 1. P1：失败清单逐项归因与真正配对 A/B

383 历史失败/超时仍未全部关闭。`deferred_executor_guardian` 在历史 ws5 中有 TRY1 timeout→TRY2 pass，说明既有不稳定；117 vs124 测试不是同集合配对 A/B。单跑通过也不证明负载是根因。

- 使用报告保存的完整历史失败清单，把每项标成 fork、upstream、fixture/build、已确认环境、平台、未归因或 flaky 待定位。区分内部 nextest exit 和外层命令链 exit，保存首轮失败，不合并不同 feature 图的总数。
- 优先 deferred guardian、mcp_optional_startup_grace、exec-server registration_retry、network-proxy、otel、install-context。若仅有时序关联，继续标未归因，不直接写“机器速度类、与本轮无关”。
- 配对基线不要动当前工作树：使用独立临时 checkout/worktree，选相同依赖、package/feature、精确测试集合、retries0/thread数、home/SQLite与环境；记录源码、二进制、负载与运行顺序。不要重复历史局部 stash 二分。
- 必要时在现有阶段边界增加观测，保持 deadline与断言。复现产品错误才做小修复；不得扩大超时、跳过用例或批量接受快照。
- 相关小批通过后再申请完整 workspace；获授权后显式排除 `exec_live`/`bridge_live` 并关闭自动 retries。CI workflow 已加此过滤，尚未 dispatch。

验收：逐项可追溯；同集合配对才能做回归归属结论；全部 workspace/跨平台仍未通过前，不称生产门禁完成。

## 2. P1：opaque 独立来源分支与 writer 生命周期

跨进程 exec 用例现在验证同 key也降级、可见内容保留、两个真实 POST及原 rollout前缀不改写。它的所有 cell 都同时改变进程随机 credential-instance，因此不能独立证明 query/credential/header/endpoint rotation 分支。

- 补同进程公共 Core/owner 运行路径：same-source 正例必须保留 opaque；只改变 credential、endpoint、未知/重复/空 query、headers 的各个负例分别降级。使用同一份复制 history，核对真实 model/path/query/auth，保留 visible 内容和旧文件字节。需要检查来源 snapshot，不把秘密加入 metadata。
- administrative 八组合只覆盖 archive/unarchive/delete 的 default/npm × daemon存在性 × group active/inactive。queue 的 install/remote/运行态、其他 install source与完整 corrupted/incomplete矩阵继续分别列出，不把一个测试函数数当 cell数。
- owner QueueList与CLI精确submission ID/content已覆盖；原 pgrep 前后快照已删除。要关闭“任何时候无第二 writer”，需真实 writer注册/锁持有生命周期证据，能检测短暂writer启动且不被系统其他测试影响。先定义观测契约，再实现最小公共路径回归。

验收：same-source 阳性控制和每个独立旋转负例均有非零执行；进程计数不能用请求凭据一致或两次采样替代。

## 3. P1：cap partial/usage 的产品兼容决策和分批实施

新的 `cap-partial-output-usage-done-{spec,plan}-2026-10-06.md` 已校正为未实施提案。先拿具体兼容决策过审，再按 Plan 第0–6步推进。

- 保留 Core Error→TurnComplete(error)、app-server turn/completed(status=failed)→exec turn.failed 的失败关闭。不得把通知名称含 completed 当作成功并删除，也不得合成桥层成功 Completed。
- 现有文本/reasoning delta及独立Error不落盘。必须新增有界 partial 保存/读取路径，并测 legacy/paginated/resume；只增加error字段无法保留截断片段。
- Responses cap usage 从 response.incomplete 的原帧获取。三协议有/无 usage均测；未知不是0，累计完整性与 app-server/exec 字段要明确 schema兼容方案，不能承诺全部零schema变化。
- 不执行截断工具，不改 cap不重采样语义；任何新上下文碎片遵守硬上限，>1k tokens人工复审，>10k禁止。
- Responses live触顶仍缺当前提交证据。先mock；真实厂商需当次授权、凭据和新binary/source receipt，限制场景与费用。

## 4. P2：长期门禁、跨平台与诊断工具

- Bazel gate的公共 just入口已修为bash；保留非零数量和三个具名wire测试通过要求，以及计数listener直至child完成后清空backlog的负控。新测试增多时更新数量依据，不能放宽到0。
- 本机只证明 macOS；Linux/Windows及CI尚未执行。行政Unix socket项须列平台边界；Windows原生Ctrl-C覆盖Retry-After、首SSE字节等待、流中等待，不能以Unix kill代替。
- Skills home隔离现在为 HostSkillsService 实例级路径注入，不能再全局改HOME；Bazel同进程/并行场景和Windows需实际跑。新增服务隔离及缓存失效测试不能替代平台验收。
- D4脚本现在只量stdio：spawn、首stderr字节、initialize成功roundtrip；stdio没有socket bind，输出明确N/A。要诊断原websocket/daemon冷启动，需要另加准确bind报告与相应RPC阶段，并比较冷/热、低/高负载。新脚本正常测到耗时不等于定位高载失败根因。
- Python codegen默认UV_PYTHON3.13仍尊重显式覆盖；未来工具链兼容变更要基于锁版本测试，不无理由升级依赖。

## 5. P0：D6仍只设计，不可关闭

新增 framing/count_tokens/tokenizer来源清单是待核候选，全部Unverified。JSON bytes不自动等于计费tokens，API名称或有限样本一致不构成Exact/证明上界。先核厂商官方计量单位、模型/版本、载荷覆盖和限制，登记可复核证明。

当前40,960-byte fail-fast继续作为legacy兼容边界；KeepWholeOrFail不实施前不改产品行为。不做部分签名截断、未知opaque补戳、DropOpaque/DropAll或历史改写。兼容裁决、全载荷边界测试及人工P0复审缺一项都保持开放。

## 每批交付

普通批<800、复杂批尽量<500 changed lines；依赖/注册/fixture/consumer一起保存。记录完整命令、feature/target、实际执行/skip/timeout、失败与复验、源码/二进制身份、脱敏wire事实。本轮报告的测试属于完整修复树，中间阶段commit没有被逐个独立测试。

先完成相关 tests，再scoped `just fix`、`just fmt`、`git diff --check`；之后不重跑tests，源码相同不构成豁免。保存tracked `.snap.new`原字节，不能用reset/clean处理。未来 commit/push、完整workspace、厂商调用、CI dispatch或发布依据下一轮用户授权；本任务文件不自动授权这些动作。
