# 后续稳定性规范（2026-10-04）

基线：`ead6f2bcae3e0cac4a8374708e9dc1d1d31296a6` + R1–R4 未提交修复 + Codex 本轮测试补充。先读同目录外的 `codex-review-2026-10-04.md`；实现前记录实际 HEAD、diff 和源码摘要。

## S1：请求身份不依赖凭据名称猜测（优先完成）

- URL/config query 的任意值不得进入持久化 endpoint 摘要，包括 `sessionkey`、`sig`、`hmac` 等未识别名称。正常 routing 参数变化也不能允许 opaque 数据跨 scope 回放。
- 对所有 query 采取统一保守规则：持久化 endpoint 身份不含 query 值；实际完整 URI/query 只在有界私有 credential-instance 缓存中比较。没有不可变实际凭据证据则降级 opaque，保留可见文本。
- 每轮可能改变授权域的 extra headers 必须参与身份判断。纯 tracing/telemetry 头不应让每一轮随机换域；白名单外的额外头按私有请求 scope 处理。
- 旧来源身份不迁移、不改写 rollout；算法改变使用新版本标识，旧 opaque 保守降级。禁止 hash/serialize/Debug 实际密钥。
- 兼容入口 `reasoning_source` 不得默认提供跨凭据相等证明；保留源码签名时使用 unbound 身份，明确说明兼容影响。

## S2：session 行政命令和 queue 的归属

- archive/delete/unarchive 是会话管理操作；UUID 不应因当前模型服务不同而不可管理。名字查找应跨 provider，多个匹配必须明确报歧义，不可自动选第一个。
- queue 写入必须经过会话的 owner server；有运行中 daemon 时禁止另起 embedded writer。环境变量不得让另一个客户端凭据成为 daemon 默认配置。
- 活跃本地 NUWAX 组与显式 remote 保持现有 fail-fast；远端模型配置和凭据由远端进程持有。UUID queue 只提交消息，不隐式切换模型。
- 本地命令处理完整/失活/损坏环境组的规则应一致；必须先确定 queue 的 owner 再决定 seeds/daemon 策略，不能仅替换 exclusion。
- cold resume 的显式 config.model_provider 表示选当前配置模型（即使 provider 与磁盘相同）；无显式路由则保留旧模型。已有加载/订阅/运行中 thread 的覆盖按 live owner 的兼容规则处理，不保证与 cold 路径相同；补 warm/subscribed 对照再决定统一策略。

## S3：容器进程级请求控制

- 在已有 NUWAX group 上增加可选 `NUWAX_REQUEST_MAX_RETRIES`、`NUWAX_STREAM_MAX_RETRIES`、`NUWAX_STREAM_IDLE_TIMEOUT_MS`。
- retries 的 0 合法；timeout 必须为正；非 Unicode/负数/溢出/空串明确报具体变量名，不输出值。有效组才消费；显式其他 provider 时忽略整组；孤立控制项沿用 output cap 的 fail-fast 规则。
- provider TOML/CLI 明确配置优先级与现有 EnvSeed 保持一致；不得把临时值写入 config.toml。后台 daemon 启动时剥离新变量，真实请求进程负责解析。
- 不新增任意 headers/query 的原始 JSON 环境注入；先完成 S1，敏感配置继续用 named provider + env_key。
- 用户文档分别说明 HTTP 握手重试、Core 采样重试、pause continuation、idle timeout，不能宣称单一总 attempt 预算。

## S4：测试与证据

- Core 桥测试单包调用必须显式开启 rust-rig，测试数为 0 算验证未执行。构建收据按实际树重建，工件区分 source/package 与 dependency feature graph。
- live 的 path/model/cap 字段在最终 request capture 中真正断言后才能记 wire_asserted；鉴权 headers 继续由离线 TCP 测试证明，不为取证落盘密钥。
- 三协议输出限制应有 mock 的真实终止语义测试；live cap 接受字段和实际触顶分别登记。加密引用、跨进程 opaque 降级单列验收。
- pause 40,960 bytes 为既有兼容上限，保留 P0 人工上下文预算复审：不是精确 10K-token 证明。不要在上述任务中顺手改变 cap 或截断签名。

## 本轮不要求

genai 新功能、完整付费矩阵、workspace 全套、远程 CI dispatch、发布、push。不要自动修改测试 deadline、禁用 TLS、变更 V8 pin 或清理运行时数据。
