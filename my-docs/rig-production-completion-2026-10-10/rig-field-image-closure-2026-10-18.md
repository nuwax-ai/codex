# rig 字段与图片收尾核对（2026-10-18）

输入基线：`cb2828ebd`。用户目标仍为环境变量选择自定义模型，经 rig 的 Chat / Responses / Anthropic 协议在 Linux/容器及 ACP 中使用。

## 当前判断

主流程已能日常使用，但“跑过工作流”不等于“全部字段保真验收完成”。还需一轮有结束条件的字段处置与最终版本验收；只开发有证据的丢失或兼容缺陷，不扩大成支持所有模型厂商的全部私有 API。

本次再次核对确认三处需要修复的保真问题：

1. MCP 的非空 `structuredContent` 会遮蔽同一结果里的 typed 图片/音频。修复保留结构化 JSON 文本和原顺序媒体，保留纯文本 fallback 优先及加密内容的既有隐私合同。此行为继承自上游，并非 rig 特有。
2. raw 工具声明转换为 `Value` 失败时，debug 构建 panic，release 构建清空整套工具；130 层合法 schema 可触发。改为 `InvalidRequest`，不能悄悄用空工具表继续。
3. raw schema 中超出可表示整数范围的值可能变为近似浮点数。应在转换前明确拒绝这类整数，不在全 workspace 开启 `arbitrary_precision`，也不伪装成保真转发。

## 字段保真的准确边界

| 范围 | 当前处置和后续要求 |
| --- | --- |
| Responses 请求 | 沿同协议经 rig HTTP 传输，clone typed request 后序列化，保留 raw tools；只做已声明的跨来源历史投影、输出上限及公共传输处理。不是转成 Chat。 |
| Chat / Anthropic 常规消息、function/MCP 工具 | 映射到 rig 的类型；消息顺序、工具名称/namespace、call/result ID、参数、结果、图片、strict 等已有转换与 wire 合同。只应补缺失断言，不能用 mock 生成了回答替代最终出站验证。 |
| 工具schema数字 | 正常i64/u64整数保留，raw超范围整数明确拒绝；小数/指数沿`serde_json::Value`浮点语义，不承诺任意精度数值逐字节保真。字符串中的数字不会被误判。 |
| 结构化输出与推理控制 | Chat 保留 schema/name/strict、effort 等；Anthropic 按其协议映射 schema、thinking/effort 与并行工具控制。协议不能表达的字段要列为限制，不能偷偷换语义。 |
| OpenAI 专属控制 | Chat/Messages 不转发 `reasoning.summary/context`、`include`、`client_metadata`、`access_programs` 等；Anthropic 不直接传 OpenAI `store`、`prompt_cache_key`、`verbosity` 及输出 schema 的 name/strict。不能称为全字段原样无损，也不应把内部控制乱发给第三方。 |
| 推理历史与签名 | 版本化 envelope 保存同来源专属块，端点/模型/授权域改变后不回放；这是有意隔离，不是跨厂商共享密文或签名。 |
| usage / cap / 错误 / 取消 | 已有缺失、部分、显式零、缓存、输出触顶、迟到 usage、终态及取消合同；最终核对不能只比较累计总数。 |
| 引用、厂商扩展和输出多模态 | Anthropic hosted 搜索引用块可保留在同来源 replay envelope，但未全部投影成通用 `OutputText.annotations`；logprobs、未知事件、模型生成图片等不承诺完整适配。普通工具/输入截图支持不能扩大成“所有输出多模态均已支持”。 |
| 文件 ID、音频、PDF | Chat/Messages 不解析 provider-scoped 图片 ID，音频输入/工具结果明确拒绝；Responses 可携带原生图片 file_id。Rig 的 Document/FileId 能力不代表 fork 已接入通用 PDF 或 Files 上传生命周期。 |

源码入口：`codex-rs/codex-rust-rig-bridge/src/{convert_request,request_messages,request_tools,request_content,convert_response,responses,reasoning,hosted_capture}.rs`。2026-09 的旧字段表不能直接作为 rig 0.44 当前收口证据。

## 截图与图片实际怎样传

MCP `image` / Code Mode `image()` → typed `InputImage` → 统一图片准备与工具输出预算 → rig → 对应协议图片块。

| 协议 | 实际请求字段 |
| --- | --- |
| Chat | `messages[].content[] = {type:"image_url", image_url:{url:"data:image/...;base64,...", detail:...}}` |
| Responses | `input[].content[] = {type:"input_image", image_url:"data:image/...;base64,...", detail:...}`；原生文件引用使用 `file_id` |
| Anthropic | `messages[].content[] = {type:"image", source:{type:"base64", media_type:"image/png", data:"..."}}`；工具图片可位于 `tool_result.content` |

Chat 的工具结果图片放到紧邻的 user 图片块，保留关联工具 call_id；不能塞进只接受文本的 Chat tool content。默认 `InlineAttachmentStore` 返回内联数据，没有统一调用国内厂商的 Files API。AttachmentStore 的 `upload()` 命名不等于已经做了远端上传。

Base64 是图片的传输编码；typed 图片块里的编码不按普通文本逐字符计入本地上下文估计。图片仍计视觉 token，仍占请求字节。把 Base64 放进 MCP text/任意 JSON 字符串，仍属于文本，不能自动享有这项图片处理。

当前防线：

- 单个含图工具结果最多 8 图、单图编码引用 16 MiB、合计 24 MiB；图文共享约 10,000 token 的可见预算，包含 256 token 控制余量。实际保留数量还受视觉预算限制，并非保证能发满 8 图。优先保留较新图片，被省略时给出标记。
- 图片解码前检查 40MP；普通准备有最长边和 patch 限制，original 同样有上限。不能称为所有原图像素永远原封不动保留。
- 最终完整请求另有 32 MiB 字节上限，并在设置的模型窗口内为输出留预算。typed 图片从文本估算扣除后重新计视觉成本，超限在发网前拒绝。
- 视觉成本是跨模型近似估计，不是各厂商精确 tokenizer；自定义模型的上下文窗口和输出上限必须符合真实能力，不能沿用不匹配的默认模型数据。已有 `CODEX_MODEL_CONTEXT_WINDOW` 会传到最终预算；未知型号未配置时仍有 272000/95% 的 fallback，这不代表探测到了厂商真实窗口。
- Chat 的 `original` 转为 `high`，Anthropic 没有该 detail 字段；不能声称三协议精细度完全相同。冷恢复/fork 有图片和工具配对合同；本地 compact 有意用摘要替换保留历史中的图片，durable rollout 仍保持追加，之后看细节需重新读取图片。

Files API 上传一次再引用，可以节省重复编码传输，但不会使图片的视觉上下文成本变成零。国内兼容网关是否支持该 API、file_id 有效期/授权域、恢复后可用性都需逐家验证。通用上传/PDF是按需功能，不作为截图主路径的收工前置条件。

## 有限的剩余收尾

1. **当前版本逐字段处置清单。** 以 Codex 实际产生的请求/响应字段为全集，标记保留、转换、明确限制或拒绝，关联源码和已有测试。重点核对工具、签名、结构化输出、图片、usage presence、cap/finish/error；补真正缺的 HTTP 断言，并修复实证丢失。不要把上游所有 API 参数都变成本项目的待开发任务。
2. **原图在协议转换后预算不得降低。** 转换前的 Original 可能按尺寸估算5120 tokens，转成Chat high或Messages无detail后，wire层统一估成1844。工具入口/历史仍有较高估计，guardian也在转换前严格检查；但普通会话不具备完全相同的完整请求预检。应把转换前规范视觉估计与转换后估计取较大值，补Chat/Messages小窗口、用户图和工具图的出站零请求回归。这是已确认的内部估算降档与合理加固，不冒称已经复现厂商实际上下文溢出，也不扩展为全厂商tokenizer项目。
3. **最终源码/二进制三协议最小实网验收。** MiMo Chat 优先复验本轮通用 Chat 格式修复，再选明确支持的 Responses 和 Anthropic 模型各一个；工具、图片、压缩后新指令、恢复最小闭环。已完成 GLM Chat 的 12 步不能冒充最终树所有厂商已重跑。共享有界调用预算，记录首次失败，不靠无限重试转绿，不为未提供的协议造端点。
4. **最终提交的 CI 复跑。** 推送后关闭本轮相关失败，保留旧问题的独立证据。旧 run `39098893052` 属于 `fee6cda7d`，不能代表后续修复树。Windows 若作为正式日常路径，其专属尾项需另行通过；Linux 主路径不能因此被无限拖延。

完成条件：字段表无未裁决的用户可见丢失、原图跨协议预算下界合同通过、最终树三协议最小真实闭环通过、本轮相关 CI 失败关闭。达到后结束这一轮开发，后续只按具体模型/使用复现安排维护。

## 本次修复验证

代码提交：`9aa9579fb`（MCP结构化结果保留typed媒体）、`0d6d3cbd2`（工具定义失败与整数精度防线、跨协议最终HTTP回归）。均在本地`test`，尚未push。

Linux验证机的6个变更Rust源/测试文件与本地镜像对应。验证：

- protocol + bridge完整测试目标，包含wire集成：**692/692，0跳过，retries=0**。
- 两crate的all-targets Clippy：`-D warnings`通过；变更文件rustfmt及`git diff --check`通过。
- 新MCP回归在旧实现2过/2败，修复后与既有纯文本/加密合同8/8；工具转换旧实现5项中3败（深层、错误形状、超范围整数），修复后mapping模块25/25。最终另用真实localhost HTTP捕获验证同一MCP结构化JSON和图片在三协议同时出现。
- 首次扩大到wire目标时发现旧测试仍期待Chat纯文本数组，按`9b51c6289`已验证的字符串合同更新完整消息断言；保留角色、内容、URL、headers、effort及request-id检查。旧首败日志保留，随后全套692通过。
- 新测试的API适配/Clippy问题在本轮修正，没有把编译失败冒充旧产品行为首败。Linux rustup组件同步曾失败，改用已安装的1.95工具链与配对Clippy，不修改源码锁定版本。

最终命令：

```bash
cargo nextest run -p codex-protocol -p codex-rust-rig-bridge --offline --retries 0 --no-fail-fast --test-threads 4
cargo clippy -p codex-protocol -p codex-rust-rig-bridge --all-targets --offline -- -D warnings
```

日志位于Linux验机`/tmp/codex-review-oct18-fidelity-final.log`、`/tmp/codex-review-oct18-fidelity-clippy-final.log`；旧首败为`/tmp/codex-review-oct18-structured-media-red.log`、`/tmp/codex-review-oct18-raw-tools-integer-red.log`、`/tmp/codex-review-oct18-fidelity-full.log`。

本次没有额外真实API调用，不重跑全workspace，也不把上轮二进制/真实API收据改标为本轮新源码验收。原图跨协议预算下界仍按上节第2项保留为下一轮合理加固。
