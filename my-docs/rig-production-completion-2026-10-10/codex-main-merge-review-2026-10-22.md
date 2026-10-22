# Codex 复审与 main 集成收据（2026-10-22）

## 结论与源码范围

本轮按用户要求复审交接、修复集成问题，并在本地把官方 main 合入 test。
集成输入是 fork `387914de0` 与 main `342ed69a1`，主线新增 300 笔提交；
fork 原有 6 笔未推送文档保留。集成分支 `codex/merge-main-20261022`，
最终 merge commit 从 `git log -1 test` 读取，两个父提交必须是上述输入。
本轮没有 push 合并结果、没有新开完整 CI、没有调用厂商 API、没有发布制品。

唯一可编辑源码仓库：`/Volumes/zhitai/git_workspace/fork-nuwax-codex`。
`/Volumes/soddy/git_workspace/fork-nuwax-codex` 及其旧 worktree 不作最新源码。
ACP 仓库 `/Volumes/soddy/git_workspace/fork-codex-acp-ts` 本轮未修改。

当前工具链随上游统一为 Rust **1.99.1** / V8 **152.2.0**；rig-core 与
rig-reqwest 继续锁 **0.44.0**，备用 genai 继续 **0.6.5**。不是额外升级 rig 的一轮。
当前有限任务见 [ROADMAP](../ROADMAP.md)，后续执行提示词见
[Claude 交接](claude-next-main-merge-2026-10-22.md)。

## 本轮发现并处理的问题

1. **鉴权接口变成可失败调用。** 上游 AuthProvider 的同步头、telemetry 与转换入口
   改为 Result；修正两桥、Core 及 live-tests 的调用与夹具，保留身份快照与刷新隔离。
   genai 不得把失败降级为空头或部分头后继续请求；新增两个失败关闭回归。
   Bearer 同步/异步错误一致，坏凭据不写部分头、不把凭据值带入错误。
2. **TLS 两种策略都要保留。** fork 的 WebSocket native-root TTL / 来源键 /
   single-flight 与上游 Windows 代理认证 fresh-root 路径分别保留。
   custom CA 读取错误必须传播；不关闭证书或 hostname 校验。测试使用新公开
   TlsMaterial API，不能在 Windows 上丢失原 PEM。
3. **启动与会话边界的接口适配。** 新 startup/config 分层继续保留 NUWAX 环境变量
   seeds、回退配置和有界停机；guardian 保留第三方活动模型并接入新 prompt-cache key。
   Core 保留 host sampling identity、cap usage 入账与 post-model hook 的失败语义。
   上游新 ToolError 分支补具体 Box；旧测试适配新的 tags、Arc 工具容器与认证签名。
4. **Bash / Linux 沙箱与上游变动合并。** 同时保留 fork 的 `--norc` 尾形重建、
   上游受保护 snapshot 的失败关闭；把 fork 的 proc/dev 单测迁入上游新测试文件。
   不恢复已由上游删除的 ProtectedCreateTarget。已知真实 deny 缺口仍开放，
   不能用单元测试全绿证明 O2/O3 已修复。
5. **构建与生成物联动。** fork CI Rust pin 同步 1.99.1；保留 fork 有意删除的
   官方发布/全量 workflow。Cargo lock、Bazel lock、稳定/实验 schema 与 Python SDK
   按工具重生成，保留 fork 枚举和桥依赖，没有手工伪造压缩 schema。
6. **普通栈的启动/恢复溢出。** 精确选中 shutdown 四用例后发现 SIGABRT。
   原先按文件名过滤并未运行这些用例，已纠正验证范围；不能沿用原 83/83 代替。
   崩溃报告指向 session/MCP 初始化的大 future 转发帧；以具体类型 Box::pin
   隔开启动、恢复、MCP、turn-context 与元数据的薄包装，保持原线程栈、超时、
   业务顺序与断言。特别是 Session::spawn 的 select 原来直接持有未装箱的
   初始化 future；修复把该具体 future 装箱，保留 biased 选择、取消和 instrument 顺序。
   最终数量由下方“最终验收补记”登记，不把中间失败称为通过。

## 定向验证与证据边界

所有本轮测试均使用合并工作树源码，不使用旧二进制替代；以下没有宣称全 workspace
或新合并树的三平台 CI 全绿。原始完整日志位于本机 `/private/tmp/zhitai-*20261022.log`；
精简收据另见同目录 `receipts/2026-10-22-main-merge.json`。

| 验证 | 实测结果 | 范围与限制 |
| --- | --- | --- |
| protocol / rig bridge / genai bridge / http-client / model-provider / app-server-protocol | **1350/1350**，1 skipped | `--profile local --retries 0 --no-fail-fast --test-threads 2`；唯一 skip 是需显式执行的 schema writer，稳定/实验 writer 已各执行通过 |
| rig + genai bridge 所有 target check | 通过 | 新 Result 鉴权 API / Arc 工具接口编译检查 |
| core + app-server + cli 所有 target check，启用 core/rust-rig | 通过（首次完整检查） | 执行在最后的普通栈边界修复之前；最终检查另补记 |
| rig / genai / http-client / model-provider Clippy | **通过，-D warnings** | 新 Rust type_complexity lint 用测试类型别名修正，不削弱 lint |
| Core/App-server/exec-server 初始定向集 | **83/83** | 图片准备、投影、cap、guardian 4MiB、shell snapshot；不包含后来才精确选中的 shutdown 四条 |
| V8 Code Mode 真实 JS smoke | **2/2** | synchronous exit 与 execute-to-pending；不是所有 Code Mode/CLI daemon 用例 |
| Linux sandbox lib check + tests | **168/168** | x86_64 Linux Rust1.99.1；源文件哈希对照，非完整 bwrap 拒绝端到端验收 |
| Linux http-client / model-provider | **271/271** | 161 + 110；retries0，0skip；当时源码快照，不能标为最终提交全树验证 |
| Bazel module lock strict / V8 module consistency | 通过 | 本地 mod deps update 后 lockfile_mode=error，无远程付费构建 |
| 稳定/实验 schema writer | 各通过 | 官方 writer 重生成 TS/JSON/zst/Python SDK |

复现六包合同：

```sh
cd /Volumes/zhitai/git_workspace/fork-nuwax-codex/codex-rs
CARGO_BUILD_JOBS=4 cargo nextest run --profile local \
  -p codex-protocol -p codex-rust-rig-bridge -p codex-rust-genai-bridge \
  -p codex-http-client -p codex-model-provider -p codex-app-server-protocol \
  --locked --retries 0 --no-fail-fast --test-threads 2
```

### 不覆盖首败的说明

- 六包首轮：1350 中 1330 pass / 19 fail / 1 timeout；当时同时编译 Core，
  多个 HTTP fixture 在客户端就绪前过期。失败身份保留：20 条隔离复验 20/20，
  随后原六包同一命令低并发完整复验 1350/1350，原断言/deadline 不变，retries=0。
  Linux 对照的 17 个同名 HTTP/model-provider 用例也全部通过；不单凭 Linux 绿
  就把 mac 首败定性成环境。
- 普通栈 shutdown 首轮 4 条 SIGABRT；第一层边界修复后仍 3 条，MCP 边界后仍 2 条。
  原日志均保留，最终补记必须以最后执行为准，不写成“只剩历史环境红”。
- 一次把 Code Mode 与 Core 合编的命令因 feature graph 过大被主动取消，未执行测试，
  不计作失败或通过。分开构建后 V8 两条真实测试通过。
- CLI 的 `cloud_config.rs` 既有 unused warning、stable rustfmt 忽略 nightly-only
  配置的提示仍存在；新工具链也提示共享 build_source_identity 的 fetch_update
  已弃用。限定包 Clippy 干净不等于全 workspace warning-free。

## #24：本地 V8 已经能做的与剩余项

复用 `.github/actions/setup-rusty-v8/action.yml` 的真实 manifest → archive/binding
双层 SHA 校验流程，下载与当前 Rust/V8 匹配的 arm64 macOS 配对资产，放到
`codex-rs/target/merge-main-20261022/rusty_v8/`，环境输出在同目录上层 `v8.env`。
导入环境后 Core/CLI check 与 Code Mode 两条真实 JS 测试通过。
这已反证“在本机完全无法编译 V8”；仍需把 action 中这套流程整理成稳定的
本地预取/缓存/离线使用入口，并验证 Linux/Windows 目标。
“离开 CI 能构建”与“断网、冷缓存能获取未提供的制品”不能混为一谈。

```sh
cd /Volumes/zhitai/git_workspace/fork-nuwax-codex/codex-rs
set -a
source target/merge-main-20261022/v8.env
set +a
# 然后执行同一源码树的 Cargo 命令；不向仓库写入绝对路径环境文件。
```

## #53：M1 已选并只执行一次

用户后续明确授权按推荐选择，因此执行一次 M1：只重跑 run `39633345230` 的
旧 mac job `119337089947`，没有重开整轮。新 attempt=2，mac job
`119602236907`，02:36:04Z 被 `GitHub Actions 1000024097` 接单，17 个步骤，
04:13:42Z 查询仍处于 `Test (workspace)`。原来“runner 执行次数=0”已不再描述本次 M1。

M1 的 head 仍是 **29e7e59d5**（Rust1.95/V8150），不是本次 main 集成树。
Ubuntu/Windows/Bazel 在 attempt2 的 API 列表中带旧完成时间，不能记成又执行一轮。
日志 API 在进行中曾返回 404；结合 runner 与步骤已知，它不能再被解释为“从未接单”。
只在终态读取 Summary 与两个 bootstrap 的判决、实际 override 绑定证据。
不足时如实开放，不自动第二次 rerun，也不靠反复等一次随机慢测消耗预算。

## 最终验收补记

最终源码已冻结，默认线程栈的四条 shutdown/恢复合同与 guardian 先 **5/5**；
随后同一源码扩展为 Core/App-server/exec-server + ThreadStore 的 **365/365**，
30.062s，retries=0。实际分布：Core72、app-server1、exec-server17、thread-store275；
覆盖原88条，并新增 turn-context 两条及全部存储lib用例。其余3617条按filter未执行，
不计通过。四条生命周期用例的栈/期限/断言未变，没有用RUST_MIN_STACK绕过。

最终 `cargo check -p codex-core -p codex-app-server -p codex-cli -p codex-live-tests
--all-targets --features codex-core/rust-rig --locked` **通过**，59.58s，覆盖最终边界修复
和真实测试harness的鉴权接口适配。live-tests只做编译，没有调用付费API。
最终Core + ThreadStore lib Clippy（双桥features、`-D warnings`）**通过**，2m24s；
`cargo fmt --all --check` **通过**。合并delta的diff-check仅报告一份上游TUI
快照的定宽尾空格；已核实该blob与main逐字节相同，保留渲染合同，不strip快照。
排除这份原样fixture后的完整staged diff-check、fork源码相对main的diff-check均通过。
合并结果阶段保存在本地test，
没有push；通过本文件或JSON收据的Git历史可定位包含这些源码的merge commit。

为避免重复同名符号或异步落盘的旧CrashReport误导，定位时按二进制UUID匹配报告与
反汇编；栈帧的静态大小只用于定位，不等于整条链的精确动态峰值，不宣称所有帧低于
某阈值。元数据两处Box::pin未改变 writer lock / SQLite / rollout / 错误返回顺序。
turn-context大段缩进的token比较确认：移除新wrapper及注释后，业务body与合并版本相同。

定位时可测的最大薄分发之一为Session::spawn的select帧，Box子future后从约368.1KiB
降至135.5KiB；这不是全部运行时的上界。最终改动在6个文件的12个具体future边界，
不涉及SQL或模型请求字段的改写。各阶段日志与摘要保留在JSON收据中。
