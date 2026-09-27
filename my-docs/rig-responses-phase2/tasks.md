# Rig Responses 第二阶段 — Tasks（执行记录）

> 以下批次与“自审修复”保留 Claude 原始历史记录，**不是当前完整验收结论**。其中旧 Value 投影、ReasoningText 序列化方向、91 项有效 replay 与反向兼容的说法已在本轮纠正；以当前 Spec/Plan 和末尾 Codex 复核证据为准。

## 批次 1：投影实现

- [x] `responses.rs`：`reject_cross_protocol_history`（Fail Fast）→
      `project_cross_protocol_history`（请求时投影）：`to_value(request)` 后对
      `input` 做 JSON 级变换——`reasoning` 项的 `encrypted_content` 若为本桥
      envelope（前缀判定）则删除该字段，item/summary/ID 保留；非 envelope 密文
      与其他 item 原样透传；持久化 rollout 不变；info 日志带
      `projected_reasoning_envelopes` 计数。
- [x] 协议事实核对中发现并登记：`ReasoningItemContent::ReasoningText` 是 codex
      内部回放形态，serde（`should_serialize_reasoning_content`）本就不将其发上
      Responses 输入（OpenAI 输入侧 reasoning 只认 summary/encrypted_content）。
      因此投影后的可见形态 = item + summary；完整 thinking 块保留在 rollout，
      不伪造上线路（"已丢失的信息不能伪造恢复"的 wire 侧体现）。
- [x] 单元测试改造（2 项规则表）+ wire 测试（出站 JSON 投影断言 + 请求侧
      envelope 仍在的对照断言）。
- 证据：`just test -p codex-rust-rig-bridge --offline --retries 0` →
  **99 passed / 0 skipped**，exit 0（98 + 2 单元改写 + 1 wire 新增 − 2 旧拒绝测试）。

## 批次 2：suite native 修正（默认 feature 绿色恢复）

- [x] `stream_no_completed.rs` / `stream_error_allows_next_turn.rs` /
      `responses_headers.rs`（3 处 provider）补
      `experimental_bridge: Some(ChatBridge::Native)`——这些测试验证 native
      重试/头语义，第三方 responses 默认走桥后在无桥 feature 构建下分发前
      Fatal（D5 同类修正，非行为变化）。
- 证据：`just test -p codex-core --offline --retries 0 --test all
  -E 'test(stream_no_completed) | test(stream_error_allows_next_turn)'` →
  **3 passed**；`--test responses_headers` → **5 passed**（均默认 feature）。

## 批次 3：真实厂商跨协议验证

- [x] `scenario_responses_cross_protocol`：合成 chat 形态历史（envelope +
      可见 summary/content + function_call/output 配对 + 后续用户消息）→
      真实 Responses 端点 → 模型作答 + usage；SSE fixture 录制入库。
- 证据：`LIVE_CASSETTE=record LIVE_VENDORS=mimo,glm
  just test … -E 'test(responses_cross_protocol)'` → run ID
  `3f89c1b4-aa35-46be-a7c3-b473b53efd45`，MiMo ✅（2.0s）GLM ✅（2.2s）
  Step 跳过（无 Responses 端点）。
- 全量 replay 回归：`LIVE_CASSETTE=replay LIVE_VENDORS=mimo,glm,step
  LIVE_INCLUDE_GENAI=0 just test -p codex-live-tests --offline --lib --test
  bridge_live --retries 0` → **91 passed / 0 skipped**（88 + 3 新场景）。
- 规范：`just fix -p codex-rust-rig-bridge -p codex-live-tests -p codex-core
  --offline`（3m55s，exit 0，零自动改动）+ `just fmt` + `git diff --check`
  clean；fix/fmt 后按 AGENTS 未重跑测试。

## 反向（responses→chat）回归证明

- 既有转换管线天然处理中立 `ResponseItem`（envelope 机制只认自己生成的前缀，
  Responses 产物无 envelope）；第一阶段 chat_default/chat_rig exec 6 项 +
  本阶段 replay 全量已覆盖。无代码变化，无新增风险。

## 遗留（2.1+）

1. provenance 管道进请求输入：同 provider 同 wire 的网关密文保留 / 异源密文
   剥离的细粒度规则（当前非 envelope 密文一律透传，跨网关时由网关报真实错误）。
2. 厂商能力声明配置化（per-provider hosted-tools 白名单）。
3. OpenAI 原生密文跨网关"翻译"：不可行，永久登记。

## 自审修复（2026-09-28 复查轮）

发现并修复一个序列化保真缺陷：投影原实现走 `serde_json::to_value` 全量往返，
带来三个隐性退化——(1) workspace 未启用 `serde_json/preserve_order`，`Value` 为
BTreeMap，出站 key 从 struct 序变字母序（与 Chat/native 路径字节形态不一致，
存量会话升级后首轮缓存前缀变化）；(2) `tools: Arc<RawValue>` 会被解析成 Value
再序列化，工具 schema 中超出 u64/i64/f64 的大整数丢精度、原始字节形态丢失；
(3) "删键"与既有 `encrypted_content: null`（字段无 skip_serializing_if）线上
形态不一致。修复为类型层方案：克隆 `ResponsesApiRequest`（tools 的 Arc 克隆
零拷贝）、envelope 项 `encrypted_content` 置 `None`、原样 `to_vec` —— key 序、
RawValue 字节、精度全部保持，投影后字段为 `null`（与无密文 reasoning 既有
线上形态一致）。

- 单元测试重写为类型层断言（可见 payload 保真 + 非 envelope 密文透传）。
- wire 断言改为 `encrypted_content == null`。
- 复验：rig-bridge **99/99**；真实厂商 cross-protocol + responses 文本
  （`null` 形态出站）MiMo/GLM ✅（2.0s/2.1s）；全量 replay **91/91**；
  `just fix -p codex-rust-rig-bridge`（1m16s，零自动改动）+ `just fmt` +
  `git diff --check` clean。

其余复查项：全仓 stale 引用扫描（旧名/拒绝函数）零命中；phase-1 提交
`0f62ba844` 内容核对干净（无 logs/临时文件混入）；suite native 修正 diff
符合原意；`response.created` 缺 response 载荷在严格泵下报错为 spec 既定
行为（真实网关 MiMo/GLM 均携带该字段，live 证据在案）。

## Claude 交接时 git 状态（历史记录）

第二阶段改动未提交（7 文件 + 4 fixtures + phase2 文档，含自审修复），交由 Codex 审查。

## Codex 独立复核与修复（2026-09-28）

完整发现与源码入口见 `my-docs/rig-responses-review-2026-09-28.md`；此节与当前 Spec/Plan 取代上面的旧兼容性及序列化结论。

- [x] 保留类型克隆投影：仅本桥 envelope 置 None→null，原始请求和 rollout 不变；补 RawValue、字段顺序、超大整数的原始 HTTP body 回归。
- [x] 修正 HTTP 建流超时、精确 Authorization、响应头/事件 metadata、顶层 error 立即终止、Responses Lite 和同 turn 路由状态。
- [x] 真正执行 shutdown/resume，检查旧 provenance/缺字段/工具配对；机械提取 session 的来源记录模块。
- [x] 更正 ReasoningText serde 方向、真实桥历史形态、原始请求证据口径和反向兼容边界。
- [x] Responses replay 走本地 HTTP + 实际 Rig 发送/解码路径，检查完整出站字节。
- [x] MiMo/GLM 从已有真实 Chat 录制构建历史后请求 Responses，精确返回工具结果独有标记；四个 xproto JSON/SSE fixtures 已重新录制。
- [x] native mock 显式选 native；扩展复测发现的共享 WebSocket mock 同类问题也已修复。

### 已完成验证

所有命令在 `codex-rs` 执行，统一 `CARGO_TARGET_DIR=/tmp/codex-review-280843-6rqdtknp/target`。使用 `just test`，未用 all-features。Cargo 的 `--offline` 仅禁止依赖联网；live 场景仍实际请求厂商。

| 项目 | 命令主体 | 本次结果 |
|---|---|---|
| 6 个相关 crates | `just test -p codex-rust-rig-bridge -p codex-api -p codex-rust-genai-bridge -p codex-history -p codex-model-provider-info -p codex-config --offline --retries 0` | 739/739，exit 0；最终 run `4f40f7fe-95bb-4d90-9326-c18f72a971a0` |
| Core 默认 feature | `just test -p codex-core --no-default-features --offline --retries 0 --lib --test all --test responses_headers -E 'test(missing_bridge_features_reject_before_native_responses_dispatch) \| test(provider_switch_keeps_native_ciphertext) \| test(stream_no_completed) \| test(stream_error_allows_next_turn) \| binary(responses_headers)'` | 10/10，exit 0；run `97970214-a13f-426b-9df6-17c3a2e1d2cc` |
| Core rust-rig 扩展集 | `just test -p codex-core --features rust-rig --offline --retries 0 --test all -E 'test(rig_responses_bridge) \| test(stream_no_completed) \| test(stream_error_allows_next_turn) \| test(responses_lite) \| test(suite::client_websockets)'` | 67/67，exit 0；run `d379f208-805c-482e-a349-18b4622673be` |
| 真实跨协议 | `LIVE_CASSETTE=record LIVE_VENDORS=mimo,glm LIVE_INCLUDE_GENAI=0 just test -p codex-live-tests --offline --test bridge_live --retries 0 -E 'test(mimo_rig_responses_cross_protocol) \| test(glm_rig_responses_cross_protocol)' --no-capture` | 2/2，exit 0；run `abb7af4b-0161-497e-8897-c7b2f34f024f` |
| 真实普通 Responses | `LIVE_CASSETTE=off LIVE_VENDORS=mimo,glm LIVE_INCLUDE_GENAI=0 just test -p codex-live-tests --offline --test bridge_live --retries 0 -E 'test(=mimo_rig_responses) \| test(=glm_rig_responses) \| test(=mimo_rig_responses_tool_round_trip) \| test(=glm_rig_responses_tool_round_trip)' --no-capture` | 4/4，exit 0；run `fdac7175-9499-4d62-80de-84ee5680c2f0` |
| 完整离线 live crate 回归 | `LIVE_CASSETTE=replay LIVE_VENDORS=mimo,glm,step LIVE_INCLUDE_GENAI=0 just test -p codex-live-tests --offline --lib --test bridge_live --retries 0 --no-capture` | 91 注册项执行通过，**48 项执行断言、43 项运行时提前返回**；exit 0；run `157b328c-a6d7-429c-a37b-521e29eb492f` |
| 配置 schema | `CARGO_NET_OFFLINE=true just write-config-schema` | exit 0 |

43 个提前返回项 = 36 GenAI + 1 A/B diff（当前禁用）+ 3 鉴权失败场景（只跑在线）+ 3 Step Responses（未配置端点）。48 个有效用例包含 39 个 Rig 场景和 9 个本地辅助/历史测试，不声称 91 个厂商场景全被验证。

测试二进制数量：相关 crates 12 个；Core 默认集 3 个，rust-rig 集 1 个；离线 live crate 2 个；两批在线均为 bridge_live 这 1 个二进制。Rig 桥本身为 62 个单元测试 + 42 个 wire 测试，共 104 项。

两批 live 合计 6 个测试场景、8 次 Responses 请求，MiMo 与 GLM 各 4 次。源 Chat 记录复用已有录制；本轮没有新增 Anthropic→Responses 真实验证。

### 扩展复测与收尾

首次 Core rust-rig 扩展集 22/25：两项 WS mock 默认走 Rig 已修；一项 MCP prewarm 缺少辅助产物，已执行 `cargo build -p codex-rmcp-client --bin test_stdio_server --offline`（exit 0）。修复后连同整个相关 WS 模块共 67/67 通过。

- 最终 `just fix -p codex-api -p codex-rust-rig-bridge -p codex-rust-genai-bridge -p codex-core -p codex-model-provider-info -p codex-live-tests --features codex-core/rust-rig --offline`：exit 0，零告警（2m01s，`fix-final.log`）。首次 Clippy 的测试辅助函数 `collapsible_match` 告警已等价合并。
- `just fmt`：exit 0；`git diff --check`：exit 0。
- 顺序为全部测试 → scoped fix → fmt；遵守 AGENTS.md，fix/fmt 后未重跑测试。格式化前的最后代码调整仅为上述等价 match 合并。
- 用户已明确选择“本轮只做相关模块与真实请求验证，然后提交”；未运行完整 workspace。本轮仅本地提交，不推送。
- 暂存新 SSE fixtures 后，默认 `git diff --check` 将协议必需的末尾空行报告为 `blank-at-eof`；保留原始录制字节，通过 `.gitattributes` 仅为 SSE fixtures 关闭该项检查，复查通过。

原始日志在 `/tmp/rig-responses-review-20260928-*.log`，未入库；新 fixtures 已扫描，未发现 `.env.local` 中的凭证值。
