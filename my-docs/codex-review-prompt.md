# Codex Fork 整体 Review 提示词

> 用途:交给 AI agent(或新同事)对 nuwax-ai/codex fork 做整体方案与代码审查。
> 仓库:`/Users/soddy/Documents/git-rust-work/fork-codex`(branch: test,已发布 npm `nuwax-codex@0.17.9`)。

---

## 一、项目背景与目标

这是 **OpenAI codex CLI 的 fork**(基线:upstream 主线 2026-09-24,commit 29f056c26 附近),
核心使命:**让 codex 能接入国内/第三方大模型厂商**(小米 MiMo、智谱 GLM、阶跃 StepFun、
DeepSeek、Ollama 等),并通过 npm 以 `nuwax-codex` 包名独立发布。

上游 codex 已删除 Chat Completions 支持(只保留 Responses API),且第三方厂商的
Responses 实现普遍残缺(如 MiMo 拒绝 `web_search` 宿主工具)。本 fork 的解法:
**自建"桥"(bridge)层**,把 codex 内部的 Responses-API 形态请求转换为 Chat
Completions(或 Anthropic Messages)协议发给厂商,再把流式响应转回 codex 的
`ResponseEvent`,使上游全部消费方(TUI/exec/app-server)零改动。

## 二、模块清单:哪些是我们自己开发的

### 纯 fork 新增 crate(上游完全没有)

| 路径 | 作用 |
|---|---|
| `codex-rs/codex-rust-rig-bridge/` | **默认桥**。基于 rig-core =0.42.0(精确锁版本),把 `ResponsesApiRequest` 转成 rig `CompletionRequest`(OpenAI Chat Completions 或 Anthropic Messages),流式转回 codex `ResponseEvent`。核心文件:`convert_request.rs`(请求映射)、`convert_response.rs`(响应/流事件映射)、`stream.rs`(流泵+错误映射)、`client.rs`(rig 客户端构造)、`bridge_impl.rs`(trait 实现) |
| `codex-rs/codex-rust-genai-bridge/` | 备用桥(基于 rust-genai 0.6.5)。**已降级为回退选项**,配置 `experimental_bridge = "genai"` 才启用。结构与 rig 桥同构 |
| `codex-rs/live-tests/` | 独立集成测试 crate(不耦合业务代码)。三厂商(MiMo/GLM/StepFun)× 两桥 × 多场景的真实模型测试矩阵 + 编译产物 E2E + 录制回放 cassette + A/B diff。凭据从 gitignored 的 `.env.local` 读 |

### 对上游文件的侵入式修改(fork 补丁面,合并冲突高发区)

| 文件 | 改动 |
|---|---|
| `codex-rs/model-provider-info/src/lib.rs` | ① 重新加回 `WireApi::Chat`(上游已删),② 新增 `WireApi::Anthropic`(显式声明 Anthropic 协议,解决 StepFun 等 URL 无标记的网关),③ 新增 `ChatBridge` 枚举(`rig`默认/`genai`/`native`逃生舱)+ `experimental_bridge` 配置字段 |
| `codex-rs/core/src/client.rs` | ① `stream_chat_api()`(fork 的 chat 路径入口,feature 门控),② `dispatch_chat_bridge()`(经 `&dyn ChatModelBridge` 分派,见下),③ `responses_routes_via_chat_bridge()`(**fork 关键策略:第三方厂商的 responses-wire 默认也走 rig 桥**,只有第一方 OpenAI 和 Bedrock 走原生),④ WireApi 分派臂扩展 |
| `codex-rs/codex-api/src/bridge.rs` | **中立 trait 定义**:`ChatModelBridge`(对象安全)+ `ChatWireProtocol` 枚举 + `chat_wire_protocol()`(URL 嗅探 `/anthropic` 的唯一收敛点)。两桥实现此 trait,core 不感知桥的具体类型 |
| `codex-rs/codex-api/src/common.rs` | `ResponseEvent` 补了 `Serialize, Deserialize`(cassette 需要);`SafetyBuffering` 补 Serialize;导出 `TextFormat` 等 |
| `codex-rs/core/src/tools/flat_name_index.rs`(新增)+ `registry.rs`(小改) | namespace 工具展平为 `mcp__ns__tool` 后的回环索引 |
| `codex-rs/core/src/session/turn_context.rs`、`models-manager/model_info.rs` | fallback 模型元数据告警降噪(自定义厂商必然 fallback,不该每轮弹警告) |
| `codex-rs/config/src/thread_config/remote.rs` | 新字段初始化 |
| `codex-rs/cli/Cargo.toml`、`codex-rs/exec/Cargo.toml` | 启用 `rust-genai` + `rust-rig` 双 feature |
| `npm/`(bin/postinstall/package.json) | npm 包 `nuwax-codex`,postinstall 从阿里云 OSS 下载平台二进制 |
| `.github/workflows/release.yml`、`live-tests.yml` | 发版 CI(tag 触发,6 平台)+ live 测试 CI(manual/nightly) |
| `.config/nextest.toml` | **按厂商分组的测试并发治理**(见下文限流数据) |

## 三、核心设计决策(审查时请验证合理性)

1. **分派策略**(core/src/client.rs):
   - `wire_api = "chat"` → 走桥;URL 含 `/anthropic` 自动切 Anthropic Messages
   - `wire_api = "anthropic"` → 显式走桥的 Anthropic 路由(StepFun 类网关)
   - `wire_api = "responses"` + 第一方 OpenAI/Bedrock → 原生(WebSocket/宿主工具/SigV4 不能丢)
   - `wire_api = "responses"` + **其他所有厂商 → 默认走 rig 桥**(厂商 Responses 实现残缺,桥转 Chat 更兼容;桥会丢弃宿主工具并 warn)
   - `experimental_bridge` 可显式选 `"rig"`(默认)/`"genai"`/`"native"`(强制原生,逃生舱)
2. **trait 隔离**:`codex-api::ChatModelBridge` 是唯一契约,两桥以单元结构体实现;
   core 的 `dispatch_chat_bridge` 只见 `&dyn ChatModelBridge`。
3. **rig 版本策略**:`rig-core = "=0.42.0"` 精确锁定(rig 上游高频 breaking,0.42 之后
   5 周 44 个破坏性变更);升级只动 rig-bridge 一个 crate。
4. **双 reqwest 共存**:workspace 其他部分用 reqwest 0.12,rig-bridge 以重命名依赖
   `reqwest_rig`(package = "reqwest", version = "0.13")使用 rig 内嵌的 0.13,类型不跨界。
5. **事件契约**(桥必须保证,测试断言钉死):`OutputItemAdded` 先于增量;
   `OutputItemDone(Reasoning)` 先于 `OutputItemDone(Message)`;恰好一个 `Completed`;
   工具参数最终值与流式增量拼接**字节一致**;401/5xx 必须以 `Http{status}` **启动错误**
   形态返回(rig 会把 HTTP 失败延迟到流内,桥做了"急切拉取首个流事件"修复,否则
   core 的重登录循环永不触发)。
6. **字段映射审计**:`my-docs/rig-bridge-implementation-plan.md` §12 有逐字段权威表
   (✅ 映射 / ❌ Chat 协议 inherent 丢失)。已知 inherent:phase、reasoning summary、
   store、include、usage_metadata、ServerModel/RateLimits 事件等。

## 四、测试体系(全在 codex-rs/live-tests)

- **L1 桥层**(`tests/bridge_live.rs`):厂商×桥×场景矩阵,协议不变量断言 + 坏 key 401 场景
- **L3 二进制层**(`tests/exec_live.rs`):编译出的 codex-exec 跑不可伪造 marker 闭环
  (模型必须真实执行 `echo <随机nonce>`,exit 0 + nonce 在聚合输出中)
- **A/B diff**:两桥同场景事件种类序列对比(稳定场景严格;工具场景为"模型非确定性
  族"跳过种类 diff,结构不变量仍由场景断言保证)
- **cassette**:`LIVE_CASSETTE=record` 录制 fixtures → `replay` 无凭据离线回归(0.1s)
- **厂商限流实测数据**(并发治理依据):MiMo 账户级**并发≤5**(`concurrency reached,
  current: 6, limit: 5`);GLM coding plan **请求频率配额**(code 1302)。
  对策:.config/nextest.toml 按厂商分组(`test(~vendor)` 正则——注意 `test(name)` 是
  精确匹配),组内串行、组间并行,重试 3 次/15s 退避
- 运行:`cargo build -p codex-exec --bin codex-exec && cargo nextest run -p codex-live-tests`

## 五、审查重点(请重点检查这些)

1. **正确性**:
   - rig 桥的 convert_request/convert_response 映射是否正确(对照 rig-core 0.42.0 源码,
     位于 `~/.cargo/registry/src/*/rig-core-0.42.0/`;另有完整 git 克隆在
     `/Users/soddy/Documents/git-workspace/rig`,但注意其 HEAD 领先 0.42.0 tag 一百多个
     提交,以 registry 里的 0.42.0 为准)
   - 事件契约五条(见三.5)是否有遗漏场景(如:并行工具、流中断、超长参数)
   - `stream_via_rig` 的急切首事件 + mpsc 泵逻辑:`next_event` 预取模式是否有竞态或
     事件丢失/乱序风险;`Created` 合成的位置
   - core 的分派逻辑:`responses_routes_via_chat_bridge` 对 Bedrock/ollama/lmstudio
     内置 provider 的判定是否安全(`is_openai()`/`is_amazon_bedrock()` 按名称匹配,
     用户自定义同名 provider 会不会误判)
2. **安全**:
   - 凭据只在 `.env.local`(gitignored)和临时 CODEX_HOME;确认无泄漏到代码/fixtures/日志
   - `write_config_toml` 把 bearer token 写进临时目录 config.toml(测试用,进程结束后
     TempDir 自动删除)——是否可接受
   - rig 桥 `client.rs` 的 `api_key_from_auth()` 剥离 Bearer 前缀逻辑
   - 双 reqwest 版本的供应链面
3. **上游合并面**:侵入式修改清单(见二)在下次 upstream 同步时的冲突风险;桥 crate 是否
   真的做到了 core 之外零耦合
4. **性能**:已知遗留——rig 桥每轮新建 reqwest 0.13 客户端(每轮 TLS 握手),受 rig 无
   per-request header 限制,记录在 backlog 等 rig 传输层稳定后做原生传输
5. **测试真实性**:A/B diff 的"模型非确定性族"白名单是否会掩盖真回归;
   cassette fixtures 是否含敏感信息
6. **文档一致性**:`my-docs/` 下四份文档(rig-bridge-implementation-plan、
   live-tests-design、openai-responses-chat-bridge、rust-genai-integration-analysis)
   与代码是否一致

## 六、关键文档索引(都在 my-docs/)

- `rig-bridge-implementation-plan.md` — rig 桥完整方案 + §12 字段映射权威审计表
- `live-tests-design.md` — 测试体系设计 + 厂商限流实证 + 三批交付记录
- `openai-responses-chat-bridge.md` — Responses↔Chat 协议映射参考(genai 桥时代,
  大部分仍适用)+ P0-P3 待办(DeepSeek R1 reasoning 剥离等)
- `rust-genai-integration-analysis.md` — genai 集成分析(历史文档)

## 七、已知遗留(非阻塞,有记录)

- rig 桥 HTTP 客户端每轮新建(P3,等 rig 传输层)
- GLM anthropic 网关对 genai/rig 两适配器请求形态的响应差异(部分在"模型非确定性族"
  白名单;rig 桥已修 `parallel_tool_calls` 泄入 anthropic 线导致 GLM 禁 thinking 的问题)
- `my-docs/openai-responses-chat-bridge.md` 的 P1-P2(DeepSeek reasoning 剥离策略、
  音频输入、usage 兜底合成等)未实施
