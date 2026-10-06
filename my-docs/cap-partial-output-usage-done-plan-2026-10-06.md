# cap 终止契约实施计划（Plan，2026-10-06）

状态：设计提案，未实施。对应 Spec：cap-partial-output-usage-done-spec-2026-10-06.md。前置：Spec 获产品过审，并明确 partial 表示、失败 usage 与线程累计不完整状态的兼容方案。未过审不实现；本轮不调用厂商。

## 分层与顺序

| 步 | 落点 | 内容 | 验收 |
|---|---|---|---|
| 0 | 契约与 schema 决策 | 决定失败 partial 的表示、usage 未知与累计不完整状态；核对现有 exec JSON、app-server v2、rollout 类型能否表达，列明新增字段/类型、旧客户端与旧历史读取策略 | 类型/消费方清单、兼容性评审；不承诺零 schema 变更 |
| 1 | bridge 终止帧解析 | 在 cap 错误映射前提取三 wire 已报告的 usage：Chat usage chunk、Anthropic usage 帧、Responses `response.incomplete.response.usage`；按实际帧顺序聚合，缺失保持 Unknown，不合成桥层成功 `Completed` | 桥级单测：有/无 usage、终止/usage 帧顺序、恰一失败且无成功完成 |
| 2 | Core 失败聚合 | 将 usage/未知和有界 partial 状态传向已选消费方；保留一次 `Error` 和随后一次 `TurnComplete(error)`，失败关闭后不再发该 turn 的 delta | core 公共路径（test_codex）三协议断言：失败收尾存在、无桥层成功 `Completed`/最终成功 message、partial 保留、usage 透传/未知 |
| 3 | app-server | 保留 `turn/completed(status=failed)` 及同源 error；按第 0 步方案传递 partial、失败 usage 和累计完整性，必要时走 v2 schema 流程（camelCase/TS/schema fixture 同步） | app-server 集成测试：关闭恰一次、partial 在前、旧客户端兼容 |
| 4 | exec `--json` | 消费 app-server 失败关闭并生成 `turn.failed`；按已选方案新增 partial/usage 映射，不输出成功 `turn.completed`；同步 exec JSON 类型和相关 SDK 导出 | exec JSON 事件序列快照、失败后进程退出、兼容性测试 |
| 5 | rollout | 现状 delta/独立 Error 不持久化；新增有界 partial 累计/部分 item 的落盘与读取路径，保留既有 `TurnComplete(error)`；usage/未知按第 0 步方案保存 | legacy/paginated 落盘与 resume：可见 partial、失败状态保留、截断工具不执行、旧历史可读 |
| 6 | （授权后）Responses live 触顶 | 新建 binary/source receipt、限定场景与费用、当次授权 | 与 Chat/Anthropic 触顶同标准 |

## 边界

- 第 0 步先决定兼容边界；第 1–2 步实现核心语义（"未知≠0"），第 3 步提供 exec 消费的通知，第 4 步依赖第 3 步，第 5 步依赖第 2 步的 partial/usage 状态。新增类型与其消费方保持可编译的小批，不能仅按文件分层拆开必要依赖。
- 每批 <500 changed lines；不改 cap 触发/重采样语义（D3 已钉）；不合成桥层成功 `Completed`、不执行截断工具，保留各层失败关闭。
- UI 展示口径（部分输出+失败态、usage 未知与累计不完整）在第 0 步定案，第 3–5 步实现与验证。
