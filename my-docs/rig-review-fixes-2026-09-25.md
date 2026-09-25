# Rig 链路复审与修复记录

日期：2026-09-25。基线：`17e33623654dc8c3f07b54e7d08762ab76e0dd53`。本记录覆盖本次 Rig 修复及其验证结果。

本轮按最近指令检查并修复默认 Rig 的请求、字段、流事件、认证、配置与辅助推理链路。前一版已有的修复单独标注；早期整体 review 中的发布/安装问题在附录保留状态。

## 实际路由

- 第三方 Chat/Anthropic 与默认第三方 Responses 使用 Rig；第三方 Responses 在该策略下实际转换成 Chat Completions。
- 内置 OpenAI/Bedrock 及显式 native 保留原生协议；GenAI 仅显式 opt-in，没有自动回退。
- Guardian 桥接场景使用普通审批回退；LMStudio 仍有原生 Responses 预加载入口。不能将当前结果描述为所有 HTTP 模型请求均经过 Rig。
- 详细字段与支持边界：[field-mapping-audit.md:1](/Users/soddy/Documents/git-rust-work/fork-codex/my-docs/field-mapping-audit.md:1)。

## 本轮已修复的问题

1. **[P0] 工具图片被 JSON 文本化或发送为不支持的 tool image — 已修复。** Function/Custom 结果共用多模态转换：Chat 图片放在全部相邻工具结果之后；Anthropic 保留支持的 Base64 tool_result。文本每段使用约 8,000 token 上限，避免把 base64 当作长文本注入。P0 来自 context review 对超过 1K token 新上下文项的复核要求。 [request_messages.rs:228](/Users/soddy/Documents/git-rust-work/fork-codex/codex-rs/codex-rust-rig-bridge/src/request_messages.rs:228)

2. **[P1] developer/system 指令被降为 user — 已修复。** 保留为 System；非法角色与非文本 system 内容在请求启动前报错。 [request_messages.rs:71](/Users/soddy/Documents/git-rust-work/fork-codex/codex-rs/codex-rust-rig-bridge/src/request_messages.rs:71)

3. **[P1] 首轮带工具时结构化输出丢失 — 已修复。** Chat 显式发送原始 response_format，保留 name/strict/schema；Anthropic 使用 typed output_schema，不再受 Chat gate 或 clear-all 参数逻辑影响。 [convert_request.rs:12](/Users/soddy/Documents/git-rust-work/fork-codex/codex-rs/codex-rust-rig-bridge/src/convert_request.rs:12)

4. **[P1] namespace/custom/AdditionalTools 转换不一致 — 已修复。** 共用工具声明解析和名称展平，custom 使用 input wrapper，历史调用及 cassette 元数据均使用同一名称来源。 [request_tools.rs:27](/Users/soddy/Documents/git-rust-work/fork-codex/codex-rs/codex-rust-rig-bridge/src/request_tools.rs:27)

5. **[P1] 推理签名、redacted/encrypted 块和来源丢失 — 已修复。** 保存完整版本化 envelope；仅向相同协议、endpoint/query 和模型重放。envelope 本身不加密，不能称作密文保护。 [reasoning.rs:13](/Users/soddy/Documents/git-rust-work/fork-codex/codex-rs/codex-rust-rig-bridge/src/reasoning.rs:13)

6. **[P1] 切换 provider/桥后错误回放推理 — 已修复。** 旧版重复明文仅兼容 Chat；原生 Responses 过滤新 envelope 与严格识别的旧桥明文，GenAI 过滤新 envelope。请求副本过滤不修改保存的历史。 [client.rs:903](/Users/soddy/Documents/git-rust-work/fork-codex/codex-rs/core/src/client.rs:903)

7. **[P2] redacted 块使可见 reasoning 索引错位 — 已修复。** 增量 content_index 按此前可见 Text 数量计算，多段 reasoning complete 不再覆盖整轮思考。 [reasoning.rs:25](/Users/soddy/Documents/git-rust-work/fork-codex/codex-rs/codex-rust-rig-bridge/src/reasoning.rs:25)

8. **[P1] 工具 Added/Delta/Done 类型、ID、输入不一致 — 已修复。** 等待 SDK 确认 ToolCall 后再输出稳定 call ID 和一致参数；custom 的所有阶段均使用 CustomToolCall 及解包后的 input。SDK 修复过的参数只发最终一致值。 [response_tools.rs:60](/Users/soddy/Documents/git-rust-work/fork-codex/codex-rs/codex-rust-rig-bridge/src/response_tools.rs:60)

9. **[P1] 超长或非法参数可能变成可执行工具输入 — 已修复。** 增量和完整参数都限制 1 MiB；非法 wrapper/重复完成直接流错误，未确认调用不生成 Done；损坏的历史 JSON 不再替换成空对象。 [response_tools.rs:15](/Users/soddy/Documents/git-rust-work/fork-codex/codex-rs/codex-rust-rig-bridge/src/response_tools.rs:15)

10. **[P1] Chat 收到 DONE 后仍等 HTTP EOF 而超时 — 已修复。** Rig 0.42 的 Chat Final 在 EOF 才刷新；transport 用 SSE parser 在完整 DONE 帧后结束输入，保留 finish_reason 后的 usage trailer。测试覆盖跨 chunk、CRLF、多行 data、UTF-8、保持连接和独立关闭截止时间。 [transport.rs:156](/Users/soddy/Documents/git-rust-work/fork-codex/codex-rs/codex-rust-rig-bridge/src/transport.rs:156)

11. **[P2] 显式 Medium effort 被忽略 — 已修复。** Chat 的 effort 原值发送，避免依赖厂商默认档位；Anthropic 不混入 OpenAI 字段。 [convert_request.rs:56](/Users/soddy/Documents/git-rust-work/fork-codex/codex-rs/codex-rust-rig-bridge/src/convert_request.rs:56)

12. **[P2] 图片 detail 和 Chat function strict 丢失 — 已修复。** auto/low/high 明确映射；original 降为 high 并告警；实际 Chat function 定义保留显式 strict。Anthropic tool strict 尚未实现，另见边界表。 [request_messages.rs:377](/Users/soddy/Documents/git-rust-work/fork-codex/codex-rs/codex-rust-rig-bridge/src/request_messages.rs:377)

13. **[P1] query 参数破坏 SDK endpoint 拼接 — 已修复。** 先分离 query，待 SDK 拼接路径后在最终 URI 编码追加；既有重复 query 的相对顺序保留。来源键包含 query 摘要，不写入凭据明文。 [client.rs:125](/Users/soddy/Documents/git-rust-work/fork-codex/codex-rs/codex-rust-rig-bridge/src/client.rs:125)

14. **[P1] query 凭据出现在网络错误中 — 已修复。** 请求启动、普通 body 和流式 body 的 reqwest 错误均移除 URL，transport Debug 也不输出 headers/query。拒绝连接测试检查错误文本不含假密钥。 [transport.rs:197](/Users/soddy/Documents/git-rust-work/fork-codex/codex-rs/codex-rust-rig-bridge/src/transport.rs:197)

15. **[P1] 认证快照绕过刷新及网关冲突检查 — 已修复。** 每请求只异步解析一次 auth，并传播失败；静态 provider headers 参与有效认证头选择。测试确保失败时没有网络连接。 [stream.rs:79](/Users/soddy/Documents/git-rust-work/fork-codex/codex-rs/codex-rust-rig-bridge/src/stream.rs:79)

16. **[P2] 次级认证头被误删、非 Bearer 认证被错误改写 — 已修复。** 保留同值异名网关头和敏感标志；只从真正 Bearer 值提 key。Basic/Token Authorization 由 transport 原样覆盖，Anthropic 静态 x-api-key 可与独立网关认证共存。 [client.rs:42](/Users/soddy/Documents/git-rust-work/fork-codex/codex-rs/codex-rust-rig-bridge/src/client.rs:42)

17. **[P2] Rig 绕过自定义 CA — 已修复。** 使用 Codex CA helper 构造 rustls 配置，保留既有 CA 环境变量优先级；本次未做真实 TLS/企业代理验收。 [client.rs:99](/Users/soddy/Documents/git-rust-work/fork-codex/codex-rs/codex-rust-rig-bridge/src/client.rs:99)

18. **[P2] 响应模型、请求 ID、错误结构和重试建议丢失 — 已修复。** 输出 ServerModel，保存 request-id/x-request-id；401/429/5xx 在启动阶段保留 status、headers、原始 body 和 Retry-After；SDK RequestError/UrlError 归为 InvalidRequest。 [stream.rs:240](/Users/soddy/Documents/git-rust-work/fork-codex/codex-rs/codex-rust-rig-bridge/src/stream.rs:240)

19. **[P1] 内置身份与 Bedrock override 校验冲突 — 已修复。** 公共 OpenAI/Bedrock/Runtime 构造器自身设置身份；合并按配置键赋身份；Bedrock override 校验忽略运行时 provider_id，避免二次验证错误。 [lib.rs:343](/Users/soddy/Documents/git-rust-work/fork-codex/codex-rs/model-provider-info/src/lib.rs:343)

20. **[P2] 内置 provider 的 bridge-only override 被配置层拒绝 — 已修复。** 允许仅选择 bridge 的内置覆盖，仍限制通过保留 ID 改 endpoint/auth；TOML→校验→merge 和远端配置均有覆盖。 [config_toml.rs:882](/Users/soddy/Documents/git-rust-work/fork-codex/codex-rs/config/src/config_toml.rs:882)

21. **[P1] 默认桥接 Azure 却宣称支持远端 compaction — 已修复。** 统一 uses_chat_bridge 路由策略；默认桥接关闭 remote compaction，显式 native Azure 保持原生能力。 [provider.rs:415](/Users/soddy/Documents/git-rust-work/fork-codex/codex-rs/model-provider/src/provider.rs:415)

22. **[P2] 远端未知 bridge 值静默回落 — 已修复。** 未知枚举明确报错，rig/genai/native 正确往返并保留配置键身份；同步测试 fixture 预期。 [remote.rs:197](/Users/soddy/Documents/git-rust-work/fork-codex/codex-rs/config/src/thread_config/remote.rs:197)

23. **[P1] Guardian 固定 Luna 后台评分绕过桥接 — 已修复。** 桥接 provider 初始化时不安装原生 scorer/prewarm，保留证据、可信技能根和普通审批回退；专项测试确认无 sampler、无启用标记、无模型 HTTP。 [extension.rs:76](/Users/soddy/Documents/git-rust-work/fork-codex/codex-rs/ext/guardian-v2/src/async_scorer/extension.rs:76)

24. **[P1] Rig replay 假绿及旧接口遗漏 — 已修复。** 24 个旧 fixture 缺 custom_tools 时兼容为空；两个 replay 入口共用严格 helper，解析/文件/终止失败均报错，不能退回旧 ResponseEvent fixture 或 live。cassette 工具名也复用生产解析。 [lib.rs:377](/Users/soddy/Documents/git-rust-work/fork-codex/codex-rs/live-tests/src/lib.rs:377)

25. **[P1] exec_live 在 replay 下仍可能启动真实请求 — 已修复。** 在创建临时配置和启动进程之前拒绝 replay，避免将离线验证变成厂商调用。 [lib.rs:653](/Users/soddy/Documents/git-rust-work/fork-codex/codex-rs/live-tests/src/lib.rs:653)

26. **[P2] CI 用测试清单冒充执行且缺配置会空跑 — 已修复。** 同一运行步骤预检厂商和必要配置，补 GLM Responses URL；移除 list 假执行检查。预检证明配置齐全，实际执行仍由测试及日志证明。 [live-tests.yml:63](/Users/soddy/Documents/git-rust-work/fork-codex/.github/workflows/live-tests.yml:63)

27. **[P2] 字段审计混淆 inherent 限制与实现遗漏 — 已修复。** 重新按协议说明字段、路由、验证层次和未支持项；更正 store、ServerModel、request ID、schema、角色和 effort 等结论，并给旧方案加当前表入口。 [field-mapping-audit.md:1](/Users/soddy/Documents/git-rust-work/fork-codex/my-docs/field-mapping-audit.md:1)

28. **[P2] 巨大生产模块与薄弱断言 — 已修复。** 拆出消息、工具、reasoning 和 transport 模块，测试移到 sibling 文件；图片断言绑定 tool_use_id 并完整比较 MIME/base64，终端测试增加独立截止时间。 [wire.rs:111](/Users/soddy/Documents/git-rust-work/fork-codex/codex-rs/codex-rust-rig-bridge/tests/wire.rs:111)

## 前一版已有修复的复核记录

29. 相邻 reasoning/message/function-call 合并、AgentMessage 明文转发已在基线中，本轮搬迁时保留。 [request_messages.rs:184](/Users/soddy/Documents/git-rust-work/fork-codex/codex-rs/codex-rust-rig-bridge/src/request_messages.rs:184)

30. 跨请求 txt/rsn ID 唯一化已在基线中，本轮保留。 [convert_response.rs:23](/Users/soddy/Documents/git-rust-work/fork-codex/codex-rs/codex-rust-rig-bridge/src/convert_response.rs:23)

31. 顶层 custom wrapper 已在基线中；本轮补齐 namespace、AdditionalTools 与流事件一致性。 [request_tools.rs:72](/Users/soddy/Documents/git-rust-work/fork-codex/codex-rs/codex-rust-rig-bridge/src/request_tools.rs:72)

32. 真实 dispatcher 的 flat-name 回环修复属于前一版；本轮沿用同一展平命名。 [flat_name_index.rs:66](/Users/soddy/Documents/git-rust-work/fork-codex/codex-rs/core/src/tools/flat_name_index.rs:66)

33. 按配置键区分第一方与同名第三方已在基线中；本轮补齐构造器和远端身份。 [lib.rs:691](/Users/soddy/Documents/git-rust-work/fork-codex/codex-rs/model-provider-info/src/lib.rs:691)

34. 独立 app-server 的 Rig/GenAI features 已在基线中。 [Cargo.toml:45](/Users/soddy/Documents/git-rust-work/fork-codex/codex-rs/app-server/Cargo.toml:45)

35. 远端 Anthropic/bridge 字段传播和配置测试的枚举补齐属于前一版；本轮增加严格解析与身份预期。 [remote.rs:246](/Users/soddy/Documents/git-rust-work/fork-codex/codex-rs/config/src/thread_config/remote.rs:246)

## 验证结果

| 验证层 | 结果 | 原始日志 |
|---|---|---|
| Rig/config/provider 四 crate | 531/531，5 个测试二进制 | [日志](/Users/soddy/.codex/visualizations/2026/09/24/01a0d4bc-2211-7643-b2fa-ce1b27e70487/rig-fixes-17e336236/final-bridge-tests.log) |
| 最终 HTTP 断言追加复跑 | 11/11，1 个测试二进制；26 次 loopback HTTP 请求，另验证认证失败不发网 | [日志](/Users/soddy/.codex/visualizations/2026/09/24/01a0d4bc-2211-7643-b2fa-ce1b27e70487/rig-fixes-17e336236/wire-final.log) |
| core 请求副本/Guardian 生命周期 | 2/2，2 个测试二进制，按名称筛选 | [日志](/Users/soddy/.codex/visualizations/2026/09/24/01a0d4bc-2211-7643-b2fa-ce1b27e70487/rig-fixes-17e336236/core-guardian-tests.log) |
| Rig 离线回放 | 18/18，1 个测试二进制，24 份 fixture 均进入当前转换；25 个其他用例按过滤条件未执行 | [日志](/Users/soddy/.codex/visualizations/2026/09/24/01a0d4bc-2211-7643-b2fa-ce1b27e70487/rig-fixes-17e336236/rig-replay.log) |
| Config schema | 重生成成功，文件无差异 | [日志](/Users/soddy/.codex/visualizations/2026/09/24/01a0d4bc-2211-7643-b2fa-ce1b27e70487/rig-fixes-17e336236/config-schema.log) |
| Bazel lock | 重生成成功 | [日志](/Users/soddy/.codex/visualizations/2026/09/24/01a0d4bc-2211-7643-b2fa-ce1b27e70487/rig-fixes-17e336236/bazel-lock.log) |
| Clippy | 7 个相关 crate 执行 just fix 后，严格 just clippy 成功，无告警 | [日志](/Users/soddy/.codex/visualizations/2026/09/24/01a0d4bc-2211-7643-b2fa-ce1b27e70487/rig-fixes-17e336236/clippy-final.log) |
| just fmt | 成功；另撤回 17 个已证明仅有格式变更的无关文件，保留修改范围；git diff --check 成功 | [记录](/Users/soddy/.codex/visualizations/2026/09/24/01a0d4bc-2211-7643-b2fa-ce1b27e70487/rig-fixes-17e336236/verification.json) |

去掉重复复跑后，共 **551 个不同用例、8 个测试二进制**。最初高并发运行中出现模型目录测试超时，限制为 4 个测试线程后完整通过；未据此改动目录业务逻辑。测试还实际发现并验证了 Chat DONE 后等待 EOF 的缺陷。

执行命令（工作目录 codex-rs）：

```sh
just test -p codex-rust-rig-bridge -p codex-model-provider-info -p codex-model-provider -p codex-config --offline --test-threads=4
just test -p codex-rust-rig-bridge --test wire --offline --test-threads=4
just test -p codex-core -p codex-guardian-v2 --lib --features codex-core/rust-rig --offline -E 'test(~bridge_tests)' --test-threads=2
LIVE_CASSETTE=replay LIVE_INCLUDE_GENAI=0 LIVE_VENDORS=mimo,glm,step \
LIVE_MIMO_ANTHROPIC_URL=replay LIVE_GLM_ANTHROPIC_URL=replay LIVE_STEP_ANTHROPIC_URL=replay \
just test -p codex-live-tests --test bridge_live --offline -E 'test(~_rig_) and not test(~auth_rejected)' --no-capture
```

所有测试在最后的 lint/格式化之前执行；依照仓库指引，fix/fmt 后未重复运行测试。编译验证和自动样式修改不能替代新的运行时测试证据。

静态检查范围为 `codex-rust-rig-bridge`、`codex-model-provider-info`、`codex-model-provider`、`codex-config`、`codex-core`、`codex-guardian-v2`、`codex-live-tests`，启用 `codex-core/rust-rig`。Clippy 同时清理了两个既有 core 测试文件的一处冗余 clone 和一条未使用 import，没有修改它们的断言或行为。

## 仍保留的支持与验证边界

- 未执行真实 MiMo/GLM/Step 请求、完整 workspace 测试、Windows/Linux 运行、Bazel 全量构建或发布。没有消耗厂商配额。
- HTTP 测试验证 SDK 序列化、SSE parser 与桥；Rig-event cassette 只验证当前响应转换及场景断言，不验证发出的请求，也不是逐事件完整 golden 对比。
- Anthropic thinking budget/effort、tool strict、音频、file ID 图片、部分 Responses 控制和引用/usage 元数据仍有未实现映射；详见字段表，不能声称协议完全无损。
- reasoning 和普通文本仍按 Codex item 类型聚合；跨类型细粒度交错存在表示限制，目前未证明新增运行故障。
- 自定义 CA 已接线；真实 TLS/企业代理未验收，每轮 client 重建及完整 HttpClientFactory 动态代理策略仍可继续优化。
- GenAI 暂停扩展：其 query endpoint 拼接和 resolver 中完整 URL 日志的问题仍在。[resolver.rs:50](/Users/soddy/Documents/git-rust-work/fork-codex/codex-rs/codex-rust-genai-bridge/src/resolver.rs:50)。

## 早期整体 review 的其他记录

以下保留其他审查代理提出的安装、发布和维护问题，避免与本轮 Rig 修复混淆。这里的局部源码复核不是对应平台或发布流程验收。

36. Ask 预设的名称/审批语义风险：前一版已修正文案；本轮未运行 TUI 审批交互。 [lib.rs:51](/Users/soddy/Documents/git-rust-work/fork-codex/codex-rs/utils/approval-presets/src/lib.rs:51)

37. Windows ZIP 缺两个沙箱辅助 EXE：stable workflow 当前已列入两个 EXE；未进行 Windows 打包验收。 [release.yml:334](/Users/soddy/Documents/git-rust-work/fork-codex/.github/workflows/release.yml:334)

38. npm 先发布再上传 OSS：stable 已调整顺序；beta 仍先 npm publish，存在上传失败后留下不可安装版本的风险。 [release-beta.yml:410](/Users/soddy/Documents/git-rust-work/fork-codex/.github/workflows/release-beta.yml:410)

39. 缓存直接解压/部分安装风险：两份下载器已有 staging+rename；完整辅助资产校验、EXDEV 分支与已有缓存的验收仍应覆盖。 [nuwax-codex.js:203](/Users/soddy/Documents/git-rust-work/fork-codex/npm/bin/nuwax-codex.js:203)

40. 缓存未按 target triple 隔离：当前已包含 target triple。 [postinstall.js:45](/Users/soddy/Documents/git-rust-work/fork-codex/npm/bin/postinstall.js:45)

41. Linux ARM64：入口脚本已有支持，postinstall 仍拒绝非 x64；两份检测应统一。 [postinstall.js:25](/Users/soddy/Documents/git-rust-work/fork-codex/npm/bin/postinstall.js:25)

42. 手动发布输入 tag 不固定 build checkout：stable 发布阶段引用了 tag，但 build checkout 仍未指定 ref；beta 同样需要核对。 [release.yml:104](/Users/soddy/Documents/git-rust-work/fork-codex/.github/workflows/release.yml:104)

43. CLI bin 改名后的 cargo_bin("codex")/Bazel 引用兼容尚未完成本轮复核；不能用本次定向测试宣称所有二进制集成通过。 [Cargo.toml:10](/Users/soddy/Documents/git-rust-work/fork-codex/codex-rs/cli/Cargo.toml:10)

44. 新增桥/live-tests 的 Bazel package 接入仍需单独完成；本轮已重生成 MODULE.bazel.lock，不代表 Bazel release 构建已通过。 [BUILD.bazel:3](/Users/soddy/Documents/git-rust-work/fork-codex/codex-rs/core/BUILD.bazel:3)

45. 下载写流错误处理已补监听器，但背压与错误竞争仍需修整/验收，不能把安装容错当作已完全验证。 [postinstall.js:88](/Users/soddy/Documents/git-rust-work/fork-codex/npm/bin/postinstall.js:88)

46. fork 更新目标已有 nuwax-codex 分支；各安装方式识别与更新行为未做端到端验证。 [update_action.rs:12](/Users/soddy/Documents/git-rust-work/fork-codex/codex-rs/tui/src/update_action.rs:12)

47. 维护建议：删除根目录未参与当前 Cargo workspace 的旧 response converter。 [convert_response.rs:1](/Users/soddy/Documents/git-rust-work/fork-codex/codex-rust-rig-bridge/src/convert_response.rs:1)

48. 维护建议：清理未使用且仍包含旧平台包名的脚本。 [update-base-package.sh:28](/Users/soddy/Documents/git-rust-work/fork-codex/npm/publish/update-base-package.sh:28)

49. 维护建议：稳定/beta workflow 与两份下载器复用实现，并恢复 fork 离线 CI 门槛；避免只修其中一份。 [release-beta.yml:380](/Users/soddy/Documents/git-rust-work/fork-codex/.github/workflows/release-beta.yml:380)

50. 供应链建议：OSS 下载增加随 npm 发布的完整性摘要；本轮没有发现实际篡改证据。 [postinstall.js:59](/Users/soddy/Documents/git-rust-work/fork-codex/npm/bin/postinstall.js:59)

51. 双 reqwest 的类型边界本轮未发现串用；GenAI 也使用 0.13，不能描述成仅 Rig 引入新版本。 [Cargo.toml:17](/Users/soddy/Documents/git-rust-work/fork-codex/codex-rs/codex-rust-genai-bridge/Cargo.toml:17)

## 建议的提交拆分

本轮包含大批模块/测试搬迁，整体变更超过仓库 800 行建议。建议按以下依赖闭合的阶段审阅提交：

1. 机械模块拆分与测试搬迁。
2. HTTP URI、认证、CA、request ID、错误与终端处理及 wire 测试。
3. reasoning 保存/回放、来源隔离及 native/GenAI 切换，writer 与 reader 同时落地。
4. 工具请求 wrapper 与响应还原同时落地。
5. provider/config 路由与 compaction/Guardian 辅助入口。
6. cassette、CI 与审计文档。
7. 发布/npm/Bazel 的独立后续修复。

不应把请求包装与响应解包、新 envelope 与回放过滤、新模块与旧模块删改拆成缺少另一半的可发布阶段。

## 复审修复(round 2,同日)

对 90bcbd2e2 独立复审(三路并行审查 + 全链路重验:clippy 清零、单元 549/554 仅存基线即有的 models_endpoint 环境 flake、replay 43/43、在线矩阵 64/64)后修复:

52. **[P1] 历史 FunctionCall 坏 JSON 永久卡死会话 — 已修复。** 历史回放中不可解析的 arguments 改为 warn+跳过该调用及其配对输出(空参数按 `{}` 回放);历史是已记录数据,不再作为新输入 fail-fast。[request_messages.rs](/Users/soddy/Documents/git-rust-work/fork-codex/codex-rs/codex-rust-rig-bridge/src/request_messages.rs)

53. **[P2] 不可解码 data:URL 图片在 Anthropic 线必 400 — 已修复。** 协议感知:Anthropic 丢弃(其 URL source 仅接受 http(s));Chat 合法透传原始 data:URL 并如实告警。此前降级为 `Url("data:...")` 外发,与告警文案矛盾。

54. **[P2] 多块 reasoning envelope 回放违反 Anthropic 单 thinking 块限制 — 已修复。** Anthropic 线仅回放首块并告警;Chat 线保持全部回放。

55. **[P2] 工具文本 8,000-token 截断低于 core 默认 10,000 输出预算 — 已修复。** 安全网上调至 24,000 token(core 预算的 2.4 倍),合法输出不再被二次截断,base64 洪水防护依旧有效。

56. **[P2] reasoning 来源不匹配静默丢弃 — 已修复。** 补 tracing::warn(含 envelope 来源摘要),换端点/模型后推理回放失效可排障。

57. **[P2] envelope 前缀字面量跨 crate 重复 — 已修复。** 桥 crate 导出 `REPLAY_PREFIX`/`is_replay_envelope`;core 在 rust-rig feature 下委托,其余构建保留镜像并由 `rig_reasoning_envelope_prefix_is_v1_mirror` 测试守护同步。

58. **[P2] custom 工具缺描述回退丢失 / 工具每请求解析两次 — 已修复。** 恢复 input 字段使用指引回退;`ToolMeta` 随转换结果返回,stream 层不再二次解析。

新增测试 5 个(坏历史跳过、空参数、双协议 data:URL、Anthropic 单块、描述回退),桥 crate 51/51。
