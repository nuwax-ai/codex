# 图片 wire 预算误算修复批次 — 2026-10-14

阶段一（图片任务第一批）：**最终 wire 预算把图片 Base64 当文本 token 计算**。基线 `bc4517dad`（origin/test 对齐）。计划与要求见 `claude-image-budget-followup-2026-10-14.md` 第一批、`claude-main-rig-upgrade-followup-2026-10-14.md` 阶段一。

## 缺陷机制

- `transport.rs` `send_streaming` 在发送前调用 `WireBudget::check(body.len())`；
  `wire_budget.rs` 把整个最终 JSON 字节数按 4 bytes/token 估为文本 token——图片
  Base64 属传输编码而非模型可见文本。600KB 级合法内联图（核心准备已缩到 ≤2048px）
  被误估为 ~15 万文本 token，128K 窗口在 HTTP 发送前被误拒
  （`ContextWindowExceeded`）。三协议同受影响（预算在共享 transport 层）。

## 修复（bridge 内部，产品改动 ~70 行 + 新模块 wire_images.rs ~200 LoC）

1. **新模块 `codex-rust-rig-bridge/src/wire_images.rs`**：按协议真实 content 路径
   walk 最终序列化 JSON，识别内联图片并产出 `WireImageCosts`：
   - Chat：`messages[].content[].image_url.url`（用户图 + 工具图补充 user 消息同形）；
   - Anthropic：`messages[].content[].image` 的 `source.type=base64`（含
     `tool_result.content[]` 嵌套——bridge 的 base64 工具图保留在该处）与
     `source.type=url`；
   - Responses 透传：`input[].content[].input_image` + `function_call_output.output[]`
     的 `input_image`（MCP 输出在序列化前已归一化为 function_call_output，
     `stream_events_utils.rs:585`）。
   - 计价口径与 core 历史估算一致（`core/src/context_manager/history.rs`）：
     普通准备图 7,373 bytes ≈ **1,844 tokens/图**（镜像常量并注明同步）；
     `detail=original` 取 **10,000 patch 上限**（wire 层不解码像素尺寸的保守具名
     策略，与 core 对不可解析 Original 引用的回退一致）；URL/file-id 引用计同等
     视觉成本、不免费；file-id+original 亦取上限。
   - 只识别 type 标签路径上的图片；**普通文本/工具 schema/参数里伪造的 data URL
     仍是文本**（专项单测），不做任何全局 data-URL 剔除。
2. **`wire_budget.rs`**：`check` → `check_body(wire_bytes, Option<WireImageCosts>)`。
   文本字节 = 总字节 − 内联图片载荷字节（data URL/base64 字符无需 JSON 转义，
   字符串长度即序列化足迹）；估算 = 文本字节/4 + Σ视觉 token。`None` 保持旧口径
   （未配窗口或测量失败）。32MiB 字节硬限、输出预留、无效配置错误全部保留，
   字节限仍按完整 body 判。
3. **`transport.rs`**：预算配置了窗口时复用 chat-family rewrite 的解析（或补做
   一次解析）测量图片成本；仅测量用途的解析失败**不新增拒绝**（warn 后回退旧
   口径），rewrite 路径的硬解析错误保持不变。请求字节、工具 schema、顺序、
   call/result、thinking/hosted replay/pause 语义零改动。

## 验证（macOS arm64 本机，rustc 1.95.0，nextest NEXTEST_PROFILE=local）

- **先行失败记录**（修复前同一选择器）：4 个新出站回归全败
  （`chat_inline_image_is_dispatched_with_unchanged_bytes`、
  `anthropic_inline_images_are_dispatched_across_user_and_tool_blocks`、
  `responses_user_and_tool_output_images_are_dispatched`、
  `same_image_with_larger_encoding_still_fits_the_window`）——合法 600KB 图在
  128K 窗口被 `ContextWindowExceeded` 拒绝；负控
  `oversized_text_is_still_rejected_before_any_connection` 修复前后均过。
- **出站回归**（真实 transport 栈 + 本地 HTTP/1.1 服务，`wire_budget_outbound_tests.rs`）：
  三协议合法大图实际派发、恰好 1 次 attempt、**socket 收到字节与序列化请求逐字节
  相等**（Anthropic 为确定性 round-trip 形态）；同图 6 倍编码差异下 30K 窗口两请求
  均派发（视觉成本不随 Base64 长度缩放）；600KB 纯文本三协议均在建连前拒绝且
  0 attempt。
- **单元**（`wire_images_tests.rs` / `wire_budget_tests.rs`）：三协议路径识别与
  排除、URL/file-id 非零计费、original 上限、文本内伪造 data URL 不绕预算、
  编码大小不变视觉估算、输出预留叠加、32MiB/配置错误保留。
- **回归**：`-p codex-rust-rig-bridge` 全 crate **280/280 过**（含既有 wire/
  hosted/replay/cassette 套件；慢项为负控 3×5s 等待）。最终字节（经 fmt +
  clippy --tests 修复后）上 `wire_budget wire_images` 选择器复跑 **17/17 过**。
- 流程：tests → `just fix -p codex-rust-rig-bridge --locked`（1 处机械修复，
  clippy --tests 全编译通过）→ `cargo fmt -p` → 选择器复跑核最终字节。
- BUILD.bazel 用 `codex_rust_crate` 宏自动收源，无需更新；无 Cargo/lock 变更。

## 边界与未验项

- wire 层不解码图片像素：original-detail 计 10,000 token 上限（具名保守策略），
  core 侧有尺寸时仍按 32px patch 精算——两层口径在"不可解析时取上限"上对齐。
- 估算为粗粒度启发（4 bytes/token + 平价图），非 provider tokenizer 认证（T21
  计量证明不受本批影响）。
- Linux/Windows 实机与 CI 未跑（本地提交未 push）；请求体 2MiB 审计捕获上限与
  attempt 预算耦合（第二批第 6 项）未动。
- 本批不含 MCP 图片入口硬界、单工具多图上限、连续截图提前压缩（第二、三批），
  也不动 Rig 版本（0.42.0 保持）与 Core 压缩生命周期。
