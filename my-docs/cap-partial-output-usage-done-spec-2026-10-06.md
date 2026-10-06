# cap 终止时 partial output / usage / Done 的产品契约（Spec，2026-10-06）

状态：设计提案，未实施。对应交接 P1 第 4 项。基线行为见 D3：三协议 cap 终止——已流出的 partial deltas 保留、恰一 `Error`、零桥层成功 `Completed`、无最终成功 agent message、截断工具不执行、零重采样。cap usage 透传与截断 partial 的持久化仍需实现；失败 turn 的关闭事件已经存在，不能将其当作成功完成而删除。

## 术语

- **cap 终止（capped terminal）**：厂商按请求 cap（`max_tokens`/`max_completion_tokens`/`max_output_tokens`）截断并返回 length/max_tokens 终止语义。
- **partial output**：cap 终止前已流出的 assistant 文本/reasoning 片段。已完整接收的 item 与尚未完成的片段需分别标识；截断工具参数不得执行。
- **桥层完成**：`ResponseEvent::Completed` 表示上游响应完成；cap 路径不得为取得 usage 或收尾而合成该成功事件。item 的 Done 与 turn 成功是不同层级，不能混称。
- **Core 失败终局**：一个 `EventMsg::Error`，随后一个携带 error 的 `EventMsg::TurnComplete`。后者关闭失败 turn，不表示成功。
- **app-server 失败终局**：`turn/completed` 通知携带 `turn.status=failed` 和同源 error；exec 消费该通知后输出 `turn.failed` 并结束等待。

## 契约（拟）

1. **单一失败状态**：最后一个 partial delta 在失败终局之前。桥层不合成成功 `Completed`；Core 保留一次 `Error` 和一次 `TurnComplete(error)`；app-server 保留一次 `turn/completed(status=failed)`；exec 保留一次 `turn.failed`。这些是同一失败在各层的通知与收尾，不是多个成功/失败终局。关闭该 turn 后不得再发它的 partial delta。
2. **partial 保留面**：
   - 流事件：已发出的 delta 原样保留（不可撤回）。
   - exec `--json`：保留已产生的 item 事件；终局为 `turn.failed`（error.message 含 Output token limit），不出现成功 `turn.completed`。让截断文本/reasoning 可观察所需的 partial item 映射仍需新增，并明确失败/截断标识。
   - rollout：**现状不持久化文本/reasoning delta 或独立 `Error` 事件**（`codex-rs/rollout/src/policy.rs`）；失败关闭由持久化的 `TurnComplete(error)` 表达。需新增有界 partial 累计/部分 item 的持久化路径，定义 legacy/paginated 历史和 resume 重建行为，不能只依赖流事件存在。
   - app-server：保留已产生的 item 更新，以及 `turn/completed(status=failed)` 和同源 error；新增 partial 表示须与 exec、rollout 的映射一致。
3. **usage 语义**：截断帧若提供 usage，应在映射失败之前提取，并向各统计面传递厂商已报告的数值；未提供则为**未知**，不得填 0。Responses 来源是 `response.incomplete.response.usage`，不是成功 `response.completed`。已知线程累计数值可保留，但本次未知使累计不再完整；采用 null、完整性状态或其他表示须先作兼容决策。
4. **成功与收尾的边界**：capped turn 不生成最终成功 agent message，也不把截断片段伪装成完整 item。保留 Core/app-server 的失败关闭及 rollout 的 `TurnComplete(error)`；未来持久化的 partial 必须能与成功完成项区分。这里不假定存在名为 Done 的 rollout 记录。
5. **兼容决策**：不得预先承诺零 schema 变更。现有 exec `TurnFailedEvent` 只有 error，app-server 的线程 usage `total/last` 是必填数字对象，不能直接表达上述累计不完整状态。复用现有 optional usage 通知是否足够、是否扩展失败 usage/partial item/累计完整性字段，须分别决定。exec JSON 类型与 SDK 导出、app-server v2 camelCase/TS/schema fixtures、rollout 序列化与旧历史读取都要评估；新增可选字段也须验证旧客户端兼容，不能假定无感。
6. **禁止**：为统计合成桥层成功 `Completed`；猜 usage；执行截断的工具调用；失败 turn 关闭后继续发其 delta；删除失败收尾以满足“无成功完成”。

## 非目标

- 不改变 cap 触发与不重采样语义（已有 D3 覆盖）。
- 不处理 cap 之外的失败类（断流/取消）——它们各自的 partial 契约另行裁决。
- Responses 协议 live 触顶验收与真实厂商行为差异另列（见 Plan 第 6 步）。

## 验收（实现时）

- Core 公共路径（test_codex）：三协议 capped turn 断言——恰好一个 `Error`，随后一个携带同源 error 的 `TurnComplete`；无桥层成功 `Completed`、无最终成功 agent message；partial delta 保留；有 usage 帧时透传、无则保持未知。
- exec `--json` 事件序列快照：partial 表示在 `turn.failed` 前，无成功 `turn.completed`；消费失败关闭后进程退出。
- rollout：新增 partial 路径确实落盘，`TurnComplete(error)` 保留；legacy/paginated 的读取和 resume 保留可见片段、失败状态，不执行截断工具、不将片段升级为成功结果。
- app-server：失败 error 与 `turn/completed(status=failed)` 均有且关闭通知恰一次，partial 表示在关闭前；usage/unknown 的 schema 与旧客户端兼容按已选方案验收。
