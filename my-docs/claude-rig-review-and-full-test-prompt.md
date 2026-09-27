# Claude：Rig 协议桥独立复审与完整测试

以下正文可直接作为任务提示词。运行时先记录当前 HEAD；如果用户给了目标 SHA，以该 SHA 为审查目标。

---

你正在审查 `/Users/soddy/Documents/git-rust-work/fork-codex`，这是官方 Codex 的 fork。
请独立检查实现与测试，发现确定问题就直接修复并补回归，完成本地完整测试验证。不要把上一位 agent 的结论当作验收结果。

## 1. 范围与授权

- 遵守根目录及子目录的 AGENTS.md。先记录 branch、HEAD、工作树状态，保留他人改动。
- 本批修复基线为 `5443a749535ff9104cc6d68f688ee9e6cd926778`。审查 `基线..目标 SHA` 的全部提交及其调用方，不能只看最后一个文档提交。再沿当前 fork 的请求入口核对整体路由。
- 允许直接修复确定的代码/测试缺陷、添加必要回归，并运行下面的完整本地 workspace 测试，无需再次询问全套本地测试许可。
- 允许使用现有测试配置做一轮有界的真实供应商验收；与本地测试分开执行和报告。缺凭据、endpoint 或模型能力时明确登记未验证，不伪造配置、不将提前返回算通过。
- 不自动 commit、amend、push、发布或重录历史 fixtures；完成后交付 diff、审查报告和测试证据。

## 2. 先读这些材料，再核对源码

1. `my-docs/rig-protocol-audit-2026-09-27.md`：字段差异、R01–R29、剩余能力缺口、§7.2 上一轮验证。
2. `my-docs/codex-review-prompt.md`：整体 fork 的原始审查重点；其中旧命令和历史结论应以当前源码、AGENTS.md 和新审计为准。
3. `my-docs/field-mapping-audit.md`、`my-docs/responses-anthropic-field-diff.md`。
4. `codex-rs/codex-rust-rig-bridge/`、`codex-api/src/bridge.rs`、`core/src/client.rs`、`live-tests/` 及相关测试配置。

实际 SDK 以 Cargo.lock 和 registry 中锁定的 `rig-core 0.42.0` 源码为准，不以另一个 Rig 仓库的最新 HEAD 代替。查看 OpenAI Responses、Chat Completions 和 Anthropic Messages 的最新官方文档，报告官方链接、核实日期及型号条件。

必须保留的边界：

- `ResponsesApiRequest` 是 Codex 内部输入；当前 Rig 实际发送 Chat Completions 或 Anthropic Messages，没有真正的 Rig Responses wire。
- 第三方 Responses 配置默认经 Rig 转 Chat；官方 OpenAI、Bedrock 和显式 native 保留原生路由。核对辅助模型请求路径，不能声称所有模型 HTTP 都走 Rig。
- GenAI 是显式备用选项，本次不扩大其功能；CLI/exec/app-server 同时编译两桥带来的 feature 组合仍要检查。
- 现有固定 max_tokens、按型号 thinking/effort、prompt caching、developer 角色、部分多模态与引用信息是已登记缺口。区分新缺陷、已知限制与需要另行设计的能力，不得将“已登记”当作正确性的豁免。
- B0–B6、promotion-full45.log、b6bae4ca 等其他项目材料不能充当这个 fork 的证据。

## 3. 重点审查

### 请求、历史和最终 HTTP

- 对照真实 HTTP JSON 核对顶层字段、tool_choice、parallel_tool_calls、strict、service_tier、store、prompt_cache_key、reasoning 和 schema；不能只断言 Rig 中间结构。
- Anthropic schema 不应经 SDK sanitizer 改写语义；output_config.format 与 effort 合并不得相互覆盖。区分 HTTP 接受、可解析 JSON、真正符合 schema 三种证据。
- thinking/signature/redacted 多块完整回传；来源切换时的隔离。envelope 是 JSON 编码，不是加密；当前来源标识不绑定认证身份，不能夸大保护范围。
- function/custom/namespace 工具 ID 和参数关联；损坏 JSON、空 ID、外部无 ID 事件、工具结果 is_error，以及多轮完整历史。
- 工具文本共享预算、边界遗漏标记、图像 detail/MIME/URL、拒绝不支持内容；对大 envelope、大图片、超长流和工具参数的内存/上下文边界逐项记录。
- URL 只按 path 识别协议；query、user-info、fragment 不误触发。认证、自定义头、anthropic-version、URL 查询参数和错误日志脱敏均需核对。

### 响应、流和执行时序

- Added → delta → Done 生命周期，text/reasoning 来回切换、并行工具顺序、response/message ID、唯一 Completed。
- Chat `[DONE]` 和 Anthropic `message_stop` 才是相应的终止门禁。检查分片、CRLF、同 chunk 多事件、终止后连接保持、缺失终止、EOF、中途错误、timeout/cancel。
- 在完整成功终止之前，不能发出会触发工具执行的 Done；length/max_tokens/content_filter/refusal/未知 stop reason 等失败也不能提前执行工具。至少核对 function 与 custom 两条消费链。
- Anthropic usage 的 message_start/message_delta 累计字段、缓存读写、thinking，缺失/null/显式 0 区别；不重复累加，录制前完成补正，回放与在线转换一致。
- 401/403/429/5xx 和读取错误保留状态/必要头；不能将带凭据的请求 URL 格式化进错误。
- 检查真实 Codex core/exec 消费方，桥事件测试不能替代实际工具进程闭环。

### 测试与兼容性

- 配置文件优先级、无全局 env mutation、无凭据回放、fixture 写失败传播、首个 Completed 后停止、完整首轮历史进入第二轮。
- 检查旧 rollout、v1 envelope、旧 cassette 的兼容性；历史 fixture 没有记录的信息不能靠回放恢复。
- 无桥 feature 必须发网前失败；分别测试默认、Rig、workspace feature 合并。
- 不降低断言、扩大白名单、把失败改 skip 或靠重试制造绿灯。历史 parallel-tools fixture 未必有多个工具，必须另有确定性多工具证据。
- 保留 SOLID/Fail Fast、错误上下文和最小 API 面；检查生产 unwrap/expect/unsafe、锁跨 await、取消与任务回收、模块大小、Bazel 源文件收集与外部配置兼容性。

## 4. 测试计划：分阶段执行并保留真实退出码

全部 Rust 测试用 `just test`，不要直接运行 `cargo test`。以下工作目录均为 `codex-rs`；构建命令可以使用 `cargo build`。每条命令单独留日志；使用管道时正确保留 nextest 退出码。

### A. 本地定向回归与 feature 组合

```sh
just test -p codex-rust-rig-bridge --offline --retries 0
just test -p codex-api --offline --retries 0
just test -p codex-core --features rust-rig --offline --retries 0
just test -p codex-core --lib --no-default-features --offline --retries 0 -E 'test(missing_bridge_features_reject_before_native_responses_dispatch)'
```

core/Rig 要运行完整该 crate，不仅是此前的 client::tests 过滤集；根据实际缺口补充默认/两桥组合。检查 test list 和 cfg，避免功能没有被编译却误判通过。

### B. 完整本地 workspace

```sh
just test --workspace --exclude codex-live-tests --offline --retries 0
```

这是授权运行的完整本地 workspace 范围，真实供应商 crate 在 C/D 独立覆盖。不要默认加 `--all-features`。`--offline` 只禁止 Cargo 下载，不限制测试运行时联网；仍要核对各测试自身的环境和访问目标。

若依赖未缓存，记录失败原因，按仓库要求补齐构建依赖后再跑；不能用“成功编译”代替测试。workspace feature 合并会开启两桥，不能代替 A 的单包无桥测试。记录本平台跳过的 OS/远程执行用例、doctest 或其他未覆盖目标，不将 nextest 结果扩大成全平台证明。

### C. 独立历史回放与测试基础设施

```sh
LIVE_CASSETTE=replay LIVE_VENDORS=mimo,glm,step LIVE_INCLUDE_GENAI=0 just test -p codex-live-tests --offline --lib --test bridge_live --retries 0
```

不要把 `LIVE_CASSETTE=replay` 套在无排除的 workspace 上：`exec_live` 明确不支持 replay。
比较 fixtures 执行前后状态，不能覆盖历史录制。分别统计实质 Rig 回放、基础设施回归、禁用 GenAI/auth/AB 提前返回；回放不能证明真实网络请求或当前供应商行为。

### D. 真实供应商与当前二进制闭环

先检查已有测试配置是否具备对应凭据、模型和 endpoint，只报告是否配置，避免输出值。测试会读取 `.env`/`.env.local`，仅清空进程变量并不能保证没有真实调用。

先构建当前 `codex-exec`，确认 `codex_exec_binary()` 最终使用的确是本次产物。若使用隔离的 `CARGO_TARGET_DIR`，必须核对 helper 的路径解析，不能默默运行旧二进制。

```sh
cargo build -p codex-exec --bin codex-exec --offline
```

按已配置厂商逐个运行；每厂商一轮、串行、`--retries 0`，保留限流/网络/能力拒绝的原始结果，不用重复付费请求刷通过。仅选 Rig bridge 场景，以及 exec 的 chat_rig、chat_default、anthropic_rig、responses_rig_default；对真正有 Responses endpoint 的厂商另跑 responses_native 控制组。

先按当前源码和 nextest test list 确认过滤表达式。真实阶段显式 `LIVE_CASSETTE=off`、`LIVE_INCLUDE_GENAI=0`，使用 `just test -p codex-live-tests --test bridge_live` 或 `--test exec_live` 加精确筛选和单线程参数；不默认 `record`。检查 schema、effort 与工具实际行为，不能只凭 HTTP 200 或模型自述。

验收矩阵应区分：Chat/Messages 桥、第三方 responses 配置转 Chat、真正 native Responses、真实 shell 进程执行/回传/最终结束。exec 的命令执行事件、exit code 和随机 marker 共同证明闭环。缺失/不支持的矩阵格子明确标为未验证或不兼容。

### E. 修复后的收尾

如有修复，先复现、再定向回归和必要的完整重跑；保留修复前失败日志。最后按 AGENTS.md 执行作用域适当的 `just fix -p ...` 与 `just fmt`，再检查最终 diff。按仓库规则，最终 fix/fmt 后不重跑测试；若它们实际改动逻辑，返回修复验证流程后重新收尾，不将旧测试当作新逻辑的证据。

不要把基线已有的无关 GenAI/config 格式变化混入修复。依赖、配置 schema、app-server API 或编译期资源若改变，执行各自要求的生成/锁文件更新。不要杀死正在等待 Rust 锁的进程。

## 5. 交付要求

将报告保存到 `my-docs/claude-rig-full-validation.md`，包括：

1. 审查 base/target SHA、最终工作树状态、SDK 版本和平台。
2. 编号问题：严重度、源码文件与行号、触发条件、影响、修复和回归证据；无新增问题也要说明审查范围。
3. 更新后的字段差异：等价、近似、明确拒绝、未实现、型号/网关条件，以及官方来源。不得宣称三协议全字段无损。
4. 测试矩阵：完整命令、退出码、日志路径、实际 binaries、passed/failed/skipped/filtered、early-return、重试/flaky；环境失败与代码失败分别说明。
5. 本地 workspace、HTTP mock、历史回放、真实厂商、真实工具进程、其他 OS 分别下结论；列明尚未验的项目与下一步。
6. 实际改动清单、剩余风险、是否具备当前 Chat/Messages 核心路径合入条件。

上一轮 §7.2 的 81 个 Rig、200 个 API、32+1 个 core，以及 runner 62（实质 24 回放 + 7 回归）只作历史基线；必须以本次执行产生的新日志下结论。
