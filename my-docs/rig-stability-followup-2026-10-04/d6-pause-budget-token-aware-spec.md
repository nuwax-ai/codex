# D6：pause 上下文预算规范（复查修订，2026-10-05）

状态：设计提案；未实施、未获产品裁决。P0 人工预算复审保持开启。

## 现行契约

- 每条暂停 assistant message 的序列化内容上限为 40,960 bytes；超过上限在下一次 POST 前明确失败，签名不得截断。
- 最多四次内部 continuation，最终请求另受现有整体字节/窗口预算约束。每条预算、累计请求预算和模型 token 预算是不同口径。
- LegacyBytes 继续原有字节检查，不改写历史、不删块、不把失败改成静默降级。
- bytes/4 不是 10K-token 证明。例如实际 1.5–3 bytes/token 时，bytes/4 会低估 token 数；不能据此关闭 P0。

## 拟增加的行为

1. 仅在有可信 tokenizer、版本和 provider 序列化/消息 framing 依据时启用 VerifiedTokens。计量整个实际携带的 message：文本、签名/密文、工具 input JSON、搜索结果、引用及 framing 开销。
2. 使用确切计数或已证明的上界。校准分位数、平均 bytes/token、ASCII/CJK 经验表均只能作观测估计，不能充当硬上限。
3. 没有证明时保持 LegacyBytes，明确 token_limit_verified=false。未知模型不能被“保守系数”升级为已证明。
4. 新模式采用 KeepWholeOrFail：原样携带完整协议内容，或下一 POST 前明确失败。未经协议正确性与产品裁决，不能删签名/搜索块后假称仍是同一 pause continuation。
5. 计量方式、版本、bytes、exact/upper-bound token 值和判定可作有界诊断元数据；不得保存明文密钥、再注入模型上下文或重写旧 rollout。

## 非目标与产品裁决

- 不在本轮改现有 cap 或自动启用新模式。40,960-byte 允许的内容可能超过真实 10K tokens；新 token 硬上限若拒绝此前内容，就是兼容变化，必须明确裁决。
- DropOpaque/DropAll 不作为 pause 发送方案。跨协议历史投影的降级与当前 provider 要求的 pause 原始续接不能混为一谈。
- tokenizer 接入、10K 的计量单位（单 block/整条 message）、厂商隐藏 framing、错误提示和配置开关仍需确定。

## 验收要求

- 受控 exact tokenizer/证明上界的桩：ASCII 高熵串、CJK、混合/无空格、JSON、工具参数、ciphertext/signature、引用和 framing，覆盖阈值前后。
- token 限制不能仅依赖普通字节 fixture。未知模型、缺证明、计量错误不得返回 verified=true。
- 真实 HTTP 验证完整字节保持、超限零后续 POST、签名及 call/result 不被截断或丢半。
- Legacy 模式既有 20KB 正向、41KB fail-fast、pause 次数/取消回归保持；新模式行为及兼容迁移单独验收。

## Provider framing 与计量单位清单（2026-10-06 补全，待核设计输入）

候选计量对象为“一次 pause continuation 实际发送的完整 message 及其相关 framing”。下表列出需检查的请求载荷，**不是已核实的厂商计费/tokenization 契约**。传输 JSON 字节可用于现有字节检查，但字段名、逗号、转义、签名/密文是否按同一方式进入模型 token 计量仍待证明；不得直接把序列化 JSON 的本地 token 数认作真实输入 token 数。

| wire | 需检查的实际请求载荷 | 待核事实 |
|---|---|---|
| Responses | `input` items，以及实际出现的文本、opaque/引用字段和 item 间 framing | 各字段与隐藏 framing 的 token 计量、是否有适用的预发送计数来源、模型/接口版本范围 |
| Chat | `messages[]` 的 role、content/parts、tool_calls/arguments，以及序列化转义与消息边界 | 每个实际 provider 的计数接口、tokenizer 与消息 framing 契约；不能从 Chat-compatible wire 推断所有厂商都无预计数 API |
| Anthropic | `system`、messages/content blocks、thinking/signature、tool_use input，以及实际请求边界 | `count_tokens` 为待核候选；返回粒度、是否包含 system/工具/framing、是否为估计及误差/支持范围都未确认，不假定按 block 返回或需另行求和 |

计量单位裁决项（需产品确认后再实现）：
1. 候选单位为整条 continuation message，覆盖全部 blocks 和已证明的 framing；单 block、整条 message 与整次请求的预算需明确区分。若计数接口只给整请求总数，不得直接把它认作单 message 的精确值。
2. 多轮累计：同一 pause 链的每次 POST 分别计量；“累计不超过 N”是另一种口径，须单独定义。
3. opaque（签名/密文/搜索结果）的序列化字节继续进入字节预算；对应 token 成本保持 Unverified，直到计量契约覆盖它们，不能按“原文 token 数”折算。

## tokenizer / 计数来源候选清单（2026-10-06，均待核）

| 候选来源 | 适用范围待核 | 当前用途与门禁 | 需记录的依据 |
|---|---|---|---|
| Anthropic `count_tokens` API | 实际 provider/model/API 版本、请求类型与支持内容 | 仅为预发送计数候选；API 名称或厂商来源不证明 Exact/上界，保持 Unverified | 官方计量契约、是否估计及误差保证、完整请求覆盖、版本与适用域 |
| Responses `usage.input_tokens` | 实际 provider 对该字段的定义 | 事后观测/校准；不能单独用于发送前判定或证明上界 | provider/model/API 版本、字段语义、实际请求与响应 |
| Chat `usage.prompt_tokens` | 实际 Chat-compatible provider 对该字段的定义 | 同上，不由 wire 名称推断计量一致性 | 同上 |
| 本地 tokenizer（如 tiktoken 系） | 明确的模型版本、tokenizer 数据及已证明的 framing/内容范围 | pin 版本和样本一致性仅为必要验证材料；有限样本匹配不能升 Exact 或 ProvenUpperBound，保持 Unverified | 完整计量映射或覆盖适用域的上界证明，以及辅助一致性测试 |
| 经验系数（bytes/4、分位数） | 无 | 永远不是证明 | — |

升级门禁：本表候选一律 `verified=false`，保持 Unverified→LegacyBytes，P0 人工预算复审继续开启。只有审查通过的证明材料明确计量单位、provider/model/tokenizer/API 版本、全部内容及隐藏 framing 的适用域，并提供该域内的确切计数依据或已证明上界后，才可考虑 VerifiedTokens；API 名称、来源权威性或 tokenizer 样本一致性均不能替代证明。未知模型、域外输入、版本变化、缺证明或计量错误不得返回 verified=true。预算兼容变化仍需产品裁决；本轮不接入计数 API、不调用厂商、不启用新模式。
