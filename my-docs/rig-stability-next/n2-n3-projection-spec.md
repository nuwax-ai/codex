# N2/N3 — 引用投影与混合轮历史前缀（Spec）

日期：2026-10-02。输入：`my-docs/codex-review-2026-10-02.md` N2/N3、
`rig-stability-next/spec.md`。审查坐标：`convert_response.rs:128`、
`transport.rs:346`、`request_messages.rs:191,239`。

## 问题

1. **N2 正文重复**：完成轮保存普通 `Message`（plain answer），同一文本又
   进 envelope `cited_text`；续轮投影得到 plain answer 与 cited answer 两份。
   wire 测试只手工回放 `WebSearchCall`、丢了实际 Message，掩盖了该路径。
2. **N2 位置**：引用文本应在对应正文位置以 cited 终态**替换**投影，而不是
   尾部追加。
3. **N3 前缀漂移**：`[client call, pending server call, client output, late
   result]` 收到完成条目后，投影删除旧 pending、把完整 call+result 搬到后面
   的 assistant —— 模型请求前缀改变。rollout 追加不等于上下文前缀稳定。
   现有混合测试手工重建请求，第三请求移除了 client call/output。

## 需求（验收边界）

- 稳定的 response/block 边界载体：引用归属按块位置确定。
- 普通正文在投影中**只出现一次**；cited 终态替换对应文本投影；保存的普通
  Message 不被改写（rollout 不变）。
- pending server call 保留原投影位置；迟到 result 追加在**其所属新响应**的
  位置；不因"去重成功"改变旧请求前缀。
- 已发送请求的 messages 前缀（逐条深比较连续请求）稳定。
- 同组多个合法 envelope 聚合后超限：超限行为沿用 N5 预算表（whole-drop 或
  明确失败），不得同时误降级未超限的对。
- 测试用真实事件累积与 Core 工具闭环；保留完整 Message、client call/output
  与 search history；覆盖多搜索、多文本、多次 pause、迟到结果、保存恢复、
  fork；深比较实际连续请求。

## 非目标

- 不改 rollout 磁盘格式中已写入的历史（不重写旧 rollout）。
- 不伪造签名/密文；不引入全量 Value 往返破坏 RawValue/精度。
- Chat 线（无 hosted 概念）行为不变。

## 设计决定（待 Plan 落实）

- **投影层去重**（N2）：投影时若同组 assistant 的 plain text 与某 cited
  block 文本相同（规范化比较），以 cited 终态**替换**该 text 块的位置，不再
  另追加 cited_text。
- **结果归属位置**（N3）：迟到 result 的 envelope 配对（call,result）在投影
  时按 result 的**响应位置**拆开：call 块留在原 assistant（原位置），result
  块放在其出现响应对应的 assistant content 尾部。去重只用于"同 id 完成对
  不得重放两次"，不再迁移 call。
- **前缀稳定性判定**：新增 wire 断言助手比较连续两次请求的 messages 前缀
  （第一个差异之前逐条相等），多轮场景全覆盖。
