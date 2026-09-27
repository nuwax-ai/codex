# Rig Responses 独立复核与修复（2026-09-28）

审查基线：`test` 分支 `0f62ba844`（第一阶段）以及 Claude 未提交的第二阶段。此次一并检查传输链、历史投影、真实恢复、字段保真及测试证据。以下是当前修复结果；原批次记录保留为历史证据，不等于当前验收。

## 1. 发现与处理

1. **P1：Responses 丢失响应头及外围事件 metadata。** 原 Rig 泵只消费响应 bytes 并调用 item 转换器，没有完整处理模型、限流、ETag、reasoning 标志、验证与 moderation 等信息。现在复用 `codex-api/src/sse/responses.rs` 的完整解码器，在独立 `responses_policy.rs` 中选择严格终止策略；native 的既有宽松策略不变。严格路径额外读取标准 `response.model`，没有配额头时不发空配额更新。回归见 `responses_regression_tests.rs::responses_preserve_header_and_sse_metadata_events`。
2. **P1：HTTP 响应头到达前没有超时。** SSE idle timeout 不覆盖建流阶段。现在 `responses.rs::stream_responses_via_rig_inner` 以同一 idle timeout 包住 Rig `send_streaming`；服务端接收请求后不发送响应头时返回 `TransportError::Timeout`。没有新增重试层。
3. **P1：SDK 自动添加的 Bearer 改写鉴权。** api-key-only、无鉴权及非 Bearer 配置必须保留已解析请求头。现在构建请求后恢复准确的 Authorization 值；原来不存在就删除 SDK 生成值。wire 测试覆盖无鉴权、api-key-only、Basic + api-key，以及既有 refreshable auth。
4. **P1：Responses Lite 请求体与头不匹配。** Core 固定传 `false`，但请求体可能使用 Lite 的 additional_tools 形态。现在仅 Responses wire 传递模型的 Lite 标志，集成测试覆盖开关两种形态。
5. **P1：同一 turn 的服务端路由状态丢失。** 原桥接口只传 extra_headers，没有把响应的 `x-codex-turn-state` 写入同一请求链的 OnceLock。新增 `ModelBridgeOptions` 传递该状态；工具续轮回传，恢复后的新 turn 不携带旧状态。
6. **P1：顶层 SSE `error` 可被忽略并被后续 Completed 掩盖。** 官方 Responses 还定义了独立的 [ResponseErrorEvent](https://github.com/openai/openai-python/blob/main/src/openai/types/responses/response_error_event.py)。严格策略将其交给已有错误分类并立即停止；failed/incomplete 同样不能继续成功。异常帧、缺少关键字段、EOF 和 idle timeout 仍有覆盖。
7. **P2：测试没有证明原始字节保真。** Claude 的类型克隆修复方向正确；旧 JSON Value 等值断言无法抓住 key 顺序、RawValue 空白或超大整数损失。新增原始 HTTP body 整体比较，使用大于 u64 的 `18446744073709551617`，并断言原请求未修改。
8. **P2：跨协议历史形态与真实 Rig 输出不一致，serde 说明写反。** 原测试使用非空 summary 和 Text；真实输出是空 summary、ReasoningText、桥生成 item ID。`should_serialize_reasoning_content` 是 skip 谓词，含 ReasoningText 会序列化。已修正 wire/live 场景与 Spec/Plan，并核对官方 [Reasoning 输入类型](https://github.com/openai/openai-python/blob/main/src/openai/types/responses/response_reasoning_item_param.py)。在线场景从已有真实 Chat Rig 录制经当前转换器重建历史，要求 Responses 逐字返回只在配对工具结果中出现的标记。
9. **P2：离线 Responses replay 没有经过请求构建。** 原 replay 只解码 SSE，不能验证投影。现在 `live-tests/src/responses_replay.rs` 通过本地 HTTP 服务运行实际 Rig 路径，断言完整出站字节、请求不变及路径，再解码录制的 SSE。fixture.request 明确为投影前输入，不能冒充实际出站字节。
10. **P2：所谓 save/resume 只是在同一实例继续第二轮。** 已替换为实际工具调用、保存、shutdown、resume、换模型/provider 后续轮；验证工具配对、旧 provenance 保留、缺字段旧记录不回填、旧文件前缀不改写，以及元数据不发给模型。
11. **P2：无桥 feature 的错误断言和配置说明陈旧。** 修正 `client_bridge_tests.rs` 的 model bridge 文案；配置说明不再声称 URL 嗅探决定协议或 Responses 走 Chat。已生成配置 schema。三个 native 语义测试显式选 native，保持其原测试目标。扩展复测还发现 `client_websockets.rs::websocket_provider` 使用默认 Rig，导致 ws:// URL 进入 HTTP builder；共享 WebSocket mock 现显式选择 native。
12. **P2：模块和变更规模。** 第一阶段既有提交含 949 行生产变更和 1275 行测试变更，不能只以 fixtures 解释规模；不重写该提交。此次将新增 provenance 逻辑从已有巨型 session 模块机械移至 `session/model_history.rs`；严格策略、wire 回归、live replay 和跨协议场景分别独立成文件。提交按传输修复、core 契约、replay/真实证据及文档分组；生成 fixtures 单独计入规模。
13. **P2：反向兼容和测试数字夸大。** 新建 Chat 测试不能证明 Responses→Chat/Anthropic 的旧 reasoning 兼容；当前转换仍会忽略无同源 envelope 的公开 reasoning。已登记为后续范围。测试报告分开统计实际验证与运行时提前返回，不能把 nextest 的 passed 全数当有效厂商覆盖。

### 修复后的源码定位

以下行号对应本轮格式化后的文件；编号与上述发现一致。

| 编号 | 源码位置（仓库相对路径:行） |
|---|---|
| 1 | `codex-rs/codex-api/src/sse/responses.rs:53`、`:673`；`codex-rs/codex-rust-rig-bridge/tests/wire/responses_regression_tests.rs:152` |
| 2 | `codex-rs/codex-rust-rig-bridge/src/responses.rs:165`；`codex-rs/codex-rust-rig-bridge/tests/wire/responses_regression_tests.rs:251` |
| 3 | `codex-rs/codex-rust-rig-bridge/src/responses.rs:151`；`codex-rs/codex-rust-rig-bridge/tests/wire/responses_regression_tests.rs:31` |
| 4 | `codex-rs/core/src/client.rs:1726`；`codex-rs/core/tests/suite/rig_responses_bridge.rs:68` |
| 5 | `codex-rs/codex-api/src/bridge.rs:53`；`codex-rs/core/src/client.rs:3236`；`codex-rs/core/tests/suite/rig_responses_bridge.rs:127` |
| 6 | `codex-rs/codex-api/src/sse/responses_policy.rs:26`；`codex-rs/codex-rust-rig-bridge/tests/wire/responses_regression_tests.rs:286` |
| 7 | `codex-rs/codex-rust-rig-bridge/tests/wire/responses_regression_tests.rs:72` |
| 8 | `codex-rs/protocol/src/models.rs:1623`；`codex-rs/live-tests/tests/bridge_live/responses_cross_protocol.rs:15` |
| 9 | `codex-rs/live-tests/src/responses_replay.rs:23` |
| 10 | `codex-rs/core/tests/suite/rig_responses_bridge.rs:127` |
| 11 | `codex-rs/core/src/client_bridge_tests.rs:150`；`codex-rs/core/tests/suite/client_websockets.rs:2633`；`codex-rs/model-provider-info/src/lib.rs:223` |
| 12 | `codex-rs/core/src/session/model_history.rs:13`；`codex-rs/codex-api/src/sse/responses_policy.rs:13` |
| 13 | `codex-rs/codex-rust-rig-bridge/src/reasoning.rs:98`；`my-docs/rig-responses-phase2/spec.md:36`；`my-docs/rig-responses-phase2/tasks.md:119` |

## 2. 当前范围

第二阶段执行的是有限、单向请求投影：仅清除本桥 reasoning envelope，保留其余字段；`None` 按现有 serde 规则发为 `null`。不修改原始历史，不全量 JSON 往返。消息、工具调用/结果配对、公开 reasoning 和身份字段均采用原请求的序列化规则。

来源 metadata 尚未参与投影。非 envelope 密文仍原样发送，跨厂商可能被拒；synthetic item ID 也不保证任意厂商接受。当前真实跨协议验证的源是已有 Chat 录制，不等于重新在线请求了 Chat，也不证明 Anthropic→Responses 或完整双向兼容。

## 3. 本次验证

统一使用隔离 `CARGO_TARGET_DIR=/tmp/codex-review-280843-6rqdtknp/target`、仓库 `just test`（8 MiB 测试线程栈），不使用裸 cargo test 或 all-features。原始日志位于 `/tmp/rig-responses-review-20260928-*.log`，没有入库。

- 相关 6 crates：`just test -p codex-rust-rig-bridge -p codex-api -p codex-rust-genai-bridge -p codex-history -p codex-model-provider-info -p codex-config --offline --retries 0`：**739/739，12 个二进制，exit 0**。其中 Rig 62 个单元 + 42 个 wire = **104 项**。
- 配置 schema：`CARGO_NET_OFFLINE=true just write-config-schema`：**exit 0**。
- Core 默认/无桥 feature：错误文案、历史过滤、native 重试及五项响应头测试，**10/10，3 个二进制，exit 0**。
- Core rust-rig 扩展集：真实关闭恢复、Lite/turn-state、native 重试、MCP prewarm、完整 client_websockets 模块等，**67/67，1 个二进制，exit 0**。
- MiMo/GLM 真实 Chat 历史→Responses：**2/2，exit 0**；分别准确返回 `XPROTO_mimo_7d31a94e` 和 `XPROTO_glm_7d31a94e`。四个新 JSON/SSE fixtures 已重录。源 Chat 历史取自已有真实录制。
- MiMo/GLM 普通 Responses 文本和工具两轮：**4/4，exit 0**。合计两批在线 **6 场景、8 次 HTTP 请求**，均在 bridge_live 这 1 个二进制运行。
- 完整离线 live crate：**91 注册项执行通过，48 项实际执行断言，43 项运行时提前返回**，2 个二进制，exit 0。43 = 36 GenAI + 1 A/B diff + 3 仅在线鉴权场景 + 3 未配置端点的 Step Responses；有效用例是 39 个 Rig 场景和 9 个本地辅助/历史测试。
- 所有精确过滤表达式、run ID 和环境见 `my-docs/rig-responses-phase2/tasks.md`。
- 最终 scoped `just fix`：**exit 0，零告警**；`just fmt`、`git diff --check`：**exit 0**。测试全部先于最终 fix/fmt 执行。首次 Clippy 指出的测试辅助函数 `collapsible_match` 已做等价合并，最终检查确认零告警；按 AGENTS.md，fix/fmt 后未重跑测试。

最初桥验证为 93/104：完整解码器引入的空 quota 事件与旧顺序断言暴露了行为差异，另一个断言原先接受 SDK 大小写改写。已修正空 quota 处理、按事件类型查找 Created 及精确鉴权期待；随后 739 项全部通过。没有将首次失败直接归为环境噪音。

首次 Core rust-rig 扩展集为 22/25：新增真实恢复及 Lite/turn-state 均通过；两个旧 WebSocket mock 的默认桥设置已修正；另一个 MCP prewarm 测试缺少隔离 target 中的 `test_stdio_server`。单独构建辅助二进制后扩大复测，67/67 全部通过。这些首次失败没有计入通过数。

## 4. 未验收

Linux/Windows、远程 exec OS、任意 Responses 厂商、完整 workspace、跨厂商密文、反向 reasoning 投影均不由上述结果证明。用户已明确选择“本轮只做相关模块与真实请求验证，然后提交”，因此没有运行完整 workspace。

## 5. 提交分组

按依赖拆为七个本地提交：共享严格解码、Rig 传输与类型投影、wire 回归、来源记录模块提取、Core 恢复/路由测试、真实录制 fixtures、live replay 与审查文档。每个手写逻辑提交控制在 800 行以内；四个生成 JSON/SSE fixtures 的 Git 变更为 1285 行，单独提交以便审查。SSE 最后的空行是协议帧分隔符，原始录制不裁剪；`.gitattributes` 仅对 SSE fixtures 关闭 `blank-at-eof` 检查。既有第一阶段提交保持原样。本轮不推送远端。
