# Anthropic 线 hosted（服务端）工具翻译 — Spec

日期：2026-09-28　状态：已实现（证据见 tasks.md）

## 1. 背景

codex 以 Responses 形态声明 hosted 工具（如 `{"type":"web_search"}`）。Chat/Anthropic 转换管线原先直接丢弃这类工具（`hosted_tools_are_dropped` 固化该行为），导致 GLM 等 Anthropic 兼容网关上模型看不到 web_search，而 Responses 透传路径正常——用户实测报告的差异即源于此。

## 2. 需求（通用机制，非单工具特例）

未来各厂商会带更多自带服务端工具，实现必须是**翻译表**而非散点特判：

1. **请求侧**：任意 hosted 工具声明按协议查表翻译。Anthropic 条目：`web_search` → `{"type":"web_search_20250305","name":"web_search"}`；无对应物的工具保持丢弃 + 警告。Chat Completions 无 hosted 概念，行为不变。
2. **响应侧**：流中的 `server_tool_use` 块按工具名匹配族（`web_search*`，覆盖官方 `web_search` 与 GLM 网关 `web_search_prime`）映射为 codex `WebSearchCall` 条目（含 query，兼容 `query`/`search_query` 两种字段）；无 codex 对应物的服务端工具记日志跳过；厂商私有的结果块（Anthropic `web_search_tool_result`、GLM assistant 侧 `tool_result`）不产生条目。
3. 历史重放安全：`WebSearchCall` 在 chat 家族转换中本就跳过（internal/local 类），无需回放服务端块。

## 3. 关键技术事实

- rig-core 0.42 的公开流式面**不暴露** `server_tool_use`：适配器把它归入内部 `TextStart` 附加参数，帧数据"never exposed on any rig surface"（模块文档明示），聚合终态记录也不含内容块。
- 因此响应侧用 transport 层 SSE tee（复用 Responses 录制的三通机制）：Anthropic 线 tee 原始字节，流泵在转发 Completed 前解析出 web-search 块并注入 `WebSearchCall` Added/Done（保持在 Completed 之前）。
- GLM anthropic 网关已真实验证接受 `web_search_20250305`（非流式 probe 2026-09-28，返回 `server_tool_use` 名为 `web_search_prime` + `tool_result` 块）。

## 4. 非目标

- 不为未知 hosted 工具猜测服务端类型；不加 rig 上游补丁。
- 服务端工具用量计费字段、结果内容条目化、Chat 线 hosted 支持。
- 响应侧条目插入位置为回合末尾（文本条目之后），不在流内原位。

## 5. 验收

1. wire：出站 tools 同时含函数工具与翻译后的服务端工具；GLM 形态流（web_search_prime + search_query + 私有 tool_result）产出恰好一个完成的 WebSearchCall 与一条文本消息；未映射服务端工具（bash 族）被忽略且流正常完成。
2. 单元：翻译表、工具名识别（官方/网关）、query 双字段。
3. 真实：GLM anthropic 端点端到端搜索问答成功，rollout 记录 web_search_call。
