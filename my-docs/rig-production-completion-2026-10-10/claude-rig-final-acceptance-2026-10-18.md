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
  → **上游测试/实现的竞态，mac 首发**；列为待复现观察，本轮不改。**（§6.15 C 补测：本机
  54 跑 0 现，且本机单次比 CI 两个值都快——复现条件仍是 runner 形状，定性不变。）**
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

**处置（task #18，本轮拍定）**：维持"harness / runner 镜像能力边界"这一分类，
**不动上游那两条用例**。理由补全：`.235` 桌面机 `pwsh` 能正常加载该模块，
所以两种改法都只能在 runner 上验证；为它们单开一手 CI 就是"为了重新跑 CI
无意义地提交"，本轮不做。补充一点分类的**边界**（不是"证明无关"）：
`Import-Module Microsoft.PowerShell.Security` 是本地自动加载（autoloading）路径，
runner 镜像上失败的正是这一步，而我**没有**在 runner 上直接实测过
`Get-Module -ListAvailable Microsoft.PowerShell.Security` 是否可见——
镜像里的模块缺席、还是模块在但 autoloading 被 `PSModuleAnalysisCachePath` /
受约束运行空间挡住，这两种子情形我用现有日志分不开（原文只到"could not be loaded"）。
要分开就得在 runner 上跑一条只读探针，属于"需要一次带这行探针的 CI"，
按同一条理由推后。**下一手判据**：本轮 run 的 windows 腿若仍以同一原文红，
即确认与 `95fb0949d`/`fd0020bb0` 等改动无关，届时"改探针 + 一手 CI 验证"
是否需要做，请用户点头。

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

### 6.15 本轮 macOS 收尾：daemon 期限放宽、nextest 优先级陷阱、#8 的 54 跑不复现

对应 §6.10 处置里挂着的那半句"**那 5 条的 unix 期限放宽，与下一次代码推送合并做**"。
本轮做了，落点提交 `981490315`（未推，压在 in-flight run 之后）。

**A. 先拆一个会静默吞掉修改的陷阱：nextest 的 override 是"逐字段、先匹配者生效"**

这不是猜的，是同一台 mac、同一份 `cargo-nextest 0.9.146` 上四次实测：

| 实验 | 配置形状 | 观测 | 结论 |
| --- | --- | --- | --- |
| 1 | 同一 filter 两条 override，前者 `300s`、后者 `1s × 2` | 用例跑满 **15.471s PASS** | 同一字段**先匹配者生效**，后面的被忽略 |
| 2 | 前者只设 `test-group`，后者设 `1s × 2` | **TIMEOUT 2.008s** | 合并是**逐字段**的：前者没设期限，后者仍生效 |
| 3 | 把某条 filter 改坏成 `test(((` | `list` 与 `run` **都不报错** | filter 解析是**惰性**的，语法错误不在配置加载时暴露 |
| 4 | 用真实配置的副本，只把新规则的 period 改成 `1s` | `packaged_daemon_start_and_explicit_replacement` **1.002s TIMEOUT** | 新规则确实命中该用例，且**优先于**它下面那条 `1m × 3` |

实验 2 顺带给出一个反向收据：`:241` 那条 mac 串行 `test-group`（`8349736da` 同族思路的
unix 版）对**已经被上面期限规则命中**的 daemon 用例仍然有效，不是 R5 那种死代码。
实验 3 解释了为什么"配置能加载"不能作为证据——所以这条规则的 filter 我用
`cargo nextest list -p codex-cli -E '<与配置逐字相同的串>'` 单独核对，输出**恰好 5 行**：
`explicit_restart_applies_feature_overrides_and_preserves_saved_settings`、
`managed_starts_ensure_one_updater_and_recover_a_missing_one`、
`packaged_daemon_explicit_replacement_migrates_running_legacy`、
`packaged_daemon_start_and_explicit_replacement`、
`restart_applies_saved_updater_preference`，无多无少。
`filter` 也刻意写成文件里主流的单行形式（不是 `'''` 多行），避免 TOML 多行串的首行换行
进入表达式——惰性解析意味着这种写法错了也不会在本地报出来。

**B. 为什么放宽到 `2m × 2`（kill 在 4m），以及为什么放在那两条规则上面**

期限数字全部来自 §6.10 那张表，不新造：上一手 mac 的贴线 PASS 是
54.902 / 99.587 / 138.699 s，对应外界 60 / 120 / 180 s，即只差 **5.1 / 20.4 / 41.3 s**；
同族本轮仍然 PASS 的 daemon 用例被测出 **20–65 %** 的 lane 变慢。取上界
138.699 × 1.65 = **228.9 s < 240 s**。
成本方向要说清：这 5 条在 mac 上被 `:241` 串行化，墙钟时间就是它们的和，
**一条 90 s 的 PASS 比"60 s kill + 重试 60 s = 120 s"更便宜**，只有真挂起的腿才多花时间
（最坏 `4m × 2` 次尝试 = 8 min/条）。位置则是硬约束：实验 1 决定了我必须把它插在原
`packaged_daemon_start_and_explicit_replacement`（`1m × 3`）与 `packaged_daemon_`
（`1m × 2`）两条**之前**，插在后面就是一条看起来改了、实际没用的 R5。
覆盖面只给 `cfg(target_os = "macos")`，没按 §6.10 原话给 `cfg(unix)`：ubuntu 这 5 条是
5.959–41.847 s 的 PASS，给它 4 分钟窗口等于给 ubuntu 的真回归加一层遮蔽。
`#![cfg(unix)]`（`cli/tests/app_server_daemon.rs:1`）让 Windows 对这 5 条零覆盖，不变。

**诚实边界**：这条**不能**在本机出收据。本机不是 runner，跑不出 20–65 % 那档争用；
实验 4 只证明"窗口确实被授予了这 5 条"，不证明"放宽后它们会转绿"。后者的收据只能来自
CI，而且**不是** in-flight 的 `39347345560`（head `6e27a6c11`，不含本条），只能是推送后的下一轮。

**C. 九项里 #8 的本机复现尝试：54 跑 0 现，定性不动，状态改口径**

`multi_agent_v2_developer_instructions…inherits_parent_developer_instructions_without_an_override`
是九项中唯一"时序不是它的敌人"（FAIL 1.420s vs PASS 1.503s，几乎同速）的一条，
按理最不该只归给 runner，所以补测：

- 串行 30 跑（`--retries 0`，单条）：**30 PASS**，0.61–0.89 s；
- 并发加压 24 跑（4 个 worker × 6 跑，同时 24 个 CPU 自旋进程超订 18 核）：**24 PASS**，0.63–1.25 s；
- 合计 **54 跑，`NotLoaded` 观测 0 次**；本机单次耗时（0.61–1.25 s）**低于 CI 的两个值**，
  说明这台机器在这条腿上比 mac runner 快，加压也没把它拉到 CI 的量级。

⇒ 结论只能到这里：**本机 54 跑不复现**，既没推翻 §6.10 的"上游测试/实现竞态、mac 首发"，
也没拿到把它升级成分叉缺陷的证据。缺的条件是 CI 那种"整个 app-server 集成组 4 并发 +
315 个 binary 抢同一台机"的真实形状；要不要在本机整组复跑（1973 项）等本轮 mac 腿给出
#8 的第二次观测再定，避免盲目烧几十分钟。

**D. 顺手抓到的一处验证setup缺陷（不是产品问题，登记并改法）**

ACP 的 vitest 套件把自己的 `CODEX_HOME` scratch 建成 `codex-acp-codex-home-*`，落点是
node 的 `os.tmpdir()`，也就是**系统盘**。实测（本轮，mac）：`/System/Volumes/Data`
**96 % 满、余 18 Gi**；`/private/var/folders/…/T` 合计 **28 G**，其中该前缀
**1919 个目录 / 25.4 GiB**；按 mtime 分桶，**24 h 内 676 个 / 10.1 GiB**（本轮产生），
48 h 内计数相同，96 h 内为全部 1919 个。按 C2 的约定**一个都没删**。
改法与 cargo 那条"三变量缺一不可"同源：跑 ACP 套件时显式
`TMPDIR=/Volumes/zhitai/tmp`，否则"我只在验证"这件事本身会把用户的系统盘吃掉 10 Gi/轮。

### 6.16 §7 第 1 项闭合：`::discovery` 的根因是"桥未实现 workspace routing"，已修

**A. 诊断窗口这一步先兑现——三腿第一次打印出自己的诊断（run `39347345560`，head `6e27a6c11`）**

§6.9 说的结构性障碍是：用例自设的 `DEFAULT_READ_TIMEOUT = 60s`
（`app-server/tests/suite/v2/account.rs:74`）与 nextest 默认界限 `30s × 2 = 60s` 重合，
内部任何一条 `timeout(60s, …)` 腿刚开始等就正好被 harness 杀掉，所以历来只有 `TIMEOUT`
一个名字。窗口放宽之后，本轮三腿**第一次**给出同一份诊断：

| 腿 | 结果 | 该腿其余状态 |
|---|---|---|
| ubuntu | `FAIL [ 60.783s]` | `Summary [3004.229s] 22927 tests run … 2 failed`（另一条是 `shell_snapshot…blocked_descriptor_path` FAIL 0.346s） |
| windows | `FAIL [ 60.394s]` | `Summary [2383.014s] 21759 … 11 failed`，**零 TIMEOUT** |
| macos | `FAIL [ 60.525s]` | `Summary [2646.520s] 22877 tests run: 22876 passed (23 slow), 1 failed`——这条就是唯一失败 |

三值离散度 **0.389 s**，与 §6.9 的 16 ms 同一量级：**仍是"卡到期限"，不是"慢"**。
panic 文本（wiremock 0.6.5 的 `Mock::expect(n)` 在 `MockServer` **drop 时**才校验，
所以它排在测试自身失败之后，这一点当初容易被读成"失败原因"）：

```text
Verifications failed:
- Mock #2.
	Expected range of matching incoming requests: == 1
	Number of matched incoming requests: 0
```

并且 `Received requests`（11 条）里 `/backend-api/wham/accounts/check` **只出现 1 次**。

**B. 本机确定性复现（不必等 CI）**

`just test -p codex-app-server -E 'test(external_auth_refreshes_on_unauthorized::discovery)'`
（`TMPDIR=/Volumes/zhitai/tmp`）→ `FAIL [ 60.506s]`、`Summary [ 60.519s] 1 test run: 0 passed,
1 failed`，同一 panic。加上探针轮一共 6 次：60.506 / 60.620 / 60.633 / 60.563 / 60.708 /
60.842 s，**全部同形**。所以这条从"CI 上拿得到一个名字"升级成"本机 1 分钟一次、可反复复现、
带完整诊断"。

**C. 五个探针，每步只排除一个假设**（探针代码已全部 `git checkout --` 回退，工作树不含插桩；
日志留在外接盘 scratch `discovery-probe{,2..6}.log`）

| # | 假设 | 探针实际打出的东西 | 结论 |
|---|---|---|---|
| 1 | mock 路径没对上（重写造出双斜杠） | `resolve provider=http://127.0.0.1:50837/backend-api/codex chatgpt=http://127.0.0.1:50839/backend-api` | **排除**：URL 拼接正确，`accounts/check` 确实以 `/backend-api/wham/accounts/check` 打到 backend |
| 2 | 用例那次 config.toml 重写是 no-op | `[probe] config.toml after rewrite: ["chatgpt_base_url = \"http://127.0.0.1:55656/backend-api/\""]` | **排除**：重写**生效**，末尾斜杠在（`account.rs:800` 的 `"/backend-api\"` → `"/backend-api/\"`） |
| 3 | 配置层被缓存，turn 读到的还是旧值 | 每次 `read_account` 都打 `layer User { file: …/.tmpX/config.toml } value=…` | **排除**：`load_user_config_layer`（`config/src/loader/mod.rs`）每次调用重读文件，链路里没有缓存 |
| 4 | `WorkspaceRoutingKey` 命中旧缓存，所以不发第二次 GET | `read_account request=false → cached_hit=false -> discovery GET`；`read_account request=true → cached_hit=true` | **前提就不成立**：key 含 `effective_chatgpt_base_url`，重写后必 MISS。真实情况是 **turn 之后连一次 `read_account` 都没有**，缓存这条无从谈起 |
| 5 | 传输选择 | `stream provider_id=Some("routing") name=OpenAI wire_api=Responses bridge=None first_party=false uses_bridge=true` → `-> stream_model_bridge (Responses)` | **命中** |

决定性时间线（探针 1，行号就是日志行号）：`account/login/start` → `:18 read_account
request=false` → `:19 cached_hit=false -> discovery GET`（**这就是全程唯一那次
`accounts/check`**）→ `thread/start` → `:28 resolve` + `:29 read_account request=true`
→ `:30 cached_hit=true`（无 HTTP）→ `:35 writing … turn/start` **之后日志里再没有任何
`[probe]` 行**。也就是说：turn 从未跑 workspace routing。`:28` 那条 `resolve` 属于
`thread/start` 而不是 turn——这一段最初被 `[mcp stderr]` 中继行与测试进程自身 `eprintln!`
的交错顺序误导，改为按 JSON-RPC `writing message to stdin` 时间线重排后才看清（登记为踩坑）。

**D. 根因（一句话）**

分叉默认策略把这条 provider 判给了桥：`provider_id = "routing"` 不是第一方 key
（`is_first_party() = false`）、`wire_api = Responses` 且 `experimental_bridge` 未设，于是
`uses_model_bridge()` 返回 true → 走 `stream_model_bridge`。而 **workspace routing 的
discovery 只存在于原生路径**：`ModelProvider::responses_api_provider()`
（`model-provider/src/provider.rs:216-251`）是 `auth_manager.workspace_routing(…)` 的
**唯一调用方**；桥路径 `stream_model_bridge`（`core/src/client.rs:1917` 附近）显式使用
`ClientRouting::ConfiguredProvider`，既不做 discovery，也不套 `apply_workspace_routing`
（routed backend origin / residency / routing-override 头），因此永远不会走
"discovery 拿 401 → `io::ErrorKind::PermissionDenied` → `account/chatgptAuthTokens/refresh`"
这条恢复（`workspace_routing.rs:181-201`）。用例 `:815 respond_to_refresh_request` 等的正是
那条 refresh 请求，它按设计不可能到达 → `timeout(60s, read_stream_until_request_message())`
到点 → FAIL；`Mock #2` 期望的第二次 `accounts/check` 一次没来，drop 时补一个 `matched 0`。

`::responses` 是**同一次构建**的对照组：它的 `base_url` 以 `/v1` 结尾，
`supports_codex_backend_routes() = false`，401 来自**模型请求本身**，而桥保留了模型请求
401 → 刷新这条恢复，所以 PASS 0.240s。结论要说准：**桥不是"完全不会刷新"，缺的只有
workspace routing 这一块**。

**E. 四分法定性，并纠正 §6.9 / §6.10 第 1 类**

- 归类：**桥未实现**（rig 桥未实现 workspace routing）→ **真实功能缺口**。不是"协议不适用"，
  不是 harness 缺陷，也不是 runner 抖动。
- 归属：**本分叉造成**。引入点可指到提交：`0f62ba844`
  （`feat(rig): same-protocol Responses passthrough (phase 1)`，即 `uses_model_bridge()`
  这套策略本身，`git log -S"uses_model_bridge" main..test` 唯一命中）；而用例文件
  `app-server/tests/suite/v2/account.rs` 在 `main..test` 上是 **0 笔分叉提交**（实测计数）。
- 所以 §7 第 1 项被撤回的那半句"**上游通过、fork 失败**"现在**有了凭据**，但凭据的形态要说清：
  不是去对比上游 CI 的运行结果（那仍然没做过），而是"失败机制整个位于分叉新增的策略函数与
  桥路径之内，测试文件与三个直接相关文件都未被分叉改过，把策略收窄后用例即通过"。
  §6.9 当时的撤回**仍然成立**（那时确实无可出示凭据）；本轮是补上凭据，不是推翻它。
- §6.9 的候选归类"真实缺陷（挂起）或 harness 死锁"、§6.10 第 1 类沿用的"确定性挂起，待根因"
  **就地作废**：既不是挂起也不是死锁，是**该等的请求在设计上永远不会发出**。
- 边界：这条**不属于**分叉要支持的国产模型场景，也不是桥"该补的通用能力"——
  `requires_openai_auth = true` + `…/backend-api/codex` 出站就是 ChatGPT 第一方形态。

**F. 修法：取窄的一侧（把有 routing 能力的 entry 钉回原生传输），不把 routing 移植进桥**

- `model-provider-info/src/lib.rs`：新增 `uses_first_party_auth_path()`
  （`requires_openai_auth` 且 `env_key` / `experimental_bearer_token` / `auth` / `aws` 全空）
  与 `requires_workspace_routing()`（前者 ∧ `supports_codex_backend_routes()`），
  `uses_model_bridge()` 的 `Responses + bridge = None` 分支改为
  `!(self.is_first_party() || self.requires_workspace_routing())`（`lib.rs:764`）。
- `model-provider/src/provider.rs:327`：原本私有的同形判断改为委托
  `provider.uses_first_party_auth_path()`，避免两份定义漂移。
- `core/src/client.rs:1915-1921`：桥路径补注释，说明"桥永不可能携带 routing 能力，
  因此在这里固定 `ConfiguredProvider` 不会丢 workspace routing"——这是把上一条不变量写死在
  它唯一的反例旁边，防止后续有人往 `None` 分支里加东西。
- 为什么不做另一半：移植 workspace routing 进桥要补 routed backend origin / residency /
  routing-override 三类头 + discovery 缓存 + 401 恢复，而分叉的桥服务的是**第三方
  provider**，它们本来就不该携带 ChatGPT 账号路由。钉传输是 3 行，移植是跨模块新功能，
  还会把"分叉默认策略"和"第一方账号路由"两条语义搅在一起。
- `experimental_bridge = "native"` 与显式 `Rig`/`Genai` 两条口子**行为不变**：用户仍可强制
  把这类 entry 拉进桥，代价（丢 workspace routing）此时是显式选择而非默认策略造成。

**G. 验证收据（本机，首败 → 修复后；同一台 mac、同一 target 目录）**

| 相位 | 命令范围 | 修复前 | 修复后 |
|---|---|---|---|
| 目标用例 | `just test -p codex-app-server -E 'test(external_auth)'` | `::discovery` **FAIL 60.506s**（§B） | **5/5 全绿**：`::discovery` **PASS 0.660s**（快 **91.7 倍**）、`::responses` PASS 0.247s、另 3 条 `refresh_{error,invalid_access_token,mismatched_workspace}_fails_turn` 0.626–0.672s；`Summary [0.887s] 5 passed` |
| P1 策略与 provider | `-p codex-model-provider-info -p codex-model-provider` | — | `Summary [10.917s] 140 passed`，含新回归 `bridge_routing_tests::chatgpt_backend_route_keeps_the_native_transport` **PASS 0.014s** |
| P2 策略另一半消费者 | `-p codex-config`（含 `bridge_config_tests`） | — | `Summary [0.369s] 386 passed` |
| P5 doctor | `-p codex-cli -E 'test(model_routing) \| test(doctor) \| test(bridge)'` | — | `Summary [16.774s] 149 passed, 405 skipped` |

新回归用例钉住的是"收窄"这个决定本身，而不只是结果：对
`{name:"OpenAI", provider_id:"routing", base_url:"…/backend-api/codex", requires_openai_auth:true}`
断言 `requires_workspace_routing() && !uses_model_bridge()`；同时断言 `WireApi::Chat` /
`Anthropic` 变体仍进桥，以及带 `env_key` / `experimental_bearer_token` / `aws` 任一样的变体
**仍进桥**（即"任一自有鉴权覆盖都不算第一方 auth path"，防止这条口子扩大到第三方形态）。

**H. 两处必须登记的不成立（都是我这轮的验证 setup 问题，不是产品问题）**

1. **过滤式误伤**：P4 我写成 `-E '… | test(rig)'`，而 nextest 的 `test(...)` 是**正则子串匹配**，
   `originating` 里就含 `rig`，于是把 9 条 `*originating*` / 图片预算用例一起拉了进来，
   它们在 0.028–10.287s 内 `deadline has elapsed` 失败。真实原因是我漏了 CI 的
   `--features codex-core/rust-rig`（`.github/workflows/fork-cargo-pr.yml:149-151` 是这么跑的），
   **不是分叉缺陷、也不由本轮改动造成**；A/B 与补齐 feature 后的收据见 §6.16.1。
2. **假绿退出码**：`just test … | tail -40; echo "P4=$?"` 取到的是 `tail` 的退出码，
   所以那一行的 `P4=0` / `P5=0` / `P6=0` **三个都是假的**（P4 明明有 9 failed）。
   从 P4b 起改成"nextest 输出重定向到文件、再单独取 rc"。**登记的原因**：这与 §6.15 D
   是同一类"验证 setup 自己给出不可信结论"的坑，而它差点掩盖了第 1 条。

**边界（不冒领）**：本轮改的是产品策略，**CI 侧"修复后转绿"的收据只能由这笔推送之后的下一轮
给出**——run `39347345560` 的源码里没有这个修复，它三腿的 FAIL 是**修复前**的证据。
修复后证据目前只有本机 mac 一条腿的 `PASS 0.660s`。同理，§7 第 2 项 (b) 那 5 条 mac daemon
期限（`981490315`）的转绿也仍要等同一轮。

#### 6.16.1 A/B 收据：P4 的 9 条与 P6 的 2 条都不是本轮改动造成

- **A/B（决定性）**：把 4 个改动文件回退到修复前
  （`git checkout 6e27a6c11 -- <4 files>`），跑**同一条** P4b 命令 →
  `Summary [37.149s] 154 tests run: 145 passed, 9 failed`；修复后（HEAD `95fb0949d`）是
  `Summary [35.007s] 154 tests run: 145 passed, 9 failed`。两份失败集合**逐条同名**
  （`guardian_context_budget::…oversized_original_inline_image_history_compacts`、
  `code_mode::code_mode_resizes_explicit_original_image`、
  `code_mode::yielded_code_mode_tool_callbacks_keep_their_originating_step`、
  `code_mode::code_mode_mcp_metadata_keeps_originating_window_after_compaction`、
  `rmcp_client::stdio_image_responses_preserve_original_detail_metadata`、
  `rmcp_client::twenty_synthetic_screenshots_trigger_formal_compaction`、
  `skills_extension::…yielded_skill_read_keeps_originating_turn_metadata`、
  `step_settings::code_mode_notifications::yielded_notifications_keep_originating_budgets_after_model_switch`、
  `step_settings::custom_tool_output_replay_preserves_originating_budget`）。
  跑完立刻 `git checkout HEAD -- <4 files>`，`git status --porcelain -- codex-rs` 为空。
  **⇒ 这 9 条与本轮改动无关**，§6.16 H 第 1 条的"不是分叉缺陷"这一半现在有了 A/B 凭据。
- **真实原因（可复现、有文本）**：按 `-p <单个 package>` 过滤跑时，cargo 只构建该 package 的
  test target，**兄弟 binary 不会被构建**，`assert_cmd` 于是找不到可执行文件：
  `Error: could not locate binary "codex-code-mode-host"`（3 条）、
  `"test_stdio_server"`（2 条）；另有 1 条表现为 `Error: deadline has elapsed`——同一根因的
  另一种形状（spawn 等待超时）。CI 用 `--workspace` 跑，这两个 binary 都在，所以 mac 腿
  `22876 passed / 1 failed` 里它们是绿的。**四分法归类：第 4 类（harness / 调用方式缺陷），
  不是产品缺陷。**
  这与 §6.15 D、§6.16 H 是同一条教训的第三次出现，固化成一条：**单 package 过滤跑出的红，
  先问"兄弟 binary 在不在"，再谈归属。**
- **P6 的 2 条逐条**：
  - `async_scorer::sampler::connection_pool::tests::stalled_handshake_does_not_delay_other_warm_sockets`：
    单独跑 **3/3 PASS**（0.562 / 0.621 / 0.602 s 三个 `Summary` 里均为那 1 条 passed），
    只在 492 项并发满负荷时失败 → **并发时序抖动**，登记为抖动，不冒领成"已修"。
  - `async_scorer::extension::tests::cached_delivery::confirmed_root_delivery_invalidates_root_and_worker_cached_approvals`：
    单独跑 3/3 FAIL（0.461 / 0.531 / 0.505 s），文本同为
    `could not locate binary "codex-code-mode-host"` → **同属上面那条调用方式缺陷**，
    与 guardian 的审批缓存逻辑无关。注意它落在 `async_scorer::extension`——正是
    `uses_model_bridge()` 的消费者模块，所以一开始被怀疑是本轮改动，A/B 之前不能下结论。
- **feature 对照**：P7 = `just test -p codex-app-server --features codex-core/rust-rig
  -E 'test(external_auth)'`（与 CI 同 feature 形状）→ **5/5 全绿**，
  `::discovery` PASS **0.696s**、`::responses` PASS 0.302s。修复不是只在默认 feature 下成立。
- **第二真实操作系统（Linux）**：`swufe@192.168.31.41`（Ubuntu 24.04.4，72 核，
  `CARGO_TARGET_DIR=~/rig-verify-20261019/target`，`NEXTEST_PROFILE=local`、
  `RUST_MIN_STACK=8388608`、`--locked`、`--retries 0`，源码经 `git bundle` 传到
  `95fb0949d`）：
  - **L1** `-p codex-model-provider-info -p codex-model-provider` →
    `Summary [8.973s] 140 tests run: 140 passed`，与 mac 的 140 项**同集合**，新回归用例在其中。
  - **L2** `-p codex-app-server --features codex-core/rust-rig -E 'test(external_auth)'` →
    `Summary [4.492s] 5 tests run: 5 passed`，其中
    **`::discovery` PASS 2.695s**（同一分支的 CI ubuntu 腿在**不含修复**的源码上是
    `FAIL 60.783s`）、`::responses` PASS 2.265s、另 3 条 2.178–2.229s。
    **⇒ 修复在第二个真实操作系统、且在与 CI 相同的 feature 形状下成立**，不是 mac 单腿结论。
  - L3（core 过滤对照，用来在 Linux 上再证一次"兄弟 binary"结论）与
    L4（`codex-exec-server` 的 `shell_snapshot…blocked_descriptor_path`，即 B3）
    的结果与归属逐条登记在 **§6.19**：L3 的 15 条 = 5 条兄弟 binary + 10 条时序形状，
    后者已用空闲机复跑（L8b）排除负载、并用系统 `bwrap` 量出机制；
    L4 因我这边的 feature 用法错误没跑成，改由 L7b 完成，**45/45 全绿**——
    B3 在真 Ubuntu 上不复现。另外 **L8a = `codex-rust-rig-bridge` 337/337 全绿**，
    这是视觉 floor 修复（`04beac00a`）的第二操作系统收据。

### 6.17 Linux 验机接手：§7 第 4 项（rustup）不是网络损坏，是 toolchain 目录被 rust-installer 写过

这台机器就是交接文档里"ubuntu 24 / 192.168.32.117（不通）"那台换了 IP——
`~/rig-verify-20261019` 的旧 clone、旧 `target`(15 G) 与旧日志都在，
`swufe@…` 的授权密钥也早已装好；换的是网段（`32.x` → `31.x`）。
所以 §9.1 的 ubuntu 通道**可以直接复用**，不需要重新搭。

接手后第一条 Linux 命令就全灭（L1–L4 各 3 秒 `rc=101`），文本：

```text
info: syncing channel updates for 1.95.0-x86_64-unknown-linux-gnu
thread 'main' panicked at src/dist/component/transaction.rs:96:9:
assertion failed: relpath.is_relative()
info: rolling back changes
```

- **为什么每次调用都会尝试装组件**：`codex-rs/rust-toolchain.toml` 是
  `channel = "1.95.0"` + `components = ["clippy", "rustfmt", "rust-src"]`，而这三个组件在
  `lib/rustlib/components` 里**都不在**（只有 `cargo-x86_64-unknown-linux-gnu`、`rustc`、
  `cargo`、`rust-std-x86_64-unknown-linux-gnu`）。于是任何在仓库目录里启动的 `cargo` 都会先
  触发一次组件事务，而该事务必然 panic——**测试根本没开始跑**。
- **`rustup component add` 给出的另一条线索**：三个组件全部
  `error: missing manifest in toolchain '1.95.0-x86_64-unknown-linux-gnu'`。
- **根因（有凭据，不是猜）**：数每个 manifest 里的**绝对路径行数**：

  | manifest | 绝对路径行 | 总行 |
  |---|---|---|
  | `manifest-cargo` | 44 | 44 |
  | `manifest-rustc` | 41 | 41 |
  | `manifest-rust-std-x86_64-unknown-linux-gnu` | 62 | 62 |
  | `manifest-cargo-x86_64-unknown-linux-gnu` | 0 | 44 |
  | `manifest-rust-docs-x86_64-unknown-linux-gnu` | 0 | 1 |

  样例：`file:/home/swufe/.rustup/toolchains/1.95.0-x86_64-unknown-linux-gnu/bin/cargo`。
  rustup 自己写的 manifest 是**相对路径**，绝对路径是 **rust-installer（`install.sh --prefix=`）**
  的格式；`lib/rustlib/install.log` 里还有
  `install: uninstalling component 'rust-std-…'` /
  `WARNING: supposedly installed file … does not exist!`，`bin/` 里留着一次回滚产生的
  `cargo.old`。⇒ 这个 toolchain 目录**不是 rustup 装的**（或被独立 installer 覆盖过），
  所以 rustup 无法管理它的组件；每次事务都在断言处炸。
- **所以 §7 第 4 项的口径要改**：原先写的是"`swufe@192.168.32.163` 的 rustup 组件安装存在
  **manifest 损坏**"。本轮测出的形状更准确：不是下载损坏、不是网络问题，是**装机方式**问题；
  表现是"每个组件操作 100% panic / missing manifest"，而不是偶发失败。
- **处置（可逆，且没有动仓库锁定版本）**：
  1. **不删**——把坏目录整段隔离：
     `mv ~/.rustup/toolchains/1.95.0-x86_64-unknown-linux-gnu ~/.rustup/quarantine/1.95.0-broken-installer`
     （1.6 G 原样保留，随时可还原）；
  2. 用 rustup 正常安装同一 pin：
     `rustup toolchain install 1.95.0-x86_64-unknown-linux-gnu -c clippy -c rustfmt -c rust-src`
     → `1.95.0-… installed - rustc 1.95.0 (59807616e 2026-04-14)`，无 panic；
  3. `rust-toolchain.toml` 未改动，仓库版本仍是 1.95.0（遵守"不擅自修改仓库锁定版本"）。
- **修完立刻见效**：L1 在 Linux 上跑通并 `140 passed`。
- **顺带登记的两条环境事实（影响后续 Linux 复跑）**：这台机器**没有装 `just`**
  （`which just` 空），所以 Linux 侧改用等价
  `cargo nextest run --locked --no-fail-fast … --retries 0` 并显式导出
  `NEXTEST_PROFILE=local` + `RUST_MIN_STACK=8388608`——这正是 §6.12 R5 里"just recipe 自己会
  重发 `RUST_MIN_STACK`，workflow 的 `env:` 是惰性的"那条的镜像问题：**绕过 just 时必须自己
  把它本来会设的变量补上**，否则跑出来的形状与 CI 不同。
  `cargo-nextest` 已装在 `~/.cargo/bin`；node v22.23.3 / npm 10.9.9 / python 3.12.3 /
  docker 均在；`/` 余 1.1 T。

### 6.18 Linux 轮收据：B1 四项里三项已在真机闭合

**A. 环境与源码通道（一条命令可复核）**

Ubuntu 24.04.4（kernel `7.0.0-38-generic`，`swufe-X99`）、**72 核 / 62 G**、`/` 余 1.1 T；
node v22.23.3、npm 10.9.9、python 3.12.3、git 2.43、docker 在；**`just` 不在**
（§6.17 末条已登记其后果）。Codex 侧源码通道：本机
`git bundle create … 4ee470d5c..test`（**86 KB**）→ scp →
`git fetch <bundle> test` → `qoder-verify-20261020` @ `95fb0949d`；
不依赖推送、不占用 CI。ACP 侧从 `0930455` 同步到 `origin/dev` = `6a76aa7`，
未跟踪的 `vendor/` **原样保留**（`git status` 里仍在）。

**B. §7 第 1 项修复的第二操作系统收据**：L1 `140 passed`、L2 `5 passed`
（`::discovery` PASS **2.695s**），逐条见 §6.16.1。

**C. B1 的 ACP 三项（全部离线，零付费）**

| 相位 | 命令 | 收据 |
|---|---|---|
| A1 | `npm ci --no-audit --no-fund --cache ~/rig-verify-20261019/npm-cache` | `added 201 packages in 6s`、**rc=0** ⇒ §9.3 的 `npm ci` 首败在 Linux 上也已闭合（lock 同步即 `6a76aa7`） |
| A2 | `npm run build` | rc=0 |
| A3 | `npm test`（vitest，**Linux 第一次跑完整产品套件**） | `Test Files 65 passed \| 6 skipped (71)`、`Tests 839 passed \| 32 skipped (871)`、`Duration 64.42s` |
| A4 | `npm run test:acp-validation`（**mock vendor server**） | `# tests 37 / # pass 37 / # fail 0` |
| A5 | `npm run test:live-budget` | `# tests 26 / # pass 26 / # fail 0` ← B1 里记的 "**node 26**" |
| A6 | `python3 -m unittest discover -s scripts/live-validation -p 'test_*.py'` | `Ran 29 tests in 5.011s — OK` ← B1 里记的 "**python 29**" |

A3 的意义要说准：§9.5 在 Windows 上抓到 3 条与路径分隔符耦合的断言、并在 `59426b6` 改成平台
中立；Linux 与 mac 用 `/`、Windows 用 `\`，**这条改动的第二个平台形状此前从未被真机覆盖**。
Linux 839 项全绿说明该修正没有把断言改成"只迁就 Windows"。

**D. 抓到并纠正的第二次"管道吞退出码"**

A6 最初写成 `python3 -m pytest … | tail -10 || python3 -m unittest …`，而这台机器**没装
pytest**。管道段的退出码是 `tail` 的 0，所以 `||` 回退分支**根本没有执行**，日志里
`A6_rc=0` 与 `No module named pytest` 并存——一项没跑却被记成通过。改用 `unittest` 显式跑
才拿到真实的 29 项。这是 §6.16 H 第 2 条的**第二次**命中（第一次差点掩盖 9 条红，这次差点
伪造 29 条绿）。**固化成一条：带 `||` 回退或带 `| tail` 的命令必须单独取真实 rc；
验证脚本一律"重定向到文件 + 紧跟一条 rc 打印"。**

**E. 边界（不冒领）**

- A3–A6 都是**离线**套件；`live-validation` 里真正打上游的 workflow **本轮不跑**
  （遵守"本轮不跑实网"）。所以 B1 的"实网收据"仍不属于本轮，§4 的 54 次上游请求照旧只绑定
  `c4d08f71b` 的二进制。
- 写本节时 L3 / L4 / L5 / L6 仍在跑，结果逐条补登（§6.16.1 与 §6.19）。

### 6.19 Linux L3 / L4：两条红都是我这边的调用缺陷，但 L3 剩下的 10 条必须先隔离复跑

**L3（`-p codex-core --features codex-core/rust-rig -E '… | test(rig)'`）**：
`Summary [19.798s] 158 tests run: 143 passed, 15 failed, 5242 skipped`、`L3_rc=100`。

- **选中 158 条本身是过滤器的错**：`test(rig)` 是**子串正则**，命中
  `o|rig|inating` 与 `o|rig|inal`，于是把 `code_mode` / `network_approval` /
  `step_settings` 这些与桥无关的大用例一起拉了进来（§6.16 H 第 3 条那节课的第四次出现）。
  这不是产品问题，但直接决定了下面的读数范围。
- **15 条按签名分两组（有文本）**：
  - **5 条 = 兄弟 binary 缺失**：`could not locate binary "codex-code-mode-host"`（3，
    `CARGO_BIN_EXE_…` 与 `target/debug/codex-code-mode-host` 两路都落空）、
    `"test_stdio_server"`（2）。与 §6.16.1 mac A/B 同因，Linux 上再次复现。
  - **10 条 = 需隔离复跑才能定性**：签名分别是 `expected Guardian approval, got TurnComplete
    (… "outcome":"allow" …)`、`timed out waiting for both Guardian network reviews`、
    `deadline has elapsed`、`the live session holds its writer lock at …/thread-writer-locks/….lock`。
    这些不是 spawn 失败的形状，**不能直接归到"兄弟 binary"那一条**。
- **必须登记的混淆因素**：这台机器**是共享的**。L3 跑的时刻
  `uptime` = `load average: 29.22, 29.61, 40.25`（72 核），另一个用户正在同机跑
  `cargo clippy --workspace --all-targets` + `cargo nextest run`。上面 10 条的签名全部是
  时序/争用形状，所以**在这台机器上它们既不能算分叉缺陷、也不能算"环境问题"结案**——
  先按 §6.16.1 的方法做隔离复跑（L3c，`-j 4`，只跑这 10 条），转绿才写"争用抖动"，
  仍红就回到根因。**在此之前 L3 的定性是"未定性"。**
- 补跑（L3b）还把 `-p codex-core` 换成
  `-p codex-core -p codex-code-mode-host -p codex-rmcp-client`（后两个 package 才拥有
  `codex-code-mode-host` 与 `test_stdio_server`：`code-mode-host/Cargo.toml:2,8`、
  `rmcp-client/src/bin/test_stdio_server.rs`），这样 5 条已知成因的用例也应转绿。

**L4（`-p codex-exec-server --features codex-core/rust-rig …`）**：`L4_rc=101`，
**一条测试都没跑**：

```text
error: the package 'codex-exec-server' does not contain this feature: codex-core/rust-rig
```

- 这是我脚本的错，不是产品缺陷：`--features pkg/feat` 只在**该 feature 属于被选 package**
  时合法，CI 那条命令能带它是因为 CI 跑 `--workspace`。B3（`shell_snapshot…blocked_descriptor_path`
  的真 Linux 复现，`exec-server/tests/exec_process/shell_snapshot.rs:185`）在 §7 第 3 项里
  一直是"只在 ubuntu 红、没有真机收据"，**这轮第一次真的把它跑起来了**（L4b，去掉该 feature）。
- 顺带把同一节课写死：**单 package 过滤跑时，feature 与兄弟 binary 都要自己补齐**——
  缺 feature 会让 cargo 在编译前就退出（rc=101，看起来像"测试失败"），缺 binary 会让用例
  在运行期报 spawn 失败（看起来像"产品缺陷"）。两者的收据形状不同，但都不是产品缺陷。

**L5（image floor，`-p codex-core -p codex-utils-image`）**：日志里
`Summary [56.361s] 20 tests run: 17 passed (2 slow), 3 failed` 与 **`L5_rc=0` 并存**。

- **`L5_rc=0` 是假的**：脚本写成 `cargo nextest … 2>&1 | tail -40` 之后
  `echo "L5_rc=$?"`，拿到的是 `tail` 的退出码。这是 §6.16 H 第 2 条 / §6.18 D 的
  **第三次命中**，而且这次是"把一条红的记成绿的"。**同一脚本里的 `L6_rc=0` 同样不可信**，
  L6 只按它自己的证据登记（见下）。（`tail -40` 还把 L5 的逐条失败输出切掉了，
  只留下 Summary 里的三行名字。）

**L5 那 3 条的归属：已用 mac 的 revert-A/B 定案，不是 `04beac00a` 造成**

同一条过滤（`test(image_preparation) | test(detail_policies) | test(legitimately_large) |
test(image_budget) | test(view_image_result)`）在 mac 上跑，**修复在树里**与
**把 `04beac00a` 的 3 个文件回退到 `04beac00a^`** 两次结果**逐条同名**：

| 相位 | 树 | 结果 |
|---|---|---|
| M-L5a | `9bad7cca0`（含 floor 修复） | `Summary [20.968s] 21 tests run: 17 passed, 4 failed`、`ML5a_rc=100` |
| M-L5a-pre | `04beac00a` 的 3 个文件回退 | `Summary [16.206s] 21 tests run: 17 passed, 4 failed`、`ML5apre_rc=100` |

失败集合：`code_mode::code_mode_unified_image_budget_preserves_legacy_contract_for_unsupported_model`、
`code_mode::code_mode_can_use_view_image_result_with_image_helper::unified_image_budget`、
`…::legacy_detail`、`code_mode::code_mode_unified_image_budget_accepts_legacy_detail_hints`。
Linux L5 的 3 条是同一族（Linux 少 1 条只因为 batch 2 的过滤器漏了
`test(view_image_result)`，那条 `::legacy_detail` 没被选中）。
⇒ **§7 第 1 项的修复与视觉 floor 修复都不是这族红的原因**；四分法仍落在第 4 类。
跑完 `git checkout HEAD -- <3 files>`，`git diff HEAD --stat -- codex-rs` 为空（本轮亲自核过）。

**为什么这族在本地"修不好"：off-CI 建不出 `codex-code-mode-host`（机制，不是猜）**

失败文本不是 spawn 错误，而是断言：
`unexpected code-mode output: [Object {"type": String("input_text"), "text": String("unsupported custom tool call: exec")}]`
——code-mode 的 custom tool 没有 host。要拿到 host 就得建 `codex-code-mode-host`，而它把
`v8 v150.4.0` 拉进构建图，build script 要去下载 prebuilt：

```text
error: failed to run custom build command for `v8 v150.4.0`
urllib.error.HTTPError: HTTP Error 404: Not Found     # Linux
response = urlopen(url) -> …                            # mac，同一处
```

CI 能建是因为 workflow 先跑了
[.github/actions/setup-rusty-v8/action.yml](../../.github/actions/setup-rusty-v8/action.yml)：
它按 `.github/scripts/rusty_v8_bazel.py resolved-v8-crate-version` 取 tag，从
`openai/codex` 的 release 下载 `librusty_v8_ptrcomp_sandbox_<target>.a.gz` +
`src_binding_ptrcomp_sandbox_<target>.rs` + 校验文件，用仓库里
`third_party/v8/rusty_v8_150_4_0_release_manifests.sha256` 做信任锚，最后导出
`RUSTY_V8_ARCHIVE` / `RUSTY_V8_SRC_BINDING_PATH`。**没有这两个变量，本地任何
`-p …codex-code-mode-host…` 的构建都在编译前失败**。本轮决定不去补这份下载
（体积大、且 CI 已经覆盖同名用例），把这族红的**权威收据放在 CI 的 `--workspace` 腿**，
本地只登记机制。**这条同时解释了 §6.16.1 那 9 条 mac 红为什么在 CI 是绿的。**

**批次 3 / 4 / 5 全灭的机制：非登录 ssh 的 PATH（我自己的脚本缺陷）**

`L3b_rc=127`、`L4b_rc=127`、`L5b_rc=127`，文本都是
`lx-verifyN.sh: line NN: cargo: command not found`。批 1 / 批 2 之所以能跑，是当时经
`bash -lc` 启动（登录 shell 会 source profile），批 3–5 改成了
`ssh host 'setsid bash script.sh'`——**非登录、非交互，`~/.cargo/bin` 不在 PATH**。
修法写进脚本本身，不依赖启动方式：`export PATH="$HOME/.cargo/bin:$PATH"`。
**推论（登记为口径约束）：这台机器上任何 rc=127 的相位都不产生测试结论**，
所以 §6.19 上面那三段（L3b/L4b/L5b）当时**一条测试都没跑**，Linux 侧真正的收据只有
L1 / L2 / L3 / L4 / L5 / L6 + 批次 7。

**L6（release 构建，第二个真实操作系统）**：`Finished release profile [optimized + debuginfo]
target(s) in 19m19s`（`CARGO_TARGET_DIR=~/rig-verify-20261019/target-release`，9.9 G，
`--locked`，源码 `95fb0949d`）。产物与哈希：

| 二进制 | sha256 | `--version` |
|---|---|---|
| `codex` | `0b3406df1f016702cdf2cc01175287981ae76efa8dcb2d8f71622df59148c5f7` | `codex-cli 0.0.0` |
| `codex-app-server` | `23664fe9025a3fa646c31bd1ad955e2f7d3c2e301f75a2cf7b96f0cc387b863d` | `codex-app-server 0.0.0` |

（`L6_rc=0` 不可信，见上；这里的凭据是 `Finished` 行 + 两个文件确实存在并能跑 `--version`。）
`warning: codex-cli (bin "codex") generated 1 warning` —— 只登记，不动它（不是本轮范围）。

**批次 7 / 8（不带 v8 依赖的相位，用来补 L4b/L5b 该给的答案）**：源码仍 `95fb0949d`，
`export PATH` 已修。三相位各自能证明什么写清楚：
`L7a` = `-p codex-rust-rig-bridge`（视觉 floor 修复就住在这个 package：
`wire_images.rs` / `wire_images_tests.rs` / `wire_budget_outbound_tests.rs`，全是 lib 单元测试，
不需要兄弟 binary）→ **task #5 的第二操作系统收据**；
`L7b` = `-p codex-exec-server -E 'test(shell_snapshot) | test(blocked_descriptor)'`
→ **B3 / task #22 第一次在真 Linux 上跑起来**；
`L7c` = L3 里 mac 基线**没有**的那 6 条（`network_approval` ×4、
`tools::sandbox_denied_exec_command_returns_original_output`、
`rig_image_history::anthropic_image_history_survives_drop_without_shutdown_resume`）
`-j 1` 单跑 → 判"争用 / 环境 / 真实"。同一脚本末尾带 **sandbox 能力探针**
（`apparmor_restrict_unprivileged_userns`、`unshare -Urm` 的真实 rc、
`kernel.unprivileged_userns_clone`），因为 `tools::sandbox_denied_*` 这类用例在
**userns 被 AppArmor 限制的机器上本来就跑不了**，没有这个探针就无法区分"争用抖动"与
"环境不支持"。L3 跑的时刻该机 `load average: 29.22, 29.61, 40.25`（72 核，另一个用户在
同机跑 `cargo nextest` + `cargo clippy --workspace`），批次 7 启动时
`load average: 45.82, 25.48, 26.81`——**共享负载如实登记，不作为已排除因素**。

- **L7a 没跑成**：`error: the argument '--locked' cannot be used multiple times`、`L7a_rc=2`。
  `run()` helper 已经带 `--locked`，我在参数里又加了一次。第五次自我造成的调用缺陷，
  与 §6.19 L4 同族（脚本本身的问题，不是产品）。重跑 = L8a。
- **L7b：45 tests run: 45 passed（`L7b_rc=0`）——B3 方向性结案**。CI ubuntu 那条红在
  **真 Ubuntu 上不复现**：

  > **这一条已被 §6.27 作废**：`blocked_descriptor_path` 在 `.41` 上从未执行——bwrap userns
  > 探测命中 `USER_NAMESPACE_FAILURES[0]`，用例打印 `skipping sandbox test` 后 `return Ok(())`，
  > nextest 仍记 PASS。下面的"真 Ubuntu 机器 PASS 0.085s"是跳过式绿，不是复现结果。

  ```text
  PASS [   0.085s] (15/45) codex-exec-server::exec_process shell_snapshot::
    shell_snapshot_concurrent_replays_keep_independent_readers::blocked_descriptor_path
  ```

  对照三方（同一条 `--workspace`/`--locked` 语义，run `39347345560`，head `6e27a6c11`）：

  | 腿 | 该用例 | 同腿其余 |
  |---|---|---|
  | ubuntu CI runner | **FAIL 0.346s**（该腿 2 条红之一，另一条是 `::discovery` 60.783s） | `Summary [3004.229s] 22927 tests run: 22925 passed, 2 failed, 48 skipped` |
  | macos CI runner | **PASS ×3**（0.310 / 0.223 / 0.295s，含 mac 专属 zsh 两条） | `Summary [2646.520s] 22877 tests run: 22876 passed, 1 failed, 71 skipped` |
  | 真 Ubuntu 机器（本机，`95fb0949d`） | **PASS 0.085s** | `Summary [11.309s] 45 tests run: 45 passed, 646 skipped` |

  **同 OS、不同机器给出相反结果**，所以这一条既不是 Linux 平台缺陷、也不是分叉造成的
  字段遗漏，四分法落第 4 类（CI runner 环境 / harness）。但按 §6.9 的纪律说清楚：
  **这只到"真机不复现"为止**，runner 上的根因（`0.346s` 的快速失败、只在该 runner 出现）
  仍未取到，不冒领"已修"或"与分叉无关"。§7 第 3 项里 B3 的状态从"待真机复现"改为
  "真机不复现、runner 根因未取"。
- **L7c 的读数（`Summary [18.138s] 6 tests run: 1 passed, 5 failed`、`L7c_rc=100`）**：
  - `rig_image_history::anthropic_image_history_survives_drop_without_shutdown_resume`
    单跑 **PASS** → L3 里那条是负载/时序形状。
  - `tools::sandbox_denied_exec_command_returns_original_output` **FAIL**，文本
    `expected sentinel output from command to reach the model` +
    `bwrap: loopback: Failed RTM_NEWADDR: Operation not permitted`。机器能力探针同批给出：
    `unshare: write failed /proc/self/uid_map: Operation not permitted`（`unshare_rc=1`）、
    `kernel.unprivileged_userns_clone = 1`。⇒ **这台机器的非特权 userns 被挡，codex 的
    Linux sandbox 起不来**，而该用例的前提正是"sandbox 拒绝这条命令"。前提不成立的机器上
    它必红：**环境不支持，不是产品缺陷**。对本 fork 的目标有直接意义，登记为事实：
    **在没有非特权 userns 的容器/宿主里，`sandbox_denied_*` 这一族用例整体失效**
    （Linux/容器 app-server 是 fork 的交付面，这条要跟着部署文档看）。
  - `network_approval::…` **4 条 FAIL**，文本是语义形状而非超时：
    `assertion failed: (left == right)`（`network_approval.rs:910`、`:2322`）、
    `expected Guardian approval, got TurnComplete(…"risk_level":"low",
    "user_authorization":"high","outcome":"allow"…)`（`:995`）、
    `Error: timed out waiting for both Guardian network reviews`。
    而 CI ubuntu 腿**这 4 条同名用例全绿**（`background_…::later_turn`、
    `background_…::same_turn`、`guardian_receives_exact_trigger_for_single_network_request`、
    `guardian_receives_exact_triggers_for_concurrent_network_requests` 各 1 次 PASS），
    mac/windows 腿没有这些名字。**⇒ 差异只能来自机器**（sandbox 起不来 / 或负载 45），
    但 L7c 本身跑在 `load average: 45.82` 下，**"隔离"不等于"空闲"**，所以现在只能记
    **未定性**。L8b 在 `load1 < 12` 的空闲条件下重跑这 5 条（4 条 `network_approval` +
    `tools::sandbox_denied_*`）并打印前后负载：
    转绿 → 归"争用 + sandbox 能力"，登记为环境；仍红 → 回到根因，不许结案。

**批次 8 把上面两个悬念各自结案（真机、空闲、有机制）**

- **L8a = `codex-rust-rig-bridge` 全套，真 Linux**：
  `Summary [15.489s] 337 tests run: 337 passed, 0 skipped`、`L8a_rc=0`。
  这个 package 就是视觉 floor 修复 `04beac00a` 的住处（`wire_images.rs` /
  `wire_images_tests.rs` / `wire_budget_outbound_tests.rs`），也是
  `uses_model_bridge()` 的下游。**⇒ B1 / task #5 的第二操作系统收据拿到**，
  与 L1(140) / L2(5) 一起构成"本轮两处改动在第二个真实操作系统上全绿"。
  （L7a 那条 rc=2 是重复 `--locked`，见上。）
- **L8b = L7c 那 5 条，空闲机重跑**：`Summary [16.532s] 5 tests run: 0 passed, 5 failed`、
  `L8b_rc=100`，结束后 `load after: 2.03 8.35 18.27`。
  **⇒ "负载/争用"这一半被排除**，剩下的必须是机制。机制用**系统 bwrap / unshare**
  直接量出来（不依赖测试框架），批 8 之后又单独开一次只读 ssh 复量并落盘
  `~/rig-verify-20261019/lx-verify-20261020c-probes.log`（L8c，2026-10-20T04:37:09Z），
  下面每一行都在 L8c 里逐字复现：

  ```text
  /proc/sys/kernel/apparmor_restrict_unprivileged_userns = 1     # Ubuntu 24.04 默认
  /proc/sys/user/max_user_namespaces                = 235952     # 名额不是瓶颈
  /sys/module/apparmor/parameters/enabled           = Y
  bwrap --ro-bind / / --unshare-net /bin/true  -> bwrap: loopback: Failed RTM_NEWADDR: Operation not permitted  (rc=1)
  bwrap --ro-bind / /              /bin/true  -> bwrap: setting up uid map: Permission denied                    (rc=1)
  unshare -Urm true                            -> write failed /proc/self/uid_map: Operation not permitted       (rc=1)
  ```

  L8c 另外把环境本身钉住：`Ubuntu 24.04.4 LTS (noble)`、`Linux 7.0.0-38-generic`、
  `bubblewrap 0.9.0`、`apparmor 4.0.1really4.0.1-0ubuntu0.24.04.5`、
  `kernel.unprivileged_userns_clone = 1`（内核允许，挡在前面的是 AppArmor 那条），
  以及 `unshare -Un /bin/true` **rc=0**（命名空间建得出来，卡在写 `uid_map`）。
  **这台机器的 bwrap 带网络和不带网络都起不来**，根因是
  `apparmor_restrict_unprivileged_userns=1`（Ubuntu 24.04 的默认限制），不是资源不足。
  于是 5 条红全部有了机制：
  - `tools::sandbox_denied_exec_command_returns_original_output` 期望的"sandbox 拒绝这条
    命令"根本没机会发生——sandbox 自己在 `RTM_NEWADDR` 处失败，模型收到的是 bwrap 错误文本，
    于是断言 `expected sentinel output from command to reach the model` 红。
  - 4 条 `network_approval` 期望"出站网络请求被拦下 → Guardian 触发审批"；network namespace
    建不起来 ⇒ 请求不被拦 ⇒ Guardian 一次都没被触发（`:2322` 的 `assert_eq!` 比的是触发列表，
    `:910` / `:995` 看到的是 turn 直接 `Completed`）。**失败文本是语义形状**
    （`expected Guardian approval, got TurnComplete(…"outcome":"allow"…)`），与超时无关——
    这正是"sandbox 未生效"的形状。

  **四分法：第 4 类（环境能力），不是产品缺陷**；CI ubuntu runner 上这 4 条全绿
  （那里 userns 可用）就是反证。
- **对本 fork 交付面的直接含义（登记为事实，不是待办）**：验证机与目标运行环境必须
  **允许非特权 userns**，否则 codex 的 Linux sandbox 起不来、`OnRequest` 的网络审批链路
  整体退化。容器/宿主上要么把 `kernel.apparmor_restrict_unprivileged_userns` 置 0
  （需 root、属安全策略变更，**本轮没有在这台机器上改**），要么给容器 `CAP_SYS_ADMIN` 等
  相应权限。这条与 §6.10 的 unix 期限讨论无关，是独立的部署要求。

### 6.20 用户点头的那一轮 dispatch 已跑完：四份收据全部到手，另有 10 条首观新红要单独定性

**run `39371971338`**：event `workflow_dispatch`、head `55fcee7cb`（= `origin/test`）、
2026-10-20T04:53:38Z → 07:40:43Z（**2h47m05s**）、总 verdict failure。按授权**只发这一手**，
没有因为看到红而补跑。日志是 run 结束后才允许下载的（在跑的时候
`gh run view --log-failed` 回 `run … is still in progress`，已实测）。

| 腿 | 预算 / 实耗 | 测试阶段 | 结果 |
|---|---|---|---|
| `bazel bridge gate (ubuntu)` | 60m / 29m48s | — | **成功** |
| `nextest (ubuntu-latest)` | 180m / 1h23m59s | 2861.494s | `22928 run: 22927 passed (24 slow), 1 failed, 48 skipped` |
| `nextest (windows)` | 150m / 2h00m37s | 3014.459s | `21760 run: 21751 passed (21 slow, 254 leaky), 8 failed, 1 timed out, 138 skipped` |
| `nextest (macos-latest)` | 330m / 2h46m56s | 3313.645s | `22878 run: 22870 passed (27 slow), 8 failed, 71 skipped` |

三腿都在 `Test (workspace)` 这一步报 failure，**没有 cancelled、没有编译失败**，
所以红点全部是"可读的用例结果"。

**四份收据逐条结案**

1. **R5（`codex-tui` ABORT）结案**：windows 日志里 `ABORT` 计数 = **0**
   （`grep -c ABORT`）。基线 `39347345560`（head `6e27a6c11`）的退出块里有 3 条
   `codex-tui … - with code 0xc00000fd: Recursion too deep; the stack overflowed. (os error 1001)`。
   ⇒ `6e27a6c11` + `cefb81d57`（把 stack 预算放进真正设置它的那条 recipe）这两笔生效。
2. **§6.15（mac daemon 窗口）结案**：`codex-cli::app_server_daemon` 在这一腿
   **18 条 PASS、0 条 FAIL/TIMEOUT**。其中 `packaged_daemon_start_and_explicit_replacement`
   实测 **PASS 173.300s**，比旧窗口（60s / 120s / 180s 三道 kill）都高，
   还带一条 `SLOW [>120.000s]` 标记；`981490315` 那条 mac-only `2m × 2` 覆盖规则
   必须排在两条 `packaged_daemon_` 规则**之前**才有效（first-match-wins），这一点
   本轮被真实跑到底验证了一次。
3. **`::discovery` 在 CI 侧转绿**：`PASS [ 1.633s] ( 832/22928)
   codex-app-server::all suite::v2::account::external_auth_refreshes_on_unauthorized::discovery`
   （`u39371` 第 5692 行）。同一 runner、一手之隔的基线是
   `FAIL [ 60.783s] ( 996/22927)`（`u39347`）。windows 与 mac 的失败集里这条也消失了。
   ⇒ **`95fb0949d` 的 CI 收据到手**，与验证机 L2 的 `PASS 2.695s` 两个独立来源一致。
4. **`8349736da` 的整机独占没能改变结果——这一半要收回**：四条 `legacy_` 仍全红，
   且与"没有独占"的基线几乎同形：

   | 用例 | 基线 39347345560 | 本轮 39371971338（已整机独占） |
   |---|---|---|
   | `legacy_capture_cancellation_terminates_descendants_without_timeout` | FAIL 34.563s | FAIL **32.056s** |
   | `legacy_capture_emits_output_and_preserves_descendant_after_normal_exit` | FAIL 11.472s | FAIL **12.515s** |
   | `legacy_tty_job_terminates_and_preserves_descendants` | FAIL 11.051s | FAIL **14.163s** |
   | `legacy_workspace_write_delete_is_limited_to_writable_roots` | FAIL 0.130s | FAIL **0.302s** |

   ⇒ 我把"CPU 争用足以 blow 掉 `unified_exec/tests.rs:905` 那条 10s 硬墙钟"写进了
   `nextest.toml` 的注释，**这个解释在 runner 上被证伪**：`threads-required` 全部保留后
   前三条反而没有变快。剩下的可能只有"这条 assert 在 windows-latest 上本来就太紧"
   或"descendant 真的没按预期退出/存活"，两者都是产品/用例侧，不是调度侧
   （登记为 task #27，注释里那半句错误解释也要跟着改）。
   `write_delete` 那条 0.3s 失败仍然与 #13 同一族（快速、确定性、语义断言）。

**顺带补一手 ubuntu 侧的既有待办**：这一腿唯一的红就是 `blocked_descriptor_path`，
本轮 `FAIL [ 25.298s] (11226/22928)`，基线是 `FAIL [  0.346s] (11032/22927)`——
**同一条用例两次用时差了 73 倍**，而真 Ubuntu 上这个 `test_case` 是 45/45 全绿（L7b）。
⇒ #23 需要的"runner 侧根因"又多一条形状证据：它不是确定性的快速失败，
没有第二手 runner 日志不许结案。

**本轮新增的 10 条首观红（都不追绿，先分桶）**

- **mac 8 条**：5 条 `suite::compact::program_tests::model_switch_program_pair::*`
  + 3 条 `suite::pending_input::queue_only_agent_mail_wakes_sleeping_root_with_previous_turn_context::reloaded_*`。
  **8 条 panic 点完全相同**：`core/tests/common/lib.rs:395:14`，文本
  `timeout waiting for event: Elapsed(())`——即 helper 里
  `timeout(wait_time.max(Duration::from_secs(10)), codex.next_event())` 的 10s 下限被击穿。
  这 8 条在**同一轮 ubuntu 腿**分别是 17 / 5 条 PASS；两簇时间戳
  （07:10:25–07:10:47、07:15:58–07:16:09）正好与 `app_server_daemon` 的 173s 腿收尾、
  以及 `daemon_startup` 一批 20–38s 的腿重叠。
  ⇒ 归"**期限形状 + 同车道争用**"，但这是相关性、不是证明；要把"我的窗口放宽"与
  "mac runner 本来就慢"分开，只能靠一次受控复跑（登记为 task #26）。
- **windows 2 条**：
  - `codex-core agent::control::tests::spawn_agent_can_fork_parent_thread_history_with_sanitized_items`
    FAIL 3.340s，panic 在 `core\src\agent\control_tests.rs:3139:5`，diff 是
    developer / `<managed_developer_instructions>` / `<persistent_mode>` 合并后的历史内容，
    **不是期限**。ubuntu 同轮 PASS，windows 前两轮（39286738661、39347345560）PASS。
  - `codex-core::all suite::code_mode::code_mode_excludes_mcp_servers_using_their_configured_identity`
    `TERMINATING [> 60.000s]` → `TIMEOUT [  60.054s]`，stdout 只有 `(test timed out)`，无诊断。
    命中的是 nextest **默认**窗口（`.config/nextest.toml:5`
    `slow-timeout = { period = "30s", terminate-after = 2 }`），不是我加过的任何一条 override；
    ubuntu 同轮 PASS 1 次。
  ⇒ 两条都是首观、都落在我本轮没碰过的文件里，先按"**未定性，需要第二手**"登记，
  不许写成"与我无关"。

### 6.21 Windows `legacy_` 家族：独占预订被自己的 A/B 证伪，改成放宽三条内部预算

**预订（`8349736da` 的 `threads-required = "num-test-threads"`）确实生效，但没用。**
两轮 head：无预订 = 39347345560（`6e27a6c11`），有预订 = 39371971338（`55fcee7cb`）。

| 腿 | 无预订 | 有预订 |
| --- | --- | --- |
| `legacy_capture_cancellation_terminates_descendants_without_timeout` | 34.563s FAIL | 32.056s FAIL |
| `legacy_capture_emits_output_and_preserves_descendant_after_normal_exit` | 11.472s FAIL | 12.515s FAIL |
| `legacy_tty_job_terminates_and_preserves_descendants` | 11.051s FAIL | 14.163s FAIL |
| `legacy_workspace_write_delete_is_limited_to_writable_roots` | 0.130s FAIL | 0.302s FAIL |

预订生效的直接证据（时间窗比对，不是推断）：
- 39371971338 里那条 32.056s 腿的窗口 `06:52:02.540–06:52:34.596` 内，全日志只有
  `codex-windows-sandbox` 自己的判定行，index 21585→21586 相邻，中间没有别的包。
- 39347345560 里同一条腿的窗口 `01:54:26.154–01:55:00.717` 内，
  `codex-utils-image legitimately_large_dimensions_still_decode`（36.177s PASS）
  与一批别的判定行在同窗报出 ⇒ 没预订时确实并发。

⇒ 并发被完全排除之后，四条腿的时长一位数都没变；预订的代价是
windows `Test (workspace)` Summary 由 **2383.014s 涨到 3014.459s（+631.445s）**。
机制改判：**hosted runner 上"沙箱孙进程把 pid marker 写出来"的启动成本**，
不是与其它测试抢 CPU。§6.12 R4 里"争用是充分原因"的口径按本节作废；
`8349736da` 的注释跟着撤掉。三次连续跑（38824922110 / 39347345560 / 39371971338）
签名逐字相同、时长彼此接近（32–34.6s / 11.3–12.5s / 10.9–14.2s），
这也是"确定性环境特征"而不是"抖动"的依据。

**但这三组数字要分清哪些是测量、哪些只是旧上限。** 每条腿都在**自己的**预算耗尽时
返回，所以：cancellation 腿的 32–34.6s 说明 marker 出现时间 **> 30s**（它撞的是 30s
capture 上限，`wait_for_process` 把 `timeout_ms` 原样传给 `WaitForSingleObject`，
`lib.rs:872`）；另两条腿的 11–14s 撞的是它们自己的 10s 上限，**只能证明 > 10s**。
⇒ CI 历史数据给的是下界，没有上界。60s/90s 是留的余量，不是"实测值的 1.7 倍"这种
已经量出来的东西；够不够，由本轮 push 触发的 run 来回答（代码注释里按这个口径写，
不写成测量结论）。panic 现场的对应关系（`w39371.txt` 23808 / 23828 / 23868 行）：
`tests.rs:710:10` → `read descendant pid: Os { code: 2, kind: NotFound }`，
`tests.rs:987:5` → `Terminate descendant did not start`，
`tests.rs:905:5` → `cancellation should end capture before the timeout`。
`/Volumes/zhitai/tmp/qoder-scratch/w39371.txt` 还确认了超时路径会 `job.terminate()`
杀掉整棵树（`lib.rs:881-898`），所以"capture 超时 ⇒ marker 读不到"是必然，
不是另一种故障；而正常退出路径会 `preserve_descendants()`（`lib.rs:899-904`），
这正是这条腿要断言的行为。

**修复（`windows-sandbox-rs/src/unified_exec/tests.rs` 三处，逐条对准 panic 点）**

| panic 点 | 原因 | 改动 |
| --- | --- | --- |
| `tests.rs:905`（cancellation） | `started_at.elapsed() < 10s`，而 capture 只能在 marker 出现后被取消唤醒 | `elapsed` 10s→**60s**，capture `Some(30_000)`→**`Some(90_000)`** |
| `tests.rs:710`（capture emits output） | capture 10s 超时后立刻 `read descendant pid`，marker 还没写 | capture `Some(10_000)`→**`Some(60_000)`** |
| `tests.rs:987`（tty job） | `wait_for_path(ready_marker, 10s)` "descendant did not start" | marker 等待 10s→**60s**，spawn `Some(30_000)`→**`Some(60_000)`** |

我自己差点制造的新坑，记下来：**第一版把 capture 抬到 180s**。nextest 默认
`slow-timeout = { period = "30s", terminate-after = 2 }`（`.config/nextest.toml:5`）
在 60s 直接杀进程 ⇒ 有诊断的 FAIL 会退化成裸 TIMEOUT，正是 §6.15/§6.19 一路在消灭的形态。
所以两条一起做：capture 收到 90s，同时给这一族加
`platform = 'cfg(windows)'` 的 `slow-timeout = { period = "2m", terminate-after = 2 }`
（kill 在 4m），把"内层预算之和 < 外层窗口"这条一致性约束写进注释。

顺带核了一下方向性风险：`wait_for_process` 的取消路径是**先查 `is_cancelled()` 再等**、
每轮最多睡 50ms（`windows-sandbox-rs/src/lib.rs:624-644`），所以放宽 capture 超时
**不会**推迟取消被察觉，也不会把这三条腿推成 §6.14 那条 1s "survived" 竞态。

本地验证边界（诚实）：mac 建不了这个 Windows-only crate，`.235` 上没有 `codex-rs` 克隆，
所以语义验证只有一条路——按分工交给 CI（本轮由一次真实 push 触发，不重新 dispatch）。
本轮能做到的门：
- rustfmt 解析通过且我的改动零格式 diff（`-p codex-windows-sandbox`、`-p codex-core`
  各 `Diff in` 计数 0）；
- nextest 配置用 `/opt/homebrew/bin/python3.14 + tomllib` 解析（默认 profile 26 条
  override），并按"第一条命中生效"逐条模拟了 4 条腿在 windows/ubuntu/macos 三平台上
  解析到的窗口：`legacy_` 两族 → idx 13 的 2m×2（仅 windows，另两平台仍走默认，
  因为这条 override 带 `platform = 'cfg(windows)'`）；`code_mode_excludes_mcp…` →
  idx 20 的 1m×2（三平台一致，且前面没有更早命中的规则）。
  这条模拟顺带纠正了一次我自己的误判：粗匹配 `package(codex-core)` 时 idx 18 的
  `binary(windows_sandbox)` 会被漏掉，看上去像是 code_mode 已有 1m×3；补上 `binary`
  谓词后确认它走的是默认窗口——与 `w39371.txt` 里 `SLOW [> 30.000s]` →
  `TERMINATING [> 60.000s]` 的实测形状一致。
- **自己写的 1m×2 被同一套预算检查推翻，改成 2m×2**：一致性约束是
  "一条腿内层预算之**和** < 外层窗口"，而 `legacy_tty_job_terminates_and_preserves_descendants`
  是**两个** lifecycle 顺序跑的（`tests.rs:1052-1054`），每个
  `60s marker + 15s collect + 10s survival + 10s exit` ⇒ 最坏 ≈190s，
  120s 窗口会在它跑满时把它杀成裸 TIMEOUT——正是本轮要消灭的形态。
  改成 `period = "2m", terminate-after = 2`（kill 4m）后，最坏 190s 有余量；
  cancellation 腿 90+10=100s、capture 腿 ≈60s+20s 更不必说。
  代价核算过：这两族正常只有 10–35s，2m 才报 SLOW，windows 测试阶段 2383s 的量级
  不受影响；job 上限 150m。
- `cargo check -p codex-core --tests --offline`（mac，5m07s，rc=0、0 warning，
  含 `core_test_support`）⇒ §6.22 的 helper 重写是可编译的。
- 交叉编译这条 Windows-only crate  checked 过但**放弃**：`rustup target add` 只往
  系统盘 `~/.rustup` 装（用户明确要求构建产物不落系统盘），且会改本地工具链。
- `threads-required` 现在只剩注释里的文字引用，配置项已撤。
- 已知紧耦合（登记，不粉饰）：外层窗口是从内层预算**推**出来的，不是量出来的。
  若某条腿真的需要跑满内层预算（tty 190s / cancellation 100s），它离被外层杀掉
  只剩 50s / 140s；真出现那种腿，收据的形状仍然会是 TIMEOUT。本轮之后要不要继续
  按实测重排这两组数字，等 push 触发的 run 说话。

### 6.22 macOS 那 8 条 `timeout waiting for event`：本机不复现，先把诊断做成可归因

受控复跑（`mac-l9.sh` / `mac-l10.sh`，日志
`/Volumes/zhitai/tmp/qoder-scratch/mac-l9.log`、`mac-l10.log`）：

| 腿集 | M-L9a 空闲 `-j 1` | M-L9b 默认并发 + 同机编译负载 |
| --- | --- | --- |
| 5 条 `model_switch_program_pair` + 3 条 `reloaded_*`（共 22 条同名族） | **22/22 PASS**，Summary 43.219s，最慢 4.026s | **22/22 PASS**，Summary 12.216s，最慢 12.184s |
| M-L10a 同 22 条（helper 已重写后） | **22/22 PASS**，Summary 43.539s | — |

两条必须写清的边界：
1. **M-L9b 想加的负载没加上。** 我的过滤器
   `test(/app_server_daemon|daemon_startup/)` 在 `codex-cli` 里只匹配到 1 条
   0.019s 的 `app_server_daemon_subcommands_parse`（`m-l9b-load.log` 1 条 PASS、
   `load_rc=0`）。真正压住这 22 条窗口的是 `codex-cli` 的 2m11s rustc 编译。
   ⇒ "车道被 daemon 腿挤满"这个假设**仍未检验**，§6.20 那条相关性推测不能升格。
2. 有意义的读数仍然是比值：同 22 条腿从空闲单线程的 0.65–4.03s 涨到并发下的
   6.08–12.18s，而 CI 失败点是 helper 里**每个事件间隔**的 10s 下限
   （`core/tests/common/lib.rs:381-401`：`wait_for_event` 用 1s 调它、被 `max(10s)` 抬到 10s）。
   本机 12 核并发下整腿最慢 12.18s 仍没触发，hosted 4 核 mac 触发了。

一个反直觉但改变判读顺序的实测：**`tokio::time::timeout` 先 poll 未来，再等 deadline**。
M-L10b 把下限强制改成 `max(Duration::from_nanos(1))`（日志里 `let deadline =
wait_time.max(Duration::from_nanos(1));` 已确认生效）重跑 `reloaded_root`，
结果 **1 passed（2.930s）**——事件已在队列里时 1ns 的 deadline 也不会失败。
⇒ CI 上那 8 条是"下一个事件 10s 真的没到"，不是"并发排队把预算吃光"；
这既排除了纯排队解释，也让我不能拿"本机不复现"当作产品无关的证据。

处置：只修可归因性，不动任何预算语义。helper 原来两个 `.expect()` 只会留下
`timeout waiting for event: Elapsed(())`，既不报 deadline 也不报最后一个真正收到的事件。
改成记录 `deadline`、事件计数与最后一个 `EventMsg` 变体名（`event_msg_name`，
只在 panic 路径上做一次 `{:?}`，稳态零开销），另加
`stream ended unexpectedly: {err:?}`。收据：M-L10a 22/22 PASS（编译 + 无回归）、
M-L10c 逐字节还原后同腿 PASS（`helper_restored=byte_identical`）。
8 条本身**不追绿**，留待下一轮 CI 用新文案判定"卡在哪个事件"。
→ 补记：**新文案已在本机实测出来过**（§6.25，cap 1ms 触发，
`last event: ThreadSettingsApplied`），所以本节这笔改动不再是"没跑过的代码"；
8 条腿自身的定性仍未变，等本轮 CI。

### 6.23 Windows 两条新项：一条是长期贴线漂移，一条未定性；顺带查出一个真实的构建机缺陷

**`code_mode_excludes_mcp_servers_using_their_configured_identity`——不是首观缺陷。**
windows 腿的历史：48.878s PASS（38824922110）→ 51.826s PASS（39347345560）→
**60.054s TIMEOUT**（39371971338）。同轮 ubuntu 48.768s PASS、macOS 17.717s PASS。
它一直贴在默认 30s×2 的 kill 线下面 8–11s，这条腿的真实成本就是"启动 MCP 服务器"。
处置：给它 `slow-timeout = { period = "1m", terminate-after = 2 }`（不加 platform 键，
因为 ubuntu 也只有 11s 余量），让这条腿在 120s 内跑完并给出自己的断言输出，
而不是被杀成一个不带诊断的 TIMEOUT——与 §6.15 对 `external_auth…::discovery` 的
处理同一形态。**2m 是余量、不是量出来的**：那条 TIMEOUT 只把真实成本界定在 >60s，
没有上界；本轮 push 触发的 run 若仍在 120s 处报 TIMEOUT，就说明 MCP 启动成本又涨了，
那时才需要去看是哪台服务器停住（这条腿本身不打印 per-server 计时，属于下一手）。

**`spawn_agent_can_fork_parent_thread_history_with_sanitized_items`——未定性，列观察项。**
windows 3.340s FAIL；同轮 ubuntu 1.499s PASS、macOS 1.598s PASS；同一条 windows 腿
上一轮（39347345560）PASS。断言是 `control_tests.rs:3139` 的内容比较，diff 里
**177 行全是 `<`（左多）、0 行 `>`** ⇒ 实际子历史比期望**多出四条消息**，
都带父 `turn_id: 01a157f6-0dd0-75a4-bfb8-69906284c889`，逐条是：
developer `<permissions instructions>` + `<persistent_mode>…no longer apply`、
user `<environment_context>`（`cwd=D:\a\codex\codex\codex-rs\core`、`shell=powershell`）、
developer `<multi_agent_mode>…no longer applies`、
developer `<managed_developer_instructions>…no longer apply`。
四条全是父会话启动时注入的那一组初始上下文 / "先前指令已失效"墓碑，**整体追加在尾部**
⇒ 形状是"父初始上下文在子历史快照之后又落了一次"，指向注入与快照的**先后时序**，
不是内容比对失手（这里没有 `>` 行，期望侧一条不缺）。
两轮 head 之间（`6e27a6c11 → 55fcee7cb`）的产品/测试代码 delta 只有三笔：
`95fb0949d`（rig 传输 pin，改 `core/src/client.rs` 与
`core/tests/suite/subagent_notifications.rs`）、`c27ec7653`（放宽 subagent hook-log 窗口）、
`9bad7cca0`（同一处注释）；这条用例自己的文件没动，非 Windows 专属，
也不在这条路径上注入上下文消息。但同一区间还有一条**调度**改动值得点名：
`8349736da` 给 windows `legacy_` 家族加了 `threads-required = "num-test-threads"`
（整机独占式预订，会串行化同批 nextest 线程）。一条对"注入 vs 快照先后"敏感的腿
被调度扰动翻红，是**候选机制**、不是已证明的因果；`aca3a703a` 已把那条预订删掉，
所以本轮 run 对这条腿恰好是一次"回到 39347 调度"的对照。
机制未证明，**不写成"与我无关"**，也不改断言：下一轮复现再深挖。

**但它那条 `<permissions instructions>` 里带 `\r\n`，顺着查出一个真实跨平台缺陷。**
CI 日志原文（39371971338，windows）：

```
text: "<permissions instructions>\nFilesystem sandboxing defines … Network access is restricted.\n# Escalation Requests\r\n\r\nCommands are run outside the sandbox …- Pipes: |\r\n- Logical operators: &&, " <3939 bytes>
```

同一个 fragment 前半用 `\n`、`# Escalation Requests` 段用 `\r\n`。前半来自 Rust 字面量，
后半来自 `prompts/templates/permissions/approval_policy/on_request.md`，
而 `prompts/src/model_messages/permissions.rs:14` 用 `include_str!` 把它**原样**嵌进
发给模型的指令。GitHub 的 windows runner 默认 `core.autocrlf=true`，检出即 CRLF。

实测链（`mac-l11.sh` / `mac-l11bcd.sh`，日志 `mac-l11.log`、`mac-l11bcd.log`）：

| 步骤 | 结果 |
| --- | --- |
| L11a 基线（LF 检出） | `codex-prompts` **41/41 PASS**（0.039s） |
| L11b 手工把 `on_request.md` 改成 CRLF（57 行带 `\r`） | **41/41 仍 PASS** ⇒ prompts 单测**不覆盖这些字节**，这个缺陷拦不住 |
| L11c `git checkout` 还原 LF | 0 行带 `\r`，41/41 PASS |
| L11d 无规则时 `git -c core.autocrlf=true checkout --force` | **57 行带 `\r`** ⇒ runner 机制在本地逐字节复现 |
| L11e 加规则后同一命令（`on_request.md`、`on_request_rule_request_permission.md`、`compact/prompt.md`） | **0 行带 `\r`**，`git status` 干净 ⇒ 修复有效 |

仓库现状佐证：`git grep -lI $'\r' HEAD -- 'codex-rs/**'` = **0 个文件** ⇒
blob 里全是 LF，CRLF 只在"检出 + `include_str!`"这一刻产生。也就是说
**同一份源码在不同构建主机上会产出不同的模型侧指令字节**（Windows 构建多 `\r`，
token 计数与文本都与其它平台不一致）。这条 fork 恰恰要发 Windows 制品，属于必修。

修复：`.gitattributes` 加 `codex-rs/prompts/templates/** text eol=lf`（这一条随
`38ace3fe1` 进入本轮 CI head），半径量完后另加四条模板目录（见下面的 M-L12–L14），
与仓库里已有的 `third_party/voice/sources.json text eol=lf` 同一形态。
分类：**真实跨平台缺陷（构建主机依赖的模型侧字节）**，已修；
与 §6.12 C 的 TUI 快照问题不是同一个。

**把这一类的半径量出来，再决定钉到哪里（`mac-l12.sh` / `mac-l13.sh` / `mac-l14.sh`）。**
一行修复只挡住了一条 CI 证据指向的那个目录，剩下的同类面是"猜"还是"量"，本轮按量处理。
方法：把 worktree 的 `.gitattributes` 移到一边（此时生效的只有已提交那份规则），
对 `codex-rs/**/templates/*` 与 `codex-rs/**/assets/*` 共 **106 个受版本控制的文件**
逐个 `rm` + `git -c core.autocrlf=true checkout --force`，再数带 `\r` 的行。

| 读数 | 内容 |
| --- | --- |
| M-L13 暴露面 | **85 / 106** 个文件在强制 autocrlf 检出下变成 CRLF，例如 `memories/write/templates/memories/consolidation.md` 880 行、`stage_one_system.md` 569 行、`ext/memories/templates/memories/read_path.md` 130 行、`collaboration-mode-templates/templates/plan.md` 128 行、`ext/goal/templates/goals/continuation.md` 56 行、`core/assets/consequential_tool_message_templates.json` 962 行 |
| M-L12 对照 | 同一条命令，`prompts/templates/on_request.md`（已被提交里的规则钉住）= **0 行**，而未钉的 `stage_one_system.md` = **569 行** ⇒ 这条属性是承重的，不是装饰 |
| M-L14 收口 | 五个目录全部钉上后，34 个模板文件在同样的强制 autocrlf 检出下 **cr_lines=0 / crlf_files=0**，`git check-attr eol` 每目录解析为 `lf`，`git status` 只剩这次改动本身 |

**半径的取舍（写明理由，不靠"顺手"）**：只钉 `include_str!` 真正嵌进二进制的
**prompt 模板目录**五个——`prompts/templates`、`memories/write/templates`、
`ext/goal/templates`、`ext/memories/templates`、`collaboration-mode-templates/templates`。
`codex-rs/core/templates/**` 六个文件同样会变 CRLF，但**全仓库没有任何 Rust 代码
`include_str!` 或按名字加载它们**（`git grep` 只命中一个无关的测试字符串），
不属于本轮已确认缺陷，不钉。
`core/assets/consequential_tool_message_templates.json`（962 行）与
`skills/src/assets/samples/**`、`login/src/assets/*.html`、`tui/assets/themes/*.tmTheme`
也**不钉**。JSON 那一条本轮就地判掉了：解析这份 blob（HEAD 版）得到
**452 个字符串值，其中含裸换行的 0 个**，全文 962 个 `\n` 全在结构空白位置上，
`\r` 落在结构位置是合法 JSON 空白 ⇒ autocrlf 检出不会改变它的解析结果，也不改变
提取出来的字符串值，**行为上没有缺陷**，因此不进属性表。
skills 样例是运行时落盘给用户执行的脚本、html/tmTheme 由浏览器与 plist 解析器消费，
CRLF 后果不同类，这三处按"遗留同类面"登记，不在收尾轮顺手扩大。

**这条修复与本轮那手 CI 的关系**：正在跑的 run `39396134392` 的 head `de8fdb3f4`
里只有 `codex-rs/prompts/templates/**` 一条（那正是 windows 证据指向的目录），
另外四条在它之后、带 `[skip ci]` 推上，不另开一手；它们改变的是 windows 构建机
读到的模板字节，没有测试断言依赖，所以交给下一次真实代码 push 顺带验证。

### 6.24 M-L15：windows `spawn_agent_can_fork…` 的 CRLF 假设——本机证伪成立

§6.23 里那条 windows 红只登记为"未定性"，但它的 diff 里那条 `<permissions instructions>`
带 `\r\n`，很容易顺手读成"CRLF 让父子历史去重失手"。本轮把这句话当成**可证伪的假设**
在 mac 上直接测（不占 CI 预算）：`mac-l15.sh`，日志 `mac-l15.log`，参数与 CI 同形
（`NEXTEST_PROFILE=local`、`RUST_MIN_STACK=8388608`、`--offline --locked --no-fail-fast`、
`-p codex-core --features codex-core/rust-rig`，过滤到这一条用例）。

| 腿 | 设置 | 结果 |
| --- | --- | --- |
| L15a | 模板为 LF（`on_request.md` **0 行**带 `\r`） | **PASS 2.192s**（编译 6m43s） |
| L15b | `prompts/templates/permissions/**/*.md` 共 **7 个文件**全改成 CRLF（`on_request.md` **57 行**带 `\r`） | `codex-prompts` 与 `codex-core` **确实重编**（`Finished … in 29.58s`，`include_str!` 的依赖指纹被 mtime 打断）⇒ 字节进了二进制，仍然 **PASS 1.686s** |
| L15c | 还原 LF 再跑 | **PASS 1.674s**，`git status` 只剩这次的文档改动 |

⇒ **假设被证伪**：CRLF 模板字节**不足以**让这条腿变红。L11b 已经证明同一套转换会让
渲染出来的 fragment 真的带上 `\r\n`（那是 §6.23 里 `on_request.md` → CRLF 的收据），
所以 L15b 不是"字节没生效"的空转，是产品代码在 CRLF 输入下依然把父子历史处理对了。

因此那行 `\r\n` 改读作 **windows 构建机上的常量、不是这一轮的差异量**：`core.autocrlf=true`
是 runner 检出期的默认、与具体 run 无关，而 `6e27a6c11 → 55fcee7cb` 之间没有任何改动模板
处理的提交 ⇒ 上一轮 PASS 的那条 windows 腿**同样**是带着 CRLF fragment 构建的。
**这条是机制推断、不是日志实证**：六条腿的日志里 `<permissions instructions>` 只在这一条
失败断言中打印过一次（`grep` 计数 w39371=1、w39347=0、old38824 的 2 次是 windows/ubuntu
**同一处** insta 快照，只到标签行、不带正文），所以没有"通过的 windows 腿打印同一 fragment"
的收据可对照。

对下一手的判据（本轮 run `39396134392` 就能读出，它同时改了两件事）：
head `de8fdb3f4` 已含 `prompts/templates/**` 钉版（windows 构建机这轮读到 LF）、
也已删掉 `threads-required` 预订（调度回到 39347）。
- 这条腿**仍红** ⇒ 直接确认 CRLF 与它无关（LF 构建下还红），剩下的候选只有调度/时序；
- 这条腿**转绿** ⇒ 也不能反过来记成"钉版修好了它"，因为 L15b 已经排除了充分性；
  绿只说明这条腿对 head 里的某项扰动敏感，与 §6.23 的时序候选一致。
两种结果都不改它的分类：**未定性、列观察项，`38ace3fe1` 不承诺让它变绿**。

### 6.25 M-L16：把 §6.22 那条新诊断真的"逼出来跑一遍"，补上 500ms / 1ms 两个读数

`d4d9f943a` 只给 `wait_for_event_with_timeout` 的超时分支加诊断。这类改动有个自身风险：
**如果新代码自己写坏了，它正好砸在它要救的那类失败上**。所以本轮不等 CI 的 mac 腿
"顺带"验证，而是在 mac 上把这条分支真逼出来（`mac-l16.sh` + `mac-l16b.sh`，
日志同名；参数与 CI 同形）。被点的用例是
`suite::approvals::approving_apply_patch_for_session_skips_future_prompts_for_same_file`
与 `suite::models_etag_responses::refresh_models_…`，临时改法是把
`wait_time.max(Duration::from_secs(10))` 就地换成 `wait_time.min(…)`，**只在工作区改、跑完立即
`git checkout` 还原，没有进入任何提交**。

| 腿 | 设置 | 结果 |
| --- | --- | --- |
| L16a | 助手原样（models_etag 用例） | **PASS 0.762s** |
| L16b | `deadline` cap 到 500ms（同一条用例） | **仍 PASS 0.586s** ⇒ 见"结论二" |
| L16c | 还原后再跑 | **PASS 0.650s** |
| L16d | cap 500ms，换成 approval 用例 | **PASS 1.215s** |
| L16e | cap **1ms**，同一条 approval 用例 | **FAIL rc=100**，panic 原文 `timeout waiting for event: Elapsed(()) (deadline 1ms, last event: ThreadSettingsApplied)` |
| L16f | `git checkout` 还原，同一条用例复跑 | **PASS 0.617s**，`git status` 只剩本轮文档改动 |

**结论一：诊断确实可用。** 三个字段都渲染出来了，`event_msg_name` 取到的是枚举变体名
（`ThreadSettingsApplied`），不是整段 Debug；CI 里那八条 mac 红若再遇到，日志会直接说出
"最后真正看到的事件"和生效的 deadline，不必重跑取信息。
`{elapsed:?}` 恒为 `Elapsed(())`、信息量全在括号里那两个字段——**不为此再动一次已提交的助手**，
那是装饰性改动，会让八条腿重新编译一遍而不换来任何判据。

**结论二（给 §6.22 的语义补一个中间读数，并记下我自己那次预期落空）**：
`deadline` 是**每一次 `next_event()` 拉取**的期限、不是整段等待的期限
（形状是 `loop { timeout(deadline, codex.next_event()) }`），这一点 §6.22 已经写过
（"`wait_for_event` 用 1s 调它、被 `max(10s)` 抬到 10s"、"**每个事件间隔**"）。
本轮新增的是**量**：我原本预期"cap 到 500ms 就会红"，实际 **L16b 与 L16d 在 500ms 下都 PASS**
（事件连续到达、每条间隔都小于期限），要压到 **1ms** 才红——**这个预期错得有据**，
错在我把"整腿耗时"当成了"单次拉取的等待"，正是 §6.22 那条语义没被我拿来算的地方。
⇒ 读数区间从 §6.22 的"1ns 也 PASS"补成"500ms 也 PASS、1ms 才 FAIL"，
CI 那八条的含义随之钉得更死：**报 `timeout waiting for event` 等于
"在 `max(wait_time, 10s)` 这段时间里一个事件都没到"**，而不是"想要的那个事件没在 10s 内到"。
所以搜索面**收窄到"harness 整段停滞"**（配置 IO、tool discovery、线程启动这一段），
**排除"模型响应慢一些"这一类解释**。

**第二操作系统没有复跑这组，如实登记**：`.41` 上此刻跑着与本项目无关的
`~/duck-linux` 全 workspace `nextest`（`ps` 实测 `cargo-nextest nextest run --locked --workspace
--no-fail-fast` 加一批 `rust-lld`，`load average` 154.93 → 177.65），不去挤别人的机器。
本轮 run 的 ubuntu 腿会编译并运行改过的 helper（等于覆盖"在第二操作系统上不炸"），
但**超时分支的实测只有 mac 一腿**。

### 6.26 run 39396134392 的收据：四判据逐条对上，并抓到一处我自己写出来的窗口不对称

**run 与日志**：head `de8fdb3f4`，`event=push`，终态 `failure`（12:05:11Z，mac 腿 196 min）。
三腿日志 `m39396.txt` / `u39396.txt` / `w39396.txt`（去 ANSI 版同名加尾缀 `c`，在 scratch），
另两手的 `m39347` / `u39347` / `w39347` / `m39371` / `u39371` / `w39371` 同目录在手；
`bazel bridge gate (ubuntu)` 绿。**一处工件缺陷先登记**：`w39347` 那份日志只枚举到它自己
Summary 里 11 条失败中的 8 条（我按 `(N/21759)` 去重实测），所以下表 windows 39347 一栏只引用
nextest 自己的计数、不引用失败名单。

| 腿 | 39347 | 39371 | 39396 | 本轮一手结论 |
| --- | --- | --- | --- | --- |
| macos | `2646.520s` 22876 passed(23 slow) / 1 failed | `3313.645s` 22870(27 slow) / **8 failed** | `3624.486s` 22877(29 slow) / **1 timed out** | §6.22 那八条全绿；新红只有一条压缩腿（已修） |
| ubuntu | `3004.229s` 22925(27 slow) / 2 failed | `2861.494s` 22927(24 slow) / 1 failed | `2039.363s` 22926(12 slow) / 2 failed | `blocked_descriptor_path` 形状不变；新增一条 ubuntu-only oauth 腿 |
| windows | `2383.014s` 21748(14 slow, 383 leaky) / 11 failed | `3014.459s` 21751(21 slow, 254 leaky) / 8 failed + 1 timed out | `2102.468s` 21752(12 slow, 266 leaky) / 8 failed | `spawn_agent…` 与 `code_mode…` 转绿；`legacy_` 四条改由自己的断言判出 |

上一手写下的四判据，**四条全部答上**：`legacy_` 四条这次全部由自己的断言判出（无一条裸
TIMEOUT）；`code_mode_excludes_mcp…` 三腿都在 2m 内跑完；mac 那八条全绿且新文案 `last event:`
在三腿日志里出现 **0 次**；`legacy_workspace_write_delete…` 形状不变。

**判据一：四条 `legacy_` 全部用自己的断言判定**——这是 `0541a7eb6` + `aca3a703a` 那一手唯一
要回答的问题，答案是"放宽生效"：

| 腿 | 本轮 wall | 报出的判据（原文） | `tests.rs` 行号 | 上一手对照 |
| --- | --- | --- | --- | --- |
| `legacy_capture_cancellation_terminates_descendants_without_timeout` | 45.077s | `assertion \`left != right\` failed  left: 0  right: 0` | 933 | 39371 32.056s / 39347 34.563s 报的是 `cancellation should end capture before the timeout` |
| `legacy_capture_emits_output_and_preserves_descendant_after_normal_exit` | 60.984s | `read descendant pid: Os { code: 2, kind: NotFound, message: "The system cannot find the file specified." }` | 716 | 同形状（39371 12.515s、39347 11.472s，行号 710） |
| `legacy_tty_job_terminates_and_preserves_descendants` | 61.160s | `Terminate descendant did not start` | 1010 | 同形状（39371 14.163s、39347 11.051s，行号 987） |
| `legacy_workspace_write_delete_is_limited_to_writable_roots` | 0.089s | `assertion failed: \`(left == right)\`: stdout=""`，left `… None, false` vs right `… Some("outside")` | 835 | 同形状（39371 0.302s、39347 0.130s，行号 829） |

四处行号位移（710→716、905→933、987→1010、829→835）**全部来自我自己 `0541a7eb6` 在这份文件
里的 +28/−5**，产品代码没动过。

**顺带查出一处我自己写错的夹具判据（必须登记）**：cancellation 那条腿旧的内层线是 30s，而它的
夹具把沙箱父进程写成固定 `Start-Sleep -Seconds 30`
（`codex-rs/windows-sandbox-rs/src/unified_exec/tests.rs:862-870`，孙进程由它派生、pid marker
由孙进程写）——**30s 的线卡在 30s 的睡眠上，这条腿在任何正常情况下都必然红**，旧那条断言从来没
测过"取消是否及时"。放宽到 60s 之后它第一次跑到真正的判据（`tests.rs:933` 的
`assert_ne!(result.exit_code, 0)`，本轮 `left: 0`），于是暴露出真正该测的事实：hosted runner 上
沙箱内第二个 pwsh 起得比父进程那 30s 睡眠还慢，孙进程没写出 pid marker，父进程正常退出、取消
从未落地。定性仍是**夹具与 runner 速度耦合，不是产品缺陷**。修法是把父睡眠拉长、让 60s 内层线
去判"marker 是否在 60s 内出现"（代价 = 一手 CI），本轮**没做**，因为本轮不为一条已定性的腿单开一手。

**判据二：`code_mode_excludes_mcp_servers_using_their_configured_identity` 不再需要放宽**。
windows 40.070s（39371 是 60.054s TIMEOUT）、ubuntu 29.266s、mac 30.801s 三腿全绿，1m×2 够用。
撤掉整机预订的收益也在这里：ubuntu 腿墙钟 2861.494 → 2039.363s、windows 3014.459 → 2102.468s，
SLOW 计数 24 → 12 与 21 → 12。

**判据三：mac 那八条 `timeout waiting for event` 本轮全绿，新诊断一次都没触发**。
`grep -c "last event:"` 在三腿日志里都是 **0**；八条腿本轮 PASS 且都落到 0.5–2.1s：
`model_switch_program_pair` 的 incomplete_rollback 10.717→**1.434s**、missing_program
10.466→**1.199s**、resume 10.452→**2.110s**、rollback 10.511→**2.121s**、standard
10.514→**1.587s**；`queue_only_agent_mail_wakes_sleeping_root_with_previous_turn_context`
的 reloaded_root 10.720→**0.789s**、reloaded_delegated 10.678→**1.143s**、reloaded_unknown
11.619→**0.552s**。⇒ §6.22 的定性不变（本机不复现、CI 也不复现），但"下一手 CI 会报出
`last event`"这个预期本轮**落空**：`d4d9f943a` 的诊断在 CI 里仍只有编译+运行覆盖，
唯一实测收据还是 §6.25 的 L16e。

**判据四 → 本轮 mac 唯一那条红，根因是窗口不对称，已修（提交 `24878ad8e`）**：
`local_compaction_retries_preserve_instructions_and_tool_metadata::metadata_enabled`
TIMEOUT 60.025s（`TERMINATING [> 60.000s]`）。跨腿把这条腿的历史摊开就看清了：

| 腿 | 39347 | 39371 | 39396 |
| --- | --- | --- | --- |
| `metadata_enabled` | windows 39.740 / mac 33.103s | windows **71.275s PASS** / ubuntu **50.876s** / mac 49.548s | windows 39.282 / ubuntu 28.858 / **mac TIMEOUT 60.025s** |
| `metadata_disabled_after_capture` | windows 23.850 / mac 16.435s | windows 41.256 / ubuntu 30.549 / mac 24.839s | windows 23.186 / ubuntu 16.130 / mac 28.483s |

`.config/nextest.toml` 给这条腿的 1m×2 窗口挂着 `platform = 'cfg(windows)'`（原注释写明它是为
windows 那条 52.614s 加的），于是 windows 的 71.275s 能过、**mac 与 ubuntu 一直骑在裸默认
30s×2 上**。这条用例把 300 × ~8 KiB 的工具调用项过三遍重试（`core/src/compact_tests.rs:58`），
代价是三腿同形，不是 mac 特性——所以修法是删掉那行平台过滤，不是再给 mac 单开一条。

**M-L17：本机把"删过滤到底有没有生效"做成 A/B/C**（`mac-l17.sh` / `mac-l17.log`，参数与
`just test` 同形：`NEXTEST_PROFILE=local`、`RUST_MIN_STACK=8388608`、
`--features codex-core/rust-rig`、`--locked`、`--no-fail-fast`）。窗口不对称这种问题不能靠
"配置读起来对"下结论：

| 腿 | 设置 | 结果 |
| --- | --- | --- |
| L17a | 最终文件 + 窗口临时收到 `1s`×1、**无**平台过滤 | 两条腿都 `TRY 2 TMT [1.004s]`、`2 timed out` ⇒ **窗口现在绑定 macOS 了** |
| L17b | 同样 `1s`×1，把 `platform = 'cfg(windows)'` 加回 | **PASS 10.983s / 19.034s** ⇒ L17a 那两刀只来自"删过滤"这一件事，不是规则顺序或首匹配 |
| L17c | 最终状态（1m×2、无过滤） | **PASS 11.645s / 18.321s**；`diff -q` 与暂存副本一致，`git status` 只剩这一份 toml |

同一段读数顺带钉住这条腿的独占成本：mac 本机 11.0–11.6s 与 18.1–19.0s（三次独立跑），CI 上
33–71s ⇒ 整条 lane 的竞争把它乘 2–4 倍，"2m 是余量不是实测"这句仍成立。
**复现者必须带 `RUST_MIN_STACK=8388608`**：不带它这两条腿直接 `has overflowed its stack` /
SIGABRT（本机实测 `TRY 2 ABRT [0.967s]`），那是栈预算问题不是速度问题。
CI 级收据（mac 腿在 >60s 时不再被裸杀）留给下一手真实代码 push——本轮不为一行窗口配置单开一手。

**mac 腿整体变慢只是背景，不是这条红的根因**：同一区间 ubuntu 与 windows 反向变快
（2861.494→2039.363、3014.459→2102.468s），而我没动过 core 的压缩代码；39347 与 39396 两条
mac 腿的 Runner Image banner 完全相同（`macos-26-arm64` / `20261013.0424.1`）⇒ **不是 image 变更**。

**三条新首观**：

1. **windows**
   `suite::v2::config_requirements_exec::managed_unified_exec_disabled_preserves_approved_command_execution`
   FAIL 10.767s，`app-server\tests\suite\v2\config_requirements_exec_tests.rs:136` 报
   `(Failed, Some(124))` vs `(Completed, Some(0))`。命令是
   `"C:\Program Files\PowerShell\7\pwsh.exe" -Command 'echo managed-exec-ok'`，夹具自设
   `timeout_ms: 10_000`（`:65`）而读超时是 60s（`:36`）——124 就是被它自己的 10s 命令超时杀的，
   与本轮 pwsh 族（conpty 11.447s、`legacy_` 30–60s）同一机制。**但这条验的正是我们改过的托管
   exec 路径**，"排除归属 ≠ 证明无关"在这里要写明：判死它需要在 .235 复跑同一条（那台机器 pwsh
   冷启动 0.024–0.026s），或把 `timeout_ms` 从 10s 放宽；本轮两者都没做，只登记。
2. **ubuntu** `suite::v2::account::login_account_chatgpt_uses_oauth_overrides` FAIL 0.197s：
   `error sending request for url (http://127.0.0.1:1455/auth/callback?code=test-code&state=…)`
   → `client error (Connect)` → `Connection refused (os error 111)`。同一腿本轮 mac 0.269s /
   windows 0.400s PASS、ubuntu 上一手 0.423s PASS ⇒ ubuntu 首观。
   0.197s 排除"端口被占"与防火墙：**测试 GET 的 `redirect_uri` 是产品按 `actual_port` 自己报出来的**
   （`login/src/server.rs:194`），所以它先成功占了 1455、随后 1455 上没人了。代码里能让这件事发生的
   只有一条路径——`bind_server` 撞 `AddrInUse` 时会向端口上现有的监听者发一条 `GET /cancel`
   （`login/src/server.rs:620-635`、`:657-666`），而 `/cancel` 的处理是
   `HandledRequest::ResponseAndExit`（`login/src/server.rs:530`），即**别的进程能把现有持有者的监听器
   关掉**；CI 用的 `profile.local` 又把 app-server 集成腿的并发放到 4
   （`codex-rs/.config/nextest.toml:60-61` 与 `:114-118`）。
   **但本轮日志不支持"确实有兄弟腿并发了"**：紧邻的四条 `login_account_chatgpt_*` 腿在
   09:16:51.72x 就完成了，而本腿起于 51.728（= 51.925 − 0.197）、止于 51.925，这些数字把它们排除在
   并发窗口之外。所以这条仍是**未定性（一次观测）**，机制是代码里读出来的候选解释、不是本轮实证。
   M-L18 本机复跑（`mac-l18.log`）：mac 上按 `profile.local` 并发跑完 `login_account_chatgpt_` 全族
   10 条，`1.452s` **10/10 全 PASS**、本尊 0.147s ⇒ 本机不复现（与 §6.22 那八条同样的处境）。
   不改代码、不放宽窗口——这不是速度问题，放宽只会把竞态藏得更深。
   下一手判据：再红时要拿到那一刻的并发名单（junit 里有起止时间戳），或在 .41 上并发复跑同族；
   本轮 `.41` 仍被别人的 `duck-linux` 全 workspace 跑占用，不去挤。
3. **mac** `local_compaction_retries_preserve_instructions_and_tool_metadata::metadata_enabled`
   ——不是"新红待定性"，就是判据四那条窗口不对称，已修并附 M-L17。

- **§6.26 这一手：CI 收据到手 + 一处我自己写出来的窗口不对称已修**。前面那笔登记的
  "run id 与三腿终态按收据在下一笔文档提交里登记"在这笔兑现——`39396134392`（head
  `de8fdb3f4`）三腿终态、四判据逐条答复与三条新首观全部写在 §6.26。本轮唯一的 mac 红
  根因是 `.config/nextest.toml` 里那条 1m×2 窗口挂着 `platform = 'cfg(windows)'`：同一
  条腿在 windows 靠它跑到 71.275s PASS，而 mac/ubuntu 一直骑在裸默认 30s×2 上，于是 mac
  在 60.025s 被裸杀。修法只删那行平台过滤（**代码提交 `24878ad8e`**，1 文件 +10/−6），
  本机 M-L17 的 A/B/C 证明"生效来自删过滤"这一件事：1s×1 无过滤 → 两条腿 1.004s TMT；
  1s×1 复原过滤 → PASS 10.983s / 19.034s；最终 1m×2 无过滤 → PASS 11.645s / 18.321s。
  **CI 级收据仍未到手**（>60s 的 mac 腿能不能跑完，只有 CI 能给），按"本轮不为重新跑 CI
  而无意义地提交"交给下一次真实代码 push 顺带覆盖。
  同段另附 M-L18：为 ubuntu 那条 oauth 首观在 mac 上并发跑完 `login_account_chatgpt_` 全族
  10 条，`1.452s` 10/10 PASS（编译 3m22s，零 CI 预算），故那条仍是未定性一次观测。
  §6.26 里还登记了三件我自己的错：cancellation 夹具那条 30s 内层线卡在自家 30s 睡眠上
  （必然红，已改为等真正的 `assert_ne!`）、`w39347` 日志只枚举到它自己 11 条失败中的 8 条、
  以及本轮"CI 会报出 `last event:`"的预期落空（八条全绿，诊断 0 次触发）。
- **这两笔一度推不出去，阻塞在本机网络上（如实登记，不是仓库或凭据问题）**：
  第一次 `git push origin test` 两条传输都失败——SSH over 443 报
  `Connection closed by 20.205.243.160 port 443` / 重试时 `Connection timed out during
  banner exchange`，显式走 HTTPS 报 `LibreSSL SSL_connect: SSL_ERROR_SYSCALL in connection
  to github.com:443`。同一时刻的只读探针：`gh auth status` 正常（`soddygo`，token 有
  `repo` 作用域），`curl -4 https://github.com` 得到 **200 / 0.67s**，而 `curl https://github.com`、
  `api.github.com`、`objects.githubusercontent.com`、`example.com` 全部 `000` 超时；
  默认路由在 **`utun4`**（Surge 的 TUN，fake-IP `198.18.0.1`），github.com 解析到
  `20.205.243.166`、无 AAAA。⇒ 判定是**本机代理隧道部分失效**，与本项目无关，
  因此没有擅动代理状态。**推送后来已完成**：出网恢复后 `api.github.com` 从 `000` 回到
  200 / 0.41s，`git push origin test` 一次成功（`3a68e29ef..d5ca43484 test -> test`），
  期间没有调整代理、也没有用重试之外的其他手段。
- **推送后的指针与 `[skip ci]` 的第三次实测**：`git fetch origin test` 后
  `origin/test` = **`d5ca43484`**（本笔）、`git rev-list --count origin/test..test` = **0**、
  工作树 clean，其前依次是 `24878ad8e`（窗口修复）、`3a68e29ef`、`5f69816b4`、`fd0020bb0`、
  `de8fdb3f4`。`gh run list --branch test` 最新一手仍是 39396134392（head `de8fdb3f4`、
  08:49:05Z），这手 push **没有产生新 run** ⇒ tip 提交消息带 `[skip ci]` 即跳过整个 push
  事件，与 §7 早前两次实测同一条行为，这里是**第三次实测**而不是预测。
  于是 §6.26 那句"CI 级收据留给下一手真实代码 push"继续成立：`24878ad8e` 已在远端，
  但它是跟着 `[skip ci]` 的 tip 一起推上去的，本身没有换来一次 CI 跑。ACP 副本本轮未动。

### 6.27 把 `blocked_descriptor_path` 拆成两种形状，并作废我自己那条"真 Ubuntu 不复现"收据

**先更正我自己**：§6.26 的三车对照表把这条腿写成"形状不变"，实测是**两种形状**：

| run | 该腿原文（截关键行） | 用时 |
|---|---|---|
| 39347345560 | `panicked at exec-server/tests/exec_process/shell_snapshot.rs:344:13:` + `""` | `FAIL [ 0.346s]` |
| 39371971338 | `Error: exec-server transport disconnected: failed to read websocket JSON-RPC message from exec-server websocket ws://127.0.0.1:45713: WebSocket protocol error: Connection reset without closing handshake; failed to resume exec-server session: recovery timed out after 25s` | `FAIL [25.298s]` |
| 39396134392 | 同上，端口 `43037` | `FAIL [25.251s]` |

`shell_snapshot.rs:344` 是 8 路并发里的
`assert!(output.ends_with(&format!("restored:input-{index}")), "{output:?}")`，panic 消息就是 `""`
⇒ 至少一路 stdout 为空；25s 那两手来自**会话恢复**路径，与"空 stdout"不是同一次失败。四轮里我
把它们当同一形状登记，是我的错。

**这条腿为什么独享传输族错误**（新事实，此前没登记）：`create_process_context(deny_fd_path)`
的第二个参数其实是 `use_remote`（exec-server/tests/exec_process.rs:93-107），而
`shell_snapshot_concurrent_replays_keep_independent_readers` 家族里只有 `DenyFdPath` 传 `true`
（shell_snapshot.rs:224）——`::bash` / `::bash_tty`（`SnapshotSandbox::None`）与
`bash_protected_transport*`（`WorkspaceWrite` 自己建 `Environment`）都不起 websocket server。
⇒ 全家族只有这一条腿带"独立 exec-server 子进程 + 25s 会话恢复"，其余四条在同样三轮里一直
PASS（0.151 / 0.721 / 0.706 / 0.175s）。

**四轮结不了案的原因，是日志里那句话有三个来源且逐字节相同**（本轮真实修复，提交
**`a4dfd599f`**）：`client_recovery.rs` 的 `recover()` 里，

1. `wait_for_process_starts()` 25s 没排空 in-flight process start ⇒ 旧文本
   `…: recovery timed out after 25s`（现 :376）；
2. 单次 `resume_once` 跑过整个预算 ⇒ **同一文本**（旧 :415，现 :421）；
3. 预算内所有重连尝试都失败 ⇒ **同一文本**（旧 :429，现 :438）。

而且第 3 类把 retryable 错误本身丢掉（`is_retryable_recovery_error` 判真后不记录），所以
"服务端已经没了、连不上"与"客户端排空卡住"在 CI 日志里是同一个字符串。修法只分岔文本 +
记下最后一次 retryable 错误：`:376` → `process starts still in flight after 25s`；
`:421` → `no resume attempt finished within 25s (last attempt: X)`；
`:438` → `no resume attempt succeeded within 25s (last attempt: X)`。
**25s 预算本身不动**：`:60-63` 的注释写明它是给服务端 30s retention 留余量的刻意值，放宽只会把
等待拉长；`#[cfg(test)]` 那份 500ms 与集成测试无关（集成测试编译到的是 `not(test)` 的 25s）。
全仓 grep 过：除本文件外没有任何代码、快照或 TS 侧断言这段文本（`my-docs` 里的引用除外）。

**归属**：这条用例由上游带来（`c98e263fb` #48078 引入 unnamed-file 回放、`8f0b05910` #48187 加
`DenyFdPath` 腿），`wait_for_process_starts` 来自 `cf17e1bc2` #28512，`client_recovery.rs` 的提交
全部带上游 PR 号。按 §6.9 的纪律说清：这**只排除"分叉新写坏的"**，不证明"与分叉无关"；本轮也没
宣称根因已修，只把"看不懂"改成"看得懂"。

**作废：L7b 的 `blocked_descriptor_path` 绿是跳过式绿。** 在 `.41` 上带 `--nocapture` 复跑同一条腿
（同一克隆 `95fb094`、warm target、编译 1.43s）：

```text
skipping sandbox test: Codex's Linux sandbox uses bubblewrap and needs access to create user namespaces.
test shell_snapshot::…::blocked_descriptor_path ... ok
    PASS [   0.090s] (1/1)
```

机制是确定的：`.41` 上 `bwrap --unshare-user --unshare-net --ro-bind / / /bin/true` 的 stderr 是
`bwrap: loopback: Failed RTM_NEWADDR: Operation not permitted`，正好命中
`sandboxing/src/bwrap.rs:35-40` 的 `USER_NAMESPACE_FAILURES[0]`，于是 `system_bwrap_warning`
返回 `Some`，测试在 `shell_snapshot.rs:201-209` 直接 `return Ok(())` ⇒ nextest 记 PASS。同一
selection 复跑（`--test exec_process -E 'test(shell_snapshot) | test(blocked_descriptor)'`）
**29 条里有 7 条**打印同样的 skip。
⇒ §6.14 批次 7 里"B3 在真 Ubuntu 上不复现 ⇒ 四分法落第 4 类（runner 环境 / harness）"的推论
**作废**：那条件腿当时根本没执行，"同 OS、不同机器给出相反结果"的前提不成立。L7b 没带
`--nocapture`，nextest 不打印通过腿的 stdout，所以那行 skip 从未进日志——是我读收据的方式错，
不是工件缺字段。这条腿现在**没有任何 Linux 真实执行收据**。

**跳过式绿到底有几条（`-j 1 --nocapture` 串行复跑 L7b 的完整 selection，同一克隆 `95fb094`，
`.41:~/rig-verify-20261019/lx-l7b-nocapture.log`）**：总数与当初完全一致
（`Summary [ 24.806s] 45 tests run: 45 passed, 646 skipped`），但 45 条里有 **7 条**打印
`skipping sandbox test` ⇒ 这 7 条从未执行：
`shell_snapshot_concurrent_replays_keep_independent_readers::{blocked_descriptor_path,
bash_protected_transport, bash_protected_transport_tty}`、
`shell_snapshot_v2_capture_failure_falls_back_and_retries::{remote_pipe_recovery,
remote_tty_recovery, remote_retry_budget_exhausted}`、
`shell_snapshot_v2_filters_profile_exports_and_stays_in_memory::remote_sandbox`。
⇒ 那份"45/45 全绿"里有 38 条是真执行、7 条是跳过，而**本案恰好落在跳过的 7 条里**。
（`START (n/45)` 序号与当初不同，nextest 的启动顺序不保证，不影响结论。）

**更难看的一点，如实登记**：能力证据当时就在同一份日志里——`lx-verify7.log:369-371` 给出
`unshare: write failed /proc/self/uid_map: Operation not permitted`、`unshare_rc=1`、
`kernel.unprivileged_userns_clone = 1`。我在**同一批的 L7c** 正是用这三行把
`tools::sandbox_denied_*` 判成"环境不支持"，却在 500 行之外把 L7b 的 45/45 当成真机复现。
不对称的根源是：**同一件环境限制，L7c 那族用例会大声红，L7b 这族 guard 静默返回 Ok**。
⇒ 立一条读收据的规矩，后续所有 Linux 侧沙箱相关用例都按它执行：**凡结果依赖
`system_bwrap_warning` 保护的用例（`exec-server/tests/exec_process.rs` 的 :215/:391/:608/:761/
:906/:971/:1045 与 `shell_snapshot.rs:202`，共 9 处 guard，全在 `codex-exec-server` 的
`exec_process` 这一个 test binary 内），Linux 收据必须带 `--nocapture` 取，否则 PASS 不可解释。**

**并且 `.41` 不能当这条腿的第二操作系统**：CI ubuntu 车道每手先执行
`sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0`（39347 00:25:28 / 39371 04:56:26 /
39396 08:51:06 三手日志都回显 `kernel.apparmor_restrict_unprivileged_userns = 0` 与
"unprivileged userns re-enabled"），而 `.41` 该值 = 1（`lx-verify-20261020c-probes.log:2`；
`kernel.unprivileged_userns_clone = 1` 但被 AppArmor 拦）。改那台机器的安全配置不在授权范围内，
我没有动。⇒ 要拿 Linux 真实执行只剩两条路：CI runner，或请机主临时置 0（要用户去说）。

**mac 侧是真实执行**（非 Linux 的 `system_bwrap_warning` 是 `sandboxing/src/lib.rs:67-73` 的 `None`
stub，不跳过）：本机 M-L19 在改动后的工作树上跑该 selection ⇒
`Summary [2.344s] 3 tests run: 3 passed, 81 skipped`（`blocked_descriptor_path` 加 mac 专属 `zsh_*` 两条）。

**改动的回归对照（本机 M-L19，`-p codex-exec-server -E 'test(recover) | test(resume) |
test(reconnect) | test(disconnect)' --no-fail-fast`）**：

| 版本 | Summary |
|---|---|
| 未修改基线（`git stash` 后重跑） | `32.593s` 62 run: 58 passed (1 slow), **4 failed**, 638 skipped |
| 带改动 `a4dfd599f` | `31.398s` 62 run: 58 passed (1 slow), **4 failed**, 638 skipped |

4 条红**逐名相同**：`remote::noise_tests::reconnect_reuses_registration_until_url_is_rejected`、
`accepted_websocket_interoperates_and_recovers_with_real_direct_executor::{policy_unavailable,
transport_disconnect}`、`initialize::discovery_prewarms_before_initialize_and_recovers_without_client_opt_in`
⇒ 改文本零行为影响。**这 4 条不是"本机既有红"，是争用形状**：同一台机器单跑复验全部转绿
（M-L19，`-E 'test(accepted_websocket…)'` ⇒ `Summary [10.813s] 2 tests run: 2 passed`；
另两条单跑 ⇒ `Summary [11.057s] 2 tests run: 2 passed`，11.053s / 5.477s），而 run 39396134392
三腿在 62-leg 并发下全 PASS（mac 6.000 / 2.595 / 5.238 / 0.739s；ubuntu 6.078 / 2.143 / 2.180 /
0.221s；windows 5.669 / 0.141 / 0.169 / 0.217s）⇒ 不追绿，只登记一条读收据的规矩：**本机
`-p codex-exec-server` 并发选择集里出这类红，先单跑复验再定性**，与 §6.25 的 mac 腿争用结论同形。

**下一手 CI 已经是真代码 push**：run **39430526421**（`event=push`、head `a4dfd599f`、13:27:21Z
入队）。它同时覆盖三件悬着的事——这条腿若再红会直接报出属于哪条分支（并带最后一次 retryable
错误）、§6.26 悬着的 `local_compaction_retries_preserve_instructions_and_tool_metadata` 跨腿窗口
收据（#34）、ubuntu oauth 回调腿的第二手观测（#35）。本轮不再单独 dispatch。

### 6.28 方案 C 落地：`NUWAX_BRIDGE` 让环境变量能选上游原生 Responses 传输

**用户拍板**："C. 纯 env 的 B" —— 官方 GPT 经中转时，要有一个变量能开启原生 codex
Responses 请求方式。落地为启动组第 9 个变量 `NUWAX_BRIDGE`（值域同 provider 的
`experimental_bridge`：`rig`|`native`|`genai`），种子层随之写入该键
（[nuwax_env.rs:264-275](file:///Volumes/zhitai/git_workspace/fork-nuwax-codex/codex-rs/utils/cli/src/nuwax_env.rs#L264)）。

**语义选择**（避免"静默忽略"这一本轮一直在追的缺陷类）：

| 组合 | 结果 |
|---|---|
| `native` + `NUWAX_WIRE_API=responses` | 走上游原生传输；`uses_model_bridge()` 返回 false |
| `native` + `chat`/`anthropic` | **启动 fail-fast**：`NUWAX_BRIDGE = "native" requires NUWAX_WIRE_API = "responses"`（这两条线只由桥服务，接受即等于吞掉变量） |
| `rig` / 未设置 | fork 默认（第三方 Responses 经 rig 桥）；未设置时**不写**该键，白名单外的既有行为零变化 |
| `genai` + `responses` | env 层放行，由 genai 桥运行期明确报错——不在 env 层复制桥的策略知识 |
| 未知值 / 空白 / 非 Unicode / 孤立设置 | fail-fast 报变量名，不回显值，不发请求 |

**隔离面同步**（新增键不能成为跨层注入的入口）：`ENV_GROUP_SEED_KEYS` 8→9
（[env_group_isolation.rs:23-33](file:///Volumes/zhitai/git_workspace/fork-nuwax-codex/codex-rs/config/src/env_group_isolation.rs#L23)），逐层来源硬错照旧 —— 新测
`environment_bridge_selection_is_legal_but_foreign_layers_cannot_override_it` 证明 config
文件、`-c`/thread-config 层写 `experimental_bridge` 一律失败且只报键名；daemon 与 live 子进程
启动剥离（[pid_start.rs:128](file:///Volumes/zhitai/git_workspace/fork-nuwax-codex/codex-rs/app-server-daemon/src/backend/pid_start.rs#L128)、
[live-tests runner.rs:81](file:///Volumes/zhitai/git_workspace/fork-nuwax-codex/codex-rs/live-tests/src/binary_turns/runner.rs#L81)），
共享守护进程不得继承某个客户端的传输选择。

**首败 → 修复后证据**（本机，离线，`RUST_MIN_STACK=8388608`）：

| 目标 | 首跑 | 修复后 |
|---|---|---|
| `-p codex-config --lib env_group` | 11 run 中 **1 FAILED**：`config layers are not in correct precedence order`（我把 user 层排在 EnvSeed 之上，层栈要求低→高） | `11 passed; 0 failed; 376 filtered out` |
| `-p codex-utils-cli --lib` | — | `59 passed; 0 failed; 1 ignored`（含 4 条新桥测试） |
| `-p codex-cli --bin codex doctor` | 自查删掉我写坏的一行 `assert_eq!(...wire_api.codex_model_provider_info::WireApi::Responses, ())` 后才编译 | `141 passed; 0 failed; 1 ignored; 184 filtered`，新腿 `doctor_reports_the_environment_bridge_switch_as_the_native_transport` ok |
| `-p codex-app-server --test all nuwax` | — | `13 passed; 0 failed`（3 条 live 腿自动跳过），含新腿 `native_bridge_env_switch_sends_the_responses_wire_without_the_rig_capture` |
| `-p codex-exec --test all nuwax` | — | `20 passed; 0 failed; 112 filtered`（fail-fast 矩阵 +2 条：`unknown bridge`、`native bridge on the chat wire`） |
| `-p codex-app-server-daemon --lib start::tests` | 首次按 `pid_start` 过滤命中 0 条（模块名是 `start`） | `2 passed; 0 failed; 1 ignored`（13 项剥离断言 + 双客户端不串味） |
| `-p codex-cli` 其他被改测试目标 | — | admin_startup_matrix 6 / queue 9 / queue_owner_matrix 5 / queue_owner_dispatch 3 / queue_writer_lifecycle 2，全 passed |
| `cargo fmt --all --check`；`clippy -p utils-cli -p config --all-targets`；`clippy -p cli -p app-server -p exec -p app-server-daemon -p live-tests --all-targets` | — | fmt 干净；clippy exit 0，仅两条**既有** warning（`cli/src/cloud_config.rs:39` unused `network_policy`、`app-server/tests/suite/v2/analytics.rs:142` nonminimal_bool），均不在本轮改动文件内 |

**真二进制收据**（debug `codex` 构建，`CODEX_HOME` 用临时目录，base URL 指向
`127.0.0.1:9` 死端口，凭据是假值，零厂商调用）：

```text
NUWAX_BRIDGE 未设 ⇒ bridge                   rig (model bridge)
NUWAX_BRIDGE=native ⇒ bridge                   native transport
两腿共同：provider source  NUWAX_* environment group（temporary, per-run credentials）
          model source     NUWAX_MODEL environment
          endpoint (redacted) http://127.0.0.1:9        （假 key 不出现在任何输出）
```

**离线端到端为什么可信**：新 app-server 腿跑的是生产二进制子进程 + 本地 wiremock，断言
`POST /v1/responses`、`Authorization: Bearer <NUWAX_API_KEY 的值>`、`model`、
`max_output_tokens=2048`、`input` 数组且无 `messages`，并且**捕获账本文件不存在** —— 后者是
"确实离开 rig 桥"的唯一直接观测量（`CODEX_RIG_REQUEST_CAPTURE_FILE` 只由桥路径写出）。

**ACP 侧无需改动**：agent 用 `env: process.env` 透传（CodexJsonRpcConnection.ts:21），
`NUWAX_` 前缀天然进入 app-server 子进程；ACP 的 live driver 本来按前缀剥离宿主
`NUWAX_*`（`scripts/live-validation/live_support.py:123`），新变量自动受同一保护。

**边界**：本轮不跑实网 —— 官方 GPT 中转在 native 传输上的真实厂商收据**未采**；已证的是
"env 变量确实切换了传输，且切换后仍满足本 fork 的 provider 契约（凭据、模型、cap、Responses
形状、用量、取消/压缩走同一套 Core 栈）"。实网那一手留给后续经授权的一轮。

### 6.29 run 39430526421 的终态收据：`a4dfd599f` 的新文本一手没触发，mac 腿再次被取消

四判据（`event=push`、head `a4dfd599f`、13:27:21Z 入队、14:58:46Z 终态 `failure`；日志缓存
`qoder-scratch/39430526421.failed.txt`）：

| 车道 | 终态 | Summary / 区间 |
|---|---|---|
| `nextest (ubuntu-latest)` | failure | `2118.784s` 22928 run: 22927 passed (14 slow), **1 failed**, 48 skipped |
| `nextest (windows)` | failure | `2373.282s` 21760 run: 21753 passed (13 slow, 317 leaky), **7 failed**, 138 skipped |
| `nextest (macos-latest)` | **cancelled** | 13:27:21Z → 13:42:24Z ⇒ 本手**零 mac 观测** |

**§6.27 结尾押在这手上的三件事，逐条对上**：

1. **#34 关闭**（compaction 窗口放宽后的跨腿收据，§6.26 欠的那一手）：
   `codex-core compact::tests::local_compaction_retries_preserve_instructions_and_tool_metadata`
   两腿 ubuntu `17.292s` / `30.428s`、windows `24.755s` / `41.119s` **全 PASS** ⇒ 上一手
   "贴到 nextest 窗口"的解释不再是本案原因。mac 腿被取消，所以这条只有两平台各一手。
2. **#35 关闭**（ubuntu oauth 回调腿的第二手观测）：
   `suite::v2::account::login_account_chatgpt_uses_oauth_overrides` ubuntu PASS `0.306s`
   （上一手 u39396 亦绿），顺带拿到 windows 一手 PASS `0.696s`。
3. **#23 没关闭，且 `a4dfd599f` 这手没被触发**：ubuntu 唯一的红仍是
   `FAIL [   0.353s] (11036/22928) … shell_snapshot_concurrent_replays_keep_independent_readers::blocked_descriptor_path`，
   `panicked at exec-server/tests/exec_process/shell_snapshot.rs:344:13:` + 消息体只有 `""`。
   ⇒ 那条提交拆出来的三个 recovery 分支文本（`process starts still in flight after 25s` /
   `no resume attempt finished within 25s (last attempt: X)` / `no resume attempt succeeded …`）
   属于 **25s 那一形状**，本手走的是 0.35s 形状，一个字都没打印。`a4dfd599f` 到目前为止
   只有 §6.27 的本机回归对照，**CI 侧既未证实也未证伪**，不能登记成"CI 已验证"。

**六手缓存日志重读，把"每次都红、两种形状"钉成 3+3**（§6.27 只登了三手；本轮补上更早两手的
原文读数，逐个从缓存文件 grep 出来，不沿用上表记忆）：

| run | head | 该腿用时 | 机制（日志原文可复原的部分） |
|---|---|---|---|
| 39233543173 | `4ee470d5c` | `0.314s` | `shell_snapshot.rs:344` 断言，stdout `""` |
| 39286738661 | `7b1a92c6a` | `25.342s` | `exec-server transport disconnected: … Connection reset without closing handshake` |
| 39347345560 | `6e27a6c11` | `0.346s` | `:344` 断言，stdout `""` |
| 39371971338 | `55fcee7cb` | `25.298s` | 同 transport reset |
| 39396134392 | `de8fdb3f4` | `25.251s` | 同 transport reset |
| 39430526421 | `a4dfd599f` | `0.353s` | `:344` 断言，stdout `""` |

⇒ 六手六红、读数各聚成两簇（0.314/0.346/0.353 与 25.342/25.298/25.251），**没有一手给出第三种
读数**；把它们当"一条不稳定用例"仍然是错的。更关键的是：0.35s 那三手里的 runner 收据**不含
任何可归因字段**（只有 `""`），所以这一形状到目前为止**不可判定**。

**windows 的 7 条是同一批已登记尾项，集合只减不增**：`codex-uds lib_tests::{private_directory_acl_is_user_only_and_inherited_by_files,
socket_validation_rejects_broad_acl_without_repairing_it}`、
`codex-utils-pty tests::windows_tests::conpty_ctrl_c_interrupts_powershell_foreground_child`、
`codex-windows-sandbox unified_exec::tests::{legacy_capture_cancellation_terminates_descendants_without_timeout,
legacy_capture_emits_output_and_preserves_descendant_after_normal_exit,
legacy_tty_job_terminates_and_preserves_descendants,
legacy_workspace_write_delete_is_limited_to_writable_roots}`。
上一手的第 8 条 `suite::v2::config_requirements_exec::managed_unified_exec_disabled_preserves_approved_command_execution`
（39396 `10.767s` FAIL）本手 ubuntu `0.934s` / windows `2.340s` **转绿** ⇒ 与 §6.21/§6.23 的
"windows 尾项家族"结论同形，本轮零新增。#13 的 write-delete 腿仍在红：`0.089s`、
`assertion failed: (left == right): stdout=""`、diff `< None / > Some("outside")`。

**mac 腿再次以 cancelled 收尾，且这次连一个 step 都没有**：`nextest (macos-latest)` 的
`run_steps = null`、`steps.length = 0`、13:27:21Z → 13:42:24Z ⇒ **在排队阶段就被取消，从未开始执行**
（§6.11 那手是跑到 12699/22877 之后撞 3h 时限）。仓库的 `fork-cargo-pr.yml` 没有 workflow 级
`concurrency` 组，本 run 之后也没有更新的 run ⇒ 不是"新 push 顶掉旧 run"这种自我取消形状，
只能记为 runner 侧容量/取消事件，Annotations 不给原因。⇒ `01cfd6449` 的 `NUWAX_BRIDGE` 代码路径
**至今没有 CI mac 观测**，它的 mac 一手只有 §6.28 登记的本机收据。这条不影响本轮结论
（三腿里 mac 只是重复观测），但"mac 腿分片"这项待办（§6.11 升格、§7 未闭合项 2）
又拿到一手支持它的证据。

**本轮改动（提交 `c387b44dc`）：把 `:344` 那条断言改成能自证的工件**（`shell_snapshot.rs:342-350`）。旧消息
`"{output:?}"` ⇒ runner 侧只看到一个 `""`，既分不开上面两种形状，也分不开"进程没起来 /
起来了但 rc 不对 / 输出被谁吞了"：

```rust
assert!(
    output.ends_with(&format!("restored:input-{index}")),
    "{shell}#{index}: output={output:?} stderr={errors:?} status={status:?} \
     closed={closed:?}"
);
```

**本机验证（M-L19，改动后工作树）**：
`cargo test -p codex-exec-server --test exec_process shell_snapshot_concurrent_replays`
⇒ `11 passed; 0 failed; 73 filtered out`（`6.85s`，含 `blocked_descriptor_path` 与 mac 专属
`zsh_*` 变体）⇒ 断言条件、并发形状（8 路）、沙箱策略都没动，只把已有的四个已绑定变量送进消息。
**下一手 push 之后的 ubuntu 日志将自带这条腿的 stderr/rc**，#23 才有结案材料；这笔改动
未 push，所以它本身还没有 runner 侧观测。

**第二操作系统（真 Ubuntu，`.41`）的 `NUWAX_BRIDGE` 收据**（分支 `qoder-verify-20261021` @
`486ed23`，`CARGO_TARGET_DIR` 与 `TMPDIR` 都指该机外接盘，离线、零厂商调用、零 CI 预算；
`/home/swufe/rig-verify-20261019/lx-verify9.log`，脚本 `qoder-scratch/lx-verify9.sh`）：

| 腿 | selection | 读数 |
|---|---|---|
| L9a | `-p codex-utils-cli -p codex-config` | `0.243s` **439 run: 439 passed**, 1 skipped（rc=0） |
| L9b | `-p codex-app-server-daemon -E 'test(start::tests)'` | `0.430s` 2 run: 2 passed, 80 skipped（rc=0） |
| L9c | `-p codex-exec --features codex-core/rust-rig -E 'test(nuwax)'` | `12.808s` 20 run: **20 passed**, 162 skipped（rc=0） |
| ↳ L9c 逐条读数 | | 20 条全部真实执行（`0.038s`–`12.803s`），该 selection 内**不含 `_live` 家族** ⇒ 没有"缺网关即 return"的跳过形状；含三协议出站 `nuwax_env_{chat,responses,anthropic}_wire_reaches_the_*_endpoint`（`0.814s` / `0.842s` / `0.797s`） |
| L9d | `-p codex-app-server --features codex-core/rust-rig -E 'test(nuwax)'` | `6.481s` 13 run: 11 passed, **2 failed**（rc=100） |
| L9e | `-p codex-cli --bin codex --features codex-core/rust-rig -E 'test(doctor)'` | `5.300s` **136 run: 136 passed**, 177 skipped（rc=0） |

- **L9d 的 11 条绿要按 §6.27 的规矩拆开读**：其中 3 条是 `suite::v2::nuwax_positive_wires_live::`
  家族（`live_env_compact_and_post_compact_recall` `0.030s`、
  `live_env_output_cap_exhaustion_fails_without_resampling` `0.030s`、
  `live_env_tool_round_trip_usage_and_interrupt_with_two_resumes` `0.034s`），
  用时只有个位毫秒 ⇒ 是"本轮不跑实网"下没有本地网关时的自动跳过，**不计入执行证据**。
  真实执行的是 8 条 PASS + 2 条环境红。同 selection 在 mac 本机是 `13 passed; 0 failed`
  （§6.28 表第 3 行，那 3 条同样为跳过）⇒ 两平台的**执行**集合一致，差异只在沙箱。

- **本轮新增的解析/隔离/端到端腿在 Linux 上是真的执行了，不是跳过式绿**（逐名点名）：
  `nuwax_env::tests::bridge_selection_seeds_the_transport_override` `0.018s`、
  `native_bridge_requires_the_responses_wire` `0.012s`、
  `invalid_or_orphaned_bridge_names_the_variable` `0.013s`、
  `non_unicode_bridge_names_the_variable` `0.012s`、
  `env_group_isolation::tests::environment_bridge_selection_is_legal_but_foreign_layers_cannot_override_it` `0.033s`、
  `config_toml::bridge_config_tests::builtin_bridge_selection_is_accepted_without_overriding_endpoint` `0.019s`、
  以及新端到端腿 `suite::v2::nuwax_positive_wires::native_bridge_env_switch_sends_the_responses_wire_without_the_rig_capture`
  **`1.000s` PASS** ⇒ "env 选原生 Responses 传输 + 不产生 rig 账本"这一判据在第二个操作系统上
  成立，且它**不依赖 bwrap**（只断言 wire 形状与账本文件不存在）。
- **L9d 的 2 条红 = §6.27 那件环境限制，不是产品缺陷也不是本轮改动**：
  `chat_env_group_executes_a_tool_round_trip_over_the_real_chat_wire`（`1.307s`，
  `nuwax_positive_wires.rs:348`）与
  `responses_env_group_executes_a_tool_round_trip_over_the_real_responses_adapter`（`1.220s`，
  `:513`）的 panic 载荷都是被 exec 的那条命令自己的输出：
  `Process exited with code 1 … Output: bwrap: loopback: Failed RTM_NEWADDR: Operation not permitted`。
  读数当场独立复核（不靠推断）：`cat /proc/sys/kernel/apparmor_restrict_unprivileged_userns` = `1`，
  `bwrap --unshare-user --unshare-net --ro-bind / / /bin/true` 的 stderr 逐字符等于上面那句，
  而该句正是 `sandboxing/src/bwrap.rs:35-40` 里 `USER_NAMESPACE_FAILURES[0]` 的原文。
  **与 blocked_descriptor 那条的区别要说清**：那 7 条 guard 靠
  `system_bwrap_warning` 静默 `return Ok(())`（跳过式绿），这两条是"沙箱里真去 exec、
  把 bwrap 的失败当业务输出带回来"⇒ 大声红。同一件环境限制两种表现，
  正是 §6.27 立那条规矩的原因。⇒ 这两条**在 `.41` 上永远不可能绿**，除非机主把该 sysctl 置 0；
  按 §6.27 的结论，`.41` 仍不能当 bwrap 依赖腿的第二平台，能当的是**不依赖沙箱的**这些腿。
- **L9d 的两红与本轮改动无因果**（同一 selection 在 mac 本机 `13 passed; 0 failed`，§6.28 表第 3 行；
  两条失败腿的名字与 `NUWAX_BRIDGE` 无关，走的是 chat / responses 的 rig 桥工具回环）。
  按"排除归属 ≠ 证明无关"的纪律，这里给的是**正面对照**（同 selection 另一平台全绿 + panic 载荷
  指向 bwrap），不是仅"我没碰那段代码"。
- **doctor 的 env 传输诊断腿在 Linux 成立**：L9e 的
  `doctor::model_config_tests::doctor_reports_the_environment_bridge_switch_as_the_native_transport`
  **`0.133s` PASS**。这条是 §6.28 那两判据的载体（循环内含 `None` ⇒ `bridge: rig (model bridge)`
  与 `Some("native")` ⇒ `bridge: native transport` 两侧），所以 Linux 一手同时覆盖了
  "默认仍走桥"与"显式 `NUWAX_BRIDGE=native` 离开桥"两种诊断输出。L9e 总计 136/136 绿。
- **`c387b44dc` 的断言改动不在 `.41` 的读数里**：L9 批次不含 `-p codex-exec-server`
  （那条腿在 `.41` 上按 §6.27 的结论本就不能当第二平台证据），它只有 mac 本机 `11 passed`
  与下一手 runner 自证两个来源。

## 6.30 A 档第一批实施：exec 侧 native 一手（`bb2c0f9eb`）与跳过式绿的自证工件（`6c176ecec`）

用户要求"分析下看如何实施"，并授权 push。分析派了四个只读取证方向（exec 侧夹具、
bwrap guard 全量、两条 Windows 红、CI 分片与 rusty_v8），先记三条**被取证推翻的旧前提**，
因为它们直接改变了该做什么：

- **A6 的阻塞前提不成立**。此前登记"A6 前置：分片 target 目录不带 `codex-exec`
  source-receipt 二进制"。实际 `codex-rs/exec/build.rs:23-65` 对 `.git` 的
  HEAD/branch/packed-refs/index 与每个 workspace member 目录都下了 `rerun-if-changed`，
  checkout 之后这些 mtime 必然新于缓存恢复的产物；而且 CI 本来就显式
  `cargo build --locked -p codex-exec --bin codex-exec`
  （`fork-cargo-pr.yml:142-143`、windows 腿 `:248-249`）。⇒ 分片不必先修缓存。
- **A3 不是产品缺陷，是分叉自己选的测试策略**。`GenerateConsoleCtrlEvent` / `CTRL_C_EVENT`
  在整个 `codex-rs` 里不存在，0x03→Ctrl-C 由 OS 的 ConPTY 宿主翻译；分叉提交
  `a63c08f0f` 已在真机 Windows 11 26200.9711 量到该 build 不再翻译 0x03 且上游 CI 同红，
  随后 `7d9dc6834` **主动删掉**了按 build 号设的门。⇒ `utils/pty` 全 crate 对
  `origin/main` 的 diff 只有一处句柄类型转换 + 一行注释；这一红只能是测试策略问题。
- **A4 的定性要往上调**（安全相关）。`legacy_workspace_write_delete_is_limited_to_writable_roots`
  （`windows-sandbox-rs/src/unified_exec/tests.rs:742-849`）左值尾巴 `None, false`
  ⇒ 不在任何可写根内的文件被删掉、受保护的 `.git` 也没了，`exit_code 0`，0.089s；
  生产路径上 `legacy.rs`/`setup.rs`/`acl.rs`/`desktop.rs`/`cap.rs`/
  `resolved_permissions.rs`/`allow.rs`/`protocol/src/permissions.rs`
  对 `origin/main` 的 diff **全为空**，测试本身是上游 `#31138` 写的；
  台账 `:2088` 记录同一形状在**本机 Windows 11 桌面**单测隔离 2/3 复现。
  ⇒ "只是 runner 环境"这一说被这条反证打掉；缺的那一手是失败时的 `sandbox_log()`
  与 `apply_legacy_session_acl_rules` 是否返回成功。
  **⚠ 这一条的定性已被 §6.32 撤回**：取证落地后它既没在真机复现（10/10 绿）、也没在
  托管 runner 复现（同一 job 里 PASS 1.872s），而 `None, false` 那个读数是**我的取证代码**
  只读 allow ACE、不减 deny 造成的过度报告，不是产品放行删除。原文保留，因为它记录了
  当时为什么值得升级，结论以 §6.32 为准。

### A1：native 传输的端到端一手，从 app-server 侧扩到 `codex exec`

判据是"账本文件"这一唯一可观测量：`CODEX_RIG_REQUEST_CAPTURE_FILE` 只由桥的传输在
每次 HTTP attempt 后写一行（`codex-rust-rig-bridge/src/request_capture.rs:176-178`
`to_vec` + `\n`、`:242-251` create+append 0600），原生路径根本不构造 recorder
（`:45-68` 只在桥侧调用）。此前只有 app-server 一条腿用它做"文件不存在"的断言，
`codex exec` 侧 8+ 条 nuwax 腿一条都不看账本 ⇒ 那侧的 "Responses 形状" 无法区分
两种传输。

新两腿共享除 `NUWAX_BRIDGE` 以外的一切，因此"缺失"才成为收据而非证据缺席：

| 来源 | selection | 读数 |
| --- | --- | --- |
| mac 本机 | `cargo test -p codex-exec --test all nuwax_env::` | `10 passed; 0 failed; 124 filtered out`，3.62s；新两腿 `nuwax_env_default_bridge_records_the_responses_attempt`（账本存在且首行 `url` 落在 `/v1/responses`）与 `nuwax_env_native_bridge_sends_the_responses_wire_without_the_rig_capture`（账本不存在）均 PASS |
| mac 本机负向泄漏检查 | 同一 selection，父进程导出 `NUWAX_BRIDGE=native`、`NUWAX_MODEL=leaked-model`、`NUWAX_BASE_URL=https://leaked.invalid/v1` | 仍 `10 passed; 0 failed`，3.19s ⇒ 九变量清空有效 |
| `.41`（分支 `qoder-verify-20261021b` @ `6c176ec`） | L10c `-p codex-exec --features codex-core/rust-rig -E 'test(nuwax)'` | `Summary [12.881s] 22 tests run: 22 passed, 162 skipped`；新两腿 `PASS 0.829s` / `PASS 0.831s`（非个位毫秒 ⇒ 真实执行） |

顺带补的真实遗漏：`nuwax_env.rs` 原先只 `env_remove` 五个变量（`run_nuwax_env_turn`）
和六个（重试腿），漏 `NUWAX_BRIDGE` 等 ⇒ 任何导出过该变量的 shell 都会把这三条 wire 腿
打成红或打成另一种传输。现统一成 `NUWAX_ENV_GROUP: [&str; 9]`。
`cargo fmt --all --check` rc=0，`cargo clippy --offline -p codex-exec --test all` 干净
（4m07s Finished）。

### A2：八条 bwrap guard 腿的跳过变成可 grep 的工件

根因确认：nextest 默认不打印通过用例的输出（`codex-rs/.config/nextest.toml` 未设
`success-output` ⇒ `never`），所以 guard 里那句 `eprintln!("skipping …")` **从来没进过
runner 日志**；此前只能靠"个位毫秒即返回"猜。新机制：
`CODEX_TEST_SKIP_RECEIPT_FILE` 每跳过一次追加一行
`{"test":…,"guard":…,"reason":…}`（`exec-server/tests/common/mod.rs:45-78`），
变量已设却写不进 = **硬失败**（静默缺收据正是这里要消除的失效模式）。
ubuntu/mac 腿在 `fork-cargo-pr.yml:145-173` 里先 `rm -f` 再跑、随后一步 `if: always()`
把收据文件 `cat` 进 job 日志 ⇒ 证据落在与历手同一个 `.raw` 里，不需要新增工件下载通道。
Windows 腿未接：`system_bwrap_warning` 在非 Linux 直接返回 `None`
（`sandboxing/src/lib.rs:67-73`），今天不可能有收据；四条 MXC guard 与
`codex-linux-sandbox` 的 32 条同类 guard 未接，等 §7 的范围拍板。

| 来源 | selection | 读数 |
| --- | --- | --- |
| mac 本机 | `-p codex-exec-server --test exec_process skip_receipt` | `1 passed; 0 failed`（逐字节比对两行 JSONL；写失败即 `read_to_string` 报错，不可能静默绿） |
| mac 本机回归 | 同 target `-E shell_snapshot` | `44 passed; 0 failed; 41 filtered out`，45.75s ⇒ 八处 guard 改写无回归 |
| `.41` L10a | `-p codex-exec-server -E 'binary(exec_process)'`，收据变量指向 `$BASE/skip-receipts-l10.jsonl` | rc=0，`Starting 60 tests across 1 binary (11 tests … skipped)`；**收据 12 行 / 8 个不同腿名**（`shell_snapshot_v2_capture_failure…` ×3、`shell_snapshot_concurrent_replays…` ×3、其余六条各 1），reason 全部为 `USER_NAMESPACE_WARNING` 那句 |
| `.41` L10b 负向对照 | 同 target 一条 guard 腿，收据路径指向不存在的目录 | `FAIL [0.066s]`，`panicked at exec-server/tests/common/mod.rs:74:31: cannot open "/nonexistent-dir/r.jsonl" for the skip receipt of remote_process_preserves_empty_workspace_roots: No such file or directory`，rc=100 ⇒ 写不进时**大声红**，不再静默绿 |

`.41` 一手把 §6.27 的"跳过式绿只能靠人工数毫秒"这一状态结束：同一批腿现在自己报名字和
宿主原因。它同时给出一条**新的、可核对的**分母——`exec_process` 这个 binary 在
无 userns 能力的宿主上有 12 个实例根本不执行。

**边界**：`ubuntu-latest` 自己在 `fork-cargo-pr.yml:98-107` 尝试把
`kernel.apparmor_restrict_unprivileged_userns` 置 0，所以下一手的 ubuntu 日志里
"零收据"与"若干收据"都是合法结果；这一改动买到的是**能区分**这两种结果，
而不是其中某一种结论。

### A 档剩余四项：等用户拍的范围（未动代码）

1. **A2 的覆盖面**。机制 + 8 条 exec-server guard 已落地；同类静默跳过还有
   `codex-linux-sandbox` 的 **32 条**（`should_skip_bwrap_tests`，
   `tests/suite/sandbox.rs:229-250`）、`codex-exec` 的 4 条 Landlock 探针
   （`tests/suite/sandbox.rs:104-121` + `None => return` 四处）、Windows MXC 4 条
   （`skip_if_mxc_unavailable`，含 `file_system_windows.rs:407` 的自带写法）。
   接到 linux-sandbox 需要给那个 crate 加一份共享入口（它的 tests 不引
   `exec-server/tests/common`），是一次跨 crate 的机械改动。
2. **A3 的立场**：`conpty_ctrl_c_interrupts_powershell_foreground_child` 要不要恢复
   `a63c08f0f` 当初加、`7d9dc6834` 删掉的按 build 门（推荐，理由见本节第 2 条推翻项），
   还是继续红着等 Windows 宿主。**这是分叉测试立场的变更，不自作主张。**
   → 已拍"恢复门"，落地与两手证据见 §6.31。
3. **A4 的处置**：Windows write-delete 那条按上表定性为 fail-open 且真机可复现，
   第一步只能是"让失败带上 `sandbox_log()` 与 ACL 落地证据"（纯测试侧、Windows-only，
   mac 无法编译验证，只能靠 `.235` 恢复后一手）；分叉内修生产 ACL 不在本轮范围。
   → 取证已落地（§6.32），但定性与"真机可复现"这两点已被 §6.32 推翻，生产 ACL 无需动。
4. **A5/A6**：rusty_v8 的 off-CI 配方卡在**下发体积仓内无记录**
   （`.github/actions/setup-rusty-v8/action.yml:28-39` 只从
   `github.com/openai/codex/releases` 取，无 vendored 副本），且已确认
   `cargo test -p codex-exec-server` 一类定向测试**根本不建 v8**
   （v8 只经 `codex-code-mode-runtime`→`code-mode-host` 与 `v8-poc` 进来），
   所以它的唯一收益是本机建 code-mode-host；CI 分片的阻塞前提已被推翻，
   剩下要拍的是目标是"消掉 mac cancelled"还是"整腿提速"（后者要动三条腿并重建对比基线）。

### 6.31 A3 落地：按 build 号恢复 ConPTY Ctrl-C 那道门，并在真机上取到"首败→修复后"两手

用户拍板选"恢复按 build 号设的门"（`a63c08f0f` 加、`7d9dc6834` 删的那道）。改动三处，全在未提交的
工作树里：`utils/pty/src/windows_tests.rs:460-462` 恢复 `CONPTY_CTRL_C_INPUT_BROKEN_SINCE = 26_100`
与 `affected_build`，等待失败分支从裸 `?` 改成 `match`——build 低于门槛才 `anyhow::ensure!`（`:493`）
直接判红，达到门槛则改走退化契约（`TTL=` 仍在动 → `session.terminate()` → 断言 `ver` 没被打出来）；
配套把 `win/mod.rs:47` 恢复成 `pub(crate) mod psuedocon;`、`win/psuedocon.rs:104` 恢复成
`pub(crate) fn windows_build_number()`（`7d9dc6834` 当年把可见性一起收回去了，门要读它就得起回来）。

**首败一手（个人 Windows `192.168.32.235`，`cmd.exe /C ver` = `10.0.26200.9711`，跑的是未门控代码）**：
`win::psuedocon::tests::windows_build_number_returns_value` PASS 0.020s（`RtlGetVersion` 在这台读得到值，
门才有得判）；ctrl-c 腿连跑 3 轮、nextest 自带 2 次尝试共 **6 次全 FAIL**，`rc_ctrlc_1/2/3` 都是 100，
时长 11.819/11.003、11.051/11.080、13.046/12.583s；每次 stderr 逐字同形——
`Error: timed out waiting for "Microsoft Windows" in PTY output:` 之后正好 **10 条**
`来自 127.0.0.1 的回复: 字节=32 时间<1ms TTL=128`，即 0x03 写进 pseudoconsole 输入管道后 ping 没断、
`cmd /C ver` 根本没跑。整腿 `Summary [22.699s] 27 tests run: 26 passed, 1 failed, 0 skipped`
（`rc_pty_all=100`），`rc_clippy=0`。

**托管 runner 是同形状，不是我这台的个别现象**：run `39396134392` 的 windows 腿
`FAIL [11.447s] (21382/21760)`，stderr 同样是 10 条 `Reply from 127.0.0.1: bytes=32 time<1ms TTL=128`、
同样没有 `ver`（缓存 `w39396.txt:25882`、`:25896`）。runner 的 build 号日志里没有，所以这里只主张
"失败形状逐字一致"，**不**主张"实测同一 build"。

**修复后一手（同一台、同一 build）**：ctrl-c 腿 PASS ×3，rc=0，时长 12.827 / 12.259 / 12.011s。这个 12s
量级本身就是证据：它等于"等满 10s 超时 → terminate → 收尾"，说明门没有把腿变成静默跳过，而是在执行
"中断只能靠 terminate"这条替代契约。整腿 `Summary [12.441s] 27 tests run: 27 passed, 0 skipped`，
`rc_fix2_pty_all=0`、`rc_fix2_clippy=0`。

**这一腿的过程里踩到两个记录坑，都记下来（它们决定收据可不可信）**：
(1) 前两把误用 `git archive HEAD` 送码，`.235` 上跑的是**已提交**的旧代码——它因此变成"未门控首败"的一手，
但不是修复后证据；验在途改动必须 `tar` 工作树。
(2) `tar -xzf` 保留源文件旧 mtime，早于 `target/` 产物时 cargo 判 fresh、静默跑旧二进制；
必须 `find <crate> -name '*.rs' -exec touch {} +`，并在脚本里 grep 改动标记确认源码到位。
另外 `cargo nextest --no-run` 第一把 `rc=101 / 0xc0000142 (STATUS_DLL_INIT_FAILED)`，第二把才起来——
这台的已知瞬时故障，不是代码问题。

**门买到什么、买不到什么（不粉饰）**：买到的是 CI windows 腿不再被一个已知 OS 回归反复判红，且真红的时候
断言的是实测行为；**买不到的是正向覆盖**——`.235`（26200.9711）与托管 runner 都落在受影响区间，
所以"0x03 能打断前台子进程"这条契约目前在任何一台跑得动的机器上都没有一手。登记为已知缺口，不当作已解决。

## 6.32 A4 撤回"fail-open"定性：取证落地后，Windows write-delete 那条在真机与托管 runner 上都判"保护模型正确"

用户拍的是"先加诊断取证"。落地范围严格留在测试侧（`windows-sandbox-rs/src/unified_exec/tests.rs`，
未提交），生产 ACL 一行没动。加了四件东西：

- `session_delete_masks(path)`：分别取"文件自身"与"父目录"两把权限，按
  `wanted = DELETE | FILE_DELETE_CHILD` 计算 **allow 减 deny** 的有效位
  （deny ACE 无论 ACE 顺序位置都优先，这是 Windows ACL 的既有语义）；
  `mask_names` 把有效位渲染成可读串，`delete_right_verdict` 给出"能不能删"的判决。
- `write_delete_acl_evidence`：**每次运行**（不只失败时）打印 before/after 两组逐路径读数。
  这是关键改动——原取证只在 panic 路径上打印，于是"红的时候才看得见"，
  而绿的时候无法证明"绿是因为保护到位"而不是"绿是因为根本没执行"。
- `PROTECTED_GIT_CHILD`（`.git\config`）：`.git` 目录本身被保护不等于里面的文件被保护，
  原来只测父目录，这一条把子文件也纳入删除探针。
- 断言收口成 7 元组（三个可写根内文件 + outside 文件 + `.git` + `.git\config` + 退出码）。

**测试契约本身没被改动过**：`git diff origin/main -- <该文件>` 里没有任何被删除的断言行，
唯一命中 `assert` 关键词的改动是 deadline 注释（外层 90s 上限、内层 60s 断言，
目的是让慢腿由断言判红而不是 nextest TIMEOUT）。所以下面这些绿不是"我把断言改松了"换来的。

| 来源 | selection | 读数 |
| --- | --- | --- |
| `.235`（Windows 11 `10.0.26200.9711`，A6） | `cargo nextest run -p codex-windows-sandbox --lib -E 'test(legacy_workspace_write_delete_is_limited_to_writable_roots)'` ×3 | 三轮全 `PASS`，0.330 / 0.311 / 0.311s（非个位毫秒 ⇒ 真执行） |
| `.235`（A7，间歇性判定） | 同一 selection 连跑 **10 次隔离迭代** | `fails=0 of 10`，时长 0.302–0.323s，每次 rc=0 |
| 托管 runner | run `39526586631` windows 腿（head `04623ca2c`） | `PASS [   1.872s] (21765/21766)`，同 job 总计 21766 run / 8 failed（名单见 §6.34），这条不在其中 |

A6 那三手的逐路径证据（after session，`.235` 上一次真实运行的原文）：

```
workspace\workspace-delete.txt: here=[absent]  in parent=[DELETE]
temp\temp-delete.txt:           here=[absent]  in parent=[DELETE]
tmp\tmp-delete.txt:             here=[absent]  in parent=[DELETE]
outside\outside-delete.txt:     here=[none]    in parent=[none]
workspace\.git:                 here=[none(DELETE granted, DELETE+FILE_CHILD denied)] in parent=[DELETE]
workspace\.git\config:          here=[none(DELETE granted, DELETE+FILE_CHILD denied)] in parent=[none(...denied)]
```

before session 六行全部 `here=[none] in parent=[none]`。读法：可写根内的三个文件被删掉了
（`absent` = 策略本来就允许），**不在任何可写根内的 `outside` 文件既没被删也没有删除权**，
`.git` 与 `.git\config` 都存活且有效位被 deny 明确收回。⇒ 与"左值尾巴 `None, false`
= 保护失效放行删除"的解读正好相反。

**这条红当初为什么会被读成安全问题**：我第一版取证用 `path_mask_allows` 只查 allow ACE、
不减 deny，于是"被授予又被沙箱自己的 deny 收回"的权限被打印成"允许删除"。按 allow-minus-deny
重算后判决翻转。**定性：harness 缺陷（取证代码过度报告），不是产品缺陷。** §6.30 那条
"A4 的定性要往上调（安全相关）"已在原处标注撤回。
**§6.39 的重判**：托管 runner 上拿到了带完整 Diff 的一手——`outside` 文件、`workspace\.git`、
`.git\config` 三者 session 后全部消失（`None, false, false`），而本地 `.235` 同证据显示三者存活。
所以"不是产品缺陷"这个定性**在 runner 这一侧不再成立**，改为"越界删除已证实、机制未证"；
本节的本地读数与"allow-minus-deny 才看得见 deny"这条方法论仍然有效，不动。

**不粉饰的部分**：加了取证之后这条腿**一次都没再红过**，所以"当年那次 `None, false` 失败时的
ACL 落地状态"这一手依然缺失，间歇性本身仍未解释（台账 `:2088` 记的"本机 2/3 复现"与
`.235` 现在 13 次连续绿无法同时成立，两组读数的差异我只能归给未测的宿主时序，不作主张）。
现在能主张的是两件事：(a) 保护模型在真机与托管 runner 上都有**直接的存活证据**，
不再依赖"它绿了所以大概没问题"；(b) 若它再次变红，同一份日志能立刻区分
"能力 ACE 从未落地"（`here=[none]` 且父目录也没 DELETE）与"落地过宽"
（出现 `granted` 而无 `denied` 尾巴）。取证成本 0.3s 量级，判定收益不对称，保留。

## 6.33 个人 Linux `.45` 上的第四项确认缺陷：`/dev/*` 的 deny mask 让 bwrap 在命令启动前 abort

`.45`（**Ubuntu 26.04.1 LTS**、`uname -r` = `7.0.0-38-generic`、`bubblewrap 0.11.1`、
`kernel.apparmor_restrict_unprivileged_userns` = **1**、16 核、stable toolchain）是本轮第一台
bwrap **真跑得起来**的 Linux 机。注意这个 sysctl 读数是 1，却并没有像 `.41` 那样阻断 userns
（§6.27 那台是 1 且 8 条 guard 腿全部跳过）；差异的机制我没测，只登记读数与结果，
不编解释。这条主机事实直接决定了此前所有 bwrap 相关结论的证据等级：`.41`/runner 上
"绿"的八条腿里有多少是真执行，第一次有了可核对的分母。

**#36 一手（修复前基线）**：`-p codex-exec-server --test exec_process` =
`Summary [12.171s] 60 tests run: 56 passed, 4 failed, 11 skipped`；
`-p codex-linux-sandbox` = `Summary [6.269s] 228 tests run: 228 passed, 3 skipped`；
跳过收据文件**未生成** ⇒ 八条 guard 腿在这台全部真执行（这正是 §6.30 A2 机制要读的东西）。

四条红分桶（每桶都要有一手，不接受"大概环境问题"）：

1. **3 条 = 宿主环境噪声**（`shell_snapshot_v2_capture_failure_falls_back_and_retries` 的
   `remote_pipe_recovery` / `remote_tty_recovery` / `remote_retry_budget_exhausted`）。
   strace 一手：`execve("/usr/bin/systemd-cat", ["-p","6","-t","im-config", …])`，
   由沙箱内 login shell 经 `/etc/profile.d` 的 run-parts 触发；`Failed to create stream fd:
   Operation not permitted` 这句在整个 `codex-rs` 里只有 `systemd-cat` 持有。
   seccomp + `--unshare-net` 让它的 `socket()` 拿 EPERM ⇒ 快照捕获阶段的非零退出。
   产品的 fallback 契约本身仍然成立（`exec_process.rs:709` 断言的
   `("original", "", Some(0), true)` 未变），所以这不是分叉缺陷，是 Ubuntu 桌面包
   往 profile.d 里塞了个在沙箱里必挂的探针。
2. **1 条 = 确认的产品缺陷**（`shell_snapshot_concurrent_replays_keep_independent_readers::blocked_descriptor_path`）。
   `.45` 上复现的失败与托管 runner 逐字同形（stderr 见 §6.34），strace 抓到的 argv 里
   这段是根因：`--dev /dev` … `--perms 000 --tmpfs /dev/fd --remount-ro /dev/fd`。

机制（读代码 + vendored 源，不是猜）：`--dev /dev` 在**所有**策略分支都是基线参数
（`linux-sandbox/src/bwrap.rs:295/535/544/575/676`），而 `vendor/bubblewrap/bubblewrap.c:1387-1412`
的 `SETUP_MOUNT_DEV` 只按固定节点表 `{null, zero, full, random, urandom, tty}` 建文件、
再加 `stdin/stdout/stderr` 三个符号链接。⇒ 宿主 `/dev` 下的其它条目（`fd`、`pts`、…）
在沙箱根里**压根不存在**，于是对 `Deny /dev/fd` 这条策略发出的 `--tmpfs /dev/fd` 找不到挂载点，
bwrap 直接 abort，**被沙箱包住的命令一次都没启动**。这是 fail-closed 的**功能失效**
（不是安全放行），后果是任何带 `/dev/*` deny 的策略在该宿主上都不能用；机制与宿主无关，
这解释了为什么同一腿在 ubuntu runner 上是"亚秒 + 空输出"这个形状。

**第一版方案错了，并且是被自己的实验证伪的**：我先做的是"补一个挂载点"——
`--perms 000 --dir /dev/fd` 再接 mask。单测全绿，leg 仍红；直接手跑 bwrap 两组对照
（E1 只 `--dir` → rc=0；E2 `--dir` + `--tmpfs` → 同一句 `Can't mount tmpfs on /newroot/dev/fd:
No such file or directory`）说明 `--dir`（`SETUP_MAKE_DIR`，client 侧 `ensure_dir`）与
tmpfs mount（privileged_op，server 侧）不在同一作用域，救不了 `/dev/*` 的 mask。
那版代码（`needs_synthetic_dev_mount_target` 及其两条测试）已整体删除，不留死代码。

**落地方案**：缺席本身就是拒绝。`append_unreadable_root_args` 在 symlink fail-closed bail
之后、`!exists()` 分支之前，跳过"合成 `/dev` 本来就不含"的目标；
`SYNTHETIC_DEV_ENTRIES: [&str; 10]`（6 个节点 + 3 个符号链接 + `--dev` 之外另行 bind 的 `shm`）
逐项对齐 vendored 那张表，`/dev` 自身与表内节点保持可 mask。三条新腿：

**⚠ 本节下面这套"落地方案 / 修复后一手 / syscall 补强"记的是 `a4c0ff2fa`，它的名单漏了
bwrap 自己创建的 5 个条目，因而对这些路径构成 fail-open；纠正与推翻它的一手见 §6.35。**
下面这张表与"leg 转绿"的读数当时确实成立，但它们现在是被作废的状态，不要按现状引用。

| 腿 | 测的是哪一面 | `.45` 读数 |
| --- | --- | --- |
| `only_dev_paths_outside_the_synthetic_set_count_as_absent` | 纯谓词，不碰宿主：`/dev/fd`、`/dev/pts/1` → true；`/dev`、`/dev/null`、`/dev/shm`、`/devfoo`、`/tmp/dev` → false | PASS 0.014s |
| `denied_dev_path_without_a_mount_point_is_denied_by_absence` | 真 argv：既不发 `--tmpfs/--dir/--remount-ro /dev/fd`，也不发 `--ro-bind-data … /dev/fd` | PASS 0.018s |
| `denied_synthetic_dev_node_keeps_its_mask` | 反向不退让：`/dev/null` 仍走 `--ro-bind-data`，掩蔽文件数 = 1 | PASS 0.016s |

第二条的前置条件写成响亮 `assert!(denied.is_dir(), "precondition: …")`，
不是 `if !denied.is_dir() { return; }`——本轮刚花了一整个 A2 把"缺前置即静默绿"变成可 grep 的
收据，测试自己不能又犯同一个失效模式；宿主 `/dev/fd` 若不存在就该大声红。
谓词那条是宿主无关的，所以 mac 本机也能给它一手。

**修复后一手（同一台；登记的是收尾后的最后一把，源码 md5 `c2283864…` 与 mac 工作树逐字节相同、
`deny_is_enforced_by_absent_dev_path` 标记 grep=9 处到位）**：
`-p codex-linux-sandbox` = `Summary [6.157s] 231 tests run: 231 passed, 3 skipped`
（228 + 3 条新腿，分母对得上）；
`blocked_descriptor_path` = `PASS [   0.909s] (1/1) … 1 test run: 1 passed, 70 skipped`
——0.909s 而不是个位毫秒，配合"未写跳过收据"就是真执行；
整族回归 `-p codex-exec-server --test exec_process` = `Summary [12.117s] 60 tests run: 57 passed, 3 failed, 11 skipped`，
剩下的 3 条正是上面第 1 桶那 3 条 im-config/systemd-cat 噪声红，**一条没多**；
`rc_clippy=0`；`cargo fmt --all --check`（在 `codex-rs` 工作区根）rc=0。

**syscall 一层的补强**（不停在单测断言上）：同一台用 `strace -f -e trace=execve` 跑这一腿，
`rc_leg=0`、`PASS [1.065s]`，抓到的 **28 次** `execve("/usr/bin/bwrap", …)` 里
**0 次**带 `"/dev/fd"` 这个参数（前一手同位置的 argv 里是
`--perms 000 --dir /dev/fd --perms 000 --tmpfs /dev/fd --remount-ro /dev/fd`），
而 `--dev /dev` 基线仍在原位。⇒ "不再对缺席路径下 mask" 这一条不是只对 fake argv 成立，
真实子进程就是这样起来的。策略里的 `{"path":"/dev/fd","access":"deny"}` 条目本身保留
（沙箱内 `/dev/fd` 依然不可读，`</dev/fd/18` 走的是失败分支），改的只是"用什么方式拒绝"。

## 6.34 下一批的两手 CI 收据：`39526586631`（head `04623ca2c`）与 `39530269408`（head `ecb4ba6bb`）

两手里 **ubuntu 腿都已到终态**，mac/windows 仍有 job 在跑。**两手都不含本节要登记的
bwrap 修复与 A3 门**——那些改动还只在未提交工作树里，所以这两手读的是"修复前"的行为，
不能拿来证明修复有效（有效性一手在 §6.33）。

取日志的通道这次必须换：run 未终态时 `gh run view --job … --log` 直接拒绝
（`run 39526586631 is still in progress; logs will be available when it is complete`），
但**已完成 job 的日志可以单独取**：
`gh api --allow-escape-sequences repos/nuwax-ai/codex/actions/jobs/<jobId>/logs`
（不带 `--allow-escape-sequences` 时 gh 会以"响应含终端转义序列"为由拒绝输出）。
清洗仍用 `perl -pe 's/\e\[[0-9;]*m//g; s/\^\[\[[0-9;]*m//g'`，两种形态都吃掉。

| job | 读数 |
| --- | --- |
| `39526586631` / `nextest (windows)` (118982561275) | `Summary [2691.695s] 21766 tests run: 21758 passed (20 slow, 322 leaky), 8 failed, 138 skipped` |
| `39526586631` / `nextest (ubuntu-latest)` (118982561606) | `Summary [2847.851s] 22935 tests run: 22934 passed (21 slow), 1 failed, 48 skipped` |
| `39530269408` / `nextest (ubuntu-latest)` (118994304753) | `Summary [2892.564s] 22938 tests run: 22937 passed (22 slow), 1 failed, 48 skipped` |
| `39526586631` / `bazel bridge gate (ubuntu)` | success（两手的 bridge gate 都 success） |

**ubuntu 那唯一一条红，两手里都是同一条腿、两种表面形状**：

- hand 1 `FAIL [   0.336s]`，`panicked at exec-server/tests/exec_process/shell_snapshot.rs:348:13`，
  `bash#0: output="" stderr="bwrap: Can't mount tmpfs on /newroot/dev/fd: No such file or directory\n"
  status=Some(1) closed=true`
  ⇒ §6.33 的机制**第一次拿到托管 runner 一手**，逐字与 `.45` 复现同形。此前这个根因只有
  我自己那台机器和代码推断两个来源。
- hand 2 `FAIL [  25.316s]`，`Error: exec-server transport disconnected: failed to read
  websocket JSON-RPC message from exec-server websocket ws://127.0.0.1:42967: WebSocket protocol
  error: Connection reset without closing handshake; failed to resume exec-server session:
  no resume attempt finished within 25s (last attempt: … Connection refused (os error 111))`
  ⇒ **这条形状我没有解释**。同一腿、同一策略、无 `bwrap:` stderr，最合理的一致读法是
  那次 abort 把 exec-server 连接一起带崩、然后按 25s resume 预算耗尽收场，但这只是
  候选解释。诚实登记：修复直接针对的是 hand-1 形状；含修复的下一手 runner 上若 25.316s
  这一形状仍在，就必须单独再查，不能算已被 §6.33 覆盖。
  **后续读数（§6.37）**：这个形状已在 `.45` 上用"按时序杀掉 exec-server"逐字复现，
  它是**服务进程在 session 建立后死亡**的客户端 25s 预算签名；本节那句候选解释
  （"abort 把连接一起带崩"）被同一台机器 8/8 次的 A 形状推翻。ubuntu 这条腿因此在
  runner 上不产出 `/dev/fd` 证据，#42 的验收一手只能来自 `.45`。

**windows 的 8 条红，逐条与已有定性的对应**：

| 腿 | 时长 | 归属 |
| --- | --- | --- |
| `codex-uds lib_tests::socket_validation_rejects_broad_acl_without_repairing_it` | 0.540s | §6.x 既有的 uds runner-image 家族（#18 已处置，未新增主张） |
| `codex-uds lib_tests::private_directory_acl_is_user_only_and_inherited_by_files` | 0.908s | 同上 |
| `codex-tui app::tests::session_lifecycle_requests::dynamic_tool_requests_ignore_other_namespaces_and_dispatch_tui_namespace` | 2.068s | 本手首次观察，尚未分桶；**§6.40 收到第二次观察 `FAIL + LEAK 14.026s`，panic 行号与文案逐字相同 ⇒ 不是 nextest kill，归因需再加诊断** |
| `codex-utils-pty tests::windows_tests::conpty_ctrl_c_interrupts_powershell_foreground_child` | 12.701s | **A3**，`7d9dc6834` 删门之后必红；§6.31 已量到 runner 与 `.235` 逐字同形，恢复门只修这一条。**§6.39 验收：`a3b96ae90` 手 `PASS [12.071s]`，四红转一绿** |
| `codex-core::all suite::auto_review::remote_model_override_uses_catalog_model_for_strict_auto_review` | 26.531s | 本手首次观察，尚未分桶 |
| `codex-windows-sandbox unified_exec::tests::legacy_capture_cancellation_terminates_descendants_without_timeout` | 57.340s | `legacy_` 家族（#27 已重分类，deadline 与整机调度那组） |
| `…::legacy_capture_emits_output_and_preserves_descendant_after_normal_exit` | 61.222s | 同上 |
| `…::legacy_tty_job_terminates_and_preserves_descendants` | 61.237s | 同上 |

同 job 里 `legacy_workspace_write_delete_is_limited_to_writable_roots` 是 `PASS [   1.872s]`
⇒ §6.32 的托管 runner 一手。

**跳过收据机制（`6c176ecec`）在 CI 里确实工作**：hand 2 的 ubuntu job 里那个 `if: always()`
步骤打印 `no skip receipt file: either the suite never ran or no leg skipped`，
而**同一个 job 确实跑了 `exec_process`**（该腿 FAIL 就是执行证据）⇒ 八条 bwrap guard 腿
在 `ubuntu-latest` 上**零跳过、全部真执行**。这正是 §6.30 A2 那条边界要买的东西：
"零收据"与"若干收据"现在可区分了。`#39` 由此可收口。

`39526586631` 的 mac job 与 `39530269408` 的 mac/windows job 在登记时仍 `in_progress`；
按既有规矩，未到终态的 job 日志不可取，也不把"还在跑"读成任何结论。

## 6.35 §6.33 那笔修复被我的下一条测量推翻了一半：纠正、以及顺带量到的一个上游 fail-open

登记顺序不修饰：**§6.33 的修复推上去之后我自己接着测，测出它是 fail-open**。
`a4c0ff2fa` 在 `origin/test` 上存在了 **10 分 25 秒**（两次推送分别产生 run `39542266851`
@ `2026-10-21T04:07:10Z` 与 `39543230422` @ `04:17:35Z`），随后被 `a3b96ae90` 取代。

### 我那份名单漏了 bwrap 自己创建的 5 个 `/dev` 条目

vendored `bubblewrap.c:1408-1459`（`SETUP_MOUNT_DEV` 从 `:1388` 起，这是紧接我引用过的那
6 个节点之后的部分）还做：`fd -> /proc/self/fd`（`:1418`）、`core -> /proc/kcore`（`:1422`）、
`mkdir pts` + **devpts 挂载**、`ptmx -> pts/ptmx`、`mkdir shm`（`:1428` 起），
以及 stdout 是 tty 时 `console`（`:1444` 起）。
我的 `SYNTHETIC_DEV_ENTRIES` 只列了 10 项，于是 `fd`/`core`/`pts`/`ptmx`/`console`
被我判成"缺席"，**对这些路径的 Deny 被静默放行**。

一手（`.45`，直接用产品 helper，策略里带 `{"path":"/dev/fd","access":"deny"}`）：

```
lr-x------ 1 soddy soddy 64 ... 0 -> pipe:[3516382]
lr-x------ 1 soddy soddy 64 ... 1 -> pipe:[3529043]
lr-x------ 1 soddy soddy 64 ... 2 -> pipe:[3529043]
exit=0
```

对照：同一条命令在无 deny 的策略下 `ls -l /dev` 显示 `fd -> /proc/self/fd`
（还有 `core -> /proc/kcore`、`ptmx -> pts/ptmx`、`pts`、`shm`、三个 stdio 符号链接）。
⇒ **分桶：harness 之外的产品缺陷，且由我引入**——原来的行为是响亮 abort（fail-closed，
功能不可用），我把它换成了"跑起来了但拒绝没生效"，这一版更坏。已 fix-forward，不掩盖。

### 为什么 `--tmpfs /dev/fd` 挂不上：不是顺序问题

裸 bwrap 四组对照（`.45`，`bubblewrap 0.11.1`、`7.0.0-38-generic`）：

| 组 | 参数顺序 | 读数 |
| --- | --- | --- |
| X1 | `--dev /dev --proc /proc … --perms 000 --tmpfs /dev/fd`（mask 在 proc **之后**） | `rc=1`，`Can't mount tmpfs on /newroot/dev/fd: No such file or directory` |
| X2 | `--dev /dev --perms 000 --tmpfs /dev/fd --proc /proc`（产品今天的顺序） | **同一句 rc=1** |

⇒ 挂载目的要穿过符号链接解析到 `/proc/self/fd`，这条路 bwrap 两种顺序都走不通。
所以"缺席即拒绝"这个机制**只对 bwrap 从不创建的条目成立**（如 `/dev/kmsg`），
不能推广到它自己建的那些——这正是我第一版犯的推理错误。

### 纠正后（`a3b96ae90`，名单 15 项 = 6 节点 + 6 符号链接 + `pts`/`shm`/`console`）

| 检查 | 读数 |
| --- | --- |
| `-p codex-linux-sandbox` 全量 | `Summary [6.361s] 232 tests run: 232 passed, 3 skipped`（228 基线 + 4 条腿） |
| 新腿 `denied_dev_symlink_is_still_masked_rather_than_declared_absent` | PASS 0.016s——断言 `/dev/fd` **仍然**拿到 `--tmpfs`，即"缺席"不得作为放过活路径的理由 |
| 新腿 `only_dev_paths_bwrap_never_creates_count_as_absent` | PASS 0.010s——`/dev/kmsg`、`/dev/sda`、`/dev/net/tun` 判缺席；`/dev`、`/dev/fd`、`/dev/core`、`/dev/pts`、`/dev/ptmx`、`/dev/console`、`/dev/pts/1` 全部保持可 mask |
| `blocked_descriptor_path` | **回到红**：`FAIL [0.683s]` + 逐字 `bwrap: Can't mount tmpfs on /newroot/dev/fd: No such file or directory` ⇒ 缺陷仍开放，但响亮；这条红**不是**新回归，是恢复已知状态 |
| 保留那一半的执行一手（`Deny /dev/kmsg`） | `rc_p4=0`、`NOT_READABLE`、`ls: cannot access '/dev/kmsg': No such file or directory` ⇒ 沙箱起得来**且**拒绝成立，不再只有 argv 证据 |
| `cargo clippy --all-targets` / `cargo fmt --all --check` | rc=0 / rc=0；跳过收据未写 |

### 顺带量到的产品 fail-open：`Deny` 落在 `/proc` 下今天是空操作（与我这笔改动无关）

机制与 F-A 证据（同一台）：

- 真实 argv：`--perms 000 --ro-bind-data 3 /proc/version` 出现在 `--proc /proc` **之前**
  （strace 一手，`--proc", "/proc` 紧跟其后）。
- 端到端走产品 helper，策略 `{"path":"/proc/version","access":"deny"}`：
  P1 `READABLE`；P2 `ls -l /proc/version` → `-r--r--r-- 1 nobody nogroup 0`
  ⇒ 看到的是新 procfs 里的真文件，mask 被过 mount 盖掉。控制 P0（无 deny）同读数。
- 裸 bwrap 顺序对照：X4（mask 在 `--proc` 前）`bytes=211`；X3（mask 在后）
  `cannot open /proc/version: Permission denied`、`bytes=unreadable`；X5 控制 `bytes=211`
  ⇒ **顺序就是因果**：procfs 挂载晚于 mask，mask 必然被吞。
- 归属核实：`git diff origin/main..a4c0ff2fa~1 -- codex-rs/linux-sandbox/src/bwrap.rs` **为空**
  （整个 crate 只有 `tests/suite/root_metadata_tests.rs` 与 main 不同）
  ⇒ 这是上游既有行为，不是分叉引入；影响面是任何落在 `/proc` 下的 deny 规则
  （用户可在策略里表达，`--proc` 在所有非禁用分支都是基线）。
- 这条我**没有**在本轮动代码：改它要动全局挂载参数顺序，不是测试侧改动，见下面选项。

### 待拍板的两个选项（量化后等指针，不自作主张）

| 选项 | 买到 | 代价与风险 |
| --- | --- | --- |
| **O1** 把 `--proc /proc` 移到 deny mask 组装之后 | `/proc/*` 的 Deny 真正生效（X3 一手已证可行）；顺带让 `/dev` 下穿过 `/proc` 的 mask 至少有一次可判的失败位置 | 全局挂载顺序变动，触及 WSL 腿的**位置断言**：`bwrap_wslg_tests.rs:81/257`、`linux_run_main_tests.rs:227/317` 直接断言 `--proc`/`--tmpfs /proc` 相邻与存在；这些断言要同步改，且执行一手只有 `.45` 能给（`.41` 无 userns）。**§6.43 已实现并按此登记一手**；但那两行"触及位置断言"经复核**不成立**（`bwrap_wslg_tests.rs:81` 与 `linux_run_main_tests.rs:226-228/316-317` 都是 `windows(2)` 形状断言，搬 `--proc` 不会让它们变红；257 那行其实是 `["--tmpfs","/proc"]`），真正阻断前移的是 `/newroot/proc` 这个目的地在根绑定之前还不存在。**判据以 §6.43 为准，本行原样保留不改写** |
| **O2** 再给 `/dev` 符号链接条目一个可用的 mask 原语 | `Deny /dev/fd`、`/dev/core` 既生效又不 abort，`blocked_descriptor_path` 那条腿才可能转绿 | 需要 bwrap 侧能力（`--symlink` 改写或等价），`launcher.rs:206-218` 的能力探测要加一项并按宿主降级；至少三条新腿 + 真机一手 |
| O3 都不动 | 零风险 | F-A 继续静默放行 `/proc` deny，F-B 继续响亮 abort，`blocked_descriptor_path` 长期红 |

推荐 **O1 先做**（安全侧、单点、有对照一手），**O2 单独拍**（它是新原语，不是一次顺序调整）。

## 6.36 第一手的 mac 腿到终态：mac 那两条红是 60s 边界，不是产品回归（提交 `42c5bb5d9`）

`39526586631`（head `04623ca2c`）四腿全部终态，run `failure`：

| job | 终态 | 读数 |
| --- | --- | --- |
| `nextest (macos-latest)` (118982561683) | failure | `Summary [3648.129s] 22885 tests run: 22883 passed (27 slow), 2 timed out, 71 skipped` |
| 其余三腿 | 已登记 §6.34 | ubuntu 1 红、windows 8 红、bridge gate success |

mac 的两条红逐字是 `TIMEOUT [  60.012s] ( 4736/22885) codex-cli::app_server_daemon
managed_daemon_restarts_from_deleted_working_directory` 与
`TIMEOUT [  60.012s] ( 4770/22885) …::managed_identity_survives_locale_and_timezone_changes`，
两条都先 `SLOW [> 30.000s]` 再 `TERMINATING [> 60.000s]`，stdout 只有
`running 1 test` / `(test timed out)`——**没有任何诊断**。除此之外 mac 腿零红。

### 先记一个测量教训：日志尾部静止 30 分钟不等于腿挂了

04:24 我读这腿时，日志尾停在 `7585/22885`（03:59:41），而步骤仍 `in_progress`，
形态酷似挂死。它 04:35:44 正常收口，水位一次性从 7585 跳到 22883 ⇒ 是托管 runner 的
日志分块上传滞后。**第二个独立一手同形互证**：`39530269408` 的 mac 腿在 04:31:54
也把尾停在 `7586/22888`。按既有规矩，"日志没动"不读成结论，这条现在有了两次一手支撑。

顺带一个覆盖交叉核对：两手的 mac 腿测试总数 `22885` vs `22888` 差 **3**，正等于
`bb2c0f9eb` 加进 `exec/tests/suite/nuwax_env.rs` 的 2 条 `#[tokio::test]` 加上
`6c176ecec` 加进 `exec-server/tests/exec_process.rs` 的 1 条（两笔各自删掉的 test 属性
实测为 0）⇒ 这两笔真到了 mac 腿，不是只到了 ubuntu。

### 这两条腿是四手连涨后越过默认 60s，不是本轮改动引入

同一台 mac 腿上同两条腿的历史读数（缓存日志逐字核过名与时长配对）：

| 手 | `managed_daemon_restarts_…deleted_working_directory` | `managed_identity_survives_locale_…` |
| --- | --- | --- |
| `39347345560` | PASS 46.008s (4733) | PASS 35.630s (4767) |
| `39371971338` | PASS 51.222s (4733) | PASS 54.032s (5240) |
| `39396134392` | PASS 59.604s (4733) | PASS 56.490s (4801) |
| `39526586631` | **TIMEOUT 60.012s** | **TIMEOUT 60.012s** |

- 归属证据：这两条腿**不在** `nextest.toml` 任何 override 里，走的是
  `[profile.default] slow-timeout = 30s × terminate-after 2`（= 60s），59.604s 那次
  距 kill 只差 0.396s。同一手该 job 从 01:19:06 起跑，第一条测试出现在 03:34:56
  ⇒ 编译 2h16m，而 `39233543173` 同一棵树是 1h32m47s（workflow 注释里的事实）；
  同手该腿还有 27 条 `SLOW`。**分桶：harness 预算 vs 托管机争用**，与 #19/#26/#27/#32
  同族；不是"协议不适用"也不是产品缺陷。
- 为什么这条红特别贵：CI 的调用是 `just test --workspace … --retries 0`
  （`fork-cargo-pr.yml`），**被 kill 的腿一次机会都没有**；本机用同一 profile 不加
  该 flag 时，同两条腿在 1s 窗口下确实打出 `RETRY 1`/`RETRY 2`，即重试是被 flag 关掉的，
  不是 nextest 不重试超时。

### 修复（`42c5bb5d9`）：给这两条腿 mac 专属窗口，并证明这个窗口真的绑上

`[[profile.default.overrides]]` 新增一块，`platform = 'cfg(target_os = "macos")'`、
`filter` 为 `package(codex-cli) & binary(app_server_daemon)` 上那两条 `test(=…)`、
`slow-timeout = { period = "1m", terminate-after = 2 }`（2m；59.604s × 1.65 = 98.3s 的
观测外推，#19 量到的 20–65 % 手间方差）。放在 #19 那块**之后**、`packaged_daemon_`
两块**之前**——`slow-timeout` 取第一个匹配的 override，位置错了就静默无效。

| 检查 | 读数 |
| --- | --- |
| 绑定探针（把新块窗口临时改 1s×1，`NEXTEST_PROFILE=local`，与 CI 同 profile） | 四条 kill 读数 **1.004s / 1.004s / 1.007s / 1.005s**（两条腿各被尝试两次：本地 profile 带着 `retries = 1`，`RETRY 1`/`RETRY 2` 都打出）⇒ 新块确实是这两条腿的第一个匹配 override，没被前面任何规则抢走 |
| 真实窗口下同一对腿（`NEXTEST_PROFILE=local`，`--no-fail-fast`） | `Summary [50.362s] 2 tests run: 2 passed, 15 skipped`，`PASS [24.413s]` / `PASS [25.943s]`，rc=0 |
| 默认窗口下同一对腿（本机基线） | `PASS [25.884s]` / `PASS [26.698s]` |
| ubuntu 腿同两条（`39526586631` job 118982561606 缓存日志） | `PASS [3.746s]` / `PASS [4.351s]` ⇒ 窗口只给 mac，不动其他腿 |

探针用的临时配置只写在 scratch（`nt-probe.toml`），**仓库里那份只含真实窗口**；
提交前 `git status` 除本节文档外只有 `codex-rs/.config/nextest.toml` 一处改动。

**runner 一手还没拿到**：这条改动只可能由 mac 腿证明，而 mac 腿单手要 ~3h16m 外加
编译 ~2h。`42c5bb5d9` 与本节这份文档目前**在本地未推**（理由见 §7）；
`c7590fe24` / `a3b96ae90` 两手仍在跑，它们的 mac 腿按上面的历史**大概率仍会在这两条腿红**——
那是"修复前"的读数，不能反过来当修复无效的证据。

## 6.37 §6.34 那条"没有解释的形状"现在有机制了：25s 是 exec-server 进程死亡的签名，不是 `/dev/fd` 的第二表面

§6.34 把 hand 2 的 `FAIL [  25.316s]` 诚实登记成"这条形状我没有解释"，并留下判据：含修复的
下一手若这个形状仍在，必须单独再查。这一节把它查到底，用的是一条**零 CI 预算**的通道：在真跑
bwrap 的 `.45` 上，按时序杀掉这条腿自己 spawn 的 exec-server，看错误原文会不会变成 runner 上
那一串。

### 一手实验：杀点决定形状，+200ms 那一档逐字命中 runner

这条腿的服务来源是 `codex-rs/exec-server/tests/common/exec_server.rs:81-95`
（`codex exec-server --listen ws://127.0.0.1:0`，stderr `inherit`，`kill_on_drop(true)` 在 :111）。
脚本 `~/lx45-kill-server.sh` 每一档重启一次这条腿，在服务进程第一次可见后按 `${d}` 毫秒
`kill -9`。被测源码 md5 仍是 `65950dd697605817317835b9cf9208c8`（= `a3b96ae90` 的内容），
脚本打的 `HEAD=ecb4ba6bb` 只是那份检出的分支指针，不改变被测源码。`.config/nextest.toml`
的 `[profile.default]` 有 `retries = 1`（`local` 只是 `inherits = "default"`），所以每档有
`TRY 1`（中枪那一跑）与 `TRY 2`（重试，回到未杀的 A 形状）：

| 杀点 | `TRY 1` 时长 | 逐字错误原文 |
| --- | --- | --- |
| 不杀（基线） | FAIL 0.718s | 无 `Error:` 行 ⇒ A 形状断言，与下面那 8 次不杀同形 |
| +0ms | FAIL 0.155s | `Error: exec-server transport disconnected` |
| +50ms | FAIL 0.145s | 同上（裸形状，**没有** resume 子句） |
| +100ms | FAIL 0.176s | 同上 |
| **+200ms** | **FAIL 25.310s** | `Error: exec-server transport disconnected: failed to read websocket JSON-RPC message from exec-server websocket ws://127.0.0.1:43257: WebSocket protocol error: Connection reset without closing handshake; failed to resume exec-server session: no resume attempt finished within 25s (last attempt: failed to connect to exec-server websocket `ws://127.0.0.1:43257`: IO error: Connection refused (os error 111))` |
| +300ms | FAIL 2.380s | `Error: deadline has elapsed` |
| +500ms | FAIL 2.387s | 同上 |

runner 那原文（`39530269408`，端口 42967）与 +200ms 这档**除端口号外逐字同形**，
时长 25.316s vs 25.310s。再叠上此前两手同一形状的 25.298s（`39371971338`）与
25.251s（`39396134392`），五个读数里的 25s 是**客户端预算常数**（`SESSION_RECOVERY_TIMEOUT`
= `Duration::from_secs(25)`，`exec-server/src/client_recovery.rs:63`），不是被测耗时。

### 由此得到的三条结论，和被推翻的那条候选解释

1. **B 形状的含义就是"服务进程没了"，而且是在 session 建立之后没的**——早于连接的三档
   （0/50/100ms）只能产出裸的 `transport disconnected`，不带 resume 子句。§6.34 的候选解释
   "那次 abort 把 exec-server 连接一起带崩"由此被推翻：同一台机器、同一份检出（两组实验相隔
   11 分钟，`/tmp/leg{1..8}.txt` 13:06 vs `~/kx-out.txt` 13:17），**8/8 次不杀的 abort 里服务
   都活着**——16 跑全部 `panicked at exec-server/tests/exec_process/shell_snapshot.rs:352:13`，
   时长 0.678–0.774s，`Error: exec-server transport disconnected` 出现 **0 次**。
   **25s 形状不是 `/dev/fd` 缺陷的第二表面。**
2. **runner 上这条腿不产出任何 `/dev/fd` 证据**：它死在 transport 层（原文那句
   `failed to read websocket JSON-RPC message`，读事件那一步经 `?` 变成 `Error:`），
   从未走到 `exec-server/tests/exec_process/shell_snapshot.rs:352` 的断言，所以 `c387b44dc`
   那份 output/stderr/status/closed 收据在这三手一次也没打印出来。#42 的验收一手只能在
   "服务活着、断言真 fire"的机器上取——那就是 `.45`（§6.35 的 `FAIL [0.683s]` + 逐字 abort
   才是这条腿的已知状态）。"`blocked_descriptor_path` 在 ubuntu 腿上红"与"红得有意义"是两件
   事，本轮把它们分开登记。
3. **`a4dfd599f` 那次相位命名开始产出区分度**：老两手的 `recovery timed out after 25s` 无法
   分辨"在飞的 start 没排干"与"连不上端口"；`39530269408` 的
   `no resume attempt finished within 25s (last attempt: … os error 111)` 把后者钉死。25s 这个
   窗口本身仍不该动（它对齐服务侧 30s 保留窗，理由写在 `a4dfd599f` 的提交说明里）。

**未闭合的子问题（登记，不追）**：runner 上是谁在 ~0.3s 处杀掉了 exec-server。`SIGKILL` 不留
stderr，那几手的日志里也确实没有任何服务侧输出；本实验证明"端口未就绪就死"与"session 建立后
才死"在原文上可分辨，所以再现时缺的是进程死亡原因（OOM / runner 清理），不是更宽的窗口。按本轮
范围约束（不重开大规模迁移、不改挂载原语），这条不进代码。

### `39530269408` 的 mac / windows 双腿终态（§6.34 登记时仍在跑）

| job | 终态读数 |
| --- | --- |
| `nextest (macos-latest)` (118994304630) | `Summary [3480.467s] 22888 tests run: 22886 passed (28 slow), 1 failed, 1 timed out, 71 skipped` |
| `nextest (windows)` (118994304838) | `Summary [3052.165s] 21769 tests run: 21760 passed (23 slow, 293 leaky), 8 failed, 1 timed out, 138 skipped` |

- mac 的 `TIMEOUT [  60.014s]` 仍是 §6.36 那两条里的
  `codex-cli::app_server_daemon managed_identity_survives_locale_and_timezone_changes`
  （`( 4776/22888)`），同手另一条 `managed_daemon_restarts_from_deleted_working_directory` 是
  `PASS [  50.955s]`——离默认 60s 只剩 9s，趋势与 §6.36 的四手一致。这手 head 是
  `ecb4ba6bb`，**不含** `42c5bb5d9` ⇒ 它是"修复前"读数，不能反过来读成窗口无效。
- windows 9 条里 7 条落在已有家族：两条 `codex-uds` ACL（0.570s / 0.969s）、
  `conpty_ctrl_c_interrupts_powershell_foreground_child`（12.185s）、四条 `legacy_`
  （49.217s / 61.886s / 61.924s / `legacy_workspace_write_delete_is_limited_to_writable_roots`
  0.189s）。**A3 的判据这手还没到**：`8a41ac7d3` 那扇门不在 `ecb4ba6bb` 里，所以
  `conpty_ctrl_c` 在这里必红；能验收它的只有 `39542266851`（head `c7590fe24`，含门）与
  `39543230422`（head `a3b96ae90`）。
- `legacy_workspace_write_delete_is_limited_to_writable_roots` 又红（0.189s），而 §6.34 手 1 同一条
  是 `PASS [   1.872s]` ⇒ #43 的"这条红是间歇"再多一手支撑。
  **（前向指针：拿到取证后的两手表决逐字同形，"间歇"这个口径在 §6.40 作废；本条读数不改写。）**
- §6.34 标"本手首次观察，尚未分桶"的两条——`codex-tui app::tests::session_lifecycle_requests::
  dynamic_tool_requests_ignore_other_namespaces_and_dispatch_tui_namespace`（2.068s）与
  `codex-core::all suite::auto_review::remote_model_override_uses_catalog_model_for_strict_auto_review`
  （26.531s）——在 `39530269408` 全绿（分别 `PASS [  18.221s]` (15975) 与 `PASS [   5.191s]` (7136)）
  ⇒ 一手一现，改挂"间歇"，不再算未分桶。

### mac 的一条首次观察：`codex-cli::daemon_telemetry` 测的是一条故意尽力而为的遥测路径

逐字：`FAIL [  11.194s] ( 3185/22888) codex-cli::daemon_telemetry
foreground_update_honors_metrics_exporter_and_reports_unconfirmed`，
`panicked at cli/tests/daemon_telemetry.rs:55:9`，
`assert_eq!(requests.len(), usize::from(metrics_enabled))`，Diff `<0 / >1` ⇒ 期望 1 个
`/metrics` 请求，实到 0 个。

- 基线（缓存日志逐字核对名与时长配对）：mac 历史四手 PASS 7.292s / 14.365s / 0.756s / 9.154s，
  ubuntu 四手 PASS 1.073s / 1.483s / 0.783s / 1.201s ⇒ 本手 mac **首次红**，ubuntu 未红过。
- 本地一手：同机 idle 连跑 8 次 **8/8 PASS，0.248–0.320s**（比该手 runner 快 35–45 倍）。
- 机制（读码，不靠猜）：`cli/src/daemon_telemetry.rs` 这条路径是产品**故意**做成尽力而为的——
  `timeout(2s, cloud_config::load_config)` 超时就整段 `return`（:18-25，注释原文
  "Skip telemetry if consent-aware configuration cannot be loaded promptly"）；
  `build_provider` 包在 `catch_unwind` 里，`Ok(Ok(Some(_)))` 之外的任何形状也 `return`（:36-45）；
  最后 `shutdown_with_timeout(2s)` 的结果被 `let _ =` 吞掉（:58）。三者任何一个被争用越过，
  产出的都正好是"0 个请求"。
- 分桶：**harness 把产品的 best-effort 断言成了保证**；产品行为是有意的（不拖慢 CLI 退出），
  不是本轮要改的语义。单条首次观察 + 本地不可复现 ⇒ 不动代码，等 `39542266851`/`39543230422`
  的 mac 腿；若再现再单独拍。

### OAuth 那一对：第二平台再现，但我先试的那个假设被时间线否掉了

windows 这手同现两红：`login_account_chatgpt_uses_oauth_overrides` FAIL 1.251s
（`app-server/tests/suite/v2/account.rs:2747`，`500` vs `302`）+
`login_account_chatgpt_redirects_to_hosted_success_page` **TIMEOUT 60.030s**；
ubuntu `39396134392` 那手前者 FAIL 0.197s，`Connection refused (os error 111)` 打在
`http://127.0.0.1:1455/auth/callback`。

- 我试的假设是**跨包争抢固定回调端口**。代码侧确实支持它成立的条件：
  `login/src/server.rs:78/80` 是 `DEFAULT_PORT 1455` / `FALLBACK_PORT 1457`，而 `bind_server`
  （:637）在 `AddrInUse` 时会**主动向在位者发 cancel**（:662 `send_cancel_request(port)`）再重试，
  10 次 200ms 之后才落到 1457；`run_login_server` 的调用点包括
  `app-server/src/request_processors/account_processor.rs:665`、`cli/src/login.rs:167/420` 与
  `login/tests/suite/login_server_e2e.rs` 的十余条腿。
- 但**时间线不支持**：该手 `codex-login` 的腿集中在 03:39:10 之后（更早的 02:07:04 / 02:19:42
  两处提及只是 `Compiling codex-login`），比这次失败（03:05:49）晚 33 分钟；
  而 `codex-app-server::all` 在 `app_server_integration`（`max-threads = 1`）里本就串行，
  日志里 03:05:4x 那段是同包同 job 的顺序流（833→859 连续编号）。
- 基率（只算缓存日志里名与时长都能逐字配对的读数：4 手 `39347345560`/`39371971338`/
  `39396134392`/`39530269408` × ubuntu/windows = 8 个组合，**15 条 leg 读数**——`39347345560`
  windows 那份里 `uses_oauth_overrides` 不可读）：**3 条红**，ubuntu 一次
  （`39396134392` 的 `uses_oauth_overrides` `FAIL [0.197s]`，同手另一条 `PASS [0.295s]`）、
  windows 一次（`39530269408` 两条同现）。同一条腿在同一平台各只红过一次。
- 分桶：**间歇、机制未证明**。按"排除归属 ≠ 证明无关"，这两条与 OAuth 回调端口产品路径同族这一
  点仍然成立，但本轮拿不到 1455 的持有者证据，不改 `nextest.toml` 也不动产品。

### 本轮状态

零 dispatch、零推送。`42c5bb5d9`、`e14d73e76` 与本节文档仍在本地（详见 §7）。
`39542266851` 与 `39543230422` 的四腿在登记时仍 `in_progress`（两手
`bazel bridge gate (ubuntu)` 已 success）；前者是**唯一含 A3 门**的手之一，后者是**唯一含
`a3b96ae90`** 的手，A4/#42 的判据只能在它上面读。`.45` 侧留痕：`~/lx45-kill-server.sh`、
汇总 `~/kx-out.txt`、逐档原文 `/tmp/kx-{base,0,50,100,200,300,500}.txt`、对照组八连跑
`/tmp/leg{1..8}.txt`。两手 ubuntu 腿随后落地，读数见 §6.38。

## 6.38 两手 ubuntu 腿落地：同一平台上的 A/B 一手——fail-open 那手把红变成绿，纠正那手恢复"响亮且有收据"的红

登记判据（§6.35/§6.36 写下，逐字复述以免事后修饰）：`blocked_descriptor_path` 在含纠正的手上
**应继续红**，因为响亮 abort 是已知缺陷的正确形态；**"绿且无收据"才是回归**。两手恰好构成一组
A/B，因为它们的唯一差异就是那份 `/dev` 名单（`git diff c7590fe24..a3b96ae90` 只动
`linux-sandbox/src/bwrap.rs` 与文档）。

| 手 / head | ubuntu job | Summary 逐字 | `blocked_descriptor_path` |
| --- | --- | --- | --- |
| `39542266851` / `c7590fe24`（含 fail-open 的 `a4c0ff2fa`） | 119032454907 | `Summary [2884.828s] 22941 tests run: 22940 passed (22 slow), 1 failed, 48 skipped` | **`PASS [   0.445s]` (11038/22941)** |
| `39543230422` / `a3b96ae90`（纠正） | 119035505119 | `Summary [2401.627s] 22942 tests run: 22941 passed (18 slow), 1 failed, 48 skipped` | **`FAIL [   0.304s]` (11037/22942)** + 逐字收据 |

### 一手一：fail-open 不是假设，它已经在托管 runner 上把红变成绿

`c7590fe24` 那手把 `/dev/fd` 判成"缺席"、静默放过 Deny，于是 bwrap 不再拿到 `--tmpfs /dev/fd`、
不再 abort，快照正常恢复 ⇒ 这条腿**绿**。而这条腿的断言是
`shell_snapshot.rs:352` `assert!(output.ends_with("restored:input-{index}"))`，
`SnapshotSandbox::DenyFdPath` 分支下 `source_check` 是空串（:241-245）——**它压根不检查
`/dev/fd` 是否不可读**。所以这条绿不表达任何拒绝成立；它表达的是"Deny 被丢掉了"。
这正是"绿且无收据"的形态，本轮把它和 §6.37 的定性接上了：**这条腿的形状本身不是判据，
收据（`:352` 那句 `output/stderr/status/closed`）才是**。

### 一手二：纠正那手恢复的正是 A4/#42 要的 runner 一手

`a3b96ae90` 的 ubuntu 腿里，`c387b44dc` 那份诊断**第一次由托管 runner 打出来**（逐字）：

```
panicked at exec-server/tests/exec_process/shell_snapshot.rs:352:13:
bash#2: output="" stderr="bwrap: Can't mount tmpfs on /newroot/dev/fd: No such file or directory\n" status=Some(1) closed=true
```

⇒ 缺陷仍开放（O2 未做，`--tmpfs` 遮不住符号链接），但**响亮、可归因、且不需要我再借自己的机器**：
§6.34 那个"ubuntu 腿上这条红到底有没有意义"的问题现在有了答案——在纠正后的名单下有意义，
在 fail-open 名单下没有。两手各自的 `exec-server transport disconnected` 出现次数 **0 / 0**
⇒ §6.37 定性的 25s 服务死亡形状在这两手**一次也没复现**，与"间歇、与 `/dev/fd` 无关"一致。
`record_test_skip` 收据仍为 `no skip receipt file: either the suite never ran or no leg skipped`
（每手两次），#39 的"零跳过、全部真执行"继续成立。

### 一手三：harness 曾经把 fail-open 写成一条绿的断言

`c7590fe24` 里 PASS 的是 `codex-linux-sandbox bwrap::tests::only_dev_paths_outside_the_synthetic_set_count_as_absent`
（`PASS [   0.008s]` 12358/22941）——断言内容就是"`/dev` 下除那 10 项合成节点之外都算缺席"，
即把 `fd`/`core`/`pts`/`ptmx`/`console` 判成缺席那条**错的规则**钉成了测试。
`a3b96ae90` 把它改名并收紧为 `only_dev_paths_bwrap_never_creates_count_as_absent`
（`PASS [   0.005s]` 12359/22942），并新增 `denied_dev_symlink_is_still_masked_rather_than_declared_absent`
（`PASS [   0.008s]` 12347/22942）——后者断言 `/dev/fd` **仍然**拿到 `--tmpfs`，
就是防这条 fail-open 再回来的门。两手测试总数 22941→22942 差 1，正等于"改名一条 + 新增一条"的净增。

### `39542266851` 唯一的红：`daemon_startup` 那条腿在 ubuntu 首次红，且 #31 的诊断真的打出来了

逐字：`FAIL [  49.425s] ( 3486/22941) codex-cli::daemon_startup
discovered_managed_daemon_can_restart_with_auto_start_disabled`，
stderr `Error: discovered timed out waiting for Server:Localbackgroundserver:` 之后**带出整块
屏幕内容**（`>_ OpenAI Codex (v0.0.0)` / `/tmp/.tmpowr5Qq` / `Tip: This is a test announcement` /
`/status` / `GPT-5.6-Terra · … · daemon-restart-test`），`Caused by: deadline has elapsed`。

- 这是**该腿自带的 ~50s 等待预算**到点（49.425s），不是 nextest kill；所以 §6.36 那条
  "被 kill 的腿零诊断"在这里不成立——#31 加的 `waiting for …: <屏幕>` 诊断按预期渲染了。
- 基线（6 份缓存 ubuntu 日志逐字配对名与时长）：`PASS 10.950s`（`39347345560`）、
  `11.683s`（`39371971338`）、`3.944s`（`39396134392`）、`11.297s`（`39530269408`）、
  `5.354s`（`39543230422`）⇒ ubuntu 六手五绿一红，且**同期另一手同一条腿是 5.354s 绿**。
- 分桶：与 #26/#19 那族同形（harness 等待预算 vs 整机争用），单条首次观察 + 同期一手绿 ⇒
  不动代码；若再现，屏幕诊断已经够定位是哪一屏没等到。

### 状态

`39542266851` 与 `39543230422` 的 mac/windows 腿在登记时仍 `in_progress`（两手
`bazel bridge gate (ubuntu)` 已 success：前者 28m52s）。按既有规矩，未到终态的 job 日志不可取，
也不把"还在跑"读成结论。A3 的判据（`conpty_ctrl_c` 应不再红）**这两手读得到**——
`8a41ac7d3` 那扇门是 `c7590fe24` 与 `a3b96ae90` 的共同祖先（`git merge-base --is-ancestor` 实测 YES），
所以它的终态要看这两手的 windows 腿。#46 的判据这两手**读不到**：`42c5bb5d9` 不是
`a3b96ae90` 的祖先（实测 NO），它仍未推送，只能由推送后的第三手 mac 腿验收。

## 6.39 `39543230422` 的 windows 腿终态：A3 验收到手，同时翻出 Windows 删除边界的一条更重的红

job 119035504947（head `a3b96ae90`）终态 `failure`：
`Summary [2121.686s] 21769 tests run: 21762 passed (13 slow, 219 leaky), 6 failed, 1 timed out, 138 skipped`。
7 条红：两条 `codex-uds` ACL（0.568s / 0.341s）、三条 `legacy_`（58.290s / 60.886s / 61.264s）、
`legacy_workspace_write_delete_is_limited_to_writable_roots`（0.116s）、
`codex-app-server::all suite::v2::auth_storage_originator::concurrent_connections_export_their_own_storage_originator`
（**TIMEOUT 60.035s**）。逐条核对上一手（`39530269408` windows 腿 `8 failed, 1 timed out` = 9 条：
2 条 OAuth（`FAIL 1.251s` + `TIMEOUT 60.030s`）+ 2 条 uds + `conpty_ctrl_c` + 4 条 `codex-windows-sandbox`）：
那三条转绿使 9→6，但本手**新增**一条首次观察的 `auth_storage_originator` TIMEOUT ⇒ 7 条。

### 一手一：A3 的判据达成——同一台 runner、同一条腿，四红转一绿

`conpty_ctrl_c_interrupts_powershell_foreground_child` **`PASS [  12.071s]` (21391/21769)**，
而它在缓存的四手上逐字为 `FAIL 12.890s`（`39347345560`）、`11.886s`（`39371971338`）、
`11.447s`（`39396134392`）、`12.185s`（`39530269408`）。这手是两扇含 `8a41ac7d3` 门的手里
**第一个到达终态**的（另一手 `39542266851` 见 §6.40；`git merge-base --is-ancestor` 对两手都实测 YES），时长仍 ~12s ⇒ **它没有走跳过分支**，而是在
build ≥ 26100 上按门里那条"降级契约"断言通过（输入可达、kill 路径是唯一中断路径）。
本地 `.235` 那三手 3/3 PASS 12.011–12.827s、`27 tests run: 27 passed, 0 skipped` 与此一致。
⇒ #40 的 runner 一手补齐；另一手 `39542266851` 的 windows 腿作为二次确认当时仍在跑
（**§6.40 已收到：`PASS [12.738s]`，判决同形再现**）。

### 一手二：`legacy_workspace_write_delete_…` 这次带着完整判决——可写根之外的东西被删了

`FAIL [   0.116s] (21768/21769)`，`panicked at windows-sandbox-rs\src\unified_exec\tests.rs:1168:9`，
`assertion failed: (left == right): stdout=""`，**Diff 逐字**：

```
left  (actual):   (0, false, false, false, None, false, false)
right (expected): (0, false, false, false, Some("outside"), true, true)
```

七个槽位依次是 `exit_code`、`workspace_file.exists()`、`temp_file.exists()`、`tmp_file.exists()`、
`fs::read_to_string(outside_file)`、`protected_git_dir.is_dir()`、`protected_git_child.exists()`
（`tests.rs:1168-1186`）。⇒ 退出码 0、可写根内三个文件如期被删；**但 `outside` 文件读不到、
`workspace\.git` 不再是目录、`.git\config` 不存在**。

同一次运行里 #41 那份取证确实打了（before/after 两段 + 逐 ACE + `<-- THIS SESSION` 标注），
after 段的六行逐字：

```
workspace\workspace-delete.txt: here=[absent] in parent=[DELETE]
temp\temp-delete.txt:           here=[absent] in parent=[DELETE]
tmp\tmp-delete.txt:             here=[absent] in parent=[DELETE]
outside\outside-delete.txt:     here=[absent] in parent=[none]
workspace\.git:                 here=[absent] in parent=[DELETE]
workspace\.git\config:          here=[absent] in parent=[absent]
```

对照本地 `.235` 的同一份证据（§6.32 逐字保留）：`outside` 是 `here=[none]`（**文件还在**）、
`.git` 与 `.git\config` 是 `here=[none(DELETE granted, DELETE+FILE_CHILD denied)]`
（**存活，且有效位被沙箱自己的 deny 明确收回**）。

⇒ **这不再能用"间歇"打发**。托管 runner 六手读数：`FAIL 0.130 / 0.302 / 0.089 / 0.189 / 0.116s`
+ 一手 `PASS [1.872s]`（`39526586631`，那手还没有这份取证；`c7590fe24` 不是它的祖先，实测 NO）；
`.235` 13 次全绿且取证显示保护落地。**（§6.40 又收到一手 `FAIL 0.236s`，判决同形 ⇒ 完整口径以 §6.40 为准。）**
红的时候有可解释的删除发生，绿的时候（本地）有可解释的 deny 落地——差的是"为什么同一份代码在
runner 上让 `outside`/`.git` 被删"。§6.32 那句"定性：harness 缺陷，不是产品缺陷"与 #43 的
"这条红是间歇"在此**标注为待重判**，历史读数不改写。

代码侧能给出的候选机制（读码，不是主张）：

- `.git` 的 deny ACE **只点名 capability SID**（`windows-sandbox-rs/src/acl.rs:834-857`
  `add_deny_write_ace` → `add_deny_ace`，mask 含 `DELETE | FILE_DELETE_CHILD`）；
  而受限令牌的 restricting SIDs 里含 **logon SID 与 Everyone**（`src/token.rs:476-479`，
  `CreateRestrictedToken` 带 `WRITE_RESTRICTED`）⇒ 任何对这两类 SID 授予删除权的 allow ACE
  都能通过双重检查，而 deny 点名的 SID 不在其列。
- 句柄式 `SetSecurityInfo`（`acl.rs:961`）**不会把可继承 ACE 推给已存在的子对象**，
  所以 `.git\config` 的保护全靠父目录那条 `FILE_DELETE_CHILD` deny——一旦父目录 deny 没落地，
  子对象没有任何独立防线。
- `src/spawn_prep.rs:291/298/303` 三处 `let _ = add_deny_write_ace(...)` **吞掉并不上报所有 ACL 写失败**；
  `.git` 是否进入保护集合还取决于 resolve 时的 `is_file()/is_dir()`（`protocol/src/permissions.rs:2369-2371`）
  与 `dunce::canonicalize` 的静默回退（`src/path_normalization.rs:4-6`、`src/allow.rs:34`）。
- 这手 `outside` **目录**的 15 条 ACE 里 Users(545) 只有 `0x001200a9`/`0x00000004`/`0x00000002`
  （无 `FILE_DELETE_CHILD`），目录本身也没有 `S-1-1-0`（全日志仅 `D:\` 有两条 Everyone ACE，
  mask `0x001200a9`、`delete-rights=[]`）⇒ 删除不是经由"父目录允许删子对象"这条路。
- **但 session 前那份 ACE dump 里有一条我先前没读到的东西**：`outside\outside-delete.txt`
  自己带着 12 条 ACE，其中 ace[0]–ace[7] 是 8 条 `S-1-5-21-<机器>-<RID>` 的
  `allow mask=0x001301bf` **`delete-rights=[DELETE]`**，全部标 `INHERITED-FROM-ANCESTOR`
  （ace[8]–ace[10] 是 544/18/RID-500 的 `0x001f01ff`，ace[11] 是 Users）。同一份 mask 也出现在
  `.git`、`.git\config` 的 session 前 dump 里，而 `0x001301bf` **正是沙箱给可写根写授予 ACE 时用的那个
  mask**（本手 after 段 `workspace`/`temp`/`tmp` 各多出的那条 ` <-- THIS SESSION` 就是它）。
  ⇒ 删除所需的那个"对象自身可 DELETE"权限，在 runner 上是**宿主祖先天生的 ACL**给的，不是沙箱给的；
  文件级 `DELETE` 足以删掉该文件，不需要父目录的 `FILE_DELETE_CHILD`。
- ⇒ 候选机制因此收窄为一条：**这 8 条 allow 里的某个 SID 是否出现在沙箱受限令牌的 restricting SIDs 里**。
  日志不打印令牌 SID 列表，deny 又只点名 capability SID（`acl.rs:834-857`），所以现有词表到此为止；
  `.235` 那侧的对照读数是"这三条对象都活着且 `here` 里 deny 明确收回了 DELETE"，差的就是这一步归属。
- 还有一条只能记为"无法事后归因"的取证局限：`.git` 与 `.git\config` 在 after 段直接是 `absent`，
  ACL 已经随对象没了 ⇒ 不能从这份日志判断那两条 deny 到底落没落地（before 段 `.git` 的 15 条 ACE 里
  本来也不含任何 deny，那还是 session 之前的状态）。

**未做的主张**：我没有说过这是产品漏洞——按"排除归属 ≠ 证明无关"，现在的状态是
**判决已知（越界删除确实发生）、机制未证**。最短的证实/否证路径是本地零 CI 的 A/B：
在 `.235` 上把 temp 根分别放进"现状祖先 ACL"与"祖先 ACL 里给自己的用户 SID 加
`(OI)(CI)` 的 `DELETE`（复现 runner 那 8 条 ace[0]–ace[7] 的形状）"两档，各 ≥3 次隔离迭代，
看 Diff 会不会翻成 `None, false, false`。这一步会改宿主 ACL 与临时目录布局，
**不在本轮已授权范围内**（`windows-sandbox-rs` 也不在本轮 scope 路径里），等指针。

### 一手三：新的 windows 首次观察是 nextest kill，零诊断

`auth_storage_originator::concurrent_connections_export_their_own_storage_originator`
`TIMEOUT [  60.035s]`（默认 30s × terminate-after 2）。基线只统计本轮缓存里**不同 job** 的读数
（名与时长逐字配对）：windows `PASS 0.891s`（`39396134392`）/`1.003s`（`39530269408`），ubuntu
`0.547s`（`39396134392`）/`1.050s`（`39530269408`）/`0.758s`（`39542266851`）/`0.616s`
（`39543230422`），mac `0.355s`（`39396134392`）/`0.564s`（`39526586631`）/`0.413s`
（`39530269408`）⇒ 这 9 条全绿，本手首次红；再往前缓存里更早的手（`0.226`–`1.021s`）同样全绿，
我不把它们混进这个逐手配对的口径里。
正常耗时亚秒级，60s 是被 kill ⇒ 按 §6.36 的教训，这条日志里不会有
任何诊断，只能按"整机争用/harness 预算"分桶，单条观察不动代码。

### 一手四：OAuth 那对第二次没有再现

同手 `login_account_chatgpt_uses_oauth_overrides` `PASS [   0.401s]`、
`login_account_chatgpt_redirects_to_hosted_success_page` `PASS [   0.303s]`
⇒ §6.37 的"间歇、机制未证明"分类再多一手支撑（该腿族在 runner 上至今仍是 15 条可读读数里 3 红）。

### 更正我自己在 §6.38 写的一句话

我担心过"带自定义消息的 `assert_eq!` 会吞掉 tuple 值"。**实测否掉**：这份日志里
`Diff < left / right >` 完整打印了左右两个七元组 ⇒ 这条腿今后每次红都自带失败槽位，
不需要为了"打印 tuple"再改测试代码。取证侧真正缺的是"删除经由哪条 SID 落地"，属产品/deny 落地问题。

### 状态

`39542266851` 的 windows 腿与两手 mac 腿在登记本节时仍 `in_progress`（`39543230422` 的
ubuntu/windows 已终态、bridge gate success）。**前向指针：`39542266851` 的 windows 腿已在
§6.40 落定，其中"越界删除是间歇"的口径在 §6.40 作废。** 本轮**零代码改动、零 dispatch、零推送**；
`42c5bb5d9`、`e14d73e76`、`8b6fac76d` 与本节文档仍在本地（`origin/test` = `f6f74f484`）。

## 6.40 第二手 `39542266851`（head `c7590fe24`）windows 腿终态：A3 二次确认到手，越界删除逐字再现

job 119032455053 终态 `failure`：
`Summary [2945.285s] 21769 tests run: 21761 passed (23 slow, 149 leaky), 8 failed, 138 skipped`
（本手**没有** TIMEOUT）。8 条红：`codex-cli::bin/codex doctor::tests::mcp_http_probe_falls_back_to_get_when_head_times_out`
`0.098s`、`codex-tui app::tests::session_lifecycle_requests::dynamic_tool_requests_ignore_other_namespaces_and_dispatch_tui_namespace`
`FAIL + LEAK 14.026s`、两条 `codex-uds` ACL（0.808s / 0.574s）、三条 `legacy_`
（46.382s / 62.252s / 61.971s）、`legacy_workspace_write_delete_…`（0.236s）。

### 一手一：A3 的二次确认——两扇含门的手都绿了

`conpty_ctrl_c_interrupts_powershell_foreground_child` **`PASS [  12.738s]` (21391/21769)**。
这一手的 head 就是 `c7590fe24`（写/delete 取证那一手），同样含 `8a41ac7d3` 那扇门。
⇒ 含门的两手逐字 `PASS 12.071s` / `PASS 12.738s`，删门之后的四手逐字
`FAIL 12.890 / 11.886 / 11.447 / 12.185s`：**四红转两绿，A3/#40 的 runner 判据闭合**，
本地 `.235` 的 3/3 绿与 12s 降级契约时长三方一致。

### 一手二：`legacy_workspace_write_delete_…` 的判决在第二手逐字同形

`FAIL [   0.236s] (21768/21769)`，`panicked at windows-sandbox-rs\src\unified_exec\tests.rs:1168:9:
assertion failed: `(left == right)`: stdout=""`，Diff 与 §6.39 逐字同形：

```
left  (actual):   (0, false, false, false, None, false, false)
right (expected): (0, false, false, false, Some("outside"), true, true)
```

after 段六行逐字（temp 根这手是 `.tmpE3kMLq`，上一手是 `.tmp4WwQtn`）：
`workspace\workspace-delete.txt: here=[absent] in parent=[DELETE]`、`temp\…: [absent]/[DELETE]`、
`tmp\…: [absent]/[DELETE]`、**`outside\outside-delete.txt: here=[absent] in parent=[none]`**、
**`workspace\.git: here=[absent] in parent=[DELETE]`**、**`workspace\.git\config: here=[absent] in parent=[absent]`**。
`outside` 这手同样**没有**收到 ` <-- THIS SESSION` 那条授予 ACE（两手的 THIS SESSION 计数都是 3：
`workspace`/`temp`/`tmp`），而 session 前的 dump 里 `outside\outside-delete.txt` 与上一手一样带着
**8 条 `allow mask=0x001301bf … delete-rights=[DELETE]` 的 `INHERITED-FROM-ANCESTOR` ACE**
（ace[0]–ace[7]，两手的 SID 各不相同 ⇒ 托管 runner 每个 job 换账号，这正是"本地绿、runner 红"的差量所在）。

⇒ **§6.39 里那个"间歇"的口径到此作废**：带这份取证的 2 手全红、判决逐字同形；把范围放回
"托管 runner 上这条腿的全部可读读数"，是 `39347345560 0.130s` / `39371971338 0.302s` /
`39396134392 0.089s` / `39530269408 0.189s` / `39542266851 0.236s` / `39543230422 0.116s` 六红 +
`39526586631` 一绿（`PASS 1.872s`；`c7590fe24` 不是那手的祖先，实测 NO，所以那手没有取证）。
同一份代码在同一台 runner 上**六次越界删除、一次没有**，本地 `.235` 13 次全绿。
机制仍未证（缺的是"那 8 条 allow 里哪个 SID 在沙箱受限令牌的 restricting SIDs 里"，日志不打令牌），
但候选已经收窄到一条、且有两手独立 ACE dump 支撑 ⇒ D1-A 那个 A/B 现在是"复现 ace[0]–ace[7] 的形状"，
不再是"随便给点宽权限"。

### 一手三：两条新的/第二次观察，都分桶到 harness

- `codex-cli::bin/codex doctor::tests::mcp_http_probe_falls_back_to_get_when_head_times_out`
  `FAIL [   0.098s] (3243/21769)`，`panicked at cli\src\doctor.rs:4049:9`，Diff 逐字
  `Err("HEAD request timed out; GET request timed out")` vs `Ok("HTTP 405")`。读码：这条测试给
  **整条 HEAD→GET 回退只 `Duration::from_millis(10)`**（`doctor.rs:4042-4045`），HEAD 按设计超时后，
  GET 还要在同一个 10ms 预算里完成一次本机回环往返。缓存里这条腿除本手外 **18 条不同 job 的读数全绿**
  （windows 6、ubuntu 7、mac 5，时长 `0.071`–`0.332s`）⇒ **分桶 harness 缺陷（亚阈值超时预算）**，
  与 #19/#46 同族；单条观察不动代码。
- `codex-tui …::dynamic_tool_requests_ignore_other_namespaces_and_dispatch_tui_namespace`
  `FAIL + LEAK [  14.026s]`，`panicked at tui\src\app\tests\session_lifecycle_requests.rs:2407:9:
  expected follow-up task registration before its next turn`。**这是第二次观察**（首次即 §6.34 表里
  `39526586631` windows 腿那条 `FAIL 2.068s`，panic 文案与行号逐字相同）；其余 17 条不同 job 的读数
  （windows 5、ubuntu 7、mac 5）全绿、时长 `9.297`–`18.983s`。注意它**不是** nextest kill，也不是 `?` 那条 5s 超时：
  走到 else 分支意味着 `events.recv()` 收到了**另一个 `AppEvent` 变体**，而断言不打印收到的是什么
  ⇒ 现状无法归因，要归因得再往这条腿加诊断。本轮不擅自扩面（#41 那种诊断是用户点名的 A4 才做的）。

### 状态

两手（`39542266851`、`39543230422`）的 ubuntu 与 windows 腿全部终态；两手的 **mac 腿在登记本节时仍
`in_progress`**（job 119032454998 / 119035505279），#46 的判据仍要等推送后的第三手。
本轮**零代码/配置改动、零 dispatch、零推送**；`42c5bb5d9`、`e14d73e76`、`8b6fac76d` 起
仍在本地（`origin/test` = `f6f74f484`）。

## 6.41 #47 落地：写/delete 取证按 `WRITE_RESTRICTED` 的两张 SID 列表求交，`.235` 一手已取

§6.40 把这条腿的机制问题收窄成一个 datum——"那 8 条 allow 里哪个 SID 在沙箱受限令牌的
restricting SIDs 里"。**读码结论是：现有取证结构性地看不见它**，所以本轮动的是取证
（#41 那条路的延续），不是产品，也不是宿主 ACL。

### 结论先说，并更正本节先前登记的一处机制错判

取证现在把"删除权从哪来"落成一行判决：
`eff = (授予 restricting 列表的位) ∩ (授予 default 列表的位)`，两侧各自点名 principal 与 ACE 序号。

**本节先前那一版（未提交）把 `user` SID 当成了 restricting 列表的授予来源，那是错的**，已随本手
改写。`d918722f2` 版 `unified_exec/tests.rs:268` 只按 caps 求值
（`if !cap_ptrs.iter().any(|cap| EqualSid(sid, *cap) != 0) { continue; }`，`:346` 的
` <-- THIS SESSION` 同源），而这条腿的真实组成在 `token.rs`：

- `spawn_prep.rs:160` `get_current_token_for_restriction()` 取**当前进程令牌**作 base；
- `spawn_prep.rs:165` → `create_workspace_write_token_with_caps_from`（`token.rs:397-402`）
  = `create_token_with_caps_from(base, caps, &[])`；
- `create_token_with_caps_from` 在 `:476` 取 base 的 **logon SID**、`:478` 取 **Everyone**
  (`world_sid()`)，`:481` 注释写死顺序 `Capabilities…, ExtraRestricting…, Logon, Everyone`。

⇒ 这条腿的 **restricting 列表 = caps + logon + everyone，没有 user**；user SID 只由
`create_token_with_caps_user_and_additional_restrictions_from`（`token.rs:455-466`，
`:460` 取 `get_user_sid_bytes`、`:463` 入列表）加入，那是 elevated/readonly 后端走的路。
`WRITE_RESTRICTED`（`token.rs:500` `DISABLE_MAX_PRIVILEGE | LUA_TOKEN | WRITE_RESTRICTED`）
对写请求是**两张列表各求一次、取交**：restricting 一侧决定"沙箱被允许到哪"，default 一侧决定
"这个令牌本来能到哪"。所以只报一张列表——无论漏掉 user 还是把 user 错加进 restricting——
都会虚报权利：前者看不见经由身份 SID 落地的授予，后者会打印出令牌其实拿不到的权利。

### 改了什么（单文件 `codex-rs/windows-sandbox-rs/src/unified_exec/tests.rs`，相对 HEAD +362/−56）

1. `SidList`（restricting / default 各持一份）：caps 走 `push_ptr`（从 `LocalSid` 按
   `GetLengthSid` 拷成 owned 字节，不让判决依赖别人的分配），按 SID 文本去重，每条带
   `can_grant`。**`can_grant=false` 的条目不参与授予结算，但仍参与 deny**——这正是
   deny-only 组的语义，把它们整条丢掉会丢一半证据。
2. `push_token_sid_lists`：**用生产构造函数** `create_workspace_write_token_with_caps_from`
   建出子令牌，再从**建出来的那块令牌**上 `crate::token::token_groups` 读 default 列表——
   `LUA_TOKEN` 把 `BUILTIN\Administrators` 变 deny-only 这件事只有建出来的令牌才带着，
   读基令牌会读错。tag 为 `user` / `logon` / `grp#N`（user 先入列表，避免它被先出现的同 SID
   组抢走 tag）。restricting 列表没有查询接口，按 `create_token_with_caps_from(base, caps, &[])`
   的入参重建：caps → logon → everyone。任何一步取不到就写 `token SID note: …`，不 panic——
   取证不能自己变成红的原因。
3. 判决 `delete_right_verdict`：两侧分别 `allow & !deny` 后取交；一侧有权、另一侧没有时，
   打印 `none (R=[…] D=[…])` 保留"差一半"的形状，而不是只报 `none`。
4. dump 标注改成区分两列表：` <-- TOKEN R=cap#2` / ` <-- TOKEN D=grp#7`（一条 ACE 同时命中
   两侧就两个都打）。**口径变化要记住**：` <-- THIS SESSION` 和上一版的 ` <-- TOKEN {tag}`
   从本手起都不再可比。
5. NULL DACL 两侧都报 `NULL-DACL`，并在文字里说明"两张列表都到 FILE_ALL_ACCESS，取交后
   删除是允许的"，不再让"读不到 DACL"被当成"没有权利"。
6. 取证在 session 前后各跑一遍，两半要分开读：**before 那半必然 `R=[no delete grant]`**，
   因为 caps 的 ACE 是 spawn 路径装的（`tests.rs:1426` 注释就是为此）。本手实测：
   before 块 `TOKEN R=` 命中 **0 次**、after 块 **8 次**。这不是缺陷，但拿 before 块下结论会错。

### `.235` 一手（离线、零 CI、零 dispatch、零推送）

- 通道核对：远端 `pty-a3-20261021` 那棵树**没有 `.git`**（tar 送过去的），沿用逐文件比哈希的做法；
  这次改用 `cat | ssh 'cat > 路径'` 传单文件并在远端 `md5sum` 回比 —— 第一次 `SHIPPED_TESTS=` 里
  `wc -l` 报 1670、本地 1811，重传后 md5 **全等**（`3a57c21f…`）。**教训**：在同一条 `&&` 里
  `cat > f && wc -l f` 会读到未落盘的行数，验收要用哈希而不是行数。
- 脚本 `~/wd47b.sh` 里先 grep 改动标记再构建：
  `MARKER two-list-evidence=1 old-single-set=0 intersection=1` —— 证明跑的是两列表版，
  不是上一手那份单集合版（`push_token_identity_sids` 命中 0）。
- attempt 1 又撞 `0xc0000142 STATUS_DLL_INIT_FAILED`（`cargo test --no-run` 退出码 -1073741502），
  同命令 attempt 2 过：**`PASS [   0.314s]`、`Summary [   0.321s] 1 test run: 1 passed, 282 skipped`**，
  `--retries 0`，日志已回读到 mac 缓存 `wd47b-1.log`（331 行）。
- 两张列表都渲染出来了，且**没有任何 `token SID note`**（0 次）：
  `restricting SID list: [cap#0(S-1-5-21-1578035584-…-2872830911), cap#1(S-1-5-21-1594489522-…),
  cap#2(S-1-5-21-130350652-…-1060332426), logon(S-1-5-5-0-3573376961), everyone(S-1-1-0)]`
  `default SID list: [user(S-1-5-21-536205229-…-1002), grp#0(…-513), grp#1(S-1-1-0),
  grp#2(S-1-5-114) deny-only, grp#3(S-1-5-32-544) deny-only, grp#4(S-1-5-32-555),
  grp#5(S-1-5-32-545), grp#6(S-1-5-2), grp#7(S-1-5-11), grp#8(S-1-5-15), grp#9(S-1-5-113),
  logon(S-1-5-5-0-3573376961), grp#11(S-1-5-64-10)]`
  ⇒ **`LUA_TOKEN` 把 `BUILTIN\Administrators`(S-1-5-32-544) 与 S-1-5-114 变成 deny-only，这一手
  正面观测到**，取证按 `can_grant=false` 处理它们：`grp#3 is deny-only, DELETE+FILE_DELETE_CHILD
  named via ace[6] not counted as a grant`。
- **判决与断言自洽**（这是本手最有价值的部分）：
  - 三个应被删除的文件 `here=[absent]`（已被删，DACL 随之消失），其父目录给出**正例**：
    `workspace`: `DELETE via R[cap#2 holds DELETE via ace[0]] x D[…| grp#7 holds DELETE via ace[7]]`；
    `temp` 同形用 `cap#0`、`tmp` 同形用 `cap#1` —— 每个可写根各自的 cap SID 各就各位。
  - 两条必须存活的路径给出**交集为 0**：
    `.git`: `none (R=[cap#2 holds none (DELETE granted via ace[2], DELETE+FILE_DELETE_CHILD
    denied via ace[0])] D=[grp#3 is deny-only … | grp#7 holds DELETE via ace[9]])`；
    `.git\config`: 同形（granted ace[1]、denied ace[0]）。
    ⇒  **#41 要的"证明 enforce 而不是 argv"这条正面收据到手，而且比上一手更强**：
    default 一侧确实**有**权利（`grp#7` = Authenticated Users 持 DELETE），是 restricting 一侧被
    deny 收回才让交集归零——这正是 `WRITE_RESTRICTED` 唯一需要的那一半，也说明"只查身份 SID"的
    修法是错的。
  - `.git\config` 的 deny 是 `ace[0] deny mask=0x00130156 flags=0x10 INHERITED-FROM-ANCESTOR`
    点名 `cap#2` ⇒ deny 沿继承落到了 `.git` 的**后代**，不是只挡在 `.git` 那一层。
- 上一手登记的"身份 SID 在本机没有正例"这条限制**作废**：after 块 `TOKEN D=` 命中 **46** 次、
  `deny-only` 结算出现 **7** 次（before 块同样 7 次，全日志 14 次），default 列表的匹配路径被真实执行过。
- 本机 `outside` 的三条继承 allow 点名的仍是**历史 session 遗留的 cap 形状 SID**
  （`S-1-5-21-3506322609…`/`…1467988422…`/`…1619863088…`，都不在本令牌两张列表里）；
  `C:\git-workspace\pty-a3-20261021\codex-rs` 目录本身带着这三条**非继承而来**的 allow
  （`flags=0x03`）⇒ 共享祖先目录会累积往次 session 的 cap ACE。CI runner 复用工作区时就是这个形状，
  它是 §6.40 那条"runner 越界删除"候选机制的载体，本手只能把机制**可判决化**，不能替 runner 判决。

⇒ **本手能证明的**：两张列表渲染正确、deny-only 结算正确、交集判决与断言一致、deny 落在 restricting
半边、deny 随继承到达子路径。**仍不能证明的**：runner 上那条越界删除到底经由哪张列表的哪个 principal
——这需要下一手的 `eff` 行。

### 下一步要什么

runner 一手（把这笔改动推上 `test` 后）三选一判决：
`outside` 出现 `DELETE via R[cap#…] x D[…]` 而文件被删 ⇒ 越界删除有 DACL 依据，产品要把 deny
覆盖到那一侧；`outside` 仍是 `none` 而文件被删 ⇒ 删除不经过 DACL，得换假设；
出现 `unreadable` / `token SID lists unavailable` / `note` ⇒ 取证在 runner 上退化。
**这需要把这笔代码提交推上 `test` 触发 CI**。

### 状态

本节 = 一笔代码改动（`unified_exec/tests.rs`，相对 HEAD +362/−56）+ 本节文档，`.235` 一手绿。
mac 两手的终态登记在 §6.42。推送状态见 §7。

## 6.42 #46：两手 mac 腿全部终态，都是同一条 leg 撞 60s，且两手都不含 `42c5bb5d9`

§6.41 登记时两手的 mac 腿还 `in_progress`；现在都终态，**均为 failure**：

| run | head | mac job | 结果 |
| --- | --- | --- | --- |
| `39542266851` | `c7590fe24` | `nextest (macos-latest)` 119032454998 | failure |
| `39543230422` | `a3b96ae90` | `nextest (macos-latest)` 119035505279 | failure |

### 红的是哪一条，怎么死的

两条手各只有**一个**红，而且是**同一条 leg**：
`codex-cli::app_server_daemon managed_daemon_restarts_from_deleted_working_directory`

```
TIMEOUT [  60.008s] ( 4722/22888) … deleted_working_directory   （39542266851）
 Summary [3388.671s] 22888 tests run: 22887 passed (25 slow), 1 timed out, 71 skipped
TIMEOUT [  60.007s] ( 4722/22888) … deleted_working_directory   （39543230422）
 Summary [3349.727s] 22888 tests run: 22887 passed (28 slow), 1 timed out, 71 skipped
```

`FAIL` 计数在两手里都是 **0**——mac 腿没有断言失败，只有超时被 kill。
除 `Test (workspace)` 外每一步（Format / Clippy / Build / Skip receipts）都成功，
没有磁盘、取消、网络类原因。日志已缓存 `m39542266851.raw`/`.txt`、`m39543230422.raw`/`.txt`。

### 60s 这个界是被证明的，不是猜的

- 两个 SHA 上 `codex-rs/.config/nextest.toml` 里**都没有点名这条 leg 的 override**（`grep -c` = 0），
  于是落到 `[profile.default] slow-timeout = { period = "30s", terminate-after = 2 }` = 60s；
- lane 用 `NEXTEST_PROFILE=local` + `--retries 0`（一次都不重试，被 kill 只留 `TIMEOUT`）；
- 同族的兄弟 leg 自带 2m×2，能跑到 `PASS [174.159s]` / `PASS [182.729s]`
  （`packaged_daemon_start_and_explicit_replacement`）——**只有没被 override 的 leg 才死在 60s**。

### 跨手对比：这条 leg 不是间歇

`46.008/35.630s`（`39347345560`）→ `51.222/54.032s`（`39371971338`）→
`59.604/56.490s`（`39396134392`）→ 双双 60.012s 被 kill（`39526586631`）→
本手 `deleted_working_directory` 60.008/60.007s 被 kill。
另一条 `managed_identity_survives_locale_and_timezone_changes` 这两手回到 `PASS [42.857s]` /
`PASS [51.549s]`（仍在 60s 内，但 4 手序列里它 35→54→56→51 抖，撞界是它 `39530269408` 那手 60.014s）。
`daemon_telemetry foreground_update_honors_…` 那条 `39530269408` 的 FAIL 也已回绿
（`PASS [9.685s]` / `PASS [14.496s]`）。**失败集合是上一手的子集，没有新增项。**

### 这两手不能用来判决 `42c5bb5d9`

`42c5bb5d9`（给这两条 leg 加 1m×2 的 mac-only override）**仍在本地未推送**，
`origin/test` = `f6f74f484`，而两个 head（`c7590fe24`、`a3b96ae90`）都是 `42c5bb5d9` 的**祖先**
⇒ 这两手重观测的是"加宽之前"的 60s 界，**不构成"加宽后仍失败"**。
判据仍要推送后的第三手：那一手里这条 leg 应该以 `PASS [>60s]` 出现（像它的兄弟 leg 那样），
若仍在 120s 界上被 kill，才需要回头查 daemon 本身为什么回不来。

## 6.43 #45 落地（§6.35 的 O1）：`/proc` 下的 deny 掩码搬到 `--proc /proc` 之后，`.45` 端到端一手已取

用户表格第 3 项"缺的是修复后复跑"。代码改一处发射位置，一手在真能跑 bwrap 的 `.45` 上取
（`7.0.0-38-generic`、`bubblewrap 0.11.1`，验证树 `1af8b5a70` + 两个未提交覆盖文件，
md5 `0838014cab56c731ade218da6afedcd4` / `bc11224ad0b7fd10101315aef08707a5` 两端逐字一致）。
**表格里点名的机器是 `.41`，一手取在 `.45`**：`.41` 的
`apparmor_restrict_unprivileged_userns=1` 让 bwrap 起不了 userns（§6.35 已登记"执行一手只有
`.45` 能给"），本轮没有去动那台宿主的 AppArmor 配置。

### 机制与落点（不是"把 `--proc` 前移"，而是"把掩码后搬"）

`--proc /proc` 在 `bwrap.rs:457-460` 发射，而策略掩码整批来自 `create_filesystem_args`
并经 `args.extend(filesystem_args)`（`:430`）落在它**之前** ⇒ 新 procfs 过 mount 把 `/proc`
下所有掩码盖掉（§6.35 的 X4/X3 顺序对照已证因果）。§6.35 推荐的 O1 登记成"把 `--proc` 移到
deny mask 组装之后"；本轮实现的是它的**镜像**——掩码搬到 `--proc` 之后。两者要的顺序结果
同一个（掩码在 mount 之后落地），但只有镜像这一版做得到。表格给的另一个方向"按内核能力跳过 deny"
**没有采用**：跳过就是明知的 fail-open，而 §6.35 的 X3 已在同一个内核上量到这条 deny 能咬住
（`Permission denied`），不存在"能力不足"。于是载体只有两种——把掩码搬到 `--proc /proc` 之后，
或把 `--proc` 前移到掩码之前；后者**做不到**：根绑定本身就在那批参数里（受限策略由
`create_filesystem_args`（`:504`）在 `:605-607` 发 `--ro-bind / /`，全写策略由
`create_bwrap_flags_full_filesystem`（`:350`）在 `:354-356` 发 `--bind / /`），
而 `/newroot/proc` 这个挂载点正是根绑定带进来的 ⇒ 把 `--proc` 挪到 `:430` 之前，就是在
**尚不存在的目的地上**挂 procfs，正落在 X1 已证的"响亮 abort"那一类。
本轮因此只做**目的地分流**，`--proc` 的位置一字未动，这段参数序上的形状断言
（`bwrap_wslg_tests.rs:81`、`linux_run_main_tests.rs:226-228/316-317/336` 的 `windows(2)`，
`bwrap.rs:1877/1884` 对 `["--bind","/","/"]` 位置的断言）**零改动**通过（见下面全量 237 passed）。
落点：

- `BwrapArgs` 多两个字段（`bwrap.rs:128-143`）：`proc_scoped_args`（目的地在新 procfs 上的
  掩码）与 `relocate_proc_destinations`（只在 `options.mount_proc` 为真时置位，`:683`）。
- `mask_sink(destination)`（`:151`）**每个掩码组取一次**，让 `--perms/--tmpfs/--remount-ro`
  三段与 `--dir` 挂载点父目录始终整组同去向、相对顺序不变（`:1496-1538`）。
- 谓词 `mask_lands_on_mounted_procfs = procfs_entry_survives_new_pid_namespace && exists()`
  （`:166`）。两半都必要：
  - **不搬 `/proc/<数字pid>`**：`mount_proc = !no_proc && !inherit_pid_namespace`
    （`linux_run_main.rs:306`）⇒ 挂 `--proc` 的手必然带 `--unshare-pid`，新 procfs 里没有宿主
    进程。把这种掩码搬过去就是在缺失目的地挂 mount——X1 已证 bwrap 对此**直接 abort**
    （`Can't mount ...: No such file or directory`）。而"该宿主 pid 在新树里不存在"本身就是这条
    deny 要的效果，搬不搬都不漏。这一条是本轮唯一把"静默 fail-open"改成"响亮 abort"的风险面，
    所以专门写了 R1/R2 两条真机护栏。
  - **`mount_proc=false` 时全都不搬**：该分支没有过 mount 事件；WSL 分支的
    `--tmpfs /proc` 是空 tmpfs，缺席即拒绝，搬动只会新增 abort 面。
- 回退分支（`run_bwrap_with_proc_fallback` 撞 preflight 后 `options.mount_proc=false`）自动
  回到"掩码留在原地"，那条路径上 host procfs 未被重挂，掩码本来就咬得住。
- **搬的是掩码，没有搬授权**：只读 carveout 的 `--ro-bind <path> <path>` 分支（`:1383-1387`）
  与 daemon socket 掩码（`append_daemon_socket_masks`，`:671/:678`）仍留在 `args` 里，位置一字未动。
  一处要说清的口径差别：`mask_sink` 挂在两个叶子 helper
  （`append_empty_file_bind_data_args`、`append_empty_directory_args`）上，这两个 helper
  除了 deny 还被 `append_read_only_subpath_args` 用到——但那两处调的是 **missing**
  （`:1377` → `append_missing_*`）与**瞬时空元数据**（`:1364/:1367`）分支，
  前者被谓词里的 `destination.exists()` 直接挡回 `args`（不存在的目的地搬过去就是 X1 abort），
  后者是 `.git` 这类工作区路径，落不进 `/proc` ⇒ 实际会被搬走的只有 deny 掩码。
- **这条修复剩下的已知风险面**：`exists()` 判的是**宿主** procfs，新 procfs 里未必有同一个条目
  ——宿主上由别的挂载盖进 `/proc` 的目录（`/proc/sys/fs/binfmt_misc`、vendor 的
  `/proc/driver/...`）在 `--proc` 重挂后的新树里不存在，这种 deny 规则会从"今天静默空操作"
  变成"响亮 abort"。方向与 §6.35 的结论一致（fail-closed 且可归因，优于 fail-open 静默放行），
  但本轮**没有**为这一类取一手：`.45` 上没有现成的 binfmt_misc 挂载，造它要动宿主。
  已实测到的护栏是 R1/R2（数字 pid 类，风险面里唯一会常态触发的一支）与全量 237 passed。

### 五条新腿（`codex-rs/linux-sandbox/src/bwrap.rs`，`.45` 一手）

`only_proc_entries_the_new_procfs_recreates_move_after_the_mount`（`:3162`）
`PASS [0.006s]` · `denied_proc_file_is_masked_after_the_proc_mount`（`:3193`）
`PASS [0.012s]` · `denied_proc_directory_mask_travels_as_one_group`（`:3221`）
`PASS [0.012s]` · `denied_proc_pid_mask_stays_before_the_proc_mount`（`:3243`）
`PASS [0.012s]` · `proc_masks_stay_put_when_the_procfs_is_not_remounted`（`:3265`）
`PASS [0.012s]` ⇒ `rc_legs=0`、`Summary [0.014s] 5 tests run: 5 passed, 166 skipped`。
**这些个位/十位毫秒的 PASS 在这一族里是可解释的**：它们只构造 argv 不做 mount，且每条都先
`assert!(denied.is_file()/is_dir(), "precondition…")` 把"缺前置条件即空转"堵死；执行证据不靠它们，
靠下面那组端到端读数。
套件全量：`rc_full=0`、`Summary [6.437s] 237 tests run: 237 passed, 3 skipped`
（232 基线 + 5 条新腿，零红、零新增跳过）；`rc_fmt=0`、`rc_clippy=0`。
构建前 MARKER 自查：`mask_lands_on_mounted_procfs=2`、`proc_scoped_args=11`、`splice_after_proc=1`
⇒ 测的是改后的源码，不是上一轮留下的旧二进制。

### 端到端"拒绝仍然咬得住"的一手（`lx45-proc-fix-e2e.sh`，走产品 helper）

| 组 | 策略 | 修复后读数 | 修复前（§6.35 登记） |
| --- | --- | --- | --- |
| C0 | 无 deny | `rc=0`、`READABLE` | 同 |
| C1 | deny 工作区里的普通文件 | `rc=0`、`NOT_READABLE` | 同（对照：非 /proc 的 deny 一直有效） |
| P1 | deny `/proc/version` | `rc=0`、**`NOT_READABLE`** | `READABLE`（fail-open） |
| P2 | deny `/proc/version` + `ls -l` | `---------- 0 soddy soddy 0 …  /proc/version` | `-r--r--r-- 1 nobody nogroup 0`（= 新 procfs 里的真文件） |
| P3 | deny 目录 `/proc/sys` | `rc=0`、**`NOT_READABLE`** | `READABLE` |
| P4 | deny `/proc/sysrq-trigger` | `rc=0`、**`NOT_READABLE`** | `READABLE` |
| R1 | deny `/proc/1`（数字 pid，不搬） | `rc=0`、`SANDBOX_STARTED` + `SELF_OK` | `rc=0`（沙箱必须仍然起得来） |
| R2 | deny `/proc/1/environ` | `rc=0`、`SANDBOX_STARTED` | 同 |
| R4 | 无 deny，`echo` | `rc=0`、`SANDBOX_STARTED` | 同 |

P2 的属主从 `nobody nogroup` 变成 `soddy soddy`、模式从 `0444` 变成 `000`：这正是"掩码落在了
userns 里新建的 procfs 上"的形状，而不是宿主 procfs 被改写。R1/R2 是这条修复唯一可能造成的
回归（把静默失效换成 abort），实测两者都 `rc=0`。

### 顺带量到的另一条、且**本轮没有修**：`/proc/self/...` 的 deny 仍然开放

R3（deny `/proc/self/environ`）修复后仍 `rc=0`、`READABLE`。strace 一手说明这不是"掩码没搬"：
`"--proc", "/proc"` 在前、`"--ro-bind-data", "3", "/proc/self/environ"` 在后 ⇒ 目的地分流确实
生效了；仍然开放是因为 `/proc/self` 由内核**按读取进程各自解析**，建会话那一刻的 `self` 与被测
进程读到的 `self` 不是同一个目录。归因：**真实缺陷、非本轮引入、非本修复的目标**，
要修得先决定"自指 deny 路径是拒绝表达（响亮报错）还是按 pid 通配展开"——那是策略语义的决定，
不在"已确认缺陷 + 验证缺口"的范围内，留给下一次拍板。

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
  **本轮登记时的实际指针状态**：`origin/test` = `6e27a6c11`，本地未推的是 `8349736da`
  （`legacy_` 整机独占调度）、`0c566acbf`（文档）、`cefb81d57`（R5 栈预算）、
  `c27ec7653`（R6 hook 窗口）、`b4be82ca7`（文档）、`7c22bf2ed`（§6.14 文档）、
  `981490315`（§6.15 mac daemon 期限）、`d0e822ad9`（文档）、`95fb0949d`
  （**§6.16 产品修复：workspace-routing entry 钉回原生传输**）与本笔文档提交，共 10 笔。
  **压在 `39347345560` 之后的理由已经消失**：该 run 三条 lane 均到终态（ubuntu
  `completed/failure` 2 failed、windows `completed/failure` 11 failed、macos
  `completed/failure` 1 failed），花掉的预算全部拿到了收据，§7 第 1/2 项也各自得到根因或终态。
  **下一轮一次带上四件事**：R5 的推论（三条 `codex-tui` ABORT 是否消失——该 run 的源码里
  没有 `cefb81d57`）、§6.15 的 mac 窗口、§6.16 `::discovery` 的转绿、`8349736da` 的
  `legacy_` 整机独占。第二操作系统的收据**已全部在推送前拿到**，不在 CI 上赌：
  Linux L1 `140 passed`、L2 `5 passed`（`::discovery` 2.695s）、
  **L8a `codex-rust-rig-bridge` 337/337**（覆盖视觉 floor 修复）、
  L7b `45 passed`（B3 真机不复现）、L6 release 两枚二进制带 sha256（§6.19）。
- **推送已执行，但这 11 笔仍未产生 CI 收据（如实登记）**：`git push origin test` 把
  `6e27a6c11..062f98cfe` 推上去了，`origin/test` = `062f98cfe`、工作树 clean。
  推送后 `gh run list --branch test` 最新一手**仍是 `39347345560`（head `6e27a6c11`）**，
  没有新 run——原因是 tip 提交 `062f98cfe` 的消息里带 `[skip ci]`，GitHub 按规则跳过整个
  push 事件（与 §7 早前"带 `[skip ci]` 的文档提交逐笔实测未触发新 run"同一条行为）。
  **所以那四件事的 CI 侧收据还没有开始产生**，本轮**没有**擅自 `workflow_dispatch`
  （workflow 头部注释写明手动 dispatch 是单独的用户授权动作，交接约束也写了
  "不要重新 dispatch，也不要为了重新跑 CI 无意义地提交"）。要么由用户授权一次 dispatch，
  要么等下一笔不带 `[skip ci]` 的真实代码提交顺带触发——两条路都只在用户点头后走。
  **追加登记**：`0c501eed0..4d9f30b6f`（把 sandbox 探测落成 L8c 收据的那笔）同样带
  `[skip ci]`，实测也没有产生 run；`origin/test` 现在是 `4d9f30b6f`，工作树 clean，
  上面"四件事的 CI 收据"仍未开始产生，dispatch 与下一笔真实代码提交仍等用户点头。
  **后续（同一轮内）**：用户点头"跑一轮 workflow_dispatch"，于是
  `gh workflow run "Fork Cargo PR" --ref test` 发出 **run `39371971338`**
  （head `55fcee7cb`，2026-10-20T04:53:38Z），这一手就是本轮唯一的一次 dispatch。
  四件事的收据**全部到手并结案于 §6.20**：R5 的 `codex-tui` ABORT 归零、
  §6.15 的 mac daemon 窗口（18 PASS，最慢 173.300s）、`::discovery` 在 CI ubuntu
  **PASS 1.633s**、`8349736da` 整机独占**没能**改变 `legacy_` 那四条（结论收回，转 task #27）。
  同轮另见 §6.20 的 10 条首观新红（mac 8 + windows 2），按授权**没有补跑第二手**。
- **本轮（§6.21–§6.23 的处置）开工前的指针状态被用户的 git 整理改写过**：上一段"本地未推
  10 笔 / `origin/test` = `4d9f30b6f`"已经不作数——`git log` 实测 `test` 与 `origin/test`
  同为 `250187989`（`gh`/`git rev-list --left-right --count` = `0 0`，即已推平、无本地领先），
  工作树只剩本轮五个未提交文件。按"用户会在会话中途整理 git"这条，后续每一笔都以
  当场 `git status` / `git log` 为准，不引用本节早前记录的哈希。
- **本轮落成四笔代码提交 + 一笔文档提交，逐笔分片**（哈希按 `git log` 实测）：
  `0541a7eb6` §6.21 的三条 Windows 内部预算、`aca3a703a` §6.21 的 nextest 窗口
  （撤 `threads-required`、`legacy_` 给 2m×2、`code_mode_excludes_mcp…` 给 1m×2）、
  `d4d9f943a` §6.22 的 mac helper 可归因化、`38ace3fe1` §6.23 的
  `.gitattributes` prompt 模板 `text eol=lf`，最后是本笔文档。
  本轮**没有再 dispatch**：这一手 push 的 tip 提交**不带** `[skip ci]`，正是要让 CI
  而不是本机来验这五笔（Windows-only crate 在 mac 建不了，见 §6.21 的验证边界）。
  run id 与三腿终态按收据在下一笔文档提交里登记，不在这里预写。
  **推送之后本轮又量了一次 §6.23 那条缺陷的半径**（M-L12–L14），另加四条模板目录的
  `text eol=lf`；这第二批 pin 与这段登记落在**同一笔**后续提交里，消息带 `[skip ci]`：
  它只改 windows 构建机读到的模板字节，没有任何测试断言依赖，为它单开一手 CI 属于
  "为了重新跑 CI 无意义地提交"，交给下一次真实代码 push 顺带带上。
  触发 CI 的那一手是 **run `39396134392`**（`gh run list --branch test` 实测
  `event=push`、`headSha=de8fdb3f4…`、`status=in_progress`），
  **本轮没有用掉第二次 dispatch**。
- **收据等到手前，先把 §6.23 里那半句"CRLF 可能解释了 windows 那条 spawn 红"拿去证伪**
  （M-L15，§6.24）：**证伪成立**，mac 上把 7 个 permissions 模板全改成 CRLF、
  `codex-prompts` / `codex-core` 确实重编之后，该用例仍 **PASS 1.686s**。
  随这条改了三处已提交的措辞：§6.23 的 diff 形状从"多出两条上下文消息"**改成实测的
  四条**（developer `permissions+persistent_mode` / user `environment_context` /
  developer `multi_agent_mode` / developer `managed_developer_instructions`，
  全部带父 `turn_id 01a157f6…`、全部追加在尾部、`>` 行数 0）；
  §6.23 的 head delta 清单补上 `8349736da` 那条**调度**改动（`threads-required` 落在
  PASS 的 `6e27a6c11` 与 FAIL 的 `55fcee7cb` 之间，`aca3a703a` 又删掉它，
  所以本轮 run 恰好是"回到 39347 调度"的对照），把它列为候选机制而非因果；
  `.gitattributes` 注释里"ubuntu 与 macOS 渲染同一 fragment 用 `\n`"这句**没有日志实证**
  （六条腿日志里该 fragment 只打印过一次，就在失败断言里），改写为
  "同一字符串内 LF/CRLF 混排只能来自 CRLF 检出"+ 本地强制 autocrlf 复现 + mac 检出为 LF，
  并显式写明这条 CRLF **不是**那条 spawn 红的原因，防止后来者把两件事重新捆在一起。
  这一笔（文档 + `.gitattributes` 注释）带 `[skip ci]`：它不改变任何规则解析，
  实测五个目录的 `git check-attr eol` 仍是 `lf`、`.gitattributes` 非注释行仍是 9 行、
  五个目录下 HEAD blob 含 CR 的文件数仍是 0。
- **§6.25（M-L16）这一笔只动文档**：实验用的临时改动（`core/tests/common/lib.rs` 里把
  `deadline` 就地 cap 成 500ms / 1ms）**没有进入任何提交**——L16c 与 L16f 各是一次
  `git checkout` 还原后的复跑（PASS 0.650s / 0.617s），脚本末尾的 `git status --porcelain`
  是空的，这就是"改过又还原干净"的收据。同样带 `[skip ci]`：`d4d9f943a` 已经在
  `39396134392` 的 head 里，本轮不为一条已经验过的诊断再开一手。

- **推送前把两处自己写坏的账改回来了**（如实登记，不留"看起来对"的版本）：
  ① §6.21 第一版把外层窗口写成 1m×2，但 tty 那条腿是**两个** lifecycle 顺序跑的
  （`tests.rs:1052-1054`），最坏 190s > 120s，窗口会在跑满时把它杀成裸 TIMEOUT——
  已改 2m×2，并把"内层预算**之和** < 外层窗口"写成注释里的约束；
  ② 第一版注释把 60s 说成"实测最坏值的 ~3 倍"，而 CI 历史只给出下界
  （cancellation >30s、另两条 >10s），没有上界 ⇒ 措辞改为"余量，够不够由本轮收据回答"。
  `0541a7eb6` 的提交消息也就地改写过一次（本机未推、纯消息、tree 不变）。
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
     → ~~"fork 历史回归 / 上游通过、fork 失败"这半句拿不出凭据，按 §6.9 撤回~~
     → **已根因并已修（§6.16，提交 `95fb0949d`）**。定性是**桥未实现 → 真实功能缺口**：
     分叉默认策略把 `name="OpenAI"` + `…/backend-api/codex` + `requires_openai_auth` 这类
     entry 判给了桥，而 workspace routing 的 discovery **只在原生路径**
     （`responses_api_provider` 是 `workspace_routing(…)` 的唯一调用方），桥路径固定
     `ClientRouting::ConfiguredProvider`，因此那条 discovery-401 → 刷新恢复永远不会发生，
     用例等的正是它。修法是把有 routing 能力的 entry 钉回原生传输（3 行 + 1 条回归用例），
     而不是把 routing 移植进桥。
     **归属凭据**：引入点可指到分叉提交 `0f62ba844`（`uses_model_bridge()` 这套策略本身，
     `git log -S` 唯一命中）；用例文件与三个直接相关文件在 `main..test` 上**0 笔分叉提交**。
     §6.9 当时撤回那半句是对的（那时确实无凭据），本轮是**补上凭据**而非推翻。
     **首败→修复后证据**：CI 三腿 `FAIL 60.783 / 60.394 / 60.525s`（本轮第一次带 wiremock
     `Mock #2 … matched 0` 诊断）→ 本机 `::discovery` **PASS 0.660s**（默认 feature）与
     **PASS 0.696s**（CI 的 `codex-core/rust-rig` feature），`::responses` 对照组不受影响；
     Linux 第二操作系统 L1 `140 passed`。
     **本项仍未闭合的部分**：只剩"CI 侧转绿的收据"——它只能由这笔推送之后的下一轮给出；
     run `39347345560` 的源码里没有这个修复。§6.10 第 1 类"确定性挂起待根因"就地作废。
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
     → **§6.15 关掉 (b) 的开发部分**：`981490315` 给这 5 条发了 mac-only `2m × 2` 窗口，
     命中与优先级用真实配置副本实测过（本机 1.002s TIMEOUT），但**转绿与否只能由推送后的
     下一轮给收据**。(a) 分片仍是待开发项，本项整体未闭合。
     → **run `39347345560`（head `6e27a6c11`）mac 腿到终态，两条待办都被改写（§6.16）**：
     job 结论 `completed/failure`，`00:23:32 → 02:11:10` = **1h47m38s**（矩阵里 macos 的
     `timeout-minutes` 现为 **330**），
     `Summary [2646.520s] 22877 tests run: 22876 passed (23 slow), 1 failed`——唯一失败就是
     第 1 项那条 `::discovery`。
     (a) **分片不再是待开发项**：这一手用 330 分钟预算中的 107 分钟跑完，**余量 3h42m**，
     "分片是为了救 3h 时限"这个前提被直接打掉；分片降级为"若 mac 编译时长再翻倍"的备选，
     本期不排。
     (b) **`981490315` 的口径降级为余量保险**：那 5 条 `codex-cli::app_server_daemon` 在
     **不含该提交的源码**上就全绿——PASS **44.412 / 44.844 / 51.009 / 72.357 / 124.981s**。
     所以上一手的 5 条 TIMEOUT 是**负载造成的贴线**，不是窗口不够；这条要如实写，
     不能把 `981490315` 说成"让 mac 腿转绿"的修复。
     定性与终态：**mac 完整验收已达成**（第一次拿到跑完且可与上一手逐条对照的 mac 收据）；
     本项剩下的只有"该腿不再有可归因分叉的失败"，而那取决于第 1 项修复推送后的下一轮。
  3. windows 尾项家族（ConPTY、unified_exec、uds ACL、elevated runner、cur 检测等）与
     ~~`shell_snapshot…blocked_descriptor` 抖动~~
     → ~~**B3 方向性结案（§6.19 L7b）**：真 Ubuntu 机器上该用例 **PASS 0.085s**（同批 45/45 全绿），
     mac CI 腿 **PASS ×3**，只有 ubuntu CI runner **FAIL 0.346s**。**同 OS 不同机器结果相反**，
     落第 4 类；runner 上的根因仍未取到，所以本项从"待真机复现"改为
     "**真机不复现、runner 根因未取**"，不再算 Linux 侧待办，留在 windows/CI 环境家族里。~~
     → **该结论已被 §6.27 作废**：`.41` 上那条 PASS 是 bwrap guard 的跳过式绿，从未执行；
     且 §6.29 把六手缓存日志重读后确认 ubuntu CI runner **六手六红、两种形状各三手**
     （0.314 / 0.346 / 0.353s 的 `:344` 空 stdout，与 25.342 / 25.298 / 25.251s 的 transport reset）。
     ⇒ 本项现在是**"runner 根因未取，且 0.35s 形状此前无可归因字段"**：`c387b44dc` 已把那条断言
     改成携带 `{shell}#{index}` / stderr / status / closed，下一手 push 后的 runner 日志才第一次
     能自证。`a4dfd599f` 分的三个 recovery 分支只覆盖 25s 那一形状，本手没触发，因此它既有
     一手也没被这轮 CI 证实。mac 侧真实执行仍只有 §6.27/§6.28 的本机收据；`.41` 按 §6.27
     不能当这条腿的第二平台。
  3b. **本轮新增的 Linux 侧事实（不是缺陷，是能力边界）**：`swufe@192.168.31.41` 上
     `kernel.apparmor_restrict_unprivileged_userns = 1`（Ubuntu 24.04 默认），
     非特权进程**能建 userns、但不能写 `uid_map`**
     （`unshare -Urm` rc=1 `write failed /proc/self/uid_map: Operation not permitted`，
     而 `unshare -Un`（不带 `-r`）rc=0），bwrap 需要的正是后者
     （`bwrap … /bin/true` → `setting up uid map: Permission denied`），
     因此 codex 的 Linux sandbox 起不来（带 `--unshare-net` 时先在
     `RTM_NEWADDR` 处失败）。在该机上
     `tools::sandbox_denied_exec_command_returns_original_output` **必红**，
     4 条 `network_approval` Guardian 用例的语义也与 CI ubuntu 腿相反（CI 那 4 条全绿）。
     空闲复跑 L8b（`load after: 2.03`）**仍 5/5 红**，负载已被排除，机制用系统 `bwrap` 直接
     量出（§6.19 批次 8）：**环境能力，第 4 类**。
     **交付面相关**：容器/无 userns 宿主里这一族用例整体失效，且 `OnRequest` 的网络审批链路
     会退化，部署与验收口径都要跟着改；本轮**没有**改那台机器的 sysctl（属安全策略变更）。
  3c. **验证基础设施缺口（待开发，未开工）**：off-CI 建不出 `codex-code-mode-host`——它把
     `v8 150.4.0` 拉进构建图，build script 要下载 `ptrcomp_sandbox_release` prebuilt，
     而这个下载在 CI 外**必然失败**（mac：urlopen 异常；Linux：HTTP 404），因为成对工件是
     `.github/actions/setup-rusty-v8` 在用 `third_party/v8/rusty_v8_150_4_0_release_manifests.sha256`
     做信任锚之后注入 `RUSTY_V8_ARCHIVE` / `RUSTY_V8_SRC_BINDING_PATH` 的。后果就是 §6.16.1
     与 §6.19 里反复出现的"本地单 package 跑出一族 code_mode 红、CI 同族全绿"。
     本轮**没有**补这份下载（体积大、CI 已覆盖同名用例），只登记机制与复现命令。
  4. ~~验机 `swufe@192.168.32.163` 的 rustup 组件安装存在 manifest 损坏；pinned 1.95.0
     的 Clippy 以 mac 结果为准，不擅自改动仓库锁定版本。~~
     → **按 §6.17 改判并已修**：不是 manifest 下载损坏，是那个 `1.95.0` toolchain 目录被
     **rust-installer**（`install.sh --prefix=`）写过——三份 manifest 里 **100 %** 的行是
     绝对路径（`manifest-cargo` 44/44、`manifest-rustc` 41/41、
     `manifest-rust-std-…` 62/62），rustup 任何组件事务都在 `relpath.is_relative()` 断言处
     panic，`rustup component add` 则报 `missing manifest`。处置是把坏目录**隔离**
     （`~/.rustup/quarantine/1.95.0-broken-installer`，1.6 G 原样保留、可随时还原）再用
     rustup 正常安装**同一个 pin**；`rust-toolchain.toml` 一个字没动。
     Linux 腿（`swufe@192.168.31.41`，即原来那台换了网段的机器）因此恢复可跑。
     仍然保留的半句：pinned 1.95.0 的 **Clippy 结果以 mac 为准**（Linux 侧本轮只跑测试）。
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

- **本轮最后的提交与推送（实测）**：`266a39d73`（文档，`[skip ci]`，实测不产生 run）→
  **`a4dfd599f`（代码**，`client_recovery.rs` 三个超时分支文本分岔，1 文件 +14/−5，**不带**
  `[skip ci]`）→ 本节这份文档。`git push origin test` 输出 `266a39d73..a4dfd599f test -> test`，
  实测产生 run **39430526421**（`event=push`、head `a4dfd599f`、13:27:21Z 入队）——与 §6.26 那手
  `[skip ci]` push 不产生 run 构成同一台机器上的正反对照，也说明"代码 push 交 CI"这条分工仍然
  只在真实代码变更时才动用。本节这份文档提交带 `[skip ci]`，且**在 39430526421 在飞时推送**：
  它不该、也不会为该 head 之外新开一手。

- **§6.28 那一手的提交与推送状态（实测，本机）**：代码 `01cfd6449`
  （`feat(cli): select the native Responses transport with NUWAX_BRIDGE`，19 文件、
  产品 4 处 + 隔离白名单 + 测试与清理名单，不带 `[skip ci]`），其后是本节这份文档提交。
  **这笔文档故意不带 `[skip ci]`**：`origin/test` 仍是 `e43a51da3`，一旦按授权推送这两笔，
  tip 若带 `[skip ci]` 会让整次 push 被跳过，`01cfd6449` 的全量 CI 收据就永远不产生
  （§6.20/§7 已两次实测过这个形状）。**本轮未推送**，`git push` 仍只在用户明确要求时执行。
  离线侧全部收据见 §6.28；工作树在本笔提交后 clean。
- **§6.29 那一手的提交与推送状态（实测，本机）**：新增一笔测试侧改动
  `c387b44dc`（`test(exec-server): make the concurrent-replay assertion name its own receipt`，
  单文件、只扩断言消息，本机 `11 passed; 0 failed; 73 filtered`，`cargo fmt --check` 与
  `cargo clippy -p codex-exec-server --test exec_process` 干净），其后是本节这份文档提交。
  **本地未推送的栈现在是三笔**：`01cfd6449` → `486ed23a4` → `c387b44dc`（+ 本笔文档），
  `origin/test` 仍是 `e43a51da3`。文档笔继续**不带 `[skip ci]`**，理由与上一条相同：按授权
  推送时 tip 带 `[skip ci]` 会跳过整次 push，`01cfd6449` 与 `c387b44dc` 的 runner 收据就永远
  不产生。**本轮既不 push 也不 dispatch**，`git push` 仍只在用户明确要求时执行。
  run 39430526421 是本节最后一次 CI 观测读取；`.41` 上 `lx-verify9` 是本轮唯一的新执行，
  零 CI 预算、零厂商调用。
- **§6.30 那一手：用户明确要求 push，栈已上去（实测，本机）**。`git push origin test` 以
  `e43a51da3..04623ca2c` 落地，触发 run **39526586631**（`event=push`、head `04623ca2c`、
  01:18:56Z 入队；四腿 01:19 全部 `in_progress`，本轮 mac 腿**没有**在排队阶段被取消）。
  这一手含 `c387b44dc` 的新断言文本，是 #23 的第一手 runner 自证。其后本地两笔代码提交
  `bb2c0f9eb`（A1，单文件 `exec/tests/suite/nuwax_env.rs`）与
  `6c176ecec`（A2，四文件：`exec-server/tests/common/mod.rs`、
  `exec-server/tests/exec_process.rs`、`exec-server/tests/exec_process/shell_snapshot.rs`、
  `.github/workflows/fork-cargo-pr.yml`）+ 本节文档笔，**一并再 push**，
  使 ubuntu 日志同一手里既有 `c387b44dc` 的自证断言、又有 A2 的收据 census。
  `.41` 上的 `lx-verify10`（L10a/L10b/L10c）是新执行：零 CI 预算、零厂商调用，
  源码经 `git bundle create … 486ed23a4..test`（**14.6 KB**）→ scp →
  `git fetch <bundle> test` → 独立分支 `qoder-verify-20261021b`，旧分支
  `qoder-verify-20261021` @ `486ed23` 原样保留。

- **§6.31–§6.34 这一手的提交与推送（实测，本机）**：三笔代码提交分片落地——
  `a4c0ff2fa`（§6.33 的产品修复：不再对合成 `/dev` 里本就不存在的 `/dev/*` deny 目标下 mask，
  单文件 `linux-sandbox/src/bwrap.rs`，含三条新腿）、
  `8a41ac7d3`（§6.31 的 A3 门：`utils/pty` 三文件，恢复 `psuedocon` 可见性以读 build 号）、
  `c7590fe24`（§6.32 的 A4 取证：`windows-sandbox-rs/src/unified_exec/tests.rs`，+346/−3，
  删掉的三行只有旧断言元组与旧 panic 消息，契约语义原样保留并多一项 `.git\config` 探针）。
  **推送分两步**：先把 `origin/test` 推到 `c7590fe24`（tip 不带 `[skip ci]`，触发本轮唯一
  一手自动 run，head 含全部三笔），随后本笔文档提交带 `[skip ci]` 单独推上，不产生第二手。
  这样买到的东西是可核对的：ubuntu 腿能同时判 §6.33 修复是否让
  `blocked_descriptor_path` 的两种形状（`0.336s` 的 `bwrap:` stderr 与 `25.316s` 的
  transport 断连）消失，windows 腿能判 A3 那道门在新 head 上是否真的不再红。
  **第二、第三平台的收据仍全部在推送前拿到，不在 CI 上赌**：`.45` 上
  231/231、leg `PASS 0.909s`、`exec_process 57 passed/3 failed`、`rc_clippy=0`、
  strace 28 次 bwrap execve 中 0 次带 `/dev/fd`；`.235` 上 A6 3/3、A7 10/10、
  §6.31 的门控后三手 PASS。**五个改动文件的 md5 在 mac 工作树、`.45`、`.235` 三处逐字节一致**
  （`c2283864…`/`aed947d8…`/`92e3e9de…`/`90a3eb4f…`/`57cf44bf…`），
  所以这些一手确实覆盖即将推送的这份源码，而不是它的前一版。
  **⚠ 上面那组 `.45` 读数记的是 `a4c0ff2fa` 那一版，其"leg 转绿/0 次 mask"状态已被 §6.35
  作废**；`.235` 的 A3/A4 三组一手与 md5 一致性结论不受影响。
  **本轮没有 dispatch**：`39526586631` 的 mac 腿与 `39530269408` 的 mac/windows 腿在登记时
  仍 `in_progress`，日志未到终态不可取（§6.34）。
  **仍未闭合的**：#24 rusty_v8 离线配方等范围拍板；A2 覆盖面三处
  （`codex-linux-sandbox` 32 条 / `codex-exec` 4 条 Landlock / Windows MXC 4 条）未接；
  ubuntu hand-2 那条 `25.316s` 形状**未被解释**，不能算已被 §6.33 覆盖。

- **同一轮内的自我推翻与纠正（实测，本机）**：上面那条把 `a4c0ff2fa` 记成"产品修复"之后，
  我接着测出的第一件事就是它本身是 fail-open（§6.35），于是 fix-forward：
  `a3b96ae90`（`fix(linux-sandbox): only wave through /dev denies bwrap never creates`，
  单文件，名单 10→15 项、新增一条"活符号链接仍必须 mask"的腿、缺席腿改用 `/dev/kmsg`、
  并修正一条我自己写反的断言——`/dev/pts/1` 应判"可 mask"而不是"缺席"，这条断言在 `.45`
  上先红了一次才改对）。**推送状态**：代码笔 `a3b96ae90` 已推上 `origin/test` 并产生
  run `39543230422`；本节这份文档笔带 `[skip ci]` 随后单独推送，不产生第二手。
  这一笔触发 run **39543230422**；它要读的判据和 39542266851 不同——
  ubuntu 的 `blocked_descriptor_path` **应当继续红**（响亮 abort 是已知缺陷的正确形态），
  若它在这一手里变成"绿且无收据"，那才是回归。39542266851（head `c7590fe24`）
  测的是已被取代的那版 `/dev` 名单，登记时它仍 `queued`，其 ubuntu 腿读数**不能**
  当作 §6.33 那版修复的验收。
  **本轮 CI 之外的新执行**全部在 `.45` 上完成（strace 与裸 bwrap 对照 + helper 端到端），
  零 CI 预算、零厂商调用。

- **§6.36 那一手（实测，本机，**未推送**）**：一笔 CI 配置改动
  `42c5bb5d9`（`test(ci): give the two macOS daemon legs the window they straddle`，
  单文件 `codex-rs/.config/nextest.toml`，只新增一个 mac 专属 override），其后是本节
  这份文档提交。**两笔都留在本地**：`origin/test` 仍是 `f6f74f484`，本地 `ahead=2`，
  工作树在本笔提交后 clean。不推的理由是预算而不是完成度——这条改动只能由 mac 腿验收，
  而 mac 腿当前一手要 ~3h16m 再加 ~2h 编译，且 `39530269408` 的 mac 腿正在跑、
  `39542266851` / `39543230422` 四腿未终态；此时再开一手会把 mac lane 排到更后。
  **等这两手落地后一并推**（tip 不带 `[skip ci]`，按实测"一次推送只在 tip 触发一轮"
  只产生一手），若期间 O1/O2 有指针，也能合进同一手，省下一整手 mac 预算。
  本轮**零 dispatch**、零厂商调用；所有验收读数来自本机 `cargo nextest`
  （绑定探针四条 kill 读数 1.004–1.007s、真实窗口 `2 passed` 24.413s/25.943s）
  与四手缓存 mac/ubuntu 日志的逐字核对。
  **登记时的四手实况**：`39526586631` 四腿终态 run `failure`（§6.34 + §6.36）；
  `39530269408` ubuntu/windows/bridge 终态、mac 腿 `in_progress`（04:31:54 尾停
  `7586/22888`）；`39542266851` 与 `39543230422` 各剩三腿在跑，
  两者的 `bazel bridge gate (ubuntu)` 已 success。
- **§6.37 那一手（实测，本机，**仍未推送**）**：本轮**没有新的代码或配置改动**——全部是
  零 CI 预算的取证与登记（`.45` 上按时序杀 exec-server 的复现实验、五份缓存 job 日志的逐字
  核对、本机 mac `daemon_telemetry` 八连跑 8/8 PASS 0.248–0.320s）。栈仍是
  `42c5bb5d9` + `e14d73e76` + 本笔文档：`origin/test` = `f6f74f484`，本地 `ahead=3`，
  工作树在本笔提交后 clean。不推的理由与上一条相同（mac 腿预算），判据也相同：#46 只能由 mac 腿
  验收，而在飞两手里 `39543230422` 是**唯一**含 `a3b96ae90`、因而能读 #42/A4 判据的一手。
  这两手落地后一并推送；若期间 O1/O2 拿到指针，就合进同一手，省一整手 mac 预算。
- **§6.38 那一手（两手 ubuntu 腿终态，纯登记）**：`39542266851`（`c7590fe24`）与
  `39543230422`（`a3b96ae90`）的 ubuntu 腿各自 1 红，构成一组 runner 侧 A/B——
  fail-open 那手 `blocked_descriptor_path` **`PASS [0.445s]`**（绿而无拒绝），纠正那手
  **`FAIL [0.304s]`** + 逐字 `bash#2: output="" stderr="bwrap: Can't mount tmpfs on
  /newroot/dev/fd: …" status=Some(1) closed=true`。A4/#42 要的"响亮且有收据"一手**已到手**，
  缺陷本身仍开放（O2 未做）。两手 `transport disconnected` 计数 0/0 ⇒ §6.37 的 25s 形状未再现。
  `39542266851` 另有一条 ubuntu 首次红 `daemon_startup::discovered_managed_daemon_can_restart_with_auto_start_disabled`
  `FAIL [49.425s]`（该腿自带 ~50s 预算，非 nextest kill；#31 的屏幕诊断已渲染），
  同期另一手同腿 `PASS [5.354s]` ⇒ 分桶 harness 争用、单条观察、不动代码。
  本轮同样**零代码/配置改动、零 dispatch、零推送**。
- **§6.39 那一手（`39543230422` windows 腿终态，纯登记）**：A3/#40 的 runner 一手到手
  （`conpty_ctrl_c` 四红转 `PASS [12.071s]`，仍 ~12s ⇒ 走的是门里的降级契约而非跳过）；
  同一腿日志翻出更重的一条——`legacy_workspace_write_delete_…` 的完整 Diff
  `(0,false,false,false,None,false,false)` vs `(0,false,false,false,Some("outside"),true,true)`
  加 session 后 `outside`/`.git`/`.git\config` 全部 `here=[absent]`，而 `.235` 同证据显示三者存活。
  ⇒ §6.32 的"不是产品缺陷"与 #43 的"间歇"在 runner 侧改判为**"越界删除已证实、机制未证"**，
  已在原处加前向指针、不改写历史。证实它的最短路径（`.235` 上两档祖先 ACL 的 A/B）要动宿主 ACL，
  且 `windows-sandbox-rs` 不在本轮 scope 路径内 ⇒ 等指针，未自行开工。
  另两条：`auth_storage_originator::concurrent_connections_…` `TIMEOUT [60.035s]` 是本轮缓存 9 条
  读数全绿后首现的 nextest kill（零诊断，按争用分桶）；OAuth 那对同手全绿。
  本轮**零代码/配置改动、零 dispatch、零推送**。
- **§6.40 那一手（`39542266851` windows 腿终态，纯登记）**：A3 的二次确认到手
  （含门的另一手 `c7590fe24` 也 `PASS [12.738s]` ⇒ 四红转两绿，#40/#44 的 runner 判据闭合）；
  同一条 `legacy_workspace_write_delete_…` 在这手 **逐字同形再现**（Diff 同、六行 after 同、
  `outside` 依旧没有 session ACE），且两手 session 前的 dump 都显示 `outside-delete.txt` 自身带着
  **8 条 `mask=0x001301bf` / `delete-rights=[DELETE]` 的宿主祖先继承 ACE**（两手 SID 集合不同 ⇒
  runner 每 job 换账号）⇒ "间歇"这个口径作废，机制候选收窄为"那 8 条 allow 里哪个 SID 在受限令牌的
  restricting SIDs 里"，而这一步日志不打印、只能靠 D1-A 的本地 A/B 或令牌诊断。
  另两条分桶：`cli doctor::mcp_http_probe_falls_back_to_get_when_head_times_out` `FAIL 0.098s` 是
  测试只给 HEAD→GET 回退 10ms 预算（`cli/src/doctor.rs:4042-4045`，其余 18 条不同 job 的读数全绿）⇒ harness 缺陷；
  `codex-tui …dynamic_tool_requests_ignore_other_namespaces…` `FAIL + LEAK 14.026s` 是第二次观察、
  同一行同一文案、不是 kill ⇒ 归因需再加诊断，本轮不擅自扩面。
  两手的 mac 腿在登记时仍 `in_progress`。本轮**零代码/配置改动、零 dispatch、零推送**。
- **§6.41 那一手（#47 取证改造，一笔代码改动 +362/−56）**：`codex-rs/windows-sandbox-rs/src/unified_exec/tests.rs`
  把写/delete 取证改成 **`WRITE_RESTRICTED` 两张 SID 列表求交**：`SidList`（restricting / default）
  + `push_token_sid_lists` 用生产构造函数建出子令牌后从**那块令牌**读 default 列表，
  判决打印 `eff = (授予 restricting) ∩ (授予 default)`，dump 标注 ` <-- TOKEN R=…` / ` <-- TOKEN D=…`。
  **本条同时更正本节先前登记的机制说法**：先前那版（未提交）把 `user` 算进 restricting 列表的
  授予来源；这条腿 `create_token_with_caps_from(base, caps, &[])`（`token.rs:397-402`）的
  restricting 列表是 **caps + logon + everyone**，user 只在 elevated 后端进入（`token.rs:455-466`）。
  真机一手在 `.235`：**`PASS 0.314s` / `Summary 0.321s 1 passed, 282 skipped`、`--retries 0`**，
  `MARKER two-list-evidence=1 old-single-set=0`，零 `token SID note`。
  观测到的关键三点：(1) `grp#3(S-1-5-32-544)` 与 `grp#2(S-1-5-114)` 以 **deny-only** 出现，
  取证正确地不计其授予但仍计其 deny；(2) `.git` 与 `.git\config` 判为 `none`，且 default 一侧
  **确实有** DELETE（`grp#7`），是 restricting 一侧被 `ace[0] deny 0x00130156` 收回才归零
  ⇒ #41 要的"证明 enforce 而不是 argv"的正面收据，且形状比上一手更强；(3) 三个被删文件
  `here=[absent]`、父目录给出 `DELETE via R[cap#N holds DELETE via ace[0]] x D[grp#7 …]` 正例。
  上一手"身份 SID 本机无正例"的限制已作废（after 块 `TOKEN D=` 46 次）。
- **§6.42 两手 mac 腿终态（#46）**：`39542266851`/`39543230422` 的 mac 腿都 failure，
  红的是同一条 `app_server_daemon managed_daemon_restarts_from_deleted_working_directory`，
  `TIMEOUT 60.008s / 60.007s`，`FAIL` 计数 0，其余 22887 通过；失败集合是上一手的子集，无新增。
  **这两手不构成对 `42c5bb5d9` 的判决**：两个 head 都是它的祖先（该 override 未推送），
  它们重观测的是加宽前的 60s 界；判据要推送后的第三手。
- **本轮推送决定**：用户表格第 1、2 行都把缺的 datum 写成"推送后 CI 一手"，
  故本轮把 #47 取证 + #46 override + 台账一起推上 `test`，取第三手与 runner 一手。
  推送内容：`42c5bb5d9`（mac override）、`e14d73e76`、`8b6fac76d`、`d918722f2`（三笔文档）
  加上本轮的 #47 代码笔与文档笔。
- **§6.43（#45）落成两笔：代码笔 `0a99662c2`（`fix(linux-sandbox)`，+280/−26）+ 本台账笔**。一手全部来自 `.45`
  真机（`7.0.0-38-generic` / `bubblewrap 0.11.1-1ubuntu0.3`，验证树 = `1af8b5a70` + 两个
  未提交覆盖文件，md5 `0838014cab56c731ade218da6afedcd4` / `bc11224ad0b7fd10101315aef08707a5`
  两端逐字一致），提交前按"数字要复核"的门槛重新 grep 过远端日志：`rc_legs=0` +
  `Summary [0.014s] 5 tests run: 5 passed, 166 skipped`、`rc_full=0` +
  `Summary [6.437s] 237 tests run: 237 passed, 3 skipped`（基线 `232 passed, 3 skipped`
  是 §6.35 那手实测的，237 = 232 + 5 条新腿、跳过数不变）、`rc_fmt=0`、`rc_clippy=0`、
  构建前 MARKER 三条齐全、端到端 `P1/P3/P4 NOT_READABLE` + `P2 ---------- 0 soddy soddy 0`
  + `R1 SANDBOX_STARTED`/`SELF_OK` + `R2`/`R4 SANDBOX_STARTED` + `R3 READABLE`（残留）。
- **本手内在提交前抓到的两处自己的账**（①"把写坏的机制说法改回来"，②"把过宽的范围说法收窄"）：
  ①§6.43 初稿把"不搬 `--proc`"的理由写成
  "会牵动 `bwrap_wslg_tests.rs:81/257`、`linux_run_main_tests.rs:227/317` 的位置断言"。
  逐条读源码后这句不成立：这些断言全是 `windows(2)` 的**形状**断言（`["--proc","/proc"]`
  出现或不出现），搬动 `--proc` 不会让它们变红；`bwrap_wslg_tests.rs` 里根本没有第 257 行那种
  `--proc` 断言（那里是 `["--tmpfs","/proc"]`），`linux_run_main_tests.rs` 的行号是 228/317。
  真正的机械原因已改写进 §6.43：根绑定本身就在 `args.extend(filesystem_args)` 那批里
  （受限策略 `create_filesystem_args`（`:504`）在 `:605-607` 发 `--ro-bind / /`，全写策略
  `create_bwrap_flags_full_filesystem`（`:350`）在 `:354-356` 发 `--bind / /`），`/newroot/proc`
  这个挂载点是根绑定带进来的 ⇒ 把 `--proc` 前移到 `:430` 之前就是在尚不存在的目的地上挂
  procfs，正落在 X1 已证的"响亮 abort"那一类。顺带修掉两处行号（`:428`→`:430`、
  `:457-461`→`:457-460`）。
  ②初稿那句"**范围只到 deny**"同样过宽：`mask_sink` 挂在两个叶子 helper
  （`append_empty_file_bind_data_args`、`append_empty_directory_args`）上，
  `append_read_only_subpath_args` 的瞬时空元数据分支（`:1364/:1367`）走的是同一对 helper，
  所以严格讲"凡是目的地在新 procfs 上的掩码都可能被搬"。已按代码实际行为改写，并把
  `exists()` 判的是**宿主** procfs 这一残留风险面写进 §6.43：宿主上由别的挂载带进 `/proc`
  的条目（`binfmt_misc`、vendor 的 `/proc/driver/...`）在新树里不存在，这类 deny 规则会从
  "静默空操作"变成"响亮 abort"，而 `.45` 上没有现成的 binfmt_misc 挂载、造它要动宿主
  ⇒ **这一面本轮没有一手**，如实登记，不含糊过去。
- **本轮（#45）的推送决定**：两笔先落本地，**等 `39562896528` 到终态再 push**。理由是这一手
  正在给 #46/#47 出判据（head `1af8b5a70` 已含 `42c5bb5d9` 的 mac override，就是"推送后的
  第三手"；windows 腿带 #47 的两张 SID 列表取证），登记时三腿仍 `in_progress`；把 #45 推上去
  会开出第二手并发抢同一批 runner，而本台账已经两次把贴线 TIMEOUT 归到争用（§6.36 与
  "the four local reds are contention"）。#45 的 ubuntu lane 收据因此在**下一手**取：
  本手 tip 是**不带** `[skip ci]` 的文档笔，push 本身就足以触发一轮。本轮**零 dispatch**。
- **§6.43 里那条没修的残留，登记为待拍板而不是待办**：`/proc/self/...` 的 deny 在修复后仍
  `READABLE`（R3），strace 一手证明分流已生效、开放来自 `/proc/self` 的内核自指语义。
  修法要在"自指 deny 路径报错"与"按 pid 通配展开"之间选一个策略语义 ⇒ 属产品决策，
  不在"已确认缺陷 + 验证缺口"范围内，本轮不擅自扩面。


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





