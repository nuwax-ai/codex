# Provider 请求控制规范

## 范围

补齐 Responses 输出 cap 与 Rig SSE 握手前 HTTP 重试；保持当前默认 Rig、旧会话投影、原始 tools 字节和凭据隔离。

- `NUWAX_MAX_OUTPUT_TOKENS` 对三种协议有效。Responses 使用 `max_output_tokens`，包含可见输出与 reasoning token；参照 [OpenAI 官方文档](https://developers.openai.com/api/docs/guides/reasoning)。
- 未配置 cap 不增加 JSON 键；配置 cap 不改写会话历史。HTTP、WebSocket、Rig 请求以及预算采用同一 cap。
- 显式请求 cap 优先于 provider 默认 cap。修改 cap 时不得把 WebSocket continuation 当作属性完全相同。
- Rig 采用 provider 的 HTTP retry policy：零重试仍发送一次；遵守 status/transport 分类和 Retry-After；每次实际尝试可审计。
- Native SSE 的 budget failure 必须立即终止，不能被后续 pending、断网或 completed 覆盖。
- 底层 reqwest protocol-nack retry 关闭，HTTP/2 重发也必须受同一预算和捕获控制。
- Responses 输出耗尽为不可自动重试的预算配置失败，提示调用者提高 cap，不能重复计费式重采样。
- 重试止于成功取得 SSE response；之后流错误由既有 Core stream retry 处理，桥不重放已消费流。取消等待后不得启动后台重试。

## 边界

保留 native 的两层预算语义：一次 Core sampling 最多 HTTP retries+1 次；整个 turn 若 Core 允许 S 次重新采样，理论上最多 (S+1)*(HTTP retries+1)，不能声称二者是一个总预算。
既有握手/首事件 timeout 也可在 HTTP retry 次数耗尽前取消等待。
不额外加入 Rig SDK agent 自动重试，不改变 429 默认策略，不开发任意环境 headers/query 导入或共享 home 跨容器支持。

## 验收

类型/原始字节回归、三协议 cap 出站、HTTP/WS payload 与 reuse、真实 Exec env、Core 路由、5xx/429/transport/Retry-After/取消/成功后断流与尝试采集。仅相关模块；完整 workspace/容器矩阵另行授权。
