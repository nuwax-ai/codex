# Rig 稳定性下一轮 — Spec（做什么）

日期：2026-10-02。基线 HEAD `f9c560ed6` + 工作树未提交 F01–F21 修复（保留并适配）。
输入：`my-docs/codex-review-2026-10-02.md` 的 N1–N7；`AGENTS.md` 约束。

## 目标

默认 Rig 请求路径稳定支持三协议（OpenAI Responses / Chat Completions /
Anthropic Messages）、`NUWAX_*` 环境启动组、会话保存/恢复/fork 与跨模型/协议
切换。Responses 必须经 Rig Responses 适配器实际请求 Responses endpoint。

## 非目标

- 不重写已提交历史，不自动 push/发布，不伪造签名/密文，不重写已有 rollout。
- 不引入破坏 RawValue/数字精度/签名的全量 `serde_json::Value` 往返。
- 不以键名白名单、手工重建请求、PASS 总数或 harness 收据替代验收。

## N1 临时 provider 来源隔离（P1）

四键白名单只证明键名不证明来源。要求：seed 的真实来源身份保留到配置层栈；
`model_providers.nuwax_env` 的任何非 seed 贡献（文件/profile/managed/CLI/
thread，**含同名合法字段 base_url/env_key**）在合并前被识别并在最终选择为
保留 provider 时硬错误；未提供环境组却选中保留 id 同样硬错误。拒绝场景零
HTTP 请求。doctor 的最终 provider 与 endpoint 来源按层栈事实推导。

### 边界决定

- seed 以独立配置层（`ConfigLayerSource::EnvSeed`，precedence 介于 Project
  与 SessionFlags 之间）进入层栈：显式 `-c`/typed CLI 仍高于环境组（既有
  文档语义不变），项目配置低于环境组（与现状一致——现状 seed 位于
  SessionFlags 内）。
- thread config / 会话标志层（同为 SessionFlags 源）高于 seed：模型与
  provider 的每线程选择保持现状；但对保留表的任何贡献被隔离校验拒绝。
- thread config 选择保留 id（组活跃时）= 采纳该进程环境组，endpoint 不可
  重定向，允许；组不活跃时选中保留 id = 无 seed 层，硬错误。
- requirements（managed）定义/替换保留表 → 硬错误。
- 刷新（refresh）路径按会话层保留 EnvSeed 层（`is_session_layer` 扩展）。

## N2 普通轮引用投影与正文位置（P1）

完成轮的普通正文（Message）只出现一次；cited 终态替换对应文本投影而不是
尾部追加；已发送请求前缀稳定。真实事件累积 → 下一请求整体 messages 深比较。

## N3 混合轮保持历史前缀（P1）

`[client call, pending server call, client output, late result]` 场景中，
pending call 保留原位置，迟到 result 追加在正确的新响应位置；不以"去重成功"
改变旧请求前缀。需要 Core 工具闭环 requests[1]/requests[2]、保存恢复与缓存
前缀证明。

## N4 generic provenance 与真实 Core 矩阵（P1）

`model_output_provenance` 进入投影决定；定义 provider/endpoint/model 与必要
授权域的身份边界；同源/换模型/换协议/换厂商/旧裸数组/unknown version/损坏
载荷/fork 的真实 Core 场景。

## N5 统一载荷预算与取消/计量验收（P0/P1）

### 预算规范（尽早定义）

分层预算，超限行为明确失败（whole-drop 或明确错误），绝不截断签名/密文：

| 层 | 单项边界 | 整体边界 | 超限行为 |
|---|---|---|---|
| 持久化 envelope（capture/导入） | 序列化 ≤40,960 B（含 cited_text/source/version） | — | 整体丢弃 + 告警（调用仍完成） |
| 请求侧回放对 | 每对（blocks+该组 cited_text）≤40,960 B | ≤64 对/请求（丢最旧） | whole-drop |
| 暂停 raw assistant 消息 | 每条 ≤40,960 B | replay+pause 调用合计 ≤64 | 明确错误（不续发部分消息） |
| tee/cassette 捕获缓冲 | — | 8 MiB | 停采 + 告警（流不受影响） |

字节边界是保守代理（bytes/4 ≈ token），**不宣称精确 token 上限**；模型可见
新增 raw block 的 >1K token 路径按上下文复审标 P0，由上述字节边界 +
model context window（compact 循环）双层兜底。缓载入的模型级精确 token 预算
为独立后续 Spec（本轮登记不实施）。

### 取消与计量验收

Core `Op::Interrupt`、超时、重试、等待续传响应头时取消、取消后工具不重复
执行；多次 pause 的 usage/request ID 语义：**最后请求的 Completed 为该 turn
的 usage 事件**（上下文占用），全轮累计计量显式汇总而不冒充上下文大小。

## N6 构建与请求证据（P2）

- codex-exec 自身提供 source SHA、dirty/源码指纹、features、target/profile
  与 binary hash 的可核验绑定；执行前校验收据。
- 最终 HTTP 层测试用脱敏 recorder（捕获 SDK/transport 转换后的实际请求体；
  凭据/header/query 不入工件），与 Core 保存块深比较。
- 失败路径也保留部分 rollout、stdout/stderr、退出码与 manifest。
- 离线回归后最小 MiMo/GLM 真实请求；未触发 pause/不支持能力标 not-run。

## N7 CI、Bazel、schema 与交付门禁（验证缺口）

remote proto、schema、依赖锁、feature 组合核对；桥/live-tests 的 Bazel 接入
范围明确 + 可执行 Cargo CI；Linux/Windows/npm/remote exec 分别登记证据；
需要授权的动作明确列出。
