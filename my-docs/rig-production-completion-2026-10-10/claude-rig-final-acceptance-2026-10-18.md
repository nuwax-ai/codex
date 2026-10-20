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

### 6.7 run 39233543173（head `4ee470d5c`，`04beac00a` floor 修复 + 文档推送触发）

本 run 由 `4ee470d5c` 触发，代码增量相对 §6.5 只有一笔：`04beac00a`（另两笔
`a3e5da2f0`、`4ee470d5c` 为文档）。已核对 `04beac00a`、`52d283204`、`8443539b0` 都是
`4ee470d5c` 的祖先，所以这三笔的代码都在这轮里。

- **整体终态：`completed` / `conclusion=failure`**。job 级：`bazel bridge gate (ubuntu)`
  ✔；`nextest (ubuntu-latest)` ✘；`nextest (windows)` ✘；`nextest (macos-latest)` ✘。
  三平台日志均由 `gh run view --log-failed` 取到并存 scratch（未提交），失败全部计入
  共享预算，没有清空账本。
- **本轮最有价值的一条：macOS nextest 第一次跑到终态**。§6.1–6.5 连续四轮是 3h
  `cancelled`，mac 整树从未被完整观测过。这轮 `09:54:36Z → 12:44:03Z` 落在时限内，
  所以 mac 的整树结果第一次可以登记（见末条）。但 2h49m27s 距离 3h 上限只剩约 10 分钟
  余量，**一次 runner 变慢就会退回 cancelled**，拆分片仍然是需要做的事。
  **（此条写于 §6.10 之前，两处过头，按 §6.10 更正：`39205629146` 等四轮被取消是事实，
  但"mac 整树从未被完整观测过"不成立——同日更早的 run `39098893052`（head `fee6cda7d`）
  mac 腿 `completed/failure` 并给出完整 `Summary`，交接文档
  `claude-final-acceptance-2026-10-18.md` §7 已登记过它。本轮的准确口径是
  "取消四轮之后**重新拿到**一份完整 mac 结果，并且有一份可比基线"。）**
- **本分叉自有 crate 零失败**：把三平台 FAIL/TERMINATING 名单逐条对照
  `codex-rust-rig-bridge` / `codex-protocol` / `codex-api`，**一个都没有**。
- **`04beac00a` 的新增测试三平台全 PASS**（逐个点名取状态，非套件级推断）：
  `canonical_floor_prices_every_image_the_request_carries`、
  `canonical_floor_prices_a_mixed_original_and_ordinary_detail_request`、
  `canonical_floor_prices_a_custom_tool_original_beside_an_ordinary_user_image`、
  `canonical_floor_ignores_forged_text_data_urls`（由旧名
  `…_ignores_non_original_details_and_forged_text_data_urls` 改名）、以及两条出站实跑
  `mixed_original_and_ordinary_images_reject_before_http`
  （ubuntu 3.021s / windows 3.045s / macos 3.035s）与
  `mixed_original_and_ordinary_images_reach_the_socket_together`（0.171/0.083/0.104s）。
  净新增 5 个（3 + 2），与 §6.6 的 707→712 对账一致。
  这把 §9.1 里"等机器恢复再点名验证 floor 测试"这项**改由 ubuntu runner 覆盖**——
  但 ubuntu-latest runner **不等于**用户那台 `192.168.32.117`，那台机器的 HEAD sha、
  release build 与 `env-determinism` 仍未闭合。
- **ubuntu**：22927 run / 22925 passed（25 slow）/ **1 failed** / **1 timed out** /
  48 skipped。两条就是 §6.5 已登记的同一对：FAILED
  `codex-exec-server::exec_process shell_snapshot::…_keep_independent_readers::blocked_descriptor_path`、
  TIMEOUT `suite::v2::account::external_auth_refreshes_on_unauthorized::discovery`
  （`TERMINATING [> 60.000s]`）。**无新增 ubuntu 失败项。**
- **windows**：21759 run / 21730 passed（21 slow、277 leaky）/ **23 failed** /
  **6 timed out** / 138 skipped。日志里能点名复原的是 **19 个** failed 名字，与 Summary
  的 23 差 4 项——nextest 的 compact 进度输出在 CI 日志里把这 4 行覆盖了，**不猜名字**，
  登记为观测限制。与 §6.5 上一轮 windows 逐项对照：
  - 两轮都红（稳定复现、非本轮引入，且都在 rig 之外）：`external_agent_config_secondary_source…`、
    external-agent-migration `detect::sessions::cur` ×4、tui `approvals_selection_popup_snapshot`、
    `codex-uds` ACL ×2、ConPTY `conpty_ctrl_c_interrupts_powershell_foreground_child`、
    `codex-windows-sandbox unified_exec::legacy_*` ×3。
  - **上一轮红、这轮转绿**：`plugin_list…global_remote_catalog_cache_ttl` 与
    `guardian-v2 post_answer_finality::exhausted_transport_retries_fail_closed_after_bounded_attempts`。
    §6.5 把这两条判为"单轮抖动"，这轮给出正面证据，判定成立。
  - 这轮新增红：`codex-core::all suite::auto_review::remote_model_override_uses_catalog_model_for_strict_auto_review`、
    `codex-core::all suite::subagent_notifications::subagent_stop_replaces_stop_and_skips_internal_subagents`、
    `codex-core::windows_sandbox windows_elevated_*` ×4、
    `codex-windows-sandbox unified_exec::legacy_workspace_write_delete_is_limited_to_writable_roots`。
    均不在 rig/桥路径上。
  - **超时 2 → 6，其中 2 条需要谨慎对待，登记但不擅动**：老面孔
    `external_auth…::discovery` 与 `windows_elevated_approved_git_pull_preserves_deny_read`
    继续超时；新增的 `codex-core image_preparation::tests::detail_policies_apply_the_expected_budgets`
    与 `codex-utils-image tests::legitimately_large_dimensions_still_decode` 在上一轮是
    **45.351s / 47.057s PASS 且被标 `SLOW [> 30.000s]`**，这轮越过 60s 界限被判超时。
    归属查过：两个文件都由本分叉提交改过（`2f048c3b4`、`4a0dd66d5`，后一个名字本身就是
    `2f048c3b4` 加的），所以**不能说与本轮无关**；但相对上一轮的唯一代码增量 `04beac00a`
    只动 `wire_images*.rs` 与 `wire_budget_outbound_tests.rs`，不在这两条测试的执行路径上，
    且同轮 windows 墙钟 1h57m vs 上一轮 1h48m（+9%）。判定为**这两个用例本来就贴着 60s
    界限跑、runner 一抖就越线**，与 §6.4 里 `8443539b0` 处理的是同一类问题（那一笔把
    dispatched floor 腿换回 `IO_DEADLINE`）。本轮**不擅自改**：可选处置是给这两条单独放宽
    deadline（动 nextest 配置/属性，属测试配置变更），或承认它们在 Windows runner 上就是
    flaky 并保持观察。交用户决定。
- **macos（原文标签"首次完整观测，无基线可比"按 §6.10 作废）**：22877 run / 22868 passed（28 slow）/ **3 failed**
  / **6 timed out** / 71 skipped。3 failed =
  `suite::v2::guardian_v2::stateful_async::classifier_modes_preserve_approval_matrix::failed`、
  `suite::v2::multi_agent_v2_developer_instructions::cold_resume_preserves_effective_developer_instructions_for_worker::inherits_parent_developer_instructions_without_an_override`、
  `codex-tui app::tests::session_lifecycle_requests::startup_defaults_tests::fresh_startup_falls_back_only_for_unsupported_config_read`。
  6 timed out = `external_auth…::discovery` + `codex-cli::app_server_daemon` ×5
  （60s/120s/180s 三档界限都出现）。因为 mac 之前四轮全被取消，**这 9 项只能登记为
  "首次观测到"，不得声称是抖动、也不得声称是本轮引入**；唯一有旁证的是前两条落在
  §6.5 已登记过的 app-server/guardian 抖动区域。
  **（名单与计数不变；"无基线可比 / 首次观测到"这半句按 §6.10 作废——9 项里 3 项在上一手
  mac 腿 `39098893052` 同样超时（含 `::discovery`），属复现；另外 6 项才是 mac 腿新观测，
  其中 5 条 daemon 超时能用"mac 腿整体变慢 20–65 % + 期限贴线"量化解释。逐条定性与
  四分法归类别只看 §6.10。）**
- **`external_auth_refreshes_on_unauthorized::discovery` 这轮在三平台全部超时**
  （ubuntu / windows / macos 均 `TERMINATING [> 60.000s]`）。未闭合清单第 1 项把它登记为
  "fork 历史回归、不得称纯环境问题"——本轮证据与该判定**一致且更硬**：它不再是 ubuntu
  独有现象，而是三平台复现，所以更不能用"runner 抖动"解释掉。
  **（此处只修正判定的一半，2026-10-20 回读后由 §6.9 改写：三平台复现 + 16 ms 离散度
  确实把"抖动"解释排除了，但**排除抖动 ≠ 归属分叉**；该归属结论后来被证明拿不出凭据，
  已撤回。"不能称纯环境问题"这半句仍然成立。）**
- **结论口径**：本轮**没有**让 CI 变绿，也不打算靠重跑凑绿。它证明的是：`04beac00a`
  的 floor 修复及其 5 个净新增用例在 linux/windows/macos 三个真实 runner 上全绿，且
  失败集合与本分叉自有 crate 无交集。

### 6.8 用户拍定后的处置：给这两条图片用例单独放宽 Windows deadline（提交 `7b1a92c6a`）

用户决定"可以放宽时间超时时间"，所以 §6.7 登记的第 5 项开放项按**单独放宽**这条路径处理，
而不是承认 flaky 继续观察。

- **改动**：`codex-rs/.config/nextest.toml` 新增一条 `[[profile.default.overrides]]`，
  `platform = 'cfg(windows)'`，`filter` 只点名这两条
  （`test(detail_policies_apply_the_expected_budgets) | test(legitimately_large_dimensions_still_decode)`），
  `slow-timeout = { period = "1m", terminate-after = 2 }`。其余平台仍是默认 `30s × 2`，
  其余 Windows 用例不受影响；`1m × 2` 与文件里既有的
  `start_thread_uses_all_default_environments_from_codex_home` 同档，不是新发明。
- **为什么是 120s 而不是"再观察一轮"**：Windows 实测是 45.351s / 47.057s（PASS 且标
  `SLOW [> 30.000s]`），也就是**已经用掉界限的 75–78%**，下一轮 runner 稍慢就越线——
  §6.7 观察到的正是这次越线。放宽到 120s 后按最差实测算余量约 2.6 倍。
- **mac 侧对照实测（同一条测试，非 Windows）**：`codex-utils-image
  tests::legitimately_large_dimensions_still_decode` 两次
  `16.019s` / `17.304s`，`Summary 1 test run: 1 passed, 20 skipped`。
  即这条测试在快机上只需 1/3 的默认窗口，Windows runner 慢 2.7–2.9 倍是平台差异，
  不是测试本身变重。
  **口径补一句（§6.10 回读 CI 日志后加的，防止被误读成 mac CI 腿）**：上面这两个数是
  **本机外接盘上的 scoped nextest**（`-p codex-utils-image`，warm cache），
  不是 GitHub 的 macos runner。CI mac 腿同一条实测是 `34.707s`（本轮
  39233543173）与 `50.708s`（上一手 39098893052），`detail_policies_apply_the_expected_budgets`
  CI mac 腿是 `40.350s`。所以"1/3 默认窗口"只对**本机**成立；CI mac runner 已经贴到
  60s 界限的 58–85 %，这恰好也是 §6.10 第 2 类"mac 腿整体慢"的同一条证据。
- **配置验证（不靠"看起来对"）**：因为 `platform = 'cfg(windows)'` 在 mac 上整条 override
  被跳过、filter 不会被求值，我把 `platform` 临时改成 `cfg(unix)` 后重跑上面那条 scoped
  nextest——**nextest 正常启动并按该 filter 选中了目标用例**，证明 filter 语法与本仓库
  既有 override 的同名写法一致可用；随后把 `platform` 改回 `cfg(windows)`，
  `diff` 与改动前的备份逐字节相同，`git diff --stat` 仍是干净的 9 行新增。
- 两个测试名在 workspace 内唯一（`core/src/image_preparation_tests.rs:309`、
  `utils/image/src/image_tests.rs:517`），所以 `test()` 子串 filter 不会顺带放宽别人。
- **推送与 CI**：`7b1a92c6a test(rig): give the two measured image tests a
  Windows-wide deadline` 已推 `origin/test`（`b636829b5..7b1a92c6a`），
  **不带** `[skip ci]`——这是真实测试配置变更，按"完整 workspace 验证交 CI"的分工，
  由 run **39286738661**（`Fork Cargo PR`，head `7b1a92c6a`，
  `createdAt 2026-10-19T16:35:56Z`，登记时为 `queued`）验证。
  **Windows 腿这两条是否因此转绿，要等该 run 结束再登记，现在不预判。**

### 6.9 回读 §6.7 已下载的三平台日志：`external_auth…::discovery` 的定性要改写

这条是复审 §7 未闭合清单第 1 项时，从**已经在手的** run 39233543173 日志
（scratch `ci-39233543173/{ubuntu,windows,macos}.log`，未重新下载）里读出来的。结论：
该项此前的定性**证据不足**，而它一直没被根因有个结构性原因。

- **日志事实（逐条对到时间戳）**：同一测试的两个参数化在三个平台**表现完全分裂**——
  - `::responses` **PASS**：ubuntu `1.210s`、windows `0.938s`、macos `0.654s`；
  - `::discovery` 一律 `SLOW [> 30.000s]` → `TERMINATING [> 60.000s]` →
    `TIMEOUT`：**60.024s / 60.028s / 60.012s**（ubuntu/windows/macos）。
  三平台的超时值离散度只有 **16 ms**，而 §6.7 里那些"抖动"项的离散度是几十个百分点
  量级。所以这不是慢，而是**卡在某个等待上直到被杀**——按四分法应归**真实缺陷（挂起）**
  或 **harness 死锁**，不能再用"runner 抖动"解释。
- **为什么历来只拿得到一个名字**：`app-server/tests/suite/v2/account.rs:74` 把
  `DEFAULT_READ_TIMEOUT` 设成 **60s**，而 nextest 默认界限正好是 `30s × 2 = 60s`。
  也就是说**该用例内部任何一条 `timeout(DEFAULT_READ_TIMEOUT, …)` 腿刚开始等，就正好
  撞到 harness 的 kill**，它自己的 `Elapsed`/`bail!` 诊断永远来不及打印。同目录另有
  30s（`account_thread_usage.rs:27`）和 10s（`auto_env.rs:22`）两种写法，60s 不是惯例。
  这与 `8443539b0` 处理过的是同一类问题（用例自设期限 ≥ 界限 → 只能记成 TIMEOUT）。
- **归属查证（`git log --no-merges main..test -- <file>` 计数）**：
  `app-server/src/external_auth.rs`、`core/src/auth.rs`、`backend-client/src/client.rs`
  三个直接相关文件上的**本分叉提交数均为 0**；用例本身由上游 `0a5b99916`（#46281）加入，
  60s 常量由上游 `02a720525`（#18161）从 10s 改上来。
  因此 §7 第 1 项"**上游通过、fork 失败**"缺少任何可出示的凭据，已就地撤回。
- **但也同样不能宣布"与分叉无关"**：用例配的是自定义
  `model_providers.routing`（`requires_openai_auth = true` + 指向 mock 的 `base_url`）的
  出站，而 provider 出站形状正落在本分叉大改的 `codex-api` / rig bridge 射程内。
  现有证据只支持一个口径：**未定性的确定性挂起，待根因**。
- **能让 CI 给出根因的最小改动（已想清代价，但本轮不单独为它触发 CI）**：给该用例单独
  `slow-timeout = { period = "1m", terminate-after = 2 }`（120s），让内部 60s 期限**先**
  到期，日志里就会出现到底是哪条腿在等（`expected account/chatgptAuthTokens/refresh
  request, got …` 还是 `Elapsed`）。每条腿最多多花约 60s，mac 腿余量按 §6.7 是 ~10 分钟，
  可承受。按"不要为了重跑 CI 无意义提交"的约束，**并入下一次真实代码推送**一起做，
  不单独 dispatch。

### 6.10 macOS 那 9 项逐条定性：先纠正"无基线"，再分三类

按用户指令"先追 macos 那 9 项的首次观测定性"。**追的第一步是把"首次观测"这个前提本身
打掉——它不成立。**

- **前提纠正（有凭据）**：§7 未闭合项 2 写的是"macos 腿前四轮撞 3h 时限被取消、完整
  macos 验收从未拿到"。逐轮核对 job 结论：`39205629146`、`39176371908`、`39146938516`、
  `39130433046`、`38824922110` 的 mac 腿确实是 `cancelled`，但更早两轮是**跑完了的**：
  - run `39098893052`（2026-10-18，head `fee6cda7d`）mac 腿 job `117590909114`
    `completed/failure`，
    `Summary [3227.443s] 22840 tests run: 22836 passed (25 slow), 1 failed, 3 timed out`；
  - run `39081573074`（2026-10-18，head `60956fb10`）mac 腿 `completed/failure`，但**没有
    测试结果**——它在编译期就断了：`error[E0433]: cannot find module or crate libc`，
    `could not compile codex-core-plugins (lib test)`，`recipe test failed … exit code 101`。
  所以本轮 mac 腿**有一份可比的上一手基线**（`fee6cda7d` vs `4ee470d5c`，区间 19 笔提交），
  "9 项全是无基线首次观测"这个说法作废，下面按"复现"与"新观测"分开定性。
  取证通道：`gh api --allow-escape-sequences -X GET repos/nuwax-ai/codex/actions/jobs/<id>/logs`。
  **踩到的坑登记一下**：jobs API 的日志里是**真的 0x1B 转义字节**，与 §6.9 用的
  `gh run view --log`（日志里是字面 `^[[`）不同，剥离式必须换成
  `s/\e\[[0-9;]*[a-zA-Z]//g`；用错的那一轮所有正则静默 0 命中，差点被当成"该轮没有失败"。
- **交接记录本身也有一处漏登（顺手核对，不是本轮引入）**：交接文档
  `claude-final-acceptance-2026-10-18.md` §7 对同一 run `39098893052` 的 mac 腿写的是
  "**唯一失败** = 开放项 1（`uploaded_original_image_history_compacts`）"，
  而该腿日志实际是 `1 failed, 3 timed out`——三条 TIMEOUT（`::discovery`、
  `managed_starts_ensure_one_updater_and_recover_a_missing_one`、
  `restart_applies_saved_updater_preference`）没进那份名单。
  这直接影响本轮定性：正因为它们当时被漏掉，"#2/#3 是复现而不是新观测"这个结论
  只能从原始日志重读得到。**登记口径按原始日志修正，不沿用二手汇总。**

**九项矩阵**（A=本轮 39233543173 / head `4ee470d5c`；B=上一手 39098893052 / head `fee6cda7d`）

| # | 用例 | mac A | mac B | ubuntu A | windows A |
|---|---|---|---|---|---|
| 1 | `external_auth…::discovery` | TIMEOUT 60.012 | TIMEOUT 60.007 | TIMEOUT 60.024 | TIMEOUT 60.028 |
| 2 | `app_server_daemon managed_starts_ensure_one_updater_and_recover_a_missing_one` | TIMEOUT 60.010 | TIMEOUT 60.007 | PASS 7.342 | 不编译 |
| 3 | `app_server_daemon restart_applies_saved_updater_preference` | TIMEOUT 60.017 | TIMEOUT 60.008 | PASS 5.959 | 不编译 |
| 4 | `app_server_daemon explicit_restart_applies_feature_overrides_and_preserves_saved_settings` | TIMEOUT 60.008 | **PASS 54.902** | PASS 6.437 | 不编译 |
| 5 | `app_server_daemon packaged_daemon_explicit_replacement_migrates_running_legacy` | TIMEOUT 120.008 | **PASS 99.587** | PASS 41.847 | 不编译 |
| 6 | `app_server_daemon packaged_daemon_start_and_explicit_replacement` | TIMEOUT 180.014 | **PASS 138.699** | PASS 20.139 | 不编译 |
| 7 | `guardian_v2::stateful_async::classifier_modes_preserve_approval_matrix::failed` | FAIL 34.625 `Error: deadline has elapsed` | PASS 2.408 | PASS 8.315 | PASS 3.840 |
| 8 | `multi_agent_v2_developer_instructions…inherits_parent_developer_instructions_without_an_override` | FAIL 1.420 `NotLoaded` ≠ `Idle` | PASS 1.503 | PASS 3.419 | PASS 3.085 |
| 9 | `codex-tui …fresh_startup_falls_back_only_for_unsupported_config_read` | FAIL 16.716 `Error: deadline has elapsed` | PASS 2.940 | PASS 2.373 | PASS 2.509 |

**第 1 类：#1，确定性挂起，跨平台一致——沿用 §6.9 定性，本轮无新证据推翻。**
四条腿（三平台 + 上一手 mac）超时值 60.007/60.012/60.024/60.028，离散 **21 ms**；
同参数化的 `::responses` 在两条 mac 腿分别 0.654s / 0.580s **PASS**。这条**不是 mac 慢**，
是 §6.9 说的"内部 60s 读期限刚起步就撞 harness 60s kill"。归 **真实挂起 / harness 结构
缺陷待根因**，处置仍按 §6.9：诊断用的 `1m × 2` 单独期限并入下一次真实代码推送。

**第 2 类：#2–#6，mac 腿整体变慢把贴线项推过界——harness 期限窗口，不是功能回归。**
关键不是"mac 慢"这一句，而是**同一批用例在 mac 上的两次测量能对上**：
本轮 mac 上仍然 PASS 的同族 daemon 用例，相对上一手 mac 全线变慢 20–65 %——
`packaged_daemon_bootstrap_seeds_local_package` 68.405→**92.764**（+36 %）、
`packaged_daemon_restart_seeds_local_package` 61.314→**86.520**（+41 %）、
`managed_daemon_restarts_from_deleted_working_directory` 46.033→55.313（+20 %）、
`managed_identity_survives_locale_and_timezone_changes` 46.310→56.309（+22 %）、
`wall_clock_shift_keeps_server_and_updater_managed` 31.601→52.060（+65 %）。
同一区间里 ubuntu 侧的 #4/#5/#6 是 6.437 / 41.847 / 20.139s，按上一手 mac 的实测值算
**mac/ubuntu ≈ 2.4–8.5 倍**（54.902/6.437 = 8.5、138.699/20.139 = 6.9、
99.587/41.847 = 2.4）（这些用例要复制整个 debug 包再拉起新可执行文件，`cli/tests/app_server_daemon.rs:692`
`copy_executable(&daemon.codex, &package.join("bin/codex"))?`）。
于是 #4/#5/#6 上一手在 mac 是 **54.902 / 99.587 / 138.699s 的贴线 PASS**，本轮乘上 20–40 %
的 runner 变慢就越过 60 / 120 / 180。
**60s、120s、180s 三个界里，120s 与 180s 是本分叉自己在 `nextest.toml:19-27` 发的**
（`packaged_daemon_` 前缀 `1m × 2`、`packaged_daemon_start_and_explicit_replacement`
`1m × 3`），不是上游默认；上一手 mac 用 138.699s 通过，说明该期限当时就只留了 30 % 余量。
另有一条结构性事实：`cli/tests/app_server_daemon.rs:1` 是 `#![cfg(unix)]`，
Windows 腿**根本不编译这个 binary**——整条 `app_server_daemon` 家族在 Windows 只有两条：
`codex-cli::app_server_daemon_windows winget_package_starts_and_reselects_daemon`（PASS 0.031s）
与 `codex-cli::bin/codex tests::app_server_daemon_subcommands_parse`（PASS 0.081s），
两者都不复制 debug 包、也不拉起常驻 daemon。
所以这 5 条在 Windows 上没有任何覆盖，不能拿"windows 也绿"给它们背书。
归 **harness（期限窗口）+ 同一类内部期限风险**：每个 daemon 用例内部按腿设
`Instant::now() + Duration::from_secs(30)`（`app_server_daemon.rs:163/317/389/473/633`），
多条腿串行即可超过 60s 外界，与被杀时**一条用例自身的诊断都不打印**：本轮 5 条里
`explicit_restart_applies…`、`managed_starts_ensure…`、`restart_applies_saved…` 三条的
nextest 输出体只有 `running 1 test` + `(test timed out)`，
`packaged_daemon_explicit_replacement_migrates_running_legacy` 与
`packaged_daemon_start_and_explicit_replacement` 两条各多一行 libtest 自己的
`has been running for over 60 seconds`。
处置：**不改产品代码**；本轮不为此单独 dispatch。要么按 §6.8 的同一形状给 mac 腿单独放宽
（`platform = 'cfg(unix)'` + 这 5 个用例名），要么把 mac 腿当"慢 runner"另片跑；
两者都并入下一次真实代码推送，理由同"不要为了重跑 CI 无意义提交"。

**第 3 类：#7–#9，mac 腿真正的首次观测（3 条在上一手 mac 都是 PASS，ubuntu/windows 也都 PASS）。**
逐条能说到字段级，但**都给不出"分叉造成"的凭据**：

- **#7 `classifier_modes_preserve_approval_matrix::failed`**：`Error: deadline has elapsed`
  在 34.625s（≈ 启动 ~4.6s + 一条 30s 腿）。期限来自
  `app-server/tests/suite/v2/guardian_v2.rs:102 const TIMEOUT = Duration::from_secs(30)`，
  该文件与 `origin/main` **逐字节相同**；用例本身
  `guardian_stateful_async_tests.rs` 也与 main 相同，它按 `Snapshot`、`Conversation`
  **两条模式串行**（`:24-39`），每条内部又有多处 `timeout(TIMEOUT, …)`
  （`guardian_v2.rs:153/164/330/375/386/1009/1050/1115/1211/1241/1603/1671`）——
  30s 串行腿 vs 60s kill，与 §6.9 完全同一形状。同一参数化家族其余 11 个 case 本轮 mac
  全 PASS（多数 20.4–21.2s，另两条 `mandatory_review` 1.074s、
  `authorization_changed_during_classification` 8.320s），只有这一条 34.6s 越线；ubuntu 同一条 8.315s。
  → **harness/上游测试竞态**，非分叉缺陷。
- **#8 `…inherits_parent_developer_instructions_without_an_override`**：
  断言点 `multi_agent_v2_developer_instructions.rs:968` 的
  `assert_eq!(authority(resumed), authority(expected))`，diff **只有 `status` 一个字段**
  （实测 `< NotLoaded` / `> Idle`），其余 9 元组成员全等。时序不是它的敌人——
  本轮 1.420s vs 上一手 PASS 1.503s，**几乎同速**。`expected = baseline`（`:859`）
  取自"子线程还加载着"时的 `thread/resume`（`:691` 断言 `Idle`），
  而 `:836` 已断言同一条子线程在 `thread/read` 里是 `NotLoaded`；父线程恢复后再恢复子线程，
  mac 上状态仍报 `NotLoaded`，即**恢复路径的状态落地在 mac 调度下晚于响应序列化**。
  分叉归属查证：该测试文件与 main 相同；`app-server/src/request_processors/thread_processor.rs`
  分叉只有 8 行，两处都不在这条路上——一处是 `has_model_resume_override`
  里新增 `request_overrides.contains_key("model_provider")`，**对该用例是冗余**
  （上游已有 `typesafe_overrides.model_provider.is_some()`，而用例传的是**类型化字段**
  `model_provider: Some("mock_provider")`，`:715-726`，上游就已进入同一分支）；
  另一处是停机 drain（`:1307-1314`），不在 resume 断言路径上。
  → **上游测试/实现的竞态，mac 首发**；列为待复现观察，本轮不改。
- **#9 `fresh_startup_falls_back_only_for_unsupported_config_read`**：
  `Error: deadline has elapsed` 在 16.716s，命中
  `tui/src/app/tests/startup_defaults_tests.rs:624`
  `tokio::time::timeout(Duration::from_secs(/*secs*/ 15), proxy).await???;`。
  这是 `for capabilities in [ …(-32601), …(-32600) ]` 的**两轮串行**，每轮各带一个 15s 期限，
  而 15s 那一行与 `origin/main` **逐字节相同**（分叉在该文件的 diff 里 `from_secs` 出现 0 次）。
  该文件相对 main 只有 **1 行分叉改动**：helper `run_startup_for_test` 里
  `/*env_seed_overrides*/ Vec::new(),`（补 `App::run` 新形参的空值，语义与上游默认一致），
  不在这条用例的判定点上。ubuntu 2.373s / windows 2.509s / mac 上一手 2.940s 都 PASS，
  本轮 mac 16.716s 说明某轮 `proxy` 流在 mac 上 15s 内没走完。
  → **harness（15s 内部期限）+ 上游用例在 mac 首发**，非分叉缺陷。

**四分法口径与诚实边界**：#1 归"真实挂起（待根因）"，#2–#6 归"harness 期限窗口"，
#7–#9 归"harness 期限窗口 / 上游竞态，mac 首发"。九项里**没有一项能落到"本分叉造成的
字段遗漏或桥未实现"**；但反过来说"与分叉无关"同样没有凭据——上面逐条给的是"分叉提交数 0"
或"分叉 diff 不在该路径"，这只能排除归属，**不能证明无关**。另外：mac 腿一共只跑过两次
出结果，本轮 mac 与上一手 mac 之间夹了 19 笔提交（含一次 main 合并），
所以 #7–#9 的"首发"也可能是 runner 侧变化；本轮 mac 全腿 22877 项里 28 项被标 slow，
而同一对图片用例在 mac CI 腿两次方向相反的摆动——
`detail_policies_apply_the_expected_budgets` 25.995s → **40.350s**（+55 %）、
`legitimately_large_dimensions_still_decode` 50.708s → **34.707s**（−32 %）——
说明 mac runner 的时序方差本身就有 1.5 倍这个量级，贴线用例被推过界不需要任何代码变化。

**顺带抓到一条反向收据（不是本轮修的，但正好对上"第二平台才有结论"）**：上一手 39098893052
里 `codex-core::all suite::guardian_context_budget::review_respects_complete_context_budget::uploaded_original_image_history_compacts`
在 **ubuntu FAIL 1.368s、mac FAIL 1.274s**（windows 该用例不出现），本轮 mac/ubuntu 同名族
全绿。闭合方式是分叉提交 `14a19eefb` 把该 fixture **改名并重写**为
`oversized_original_inline_image_history_compacts`：旧 fixture 是"1×1 PNG + original 细节"，
新 fixture 改成真实 `2_560 × 2_048` PNG（5,120 patches，低于工具上限、高于评审 4,500 token
窗口）。口径要写清：**这条不是产品缺陷被产品修复关掉**，是分叉自有 fixture 的重写；
本轮真正的视觉 floor 修复是 `c4d08f71b` / `52d283204` / `04beac00a`，两件事不要混。

**风险登记（实测未观测，本轮不修）**：分叉提交 `4d9c62ae6` 在
`thread_processor.rs:1307-1314` 加了 `wait_for_shutdown_bound_tasks_bounded(Duration::from_secs(10))`，
实现在 `core/src/thread_manager.rs:1544-1567`——外层 `tokio::time::timeout(10s)` 里又调
`shutdown_all_threads_bounded(10s)` 再 `tracker.wait()`，即在**已有停机等待之后**再叠一段
最长 10s 的收尾。而 `multi_agent_v2_developer_instructions.rs:45-47` 的
`READ_TIMEOUT` 是 **Windows 25s / 非 Windows 10s**，同一用例又在 `:734` 用
`timeout(READ_TIMEOUT, app_server.shutdown_gracefully())` 包住停机——
也就是**分叉新增的最长 10s 恰好等于 ubuntu/mac 腿的全部容忍**。
实测四条腿（本轮 ubuntu/windows/macos + 上一手 mac）里
`final shutdown drain` / `shutdown_bound_tasks` / `exceeded final` 关键字出现次数**均为 0**，
所以这只是结构风险，不是已确认缺陷。按本轮范围（只修已确认缺陷）登记不动。

### 6.11 run 39286738661 收据：`7b1a92c6a` 的 Windows 放宽验证通过，但 mac 腿退回 cancelled

这条 run 是 §6.8 那次真实测试配置变更（`7b1a92c6a`，Windows-only `1m × 2`）触发的，
2026-10-19T16:35:56Z 起、`completed / conclusion=failure`。三平台 + gate 逐条：
`bazel bridge gate (ubuntu)` ✔；`nextest (windows)` ✘ `21759 run / 21735 passed
(24 slow, 244 leaky) / 21 failed / 3 timed out`；`nextest (ubuntu-latest)` ✘
`22927 run / 22925 passed (13 slow) / 1 failed / 1 timed out`；
`nextest (macos-latest)` **`cancelled`**（`16:39:21Z → 19:40:15Z`，正好 3h00m54s 撞 job 时限）。

- **§6.8 的放宽**必要且充分，有实测**（这条可以宣布闭合了）：两条用例在 Windows 腿
  `detail_policies_apply_the_expected_budgets` **PASS 60.423s**、
  `legitimately_large_dimensions_still_decode` **PASS 63.198s**。
  两个值都 **> 60s**，也就是**若仍按旧的 `30s × 2` 窗口，本轮这两条还会被判 TIMEOUT**；
  放宽到 `1m × 2` 之后它们在新窗口 120s 内用完 50–53 % 通过。
  顺带印证 §6.10 的口径：贴线不是 mac 独有的，Windows runner 上同一对用例也从
  45.4/47.1 涨到 60.4/63.2（+33 %/+34 %），与 mac 腿 +55 %/−32 % 的摆动是同一量级现象。
- **失败集合零新增**（这是本轮唯一想要的结论）：把 Windows 21 条 FAIL/3 条 TIMEOUT 逐条
  与上一手 `4ee470d5c` 的 23 条/6 条对照，
  **本轮有而上一手没有的：0 条**；上一手有而本轮消失的 4 条 = 上面那两条图片用例
  （由 `7b1a92c6a` 解释）+ `suite::auto_review::remote_model_override_uses_catalog_model_for_strict_auto_review`
  + `suite::code_mode::code_mode_excludes_mcp_servers_using_their_configured_identity`
  （后两条本次改动**碰不到**，只能记抖动，不冒领）。
- **ubuntu 腿两条不变**：`shell_snapshot…blocked_descriptor_path` FAIL 25.342s
  （§6.5 已登记的抖动家族）与 `external_auth…::discovery` TIMEOUT **60.012s**。
  `::discovery` 至此在本分叉已经累计 **5 条腿复现**（上一手三平台 + 本轮 ubuntu/windows，
  本轮 mac 腿在被取消前未跑到该用例），离散 60.007–60.042。**§6.9 的诊断期限仍未落地**
  （按约定并入下一次真实代码推送），所以这条继续挂"未定性、待根因"。
- **需要一处口径升级（§6.10 第 2 类的处置不完整）**：本轮 mac 腿被取消在 12699/22877
  （55.5 %），说明阻断项是 **3h job 时限本身**，不是那 5 条 daemon 期限。
  也就是说：即使按 §6.10 给 mac 腿单独放宽那 5 条，mac 腿**仍然跑不完**——
  放宽只是把"9 项观测"变成"更多项观测"，不解决终态。
  因此"mac 腿分片"从 §6.7 的建议升格为**待办开发项**（下一步做，见 §7 未闭合项 2）。

### 6.12 本轮 macOS / Windows 继续处理：实机二测、四类定性与三笔修复

用户指示：linux 腿暂缓（`.117` 不可达，等公司检查 IP/网络后提供机器），继续处理 mac 与
Windows 的错误。以下所有 Windows 收据都在验证机 `.235`（Windows 11 **26200.9711**，
即 §6.8/pty 那次"ConPTY 输入回归"探测用的同一台）上取得，scratch 目录
`~/codex-win-verify`，源码为 `7e1d23695` 的 `git archive` 解包，工具链
`1.95.0-x86_64-pc-windows-msvc`。全部构建产物落在该机 C: 与本机 4T 外接盘，不占系统盘。

**A. mac 腿按实测改限时（`efc27910b`，与 §6.11 的口径升级配套）**
度量而不是猜：mac 腿在同一步骤内付 **1h32m47s / 1h50m21s** 编译，run 39286738661 是在
12699/22877 起跑测试后 3m47s 被 3h 时限切掉。编译是固定成本且**在一个 job 内不可分片**，
所以同 job 内拆分救不了它。本轮把 `timeout-minutes` 改成 per-matrix（ubuntu 180 /
macos 330，windows 维持 150），并把上游 `nextest archive` + `--partition hash:N/4` 记为
持久解（未做：该分片的 target 目录不带 `codex-exec` source-receipt 二进制，需要先出方案）。

**B. Windows elevated 早退的机制级修复（`46971f9ad`）**
`windows_elevated_*` 5 条里有 4 条在 **0.066–0.072s** 退出。根因不是期限：
Windows 把被占用镜像的覆盖写报成 `ERROR_SHARING_VIOLATION`（os error 32），而 std 的
`decode_error_kind`（pin 的 1.95.0 源码
`library/std/src/sys/io/error/windows.rs:13` 起的表）**没有这一项的映射分支**，
所以它永远不会变成 `PermissionDenied`，既有那句"保留已 staged 副本"的守卫在这些腿上不可达。改为**字节比较**：源与目标一致时
按幂等处理跳过（与分叉自身 `tests/support/src/lib.rs` 里 runner 暂存的写法同构）。
诚实标注验证缺口：该文件整体 `#![cfg(target_os = "windows")]`，本地 cargo **编不到**；
表达式只在隔离 `rustc` 校验里通过（0 warning），第一条真实收据是 run 39341887096。

**C. 二实机把四族"runner 日志推断"换成了正向收据**
- `codex-uds`：桌面 **10 passed / 0 failed**，含 CI 上持续红的
  `private_directory_acl_is_user_only_and_inherited_by_files` 与
  `socket_validation_rejects_broad_acl_without_repairing_it`。→ runner 侧
  PowerShell `Microsoft.PowerShell.Security` 自动加载损坏属 **harness 缺陷**；
  分叉的控制通道 ACL 保证现在是被**实测证明**的，不再是"排除归属"。
- `external-agent-migration` 的 `cur::tests`：同一测试二进制，默认 TEMP 下
  **23 passed / 0 failed**；把 `TMP`+`TEMP` 指到一个 `codex-test-temp` 兄弟目录后
  **恰好是 CI 那 4 条 FAIL**（逐条同名），再换无横线兄弟目录又 **23/23**。
  → 触发点是**分叉自己的 workflow** 把 Windows 测试 TEMP 命名成带横线的
  `codex-test-temp`；`decode_cur_project_path`（`cur.rs:100`）只重并**一对相邻祖先**，
  所以祖先里的 `-` 使整条编码路径无法还原、检测返回空集。该解码器与 4 条用例在
  `origin/main` 上与本分叉**零 diff**，是上游缺陷：本轮只改 harness（`62568aab3`），
  不改上游语义，缺陷单独上报。
- `codex-windows-sandbox` legacy 家族：批内 **22 passed / 1 failed**，其中
  `legacy_capture_cancellation…`、`legacy_capture_emits_output…`、
  `legacy_tty_job_terminates…` 三条（CI 上 34.9s / 29.7s / 13.1s FAIL）在桌面 **全部 ok**
  → 记 runner 争用抖动，不冒领修复。机制也查到了：`unified_exec/tests.rs:905` 是
  `started_at.elapsed() < 10s` 的**硬墙钟断言**，而这个家族本来就已经在
  `windows_sandbox_legacy_sessions` 组里串行，所以**放宽 nextest 期限对它们无效**——
  判负来自机器级争用，不是测试间并发。这解释了为什么本轮没有给它们加窗口。
- `codex-utils-pty`：**25 passed / 1 failed**，唯一失败仍是
  `conpty_ctrl_c_interrupts_powershell_foreground_child`，在桌面上同样复现（见 F）。

**D. 两条 subagent 误读的纠正（都影响定性，必须记）**
1. 时长归属错读：`legacy_workspace_write_delete_is_limited_to_writable_roots` 曾被记成
   35.770s（实为 **0.139s**），derived 自 awk 字段错切。重做正则后确认。
2. "需要交互式控制台 runner"：被 `conpty_ctrl_c` 的失败反驳——它在真实桌面上也红。
   同理，write_delete 单条隔离 `--exact --test-threads=1` 跑 3 次：1 次
   `STATUS_DLL_INIT_FAILED (0xc0000142)`、2 次同签名 FAIL（`None, false`，helper 只写了
   `START` 没有 `SUCCESS`），所以它**不是**批内竞态；但测试与文件在上游同名存在、
   分叉零 diff，且 GH 两次运行一败一过 → 归 **上游既有 + 环境敏感**，本轮不修，只上报。
   这条同时纠正 §6.10 里我先前"fixture/temp 树生命周期竞态"的猜测：树没提前消失，
   `assert` 在 `TempDir` drop 之前。

**E. 新确认并修复的真实缺陷：guardian 指标发布顺序竞态（`ffab003d6`）**
`exhausted_transport_retries_fail_closed_after_bounded_attempts` 在 Windows 腿
`left == []`、右值 `("failure", Some("flex_unavailable"))`（run 39286738661，
上一手 39098893052 同条 PASS 0.694s，即抖动而非首现）。机制由源码证实：
分类任务先 `score_progress.fail_closed()`（`classification.rs:499`）再
`record_classification()`（`classification.rs:509`），而用例只等缓存分数、
然后就内联读指标 ⇒ 窗口天然存在。改用本模块已有的 `wait_for_outcome`
（另两条 sibling 用例已经在用），精确值断言不变，所以写错 `failure_reason` 仍会红。
macOS 收据：`post_answer_finality` **6 passed / 0 failed**，`cargo fmt --check` 干净。

**F. 三笔待办与一项需要拍的**
- Windows 腿 `codex-tui` 有 **3 条 ABORT**（`0xc00000fd` 栈溢出），两条 run 同名同态，
  ubuntu 同 3 条 **PASS** → Windows-only 栈深问题，非分叉引入。已在 `.235` 排入
  `RUST_MIN_STACK` 8 MiB vs 32 MiB 对照探针（结果待补）。
  → **已闭环，但两处口径要改**：探针结论是"栈预算不足、16 MiB 全绿"，已提交 `6e27a6c11`；
  "两条 run 同名同态"与"非分叉引入"都不成立。见 §6.13 R1。
  → **R1 的旋钮位置又被推翻一次**：`6e27a6c11` 的步骤 env 被 justfile recipe 覆盖，
  CI 仍跑 8 MiB；生效的修复是 `cefb81d57`（改 recipe 本身 + 回滚死代码）。见 §6.13 R5。
- `approvals_selection_popup@windows.snap` 过期确认：分叉提交 `280843aae`（D6）加了
  "Ask" 档并只刷新了非 Windows fixture（`git log` 显示基线 `.snap` 由该提交改、
  `@windows.snap` 最后改动仍是上游 `00d92ad3e`），CI diff 的 `+new` 侧正是
  Read Only / Ask for approval / **Ask** / Full Access 四档。已在 `.235` 用
  `INSTA_UPDATE=always` 真实渲染（`include_read_only = cfg!(windows)`，本地无法渲染），
  取回后与 CI `+new` 逐字节比对（见 §6.12 补充）。
  → **已闭环**：`.235` 真实渲染取回后与 CI `+new` 侧重建**逐字节相同**（959 B，`cmp` 通过），
  已提交 `59f3aa39b`；另外三个 `@windows*` fixture 由 Windows 腿 PASS 证明**不过期**，
  所以全腿只有这一份过期快照。见 §6.13 R2。
- ubuntu 独有的 `blocked_descriptor_path`（§6.11 的 B3）现在有了**方向**：失败原文是
  `exec-server transport disconnected: … WebSocket protocol error: Connection reset
  without closing handshake; failed to resume exec-server session: recovery timed out
  after 25s`，即 exec-server 进程在 bwrap 沙箱腿里**直接掉线**，25s 是恢复窗口耗尽而不是
  快照逻辑超时。mac 同一用例 **0.459–0.511s PASS**（走 Seatbelt，不经 bwrap），
  Windows 看不见这条，所以**必须等 Linux 实机**。到机器上的定向复现：
  `bwrap --unshare-user --unshare-net true` 先证实 user namespace 可用，再
  `RUST_MIN_STACK=8388608 cargo nextest run -p codex-exec-server --test exec_process -E
  'test(blocked_descriptor_path)' --nocapture`，看 exec-server 是崩在 spawn 还是被
  deny-fd 快照拖死。
- ConPTY `0x03` 在 24H2+ 不再生成 ctrl-c 事件：**需要你拍**。`7d9dc6834` 以"单次探测
  不足以确立 OS 级能力边界"为由撤掉了 `a63c08f0f` 的 build 门控；现在证据是
  GH runner 两次 + 本机 26200.9711 一次，同一签名。三个选项：
  (1) 恢复 build 门控（只改测试，Windows 转绿，kill 路径仍被覆盖）；
  (2) 改产品中断路径（`AttachConsole` + `GenerateConsoleCtrlEvent`，`a63c08f0f`
      实测从 owning process 走不通，风险高、工作量未估）；
  (3) 保持这条红（现状，Windows 腿不可能绿）。本轮未动，按你的选择执行。

### 6.13 本轮补充收据：栈预算、快照闭环、TEMP 归因的 CI 原生前后对照

接 §6.12。三笔新提交都已推送（`ffab003d6` + `62568aab3` + `59f3aa39b` + `6e27a6c11`
一次 push，head `6e27a6c11`，收据 run **39347345560**；在飞的 39341887096 head
`efc27910b` 是新 head 的**祖先**，其信息被新 run 完整覆盖，故未重复 dispatch）。

**R1. Windows `codex-tui` 三条 ABORT 的机制与修复（`6e27a6c11`）**

先纠正 §6.12 F 的两处口径：

1. "两条 run 同名同态"**不成立**。三条 Windows 腿的 ABORT 集合与时长都随 run 变：
   run 39098893052 是 **4 条**（多出
   `app_server_session::rollout_history::tests::remote_resume_restores_saved_server_profile_without_permission_overrides`），
   39205629146 与 39286738661 是 3 条；同一条腿三次时长分别是
   1.461s / 1.333s / 1.524s。→ 正确表述是"**栈预算落在阈值边缘，哪几条腿掉进去随 run 变**"，
   不是"这三个用例坏了"。这也正是栈预算类缺陷的签名。
2. "非分叉引入"证据不足（排除归属 ≠ 证明无关）。查到的**能查的部分**：三条腿的**用例**
   都由上游提交引入（`b588812e8` #49472、`ed32158e9` #38894、`4d7a5c7c7` #34199），
   且 `permission_selection_tests.rs` 对分叉**零 diff**；但 `app_server_session.rs` 有
   分叉 +36/−4（`config_request_overrides_from_config` 加三个 per-thread insert），
   `session_lifecycle_requests.rs` 有分叉 +1 行。要真正排除分叉，需要在 Windows 上
   再建一份 merge-base 构建做同栈对比（本轮成本不做）。修复本身是 **harness 栈预算**，
   两种归属下都成立，所以不阻塞。

度量（`.235`，同一份 `7e1d23695` 源码、同一 `target/`，先 libtest 再 nextest）：

| 栈预算 | libtest（三条分别） | nextest（CI 实际用的调度器） |
| --- | --- | --- |
| 未设置 | —（CI 即此状态：ABORT） | **ABORT**（含 retry，Summary `0 passed, 2 failed`） |
| 8 MiB | **三条全部 `0xc00000fd`** | **ABORT** |
| 12 MiB | 3 passed（3.56s / 17.97s / 2.53s） | 未测 |
| 16 MiB | 3 passed（1.62s / 6.99s / 1.01s） | **3 passed**（2 leaky） |
| 32 MiB | 3 passed（1.69s / 6.90s / 0.99s） | 未测 |

→ 需求在 8 MiB 与 12 MiB 之间；取 **16 MiB** 留余量。修复只加在 Windows 腿的
`Test (workspace)` 步骤（`env: RUST_MIN_STACK: 16777216`），ubuntu/mac 腿不动。
诚实边界：这是 **debug 构建 + MSVC 帧布局**的栈预算，不等于产品运行期会溢出
（本轮没做 Windows release 侧验证）。
**本段最后一句的落点已作废**：`6e27a6c11` 的步骤 env 会被 justfile 里的 recipe 赋值
覆盖，实际等于没改；16 MiB 这个数与度量都仍然成立，只是旋钮位置错了，见 **R5**。

**R2. 快照闭环，并且"只有这一份过期"被证明**

`INSTA_UPDATE=always` 在 `.235` 渲染 → `scp` 取回 959 B → 与 §6.12 里我从 CI diff
`+new` 侧重建的文件 `cmp` **逐字节相同**；内容 diff 恰好是插入 `3. Ask` 四行说明 +
`Full Access` 顺延为 `4.`，外加 insta 把头部 `source:` 从 `tests.rs` 写成
`tests/permissions.rs`（insta 只比内容，头部无害）。过期起点也钉死：`@windows.snap`
最后一次由上游 `00d92ad3e`（2026-09-22）刷新，"Ask" 文案由分叉 `280843aae`
（2026-09-27）引入 ⇒ 过期约三周。

另外三个 `@windows*` fixture 用 Windows 腿的 PASS 收据证明**不过期**（三处渲染都在
`#[cfg(target_os = "windows")]` 分支里，PASS 确实执行了它们）：

- `approvals_selection_popup@windows_degraded` →
  `approvals_selection_popup_snapshot_windows_degraded_sandbox` PASS 0.092s
- `permissions_selection_history_full_access_to_default@windows` → 同用例 PASS 0.077s
- `esc_interrupt_goal_paused_footer@windows` → `esc_interrupt_pauses_active_goal_turn` PASS 0.082s

所以 Windows 腿**唯一**过期快照就是已修的那份；这条预先排除省掉了一次 CI 循环。

**R3. uds 两条的归因翻案，与 cur / external_agent_config 的 CI 原生前后对照**

先记一个方法论坑：日志文件名里的 job id 不含平台信息，`39286738661` 的
`118175345095` 是 **ubuntu** 腿、`118175345058` 才是 **windows** 腿、`118175344562`
是 cancelled 的 mac 腿。我一度把 ubuntu 的"0 ABORT / 22925 passed"读成 Windows 腿的
结果，靠 `gh run view --json jobs` 才纠正。以后一律先映射 job→lane 再读。

拿到映射后做了一次**CI 自己的**前后对照（比桌面推断强）：横线 TEMP 根由
`8d5650e3d`（2026-10-16，"run 38782462743 把 C: 填满"那次的应对）引入。用它做祖先检查
选出最近一个**不含**该提交的 run —— **38528171490**（10-14，head `bc4517dad`）：

| 用例族 | 38528171490（无横线根） | 39081573074 之后（有横线根） |
| --- | --- | --- |
| `external-agent-migration cur::tests` | **PASS** 0.065–1.725s | **4 条 FAIL** |
| `external_agent_config_secondary_source_imports_session_and_plugin_end_to_end` | **PASS** 1.954s | **FAIL** 0.502s |
| `codex-uds lib_tests::socket_validation_rejects_broad_acl_without_repairing_it` | **FAIL 0.812s** | FAIL 0.605s |
| `codex-uds lib_tests::private_directory_acl_is_user_only_and_inherited_by_files` | **FAIL 1.254s** | FAIL 0.821s |

→ `cur` 4 条 + `external_agent_config` 1 条 = **分叉 harness 自己引入**（横线 TEMP 根）。
两条证据强度不同，分开记：`cur` 4 条既有 CI 前后对照，也有**桌面同二进制 A/B**
（4 FAIL vs 23/23，§6.12 C）；`external_agent_config` 这条**只有 CI 前后对照**
（PASS 1.954s → FAIL 0.502s，三条 run 一致），没有做过桌面 A/B，因为要另建
`codex-app-server` 的测试二进制，成本不付。它读的是同一批 Cursor 会话目录、
走同一个 `decode_cur_project_path`，机制同源，但这是**推断不是实测**。
→ **uds 两条与 TEMP 根无关**：横线根引入前就在红，且时长逐次相同（0.812s / 1.254s，
确定性而非抖动）。失败原文（`r39286738661` job `118175345058` stderr）是

```
thread '…socket_validation_rejects_broad_acl_without_repairing_it' panicked at
uds\src\lib_tests.rs:80:9:
Get-Acl : The 'Get-Acl' command was found in the module
'Microsoft.PowerShell.Security', but the module could not be loaded.
```

`lib_tests.rs:80` 是 `assert!(output.status.success())`，即**用例自带**的 PowerShell
ACL 探针在 runner 镜像上加载不了 `Microsoft.PowerShell.Security`。这两条用例由上游
`73e94ee7a`（#42326）引入，探针脚本与产品实现无关。分类：**harness / runner 镜像能力
边界**，不是协议不适用、不是桥未实现、也不是产品字段遗漏。产品侧保证已被桌面实测证明
（§6.12 C：`codex-uds` 桌面 10/0）。

本轮**没有**动它，理由是可选的两种改法都无法在 CI 外验证（桌面这台 PowerShell 正常，
复现不了 runner 的模块加载状态），而它们都要改上游安全用例的断言语义：
(1) 探针前置 `Import-Module Microsoft.PowerShell.Security` / `-ExecutionPolicy Bypass`；
(2) 去掉 PowerShell，改 `icacls` 或 Win32 `GetNamedSecurityInfoW` 直接读 ACL。
run 39347345560 会继续给出这两条的现状（预期仍红）。

**R4. windows-sandbox `legacy_` 家族：0.025s ↔ 12–36s 的落差，"争用"这个说法太粗**

桌面单跑（`.235`，16 逻辑核，机器空闲）：`legacy_capture_cancellation` **0.026s PASS**、
`legacy_capture_emits_output` **0.024s PASS**、`legacy_tty_job_terminates` **0.025s PASS**，
整包 22/23。CI（4 核 runner）同三条 **35.770s / 12.634s / 11.919s FAIL**，撞的都是
`unified_exec/tests.rs:905` 的 `elapsed() < 10s` 硬墙钟断言。
1400 倍的落差不能用"核少 4 倍"解释，所以 §6.12 C 的"runner 争用抖动"口径需要机制证据。
判定实验（同一台机器，起 24 个 `yes > /dev/null` 烧核进程后重跑 `test(legacy_)`）：

| 腿 | 空闲 | 24 烧核进程下 |
| --- | --- | --- |
| `legacy_capture_emits_output_and_preserves_descendant_after_normal_exit` | 0.024s PASS | 0.104s PASS |
| `legacy_tty_job_terminates_and_preserves_descendants` | 0.025s PASS | 0.083s PASS |
| `legacy_non_tty_cmd_emits_output` | （未单列） | **41.033s PASS（标 slow）** |
| `legacy_workspace_write_delete_is_limited_to_writable_roots` | 0.23s FAIL | 0.44–0.505s FAIL（同签名，非竞态，续 §6.12 D2） |

包 Summary：`22/23 tests run: 21 passed (1 slow), 1 failed`，耗时 46.340s（空闲时 15.845s）。

→ **争用是充分原因**：CPU 过量分配一条腿从 ~0.03s 拉到 **41.033s**，已经是那条 10s 硬
墙钟断言的 4 倍，与 CI 上 12.634s / 11.919s / 35.770s 的失败形态同类。所以不需要再假设
runner 有每条腿固定的 spawn 开销。
→ 由此得出下一步修复（`8349736da`，本地未推）：既有的 `test-group =
windows_sandbox_legacy_sessions` 只能阻止这个家族**内部**重叠，挡不住其余 315 个 binary
的测试一起抢 CPU；加 `threads-required = "num-test-threads"` 才是"这条腿跑的时候整机
独占"。语法按安装版本实证校验过：把值改成非法字符串时 nextest 明确报
`profile.default.overrides[12].threads-required: invalid value …, expected an integer,
the string "num-cpus" or the string "num-test-threads"`（源码
`nextest-runner-0.124.0/src/config/elements/threads_required.rs`），改回
`"num-test-threads"` 解析通过；只加 `platform = 'cfg(windows)'`，ubuntu/mac 调度不动。
诚实边界：这台 16 核桌面能证明**争用足以**打爆断言，但不能证明 GH 4 核 runner 上的
12–36s 全部由此产生；独占调度是否真的让它们转绿，第一条收据只能是 CI。因此这一笔
**压在 39347345560 出结果之后再推**，保持"一个 run 只有一个可归因 delta"。

**R5. `6e27a6c11` 的栈预算是死代码：真正生效的旋钮在 justfile（`cefb81d57`）**

复核时发现的**我自己的无效修复**，必须记下来。`justfile:8` 是上游
`rust_min_stack := "8388608" # 8 MiB`（`e8ba912fc`，2026-04-22，#19067），
而 `test` 的 unix / windows 两条 recipe 自己写 `RUST_MIN_STACK={{ rust_min_stack }}`
（`justfile:100` / `:104`）。recipe 内赋值发生在步骤环境之后，所以 workflow 里
`env: RUST_MIN_STACK: 16777216` 被覆盖掉，**CI 仍然按 8 MiB 跑**——也就是我在桌面上
实测三条 `codex-tui` 腿全部 `0xc00000fd` 崩溃的那个值。收据：

| 检查 | 命令 | 结果 |
| --- | --- | --- |
| 改前的 unix 渲染 | `just -n test`（本机） | `RUST_MIN_STACK=8388608 …`（与上游一致） |
| 改后的 unix 渲染 | `just -n test` | 同上，**未变**（证明只动 Windows） |
| 改后的 Windows 渲染 | `ssh -n win 'cd ~/codex-win-verify && just --evaluate stack_for_os'` | `16777216` |
| 改后的 Windows recipe | 同机 `just -n test` | `$env:RUST_MIN_STACK = "16777216"; …` |
| 死代码是否清干净 | `git diff 59f3aa39b -- .github/workflows/fork-cargo-pr.yml` | **空**（逐字节回到加 env 之前） |
| 还有谁在设这个变量 | `grep -rn RUST_MIN_STACK .github/ justfile scripts/` | 只剩 justfile 三处 |
| Windows 腿是否全走 recipe | `grep -n "cargo nextest\|just test" .github/workflows/*.yml` | p0 三条测试步骤都是 `just test`；唯一裸 `cargo nextest` 是 `live-tests.yml:80` 的 `codex-live-tests`，不含 `codex-tui` 腿 |

Windows 那台需要先装 `just`（`choco`/已有脚本，`just 1.59.0`，INSTALL-RC=0），并且
新的 justfile 已同步到 `~/codex-win-verify/justfile`；渲染值就是 CI 会用的值。

**副作用（对未来本地复现有用）**：在 mac 上不带 `RUST_MIN_STACK` 直接跑
`cargo nextest run -p codex-core --test all` 时，
`subagent_stop_replaces_stop_and_skips_internal_subagents` 两次都在 0.4s
`has overflowed its stack` / SIGABRT；改用 `just test` 后 **1.225s PASS**。
所以这个变量在 Unix 腿上同样是承重的，任何"CI 复现"都必须走 recipe 而不是裸 nextest。

对 39347345560 的推论（待该 run 的 Windows 腿证实）：它的 head 是 `6e27a6c11`，
栈预算**没生效**，所以三条 ABORT 预期仍在原位。这不是"重跑同样内容"——
`cefb81d57` 携带的是该 run 结构上不可能测到的 delta，因此必须推，而不是重新 dispatch。

**R6. `subagent_stop_replaces_stop_and_skips_internal_subagents`：2s 窗口改成 20s（`c27ec7653`）**

签名（两条 Windows 腿逐字相同）：
`Error: expected at least 2 entries in subagent_stop_hook_log.jsonl, got 1`。
同一 run 三条 lane 对照（`r39286738661`，各 log 含 ~378 处 Windows 标记以确证 lane 归属）：

| lane | 结果 | 时长 |
| --- | --- | --- |
| ubuntu | PASS | 1.377s |
| macOS | PASS | 0.814s |
| windows（39098893052 / 39286738661） | FAIL | 5.986s / 5.624s |
| windows（另一 run，10-19 05:38） | PASS | 4.643s |

`wait_for_hook_log` 的 `Duration::from_secs(2)`（`subagent_notifications.rs:347`）是
"第二次 `python3` hook 追加"的等待上限。失败腿总时长 ≈ 断言窗口 + ~3.6s，即窗口耗尽后
立即报错；而 Windows 也有一条 4.643s 的 PASS ⇒ **间歇且平台独选**，符合 runner 争用下
追加更慢，不符合"hook 确定性丢失"（那样每次 Windows 都会红）。所以分类是
**harness 计时窗口**，不是桥未实现也不是产品字段遗漏。

改动只放宽窗口：循环一到期望计数就返回，健康路径零成本；最坏 20 + 3.6 ≈ **24s**，
仍在 `[profile.default] slow-timeout = { period = "30s", terminate-after = 2 }`
（60s kill）内，不需要额外 override，也不需要动断言。mac 复跑 **1.225s PASS**，
`cargo fmt --check` 干净。诚实边界：`.235` 桌面本身复现不了 runner 争用（其他家族在
桌面上全部转绿），所以这条的 Windows 收据只能是 CI。

### 6.14 `legacy_workspace_write_delete`：从"环境敏感"改判为"边界确实失守"（未修，待拍）

这条是 §6.12 D2 遗留的 task #13。本轮把它从"上游既有 + 环境敏感"**改判**为
"Windows workspace-write 边界的真实缺口"。三个旧说法全部被实测推翻，逐个记：

**A. 推翻"helper 只写了 START 没写 SUCCESS"**
`.235` 上 7 次单跑（`--exact --nocapture`，0.20–0.27s），沙箱日志**每次都有**
`START:` 和 `SUCCESS:` 两行，cmd 捕获到的 stdout 为**空**——`del /f /q` 失败时 cmd
会打印错误行到 stdout，空输出说明四条 `del` 与 `rmdir` **全部成功**。

**B. 推翻"是 stat 假象"**
`fs::read` 给出的是 `Os { code: 2, kind: NotFound }`（父进程未沙箱化，它的视图是权威的），
且 `outside` 目录 `read_dir` 成功、内容为空 ⇒ 文件是**真被删了**，不是读不到。

**C. 推翻"祖先目录被上一次跑污染"**
先按此假设做 A/B：`codex-rs`（`sandbox_cwd()`，所有 Windows 沙箱腿共用的祖先）的 DACL
上确实挂着两条**继承用**的遗留 ACE（`S-1-5-21-2076975351-…`、
`S-1-5-21-2403252565-…`，均 `Modify, Synchronize` + `(OI)(CI)`）。用 `Set-Acl` 逐条删掉、
复查只剩 `SYSTEM`/`Administrators`/`soddygo` 之后，**原样（未打补丁的）测试仍 FAILED**。
⇒ 污染不是原因，这条假设作废。

**D. 实测到的边界行为是"能删不能建"的不对称**
在 scratch 副本上插桩（只改 `.235` 的那份解包源码，跑完已从 `.wdbak`/`.dbgbak` 还原并重新编译）：

| 观测 | 结果 |
| --- | --- |
| `compute_allow_paths_for_permissions` 的 allow 集 | 恰好 `workspace` / `temp` / `tmp` 三个声明根，**没有祖先** |
| deny 集 | 恰好 `workspace\.git`（`acl.rs` 的 deny mask 含 `DELETE \| FILE_DELETE_CHILD`，设计是对的） |
| 本轮 mint 的 per-root capability SID | 3 个全新 SID；`workspace` 上确实出现了**本轮**那条非继承 `(OI)(CI)(M)` ACE |
| ACE 调用是否报错 | 把 `let _ =` 改成打印后：**一条错误都没有** |
| 沙箱内 `echo > %USERPROFILE%\sb-escape-probe.txt` | **被拒**（父进程复查文件不存在） |
| 沙箱内 `echo > %OUTSIDE_DIR%\canary-create.txt` | **被拒**（`CREATE-OUTSIDE-DENIED`） |
| 沙箱内 `del %OUTSIDE_DELETE%` | **成功**（目录清空） |
| 沙箱内 `rmdir %PROTECTED_GIT_DIR%` | **成功**（事后 `icacls` 报"找不到文件"） |

⇒ 结论：workspace-write 在**新建写入**上是生效的，但在**删除已存在对象**上失效，
包括被显式列为只读载片的 `.git`。这台机器上确定性复现，CI 的失败签名与这里逐字段相同
（`(0,false,false,false,None,false)`）。命令实际以持久本地账户
`CodexSandboxOffline`/`CodexSandboxOnline` 运行（`whoami /groups` 在该 token 下直接崩，
Git-for-Windows 的 `whoami.exe` 报 `CreateFileMapping … Win32 error 5`——这是 Git 工具的
兼容问题，不是产品问题，但也意味着我拿不到 token 组成员列表）。

**E. 还差最后一环，且它落在产品侧**
"创建被拒 / 删除放行"在 WRITE_RESTRICTED 语义下不该同时成立，说明限制 SID 与实际删除
路径用的 SID 不是我推断的那一组。要把机制钉死，需要在**运行时** dump token 的
restricting SID 列表（`GetTokenInformation(TokenGroups)` 于沙箱子进程内），这已经超出
harness 范围，要动 `token.rs`/`acl.rs` 的可观测性甚至授权逻辑。

**F. 需要你拍的三个选项（本轮未动产品代码）**
1. **fork 侧补授权**：把 deny ACE 的覆盖面从"per-root capability SID"扩到沙箱账户 SID，
   或在 seed 阶段就把非根目录对象的 DELETE 权限收掉。风险最高，直接改安全边界。
2. **只上报上游 + 本轮保持红**（我的建议）：这条是上游引入的用例，测试与实现在本分叉
   **零 diff**，且 Windows 腿的这条红与 rig 集成无关；分叉没有义务在上游 ACL 模型里
   替它重做授权，但它是**真实可复现的 Windows 沙箱越界**，值得单独一份复现包
   （本文 A–D 的表 + `.235` 上的确定性步骤）。
3. **改测试断言**：`legacy_workspace_write_delete` 独红会让 Windows 腿不可能全绿，
   若只追求绿灯可以放宽那条 tuple——但这是**掩盖**，与"边界失守"的证据冲突，不建议。

**G. 对你机器的两处状态变更（登记）**：`.235` 的 `~/codex-win-verify` 是 scratch 树，
为做 C 的 A/B，我删除了 `codex-rs` 目录上那两条遗留的继承型 `Modify` ACE（只会收紧权限，
`SYSTEM`/`Administrators`/`soddygo` 原样保留）；插桩过的两个源文件已还原、测试二进制已
按还原后的源码重编译；`fixed-del-root`、`token-*.txt`、`acl-*.txt`、`patch_*.py`、
`aclprobe.ps1`、`aclstrip1.ps1` 与 `%USERPROFILE%\sb-escape-probe.txt`（实测从未被创建）
均已清理。

## 7. 提交、推送与工作树状态

- **工作副本（接手时核实，与交接文档不一致处以实际为准）**：`nuwax-ai/codex` 在
  `/Volumes/soddy/…` 与 `/Volumes/zhitai/…` 各有一个**互相独立的 clone**，都在 `test`
  分支。soddy 副本停在 `3d1370d31`（纯文档、**从未推送**，其 §6.5 与 §7 即本节上一版），
  并带着旧 worktree `.claude/worktrees/merge-main-20261014` @ `cb2828ebd`；zhitai 副本
  是接手当天 13:24 从 origin 新建、零自有提交。按用户决定，**本轮及后续代码工作在
  zhitai 副本**；soddy 副本的未推送文档提交不 cherry-pick（会引入分叉历史），其内容
  已在本节按复核事实重写。旧 worktree 仍不得当最新源码使用。
- Codex `test`：`646069aa2`（= 交接时的 `origin/test`）之上，本轮新增代码提交
  `04beac00a`（视觉 floor）与 `7b1a92c6a`（两条图片用例的 Windows deadline，§6.8），
  以及文档提交 `a3e5da2f0`、`4ee470d5c`（不带 `[skip ci]`：它们与代码提交 `04beac00a`
  在同一次推送里，按 GitHub"一次推送只在 tip 触发一轮"的行为，只产生了 §6.7 的
  run `39233543173`（head `4ee470d5c`）；`39205629146` 是更早、head `646069aa2` 的那轮，
  不是这两笔触发的）、`5262eb5e6`、`10b30ecd4`、
  `c7ece7dcc`、`b636829b5`、`0300c3b78`、`1605c3fb6`、`101c73b35`、`3439ae1c1`、
  `0037503db`（均带 `[skip ci]`，逐笔实测未触发新 run）。
  `origin/test` = `0037503db`；`7b1a92c6a` 触发的 run `39286738661` 已到终态并登记在 §6.11。
  本节这份文档提交在其后、同样带 `[skip ci]`，不额外占用 CI 预算。工作树 clean。
  **§6.12–§6.13 之后的实际指针状态（本次登记时核实）**：`origin/test` = `6e27a6c11`，
  本地未推的四笔是 `8349736da`（`legacy_` 整机独占调度）、`0c566acbf`（文档）、
  `cefb81d57`（R5 栈预算）、`c27ec7653`（R6 hook 窗口）。它们**刻意压在 run
  `39347345560`（head `6e27a6c11`，三条 lane 均 in_progress）出结果之后**：该 run
  已经花掉一轮预算，若它的 TEMP 归因或 elevated 修复仍未闭合，下一轮应当把那笔修正
  一起带上，而不是再单独烧一轮。R5 的推论（该 run 的三条 `codex-tui` ABORT 仍在）
  也在这笔推送后由下一轮给出收据。
- ACP `dev`：`19207d3`（= 交接时的 `origin/dev`）之上本轮新增 `0930455`、`147280b`、
  `d188807`、`59426b6`（三条分隔符耦合断言改平台中立，§9.5.1）、`6a76aa7`
  （`package-lock.json` 同步，§9.6）；`origin/dev` = `6a76aa7`，五笔均已推送；
  `vendor/` 未跟踪，须保留，实测仍在且未受影响。本轮对
  `958b63f..19207d3`（case@VENDOR/wire 路由、共享预算、显式窗口传播）的独立复核抓到
  5 个真实旁路并全部闭合，结论登记在 §8；跨系统实机验证（含 Windows 上确认的第 6 个
  真实缺陷、以及 Windows 上 ACP 产品套件的 3 项分隔符耦合红**及其转绿收据**）登记在 §9。
- 构建位置（影响复现，登记备查）：本轮及后续 cargo 工作在 zhitai 副本，
  `CARGO_TARGET_DIR` 用仓库内 `codex-rs/target`，`CARGO_HOME` 指向
  `/Volumes/zhitai/cargo-home`（由 `~/.cargo` 完整复制，`cargo +1.95.0 metadata
  --offline` 验证过依赖闭包齐全）。**三个变量缺一不可**：实测只设前两个时，一次
  712 测试的 nextest run 仍吃掉系统盘 3 GiB——Rust `tempfile` 走 `$TMPDIR`，默认落在
  `/var/folders/…`。因此测试 scratch 也必须显式 `TMPDIR=/Volumes/zhitai/tmp`，并在跑完
  后 `df -h /` 复查系统盘没有增长。系统盘上另有 `~/.cargo`(5.0G)、`~/.rustup`(3.6G)
  两份旧副本与 `/var/folders/…/T` 的 23G 遗留测试 scratch（20685 项，跨多套测试），
  本轮未写入、未清理，是否清理由用户决定。
- **自查抓到的一处同类偏差（§9.6 的副作用，登记并纠正）**：§9.6 那次
  `npm install --package-lock-only` 虽然在 mac 上执行、且**没有**安装任何依赖，仍然往
  系统盘的 `~/.npm/_cacache` 写了 **122 个文件 / 3.2 MiB**（packument 与索引条目）。
  实测方式：`find ~/.npm -type f -newermt '-6 hours'` 计数 + `du -ch` 求和。
  结论：**"没装依赖"不等于"没写系统盘"**。npm 没有仓库级覆盖变量，只能显式
  `--cache /Volumes/zhitai/tmp/npm-cache`——§9.6 的 `npm ci` 验证就是这么做的
  （那份缓存 37 MiB 全在外接盘），只有 lock 同步那一条命令漏了。后续任何 npm 操作
  一律带 `--cache`。系统盘当前 `df -h /` 余 22 Gi。
- 实网证据对应关系：MiMo Chat / GLM Responses / Step Anthropic 各 12 步、共 54 次上游
  请求（预算 90）绑定 **`c4d08f71b` 构建的二进制**，不覆盖 `52d283204`、`8443539b0`、
  `04beac00a`。任何新收据必须绑定真正使用的源码与二进制。
- 已完成（Linux/容器 + ACP + rig 核心路径）：三协议 Chat/Responses/Anthropic 出站、
  跨协议视觉预算下界（含 CustomToolCallOutput、含 Original 与普通图混合）、Chat 文本
  合并、停机晚注册线程、同 ID 恢复实例、MCP structuredContent 遮蔽、工具定义失败清空、
  超范围整数近似化。
- 仍阻断 / 未闭合（不属于 rig 核心路径，独立登记）：
  1. ~~`external_auth_refreshes_on_unauthorized::discovery`——fork 历史回归，ubuntu 表现为
     60s TIMEOUT；上游通过、fork 失败，不得称纯环境问题。~~
     → **"fork 历史回归 / 上游通过、fork 失败"这半句拿不出凭据，按 §6.9 撤回**：该用例
     由上游提交 `0a5b99916`(#46281) 引入，内部 60s 读超时由 `02a720525`(#18161) 从 10s
     改成 60s，且 `app-server/src/external_auth.rs`、`core/src/auth.rs`、
     `backend-client/src/client.rs` 三个直接相关文件上的**本分叉提交数为 0**。
     但也**不能反过来说与分叉无关**：用例配置的是自定义
     `model_providers.routing` 出站，正落在本分叉改过的 `codex-api` / rig bridge 范围内。
     现在口径是**未定性、待根因**，并且 §6.9 说明为什么现有 CI 结构上不可能给出诊断。
  2. ~~macos nextest 连续四轮撞 3h job 时限被取消，完整 macos 验收从未拿到~~
     → run 39233543173（§6.7）**第一次跑完**，2h49m27s 完成、结论仍为 failure。剩下的
     是它只带来 9 项**无基线可比**的首次观测（3 failed + 6 timed out，含 5 条
     `codex-cli::app_server_daemon` 超时），以及余量只有约 10 分钟、建议继续拆分片。
     ~~"第一次跑完"与"无基线可比"两处都不成立，按 §6.10 更正~~：更早的
     `39098893052`（head `fee6cda7d`，交接文档 §7 已登记）mac 腿跑到终态并给出
     `22836 passed / 1 failed / 3 timed out`，所以本轮 9 项里 **3 项是复现**
     （`::discovery`、`managed_starts_ensure_one_updater_and_recover_a_missing_one`、
     `restart_applies_saved_updater_preference`）、**6 项是 mac 新观测**。
     9 项已逐条定性完毕（§6.10）：#1 真实挂起待根因，#2–#6 harness 期限窗口
     （mac 腿整体变慢 20–65 %，且 120s/180s 两档是分叉自己发的；`#![cfg(unix)]` 使
     Windows 对这 5 条零覆盖），#7–#9 harness 期限 / 上游竞态在 mac 首发。
     **九项里没有一项能落到分叉造成的字段遗漏或桥未实现**，但也只到"排除归属"为止。
     本项剩下的不是定性，而是**要不要为 mac 腿单独放宽那 5 条 daemon 期限**——
     与 §6.9 的诊断期限一起并入下一次真实代码推送，不单独 dispatch。
     余量只有约 10 分钟、建议继续拆分片这句仍然成立。
     → **按 §6.11 再修正一次口径**：下一手 run `39286738661`（head `7b1a92c6a`）mac 腿
     又退回 `cancelled`（3h00m54s，只跑到 12699/22877 = 55.5 %）。所以**"单独放宽那 5 条
     期限"不足以闭合本项**——真正阻断的是 3h job 时限本身。本项现在拆成两个待办：
     (a) mac 腿**分片**（升格为待开发项），(b) 那 5 条的 unix 期限放宽（仍与下一次代码推送
     合并做）。定性与终态：定性已完成且结论不变，**mac 完整验收仍为未达成**。
  3. windows 尾项家族（ConPTY、unified_exec、uds ACL、elevated runner、cur 检测等）与
     `shell_snapshot…blocked_descriptor` 抖动。
  4. 验机 `swufe@192.168.32.163` 的 rustup 组件安装存在 manifest 损坏；pinned 1.95.0
     的 Clippy 以 mac 结果为准，不擅自改动仓库锁定版本。
  5. ~~**本分叉自有测试贴线**：windows 上
     `codex-core image_preparation::tests::detail_policies_apply_the_expected_budgets`
     与 `codex-utils-image tests::legitimately_large_dimensions_still_decode` 上一轮以
     45.351s / 47.057s PASS 并被标 SLOW，本轮越过 60s 界限判超时。两条都由本分叉提交
     改过（`2f048c3b4`、`4a0dd66d5`），处置（单独放宽 deadline / 承认 flaky 继续观察）
     交用户决定，证据见 §6.7。~~
     → 用户拍定"单独放宽"，已按 §6.8 实现（`7b1a92c6a`，Windows-only，`1m × 2`）。
     **此项在 CI 侧尚未闭合**：run 39286738661 结果待登记，Windows 腿这两条转绿之前
     不得声称已解决。
     → **已闭合（§6.11）**：run 39286738661 Windows 腿两条 **PASS 60.423s / 63.198s**，
     且该腿失败集合逐条对照上一手**零新增**，4 项转绿里有 2 项就是这两条（另 2 项与本改动
     无关，记抖动不冒领）。两个 PASS 值都 > 60s，反证旧窗口本轮仍会判 TIMEOUT，
     放宽既必要也充分。**注意本项闭合的范围只有"这两条不再被判超时"**：
     它没有让 CI 变绿，也不覆盖 mac 腿（该 mac 腿同一对用例是 40.350s / 34.707s PASS）。

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

## 9. 跨系统实机验证（ubuntu 24 + Windows，提交 `147280b`）

用户提供两台实机用于验证：`ubuntu 24 / 192.168.32.117` 与 `Windows / 192.168.32.235`。
两台都用独立 scratch 目录，不碰机器上既存的工作副本与容器；凭据只在会话内使用，不写入
任何文件、提交或记录。

### 9.1 ubuntu 24（`~/rig-verify-20261019`，全新 clone）

clone 发生在 `4ee470d5c` 推上 `origin/test` 之后，按推送顺序其 HEAD 应为 `4ee470d5c`，
但当场没留 `rev-parse` 输出，所以**这条按推断登记，不作为已证事实**。

- **工具链判正**：首轮非交互 SSH 报 `cargo MISSING`，据此写下"这台机器没有 Rust"——
  错。`bash -lc` 探测得到 `1.95.0-x86_64-unknown-linux-gnu` 与已安装的 `cargo-nextest`，
  与 `rust-toolchain.toml` 的 pin 一致。教训登记：任何"环境缺失"结论必须以登录 shell
  的探测为准，同一类错误还让 `nextest list` 一度返回空。
- `nextest`（`codex-protocol` + `codex-rust-rig-bridge`，含 rust-rig feature）**708/708，
  exit 0**。与 mac 的 712 差 4 项，逐项核对为 `codex-rs/protocol/src/permissions.rs` 中
  4 个 `cfg(target_os = "macos")` 权限测试；rig-bridge 子集两平台同为 **337**。差额解释
  清楚，不是漏跑。
- 诚实边界：机器在跑完 nextest 后掉线，所以不声称"floor 修复的 5 个新增测试已在**这台
  机器**上逐项点名验证"。§6.7 之后，"点名验证 floor 测试在 Linux 上为绿"这件事已由
  `nextest (ubuntu-latest)` 覆盖（5 个净新增用例逐个取到 PASS），所以那台机器恢复后要
  补的只剩两件：`git -C ~/rig-verify-20261019/codex rev-parse HEAD` 记进收据、
  release build + 取 sha256 + `env-determinism`。**runner 不等于这台实机**：内核、镜像、
  凭据环境都不同，不能用 CI 冒充实机产品路径收据。
- ACP 驱动套件（@ `0930455`）：python **28/28**、node live **25/25**、stress **4/4**。
  Linux 的 node reporter 是 TAP（`# tests`），不是 mac/Windows 的 `ℹ`，第一次按 `ℹ` grep
  得到空输出，改按 TAP 行确认后计数如上。
- **未完成**：`cargo +1.95.0 build --release -p codex-cli --features
  codex-core/rust-rig` 在 907 行 `Compiling`（已到 app-server）时机器掉线，此后
  ICMP 100% 丢包、22 端口超时，`env-determinism` 零付费实机用例因此**没有在 Linux 上
  跑成**。二进制未落盘确认，不声称任何 Linux 实机产品路径收据。机器恢复后需要重跑
  release build → 取 `codex`/`codex-exec` 的 sha256 → 跑 `env-determinism`，并用
  `147280b` 之后的驱动复跑 python 29 / node 26。

### 9.2 Windows（`/c/git-workspace/rig-verify-20261019/acp`，浅 clone）

clone 初始 HEAD 是 `0930455`，红→绿复跑时 `git pull --ff-only` 到 `147280b`。

- `npm install` 成功，postinstall 从 OSS 发布渠道取到
  **nuwax-codex 0.18.12 windows-msvc**（113 MB zip）——这条是 §9.3 lock 结论的正面证据：
  渠道上确实有 0.18.12，`package.json` 声明的也是 `^0.18.12`。
- 首轮 28 项里 **4 项红**，两类原因，都先用探针证实再改：
  1. **3 项 harness 无法在 Windows 落地**：fake exec 写成 `#!/usr/bin/env python3`
     脚本，`subprocess` 直接 spawn 得到 `WinError 193`。探针实测：`.cmd` 批处理**可以**
     被 `subprocess` 直接启动（rc 0、stdout 正确），shebang 脚本不行；且
     `os.access(path, X_OK)` 在 Windows 对任何存在的文件都返回 True，驱动那道
     "必须可执行"的闸门在 Windows 上等价于"必须存在"。修法：`write_fake_exec()` 按平台
     产出 `.cmd`（绝对路径解释器）或 POSIX 脚本，被测代码路径不变。
  2. **1 项断言写死了 POSIX 分隔符**：`CODEX_HOME` 断言期望 `/isolated/home`，而
     `str(Path("/isolated/home"))` 在 Windows 合法地给出 `\isolated\home`。改成与传入的
     同一个 `Path` 比较，仍然校验"驱动没有让父环境的 home 逃出去"。
- 修完这两类后仍剩 **2 项红**，且都是 `budgetDenied` 少计（`0 not >= 1`、`0 != 3`）。
  **这两条失败信息是假的**，真实原因用一个保留全部工件的诊断跑（`~/diag-run`）抓到：

  ```
  === fresh: rc=0        PASS wire-chat; 3/3; denied=0
  === resume: rc=1       live validation failed: budget gateway could not start
  === lock exists: True  {"pid":22788,"owner":"cb8f…"}     ← 持有者是已死的第一轮网关
  === summary: {'attemptsUsed': 3, 'budgetDenied': 0, ...} ← 上一轮的旧 summary.json
  ```

  即：Windows 的 `terminate()` 是 `TerminateProcess`，node 收不到 SIGTERM，
  `request-ledger.jsonl.lock` 不会被释放；第二轮 `--resume` / `--only-failed` 在网关
  启动处**按设计 fail closed**（网关不会自动接管别人的锁，这正是防双倍预算的那条不变量），
  但驱动没有把这次启动失败写进 summary，而是让调用方读到**上一轮遗留的 summary.json**，
  于是"预算拒绝少计"这种像是记账漏洞的现象被报了出来。
- **定性**：这是验收驱动的**真实缺陷**（第 6 个），不是产品 Rust 路径缺陷，也不是
  "Windows 不支持"。危害在于证据完整性——在 Windows 上任何 resume/fixup 的收据都可能是
  上一轮的；预算本身没有双倍花费（fail closed 正确）。mac/Linux 靠 SIGTERM 恰好掩盖了它，
  所以单平台自查看不见。
- **修复**（`147280b`）：网关新增 token 鉴权的 `POST /control/shutdown`，走与 SIGTERM
  相同的 `close()`→释放锁→退出路径；驱动先请求协作关闭，只在超时仍存活时才 escalate 到
  `terminate()`/`kill()`。该路由位于全局鉴权检查之后，未授权调用得到 403 且服务继续，
  为此补一条 node 契约（新路由不会给外部一个掐掉验收跑的入口）。新增
  `GatewayShutdownContract` 钉住不变量：一轮正常跑结束后锁必须消失，`--resume` 必须真的
  连上网关并把耗尽预算的两次请求计为 denial（顺带记正一条口径：
  `attemptsUsed` 只统计 `admitted`，被拒绝的请求不占"已用"）。
- **绿**：Windows python **29/29**、node live **26/26**、stress **4/4**；macOS 同步
  **29/26/4**。红→绿证据即上面的 `lock exists: True` + `could not start`，与修复后同一
  套件的 lock 断言通过。scratch clone 里 `package-lock.json` 被 `npm install` 改写过，
  按 §9.3 的结论**未提交**，保留作证据。
- Windows 侧另一条操作性事实：`live-smallbatch.sh` / `live-fixup.sh` 是 bash 包装，
  Windows 上直接调 `live-validation.py`；本轮没有为此改动脚本。
- 一处我自己的误判登记：第一次在 Windows 后台执行复合命令没有建出目录，我先怀疑主机；
  单独 `mkdir -p` 得到 `rc=0`+可写，说明是**我的命令**问题不是机器问题，随后改前台执行。

### 9.3 `npm ci` 在 `dev` 上失败（跨平台、影响发布渠道，需用户决定）

- macOS 上 `npm ci` 直接失败：`Invalid: lock file's nuwax-codex@0.18.3 does not satisfy
  nuwax-codex@0.18.12`。Windows 上 `npm install` "成功"，是因为它会**顺手改写 lock**：
  `1.2.12 → 1.2.18`、`^0.18.3 → ^0.18.12`、`0.18.3 → 0.18.12`。
- 也就是说 lock 是陈的，`vendor/nuwax-codex` 只是未跟踪的 Rust staging、不是 npm 源，
  而 0.18.12 在三处一致（`package.json` 声明、mac 本地安装、Windows postinstall 从 OSS
  取到的包）。
- **不在本轮擅动的理由**：`npm install` 在配了国内镜像的机器上把 `resolved` 从
  `registry.npmjs.org` 改写成了 `registry.npmmirror.com`。lock 里写死哪个下载域，是喂给
  用户 npm 发布渠道的供应链决定，不该由一次验证跑的机器配置顺带定下来。要同步请在
  **确定的 registry** 上显式做（`npm install --package-lock-only`），并核对 `integrity`
  字段。本轮只登记，不改。
- **严重性按实际影响定级（读 workflow 后更正直觉）**：`ci.yml`、`e2e.yml`、
  `publish.yml`、`publish-beta.yml` 全部用 `npm install`，**没有任何 job 用 `npm ci`**。
  所以这条陈旧 lock 不会让 CI 变红——它伤害的是"可复现安装"这条路：任何消费者（或将来
  改成 `npm ci` 的 job）按 lock 装就会直接失败，而 `publish.yml` 是 `npm install` +
  `npm publish`，实际发布出去的依赖树来自 runner 当场解析、不由这份 lock 约束。登记为
  **可复现性缺陷 + 误导性的 lock 元数据**，不是当前构建阻断项；是否同步 lock、用哪个
  registry，交用户决定。
- **后续（已按建议闭合，见 §9.6）**：用户拍定"按建议来"，即在
  **`registry.npmjs.org`** 上显式 `npm install --package-lock-only` 同步并核对 `integrity`。
  上面"不在本轮擅动"的理由仍然成立，所以本节保留原文，处置与收据登记在 §9.6。

### 9.4 用发布渠道的 Windows 二进制跑通 `env-determinism`（零付费）

`npm install` 的 postinstall 已经把渠道产物取到本地，所以这条不需要源码构建就能验证**
真实发布出去的二进制**：`vendor/nuwax-codex/0.18.12/nuwax-codex.exe`，
`codex-cli 0.18.12`，sha256 `9f5d80753d803684b56f083e081b9d38d3fe6d7172a793b3820402b043c32765`。

- 驱动结果：`PASS env-determinism … no real vendor request; actualAttempts=0`，
  `1 passed / 0 failed`，`denied=0`。零付费成立（凭据文件只写死向 `127.0.0.1` 的假路由，
  本轮没有向两台实机写入任何真实密钥）。
- `sink-receipt.json` 的 5 条记录逐条对齐，覆盖三协议与 Anthropic 三种 base URL 形状：
  `/v1/chat/completions`、`/v1/responses`、`/v1/messages`×3（bare、`/v1`、`/v1/messages`），
  `model=local-preflight`、`authenticated=True` 全成立。即**windows-msvc 渠道产物的
  NUWAX 路由路径/模型/鉴权三项决定正确**。
- 一处渠道一致行为（不是缺陷，但会影响驱动用法）：npm 包只发布单个入口
  `nuwax-codex(.exe)` + 子命令 `exec`，**没有 `codex-exec` 兄弟二进制**；mac 的
  `vendor/nuwax-codex/*/` 同样只有一个 `nuwax-codex`。Windows 上因此用 `.cmd` 包装把
  `exec` 补上，`codexExecSha256=501acbe76dde702e879387a0639c6646678711633b340defd53b23211ebdec80`
  哈希的是**包装器**、不是二进制，真正的绑定是 `codexSha256`。已把这层写进 ACP
  `scripts/live-validation/README.md`，避免下一个按 npm 渠道验收的人被
  "the codex-exec it names is missing" 挡住。
- 边界：这条证据证明的是 **0.18.12 渠道产物**的三协议路由，不含 `04beac00a` 的视觉
  floor 修复（该二进制里没有它），也不替代 §9.1 尚未完成的 Linux 实机跑。
  harness 沿用了既有的 `--dangerously-bypass-approvals-and-sandbox`，但提示词仅为
  `Reply with exactly: pong`、`CODEX_HOME`/工作目录隔离在 scratch、出站只到 127.0.0.1。

### 9.5 Windows 上的 ACP **产品**套件（vitest）：3 项红全是断言/快照与 POSIX 分隔符耦合

§9.2 覆盖的是验收驱动；这条是**产品测试套件本身**在非 POSIX 平台上的结果，之前没有
Windows 数据。命令与 mac 完全同一：ACP `d188807`，`npm test` =
`vitest run --no-file-parallelism`，日志 `C:\git-workspace\rig-verify-20261019\vitest-win.log`。

- **计数**：Windows `Test Files 2 failed | 62 passed | 7 skipped (71)`、
  `Tests 3 failed | 835 passed | 33 skipped (871)`；**同一提交同一命令**在 mac 复跑得
  `65 passed | 6 skipped (71)`、`839 passed | 32 skipped (871)`。
- **两个平台的差逐项可解释，不是环境吞了用例**：多出的 1 个 skipped 文件是
  `src/__tests__/CodexACPAgent/process-exit-error.test.ts:14` 的
  `it.skipIf(process.platform === 'win32')`——该用例的假 codex 写成 `#!/bin/sh` 脚本，
  作者已显式在 Windows 上跳过。于是 839 = 835 passed + 3 failed + 1 skipped 恰好对齐，
  文件数 6→7 也恰好对齐。（Windows 墙钟 86.07s vs mac 26.19s，平台速度差异，不影响判定。）
- **3 项红的来源逐条查实**（两类、三条，都只关乎路径字符串）：
  1. `CodexAcpClient.test.ts > prefers ACP additional directories over legacy meta roots
     for new session skill discovery`：`:509` 断言
     `extraRoots: ["/workspace/extra/.agents/skills"]`，而产品代码
     `src/CodexAcpClient.ts:934` 是
     `additionalRoots.map(root => path.join(root, ".agents", "skills"))`，Windows 上
     `path.join` 合法地产出 `\workspace\extra\.agents\skills`。
  2. `CodexAcpClient.test.ts > applies ACP additional directories to turn skill discovery
     and sandbox policy`：`:1100` 同一条期望、同一个成因。
  3. `load-session.test.ts > should recover response item function calls when app-server
     history omits tool items`：`:783` 的 `toMatchFileSnapshot` 与
     `data/load-session-response-item-history-fallback.json:259` 里存死的
     `Read file '/test/project/src/index.ts'` 不符；标题由
     `src/CodexToolCallMapper.ts:571` `title: \`Read file '${commandAction.path}'\``
     原样拼出、无归一化，路径则来自
     `src/ResponseItemHistoryFallback.ts:1030 return path.join(cwd, targetPath)`。
- **定性（按"协议不适用 / 桥未实现 / 真实字段遗漏 / harness 问题"四分法）**：这是
  **测试断言与快照基线跟 POSIX 分隔符耦合**，不是产品缺陷。ACP 代理在 Windows 上
  spawn 的是同机 Windows 版 codex 进程，`extraRoots`、`additionalDirectories` 和工具
  标题的消费者都只在本机解释这些字符串，`path.join` 给本机分隔符正是正确形状；协议
  字段本身对分隔符没有任何约定，所以也谈不上"字段遗漏"或"桥未实现"。
- **当时不修的理由（该取舍已由用户拍定并落地，见 §9.5.1；原文保留以便核对定性没有变）**：
  修法只有 (a) 把 3 处期望改成用 `path.join` 构造、并重新生成
  快照基线，或 (b) 在产品里做分隔符归一化。(b) 会改变发往本地进程的字符串形状，属产品
  行为变更，超出"只修已确认缺陷"的授权范围；(a) 要动产品测试套件的期望与快照基线，同样
  不在本轮范围内，而且快照要么按平台各存一份、要么存归一化形式，是个需要用户拍的取舍。
  当时登记为**已定性的测试可移植性缺口**。
- **一处值得留档的真实假设（不在上面 3 项红的范围内，也不是本轮缺陷）**：
  `ResponseItemHistoryFallback.ts:1030` 用**代理进程所在平台**的语法拼 `cwd`。若将来出现
  "ACP 代理跑在 Windows、app-server 跑在 Linux 容器"的混合拓扑，这个假设不成立。本轮
  没有任何证据表明该拓扑属受支持配置，§9.4 的渠道收据也不是它，因此只登记、不处理。
- **反向证据（排除"Windows 构建/类型层面有问题"）**：同一目录 `npm run typecheck`
  返回 rc=0（四条 `tsc --noEmit` 全过）。所以这 3 项红纯粹是运行时路径字符串，不是
  Windows 上的编译或类型差异。

#### 9.5.1 修复后收据：Windows 上 3 红 → 0 红（ACP `59426b6`）

用户拍定"按建议来"，即选项 (a)：**只改测试期望与快照比较方式，不动产品代码**。
修法是把三条断言从 POSIX 分隔符上解绑，`path.join` 的产品行为一个字没改。

- `CodexAcpClient.test.ts`：新增 `posixShape()` 递归归一化 +
  `extraRootsCalls(spy)`，两处断言由
  `expect(spy).toHaveBeenCalledWith({...})` 改为
  `expect(extraRootsCalls(extraRootsSetSpy)).toContainEqual({extraRoots: ["/workspace/extra/.agents/skills"]})`。
  选 `toContainEqual` 而不是 `lastCall` + `toEqual`，是为了**保住
  `toHaveBeenCalledWith` 原有的"任意一次调用命中即可"语义**——这是我第一版实现差点改错的
  地方（`expect(spy.mock.lastCall).toEqual(...)` 会静默收紧成"只看最后一次"）。
- `load-session.test.ts:783`：`toMatchFileSnapshot` 比的是文本，所以只在**序列化后的
  dump** 上做 `dump.replace(/\\\\/g, "/")`，把 Windows 上被 `JSON.stringify` 转义成 `\\`
  的分隔符折回基线形状。**规则刻意只做 `\\` 而不是任意 `\`**：基线里另有 4 处
  `\n`/`\t` JSON 转义（`data/…json` 的 211/243/280/312 行），若做全量 `\`→`/`，
  这三处在**每个平台**都会被改坏——那是把一个可移植性修复换成把基线改错。快照基线本身
  **未重新生成**，仍是一份 POSIX 形状。
- **首败→修复后（同一台 Windows 实机、同一命令、同一 node_modules）**：
  - 修复前（`147280b`，即 §9.5 那条）：`Test Files 2 failed | 62 passed | 7 skipped (71)`、
    `Tests 3 failed | 835 passed | 33 skipped (871)`，失败名逐条为
    `ACP server test > prefers ACP additional directories over legacy meta roots for new
    session skill discovery`、`ACP server test > applies ACP additional directories to turn
    skill discovery and sandbox policy`、`CodexACPAgent - loadSession > should recover
    response item function calls when app-server history omits tool items`。
  - 修复后（`59426b6`）：`Test Files 64 passed | 7 skipped (71)`、
    `Tests 838 passed | 33 skipped (871)`，**0 failed**，`Duration 80.32s`，
    且 `npm test` **进程退出码实测 0**。修复前那一轮没有单独记录退出码，但同一命令
    当时报 3 failed，按 vitest 语义必然非 0——这里只把**修复后**记为实测。
    `CodexAcpClient.test.ts (105 tests) 24519ms` 与 `load-session.test.ts` 均为 `✓`。
  - **算术闭合**：835 + 3 = 838，总数仍 871、skipped 仍 33，
    所以转绿的是**那 3 条本身**、不是有用例被跳过或消失；与 mac 的
    `839 passed | 32 skipped` 差 1，仍是 §9.5 说的 `it.skipIf(win32)` 那一条。
- **mac 侧没有回归**：同一提交 mac `npm test` =
  `Test Files 65 passed | 6 skipped (71)`、`Tests 839 passed | 32 skipped (871)`（27.27s），
  与修复前完全一致；`npm run typecheck` rc=0；两个被改文件单独重跑
  `Test Files 2 passed (2)`、`Tests 110 passed (110)`。
- **边界（诚实登记）**：修的是测试可移植性，不是产品缺陷，因此这条**不改变** §9.5 的定性；
  §9.5 末尾登记的"混合 OS 拓扑假设"仍然存在、本轮仍未处理。Windows 验证用的是
  既有 node_modules（`npm test` 只跑 vitest），没有重装依赖。

### 9.6 `package-lock.json` 同步收据：`npm ci` 首败 → rc=0（ACP `6a76aa7`）

按 §9.3 的建议路径闭合：在**确定的 registry**（`registry.npmjs.org`，即 mac 的
`npm config get registry`、也是 lock 里原本全域使用的域）上显式同步，而不是让某台配了
镜像的机器顺手改写。

- **首败证据（改动前的 lock，原样 `git show HEAD:package-lock.json`）**：
  `npm ci` **exit=1**，`npm error code EUSAGE` +
  `Invalid: lock file's nuwax-codex@0.18.3 does not satisfy nuwax-codex@0.18.12`。
- **改动**：`npm install --package-lock-only --registry=https://registry.npmjs.org`
  （`--package-lock-only` 保证**不碰工作树里的 node_modules**，测试环境不动）。
  diff 极小：**6 增 6 删**，只有 `version 1.2.12 → 1.2.18`、根依赖
  `^0.18.3 → ^0.18.12`、`node_modules/nuwax-codex` 的 version/resolved/integrity 三行。
- **供应链核对（这是当初把它列为"需用户决定"的那一项）**：
  - 全部 **268** 条 `resolved` 仍指向 `registry.npmjs.org`，一条都没被换成镜像域；
    `missing-integrity = 0`（每条都有 `integrity`）。`lockfileVersion` 仍是 3。
  - 新 `integrity` `sha512-NlHdwGgyi/Pj+36AJWEcckrJ5WFprBQKVW3vvVK/W+3BzjrbsnYcn3GDrPVlLQ0NEQTBVN+js70plyMpX8IgNA==`
    与 §9.2 里 Windows 那台**经 `registry.npmmirror.com` 解析**时 npm 记下的值**逐字符相同**
    （`npm view nuwax-codex@0.18.12 dist.integrity --registry=https://registry.npmjs.org`
    亦给出同值）。两个域给出同一 sha512，说明是**同一份 tarball 字节**，换域没有换产物——
    这正是当时不能仅凭"装上了"就同步 lock 的原因，也是这次可以安全同步的依据。
- **修复后证据**：临时目录里只放 `package.json` + 新 lock，
  `npm ci --ignore-scripts` **exit=0**、日志 0 条 `npm error`，
  `node_modules/nuwax-codex` 实际装成 **0.18.12**（163 个顶层包）。
- **第二平台复测（这台机器正是当初改写 lock 的那类环境）**：Windows
  `192.168.32.235` 上 `npm config get registry` = `https://registry.npmmirror.com`
  ——即 §9.3 里"配了国内镜像"的具体来源。在该机上对**同步后的 lock** 跑
  `npm ci --dry-run --ignore-scripts` 得 `up to date`、**exit=0**；对**同步前的
  lock**（`git show 6a76aa7^:package-lock.json`）跑同一命令得
  `npm error code EUSAGE` +
  `Invalid: lock file's nuwax-codex@0.18.3 does not satisfy nuwax-codex@0.18.12`、
  **exit=1**。所以这条陈旧 lock 的阻断在两个平台都实测复现，修复也在两个平台都实测成立。
  （该 Windows 目录已有 node_modules，`--dry-run` 走的是 `up to date` 快路径，
  因此这条只证明**lock 与 package.json 的一致性检查**在镜像机上通过，
  **不**是一次真实下载收据；"下载地址仍钉在 `registry.npmjs.org`"的依据是
  mac 侧对新 lock 268 条 `resolved` 的逐条统计，见上。）
- **该 verify 工作副本的收尾**：Windows 上那份被 `npm install` 顺改写成
  `registry.npmmirror.com` 的本地 lock 没有直接丢弃，先复制到
  `C:\git-workspace\rig-verify-20261019\package-lock.npmmirror.local.json` 存档，
  再 `git checkout --` 撤掉本地改动并 `--ff-only` 拉到 `6a76aa7`。该目录现为
  `6a76aa7`、除既有未跟踪 `vendor/` 与 `scripts/live-validation/__pycache__/` 外干净；
  我建的 `lockold` 临时目录已删除。存档那份可随时删。
- **为什么这仍然不是"CI 变绿"类修复**：§9.3 已查实四个 workflow 全用 `npm install`，
  没有 job 用 `npm ci`。这条闭合的是**可复现安装**与**发布渠道不再被一份自相矛盾的 lock
  误导**，不改变任何测试计数。





