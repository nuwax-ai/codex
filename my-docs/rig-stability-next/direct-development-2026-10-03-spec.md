# 2026-10-03 直接开发规范

基线：`20898140f2b3638aac6ab328b24d2f604871bfc8`（test）。保留 Claude 未提交改动；本轮由 Codex 直接开发，不再生成 Claude 任务交接。

## 必须满足的行为

1. Anthropic hosted replay 使用持久化响应和片段身份；相同正文不能充当归属证明。暂停、普通工具、thinking、重复正文、resume/fork 不改变次序。旧 v1/v2 仅回放可验证调用对，不提升成有归属的引用。
2. 损坏、过滤、去重和预算处理之后，每个接受的调用块只能出现一次；完整布局必须覆盖其实名片段和接受调用，丢失载体不得吞掉合法兄弟调用或重复插入旧正文。
3. 历史来源在实际请求的 client setup 时绑定；跨 provider config key/model/wire/bridge/endpoint/authorization domain 只移除不兼容 opaque 字段，保留可见文本和合法工具历史，持久化数据不得改写。
4. 构建收据使用源码内容摘要和完整 Git SHA，区分包级 features 与未知依赖 feature 图。自报告不等于认证供应链证明；使用收据检验过的同一个 executable。
5. 最终请求记录覆盖每个 HTTP attempt，含 pause 后续请求和 Responses；`body_raw` 保留序列化字节，parsed body 仅用于方便检查。无 headers，URL userinfo 和 query values 脱敏。正文可能包含敏感 prompt/tool 内容，不宣称天然无秘密。
6. exec marker/compact/websearch 场景在所有错误分支保留 partial rollout、原始输出和主要错误，包括 exit 0 后断言失败及捕获 I/O 失败。
7. NUWAX 正向注入和负向隔离使用实际公共 API 验证；不能通过全局关闭 remote control 改变其他测试语义。
8. GLM `web_search_prime` 采用与生产桥一致的搜索工具家族识别，input 仍须 object，调用/结果 ID 严格匹配；不把合法 string result 转成伪造网页结果。
9. 收据完整文件 SHA 校验保留，拒绝非 regular binary 路径；已完成 pipes/exit 必须在后置校验前落盘。双轮 scene 的外层 watchdog 必须覆盖两个内层 300s timeout 和 drain/identity 开销。
10. 子进程每个 stdout/stderr 捕获最多 16 MiB，超限保留允许前缀并明确失败，主动停止子进程，不等待 300s deadline 后假称网络超时。
11. 三协议无 primary credential 时不得附加 SDK 合成的空认证头。显式空 key 与缺省 absence 区分，Basic/Token gateway Authorization 原样保留；不能转换 SDK key 字符串的实际 key 明确 InvalidRequest。

## 预算与证据边界

- 单个 hosted envelope/layout 9,800 serialized bytes；独立最多 64 layouts、64 KiB layout contribution，调用对另有条数限制。超限 whole drop，签名/密文不得截断。
- pause 原始续接使用独立预算：每条序列化 assistant message 最多 40,960 bytes，最多 4 次内部 continuation；这是恢复既有上限，不随持久化 envelope/layout 上限收紧。超限明确失败，签名内容不得截断。40,960/4=10,240 只是字节估算，不能证明未知 tokenizer 下满足 10K-token 单项要求；本项保留为 **P0 人工上下文预算复审项**，后续须单独设计 token-aware 限制及其行为兼容策略。
- 最终发送前检查整个请求（含工具、replay、pause）的 32 MiB 硬边界；已知 usable context window 时使用仓库现有 bytes/4 估算并预留请求输出预算。未知厂商 tokenizer 时不宣称精确 token 上限；图片、JSON 开销会影响估算。
- 新的模型可见 raw fragments 可超过 1K tokens，按 AGENTS 记为 **P0 人工上下文复审项**。局部字节上限不等于所有模型真实 tokenizer 验收。
- 请求捕获：2 MiB/body、64 attempts、16 MiB aggregate。捕获是显式诊断开关 `CODEX_RIG_REQUEST_CAPTURE_FILE`，新文件 Unix 0600，写入失败或超限明确失败。记录发送前的最终 attempt 不等于厂商已经收到。
- 基础 websearch 成功要求匹配 completed pairs 和最终回答；引用能力另记，不要求合法未引用回答伪造 citation。
- 本轮仅运行相关模块和定向集成/真实厂商验证；完整 workspace、跨平台、远程 CI、发布验收单列。未执行不得写为通过。

## 兼容与可观察变化

- 新 source 使用 endpoint/model/auth-domain/proof-kind。旧算法捕获的 payload 通常因 source 不匹配整体降级；能解析 v1/v2 不等于保留其 opaque 内容。磁盘原记录保持原样，下一次请求使用投影副本。
- `rawResponseItem` 可观察到 hosted `wire_blocks.version = 3` 和新 `rigseg_` item ID。ID 是 opaque 字符串；Added/Delta/Done 保持引用一致，工具 `call_id` 保持原值。不保证 ID 前缀、UUID 形态或 item ID 等于 call ID。嵌入调用方需按 envelope version 解析。
- Rust 嵌入源码兼容边界：`ModelBridgeOptions` 新增 auth-domain/kind/context-window 三个字段；`ModelOutputProvenance` 新增 endpoint/auth-domain/kind。外部完整 struct literal 需补这些字段（不能把可选 serde 字段等同 Rust struct literal 兼容）。历史 JSON 缺字段仍可读取；AuthProvider 新 snapshot 方法默认 None，不要求旧实现实现它。六参 loader 已恢复兼容 wrapper。
- 旧公开无 auth-domain 的 free functions 保留 `legacy-unscoped` 行为，仅是源码兼容入口，不提供跨账户证明；Core ModelBridge 使用实际 source metadata 和保守投影。
- selector 只能证明配置选择器一致；签名、密文、加密 citation index 在没有 account/anonymous 或实际凭据实例证明时不回放。Chat 保留可见 `reasoning_content`，Anthropic 不将无签名文字伪装成 thinking。
- 为避免正常 API Key 请求被全量降级，增加 `credentialInstance`：只有承诺不可变凭据快照的具体 AuthProvider 可使用；默认 None，不用 telemetry snapshot 猜测。实际 headers 和完整 endpoint/query scope 在私有内存缓存中比较，不 hash、serialize 或 Debug 密钥，持久化仅随机实例 ID。最多 64 entries，单 entry 合计 16 KiB。同进程同凭据继续完整回放；轮换、淘汰或重启获得新 ID，旧 opaque 数据保守降级。无稳定 snapshot 的动态签名 provider 继续 unknown。
- 收据 subprocess 超时后，持有 pipe 的 descendant 无法用跨平台 safe std 强制取消；全局最多 16 pipe readers，耗尽后在 spawn 前明确失败，不会无限增加线程。

## 官方字段核对

依据 [Anthropic Messages API](https://platform.claude.com/docs/en/api/typescript/messages) 与 [web search 协议](https://platform.claude.com/docs/en/agents-and-tools/tool-use/web-search-tool)：

| 载荷 | 必须区分的字段 | 本轮处理 |
|---|---|---|
| 用户提供的搜索引用 `search_result_location` | `source`、搜索结果序号和内容块范围 | 合法 schema 验证，不能添伪造来源补缺字段 |
| 服务端网页搜索引用 `web_search_result_location` | `url`、`title`、`encrypted_index`、`cited_text` | 保留原始字段；来源不兼容时不重放 opaque index |
| 搜索结果 | `encrypted_content` | 同一绑定来源原样回放；不截断、不重新编码密文 |
| SSE | 外层 `event.index` 与 citation 内部范围含义不同 | 拆分 mock 响应只重编号外层；不能改引用内部索引 |

`encrypted_content` 与 `encrypted_index` 是多轮引用回放数据；普通文字回答成功不能证明这些字段已经保真。

## 审查发现登记

| 类别 | 本轮确认的问题 | 处理 |
|---|---|---|
| Context | concat-text 定位错响应；pair-only 串到尾部；carrier 丢失误继承；dedupe 后 layout 无界；重复/遗漏 indices；跨 thinking/tool 边界 | v3 response/segment IDs、全覆盖校验、独立预算、原历史边界 |
| Breaking | 原 loader 六参 API 未恢复；非 UTF-8 argv panic；receipt 只识别首参；pause recorder 丢失 | 兼容 wrapper、args_os、严格独立命令、recorders 全程传递 |
| Testing | GLM validator 硬编码 v1；全局 remote disable；只保留 spawn 错误；Bazel cargo-bin env；无 receipt 校验矩阵；只比 parsed body | 修复 harness、局部环境、完整 scenario wrapper、cargo_bin、内容校验矩阵、原始字节比较 |
| Size | Claude 累计超过 3K 改动行，不是五个已独立构建批次；新增 identity 单模块已超过 500 行 | 私有模块拆分；后续提交按真实依赖分批，未经各批构建不宣称可独立落地 |

此前 `/tmp/final-bridge-live.log` 为 338 run / 337 pass / 1 fail。唯一失败的 `invalid search replay envelope` 来自 v1-only 校验，不足以证明 GLM 第二轮无回答。已检查对应保留 rollout 有回答和 completed searches；运行时提前返回与 nextest PASS 分开计数。其余 Claude 声称的新结果在未找到命令和日志前仅视为报告，不转成独立复验证据。
