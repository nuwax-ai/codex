# rig 接入最终开发与验收（2026-10-18，Claude 轮）

> 本轮输入：Codex `test@99397fbd7`（干净）、ACP `dev@958b63f`（vendor/ 未跟踪保留）。
> 目标不变：NUWAX 环境变量 → rig 三协议 → Linux/容器 app-server + ACP 日常使用。
> 本轮范围：字段处置审计、原图跨协议视觉预算下界、最终源码三协议实网闭环、推送后 CI。

## 判决

**核心接入开发收口。** 完成条件逐条满足：

1. 字段处置表（§2）无未裁决的用户可见丢失；已发现的静默丢失（MCP 结构化结果遮蔽媒体、
   工具定义转换失败清空工具表、超范围整数精度、Chat 纯文本数组）均在此前提交中修复并留有
   红→绿记录，本轮复核无新增。
2. 原图跨协议预算下界合同通过（§3）：禁用 floor 的对照实验下负例失败（请求照发），
   启用后发网前拒绝、HTTP 请求数为 0；窗口充足的请求图片完整到达接收端。
3. 最终源码统一重建二进制后，MiMo Chat、GLM Responses、Step Anthropic 三协议
   12 步连续工作流全部一次通过（§4），54 次真实上游请求、预算 90、denied=0。
4. CI 复跑收据见 §6。

## 1. 本轮修改与根因

### Codex `c4d08f71b` fix(rig): floor converted originals at their canonical vision estimate

- **根因**：Chat 把 `detail:"original"` 收敛为 `high`、Anthropic 线无 detail 字段后，
  最终 wire 层对保留原图只按 prepared-image 估计计 1844 tokens；2560×2048 原图的
  转换前规范估计是 5120（32px patch 计数）。普通会话的最终出站预算因此低于转换前估计，
  小窗口下会把超窗请求放出网。工具入口/历史/guardian 已有保护，Responses passthrough
  原样序列化保留 original detail、由 wire walk 以同一定价器计费，均不受影响。
- **修复**：转换线在每次 attempt 构建请求时，从当前 `conversion_request.input` 计算
  canonical 预转换视觉估计（`wire_images::canonical_vision_floor`：仅 Original detail 计入，
  与 passthrough wire walk 的 `record_image_reference` 同一估算器和 10k 上限），
  经 `WireBudget.vision_floor_tokens` 传入传输层；`check_body` 取
  `max(wire walk 视觉和, floor)`——只升不降、不相加、不把压缩已移除的图片算回
  （floor 只遍历当前请求实际携带的图片项）。floor 为传输层内部值，不进入厂商请求体。
  暂停续跑对截断后的输入重算；压缩后重建请求同理使用当前数据。
- **红→绿**（对照实验，产品代码未被改绿）：临时将 floor 函数改为恒返回 0（等价旧行为），
  `converted_originals_are_rejected_before_http_on_small_windows` 与
  `multiple_originals_sum_the_floor_and_mixed_text_still_counts` 双双失败（请求被发出）；
  恢复后通过。负例断言 `ApiError::ContextWindowExceeded` 且 attempts==0、无 dispatched body。
- **测试**（新增 14 个，均在最终命令内通过）：
  - 单元：floor 对用户/工具原图按 patch 计数（2×5120）；非 original detail 与文本内伪造
    data URL 计 0；不可定价原图（乱码 base64/远程 URL/file 引用）按 10k 上限；
    floor 与 passthrough walk 对同一原图估值相等；空/压缩后形态 floor=0；
    `check_body` 的 max 语义（只升不降不加倍）。
  - 出站（真实 转换→Rig SDK→本地 socket）：Chat/Anthropic × 用户图/工具图 ×
    窗口 3000 拒绝（0 连接）与窗口 8000 放行（图片完整到达，Chat 呈现 `detail:"high"`、
    Anthropic 呈现 base64 source）；双原图求和（6000 拒/12000 收，两图都到 socket）；
    压缩后纯文本历史在 1200 小窗口正常放行。
- **明确不冒称**：这是内部预算一致性加固，未复现也不断言厂商真实 tokenizer 溢出；
  未放宽任何窗口/限额/断言；文本、字节上限（32 MiB）、输出预留、既有图片数量/解码/入口
  限制全部保持。

### ACP `dev` 提交（仅验收驱动，产品源码未改）

`scripts/live-validation/{live-validation.py,live_support.py,test_live_support.py}`：
- case 语法扩展 `name@VENDOR/wire`（如 `acp-workflow@MIMO/chat`），固定路由的 wire-* 等
  case 拒绝覆盖；行名/工件名带后缀，冷恢复正确映射同路由的 main-text 会话目录；
  重复 spec 拒绝。默认行为不变。
- `provider_env` 读取 `LIVE_WINDOW_<VENDOR>`（正整数）设置子进程
  `CODEX_MODEL_CONTEXT_WINDOW`，使实网运行的本地预算符合厂商公布窗口；
  宿主残留的原始 LIVE_* 键不透传（既有剥离合同保持，新增负例断言）。
- 合同测试 14/14（mac 与验机均跑），覆盖 parse 语义、窗口传播、乱值忽略。

## 2. 当前版本字段处置表（源 → 目标 → 处置 → 源码 → 证据）

桥源码入口：`codex-rs/codex-rust-rig-bridge/src/`；`W` = tests/wire/ 下文件，
`I` = crate 内部测试文件。Responses 走同协议 passthrough（`responses.rs`），非转换。

| 源字段（Codex 协议） | Chat 目标 | Anthropic 目标 | 处置 | 源码 | 测试证据 |
| --- | --- | --- | --- | --- | --- |
| Message role=user / content[] | messages[].role=user / content[] | 同左 | 保序保留；图文混合入专用块 | request_messages.rs:173、request_content.rs:231 | W history_tests、wire.rs 图片矩阵 |
| role=assistant（连续合并） | assistant（reasoning→text→tool_calls 保序） | assistant（thinking→text→tool_use） | 相邻 assistant 项合并为同轮；DeepSeek 型 reasoning 回放保留 | request_messages.rs:136 | W history_tests::actual_chat/anthropic_second_request_*、legacy_*_replays |
| role=system/developer | system（拼接） | system | 纯文本保留；非文本拒绝 | request_messages.rs:160 | W wire.rs::final_chat_request_preserves_url_headers_role_effort |
| 未知 role | — | — | 明确拒绝 InvalidRequest | request_messages.rs:180 | I request_messages_tests |
| FunctionCall/CustomToolCall（name、namespace、call_id、arguments） | tool_calls[].function.{name,arguments}+tool_call_id | tool_use.{name,input,id} | namespace 折入名称（映射表双向）；空 call_id 拒绝 | request_messages.rs:30、request_tools.rs | W namespaced_additional_custom_tools_keep_wrapper_and_history_name |
| FunctionCallOutput（call_id、内容、success） | tool role + 紧邻 user 图片块 | tool_result.content[] | 配对保留；is_error 映射；缺省 success 不发明 | request_content.rs:13 | W history_tests::tool_success_flag_is_preserved…、wire.rs::function_and_custom_image_results… |
| MCP structuredContent + typed 媒体 | 结构化 JSON 文本 + 原顺序媒体块 | 同左 | 两者并存保留（9aa9579fb 修复遮蔽） | request_content.rs:110 | W mcp_structured_media_tests（+ localhost 三线 HTTP 回归） |
| 工具 schema（strict、i64/u64） | function.strict/parameters | tools[].strict/input_schema | 原样保留；raw→Value 失败与超范围整数明确拒绝（0d6d3cbd2） | request_tools.rs | I mapping 模块 25 用例（旧实现 3 败→修复后全绿） |
| 工具选择 tool_choice | tool_choice | tool_choice{,disable_parallel_tool_use} | auto/none/required/具名映射；不可满足时拒绝 | convert_request.rs:243 | W wire.rs::output_schema_survives_first_tool_turn |
| 图片 InputImage（Inline data URL） | image_url{url,detail}（original→high） | image{source.base64}（无 detail） | 专用图片块；视觉预算见 §3；file_id 引用明确拒绝 | request_content.rs:190 | 本轮 floor 套件 + W mcp_structured_media_tests |
| 图片 detail=original 的预算 | 高于 wire 估计的 floor | 同左 | max(转换前规范估计，转换后估计)；不入请求体 | wire_images.rs、wire_budget.rs、stream.rs | 本轮 14 个新测试 + 红绿对照 |
| InputAudio / 助手图片、音频历史 | — | — | 明确拒绝（记为限制） | request_content.rs:247-269 | I request_content 转换测试 |
| Reasoning（encrypted/opaque、签名） | 版本化 envelope 同源回放 | 同左 | 端点/模型/授权域变更不回放；密文不跨厂商 | reasoning.rs、transport_identity.rs | W scope_replay_tests、identity_replay_tests、I reasoning_projection_tests |
| WebSearchCall（引用块） | 丢弃（Chat 无宿主搜索） | server_tool_use/web_search_tool_result 回放 | 仅 Anthropic 宿主搜索保留；引用不投影为通用 annotations（记为限制） | hosted_tools.rs、hosted_replay.rs | W pause_turn_tests、hosted_tools_wire_tests |
| 结构化输出 text.format{name,strict,schema} | response_format.json_schema | output_config.format | Chat 保留 name/strict；Anthropic 无 name/strict（记为限制） | convert_request.rs:111 | W wire.rs::output_schema_survives_first_tool_turn |
| effort / verbosity / tier / store / cache key | Chat 部分保留 | 仅语义重叠项跨线 | 无等价字段的明确不转发并记日志（见限制） | convert_request.rs:33-75、285 | W wire.rs::anthropic_wire_carries_effort_and_service_tier… |
| usage（缺失/部分/显式零/cache） | 聚合+终态权威 | start/cache 计数规则 | 终态权威；缺失失败流；显式零覆盖 | usage.rs | W usage_tests（含 cache、显式零、中间累计、缺 usage） |
| cap/finish/错误/取消/重试 | CapExhausted 终态 | 同左 | 截断不可被 finish_reason 掩盖；取消即断流；重试按 provider 策略 | request_retry.rs、stream_pump.rs | W max_tokens_tests、terminal_tests、error_tests、cancellation_tests、request_retry_tests |
| 字节/上下文预算 | 32 MiB + 窗口 + 输出预留 | 同左 | 发网前拒绝；typed 图片按视觉而非文本计费 | wire_budget.rs、wire_images.rs、transport.rs | I wire_budget*/wire_images* 全套 + 本轮 floor 套件 |

**明确限制（非实现遗漏，协议无等价或未纳入本轮）**：OpenAI 专属
`reasoning.summary/context`、`include`、`client_metadata`、`access_programs`、`logprobs`、
`stream_options`；Anthropic 不收 OpenAI `store/prompt_cache_key/verbosity` 与输出 schema 的
name/strict；搜索引用块不展开为通用 annotations；音频输入/输出、模型生成图片、通用
PDF/Files 上传生命周期、厂商私有参数不承诺适配；rig `Document/FileId` 能力未接入为
通用上传。不能宣称三协议任意字段完全无损，也不把 OpenAI 内部字段发给第三方。

## 3. 预算下界合同要点（实现摘录）

- floor 只在转换线计算（`stream_via_rig_attempt` 构造 `WireBudget` 时，基于截断后的
  当前输入）；Responses passthrough 显式 `vision_floor_tokens: 0` 并注明原因（wire 保留
  original，walk 已用同一估算器计费）。
- 计费为 `max(walk_vision_sum, floor)`：小原图（1 patch）floor=1，不降低 walk 的 1844；
  多原图求和；文本内伪造 data URL 不参与（仍按文本）。
- 每请求/每 attempt 重算，压缩重建与暂停续跑不沿用旧值。

## 4. 最终二进制与三协议实网收据

- 源码：Codex `test@c4d08f71b`（= 99397fbd7 + 本轮 floor 修复），工作树 clean；
  验机镜像 9800 个 tracked 文件逐文件 sha256 与本地一致（0 差异）；ACP 镜像 1359 个
  tracked 文件一致（0 差异），vendor/ 保留。
- 构建：验机 1.95.0-x86_64 工具链，`cargo build --release -p codex-cli -p codex-exec
  --features codex-core/rust-rig`（既有 codex-cli network_policy 未使用警告不变）。
  - codex `bdb4d3b78199b12d9b8edd7e7de7e45468d548de567a29227f611646f52e4156`
  - codex-exec `be37c234e3e4a891581801ef5215f539cacbdf8cf1495019d80027edc8c21180`
  - binary-receipt.json 将两 SHA 与本次用例绑定。
- 定向回归（验机，pinned 工具链）：`cargo nextest run -p codex-protocol
  -p codex-rust-rig-bridge --offline --retries 0 --no-fail-fast --test-threads 4`
  → **702/702，0 跳过**（mac 同命令 706/706；平台门控差异）。
  Clippy：mac 上以同版本 pinned 工具链 `--all-targets -D warnings` 通过；验机 rustup
  工具链 manifest 损坏且安装组件 panic（环境缺陷，登记；不影响源码锁定）。变更文件
  rustfmt 与 `git diff --check` 通过。
- **实网（预算 90，实际 54，denied=0，activeRequests=0，rc=0；无重试凑绿）**：

| 用例 | 协议/模型 | 结果 | 明细 |
| --- | --- | --- | --- |
| env-determinism | 本地哑端点 | PASS | 0 真实请求 |
| acp-workflow@MIMO/chat | MiMo `mimo-v2.6-flash` | PASS | 12/12 步，18 次 |
| acp-workflow@GLM/responses | GLM `GLM-5.3-Flash`（Responses 线） | PASS | 12/12 步，18 次 |
| acp-workflow@STEP/anthropic | Step `step-5-preview` | PASS | 12/12 步，18 次 |

  每条 12 步：中文对话→文件写（隐藏 nonce 读回）→shell→MCP 截图（真值账本+结构化
  结果与 typed 图并存）→流中取消→续话→原生压缩（usage 下降可观察：如 MIMO
  10825→6653）→压缩后工具调用→无孤儿停机→冷恢复 usage 一致→恢复后工具调用；
  BASE64_RUN 检查通过（图片走结构化块，未塞文本）。
  三家窗口按官方公布 1M 的保守下界 200000 设置（`CODEX_MODEL_CONTEXT_WINDOW`，
  生效 190000=95%）；输出上限 2048。旧 GLM 12 步收据与 rig-events 回放未用作本轮证据。
- 凭据：实网运行后对 live-run3 全部工件+日志按三把密钥原文/base64 扫描 0 命中；
  验机临时凭据文件已删除；无 budget-gateway/tsx/app-server 残留进程。

## 5. 用户启动方式（不变，详见 nuwax-rig-quickstart-2026-10-18.md）

```bash
export NUWAX_BASE_URL=… NUWAX_WIRE_API=chat|responses|anthropic NUWAX_MODEL=… NUWAX_API_KEY=…
export CODEX_PATH=/path/to/codex-rs/target/release/codex   # 指向 codex CLI，勿用 npm 附带旧版
cd /path/to/fork-codex-acp-ts && node --import tsx src/index.ts
```

四变量成组提供；自定义型号按厂商公布窗口设置 `CODEX_MODEL_CONTEXT_WINDOW`（本轮实测
三家为 1M 窗口级别，取 200000 保守下界即正常使用）；显式 CLI/provider 覆盖优先。

## 6. CI 复跑收据

### 6.1 run 39130433046（head `c4d08f71b`，floor 修复推送触发）

- bazel bridge gate：✔ success。
- **ubuntu（22921 tests，2 failed + 1 timed out）**：
  - `env_group_stream_interrupt…two_resumes` — **本轮范围**：中间提交 9b51c6289（Chat 纯文本
    段合并，有真实厂商对照与自身红→绿）打破了该测试"prompt 必须整串等于一条 user 文本"的
    旧假设；CI panic 载荷显示 prompt 以 `…environment_context…\n\nstall this turn` 完整到达。
    修复 `7fd7a7b1d`：按段（`\n\n` 分隔）精确匹配，逐段精确性保留；盒上红→绿复核（旧断言
    CI 三平台红 / 新断言 2.4s PASS）。
  - `external_auth_refreshes_on_unauthorized::discovery` 60s 超时 — **既有失败，非本轮与
    中间提交所致**（证据：同样出现在 run 39098893052@fee6cda7d 与 run 39081573074@60956fb10
    的三平台日志；盒上 b514cd6cd（=fee6cda7d+docs）3/3 确定性超时；**上游 cb03f28a4 同机
    同环境 2.6s PASS** → fork 历史回归，窗口 2026-10-10（账本最后一次绿）..10-17，位于
    ChatGPT 认证刷新/discovery 路径，与 rig/NUWAX 无关；historical-open-tests.tsv 原有
    TMT/needs_current_evidence 登记）。归为开放项，不在本轮追修。
  - `shell_snapshot…blocked_descriptor_path` — 已登记抖动家族（#33 红 / 上轮绿 / 本轮红）。
- **windows（21753 tests，17 failed + 1 timed out）**：= 已登记 windows 尾项
  （unified_exec legacy ×4、cur ×4、uds ACL ×2、conpty、approvals popup、external_agent_config）
  + 上述两个跨平台项（env_group 已修、external_auth 既有）。无新增 windows 专属家族。
- **macos**：Format/Clippy/构建步骤全过；workspace 测试跑到 19664/22871（尾段全 PASS）后
  job 3h01m 超时被取消 — runner 容量问题（上轮同 job 2h29m 完成），非产品失败；取消前已
  出现的 daemon 系列超时与 `token_budget_auto_compact_fallback` 失败视为饥饿抖动，
  ubuntu/windows 完整运行均无对应失败，随下次 run 复核。
- **上轮 4 项登记的复核**：hooks、root_metadata（bwrap 声明）、host_roots 本轮 CI 全绿
  （中间提交 170de0c0e 的夹具修复生效）；开放项 1 `uploaded_original_image_history_compacts`
  已被 14a19eefb 的夹具重写取代（测试名不复存在，本轮三平台无此失败）。
- **账本更正**：此前两轮的 CI 归因只提取了 FAILED 家族、漏了 TIMEOUT 家族，导致
  external_auth 的超时未入账。本轮起 TIMEOUT 一并计入失败清单。

### 6.2 run 39146938516（head `7fd7a7b1d`，测试修复推送触发）

- bazel bridge gate：✔ success。
- **本轮两项修复的 CI 判定：`env_group_stream_interrupt…two_resumes` 三平台全部 PASS**
  （相对 run 39130433046 消失）；floor 产品提交所在 run 39130433046 的 protocol+bridge
  相关测试全绿在本轮保持。
- ubuntu 剩余失败 = `external_auth…discovery` 超时（§6.1 已归因的既有 fork 回归）+
  `shell_snapshot…blocked_descriptor`（已登记抖动）+ `login_account_chatgpt_redirects_to_
  hosted_success_page`（新名，判定抖动：run1 含产品提交时 PASS、run2 相对 run1 的唯一
  差异是无关测试文件的断言更新、盒上 3/3 PASS）。
- windows 剩余失败 = 已登记尾项（unified_exec ×4、cur ×4、uds ACL ×2、conpty、approvals
  popup、external_agent_config、subagent_notifications、windows_elevated ×4 即上轮登记的
  elevated runner 握手家族）+ external_auth 超时 + 两个单轮首现抖动
  （auto_review remote_model_override、compaction metadata_enabled 超时；同上逻辑与历史
  无法归因本轮差异）。
- macos：连续第二次 3h job 时限被取消（runner 容量）；取消前 env_group 亦 PASS，其余
  失败均在 daemon 饥饿/已登记家族内。macos 完整通过需要拆分分片或提高 job 时限
  （上轮账本同建议），属 CI 基建项。
- 结论：**本轮引入/触及的所有家族在 CI 关闭**；剩余红项全部为已登记开放项或有独立
  抖动证据，与 rig/NUWAX 核心路径无关。

### 6.3 run 39176371908（head `3533bbf4a`，文档提交推送触发；代码与 7fd7a7b1d 一致）

- bazel ✔；三平台 `env_group_stream_interrupt` 保持 PASS。
- **ubuntu（22921 tests）到达本轮最干净状态：仅剩 2 个登记项**——`external_auth…discovery`
  超时（§6.1 既有 fork 回归）+ `shell_snapshot…blocked_descriptor`（登记抖动）。
- windows（21753 tests，22 failed + 8 timed out）：FAILED 18 全部为已登记尾项
  （unified_exec ×4、cur ×4、uds ACL ×2、windows_elevated ×4、conpty、approvals、
  external_agent_config、subagent_notifications）；TIMEOUT 除 external_auth 与上轮已判
  抖动的 compaction metadata_enabled 外，新现 6 个名字（detail_policies、code_mode、
  compact_remote ×2、windows_elevated git_pull、legitimately_large_dimensions）——
  本轮树与上轮代码逐字节一致（仅新增文档文件），且这些测试在上两轮（含 c4d08f71b）
  全部 PASS，判定为 windows runner 60s 界限的平台抖动，非产品回归。
- macos：连续第三次 3h job 时限取消；取消前仅 external_auth + daemon 饥饿超时族 + 一个
  hooks 复发（ubuntu 同期绿）。macos 完整化属 CI 基建项（拆分片/提时限）。

### 6.4 交叉审查轮自查发现与补修（提交 `52d283204`、`8443539b0`）

- **自查抓到一个真实合同缺口**：`CustomToolCallOutput` 与 `FunctionCallOutput` 共用
  `FunctionCallOutputPayload` 且桥经同一路径发送其图片，但 floor 遍历只匹配了后者——
  自定义工具结果里的原图不计入转换前下界。修复为双变体合并匹配；新单元测试在旧实现
  floor=0 红、修复后 5120 绿；mac 707/707、验机 332/332。
  （复查同时确认 `ResponseItem` 无其他可携带图片且被桥发送的变体：`McpToolCallOutput`
  只存在于 wire 输入枚举 `ResponseInputItem`，桥不处理；`ImageGenerationCall` 被转换
  明确跳过；AgentMessage 仅文本/加密。floor 覆盖集 = Message + 两种工具输出，恰为
  转换实际发图的全部来源。）
- 测试加固：floor 出站 e2e 的接受窗口按正/负例分离（负例保留 750ms 零连接证明窗，
  正例放宽到本文件既有的 IO_DEADLINE，规避文件内已记录的 macOS 慢速信任库初始化）。
- 推送后 CI 收据见 §6.5。

### 6.5 run 39205629146（head `646069aa2`，交叉审查轮两笔修复 + 文档推送触发）

本节由接手方独立复核后重写；上一稿只在 soddy 工作副本里存在过、从未推送，其两处
口径已更正（见末条）。

- **整体终态：`completed` / `conclusion=failure`**。"floor 测试全绿"不等于"CI 通过"，
  本 run 从未通过。job 级：`bazel bridge gate (ubuntu)` ✔；`nextest (ubuntu-latest)`
  ✘ failure；`nextest (windows)` ✘ failure；`nextest (macos-latest)` ⚪ **cancelled**
  （3h job 时限，连续第四次）。
- ubuntu 22922 run：22920 passed（26 slow）、**1 FAILED**
  `shell_snapshot…concurrent_replays_keep_independent_readers::blocked_descriptor_path`、
  **1 TIMEOUT** `suite::v2::account::external_auth_refreshes_on_unauthorized::discovery`
  （60.015s 后 `TERMINATING`）、48 skipped。即与 §6.3 相同的两个登记项。
- windows 21754 run：21735 passed（22 slow、323 leaky）、**17 FAILED + 2 TIMEOUT**。
  失败全部落在非 rig 区域：app-server v2（external_agent_config、plugin_list）、
  external-agent-migration `cur` ×4、guardian-v2 `post_answer_finality`、tui insta
  snapshot、uds ACL ×2、ConPTY、unified_exec ×3。与本轮树差异（floor 变体臂合并 +
  测试窗口拆分）无交集。
- **`codex-rust-rig-bridge` 在任何平台都没有失败项。** floor / wire_budget 相关 24 个
  测试名在三平台全 PASS，含 `canonical_floor_prices_original_custom_tool_output_images`
  与加固后的 `converted_originals_are_rejected_before_http_on_small_windows`
  （macos 3.040s）。macos 结果需 `gh run view --log` 才能取到（`--log-failed` 不导出
  cancelled job），取消前 rig-bridge 套件已跑完。
- 两个新增 windows 名字独立复核：`plugin_list…global_remote_catalog_cache_ttl` 与
  `post_answer_finality::exhausted_transport_retries_fail_closed_after_bounded_attempts`
  在此前三轮（39130433046 / 39146938516 / 39176371908）**三平台均 PASS**，
  单轮抖动判定成立。
- 对上一稿的更正：① external_auth 在 ubuntu 是 **TIMEOUT（60s 界限）而非 FAILED**；
  ② 必须登记 run 整体 `failure`；③ "三平台全绿"限定为 floor 测试集，不是整个 run。

### 6.6 接手复审抓到的第二个真实预算缺口（提交 `04beac00a`）

**缺口**：`canonical_vision_floor` 只对 `detail: "original"` 的图片计费，普通 high/auto/
low 一律贡献 0（旧实现 `original_detail_floor` 对非 Original 直接 `return 0`）。而传输层
取 `max(wire walk, floor)`。于是一张 Original（patch 估计 5120）配一张普通 high
（1844）：正确规范成本 6964，转换后 wire walk = 3688，`max(3688, 5120)` 只得到 5120
——**Original 抬高了 floor，floor 随即整体取代 walk，把普通图片的贡献从预算里删掉了**。
上轮 §6.4 的"覆盖集穷举"结论没错（变体臂确实齐全），缺口在同一臂**内部**的计价分支；
旧测试名 `canonical_floor_ignores_non_original_details_…` 把缺陷当预期行为固化，所以
自查没看见。

**红（首败证据，修复前）**：4 项失败。

| 测试 | 失败信息 |
| --- | --- |
| `canonical_floor_prices_every_image_the_request_carries` | `left: 0`, `right: 7376` |
| `canonical_floor_prices_a_mixed_original_and_ordinary_detail_request` | `left: 5120`, `right: 6964` |
| `canonical_floor_prices_a_custom_tool_original_beside_an_ordinary_user_image` | `left: 5120`, `right: 6964` |
| `mixed_original_and_ordinary_images_reject_before_http`（出站产品路径） | `Chat user: … got None` —— 请求**被真的发了出去** |

出站两项走完整产品路径（`stream_via_rig_with_context` → 真实转换 → rig SDK 序列化 →
localhost HTTP socket），Chat 与 Anthropic、用户图与 function 工具结果四种组合全覆盖。
正例 `…_reach_the_socket_together` 修复前即通过（它只约束"不得删图/降 detail"）。

**修复**：floor 改为对请求内**每一张**图片计价——Original 沿用有界头估计/保守上限，
其余用 prepared 估计（1844）。仍是与 walk 取 `max`、**绝不相加**；仍逐 attempt 从
`conversion_request.input` 取值（在 pause 截断与 hosted-replay 剥离之后），压缩已移除的
图片不会被复活。

**保持不变量**：无 Original 时 floor 恰等于 passthrough walk 的视觉总和，因此本改动
只会抬高本来就偏低的计费，对"仅普通图片"的请求**不可能引入任何新拒绝**。小 Original
（patch 低于 prepared）、压缩后纯文本、伪造 text data URL 三类误计场景均有测试守住。

**绿**：`codex-protocol` + `codex-rust-rig-bridge` **712/712**（上轮同范围 707，净 +5：
`wire_images_tests` 15→18——把固化缺陷的 1 个旧测试拆成 2 个再补 2 个混合场景；
`wire_budget_outbound_tests` 10→12——出站拒绝正例各 1）。clippy
`--all-targets -D warnings` 干净，rustfmt 干净，改动仅 3 个文件。

**边界声明**：这是**内部预算下界**缺陷的闭合，不冒称已复现厂商真实 tokenizer 溢出。
本轮按决定不跑实网，故不产生新实网收据；绑定 `c4d08f71b` 的 54 次旧实网证据不能被
改写为覆盖 `04beac00a`。

**一处明确的"协议不适用"判定**（避免误登记为字段遗漏）：Anthropic hosted web-search
replay 的图片块由 `ResponseItem::WebSearchCall.wire_blocks` 直接以厂商原生形状上线，
不经 Codex `detail` 语义，因此只由 walk 按 prepared 计费、不进 floor——不存在
original→high 塌陷，无少计。

## 7. 提交、推送与工作树状态

- **工作副本（接手时核实，与交接文档不一致处以实际为准）**：`nuwax-ai/codex` 在
  `/Volumes/soddy/…` 与 `/Volumes/zhitai/…` 各有一个**互相独立的 clone**，都在 `test`
  分支。soddy 副本停在 `3d1370d31`（纯文档、**从未推送**，其 §6.5 与 §7 即本节上一版），
  并带着旧 worktree `.claude/worktrees/merge-main-20261014` @ `cb2828ebd`；zhitai 副本
  是接手当天 13:24 从 origin 新建、零自有提交。按用户决定，**本轮及后续代码工作在
  zhitai 副本**；soddy 副本的未推送文档提交不 cherry-pick（会引入分叉历史），其内容
  已在本节按复核事实重写。旧 worktree 仍不得当最新源码使用。
- Codex `test`：`646069aa2`（= 交接时的 `origin/test`）之上本轮新增 `04beac00a` 与本文
  档提交。工作树 clean。
- ACP `dev`：`19207d3`（= 交接时的 `origin/dev`）之上本轮新增 `0930455`；`vendor/`
  未跟踪，须保留。本轮对 `958b63f..19207d3`（case@VENDOR/wire 路由、共享预算、
  显式窗口传播）的独立复核抓到 5 个真实旁路并全部闭合，结论登记在 §8。
- 构建位置（影响复现，登记备查）：本轮及后续 cargo 工作在 zhitai 副本，
  `CARGO_TARGET_DIR` 用仓库内 `codex-rs/target`，`CARGO_HOME` 指向
  `/Volumes/zhitai/cargo-home`（由 `~/.cargo` 完整复制，`cargo +1.95.0 metadata
  --offline` 验证过依赖闭包齐全）。两者都落在项目所在的 4T 外接雷电盘上；系统盘
  只剩 `~/.cargo`(5.0G) 与 `~/.rustup`(3.6G) 两份旧副本，未被本轮写入，是否清理由
  用户决定。
- 实网证据对应关系：MiMo Chat / GLM Responses / Step Anthropic 各 12 步、共 54 次上游
  请求（预算 90）绑定 **`c4d08f71b` 构建的二进制**，不覆盖 `52d283204`、`8443539b0`、
  `04beac00a`。任何新收据必须绑定真正使用的源码与二进制。
- 已完成（Linux/容器 + ACP + rig 核心路径）：三协议 Chat/Responses/Anthropic 出站、
  跨协议视觉预算下界（含 CustomToolCallOutput、含 Original 与普通图混合）、Chat 文本
  合并、停机晚注册线程、同 ID 恢复实例、MCP structuredContent 遮蔽、工具定义失败清空、
  超范围整数近似化。
- 仍阻断 / 未闭合（不属于 rig 核心路径，独立登记）：
  1. `external_auth_refreshes_on_unauthorized::discovery`——fork 历史回归，ubuntu 表现为
     60s TIMEOUT；上游通过、fork 失败，不得称纯环境问题。
  2. macos nextest 连续四轮撞 3h job 时限被取消，完整 macos 验收从未拿到（建议拆分片）。
  3. windows 尾项家族（ConPTY、unified_exec、uds ACL、elevated runner、cur 检测等）与
     `shell_snapshot…blocked_descriptor` 抖动。
  4. 验机 `swufe@192.168.32.163` 的 rustup 组件安装存在 manifest 损坏；pinned 1.95.0
     的 Clippy 以 mac 结果为准，不擅自改动仓库锁定版本。

## 8. ACP `958b63f..19207d3` 独立复核（提交 `0930455`）

复核对象是交接文档点名的三项："case 路由、共享预算、窗口传播没有旁路"。方法是从
`live-validation.py` 的唯一子进程 env 构造点出发穷举入口，而不是相信上一轮的提交说明。

**确认没有旁路的部分（有源码位置，不是推断）**：

- 子进程环境只有 `live_support.py:provider_env` 一个构造点（调用者
  `live-validation.py:147` 与 preflight `live_support.py:290`），没有任何 case 自建 env。
  它剥离 `NUWAX_`/`LIVE_`/`OPENAI_`/`ANTHROPIC_` 前缀与 9 个 `CODEX_*` 宿主覆盖项，
  注入的 Key 只有 loopback gateway capability。
- 独立 helper 一律先拒后跑：`vision-direct.py:13` → `require_gateway_env()`，
  三个 TS helper → `gateway-contract.ts:requireBudgetGateway()`，后者同时校验
  `NUWAX_BASE_URL` 必须等于 `{origin}/routes/{vendor}/{wire}`、`NUWAX_API_KEY` 必须
  等于 gateway token、origin 必须是 127.0.0.1 且 `redirect: "error"`。
- 预算扣减在 `budget-gateway.mjs:132` 于 `request()` 打开 socket **之前**同步完成并
  落 journal；3xx 在 `:146` 被拒，客户端无法靠跟随 Location 离开计量点。
- 路由后缀不旁路去重：`OVERRIDE_REJECTED` 让五个固定路由 case 拒绝 override；
  `main-cold-recall` 通过 `label.replace(...)` 精确落到**同一路由**的 `main-text` 会话。
- 窗口传播到 ACP：三个 TS helper 都用 `...process.env` 启动 agent
  （`acp-live-smoke.ts:47` 等），`CODEX_MODEL_CONTEXT_WINDOW` 与 `CODEX_PATH` 随继承链
  进入 app-server 子进程。

**抓到 5 个真实缺陷**（全部先红后绿，ACP `dev` 提交 `0930455`）：

| # | 缺陷 | 真实后果 | 首败证据 |
| --- | --- | --- | --- |
| 1 | `LIVE_WINDOW_<VENDOR>` 解析失败被静默丢弃 | 子进程按 Codex 默认窗口跑，收据却像已按厂商公布窗口配置；付费请求打在错误预算上 | `AssertionError: ValidationError not raised (declared='not-a-number')`；`'²'` 直接 `ValueError: invalid literal for int()` 崩溃 |
| 2 | `--only-failed` 不带 `--cases` 回落到 13 个默认 spec | 记为 `main-text-GLM-chat` 的失败被"重试"成 13 个 plain GLM/chat case：本轮从未选择的路由重新花钱，`results.jsonl` 追加早期收据里没有的标签 | 修复前 manifest 实际为 `['env-determinism', 'wire-chat', …]`（13 项）而非 `['main-text@GLM/chat']` |
| 3 | 重复 case 只按原始字符串比较 | 同一路由两种拼写（`main-text@GLM/chat` 与 `main-text@glm/chat`）得到同一行标签，付两次钱且后一行覆盖前一行 → 摘要少一项 | 新增守卫测试在实现前 `AttributeError`，实现后拒绝并报出冲突标签 |
| 4 | vendor token 从未校验 | ` GLM ` / `glm-mimo` 之类生成一个永不可能命中的路由，直到 gateway 调用才失败，且 §8-3 的标签解码依赖"vendor 必为大写标识符"这一不变量 | `Tuples differ: ('main-text', ' LOCAL_V1 ', 'anthropic')` |
| 5 | `DEFAULT_CODEX` 指向被禁止的 `merge-main-20261014` worktree；文件不存在时报错却指向凭据文件；`codex.is_file()` 为假时收据静默省略 `codexSha256` | 在旧 worktree 仍在的机器上会哈希并验收一个无人选择的（修复前）二进制；缺哈希的收据无法绑定构建 | `'CODEX_BIN' not found in 'live validation failed: credential env file or executable codex-exec is missing'` |

修复 4 与 3 互为前提：`spec_from_row` 靠"vendor 段必为大写"来区分
`wire-chat`（case 名本身以 wire 词结尾）与 `main-text-GLM-chat`（路由行），所以
vendor 必须先被强制成该形状，标签才能唯一回译。测试里
`test_routed_and_plain_row_names_are_told_apart` 与
`test_row_labels_round_trip_back_to_the_same_route` 钉住这条不变量。

**与 §6.6 同类的自查盲区**：`19207d3` 的提交说明写着
"window propagation, garbage ignored"——把缺陷 1 当成设计写进了契约测试
（`test_live_support.py` 旧断言 `"not-a-number"` → 不设窗口）。上一轮因此看不见它。
本轮把该断言改为必须拒绝，并在 README 里第一次写下 `LIVE_WINDOW_<VENDOR>` 的语义
（此前完全无文档）。

**明确登记为"设计选择/非缺陷"的项**（避免误报）：gateway 默认额度 96 与
`README.md` 一致，不存在文档与实现不符；`LIVE_WINDOW_*` 完全缺省时沿用 Codex 自身默认
是保留行为（`env-determinism` 的 LOCAL 三形状依赖它），只有"写了但用不了"才fail closed。

**我自己的实现也被测试抓回一次**：narrowing 过滤初版把 spec 字符串与 case 名集合比较，
`--cases main-screenshot --only-failed` 漏掉了记为 `main-screenshot@STEP/anthropic` 的
失败路由（`Lists differ: ['main-screenshot'] != ['main-screenshot@STEP/anthropic',
'main-screenshot']`）。规则最终定为：plain 名覆盖该 case 已记录的所有路由，routed 名
只钉自己那条，并写进 README。

**绿**：`python3 -m unittest discover -s scripts/live-validation -p 'test_*.py'`
**28/28**（14→28）；`node --import tsx --test scripts/live-validation/*.test.mjs`
**25/25**；`scripts/acp-validation/stress-contracts.test.mjs` **4/4**。后两者未改动，
用于确认驱动改动没有破坏网关与 workflow 契约。

**边界声明**：按本轮决定不跑实网，所以缺陷 1–5 的证据是**本地契约与本地假 upstream**
（`test_live_support.py` 的 localhost 网关 + fake exec，零付费请求）。54 次真实上游
收据仍只绑定 `c4d08f71b` 构建的二进制；`0930455` 之后的驱动若要声称覆盖新二进制，必须
重跑并另立收据。缺陷 2/3 的付费影响只有实网跑过才会计入真实账单，此处不冒称已量化。


