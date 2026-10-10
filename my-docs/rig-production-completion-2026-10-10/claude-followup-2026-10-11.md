# Claude Code：阶段修复后的完整后续开发

先读 `codex-review-2026-10-11.md`、本目录Spec/Plan/Tasks、修订results/handoff及两个canonical/CI ledger。记录实际HEAD/分支/dirty树，不reset/stash/clean、不覆盖并行工作。新审查优先于旧“全绿/终局/并发固有/已完成”。当前修复checkpoint看git log，本提示词不要求退回070098774。

## 第一批：cap完整失败管线（最高优先）

1. 保留本轮安全默认：cap不发普通Done、不执行工具，不用dangerous flush让durable测试变绿。失败前已显示delta保留。
2. 将ApiError::CapExhausted的reported usage/response信息通过错误映射传入Core，保持非重试与既有错误文案；失败各层恰一次终局，不能合成成功Completed。
3. 为每sampling创建stable host key并持久化；provider response ID只作辅助，不能凭跨请求相同ID猜同一response。沿成功/cap/retry/pause/重复快照/恢复的同一个reducer处理known/incomplete/unknown，旧usage口径不静默改变。
4. 在实际stream观察点进行有界partial采集，不等失败后从普通history猜最后片段。采集期、单fragment、turn、metadata/序列化/render分别硬限；避免先collect/format无界全文后才截断。
5. 使用已有CapPartial类型前落实旧reader实测与显式启用/升级降级策略；legacy/paginated写读、crash/export/archive/delete、继续和两次resume都验。诊断文本不当可执行工具、不冒充人类授权、不升级完整assistant。
6. 模型可见注入先有完整render tokenizer/上界证明、core/context struct/trait/matcher注册及P0人工门；没有证明只作诊断不能关闭“模型resume可见”验收。
7. 打通ThreadTokenUsage v2(nullable/TS/schema)、rawResponseItem/*兼容、exec/SDK/UI与unknown持久性。终止后usage、显式0/缺席、负/溢出、Anthropic start/delta、Responses SSE/WS/Rig各有wire用例。

DoD：公共Core/RPC/exec真实请求、事件、rollout/read/resume和旧客户端断言全齐；合法JSON工具cap也零执行；恰一次failed、零成功Completed/重采样；不能用optional None可过断言或codec单测替代。

## 第二批：代理与普通生产默认

保留实际auth域、CGI、跨策略cache、空runloop正等待、Factory逐目的地/逐hop选路等本轮修复。ResolvedDefault目前是opt-in，Windows/WS/SOCKS/特殊userinfo委托SDK，不能把未优化写成不支持或已全平台等价。

- 补普通Basic Auth、percent编码、CONNECT/407/TLS/SNI/IPv6的实际请求头双轨；现有URI/计数断言不够。
- 两版本reqwest对照手动系统/env/NO_PROXY/CGI/重定向/复用；系统PAC策略独立，不偷换默认。
- 注入spawn失败、永久卡读/有界重建、stale/TTL/取消，完善线程生命周期；真实低载冷/暖原预算和runloop因果实验。
- 默认Core/OTLP实际路径仍ReqwestDefault；替代fixture的PASS不能关闭它。准备具体默认迁移diff和兼容结果后裁决，期间继续独立任务。

## 第三批：CI与栈问题实际收口

- 使用ci-run6-failure-ledger的827真实终态，不按“固有边界”概括。strict missing host类别仅已证子集，其他逐个定位；23旧BOUNDARY在macCI全PASS必须保留新窗口。
- 保持V8 pin，准备可核archive/binding/feature配对或正确源码构建。host crate与需要它的测试/辅助binary一致，不能只exclude编译目标、不能假造host。若用户裁决缩小平台覆盖，明确整个消费者集合与缺失能力。
- 对bwrap被拒定位具体能力/配置与可用runner，运行受控正负校准；不以一条EPERM推定所有失败。确需其他环境时提供具体条件并继续其余工作，不加skip求绿。
- 复现Ubuntu SIGABRT与Windows0xc00000fd：正常显式4MiB线程、同feature/集合/并发，量future构造/移动/poll，最小修复，不加栈/放宽deadline。
- 每个FAIL/TIMEOUT/ABORT/LEAK保留原身份与首败、修复和当前证据；setup进入达成不等于最终门禁绿。

## 第四批：原全范围剩余验收

继续原T21–T40：D6可信计量与KeepWholeOrFail、owner四命令/安装/flags/运行状态/crash/锁、三wire来源旋转/旧历史/RawValue/hosted合法顺序、原生Linux/Windows/容器/异OS、Bazel/完整workspace、实际厂商与安装升级回滚。

普通Compaction必须有实际授权域连续性；跨进程credentialInstance缺证保守降级。需要同凭据跨进程兼容时设计真实可验证来源/可见历史回退，不恢复kind-only或backend自行拒绝作为许可；grant不能由持久bool授予。当前账号重启测试已补固定account/user claims并通过；API key进程实例不能据此跨进程互信。已要求grant却不能验证时客户端fail-fast；继续补未知/旧metadata在选种子处的完整拒绝与可见fallback公共负控。

## 执行和提交边界

相关just test、Core rust-rig、offline/locked、retries0、真实非零匹配和HTTP/RPC断言；fresh辅助binary，source/git/dependency/bin/receipt分别留证。不能复制旧机临时target/V8缓存当clean平台证明。

每批先tests，最后scoped just fix、just fmt、diff-check；该批最后之后不再test。依赖/Bazel lock、ConfigToml/API schema/TS和compile_data同步。复杂<500、普通<800，机械移动独立；并行worker明确ownership且不回退他人。

可以按用户转交本提示词的授权自主开发、必要离线验证并分批本地commit；完整workspace按AGENTS确认已有授权/执行条件。paid live、远程手动dispatch、push/tag/发布需要对应明确授权。平台/费用阻断不停止其他独立任务。

结果写 `results-followup-2026-10-11.md`，40项逐一写实现/验证/开放，不把新报告或分类标签当完成。最终提交列表、准确PASS/FAIL/SKIP/TIMEOUT/ABORT/LEAK、首败复验、source/bin与发布范围完整交付。
