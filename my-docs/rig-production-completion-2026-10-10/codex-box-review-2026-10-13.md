# Codex 复审：turn future 装箱

审查起点 `a8e1a655a`，生产改动 `695b2a63b`（基线 `0b5cdf8bf`），工作树起始干净。范围为5个Rust文件+209/-160，共369 changed lines；含文档451行，符合复杂批<500目标。未改4MiB测试常量、API/config/schema/依赖、历史、V8 pin或模型上下文，无P0新注入。

## 生产代码结论

完整diff实际新增 **55个Box::pin**：handlers28、turn13、turn_input8、client4、regular2。逐一核对参数、分支、borrow/ownership、await与错误传播；具体Pin<Box<F>>保留静态类型，未dyn擦除。stream的instrument仍在or_cancel内，OrCancelExt仍用原tokio::select处理取消/结果；未发现业务语义回归。

装箱在执行到这些边界时新增heap分配/释放，未测性能成本；不能把“纯机械控制流包装”写成无分配或零性能差异。广泛boxing在当前尺寸内构成一个连贯改动，没有实证表明应删减某个边界。未添加镜像实现的新测试，复用已有集成覆盖。

## 已修正的证据问题

1. `results-followup-2026-10-13.md:19` 的“四job均failure”错误：实际Bazel success，三个nextest failure（gh直接核对run38358243799，head0b5cdf8bf）。对应文档已纠正。
2. 同文档`:32–33` 将编译器浮动定为CI/local根因，但前一行又报告两版本在修复前4MiB均通过。版本是待对照的构建条件，未建立唯一因果；Tasks同步取消“根因已定”的措辞。
3. 同文档`:32` 以workspace metadata并集证明test binary features不充分。旧崩溃basename `codex_core-ef8ac5bbb7ed29cb` 同名fingerprint显示default+rust-rig；未保留UUID/SHA与所有测量逐次关联，不能把它直接当CI default+genai+rig的完整复现。本轮测试明确两个feature并记录实际产物。
4. 同文档`:43` 前3MiB FAIL vs 后3.25MiB PASS/2.875MiB FAIL，区间重叠、预算不同，不量化改进。需要同编译图/同预算before-after；本轮不修改原始测试预算，也不把低栈失败当CI4MiB同形故障。
5. 同文档`:35` 静态prologue不直接证明动态峰值或“全部互斥分支状态相加”。父future布局、构造/移动临时量、poll栈帧与优化器函数内联是不同量；[上游13429](https://github.com/openai/codex/pull/13429)只支持heap指针缩小外层future这一修法。handlers/turn_input两段新增注释已作对应澄清。
6. 交接文本“33处”不符合实际55处；报告补准确分项，未为凑数调整正确代码。

这些是证据/注释问题；没有查到需要再改装箱行为的产品缺陷。候选修复合理、本机原始合同通过，**CI guardian修复验收仍开放**。

## 独立验证

macOS arm64，显式 `--features rust-genai,rust-rig --locked --offline --retries 0`，mock请求实跑，无付费厂商调用、无完整workspace。

- `RUSTUP_TOOLCHAIN=1.99.0 just test -p codex-core … -E 'test(guardian_ephemeral_retry_preserves_parallel_trunk_and_fork_history)'`：1/1 PASS（专用线程仍4,194,304字节）。
- `RUSTUP_TOOLCHAIN=1.95.0 just test -p codex-core …` 同选择器：1/1 PASS。justfile的8MiB默认不覆盖测试自建4MiB线程。
- 1.95.0选择turn_input单测、prewarm取消、Rig三wire cap、WS request/fallback、guardian取消、resume/compact/token_usage和四个public turn mode：168实际执行，166 PASS、2 FAIL。失败都是缺 `codex-code-mode-host` 二进制，不冒称这两个用例通过。
- 按仓库现有setup-rusty-v8 Action的双层校验下载原150.4.0 arm64 archive+binding，使用Python3.13执行同脚本；1.95.0构建code-mode-host成功，没有变更pin或manifest。随后单独复跑两条受阻CodeMode取消用例，**2/2 PASS**。所选168个不同业务回归最终均通过；另同一个原始4MiB合同在两工具链各通过一次，共170次通过执行，不混称170个不同身份。
- 构建空间曾紧张，删除可再生成的target/debug/incremental缓存；源文件与已编译产物未删，用户Bazel缓存未动。

上述相同features的两份libtest产物均记录SHA256、Mach-O UUID和rustc -vV，详细本机收据 `/private/tmp/codex-box-review-build-evidence.json`；日志 `/private/tmp/codex-box-review-199-4m.log`、`codex-box-review-195-4m.log`、`codex-box-review-195-paths.log`、`codex-box-review-code-mode-cancel.log`。1.99有既有deprecated/linker/future-incompatibility warnings，未把它们当本次Box变化或通过升级pin求无警告。

可跨电脑保存的精简收据见 [codex-box-build-evidence-2026-10-13.json](codex-box-build-evidence-2026-10-13.json)：对应原始4MiB验证时的clean HEAD `a8e1a655a`，不冒称后续注释/文档修正已存在于该二进制。后续实际启动当前HEAD时需重建candidate，不能跳过receipt。

完整选集包含session::turn_input::tests、两个prewarm用例、rig_output_cap、responses_websocket_streams_request、websocket_fallback、guardian_review_cancellation、suite::resume/compact/token_usage_rollout，以及start_if_idle/recover/continue/concurrent_started_steered四类public合同。未选择daemon-recovery模块flaky用例，故不声称其全族通过；上一轮stash基线记录仍须单独保留。

最后仅修改证据与注释，按tests→scoped fix→just fmt→diff-check收尾，最终fix/fmt后不重新测试。本轮不push、不dispatch，不把旧run当包含695b2a63b的CI验收。

`RUSTUP_TOOLCHAIN=1.95.0 just fix -p codex-core --features rust-genai,rust-rig --locked --offline` 已通过、无告警。原boxing提交保留，不重写695b2a63b/a8e1a655a历史；复审修正作为单独本地checkpoint保存。

`just fmt`、最终`git diff --check`通过，工作区没有范围外格式化改动。未运行完整本地workspace，未用删测试、提高栈或延长期限验收R11。
