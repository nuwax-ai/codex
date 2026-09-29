# Qoder 实施方案核实与修正（2026-09-29，第三轮）

核实对象：`fork-fix-implementation-plan-2026-09-29.md`（Qoder 著）及其对 round2 的
两项"纠正"。方法：逐锚点读码 + 今日 live 日志作证。**结论：方案整体可执行，
但它的"纠正 1"本身是错的（需撤回对 round2 的指控），"纠正 2"成立且需扩大。**

---

## 一、对 Qoder 两项"纠正"的核实

### 纠正 1（"日志串臆造"）——**推翻，Qoder 错了** ❌

Qoder 断言 `"Dispatching chat stream via rig"` 全仓零匹配、round2 的 N1 锚点是臆造。
**实测该串真实存在**：

- 源码：`codex-rs/codex-rust-rig-bridge/src/stream.rs:169`——`tracing::info!(...
  "Dispatching chat stream via rig")`，桥每次分派时发出（含 model/protocol/
  message_count/tool_count 字段）。
- **live 铁证**（今日 01:45，Step anthropic 场景 stderr 原文）：
  `codex_rust_rig_bridge::stream: Dispatching chat stream via rig
  model=step-5-preview protocol=Anthropic message_count=4 tool_count=12`
  —— info 级、`RUST_LOG=info` 下可达 stderr，桥路径的可靠标记。

Qoder 大概率把 grep 范围限定在了 `core/src/client.rs`（分派确实在那里、确实无日志），
漏扫了桥 crate。**round2 的锚点没有臆造**；N1 测试若走 live/exec 路径完全可以用它断言。

不过 Qoder 换掉的方法（结构性证明）**仍然是对的选择**——core 集成测试的 harness
不捕获 tracing 输出，断言日志串在 suite 测试里不可行。**保留方案的测试方法，
但"纠正 1"的指控与理由（"零匹配"）必须从方案里删除**，否则错误指控会留在文档里。

### 纠正 2（错名 4 处）——**成立，且实际比 Qoder 说的还多** ✅

实测 `grep -rn responses_routes_via_chat_bridge my-docs/`（排除复核文档自身）：
- `FORK.md:50`、`codex-review-prompt.md:37`、`codex-review-prompt.md:97`、
  `rig-bridge-implementation-plan.md:140` —— **4 处确认**（round1/rebuttal 说
  "两处"是漏数，Qoder 对）。
- **旧符号超出 Qoder 清单**：`codex-review-prompt.md` 里 `ChatModelBridge`/
  `ChatWireProtocol`/`dispatch_chat_bridge` 出现在 **:37、:38、:58、:59 四行**
  （Qoder 只列了 :37 两处）；且 **:37 的 `stream_chat_api()` 也是死符号**
  （`grep stream_chat_api core/src/client.rs` 零匹配）。
- `rig-bridge-implementation-plan.md:138` 还有第三处过时结论："桥在转换时自动丢弃
  不支持的宿主工具，用户无需任何 workaround"——现状是 hosted 工具翻译表
  （web_search → Anthropic 服务端工具），不是纯丢弃。盖废止戳可一并解决。

## 二、方案其余锚点抽验（全部通过）

| 方案锚点 | 实测 | 结论 |
|---|---|---|
| 无桥 feature 时 Fatal | `client.rs:2395-2403` `CodexErr::Fatal("...enable rust-rig or rust-genai...")` | ✓（方案写 2393-2400，实际略偏，无碍） |
| `ModelBridge` trait 位置 | `codex-api/src/bridge.rs:62-64` | ✓ |
| 真实分派符号 | `stream_model_bridge`/`dispatch_model_bridge`/`uses_model_bridge()` 均在 | ✓ |
| C1 顶层副本方向 | 顶层 330 行 = 早期原型（旧 PendingRigMessage），零构建引用 | ✓（前轮已 /usr/bin/diff 实证） |
| Batch 2 样板 | `rig_responses_bridge.rs` 确有 `#![cfg(feature="rust-rig")]` + 手写 provider + mount_response_sequence/path 断言 | ✓ |

## 三、实施方案修正与执行决策（最终版）

在 Qoder 方案上做 **4 处修正**后即可执行：

1. **Batch 0（新增，先做）**：修改方案文档自身——删除"纠正 1"的错误断言与对
   round2 的"臆造"指控，改记："日志串存在于 stream.rs:169（live 已验证），
   core 集成测试因 harness 不捕获 tracing 改用结构性证明"。
2. **Batch 1.1 扩大清单**：错名 4 处 + `stream_chat_api()`（:37）+
   `ChatModelBridge/ChatWireProtocol/dispatch_chat_bridge`（:37/:38/:58/:59）+
   plan 文档 :138 的"自动丢弃"过时结论（由废止戳覆盖）。
3. **Batch 2 保持 Qoder 方法**（结构性证明），可选附加：live/exec 场景里断言
   stderr 含 `Dispatching chat stream via rig`（live harness 捕获 stderr，可行）。
4. **Batch 3 补强措辞**：既有证据（今日 636/636 四包、core compact 70/70、
   live 9 场景矩阵）可直接引用进 validation 文档，新增跑桥全量 + core（code-mode
   子集排除口径与今日基线一致），产出 `claude-rig-full-validation.md`。

其余批次（1.2/1.3/1.4、Batch 4、Batch 5）按 Qoder 方案原文执行。

## 四、流程备注

- 本轮再次印证：三方（Qoder→我→Qoder）每轮都有锚点错误，唯一可靠的仲裁是
  **打开文件读代码 + live 日志**。"锚点先核实再进方案"应成为固定纪律
  （Qoder 本轮也是这么做的，只是 grep 范围错了）。
- `diff` 别名劫持问题仍在，方案已含提醒，执行时一律 `/usr/bin/diff`。
