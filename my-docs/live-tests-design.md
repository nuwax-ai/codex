# live-tests 模块设计(Plan 层文档)

> 对象:`codex-rs/live-tests`(独立集成测试 crate,不与任何业务 crate 耦合)。
> 定位:用**真实模型厂商**验证 fork 的桥接层与编译产物,是发布前的最后一道验收。
> 关联:`rig-bridge-implementation-plan.md`(桥本体)、`openai-responses-chat-bridge.md`(协议映射)。

## 1. 设计原则(为什么这样设计)

1. **单一职责、零耦合**:测试逻辑全部收在本 crate;被测物(两个桥 crate、codex-exec
   二进制)不知道测试的存在。加厂商 = 改 `.env.local`,不改代码。
2. **测用户真正运行的东西**:L3 套件只测**编译出来的 codex-exec 二进制**,不测进程内
   函数——配置解析、feature 组合、链接产物全部被真实覆盖。
3. **不可伪造的验收信号**:marker 技术(随机 nonce → 模型必须真实执行 `echo` →
   exit 0 + nonce 出现在命令聚合输出)。主判据是本地执行证据;模型转述是加分项
   (MiMo 偶尔偷懒不转述,不应连坐)。
4. **失败可回溯**:每次运行落盘完整产物(events.jsonl / final_message / stderr /
   桥层逐事件日志),失败现场永不丢失。
5. **无凭据即绿**:未配置 key 时全部跳过,CI 与本地无密环境零干扰。

## 2. 三层验证金字塔

| 层 | 套件 | 测什么 | 失败定位 |
|---|---|---|---|
| L1 桥层 | `tests/bridge_live.rs`(7) | 协议转换正确性:事件顺序、增量重组、end_turn、usage;两桥 A/B | 桥的 convert 代码 |
| L3 二进制层 | `tests/exec_live.rs`(7) | 完整 agent 闭环:配置解析 → 分派 → 工具执行 → 多轮回放 → 默认桥策略 | core 集成/配置/二进制构建 |
| (未来 L2) | — | codex-core 会话级(进程内 Session),mock provider 已由 upstream `core/tests/suite` 覆盖,暂不重复 | — |

分派矩阵(L3 覆盖 fork 全部路由分支):

| 用例 | wire_api | experimental_bridge | 期望路径 |
|---|---|---|---|
| chat-genai / chat-rig | chat | 显式 genai / rig | 对应桥 |
| chat-default | chat | 不配 | **rig**(fork 默认) |
| responses-rig-default | responses | 不配 | **rig**(第三方默认走桥) |
| responses-native | responses | "native" | 原生(逃生舱) |
| anthropic-genai / anthropic-rig | chat + /anthropic URL | 显式 | 对应桥的 Anthropic 路由 |

## 3. 厂商模型(vendor 抽象)

```
.env.local(.env.example 为模板,gitignored)
LIVE_VENDOR_NAME=mimo                        # 厂商标识,决定产物目录 logs/live-<vendor>/
LIVE_VENDOR_API_KEY=…                        # 必填,缺失 → 全部跳过
LIVE_VENDOR_CHAT_URL=…                       # 必填(非 mimo 厂商);mimo 有内置默认
LIVE_VENDOR_ANTHROPIC_URL=…                  # 可选;缺省 → anthropic 套件跳过(绝不跨厂商回退)
LIVE_VENDOR_MODEL=…
```

规则:**任何 URL 都不做跨厂商回退**——厂商 A 的 key 发到厂商 B 的端点是最危险的
静默错误。回退链仅 `LIVE_VENDOR_* → MIMO_*(历史兼容)`。

## 4. 稳定性策略(与真实模型共处)

- **主判据抗模型方差**:命令执行证据(exit 0 + marker)为主,转述为辅(已吸收
  MiMo "跑完工具不总结" 与 "不调工具直接输出文本" 两个真实方差)。
- **重试**:依赖仓库 nextest 配置的自动重试(网络/模型抖动),主断言保持严格。
- **二进制新鲜度守卫**:`codex_exec_binary()` 对 binary mtime 与桥/core 源码 mtime
  做比较,过期即打印醒目警告——"测了旧二进制"曾经真实咬过我们(unknown variant)。
- **超时**:每事件 180s、整进程 300s,防挂死;日志提示重建命令,缺失即失败
  (fail-fast,不静默跳过)。

## 5. 产物目录(排查体验)

```
logs/live-<vendor>/                     # gitignored(/logs/)
├── bridge/<tag>-<nonce>.log            # L1:每个 turn 的逐事件 Debug 流(两桥可 diff)
├── mimo-<scenario>-<nonce>-<pid>/      # L3:每次运行
│   ├── events.jsonl                    #   codex-exec 完整事件流(核心排查文件)
│   ├── final_message.txt               #   模型最终回答
│   └── stderr.log                      #   RUST_LOG=info 进程日志(含 dispatch 分派记录)
└── (无索引时的约定:ls -t 第一个即最新)
```

## 6. 演进路线(按需实施,当前均未阻塞)

| # | 演进项 | 动机 | 形态 |
|---|---|---|---|
| E1 | 多厂商矩阵 | **已实现(2026-09-24)**:`LIVE_VENDORS=mimo,glm` + `bridge_matrix!`/`exec_matrix!` 宏(paste)按 厂商×桥×场景 生成测试 fn;30 用例全绿。加厂商 = .env 一段 + 宏列表加一个名字 |
| E2 | 能力声明 | 部分实现:anthropic 网关(可选 URL→跳过)、responses 端点(可选独立 URL,GLM 与 chat 不同源)已按"可选即能力"落地;reasoning/tools 能力开关待真正需要时再加 |
| E3 | 运行清单 | 知道某次产物测的是什么代码 | 每次运行写 `manifest.json`(git rev、binary mtime、vendor、场景);E1 前置收益最大 |
| E4 | 产物保留策略 | logs 无限增长 | 保留最近 N 次运行的清理钩子(测试开始时执行) |
| E5 | rig 桥单测移植 | 23 个 genai 桥单测的 rig 版 | 纯离线,补齐 rig 桥的转换边角覆盖(当前由 L1 live 兜底) |

## 7. 运行手册

```bash
cargo build -p codex-exec --bin codex-exec   # L3 前置(改桥/core 后必须重建)
cargo nextest run -p codex-live-tests        # 14 用例
cargo nextest run -p codex-live-tests --no-capture   # 查看逐事件输出与产物路径
```

## 8. 社区工具调研(2026-09-24)

Rust 社区**没有**成熟的"live LLM/agent E2E 测试框架"——本 crate 的手写 harness
填补的正是这个空缺,核心资产(不可伪造 marker、分派矩阵、产物落盘)没有现成替代。
但以下工具值得按需接入,均与现有结构正交:

| 工具 | 是什么 | 接入价值 | 建议 |
|---|---|---|---|
| **rig-cassette**(rig 自带,workspace 内) | HTTP 录制/回放(VCR) | 录一次 MiMo/GLM 真实流 → 离线确定性回归桥转换逻辑,不怕限流/厂商改版;rig 自己的 fixture 语料就这么做的 | **首选**,下个迭代评估其公开 API 能否挂到我们的桥传输层 |
| **insta** | 快照测试 | 对事件流"形状"(事件种类序列)做脱敏快照,L1 加离线回归层 | 次选,与 cassette 二选一或并用 |
| **wiremock**(upstream 已用) | mock HTTP | upstream 套件在用;live 场景不适用 | 不动 |
| **cargo-nextest**(已用) | 测试运行器 | retries 已在吸收 flake;后续可用 test group 把 live 套件限并发,防厂商限流 | 按需 |
| promptfoo / LangSmith / Braintrust | JS 生态/托管评测 | 不匹配 Rust 嵌入式需求 | 不引入 |

原则:框架不引入,胶水保持自有——我们的核心竞争力是"验证 codex 全链路"这件事
本身,通用评测工具覆盖不了。

## 9. rig-cassette 评估结论与优化总盘点(2026-09-24)

### 9.1 rig-cassette:暂不接入(三个硬事实)

1. **版本线断裂**:crates.io 上只有 0.0.1(独立实验版本),不在我们锁定的
   rig-core v0.42.0 tag 内——它是 tag 之后新增的 crate。
2. **绑定 rig HEAD 架构**:其抽象(EffectLog、provider cassette engine、
   agent/ECS runtime 集成)面向 rig 自家 0.42 之后的 Wire/Bound/effects 体系,
   接它 = 接触我们刻意回避的不稳定层。
3. **覆盖面不匹配**:它录的是 rig 传输层;我们的核心资产是
   "ResponsesApiRequest → ResponseEvent" 的桥边界,且 genai 桥它覆盖不了。

**替代方案(推荐):桥边界自制 cassette** —— 在我们自己的公共 API 面上录制回放:

- `LIVE_RECORD=1` 时 run_turn 把 (request, events) 落成 JSON fixture
  (`tests/fixtures/<vendor>/<bridge>-<scenario>.json`);
- 回放模式无网络构造同样的事件流,**跑同一套断言**——桥重构/rig 升级后全量
  回放即可验证转换逻辑等价性;
- 两桥通用、断言零重复、不依赖 rig 内部;
- 前置小改动:`ResponseEvent` 补 `Serialize, Deserialize` derive(目前仅 Debug);
  估算 ~150 行 + 每场景录一次。

### 9.2 rig-bridge 优化盘点(按优先级)

| 级 | 项 | 动机 | 估算 |
|---|---|---|---|
| P0 | 错误路径 L1 场景:坏 key → 断言 `Http{401}` 透传 | `map_completion_error` 的状态码提取**当前零测试**;401 触发 core 重登录循环是关键链路 | 半天 |
| P0 | 桥边界 cassette(9.1) | 离线确定性回归,防 rig 升级/桥重构回归 | 1 天 |
| P1 | Anthropic 工具两轮场景(thinking 签名回放) | anthropic 桥最经典的断裂点;当前 anthropic 只测了纯对话 | 半天 |
| P1 | 并行工具调用场景(一轮两个 function call) | 多 internal_call_id 累积器逻辑未被真实验证 | 半天 |
| P1 | usage 合理性不变量(input>0、total≥input+output) | 一行断言,捕获 usage 映射错位 | 0.5h |
| P2 | 丢弃宿主工具从 debug 提到 warn + 每次 dispatch 带 span | 兼容性静默损失可见化 | 1h |
| P2 | E5:genai 桥 23 个单测的 rig 版移植 | 离线覆盖转换边角(老 uint 边界等) | 1 天 |
| P3 | HTTP 客户端复用(当前每轮新建 = 每轮 TLS 握手) | 长会话性能;受 rig 无 per-request header 限制,**等 v0.43+ 传输层稳定做原生传输时一并解决** | 阻塞中 |

### 9.3 live-tests 优化盘点

| 级 | 项 | 动机 | 估算 |
|---|---|---|---|
| P0 | E3:每次运行写 manifest.json(git rev、binary mtime、vendor)+ INDEX.md 汇总(每测试 pass/fail + 产物链接) | logs/ 从"目录堆"变成"可浏览报告";排查直接定位 | 半天 |
| P1 | A/B 自动 diff:两桥同场景事件**种类序列**对比,不一致即报 | 把"两桥可对比"从可能变自动 | 半天 |
| P1 | CI nightly:GitHub Actions manual-dispatch + LIVE_* secrets 跑全矩阵 | 厂商改版/限流当日发现 | 半天 |
| P2 | E4:产物保留策略(留最近 N 组) | logs 无限增长 | 1h |
| P2 | test-group 限并发(nextest groups) | 多厂商并发打限流 | 1h |
