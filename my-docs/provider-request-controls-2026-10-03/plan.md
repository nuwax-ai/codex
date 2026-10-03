# Provider 请求控制实现计划

## 阶段 A：输出 cap

在 codex-api 的 Responses HTTP/WS 类型加入可选 u64 cap；None 不序列化。Core request builder 采用 provider 值，WS reuse 比较 cap；所有构造器显式初始化。
Rig Responses 在类型副本中补 provider fallback，并使用相同 cap 做预算；不做 Value 往返。Chat/Anthropic 沿用 SDK 的模型映射。
开放现有 NUWAX cap 的 Responses 解析，更新原先 unsupported 回归为真实 HTTP 字段验证。

## 阶段 B：HTTP retry

独立 request_retry 模块接收最终 Request<Bytes> 和 RetryConfig，使用 codex-client 的 RetryPolicy/退避以及 codex-http-client RetryAfter。
按 attempt 克隆 Bytes、method/URI/version/headers/extensions；在每次发送前采集。只重试可识别的 HTTP status 或 reqwest 网络故障；build、预算、采集、未知 instance 错误 fail-fast。
pool 与 custom CA 共用关闭 reqwest 内部 retry 的 builder，避免 HTTP/2 GOAWAY/REFUSED_STREAM 额外发送。
成功 response 直接返回；不读、不重试 SSE body。外层 decorator 保持现有错误脱敏、request ID 和 tee 行为。

## 验证顺序

先类型、桥 wire、Core/Exec 定向验证，再 scoped fix/fmt。保留前轮未提交变更，不自动 commit/push；新增依赖需同步 Cargo 与 Bazel lock。
