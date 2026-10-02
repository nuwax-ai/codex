# N2/N3 — Plan（如何实现）

依赖批次，每批可独立构建，tests → scoped fix → fmt。

## B1 投影层引用归属与正文去重（N2）

改动面：`codex-rust-rig-bridge/src/request_messages.rs`（投影）、
`transport.rs`（组内 splice）。

1. `ReplayGroup` 增加"归属文本块"概念：cited block 的目标位置 = 同组
   assistant 内与其文本（规范化后）相同的 text 块；不同则维持尾部追加。
2. 投影构建：命中时用 cited 终态（text+citations）**替换**该 text 块，
   plain answer 不再重复出现。
3. 深比较 wire 回归：真实完成轮事件累积（含保存的 Message 与
   WebSearchCall envelope）→ 下一请求 messages 整体断言：正文一次、位置
   正确、前缀稳定。

## B2 混合轮前缀稳定（N3）

改动面：`request_messages.rs`（dedup/迁移逻辑）。

1. 拆开"完成对"迁移：同 id 完成条目到达时，不再删除原 pending 投影位置；
   server_tool_use 块留原 assistant，result 块投影到其所属响应位置的
   assistant 尾部。
2. 同 id 多完成条目：首个有效完成获胜（保持），但获胜方式是"丢弃后续
   result 块"，不是移动 call。
3. wire 回归 `[client call, pending server call, client output, late
   result]` 四步真实序列：requests[1]/requests[2] 前缀逐条相等；client
   call/output 不消失。

## B3 真实 Core 工具闭环矩阵

`codex-core/tests/suite/rig_responses_bridge.rs` 或新集成测试文件：

- 多搜索轮（≥2 对）、多文本块、多次 pause 续接、迟到结果（跨响应）、保存
  恢复（resume 重载历史）、fork（复制历史）后连续请求深比较与缓存前缀
  （除最后一条 user 消息外逐字节一致）。
- 超限聚合场景：两个各自合法 envelope 同组聚合超 40,960B → 按预算表
  whole-drop，未超限对不受牵连。

## 验证

- 每批：`just test -p codex-rust-rig-bridge`（168+ 用例全量）+
  相关 core 选择；wire 深比较新用例先行（红→绿）。
- B3 后：core rig bridge 套件选择集 + bridge 全量。
