# N2/N3 — Tasks

2026-10-03 第四轮。第三轮 v2 验证只证明当时场景；六项新增缺陷由以下完整实现与回归负责。

- [x] 实现 v3 显式响应 ID 与稳定 Message/Reasoning/客户端工具消息段 ID，无协议 schema 新字段
- [x] 捕获全响应顺序与 owner/part，保留思考、签名与工具边界
- [x] 每个 sibling 独立声明响应；首 carrier 超限降级为同响应 pair-only
- [x] prompt 转换携带指定消息段范围与 pair-only 明确边界，不再按文本查找归属
- [x] sibling 完整索引覆盖先校验；去重/预算过滤后严格递增、唯一、完整覆盖再校验
- [x] 独立 layout 数目与全部字节预算；移除未贡献接受块的重复 carrier/layout
- [x] v1/v2 pair-only 兼容，未知/损坏/source 不匹配删除 payload，不改普通历史
- [x] 六项 unit/wire 回归实现，实际 HTTP 请求内容作为断言
- [x] 真实 Core pause + for_prompt + save/resume/fork 回归实现
- [ ] 主代理统一运行 bridge 与 Core Anthropic 定向测试，登记最终退出码/计数
- [ ] 主代理执行 scoped fix / fmt，按规则不在其后重复测试
- [ ] 整体 request/model 预算、source/授权域矩阵与 binary acceptance 由对应工作流补齐
- [ ] 更新全局 verification：明确本地 mock、实际平台、远端 CI 与发布边界
