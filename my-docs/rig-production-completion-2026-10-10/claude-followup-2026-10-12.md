# Claude Code 跨电脑继续开发（2026-10-12）

## 1. 项目目标与先读入口

这是 `nuwax-ai/codex` 的官方 Codex fork，工作分支为 `test`。保留官方 Core、rollout、工具执行、app-server、exec、TUI 的行为，扩展第三方模型配置及环境变量启动；同一容器中的不同进程应能使用不同 provider/model/key/base URL/wire/home/retry/idle 配置且互不污染。

默认第三方路径使用 rig。`wire_api=responses` 必须使用 rig 的 OpenAI Responses 适配器；Chat Completions 与 Anthropic Messages 分别使用对应适配器。不要恢复过时的“Responses 默认转 Chat”路由。native 显式选择仍有独立覆盖。目标是协议字段、来源授权、流终局、工具调用配对、usage、旧历史投影和传输行为都具有可复核的兼容证据。

按顺序读本目录：

1. `codex-review-2026-10-12.md`（本轮独立复审及修复）。
2. `spec.md`、`plan.md`、`tasks.md`（需求/方案/执行分别维护）。
3. `results-followup-2026-10-11.md`、`ci-run6-attribution-2026-10-11.md`、`ci-run6-failure-ledger.tsv`。
4. `codex-review-2026-10-11.md`、`results.md` 及 canonical ledger（历史结论保留窗口，不能覆盖新证据）。

先记录 `git status --short`、HEAD、分支和 remote。工作树干净才 `git fetch origin test`、`git merge --ff-only origin/test`；有并行改动或分叉先说明现状，不能 reset/stash-all/clean/强推。使用远端最新 checkpoint，不退回旧基线，不复制旧电脑临时 target 或 V8 缓存冒充新验收。

## 2. 第一批：当前 SHA 的 CI 和平台失败定位

本轮 push 自动触发 `Fork Cargo PR`，先读取该 SHA 对应的 run/job/attempt 结果。不要拿旧 run `38065399719` 的绿项验收新提交，也不要未经授权手动 workflow_dispatch。

- 验证 host 实际 triple、V8 pin `150.4.0`、sandbox feature、归档和 binding 配对、双层 SHA256 校验、真实 host/helper 二进制存在。保持 pin，不回退 denoland 默认归档，不关闭 TLS/校验、不假造 host。
- 核对解除 V8 排除后的完整消费者集合。567 是历史三平台分类身份，macOS 184 个代表样本不等于 567 全绿。每个平台按完整 `(platform,binary,test)` 与新 run 对照，新增失败另列。
- 补 80 个 explicit 分类行缺失的首败摘录或确定的 run/job/log 位置；487 个间接分类中只凭名字的项须复核，不能把分类当 PASS。
- Linux 用 `bash scripts/bwrap_capability_check.sh --bwrap <实际使用的二进制> --timeout 10` 校准 runner，普通与 `-v` 应同类。0/1/2/3/4/5/77/64 分别表示探针通过/缺工具/userns拒绝/netlink权限/意外/超时/非Linux/坏参数。它只证明所测探针能力；同一 runner 上仍要运行 Codex 沙箱公共测试。
- 原 guardian 栈溢出须在 Linux/Windows 相同 feature、明确 4MiB 线程及原预算复现，区分 future 构造/移动/poll；macOS 2MiB PASS 不关项。禁止增栈、改 deadline 或 skip 求绿。
- 逐项处理 assertion-open 79、no-evidence 14、deadline 53、Windows PASS+LEAK 311、PowerShell 2；保留首次失败及复验，不概括为并发固有。voice-host 的 Linux/Windows gstreamer 供给仍单列。

## 3. 第二批：cap/usage 未完成合同

先保持本轮 nonce+序列号键、实际 step model 归因、UTF-8 前缀冻结、初始 Added 文本和多段 summary 修复。恢复同一 turn 的新 Session 不得覆盖已计 sampling spend。诊断默认关闭，关闭时不采集全文。

### 3.1 metadata 与序列化总界（优先）

现有 16KiB/64KiB/32 限额针对 fragment 文本和数量，**没有覆盖任意长 provider item/call ID、usage 的 response ID、JSON metadata 与最终序列化字节**。设计独立的 metadata/事件总限，先在 Spec 定义身份超限的可观测语义，再实现有界 key/引用/序列化/render。不能直接截断身份造成别名，不能先 clone/format 无界字符串后才裁切。覆盖巨型 ID、重复 ID、不同 ID 同前缀、Unicode、summary 多段及终局；不改实际工具 identity/授权。

### 3.2 三协议与客户端闭环

- 处理最后 scoped lint 的两条结构告警：`ToolError::Codex(CodexErr)` 增大整个错误枚举（240 bytes）；`SessionState::record_token_usage` 达8个参数。分别做最小装箱边界与 typed 参数整理，保留错误类别/所有权/usage wire 语义，配真实公共错误及usage回归；不能仅加 allow 消警或把装箱当已证实的跨平台栈修复。
- Chat `finish_reason=length` 后独立 usage chunk、Anthropic start/delta、Responses SSE/WS/Rig 的 cap usage：缺失/部分/显式零/负值/溢出、首终局后迟到帧/取消均有实际 wire 用例。计数 presence 不能被归一化零伪造。
- app-server thread/read/attach/重连/分页、exec JSON/SDK/UI、旧 reader 必须实测。现有 codec 或 `Value` 可解析不能冒称旧客户端兼容。legacy/paginated/crash/archive/delete/两次 resume 不改 rollout 前缀。
- 诊断片段自身不执行工具。桥层截断或未完成调用零执行；**native 已收到完整 OutputItemDone 的工具按既有流式调度可能先执行**。要改成“任何完整工具也等终局”须先明确新的 Spec、性能与取消/并行合同，再实现，不用未注册工具证明零执行。
- 模型可见恢复尚未完成：先取得完整 render 的 Exact/ProvenUpperBound 计量证明、typed context fragment 与 P0 人工门。缺证明仅能保留诊断，不能自动注入或宣称 resume 模型已看见 partial。

## 4. 第三批：代理默认路径与原 T21–T40

ResolvedDefault 保持 opt-in，默认迁移不是本提示词的隐含授权。先补 cold/并发原预算、手动系统/env/NO_PROXY/CGI/逐 hop/鉴权/CONNECT/SNI/PAC/WS/SOCKS 的参考双轨，确保所有普通 factory 入口与实际默认路径都被测；替代 fixture 直连通过不能关闭默认性能问题。

继续 D6 可证明预算、owner 四命令/锁/crash/队列、双进程隔离、三协议字段最新审计、凭据/endpoint/header/query/model/wire 来源旋转、旧历史/RawValue 大数字、signed thinking/search/citation/pause 合法配对、原生 Linux/Windows/容器及异 OS app/exec 验收。40 项逐项登记实现/验证/开放，复合 DoD 没全达成不勾完成；不要重做已有独立证据。最后准备安装升级、工件来源、回滚，不直接发布。

## 5. 开发和验收规则

- 可自主做局部修复、相关离线测试及小批本地 commit；并行工作明确文件 ownership，不回退别人。复杂批 <500 行，普通批 <800 行，生成物独立说明。
- Rust 用 `just test`、`--locked`、适当 `--offline`、`--retries 0`，记录实际非零匹配、PASS/FAIL/SKIP/TIMEOUT/ABORT/LEAK；test 名全齐不等于断言覆盖。需要的 workspace helper fresh build，source/git/features/bin/hash/receipt 分开记。
- 每批先 tests，再 scoped `just fix -p ...`、`just fmt`、diff-check；最终 fix/fmt 后不再跑 tests。config/API/dependency/BUILD 变更同步 schema/TS/Bazel lock/runfiles。
- 完整 workspace 依 AGENTS 取得授权；付费 live、改默认代理、远程 push/手动 dispatch/tag/npm/部署另需明确授权。平台缺失继续独立任务，不能假报 PASS。
- API key、`.env.local`、原始请求认证头、完整模型日志不得入库或回显。source digest 与新二进制必须匹配，旧电脑 receipt 仅是历史证据。

新增结果写 `results-followup-2026-10-12.md`，更新 Tasks 的对应项。最终提交清单、准确测试结果、新 CI SHA/URL、修复/未完成/需裁决项逐一交付。
