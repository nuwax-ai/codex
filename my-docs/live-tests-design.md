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
