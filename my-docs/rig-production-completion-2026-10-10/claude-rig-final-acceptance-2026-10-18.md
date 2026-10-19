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
- **macos（首次完整观测，无基线可比）**：22877 run / 22868 passed（28 slow）/ **3 failed**
  / **6 timed out** / 71 skipped。3 failed =
  `suite::v2::guardian_v2::stateful_async::classifier_modes_preserve_approval_matrix::failed`、
  `suite::v2::multi_agent_v2_developer_instructions::cold_resume_preserves_effective_developer_instructions_for_worker::inherits_parent_developer_instructions_without_an_override`、
  `codex-tui app::tests::session_lifecycle_requests::startup_defaults_tests::fresh_startup_falls_back_only_for_unsupported_config_read`。
  6 timed out = `external_auth…::discovery` + `codex-cli::app_server_daemon` ×5
  （60s/120s/180s 三档界限都出现）。因为 mac 之前四轮全被取消，**这 9 项只能登记为
  "首次观测到"，不得声称是抖动、也不得声称是本轮引入**；唯一有旁证的是前两条落在
  §6.5 已登记过的 app-server/guardian 抖动区域。
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
  以及文档提交 `a3e5da2f0`、`4ee470d5c`、`5262eb5e6`、`10b30ecd4`、`c7ece7dcc`、
  `b636829b5`（均带 `[skip ci]`，实测未触发 CI）。`origin/test` = `7b1a92c6a`，
  本节这份文档提交在其后、同样带 `[skip ci]`，不额外占用 CI 预算。工作树 clean。
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





