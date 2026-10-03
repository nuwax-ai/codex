# N2/N3 — 响应归属与有序重放（Spec）

2026-10-03 第四轮。目标：根治第三轮 v2 的六个缺陷；v2 的 concat 相等只能验证正文，不能证明响应归属。此前 R07 通过记录不作为第四轮验收。

## 需求

- 每个捕获响应有独立稳定响应 ID；每个 Message、Reasoning、客户端工具有稳定消息段 ID，持久化后进入 prompt，resume/fork 仍可关联。
- 所有 v3 sibling envelope 都声明其响应，完整 layout 缺失或超限时不得继承较早响应的布局。
- 引用只能注入指定消息段。正文相等用于验证已经命名的消息段，不能寻找、猜测或领取归属。指定 owner 不存在时保留普通历史，降级 pair-only，不补入正文。
- 搜索 call/result 各出现一次，并保持各自原响应位置。pair-only 和 text-only pause 均是明确边界；迟到结果不能排在原 call 前，不能改变原请求前缀。
- 按原始 content-block 顺序恢复正文、引用、搜索、thinking/redacted thinking 与客户端工具。思考签名、密文和客户端 call_id 保留，不跨消息段拼正文。
- 原始索引严格递增且唯一；载体与 sibling 的 pair 索引完整覆盖先校验，去重/预算过滤后再次校验完整覆盖。损坏载体不得复制 call 或遗漏接受的 pair。
- 独立限制 pair 与 layout 的数目和字节贡献；已去重或全部预算删除的 pair 不得留下任意数量的 layout/text。超限完整 layout 降级为同响应 pair-only。
- v1/v2 兼容仅作为 pair-only：历史条目位置提供保守边界，不推断正文归属。未知版本、跨 source、损坏 envelope 删除重放 payload，保留普通历史。

## v3 载体

响应 ID 是桥接层每次 attempt 生成的稳定 nonce；已有 ResponseItem.id 持久化 `rigseg_<response_id>_<ordinal>`，不扩展 Responses passthrough 元数据或 app-server API。

```json
{"version":3,"source":"same-source-hash","response_id":"nonce",
 "blocks":["raw call/result"],"block_indices":[1,2],
 "layout":[
  {"kind":"text","index":0,"owner":"rigseg_nonce_0","block":{"type":"text","text":"intro"}},
  {"kind":"pair","index":1},{"kind":"pair","index":2},
  {"kind":"segment","index":3,"owner":"rigseg_nonce_1","part":0},
  {"kind":"cited","index":4,"owner":"rigseg_nonce_2","block":{"type":"text","text":"answer","citations":[]}}]}
```

- `layout` 可只在首个 pair envelope 上保存；`response_id` 和 `block_indices` 在每个 sibling 上必需。
- `segment` 引用对应 Reasoning/client-tool 消息段的第几个现存投影块，不复制签名/工具参数。
- 空 text 块不生成公开 Message，也不声明 owner。
- 迟到 result 的原 call 克隆使用 `u64::MAX` foreign index；去重保留 call 原位置，只把 result 放入新响应。

## 预算与人工审查

- 每 pair envelope / 单 layout：序列化 9800 字节 whole-drop / pair-only 降级，预留 framing 空间；每请求最多 64 个接受搜索对。
- 完整 layout 独立最多 64 个、合计 65536 序列化字节。每个已接受块最多对应一个响应边界；无接受块的响应/layout 被移除。
- **P0 人工审查**：新 layout/ciphertext 条目可跨 1k tokens，使用 >=1000 字节的保守审查触发并记录。不能把字节或 bytes/4 宣称为任意 provider tokenizer 的精确 token 数；未知 tokenizer 下只有保守字节 fallback。最终模型上下文预算由请求发送路径整体 gate 承担。
- 不截断签名或密文。RawValue/未知 vendor 字段与数值精度原样保留，预算不改写普通历史。

## 验收矩阵

必须同时具备：六个缺陷的 unit/wire 回归；真实 Core pause → for_prompt → HTTP；相同正文归属与 call-before-result；保存/恢复/复制 fork 的完整请求前缀比较；source/provider gate；旧版本/损坏/载体超限/去重后的 whole-drop 证据。

实现完成和本地验证通过是不同状态；本轮最终命令、退出码、执行计数由统一验证记录登记。Linux/Windows、远端 CI、发布验收另列，不以本地 mock 替代。
