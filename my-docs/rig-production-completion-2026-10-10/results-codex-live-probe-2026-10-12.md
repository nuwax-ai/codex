# Codex 独立有限 live 验证：MiMo / GLM / Step

2026-10-12。用户在当前会话提供三家测试密钥、明确模型与端点，用于测试验证；本次据此进行有限 live 请求，不沿用旧文档“未授权真实请求”的状态。这里只统计本次 Codex 独立控制器，不包含共享工作区其它开发会话的请求。

## 结果

| 厂商 | 模型 | Chat | Anthropic | Responses |
|---|---|---|---|---|
| MiMo | mimo-v2.6-flash | PASS | PASS | PASS |
| MiMo | mimo-v2.5 | PASS | PASS | PASS |
| GLM | GLM-5.3-Flash | PASS | PASS | PASS |
| Step | step-5-preview | PASS | PASS | 未配置/未请求 |
| Step | step-3.7-flash | PASS | PASS | 未配置/未请求 |

**13/13 基础模型/协议组合实跑通过**：真实厂商 HTTP 200、准确 model/auth/cap、指定 marker 回复、恰一次完成、零失败、非零 input/output usage，专属 Rig request capture 存在。每组合仅一次实际 provider POST。

**真实工具往返**：MiMo-v2.6-flash/Chat、GLM-5.3-Flash/Responses、Step-5-preview/Anthropic 最终均通过。验证实际 `printf` 命令执行成功并输出随机 marker、第二次请求包含相同 call ID 配对的 marker-bearing 工具结果（不以用户 prompt 中已有 marker 冒充）、最终答案准确 marker、completion 和 usage。

首败保留：MiMo 第一条英文泛称“terminal tool”的请求仅回复 marker，没有执行工具，只有一次 POST；不能当工具 PASS。改为明确中文要求 `exec_command`、真实执行及只读 printf 权限后，一次限定复验以两次 POST 通过。它证明该真实模型工具路径可用，不证明模型每次都会按工具意图调用；未以修改 Rig 或重放终局“修复”模型选择。

实际请求账目：主模型基础 8 + 备用模型基础 5 + 首次工具批 5 + MiMo 定向复验 2 = **20 次 provider POST**。所有外发请求均带 cap=2048；请求/stream retry 配为 0，并有独立转发硬闸门：基础每组合最多 1、工具每组合最多 2，超预算阻断在本地，不向厂商发送。

## 配置与来源证据

使用真实 `codex-exec`，配置通过 `NUWAX_BASE_URL/WIRE_API/MODEL/API_KEY/MAX_OUTPUT_TOKENS/REQUEST_MAX_RETRIES/STREAM_MAX_RETRIES/STREAM_IDLE_TIMEOUT_MS` 进入子进程；临时 config.toml 只有 approval=never、sandbox=read-only、reasoning effort=none，无 provider 和凭据。没有 native/GenAI 覆盖。

四批开始时自报告 receipt 和二进制 SHA 一致：

- build Git：`b5a886e396a77edbc01b0e5acf3af3ca947bc41c`，macOS arm64/debug。
- source fingerprint：`77fff998c63fe1c2b344443da853594cc8928cda5c6d69e67a4546300cc7cdf0`。
- binary SHA256：`62e8b5cb04809dd6a6a02542e204d1351b57eba8bd0dab2da437af22ec4ac3c0`。
- 本次先运行 `cargo build -p codex-exec --bin codex-exec --locked --offline` 成功；测试不是 `cargo test`，是公共 CLI 的独立有界 live 场景。

请求通过本地计数闸门转发至用户指定 HTTPS 端点，TLS 证书验证开启，路径保持不变：

- MiMo：`/v1/chat/completions`、`/anthropic/v1/messages`、`/v1/responses`。
- GLM：`/api/coding/paas/v4/chat/completions`、`/api/anthropic/v1/messages`、`/api/v1/responses`。
- Step：`/step_plan/v1/chat/completions`、`/step_plan/v1/messages`。

这是 env→Core→Rig→真实厂商解析/usage/工具证据；Core 看见的是 loopback provider origin，上游 TLS 由计数闸门建立。**不能把它冒称 Codex 自身直接 HTTPS 的 TLS/代理/连接性能验收**，也未验证跨真实 origin 的历史授权/旋转。CLI receipt 的 resolved dependency feature graph 仍标 unknown，实际 Rig 路径由其专属 capture 补证。

## 密钥与收据

密钥只放 Git 外 0600 临时文件和子进程环境。场景结束对全部临时文件做字节扫描（包括 SQLite），未发现原始密钥；所有临时 home/cwd 自动删除，凭据文件已删除、运行器配置已清空。保留的 JSON 不含认证头、凭据、原始请求全文、模型全文或私有配置。`.env.example` 只更新公开端点与空 key 占位。

机器可读统计工件：[codex-live-probe-receipt-2026-10-12.json](codex-live-probe-receipt-2026-10-12.json)。该 JSON 是生成的统计/receipt 工件，不是额外产品逻辑代码；保存首次失败，不将复验覆盖旧结果。

## 后续最短任务

1. 将 typed live helper 的 retry/idle 控制真正传入 child，已有 `Startup` 能表达但当前 public helper 默认 None；不能靠父环境设 0（runner 会剥除）。nextest live 默认重跑也需明确 `--retries 0`。Anthropic pause continuation 另有内部请求，需要独立预算，不能把重试=0 当绝对请求上限。
2. live marker 验收补本次采用的真实配对工具结果、最终 marker 和 usage 断言；当前“执行输出有 marker + 最终答案非空”不足。非泄露检查改字节扫描 home/cwd/artifacts，传播读取错误；现有 read_to_string(...).unwrap_or_default() 会漏掉二进制文件。
3. 选定一个主模型继续当前二进制直接 HTTPS、app-server/ACP、真实取消/resume/compaction 与 cap 触顶验证，分别限制请求预算。此次接受并发送 cap 字段 **不是实际触顶证据**。未测试各模型所有工具/协议组合的可靠性，不扩称三平台或全厂商发布验收。

开发中心仍是 Codex/Rig；不修改相邻 ACP 仓库，不改变代理默认、V8 pin 或沙箱逻辑，不 push/dispatch。后续密钥需求通过私密配置提供，勿复制到交接提示词或 Git。
