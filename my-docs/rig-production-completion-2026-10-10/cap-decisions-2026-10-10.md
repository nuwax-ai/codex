# cap 七决策落实记录（2026-10-10，T15）

依据：`cap-partial-usage-implementation-proposal-2026-10-07.md`（提案）与 tasks.md T15。本文件把七个未决决策点落成可执行契约；标注"已实施"的条目有对应提交与测试证据，其余为已裁决的待实施契约。产品若否决任一条，后续批次（T17–T20）按否决结果重做。

## 决策

| 决策点 | 裁决 | 状态 |
|---|---|---|
| D1-a 失败载体 | 独立 `CapPartial` 事件 + paginated `TurnItem::CapPartial`（不复用 Message status） | 已裁决，T18 实施 |
| D1-b 旧读取器 | 不预设兼容；V-D1-1 实测改前二进制读改后 rollout 与改后读旧，按实际行为登记（整份失败/单行跳过/保留） | 已裁决（实测前置），T18 执行 |
| D1-c 预算数值 | 单片段 2,000 token / 16 KiB；每 turn 8,192 token / 64 KiB / 32 片段；保前缀 + `truncated` 标记；>1k token 片段保留 P0 人工门，>10k 禁止；无 tokenizer 证明 → unverified，不注入模型 | 已裁决，T18 实施 |
| D2-a 失败 usage 载体 | 类型化 `ApiError::CapExhausted { message, response_id, reported_usage }`；presence 保留（`ReportedUsageCounters` 全 Option、显式零与缺席区分、Complete 仅全字段直报） | **已实施**（`5667058bc`；三线提取中 Responses SSE/WS 与 Anthropic wire 观测完成，rig 归一化回退只认非零） |
| D3-a exec 事件 | 内嵌 `turn.failed`（不新增独立 turn.partial 事件），新字段 `skip_serializing_if` 保持无 cap 时旧字节 | 已裁决，T20 实施 |
| D4-a 完整性枚举 | 三值 `UsageCompleteness`（Complete/Incomplete/Unknown）+ 全响应已知小计（`ReportedUsageCounts`），与 legacy `total/last` 并存不覆盖其口径 | 类型已实施（protocol）；reducer/持久化 T17–T19 |
| D4-b 旧历史 | 无完整性证据 → Unknown（不默认 Incomplete/Complete）；后续成功不抹旧缺口 | 已裁决，T19 实施 |

## 已实施语义要点（D2）

- Responses 终止帧 usage 逐字段 presence 解析；错误立即发出不缓冲（SSE 缓冲白名单迁移，HEAD 对拍验证）。
- Anthropic：message_start/message_delta 累积观测（显式零保留）；total 从不上（wire 无此字段）→ 恒 ≤Incomplete。
- rig 归一化 usage 的零与缺席不可分 → 回退报告只认非零计数、恒 Incomplete（如实弱化，V-D2-1 迟到 usage chunk 测试后可收紧）。
- message 文本逐字保留，既有按消息匹配的下游断言继续命中。

## 开放项（进入 T17 前必须定）

1. **稳定 response_key**：host 在 sampling 开始创建并持久化；成功 Completed 与 CapExhausted 共用 reducer；同 key 快照替换、跨 response 逐计数相加；provider id 仅辅助。实现位置与持久化载体在 T17 批内设计评审。
2. V-D2-1：Chat 终止后迟到 usage chunk 的 wire 顺序测试（rig 是否在 finish 后丢弃后续 chunk）。
3. 模型可见 fragment（D1 的 ContextualUserFragment 注册、预算内 render、识别器注册）全部在 T18，先过 P0 人工门再注入。

## 关联契约变更登记（R1b 顺带）

`9d307f72d` 将 Compaction 项的投影兼容放宽为 scope 级（provider/endpoint/wire/bridge + evidence kind 相等；模型与进程内实例 id 不再阻断），Reasoning/WebSearch 保持严格。理由与边界见该提交信息与 results.md 批 F。
