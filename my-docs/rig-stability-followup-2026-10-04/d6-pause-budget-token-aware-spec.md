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
