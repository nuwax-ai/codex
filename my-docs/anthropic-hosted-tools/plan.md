# Anthropic 线 hosted 工具翻译 — Plan

## 1. 模块

新增 fork crate 内模块 `codex-rust-rig-bridge/src/hosted_tools.rs`（+ `hosted_tools_tests.rs`）：

- `anthropic_server_tool(&Value) -> Option<Value>`：请求侧翻译表。新工具 = 新表项。
- `is_web_search_server_use(name)` / `web_search_action(input)`：响应侧族匹配与动作提取（`query`/`search_query`）。
- `web_search_blocks_from_anthropic_sse(bytes) -> Vec<Value>`：SSE 帧走查器，组装 `server_tool_use`（content_block_start/input_json_delta/content_block_stop），只返回 web-search 族；畸形帧跳过（语义流此刻已完成校验）。
- `web_search_call_events(block) -> Vec<ResponseEvent>`：Added+Done 事件对。

## 2. 数据流

- `request_tools.rs`：hosted 工具（非 function/custom）原样收集进 `ToolMeta.hosted_tools`，不再丢弃即失。
- `stream.rs`：Anthropic 线构造时翻译表过滤得到 `RigHttpClient.anthropic_server_tools`；`transport.rs` 在既有 anthropic 请求体改写分支追加这些条目（服务端工具无函数 schema）。
- 响应侧：`transport.rs` 给 Anthropic 线挂 `anthropic_sse_tee`（与 Responses 录制同构的三通）；`stream.rs` 泵在 `pending.completed_emitted()` 后从 tee 解析块，把事件对插到 Completed 之前（基准位一次计算，防插入错位）。
- rig 公开流式面不暴露服务端块是设计事实，本方案不改 rig、不动归一化层。

## 3. 兼容与边界

- Responses 路径零改动（tee 字段为 None）；Chat 线 hosted 依旧丢弃。
- GLM 私有块（assistant 侧 `tool_result`）走 Unknown/空部件路径，已被忽略。
- tee 在 Final 处理时必然已含终帧（tee 写入在下游解析之前）。
