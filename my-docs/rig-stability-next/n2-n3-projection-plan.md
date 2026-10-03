# N2/N3 — Plan

2026-10-03 第四轮，替代 v2 的文本归属启发式。

1. `convert_response.rs` 为每 attempt 生成响应 nonce，Message/Reasoning/客户端工具开始时分配有序 `rigseg` ID。Added/Done/工具输入 delta 同 ID；客户端 call_id 不变。
2. `hosted_replay.rs` 从 SSE 完整恢复带索引的全部块，`hosted_tools.rs` 根据本次实际发出的有序消息段建立 owner/part 映射；思考与工具边界纳入 layout。无法映射时只保存明确响应 ID 的 pair。
3. 每个 v3 envelope 持有响应 ID、平行索引。完整 envelope 超限先删除 layout，保留有效 pair 的响应边界；pair 自身超限删除 payload，完成状态保留。
4. `request_messages.rs` 读取持久化消息段 ID，把每段在 pinned Rig 0.42 Anthropic 序列化内容中的范围传给 ReplayGroup。精确计数：非空 text 1、Reasoning content.len、客户端工具 1；SDK server anchors 已由 transport 移除，不计入范围。
5. 相同响应按显式 response_id 合并；在去重前验证所有 sibling 索引对 layout 的完整覆盖和冲突。v1/v2 使用历史条目自身的明确插入边界，只有 pair-only 能力。
6. `sanitize_for_request` 全请求去重并整对预算，保留 call/result 原群组位置；移除未贡献接受块的响应/layout，过滤 layout pair 条目并复验完整覆盖；独立限制完整 layout 数目和全部序列化字节。
7. `transport_identity.rs` 只使用命名 owner 对应的范围。验证每段正文、思考/tool 全部 parts、段顺序和 pair 覆盖；重建响应本身。owner 缺失、正文被编辑、范围不连续、载体缺失等均在该响应边界 pair-only，普通历史不改写、不补正文。
8. 统一验证：先 bridge 项目测试，再 Core Anthropic 闭环回归；主代理集中处理格式、scoped fix 与证据，子工作流不并行启动构建。

## 破坏面

无协议类型/schema 新字段；ResponseItem.id 的值有新的前缀，保存与 prompt 归属依赖该 ID。原生 Responses/Chat 不接收 hosted v3 投影。v1/v2 存量 envelope 的引用不再注入，pair-only 保守降级，不迁移旧 rollout。source gate 仍在构建 SDK anchor 前执行。

## 验证产物

- Unit：命名 owner 的相同正文、pair-only 早期响应、missing carrier、thinking/tool 全部 parts、损坏索引、缺 owner、去重/独立 layout 预算。
- Wire：真实 pause event → 序列化/反序列化 → 请求；超限首 carrier 后 siblings；签名/工具边界；损坏覆盖；100 个去重 carrier 不注入额外引用。
- Core：真实 for_prompt、两种 pause、保存/恢复与复制 fork 的完整请求前缀深比较；既有 cited/mixed/interruption 继续执行。
