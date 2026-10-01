# R2+R3 Spec：hosted 搜索历史保真与暂停续接（规范文档）

日期：2026-10-01。来源：`codex-review-2026-10-01.md` R2（条目 4–6）与 R3（条目 7–9）。
R2 与 R3 共享同一持久化载体与投影路径，合并为一个 Spec/Plan/Tasks 簇。

## 问题陈述（做什么）

阶段 D 初版已持久化并回放搜索对，但存在以下缺陷（均已在审查中复现或定位）：

1. **来源隔离缺失（R2-4）**：`request_messages.rs` 只检查目标协议是否为
   Anthropic，不检查载荷来源。GLM 产生的原始块/密文可能被发往另一家
   Anthropic 兼容网关或另一模型。
2. **恢复侧硬上限不成立（R2-6）**：`convert_request.rs` 的 64 上限按
   assistant 分组计数；同一 assistant 的 65 对可通过。40,960 字节检查只在
   事件发射侧（`hosted_tools.rs`），旧 rollout / 导入载荷在请求构建侧
   无大小、数量、id/形状校验。bytes/4 是估算，不得称为精确 token 上限。
3. **暂停续接不是原样回传（R3-7）**：`assistant_continuation_items` 只重建
   文本与搜索对；thinking/signature、redacted_thinking、文本 citations、
   client tool_use 块及原始交错顺序丢失。
4. **跨响应配对缺失（R3-8）**：配对只遍历当前响应的 uses。混合
   server/client 轮中，下一响应只带回旧 server id 的结果时结果被丢弃、
   call 永远 in_progress。
5. **引用未保存（R3-9）**：SDK Text 的 citations 元数据在
   `convert_response.rs` 只读 `.text` 时丢弃；搜索 result 块的持久化不能
   恢复逐文本引用。

## 需求（验收边界）

- **S1 版本化载体**：`WebSearchCall.wire_blocks` 从裸数组升级为带版本的
  envelope：`{"version":1,"source":"<opaque>","blocks":[...],"cited_text":[...]}`。
  `source` 为凭据无关的身份（与 `client.rs::reasoning_source` 同一构造：
  协议 + sha256(base_url, query, model)），永不包含明文 endpoint、模型名或
  密钥。
- **S2 来源门禁**：回放（请求构建侧）比较 envelope.source 与当前请求的
  source；不一致、缺失、无法解析（含旧版裸数组）→ 保守降级：该对不回放
  （不猜测密文可移植性），call 条目本身保留在历史，不报错、不截断。
- **S3 请求侧硬上限**：按**真实对**计数（跨全部分组的 server_tool_use
  总数）≤ 64 对/请求，超限丢最旧；单对序列化字节 > 40,960 → 丢该对载荷
  保 call 条目；块形状校验（server_tool_use 需 id+name；result 的
  tool_use_id 必须匹配）。上限作用于**每次请求构建**，覆盖新建、加载、
  resume、fork、切 provider 全部路径。超限/降级后的 assistant 消息不得为
  空、不得留下悬空 call/result。
- **S4 暂停原样续接**：暂停续接请求中，被暂停 assistant 消息的 content
  数组必须与该次响应的原始块**深度相等**（含字段顺序语义下的全部块：
  text+citations、thinking+signature、redacted_thinking、tool_use、
  server_tool_use、web_search_tool_result/tool_result），增量
  （input_json/text/thinking/signature）到达终态。工具数组不变。
- **S5 跨响应配对**：按 server id 关联。本次响应未配对的 result，若
  请求输入中存在同 id 的 in_progress 调用，则发出**新的追加**完成条目
  （append-only，不改旧 rollout）；请求构建时同 id 的 in_progress 与
  completed 条目去重（投影级，保留 completed）。client tool_result 仍先于
  server 结果返回。
- **S6 引用持久化**：带 citations 的 text 块以 `cited_text` 条目随 envelope
  保存（文本+citations，按出现顺序）；回放时按原位置拼回 assistant
  content。core 用户可见面（TUI/协议）不新增 citations 表达——映射为
  已登记的后续工作。
- **S7 不变量**：追加历史不动旧 rollout；Responses/native 请求副本继续
  清空 wire_blocks（F09 语义不变）；`hosted_results_replay=false` 继续整体
  关闭回放（含旧格式）。

## 非目标

- 不做跨厂商密文翻译或"兼容性猜测"；未知来源一律降级。
- 不为 core/协议新增用户可见 citations/搜索结果字段（仅桥内载荷保真）。
- 不改 `reasoning_source` 的既有构造与 reasoning envelope 投影。
- 不重写已提交历史；旧 rollout 中的裸数组按"未知来源降级"处理。
