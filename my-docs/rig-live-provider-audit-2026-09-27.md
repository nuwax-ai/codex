# Rig 真实厂商请求审计（2026-09-27）

## 1. 范围与结论

基线：分支 `test`，`280843aae2100491d2aeeb6fefb3c098cc46dc09`；SDK `rig-core = 0.42.0`。本轮是在该基线之上的未提交修复，没有更新依赖。

使用用户指定的 MiMo、GLM、Step Plan 实际凭据和端点。共 **46 个在线测试、61 次真实 HTTP 请求**：58 次 200、3 次预期的错误凭据 401。所有捕获均完整 EOF，没有捕获错误或下游断连。没有请求 xAI，没有启用 GenAI 场景。

**发现并修复一处 P3 诊断字段缺口**：Step 的 `X-Trace-Id`、GLM Chat 的 `X-LOG-ID` 未进入 `ResponseStream.upstream_request_id`。54 个成功 Rig 回合的原始文本、reasoning、usage 与 Codex 事件逐值相等；工具续轮未发现内容丢失。此结论限于本次样本，不表示所有协议字段均已实现。

**厂商接受参数，不代表遵守参数。** 本轮 GLM 两种协议、MiMo Anthropic 的 schema 场景输出不是 JSON；Step Anthropic 虽返回符合结构的 JSON，内容却不相关且有大量空白。Step 还在收到 `thinking: disabled` 后返回可见 thinking。这些内容已存在于原始 SSE，不能归因于桥重复、丢失或错误拼接。

### 实际请求路径

| 厂商 | Chat 完整路径 | Anthropic 完整路径 | 原生 Responses 完整路径 |
|---|---|---|---|
| MiMo | `https://token-plan-cn.xiaomimimo.com/v1/chat/completions` | `https://token-plan-cn.xiaomimimo.com/anthropic/v1/messages` | `https://token-plan-cn.xiaomimimo.com/v1/responses` |
| GLM | `https://open.bigmodel.cn/api/coding/paas/v4/chat/completions` | `https://open.bigmodel.cn/api/anthropic/v1/messages` | `https://open.bigmodel.cn/api/v1/responses` |
| Step | `https://api.stepfun.com/step_plan/v1/chat/completions` | `https://api.stepfun.com/step_plan/v1/messages` | 本轮未测 |

Rig 的 Anthropic 客户端会规范化 base URL 的 `/v1` 后缀，实际路径没有重复 `/v1`。Chat 和 Messages 经 Rig；MiMo/GLM 的 Responses 通过 `experimental_bridge = "native"` 验证。**当前本 fork 的 Rig bridge 只接入 Chat/Anthropic；Rig 0.42.0 本身已有 Responses 客户端，本桥尚未接入。** 第三方 `wire_api = "responses"` 的默认 Rig 路由仍会请求 Chat，不能与本表的原生 Responses 混称。SDK 与本桥的源码依据见 [协议审计中的能力澄清](rig-protocol-audit-2026-09-27.md)。本轮真实请求没有覆盖 Rig Responses 路径。

## 2. 实测矩阵和证据

| 批次 | 模型 | 在线测试 | HTTP 请求 | 结果 |
|---|---|---:|---:|---|
| 主测 | `mimo-v2.6-flash`、`GLM-5.3-Flash` | 24 | 30 | 28×200、2×预期 401 |
| Step 主测 | `step-5-preview` | 12 | 15 | 14×200、1×预期 401 |
| 原生 Responses E2E | `mimo-v2.6-flash`、`GLM-5.3-Flash` | 2 | 4 | 真正执行命令、回传结果、完成回复 |
| 修复后追踪头复验 | `GLM-5.3-Flash`、`step-5-preview`，各 Chat/Anthropic | 4 | 4 | 4×200，追踪字段与响应头一致 |
| 备用型号工具续轮 | `mimo-v2.5`、`step-3.7-flash`，各 Chat/Anthropic | 4 | 8 | 8×200，4 条续轮完整 |
| **合计** | **5 个型号** | **46** | **61** | **全部测试满足各自现有断言** |

三家主测每家 12 场景：Chat 文本/schema/low effort/工具续轮/并行工具/错误凭据；Anthropic 文本/high effort/none effort/schema/工具续轮/失败工具续轮。三个双轮场景使每家产生 15 个请求。

证据根目录：`logs/live-wire-review-2026-09-27/`（gitignored）。

- `wire/001-*` 至 `wire/061-*`：每次实际 HTTP 的 `request.json`、`response.body`、`meta.json`。
- `fixtures/`：主测 42 份 Codex 事件 + 42 份 Rig 中间事件；`verification-fixtures/`、`alternate-fixtures/` 分别保存复验和备用型号。
- `wire-audit-summary.json`：每个请求的状态、模型、响应 ID、终止原因、原始/转换 usage、文本比对结果；54 个 Rig 回合文本/reasoning/usage 均相等。
- `primary-tests.log`、`step-tests.log`、`native-tests.log`、`verification-tests.log`、`alternate-tests.log`：真实 nextest 结果；同名 `*-command.json` 保存命令及不含凭据的运行参数。
- `logs/live-{vendor}/bridge/*.log`：桥事件和新增的上游追踪字段；末轮 12 个回合逐一核对了响应头与日志，包括没有该字段时的 `None`。
- 原生 E2E：`logs/live-glm/glm-responses-native-1790512817656091000-82922/`、`logs/live-mimo/mimo-responses-native-1790512831488912000-84534/`，保留 events、stderr、最终答案、manifest。

捕获方式为本机 loopback 转发到真实 HTTPS 厂商，HTTP body 原样转发，TLS 使用系统 CA 验证。只录必要的响应头和认证头是否存在，不录认证值；没有将密钥加入源码、fixture 或报告。该方式验证实际协议交换，未覆盖 SDK 直连厂商的 TLS 行为。响应头采用白名单，不能由“未录制”推断上游绝对不存在其他头。

## 3. 已修复的问题

### F1 / P3：供应商追踪头未传到 Codex 诊断字段

证据：`wire/031-step-anthropic/meta.json:16`、`040-step-chat/meta.json:15` 返回 `X-Trace-Id`；GLM Chat 返回 `X-LOG-ID`。原 `codex-rs/codex-rust-rig-bridge/src/transport.rs:210` 仅识别 `x-request-id` 和 `request-id`。

修复后按以下顺序选择非空、有效的头值：

1. `x-request-id`
2. `request-id`
3. `x-trace-id`
4. `x-log-id`

该字段用于 feedback/rollout trace 的请求关联，不替代 SSE 中的 `Completed.response_id`。会话续接仍使用模型响应 ID（`core/src/client.rs:1470`）；`codex-api/src/common.rs:414` 的注释同步说明诊断 fallback。

新增 `codex-rs/codex-rust-rig-bridge/tests/wire/request_id_tests.rs`：8 种头组合 × 2 协议，覆盖优先级、大小写、空 canonical 值回退、无头、模型响应 ID 独立且仅完成一次。**修复前红、修复后绿**，分别见 `request-id-before.log` 与 `rig-tests-after.log`。真实复验 `050/052/054/057` 的字段也与响应头一致。

### 证据设施补充

- `LIVE_FIXTURE_DIR` 同时覆盖两种 fixture 的读写根目录，本轮不覆盖仓库历史 fixture。
- `drain_stream` 日志记录 `upstream_request_id`，可以与原始响应头交叉核对。
- 新增隔离子进程测试覆盖自定义目录和原默认目录，并要求执行完成 marker，避免 `--exact` 零匹配却退出成功的假绿。

## 4. 字段逐项核对与厂商差异

| 字段 / 语义 | 本轮观察 | 裁决 |
|---|---|---|
| 文本、reasoning 增量与完成项 | 54 个成功 Rig 回合均与原始 SSE 相等 | 未见丢失/重复拼接 |
| 工具名称、arguments、call ID | 主测 15 个、备用型号 4 个调用；Added/Delta/Done 关联一致 | 未见工具事件错配 |
| 工具续轮历史 | Rig 13 条、native 2 条续轮；原消息前缀不变，首轮输出完整追加 | 未见历史丢失 |
| Chat 工具参数字符串 | MiMo `{"city": "北京"}` 续轮变为 `{"city":"北京"}` | JSON 空白规范化，值不变；不是逐字字符串保证 |
| thinking/signature | GLM 24 字符、Step 32 字符签名完整回传；MiMo 样本没有非空签名 | 非空 MiMo signature 未验；空 signature 省略不等于有值签名丢失 |
| `tool_result.is_error` | 三家失败工具续轮实际发送 `true`，模型返回失败说明 | 字段映射有真实证据 |
| 并行工具 | 三家 Chat 均返回北京/上海两次独立调用、不同 ID、合法参数 | 本轮确实多调用，但现有测试只断言至少一个 |
| Chat usage | MiMo 的 finish 后独立 usage chunk、Step 重复累计 usage 均保留最终计数 | 不是把所有累计帧相加 |
| Anthropic usage | 累计更新合并后，input 包含缓存读/写；缺失 reasoning 分项按当前类型显示 0 | 未单列 token 不证明没有 thinking；本轮未发现已报告计数丢失 |
| 缓存 | GLM `015` cache-read=128，MiMo `028`=64；native 也有非零缓存 | 非零缓存计数有实证 |
| 完成条件 | Chat `[DONE]`、Anthropic `message_stop`；原生 Responses `response.completed` | 正常结束全对齐；本轮未制造真实断流 |
| HTTP 追踪 ID / 模型响应 ID | 前者来自头，后者来自 SSE；修复新增 trace/log fallback | 两种 ID 没有混用 |
| Anthropic 附加 usage | GLM 的 `service_tier=standard`、`server_tool_use` 零计数未进入 Rig `PartialUsage` 类型 | 无非默认/非零样本，不能声称这部分元数据无损 |

### Schema 与 thinking 的真实结果

| 主测模型 | Chat schema | Anthropic schema | Anthropic `thinking: disabled` |
|---|---|---|---|
| `mimo-v2.6-flash` | 本次结构/答案符合 | 218 字符 Markdown，非 JSON | 可见 thinking 为 0 |
| `GLM-5.3-Flash` | 244 字符非 JSON | 361 字符非 JSON | 可见 thinking 为 0 |
| `step-5-preview` | 本次结构/答案符合 | JSON 结构符合，答案语义失败且大量空白 | 仍有 67 字符 thinking |

三种非 JSON 案例的最终 HTTP 请求都有正确 schema（GLM `004/011`、MiMo `019`），不是桥漏字段。Chat 场景发送的是 `strict:false`；本轮未验证 `strict:true` 的厂商行为。

Step `034` 的最终 assistant 文本为 **14,405 字符 / 14,845 UTF-8 字节**，包括 8,411 个 tab 和 4,900 个空格；`city1` 为无关多语内容，`city2` 为“ 城市简称”。原始与转换文本完全相等，终止是 `end_turn`，provider output token 为 5,419（含 reasoning）。按仓库字节估算法文本约 3,712 token，触发 context 技能的 **P0 人工复核门槛**；已人工检查并归因为模型输出质量异常，**不是此次代码新增的 P0 缺陷**。没有为让测试通过而截断或改写模型输出。

### 官方文档与本轮实测的关系

- MiMo 要求带工具调用的多轮完整回传 `reasoning_content`。本轮 Chat/Anthropic 历史均完整保留；但官方说明页列举的型号与实测型号不完全相同，不能外推全部型号行为。[MiMo 多轮 reasoning 回传](https://platform.xiaomimimo.com/docs/en-US/usage-guide/passing-back-reasoning_content)
- GLM-5.3-Flash 文档写明 thinking 仅支持 enabled；本轮 Anthropic 兼容网关接受 disabled 且没有可见 thinking，这不足以证明内部推理关闭。结构化输出指南主要展示 `json_object`，不能据此承诺 `json_schema` 严格执行。[GLM 型号说明](https://docs.bigmodel.cn/cn/guide/models/vlm/glm-5.3-flash)、[GLM 结构化输出](https://docs.bigmodel.cn/cn/guide/capabilities/struct-output)
- Step 文档分别使用 Chat `reasoning_effort`、Messages `output_config.effort`，并描述 JSON Schema。请求字段映射符合该形态；响应是否满足要求仍需单独验证。[Step 模型说明](https://platform.stepfun.com/docs/zh/guides/models/step-5-preview)、[Step Chat API](https://platform.stepfun.com/docs/zh/api-reference/chat/chat-completion-create)
- GLM Responses 文档声明无 `[DONE]`，本次网关实际附带该标记；MiMo 没有。Codex native 以 `response.completed` 处理完成，所以两种表现均可用；MiMo 的 SSE `data:` 后不含空格也正常解析。[GLM Responses](https://docs.bigmodel.cn/cn/guide/develop/responses/introduction)

官方页面检索日期同本文；可读 markdown/页面快照保存在 `official-docs/`，避免把检索超时或搜索摘要当作字段依据。

## 5. 验证命令与计数

统一使用 `just test`，保留仓库的 `RUST_MIN_STACK=8388608`，禁用重试，单并发。隔离构建目录：`/tmp/codex-review-280843-6rqdtknp/target`。下列为已执行入口；完整环境、逐批参数和捕获路由见对应 command JSON。

```sh
# 三家主测（两批）
LIVE_VENDORS=mimo,glm LIVE_CASSETTE=record just test --offline -p codex-live-tests --test bridge_live \
  --retries 0 -j 1 -E 'test(/^(mimo|glm)_rig_/)'
LIVE_VENDORS=step LIVE_CASSETTE=record just test --offline -p codex-live-tests --test bridge_live \
  --retries 0 -j 1 -E 'test(/^step_rig_/)'

# native 构建与真实 E2E
cargo build --offline -p codex-exec --bin codex-exec
LIVE_CASSETTE=off just test --offline -p codex-live-tests --test exec_live \
  --retries 0 -j 1 -E 'test(/^(mimo|glm)_responses_native$/)'

# 本地回归
just test --offline -p codex-rust-rig-bridge --retries 0 -j 1
LIVE_CASSETTE=replay just test --offline -p codex-live-tests --lib --retries 0 -j 1
LIVE_CASSETTE=replay just test --offline -p codex-live-tests --test bridge_live \
  --retries 0 -j 1 -E 'test(/^(mimo|glm|step)_rig_/) & !test(auth_rejected)'

just clippy --offline -p codex-rust-rig-bridge -p codex-live-tests -- -D warnings
just fmt
```

复现时需明确设置 `CARGO_TARGET_DIR`、`LIVE_VENDORS`、`LIVE_INCLUDE_GENAI=0`，以及绝对路径 `LIVE_FIXTURE_DIR`。`--offline` 只禁止 Cargo 下载依赖，**record 模式仍会真实请求模型**；replay 才不请求厂商。本轮 replay 指向本次的新 fixture，过滤掉 3 个鉴权场景，未把它们的运行时提前返回计作回放验证。

本地验证为 **122 项 / 4 个测试二进制目标**：Rig lib 52 + wire 30 = 82；live-tests lib 7；新录制回放 33。均零失败、零重试。Clippy `-D warnings` 通过，`just fmt` 退出 0。本次变更完成格式化；格式化触及的 6 个无关原有文件已恢复，因此不声称整个仓库的 `fmt-check` 均无历史差异。按仓库规则，最终格式化后没有重跑测试。

## 6. 仍需保留的验收边界

1. `bridge_live.rs:263` 和 `anthropic_controls.rs:43` 是 schema **接受性**测试，没有结构/语义合规硬门槛；上表独立报告了真实输出结果。
2. `bridge_live.rs:460` 并行场景只要求调用数 ≥1；本轮人工检查了实际两调用、ID 唯一性和城市参数，但测试本身尚未保障全部条件。
3. `anthropic_controls.rs:90` 的 none effort 只诊断可见 thinking；0 可见字符不能证明内部未计算。high/low 字段被接受也不能证明强度实际生效。
4. `bridge_live.rs:329` 的真实错误凭据测试只走 Chat；Anthropic 的真实 401、core 重登录流程未在本轮执行。已有本地 auth 测试不能替代这些真实路径。
5. 原生 Responses 本轮覆盖工具闭环及 usage，未覆盖其 schema、取消、存储与 previous_response_id；其临时 rollout 文件已删除，未另行核对 rollout 落盘，仅核对 raw SSE、stderr、exec JSONL。
6. 未新增验证图像/音视频/文件、跨厂商历史迁移、redacted thinking、真实网络中断/429/5xx、Linux/Windows。已有协议审计中不等价或未映射字段仍保持原边界。

本轮没有更改厂商控制策略，也没有把模型不遵从参数改成静默降级。此次生产修复仅补齐诊断关联字段；通用 schema 强校验或厂商能力配置应作为独立需求设计，不能用这一轮偶发生成结果自动改写模型能力。

---

## 追加：Rig Responses 第一阶段真实验证（2026-09-28）

本轮实施同协议直通（`my-docs/rig-responses-phase1/`）：`wire_api = "responses"` 的第三方
provider 默认经 rig 桥直发 `POST {base}/responses`（Codex `ResponsesApiRequest` 原样序列化，
不经 Chat 转换）。真实厂商证据：

| 场景 | MiMo | GLM | Step |
|---|---|---|---|
| bridge 级文本（responses 直通） | ✅（reasoning+text+usage） | ✅ | 跳过（无 Responses 端点） |
| bridge 级两轮工具闭环 | ✅ t1 调用/t2 回传 | ✅ | 跳过 |
| exec 级 marker 闭环（responses-rig-default） | ✅ | ✅ | 跳过 |
| Chat/Anthropic 转换回归 | ✅ | ✅ | ✅ |
| native Responses 回归 | ✅ | ✅ | 跳过 |

厂商行为发现（接受 ≠ 遵守，分别登记）：

- **MiMo Responses 网关拒绝 hosted tools**：携带 `web_search` 工具的请求收到
  HTTP 400 `responses_feature_not_supported`（"tool type 'web_search' is not supported by
  this gateway phase"）。直通路线如实暴露该差异；配置层 `web_search = "disabled"` 是
  显式解法（与 native 场景先例一致）。旧 Chat 转换路线掩盖了这一点。
- **MiMo Responses 不发送 `response.function_call_arguments.delta`**：函数调用以
  output_item.added+done 整体下发（完整参数在 done 中）。官方协议中 delta 帧可选；
  responses 工具场景断言已按"有增量必须可重组 / 无增量则整体参数完整有效 JSON"放宽。
- **GLM Responses 接受 hosted `web_search` 工具**（请求 200 并完成闭环）；是否真正
  执行搜索未单独验证。
- 原始 SSE fixtures 已录制并入库（`live-tests/tests/fixtures/{mimo,glm}/responses-sse-*.txt`），
  replay 经同一严格终止泵离线回归。
