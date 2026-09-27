# Rig Responses 第一阶段 — Tasks（执行记录）

> 证据均为本机 macOS 实测；命令经 `just test`（RUST_MIN_STACK=8MB + NEXTEST_PROFILE=local）。

## 批次 1：协议路由与 Rig Responses 传输

- [x] T1.1 codex-api：`ModelWireProtocol`（Responses|ChatCompletions|Anthropic）与
      `ModelBridge` trait 更名；删除 `chat_wire_protocol`（URL 嗅探退出分发）；
      `pub mod sse::responses` + 根级 re-export 公开解码器（native 行为零变化）。
- [x] T1.2 model-provider-info：`uses_model_bridge` 更名（6 文件；文档语义更新为
      “使用桥 ≠ 实际发 Chat”）。
- [x] T1.3 rig-bridge：`RigProtocol::Responses` + `build_responses_client`
      （rig `providers::openai::Client`，锁定版默认即 Responses 客户端）+
      transport Responses 透传分支（跳过全部 body 注入与 SSE terminal 改写；
      保留请求 ID 捕获回退链、错误脱敏、查询参数重编码、自定义 CA）。
- [x] T1.4 rig-bridge `responses.rs`（新模块，310 LoC）：`serde` 直序列化
      `ResponsesApiRequest` → `client.post_sse("/responses")?.body(bytes)?` →
      `HttpClientExt::send_streaming`（rig 真正发送 HTTP）；
      `InvalidStatusCodeWithDetails` → `TransportError::Http{status,headers,body,retry_after}`
      （401 恢复环可触发）；rig 字节流 → `codex_http_client::ByteStream` 适配；
      无叠加重试。
- [x] T1.5 core：`dispatch_model_bridge` 显式 `wire_api` match（无 URL 嗅探）；
      `stream_model_bridge` 的 span/telemetry 端点按实际 wire 计算
      （responses → `/responses`）。
- [x] T1.6 genai 桥编译适配：`ModelWireProtocol::Responses` → InvalidRequest 明确报错。
- [x] T1.7 wire 测试（`tests/wire/responses_wire_tests.rs` + 三协议扩展既有套件）。
- 证据：
  - `just test -p codex-rust-rig-bridge --offline --retries 0` →
    **98 passed / 0 skipped**，exit 0（原 82 + 新增 16）。
  - `just test -p codex-api -p codex-model-provider-info -p codex-config --offline --retries 0`
    → **581 passed**，exit 0。
  - `cargo check -p codex-core --offline`（默认 feature）与 `--features rust-rig` 均
    exit 0；`-p codex-tui -p codex-exec -p codex-app-server` check exit 0。
    workspace `--all-targets` 因 `codex-voice-host` 的 gstreamer-sys 系统库缺失失败
    （本机无 gstreamer，与本次改动无关）。

## 批次 2：事件、错误处理与来源元数据

- [x] T2.1 严格终止 pump（`strict_responses_pump`）：critical-kind 必需字段缺失即错、
      坏 JSON 帧即错、`response.failed`/`incomplete` 立即报错并停止、断流报
      `stream closed before response.completed`、idle timeout、`[DONE]` 哨兵兼容
      （GLM 实测附带）、无合成 Created/item ID/Completed。9 项单元测试 +
      wire 级失败矩阵（`error_tests` 扩展为三协议循环）。
- [x] T2.2 envelope 混入 Fail Fast（`reject_cross_protocol_history`：跨协议旧会话
      InvalidRequest；原生密文与缺失 encrypted_content 放行）。
- [x] T2.3 codex-history：`ModelOutputProvenance{wire_protocol,bridge,provider,model}`
      （`#[serde(default)]` 旧记录缺省读取；无凭证；不发给模型）。
- [x] T2.4 core 持久化：`record_model_generated_items` 仅模型输出路径标注，
      持久化时从当前请求上下文捕获；hook/user 注入与 native first-party 不标注；
      resume 不回填。
- [x] T2.5 集成 `core/tests/suite/rig_responses_bridge.rs`（`#![cfg(feature="rust-rig")]`）。
- 证据：
  - `just test -p codex-core --features rust-rig --offline --retries 0 --test all
     -E 'test(stream_no_completed) | test(stream_error_allows_next_turn) | test(rig_responses_bridge)'`
    → run ID `aba60f7e-b716-43eb-a892-264247a2da14`，**4 passed**，exit 0
    （含 `responses_bridge_same_wire_round_trip_and_provenance`：真实 `/v1/responses`
    路由、两轮历史逐字回放、rollout provenance 断言）。
  - 基线对照：同一测试集在 `-p codex-core`（默认 feature 无桥）2 失败且 0 请求到达
    server —— 分发前 Fatal 的既有配置现象，非本次回归。
  - `just test -p codex-rust-genai-bridge -p codex-history -p codex-model-provider
     --offline --retries 0` → **154 passed**，exit 0。

## 批次 3：集成、真实厂商验证与文档

- [x] T3.1 `responses_base_url` 显式化：`LIVE_<V>_RESPONSES_URL` 缺失 → 场景跳过
      并打印 notice（replay 也不回退）；`.env.local` 追加 `LIVE_MIMO_RESPONSES_URL`
      （同源值，脚本复制、未打印密钥）；config 测试同步。
- [x] T3.2 exec 级 `responses_rig_default` 修正：显式 Responses 端点 + 断言
      `Dispatching responses stream via rig` 分发日志；bridge 级新增
      `scenario_responses`/`scenario_responses_tool_round_trip`（rig-only matrix）；
      原始 SSE 录制（transport tee → `responses-sse-<tag>.txt`）+
      `replay_responses_sse` 严格泵离线回放。
- [x] T3.3 真实厂商（2026-09-28，证据存 `logs/live-{mimo,glm}/`）：
  - bridge 级：MiMo ✅ 文本（reasoning+text+usage）、✅ 两轮工具闭环；
    GLM ✅✅；Step 跳过（无 Responses 端点，显式 notice）。
    record 运行 `cfa30eab-2573-4e69-a197-c9a62d112ac8`：3 passed（mimo/glm tool + step skip）。
  - exec 级（run `d141341a-fee6-4930-ba29-8c6616a21f23`）：**12 passed** ——
    responses_rig_default MiMo/GLM ✅（marker 工具闭环）、responses_native MiMo/GLM ✅、
    chat_default/chat_rig × 3 家 ✅（转换路径回归）；Step responses 两项显式跳过。
  - 接受 ≠ 遵守登记（写入 rig-live-provider-audit 追加节）：
    MiMo Responses 拒绝 hosted `web_search`（HTTP 400，配置层 `web_search="disabled"`
    为显式解法）；MiMo Responses 无 `function_call_arguments.delta`（整体下发，
    断言按协议可选性放宽）；GLM 接受 hosted web_search（未验证是否真执行）。
- [x] T3.4 回归与规范：`just fix -p`（7 个受影响 crate，16m47s，exit 0，零违规、
  零自动改动）+ `just fmt`（仅本次文件）+ `git diff --check` clean。
  fmt 后按 AGENTS 未重跑测试（fmt 仅格式）。
- [x] T3.5 文档：spec/plan/tasks、审计文档追加节（真实厂商发现）、
  rig-protocol-audit 既有未提交修正保留未动。
- 离线 replay 全量回归：
  `LIVE_CASSETTE=replay LIVE_VENDORS=mimo,glm,step LIVE_INCLUDE_GENAI=0
   just test -p codex-live-tests --offline --lib --test bridge_live --retries 0`
  → **88 passed / 0 skipped**（含 6 项 responses 场景离线严格泵回放）；lib 7/7。

## 最终交付核对清单

### 实际请求链路与改动范围
`core stream()` → `uses_model_bridge()` → `stream_model_bridge` →
`dispatch_model_bridge(wire_api → ModelWireProtocol::Responses)` →
`RigModelBridge` → `stream_via_rig(Responses)` → `responses.rs` 直通
（序列化 → rig openai Client `post_sse("/responses")` → RigHttpClient 装饰器 →
reqwest 0.13 发送 → 原始 SSE → codex-api 解码器 → 严格终止 pump → ResponseStream）。
41 文件改动（+948/−292）+ 17 新文件（3 源码/测试、1 集成测试、12 fixtures、4 文档）。

### 字段处理清单（Responses 路径）
- 保留（原样序列化）：instructions、input、tools（含 namespace/custom/strict）、
  tool_choice、parallel_tool_calls、reasoning（effort/summary/context）、text
  （verbosity/format+strict+name）、include、store、stream、stream_options、
  service_tier、prompt_cache_key、client_metadata、access_programs（wire 级
  `assert_eq!(body, to_value(request))` 全量对照证明）。
- 拒绝（Fail Fast）：历史含 rig chat/anthropic 回放 envelope（跨协议旧会话）；
  genai 桥 + wire=responses。
- 转换：无（同协议直通，不经 Chat 转换管线）。
- 未覆盖（协议未建模/网关侧）：server 端 hosted tools 执行语义（web_search 等，
  由配置层声明）；`previous_response_id`（codex 不发送，未变化）。

### 测试汇总
| 套件 | 结果 |
|---|---|
| rig-bridge（单元+wire，98 项） | 98/98 |
| codex-api + model-provider-info + config | 581/581 |
| genai + history + model-provider | 154/154 |
| core 定向（rust-rig，4 项） | 4/4 |
| live replay 全量（88 项） | 88/88 |
| 真实厂商 exec 矩阵（12 项） | 12/12 |

### 真实厂商证据与限制
见上文 T3.3 与 `my-docs/rig-live-provider-audit-2026-09-27.md` 追加节；
fixtures 已入库（含凭证扫描通过）。限制：未测真实 429/5xx/网络中断（本地
wire 矩阵覆盖这些失败类别的行为语义）；未测 Linux/Windows；GLM web_search
执行语义未验证。

### 第二阶段遗留
1. 旧会话跨协议切换的历史投影（原始历史＋provenance＋目标协议能力）；
   provenance 已在本期持久化。
2. 依据 provenance 的 cache/计费核对（cache_control 候选仍挂起）。
3. 厂商能力声明配置化（per-provider hosted-tools 白名单）替代逐场景
   `web_search="disabled"`。
4. core 无桥 feature 下既有 suite 失败（stream_no_completed 等 2 项）为
   配置现象，建议后续为 core 测试启用桥 feature 或标注 feature-gated。

### git 状态
按指令未 commit / 未 push；工作树保留全部改动（含 Codex 此前对两份审计
文档的未提交修正，未被覆盖）。
