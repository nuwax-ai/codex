# 给 Codex 复审：Rig 0.44 候选三轮开发 + ACP/记忆修复 + 付费录制收据

日期：2026-10-16。审查范围 `29d55354c..73f8fcb06`（12 个提交，全部在
`codex/rig-044-20261014`，未 push、未合回）。工作树干净。本文件是复审
请求与自曝弱点清单；详细过程见
[rig-044-migration-notes.md](rig-044-migration-notes.md) 第五至十一轮。

## 提交地图

| 提交 | 内容 |
|---|---|
| `6c2d2c562` | fix(rig): 修复 0.44 下 cap 终态被截断工具覆盖 + 严格 usage 恢复 |
| `c0d07a7a2` | test(rig): 生命周期证据（图片历史三 wire + v2 cassette 往返）+ 两个回放保真修复 |
| `2f048c3b4` | feat(core): 工具输出图片入口硬限（额度表见下） |
| `4ca0e4fa0` | test(core): 20 截图压缩边界 stress |
| `112501442`/`1815028ed`/`f2cb2929a` | docs: 三批进度 + Linux/Windows 平台收据 |
| `f6b1ad38f` | fix(guardian): 第三方 provider 审查用当前活动模型 |
| `a2a6f5aae` | docs: ACP dev 联调收据（六场景 + 真实识图） |
| `d115bfdbe` | fix(memories): 提取/巩固模型同类修复 + guardian-v2 两处 |
| `45abfd28e` | test(live): 三厂商付费录制，150 个 v2 fixture |
| `73f8fcb06` | docs: 第十一轮收据 |

## 对上轮复审的一处更正（请先核）

上轮「Core cap 含原生对照 30/30」是**假通过**：复审沙箱设置了禁网
env，`skip_if_no_network!` 全部提前 return（复审日志 30 项均 0.02s）。
真实执行同选择器 7 项失败，均为 0.44 迁移真实缺陷，`6c2d2c562` 修复：
(1) rig 0.44 以无 native End 关闭被输出预算截断的工具调用，桥的严格
invalid-arguments 检查把 cap 终态替换成可重试流错误（还会耗尽单发
mock）；(2) 0.44 宽容 usage 读取（as_u64/null 兜底）丢失 0.42 的
"invalid value: integer"/"missing field `usage`" 可重试契约，wire 级
观测器恢复严格性。请复核我对该 env 跳过宏的判断与两处修复语义。

## 重点复审项（按风险排序，含自曝弱点）

1. **第三方 provider 模型选择三连修**（f6b1ad38f + d115bfdbe）：
   guardian 审查、guardian-v2 两处、memory 提取/巩固，统一规则——
   显式 override 恒优先；`config.model_provider_id == "openai"` 保留
   目录首选；否则用当前活动模型。弱点：判定信号用的是
   `model_provider_id` 字符串等值，未核对 provider.info().provider_id
   的一致性；代理 OpenAI 目录的网关型 provider（chatgpt auth）会被
   降级到活动模型（行为保守但可接受？）；memory 侧无活动模型时最后
   兜底仍是目录别名（可能再 400）。memories/write 的 Cargo.toml 把
   codex-model-provider-info 从 dev-deps 移到 deps。
2. **图片入口硬限**（2f048c3b4）：实施在 output-truncation 条目截断
   （MCP 与 Code Mode 共同路径，零新增传参）+ 提交期幂等兜底。语义
   变化：图片按 1,844 视觉 token 预扣同一预算（original 走 patch 记账
   豁免）；超限保最新、单条有界省略标记；wall-time 头部移到截断之后
   （不再被图片挤掉）。三个既有测试按新语义更新（truncate_tests 的
   tiny-policy 图片零成本钉住、formatted 合并、step_settings 小预算）。
   弱点：metadata 大小上限未做成显式常量；Code Mode 普通 JS return /
   text() 负控未新增测试（勘路确认合同成立）；该批 891 insertions
   略超"普通批 <800"指引（其中约六成为测试）。
3. **v2 fixture 覆盖**（45abfd28e）：150 文件以 0.44 格式（items+
   terminal）替换 v1 载荷。录制轮 84/84 真实付费绿（三厂商三 wire）；
   录后离线 replay+roundtrip 91/91。自扫凭据零泄漏（认证头名 + 三个
   真实键值两轮扫描），建议独立复核。注意 replay 的 PASS 计数含
   early-Ok 内部跳过（既有语义，勿当全场景执行数）。
4. **ACP dev 联调**（收据在 a2a6f5aae）：驱动脚本在 ACP 仓库 `tmp/`
   （gitignored，未提交——若需留档请告知）。六场景：环境变量传递、
   截图工具真实执行、**真实识图**（纯红 64×64 → "red"，依赖 guardian
   修复）、取消、/compact 压缩、杀进程冷恢复召回令牌。识图成功同时
   证明 guardian 修复在生产链路生效。

## 平台与环境定性（差分证据在 notes 第八/九轮）

- mac worktree startup 簇（mcp grace/realtime 等 8 项）：/tmp 同提交
  基线全过 0.2-0.7s、本 worktree 稳定 5s 超时——/Volumes 卷 IO。
- Linux `anthropic_interrupt_during_tool_runs_it_once`：Ubuntu 24.04
  AppArmor 限 userns（`unshare -Urn` 同败），bwrap 无法建 loopback。
- Windows 同测试：shell 工具走 exec approval（夹具按 Unix 沙箱语义）。
- guardian-v2 `pool_pressure` + flaky `stalled_handshake`：/tmp 基线
  worktree 复现，先于两修复存在。

## 验证收据（真实执行，均 `--locked --retries 0`）

| 范围 | 结果 |
|---|---|
| bridge 全 crate（mac） | 299/299 |
| 显式 rig core 家族 + rig_image（mac） | 50/50 |
| utils image/truncation | 15/15、29/29 |
| memories-write / guardian-v2 相关 | 54/54 / 目标套件绿 |
| Linux 主验 | bridge 299/299；core 68/70（2 环境性）；replay 195/195；nuwax 3/3 |
| Windows 原生 | bridge 297/297；core 58/59（1 环境性）；replay 187 有效；nuwax 3/3；stress PASS |
| 付费录制 / 录后离线 | 84/84 / 91/91 |
| ACP dev（MiMo+GLM） | 关键场景 6/6 + 识图 PASS |

## 未闭环（非本批代码）

全量 workspace/CI；代理/TLS；`anthropic_interrupt` 跨平台夹具适配；
guardian-v2 两个既有失败；短生命周期 `codex exec` 退出杀后台巩固
agent（`failed_agent`，长驻宿主不受影响，新发现待定性）；合回 test
与 push 待授权。

## 复审建议命令

```bash
just test -p codex-rust-rig-bridge --locked --offline --retries 0
just test -p codex-core --features rust-rig,rust-genai --test all --locked \
  --offline --retries 0 -E 'test(rig_) + test(mcp_tool_output_caps) + test(twenty_synthetic)'
just test -p codex-utils-image -p codex-utils-output-truncation -p codex-memories-write --locked --offline --retries 0
LIVE_CASSETTE=replay LIVE_VENDORS=mimo,glm,step just test -p codex-live-tests --locked --offline --retries 0
```

注意本机（mac worktree）的 startup 簇会因卷 IO 假红，用 /tmp 基线或
Linux 复核该簇。
