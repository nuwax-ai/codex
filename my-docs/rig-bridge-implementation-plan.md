# codex-rust-rig-bridge 实施方案(Plan 层文档)

> 目标:新增平行模块 `codex-rust-rig-bridge`,用 rig(rig-core)替代 genai 承载
> `wire_api = "chat"` 的模型请求;现有 `codex-rust-genai-bridge` 保持不动,
> 两桥共存可 A/B,验证后切换默认,genai 桥保留作回滚。
>
> 关联文档:`openai-responses-chat-bridge.md`(协议映射参考)、
> `rust-genai-integration-analysis.md`(genai 集成分析,本方案为其 rig 版)。

## 1. 背景与决策依据(调研结论)

| 维度 | genai 0.6.5(现状) | rig-core 0.42.0(目标) |
|---|---|---|
| 维护 | 单维护者,稳定版停滞近一年(0.7.0-beta.24 尚未转正) | 团队维护,提交到当天,PR #2586 |
| DeepSeek 兼容 | 需我们在 bridge 手工处理 | `finalize_deepseek` 库级处理(string content、工具轮空 content、tool_calls index 回传) |
| reasoning 回传 | ContentPart::ReasoningContent 手工注入 | `Reasoning` 一等公民 + provenance 门控(跨 provider 自动剥离) |
| usage 归一 | 手工映射 | DeepSeek `prompt_cache_hit_tokens` 等库级归一 |
| provider | 27+ adapter | 26 原生 provider + 23 OpenAI 方言(含 deepseek/ollama 原生/openrouter/xiaomimimo) |
| API 稳定性 | 差但慢(变更少) | **很差且快**(0.42 后 5 周 44 个 breaking)→ 必须锁版本 + 模块隔离 |

结论:**能力全项覆盖且优于 genai,代价是 API 漂移,用"锁死版本 + 单 crate 隔离"对冲**。

## 2. 版本与依赖策略

- `rig-core = "=0.42.0"`(**精确锁定**,不追 main;升级 = 只改这一个 crate)
- rig-reqwest 未发布(carry-over:0.42.0 自带 reqwest 0.13 传输)→ **接受 workspace 内
  reqwest 0.12(codex)/0.13(rig)双版本共存**;两者不跨边界传类型,无冲突
- 后续优化(非本期):对 v0.43+ 的传输抽象(`HttpClientExt`)实现 codex http-client 传输,
  消掉 reqwest 0.13;在此之前不动 workspace 的 reqwest 0.12

## 3. 模块设计

```
codex-rs/codex-rust-rig-bridge/
├── Cargo.toml        # rig-core = "=0.42.0";依赖 codex-api/protocol/tools(与 genai 桥同构)
└── src/
    ├── lib.rs        # 唯一公开入口 stream_via_rig
    ├── client.rs     # codex Provider/auth → rig Client(openai chat 路由 / anthropic 路由)
    ├── convert_request.rs   # ResponsesApiRequest → rig CompletionRequest
    ├── convert_response.rs  # rig StreamEvent → codex ResponseEvent
    └── stream.rs     # 流泵:idle 超时、错误映射、ResponseStream 装配
```

公共 API(与 genai 桥同形,core 侧接入点对称):

```rust
pub async fn stream_via_rig(
    request: &ResponsesApiRequest,
    api_provider: &Provider,          // codex-api Provider(base_url/headers)
    api_auth: &SharedAuthProvider,    // Bearer token 来源
    extra_headers: HeaderMap,
    idle_timeout: Duration,
) -> Result<ResponseStream, ApiError>;
```

设计约束(继承 genai 桥的合并友好原则):
- **不依赖 codex-core**(方向:core → bridge)
- 协议类型(ResponseItem/ResponseEvent)仍是唯一边界,codex 其余模块零感知
- AdapterKind 概念内化:bridge 内按 base_url 判定(含 `/anthropic` → rig Anthropic
  provider,需设 max_tokens 默认值;否则 OpenAI chat 路由)

### 3.1 请求映射(要点)

| codex 侧 | rig 侧 | 备注 |
|---|---|---|
| instructions | `Message::System` 首条(chat 路由对 instructions 兼容性最好) | |
| Message{role,content} | User/Assistant + Text/Image content | assistant 文本+ToolCall+Reasoning 同消息混排(rig 原生支持) |
| Reasoning{encrypted_content} | `AssistantContent::Reasoning` | DeepSeek 回传由 rig 的 provenance 规则处理 |
| FunctionCall | `AssistantContent::ToolCall`(call_id 映射) | |
| FunctionCallOutput | `UserContent::ToolResult` | |
| namespace 工具展平 | `ToolDefinition{name: mcp__ns__tool,...}` | 沿用 flat_name_index 回环 |
| reasoning.effort / service_tier / parallel_tool_calls / verbosity | `additional_params`(裸 JSON 注入) | rig chat 路由未类型化这些字段 |
| text.format | `output_schema` | rig 恒 strict(顺带解决 P0-4) |

### 3.2 响应映射(要点)

| rig StreamEvent | codex ResponseEvent |
|---|---|
| Delta::Text | OutputTextDelta |
| Delta::Reasoning | ReasoningContentDelta(自增 content_index) |
| Delta::ToolName/ToolArguments | OutputItemAdded(FunctionCall) + ToolCallInputDelta(前缀 diff 逻辑保留) |
| BlockClose::Reasoning/Text | OutputItemDone(Reasoning/Message) |
| StreamFinal{usage,finish_reason,response_id} | Completed{token_usage, end_turn = finish≠tool_use} |

## 4. core 接线(最小侵入)

1. `core/Cargo.toml`:新增可选依赖 + feature `rust-rig = ["dep:codex-rust-rig-bridge"]`
2. `model-provider-info`:`ModelProviderInfo` 增加字段
   `experimental_bridge: Option<BridgeKind>`(`"genai" | "rig"`,默认 None→genai);
   配置示例:
   ```toml
   [model_providers.mimo]
   base_url = "https://token-plan-cn.xiaomimimo.com/v1"
   wire_api = "chat"
   experimental_bridge = "rig"
   ```
3. `core/src/client.rs` `stream_chat_api` 循环内按 `experimental_bridge` 分派。
   **2026-09-24 更新:默认桥已切换为 rig**(`ChatBridge::default = Rig`),
   即不配置 `experimental_bridge` 的 `wire_api = "chat"` provider 走 rig;
   配置 `"genai"` 显式回退旧桥。E2E `e2e_chat_default_bridge_is_rig`
   通过 RUST_LOG 的 dispatch 日志断言验证了默认路径确实走 rig。
4. cli 默认同时启用 `rust-genai` + `rust-rig`(单二进制可 A/B)

## 5. 测试方案(复用既有 live 体系)

| 层 | 内容 | 完成标准 |
|---|---|---|
| 单元 | 移植 genai 桥关键用例:reasoning 回传注入、FunctionCall 合并进 assistant 消息、namespace 展平、usage 映射 | cargo nextest 全绿 |
| L1 live(bridge) | `tests/live_mimo.rs` 同款四用例(chat/effort/tool 两轮/anthropic),走 `stream_via_rig` | 4/4,与 genai 桥同 base_url 可对比事件流 |
| L3 live(exec) | exec E2E 增加 `experimental_bridge = "rig"` 变体(chat+anthropic 两协议 × 两桥 = 4 组 A/B) | marker 闭环断言全过,JSONL 落盘 |

## 6. 实施任务分解(状态:2026-09-24 首版全部完成,T1-T7 已交付)

| # | 任务 | 完成标准 |
|---|---|---|
| T1 | crate 脚手架 + workspace 依赖(`=0.42.0`)+ 空实现编译通过 | cargo check 绿 |
| T2 | convert_request.rs 全量映射 + 单测 | 单测绿 |
| T3 | client.rs + stream.rs(OpenAI chat 路由先通) | L1 chat 用例过 |
| T4 | convert_response.rs(增量/done/completed 语义与 genai 桥对齐) | L1 tool 两轮用例过 |
| T5 | anthropic 路由 + max_tokens 默认 | L1 anthropic 用例过 |
| T6 | core 接线(feature + 配置字段 + 分派)+ exec E2E rig 变体 | 三协议 E2E 全绿 |
| T7 | 文档更新(my-docs)+ git commit | — |

## 7. 风险与回滚

- **rig API 漂移**:锁 `=0.42.0`;升级时只动本 crate + 本文档追加迁移笔记
- **双 reqwest**:体积/编译时间上升;不影响正确性(类型不跨界);T6 后评估是否提前做原生传输
- **行为差异**:以 L1/L3 live 测试为验收基准,genai 桥结果为对照
- **回滚**:`experimental_bridge` 默认 genai,不配置即回到现状;极端情况 feature 关掉 rig 即编译排除
