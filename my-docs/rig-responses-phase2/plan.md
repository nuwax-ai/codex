# Rig Responses 第二阶段 — Plan（技术方案）

## 1. 请求投影

`ResponsesApiRequest::clone` → 在克隆的 input 中匹配 Reasoning.encrypted_content 的本桥前缀 → 置为 None → 一次 `serde_json::to_vec` → Rig OpenAI Client 发送 /responses。

`tools` 的 Arc<RawValue> 只克隆引用，不解析重编码。原始请求保持不可变；日志仅记录投影数量。非 envelope 密文和其他 item 不调整。

## 2. Responses 解码与修复

Rig 继续负责 HTTP 发送；使用 codex-api 的同一完整 Responses 解码管线，严格策略位于独立模块，原生策略保持原行为。严格路径保留响应头、模型/限流/ETag/reasoning 信息、事件外围 metadata，并将 failed/incomplete/顶层 error 及时终止。无配额头时不合成空配额更新。

建流等待 HTTP 响应受 idle_timeout 约束；请求 Authorization 与已解析鉴权值一致。ModelBridgeOptions 携带 turn_state，服务端状态在同 turn 后续请求回传。Responses Lite 请求体与其选用请求头保持一致。

## 3. 验证层次

- 单元：类型层投影不修改调用方请求，原始非 envelope 密文保留。
- wire：原始 HTTP body 字节比较（大整数、空白、非字母字段顺序），真实 reasoning_text 形态，认证、响应头、metadata、HTTP 响应前超时与终止错误。
- core：Lite 开关矩阵；实际工具调用后保存、关闭、恢复，检查历史、来源信息、旧记录缺字段和 turn-state 生命周期。
- live replay：本地 HTTP 服务返回原始 SSE，当前 Rig 构建并发送请求，服务端断言投影后字节；不是只调用入站解码器。
- live：从已有真实 Rig fixture 经当前转换器重建旧 Chat 输出，追加配对工具结果及标记，要求 Responses 模型返回该标记；证据区分回放与在线请求。

录制 JSON 的 request 是投影前桥输入，不将它表述为厂商实际收到的字节。完整出站保真由 wire/loopback 回放断言证明。

## 4. 后续阶段

来源元数据参与投影、异源密文规则、反向公开 reasoning 投影和更广厂商矩阵均另行设计。本期不以“ResponsesItem 协议中立”为由宣称零风险或完整双向兼容。
