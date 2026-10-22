# Windows TUI 最终定向类型验收（2026-10-22）

**最终 PASS**：原生 `x86_64-pc-windows-msvc` / Rust 1.99.1，执行
`cargo +1.99.1 check -p codex-tui --tests --locked`，exit **0**，
`Finished ... in 40.86s`。实际窗口为北京时间 **19:26:37–19:27:18**
（UTC `11:26:37.4363988Z–11:27:18.6926010Z`）。

最终增量只同步 `codex-rs/tui/src/app/tests/backend_sessions_tests.rs`，
SHA256 **`29decf0307340b355640fae57644aa19eac2a1e68d87e360d9746b74c44ad155`**。
该改动在原 5s 边界内确认 proxy task 已取消，再检查原 EOF 5s 边界；无产品源码变化。
最终七文件在 Windows 上逐字节校验，且完成后与本地冻结树再次核对，六个其它文件未变。

## 历史首轮保留

第一次同命令已在 19:07–19:16 原生通过，耗时 **8m15s**，关闭 CI 的七处
TUI 测试编译错误。那次 backend_sessions SHA 是
`c7ece7e204fbb488990ec6f32ec25c066851c907435f09b34d4b2a111280af21`。
首轮收据和日志均保留，没有用最终收据覆盖它们：

- `receipt-initial.md` / `validation-receipt-initial.json`
- `source-manifest.json` / `check-receipt.json` / `tui-check.log`

## 输入和边界

- 源码为 `git archive 850a87390` 加冻结 TUI 四文件、exec build_source_identity
  helper、genai resolver/resolver_tests 七文件。最终清单为 `source-manifest-final.json`。
- 独立镜像为 `C:\Users\soddygo\nuwax-tui-review-20261022\repo`，未重置或编辑
  用户原 checkout；仅任务目录及明确复用的原 target 构建缓存发生写入。
- Rust 工具链为 `1.99.1 (58cf8f9c3 2026-10-13)`，LLVM 23.1.1。
  Windows V8 152.2.0 缓存 archive/binding 先按仓库可信清单核验，再设置正常构建
  overrides；没有改 pin。这不是 Windows V8 执行收据。
- 保留未改 Windows HTTP 依赖的六条 `dead_code` 警告；这是类型检查通过，
  不是严格 Clippy 无警告。
- **原生 TUI 运行测试未执行**，未构建测试 EXE，未运行完整 TUI 或 workspace。
  缓存查询原本为零个 TUI test EXE；最终增量任务明确只做类型复验。
- 没有 push、CI trigger、模型调用、账户或宿主策略修改。

## 最终工件

- `tui-check-final.log`：原始最终日志，SHA256
  `705d32516d83cf48949dcbe0e7a74f8904828c2bc70026fae7273614c7f36212`
- `check-session-final.log`：七文件哈希、工具链、原生输出尾部
- `check-receipt-final.json`：实际退出码、时间、环境路径（保留原 UTF-8 BOM）
- `source-manifest-final.json`：最终七文件 SHA
- `validation-receipt.json`：规范化最终收据，含原 8m15s 历史指针

当前结论只覆盖这个 Windows 源码快照的 TUI tests 类型检查；不扩展为完整最终树
Windows runtime/CI 全绿。
