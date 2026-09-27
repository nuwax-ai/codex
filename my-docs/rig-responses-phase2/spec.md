# Rig Responses 第二阶段 — Spec（规范）

日期：2026-09-28。当前范围是 **Rig Chat/Anthropic replay envelope → Responses 的有限投影**；通用旧会话跨协议兼容尚未完成。验证记录见 tasks.md。

## 1. 目标与范围

发送 Responses 请求时，仅在请求副本中清除本桥 reasoning replay envelope。原始请求对象和持久化 rollout 保持不变；消息顺序、工具调用与结果配对、已有身份信息不改写。

| 输入 | Responses 出站行为 |
|---|---|
| Reasoning.encrypted_content 以 `codex-rig-reasoning-v1:` 开头 | 置为 `None`，序列化为 `null`；保留同项其余字段 |
| 非 envelope 密文 | 保留，不推断来源或跨网关可移植性 |
| 其他历史项 | 保留，采用现有 ResponsesApiRequest 序列化规则 |
| 无 envelope 的 Responses 请求 | 完整出站字节与直接序列化一致 |

## 2. 协议与序列化约束

- 实际 Rig 输出的 reasoning 是 `summary=[]`、`content.type="reasoning_text"`，并带桥生成的 item ID。
- `models.rs::should_serialize_reasoning_content` 是 **skip_serializing_if** 谓词：含 ReasoningText 的 content 会序列化；仅含 Text 的 content 被省略。此前文档将其写反。
- OpenAI 官方 SDK 从 OpenAPI 生成的 [Reasoning 输入类型](https://github.com/openai/openai-python/blob/main/src/openai/types/responses/response_reasoning_item_param.py) 包含 `content` / `reasoning_text`。厂商是否接受具体历史仍需实际请求验证，不能由类型定义推断。
- 采用类型克隆后序列化，禁止通过 `serde_json::Value` 全量往返，以保留工具 RawValue 的字节、字段顺序和大整数精度。
- envelope 前缀只能确定这是本桥封装，不能证明其中厂商签名可跨协议重放；这里只移除封装，不伪造加密信息。

## 3. 验收

1. 原始 HTTP body 全字节对照，覆盖有/无 envelope 及大整数工具 schema。
2. 真实桥输出形态进入投影；reasoning_text、item ID 和工具配对均保留。
3. 离线 Responses replay 通过本地 HTTP 服务执行真实 Rig 发送路径并断言出站请求，再解码录制的原始 SSE。
4. MiMo/GLM 使用实际桥转换产物和配对工具结果标记验证 Responses 续轮；Step 未配置 Responses endpoint 时不计为有效通过。
5. core 实际关闭后恢复，验证旧来源元数据、无来源旧记录以及原始历史未被回填或重写。

## 4. 未完成的兼容边界

- provenance 尚未进入请求投影决策；异源非 envelope 密文仍透传，服务端可能拒绝。
- 保留桥生成 ID 不等于任意厂商接受这些 ID；结论限定于已经验证的厂商和场景。
- Responses→Chat/Anthropic 的既有转换会忽略无同源 envelope 的公开 reasoning。新建 Chat 测试和入站 replay 不能证明反向历史兼容，本阶段不声称已解决。
- 不实现会话存储迁移、跨厂商密文翻译或完整双向会话投影。
