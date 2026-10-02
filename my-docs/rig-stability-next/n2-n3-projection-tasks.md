# N2/N3 — Tasks

- [ ] B1 投影层引用归属与正文去重
  - [ ] B1a ReplayGroup 归属文本块 + 替换投影（红→绿 wire 用例：正文一次）
  - [ ] B1b 多文本/部分匹配边界（无对应文本时维持尾部追加）
- [ ] B2 混合轮前缀稳定
  - [ ] B2a 完成对拆开：call 留原位、result 到新响应位置
  - [ ] B2b `[client call, pending, client output, late result]` 真实四步
        wire 回归：requests[1]/[2] 前缀逐条相等
- [ ] B3 真实 Core 工具闭环矩阵
  - [ ] B3a 多搜索 + 多文本 + 迟到结果（事件累积 → 请求深比较）
  - [ ] B3b 多次 pause 续接后的前缀与引用
  - [ ] B3c 保存恢复（resume）与 fork 后的请求投影
  - [ ] B3d 同组聚合超限 whole-drop 不牵连未超限对
- [ ] 验证记录更新（命令、退出码、计数）
