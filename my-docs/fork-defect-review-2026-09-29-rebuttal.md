# fork 缺陷清单独立复核报告（2026-09-29）

复核者：Claude（独立读码验证，未改任何代码）。方法：逐条打开坐标读代码 + 引用
仓库既有测试与 live 日志作证据。结论分三类：**确认 / 推翻 / 存疑**。

## 总评

清单质量高：坐标全部真实、机制描述大体准确。**B 组 5 条"代码缺陷"里 0 条成立
为正确性缺陷**——B1/B5 多项实为"对齐官方 native 行为"，B3 的核心断言（错误体
被销毁、超窗分类永远不触发）被 live 证据直接推翻，B4/B5 其余为不可达或按设计。
真问题集中在 **A（文档）与 C（仓库卫生）**，外加清单自己没意识到的两个测试
缺口（见 §新发现）。

---

## A. 文档（全部确认）

### A1 — 确认 ✅ 文档缺陷
- `rg -rn responses_routes_via_chat_bridge --type rust` **零匹配**；该名只存在于
  FORK.md:50 与 codex-review-prompt.md:37（两处同源错名，旧计划文档也用过）。
- 真实符号：`stream_model_bridge`（core/src/client.rs）、`uses_model_bridge()`、
  `dispatch_model_bridge`。**两份文档都要改。**

### A2 — 确认 ✅ 值得补
- 已核实三个开放项至今未做：`cache_control` 在桥内零出现（grep 无匹配）；
  每轮新建 reqwest client（stream.rs:117 / responses.rs:96 每次 `http_client(&headers,..)`）；
  Anthropic 固定 `DEFAULT_ANTHROPIC_MAX_TOKENS`（client.rs:120）。
- FORK.md §9 是"范围说明"不是"已知局限"，建议补 §10 已知局限。

### A3 — 确认 ✅
- rig-bridge-implementation-plan.md:202/204 仍写 `ChatModelBridge`（真实是
  `ModelBridge`/`ModelWireProtocol`，桥内 URL 嗅探也早已移除）。旧文档需盖
  "已被 FORK.md 取代"戳。

## B. 代码缺陷

### B1 — 推翻（②对齐官方，非 fork 缺陷）
- **native Responses 对 incomplete 同样整轮报错**：`codex-api/src/sse/responses.rs:512-521`
  —— `"response.incomplete"` → `Err(ApiError::Stream("Incomplete response returned, reason: {reason}"))`，
  不返回截断补全。桥的 `FinishReason::Length → Err`（convert_response.rs）与此一致，
  且该对齐**已被桥自己的 wire 测试固化**：`responses_regression_tests.rs:296-310`
  明确断言 incomplete → `Stream("Incomplete response returned, reason: max_output_tokens")`。
- 残留的只是**产品局限**（16384 固定上限对长输出/重思考模型偏小）——审计 backlog
  早有登记，与"整轮报错 vs 返回截断"的正确性定性无关。复核者自己的开放问题 1
  正是答案。

### B2 — 大部分推翻（②按设计；"网关 400"不成立；测试缺口成立）
- 丢弃 `WebSearchCall` 历史是**协议翻译的唯一稳妥解**：OpenAI 形态的
  web_search_call 条目在 Chat/Anthropic 线上不合法；忠实回放需伪造
  server_tool_use/tool_result 块——块 id 是网关签发的，伪造比丢弃风险更大。
- **"配对校验可能 400"不成立**：历史里 server_tool_use 和 tool_result **都不发**
  （request_messages.rs:207-219 整类跳过），不存在孤儿块；回放的 assistant
  文本（含引用）保留了检索结论。
- 成立的部分：多轮 web_search 的 live 覆盖确实缺失；"检索原文上下文丢失"是
  翻译固有损耗，应记入 FORK.md §4 已知局限而非代码缺陷。

### B3 — 核心断言推翻（证据充分）；残留一个测试缺口
**"错误体被 with_terminal_check 销毁 → 超窗分类永远不触发"不成立**，链路逐环：

1. **非 2xx 到不了 body 包装层**：transport.rs 中
   `HttpClientExt::send_streaming(...).await.map_err(sanitize_error)?` ——rig 对
   非 2xx 返回 `Err(InvalidStatusCodeWithDetails{status, headers, body})`（body
   由 rig 读好放进错误），`?` 处短路；`response.map(|body| ...)` 里的
   with_terminal_check 只作用于 **2xx** 响应体（Anthropic stream=true 的 2xx 必为 SSE）。
2. **live 实锤错误体完整存活**（Anthropic 线、真网关）：
   `ProviderResponseError: status 400 Bad Request: {"error":{"message":"The input you provided is invalid","type":"input_invalid"}}`
   —— Step 网关 400 的 body 原文穿透 rig 到达桥（本机 live 日志，2026-09-29）。
3. **桥保留 body 进分类器**：stream.rs map_completion_error 把
   `e.provider_response_body()` 放进 `TransportError::Http{body}`；
   api_bridge.rs 的超窗分类读的正是这个 body（分类器 22 个单测全绿）。
4. **sanitize_error 无损**：只在 rig 自己读体失败（body 以
   "failed to read error response body:" 开头）时重写——那时本来就没有可丢的
   body，重写是为去 URL 泄漏。
5. **分类→trim 回路已有测试**：`core/tests/suite/compact.rs::
   auto_compact_recovers_from_http_context_window_rejection_by_trimming`
   （400 拒绝一次 → 重试输入严格变少 → 回合完成）+ Responses 桥线 wire 断言
   `[ApiError::ContextWindowExceeded]`（responses_regression_tests.rs:305）。

**清单说对的残留点（转为测试缺口，见新发现 N1）**：上述 core 集成测试走的是
native 路径（测试 provider 克隆自 built-in openai，`is_first_party()`=true →
`uses_model_bridge()`=false）；Chat/Anthropic **桥线**的"400→分类→trim"端到端
没有专门集成测试（各环节分别有测试，链路无整体覆盖）。另外"200+非 SSE JSON
错误体"这种病态网关行为在两条线上都会 EOF 报错丢体——记为已知边界即可。

### B4 — 推翻（不可达路径）
- `request_tools.rs:41-47`：`tools` 是 `Vec<响应结构>`，`to_value` 必产 Array，
  `from_value::<Vec<Value>>` 对 Array 必成功——`unwrap_or_default` 实际不可达，
  "静默零工具发出"不会发生。至多值得一个 `debug_assert!`，非缺陷。

### B5 — 逐项
| 项 | 结论 | 依据 |
|---|---|---|
| finish_reason 精确串匹配 | **推翻（对齐官方）** | native 对**任意** incomplete reason 都报错（responses.rs:512-521）；桥对任意非 Stop/ToolCalls 也报错，语义一致，wire 测试固化 |
| `response_id.unwrap_or_default()` 空串 | **推翻（nit）** | 下游只作展示/关联用；网关异常时无 id 可用，空串是既定容错 |
| reasoning.rs 首 part 非 Text 丢 delta | **存疑（需专项）** | 未深读 genai/reasoning 全路径；登记待查 |
| tee 无界 Vec | **半确认（P3 优化）** | 每请求一块、随流结束释放（Arc 双持随 client/pump drop），**非泄漏**；但可在无 hosted 工具时跳过 tee 省一次全量拷贝 |
| map_tool_choice 未知串→Specific | **推翻（按设计）** | codex 的 tool_choice 语义本就是 `auto/none/required/<工具名>`——未知串**就是**工具名；native 行为相同 |
| hosted 解析器块索引校验 | **推翻（nit）** | 宽松解析器，畸形帧跳过是设计（语义流已校验过响应）；可加 index 一致性 debug 断言 |

## C. 仓库卫生

### C1 — 确认 ✅ 应删顶层副本（可安全删除）
- `/usr/bin/diff` 实测：顶层 330 行是**早期原型快照**（旧版 PendingRigMessage：
  HashMap 工具表、text_item_added 标志位等旧设计），真实 crate 295 行是重构后
  版本（PendingTools 独立模块 + 本轮 hosted tools 改动）。**差异是"顶层落后"，
  不是"真实桥缺代码"**——清单担心的方向反了。
- 引用面核查：workspace member 指向 `codex-rs/codex-rust-rig-bridge`
  （codex-rs/Cargo.toml:147/182）；justfile / MODULE.bazel / workflows 均无顶层
  路径引用。**直接删 `codex-rust-rig-bridge/`（顶层目录）安全。**
- 复核插曲：shell 里 `diff` 被别名劫持会静默吞输出（本轮曾两次显示"无差异"），
  复核用 `/usr/bin/diff`——这也解释了清单作者可能的困惑来源之一。

## D. 审计开放项交叉核对

全部属实：cache_control 缺失 ✓、每轮新 client ✓、固定 max_tokens ✓、
parallel-tool 断言弱 ✓、真实 401 重登录未测 ✓、跨轮 web_search 未测 ✓（=B2）、
`claude-rig-full-validation.md` 确未产出（全 workspace 测试从未跑过）✓。

## 新发现（清单未提到的类别）

- **N1（测试缺口，B3 的残留）**：桥线（chat/anthropic wire）缺"HTTP 400 超窗
  body → ContextWindowExceeded → 压缩 trim 重试"的**端到端**集成测试（现有
  core 集成测试走 native 路径，桥的 map_completion_error 保体仅有 live 旁证）。
  建议在桥 wire 测试加：serve 400（context_length_exceeded body）→ 断言
  ApiError 分类；或 core 集成测试用 wire_api="chat" + mock 400 序列。
- **并发/锁**：无问题——tee 是 std::Mutex 短临界区，泵在 await 前 clone 出
  字节释放锁（刻意修过）；无跨 await 持锁。
- **feature-gate**：无问题——core 无桥 feature 时 `uses_model_bridge()`=false
  回落 native，相关 suite 测试已显式 `experimental_bridge="native"`。
- **Bazel**：无问题——`defs.bzl` glob `src/**/*.rs` 自动收录新文件；wire 测试
  经 `#[path]` 注册。
- **app-server v2**：fork 未加任何 API 面。
- **rollout 恢复兼容**：fork 写入的 `web_search_call` 条目是官方原生 item 类型，
  官方 codex 可读；压缩记录（compacted）结构上游自带。无兼容性风险。

## 修复批次建议（供决策，未动代码）

1. **P2 文档批**：A1 两处错名改正；FORK.md 补 §10 已知局限（固定 max_tokens、
   cache_control、每轮 client、跨轮 web_search 上下文丢失、200+非 SSE 病态网关）；
   旧计划文档盖废止戳；删顶层 `codex-rust-rig-bridge/`（C1）。
2. **P3 代码批（全是小优化，无正确性）**：tee 在无 hosted 工具时跳过；
   request_tools 加 debug_assert；reasoning.rs 存疑项专项核查。
3. **P2 测试批**：N1 桥线超窗端到端；跨轮 web_search live 场景（B2 残留）。
