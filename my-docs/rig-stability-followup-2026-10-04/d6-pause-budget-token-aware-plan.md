# D6：pause 预算实施计划（复查修订，2026-10-05）

对应 Spec：d6-pause-budget-token-aware-spec.md。仅设计；不在当前提交启用新预算。

## 分层

| 层 | 拟落点 | 责任 |
|---|---|---|
| 计量与证明 | bridge 私有 pause_budget 模块 | 区分 LegacyBytes、ExactTokens、ProvenUpperBound；携带 tokenizer/framing 版本和覆盖范围 |
| 判定 | chainer 构建实际续接 message 后、发送前 | 检查完整序列化内容，KeepWholeOrFail；不修改原始 signed/opaque 块 |
| 诊断 | 已有 telemetry 或单独有界 metadata | 输出模式/计量值/证明状态/错误分类，不记录秘密内容 |
| 测试 | 独立 *_tests.rs + 公共 HTTP suite | 证明范围、阈值、零发送、协议完整性与 Legacy 兼容 |

## 实施顺序

1. 先建立计量单位及 provider framing 清单。opaque 字符串也是实际输入的一部分，不能简单除以经验 K；校准 95 分位不产生最坏情况上界。
2. 定义 estimator 的显式 Send future/同步契约和 Unverified/Exact/UpperBound 结果。未知或超出证明适用范围只给 unverified，不能隐式转换成 exact。
3. 引入 exact 测试桩及已证明上界的受控桩，先覆盖完整 JSON/role/tool/citation/framing，再接真实受信 tokenizer。
4. Legacy 路径仍执行当前 40,960-byte fail-fast；不引入 DropOpaque/DropAll。token-aware 路径在产品裁决后独立 opt-in，真实超限返回明确错误，保留原历史。
5. 若需要 rollout 新 metadata，单独评估 API/schema、尺寸上限与读侧兼容。旧记录缺字段不获得 tokenizer 证明，不回填或改写磁盘。
6. 运行独立单元/公共 HTTP 回归后才增加受控厂商验收；仅有本地 tokenizer 计数不能宣称厂商隐藏 framing 已验证。

## 必须保留的边界

- 模型输入计数的可信度来自证明适用范围，不来自名字 token-aware 或一套 boundary 单元测试。
- 不同时改变 pause 次数、签名回放、source 身份、历史投影或默认 cap。各项兼容变化分别审查。
- 在原有字节范围内新增 token 拒绝会降低部分内容的可用容量，不能称为“零行为变化”；配置/迁移/错误提示须由产品确认。
- 持续保留 P0 人工复审，直到实际 provider、模型、tokenizer 与 framing 的相关验收完成。
