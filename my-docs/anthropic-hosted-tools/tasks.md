# Anthropic 线 hosted 工具翻译 — Tasks

统一使用隔离 `CARGO_TARGET_DIR=/tmp/codex-nuwax-home-target`、仓库根 `just test`。日期：2026-09-28。

## T1 实现

- [x] T1.1 `hosted_tools.rs` 翻译表 + SSE 走查器 + 事件构造（+ `hosted_tools_tests.rs`）
- [x] T1.2 `request_tools.rs`：hosted 工具收集进 `ToolMeta.hosted_tools`（不再丢弃即失）；`convert_request_tests.rs::hosted_tools_are_dropped` 扩展断言收集结果
- [x] T1.3 `transport.rs`：`anthropic_server_tools` 请求体追加 + `anthropic_sse_tee` 三通（Anthropic 分支，Responses 路径零改动）
- [x] T1.4 `stream.rs`：构造时翻译（仅 Anthropic）；泵在 Completed 前注入 `WebSearchCall`（基准位一次计算）
- [x] T1.5 `responses.rs`：新字段置空（None/空表）

## T2 测试

- [x] T2.1 定向全量：`just test -p codex-rust-rig-bridge --offline --retries 0` → **111 项：110 过 / 1 既有负载抖动**（`responses_time_out_when_server_never_sends_headers`，Responses 超时路径本改动零行为变更；隔离复跑 **3/3 通过**，两种失败形态均为该测试支撑代码自身竞态）。三个新 wire 用例全过：注入、GLM 形态映射（web_search_prime + search_query + 私有 tool_result → 恰好 1 个完成 WebSearchCall + 1 条文本）、未映射服务端工具忽略
- [x] T2.2 `just fmt`；`just fix -p codex-rust-rig-bridge -p codex-tui` → exit 0 零告警
- [ ] T2.3 真实 GLM 验证（待填：端到端搜索问答 + rollout 记录 web_search_call）

## T3 文档与提交

- [x] T3.1 `my-docs/anthropic-hosted-tools/{spec,plan,tasks}.md`
- [x] T3.2 `codex-review-prompt.md` §二登记（hosted 工具翻译在 fork crate 内，无上游面变化；登记 tui daemon 排除项）
- [ ] T3.3 提交：桥实现+测试、tui daemon 修复、文档三组；不自动 push（等用户确认）
