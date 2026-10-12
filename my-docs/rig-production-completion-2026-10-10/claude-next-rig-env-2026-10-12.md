# 给 Claude Code：先闭环 Codex 的 Rig 环境启动与 Linux 服务验收

**同日最新进展：** 后续批次已实现 app-server Chat/Responses 正例、typed 环境 live harness、原生 Linux 离线工具闭环及两个历史失败切片；见 `results-rig-env-2026-10-12.md`。用户随后提供测试密钥并授权有限 live 验证：全部 13 个所列模型/协议基础组合及三个主模型工具往返已通过（保留 MiMo 首次未调用工具与复验），独立计数 20 次请求；见 `results-codex-live-probe-2026-10-12.md` 和对应脱敏 receipt。密钥未进入本提示词，临时凭据已清理。不要重复开发已完成部分；下一步先补该结果文档末尾的预算、工具断言及字节脱敏扫描，再做限定主路径深入验证。以下原批次计划保留作范围/合同参考。

直接复制本文件作为下一轮任务。主仓库 `/Volumes/soddy/git_workspace/fork-nuwax-codex`，工作分支 `test`。用户目标：用环境变量配置国内模型或自定义第三方平台，通过 Rig 启动并稳定使用 fork Codex。首个运行目标是 Linux/容器的 app-server；实际客户端是 `/Volumes/soddy/git_workspace/fork-codex-acp-ts` 的 `dev`，但本轮开发中心仍放在 Codex，不扩大为 ACP 重构。

先读 AGENTS.md、本目录 `codex-review-claude-2026-10-12.md`、`results-followup-2026-10-12.md`、spec/plan/tasks 和原 handoff。记录分支、HEAD、工作区及 remote；尊重已有并行改动，不 reset/clean/stash-all/强推，不重写 Codex 已保存的阶段提交。当前修复包括连接阶段 TLS fallback、typed rustls alert、cap 身份/包络记账、Anthropic usage overflow guard、真实请求数和 local/remote v2 压缩回归，不重复开发这些部分。

## 批次 A：当前版本的 Codex 公共路径验收

1. 先建立 source/bin 对应关系：当前 HEAD+dirty/source digest、Rust host triple/features、实际 CLI/app-server 二进制路径、版本与 hash。CLI/exec/app-server 已启用 rust-rig；第三方默认 rig、wire=responses 必须走真实 Responses 适配器，不转 Chat，也不能通过 native fixture 代验。
2. 复用现有 `exec/tests/suite/nuwax_env*`、`app-server/tests/suite/v2/nuwax_positive.rs`、`nuwax_isolation.rs` 和 bridge fixtures，补最小缺口：app-server 的 Chat/Responses 正向环境启动（现有 positive 主体为 Anthropic），经 public JSON-RPC 实际 start/turn/工具往返/取消/两次 resume。已有覆盖仅在当前源码上复验，避免重复建框架。
3. 按真实 wire 断言 URL、auth、model、cap、请求/stream retries、idle、终局；显式 CLI provider/model 覆盖和失活、坏配置发请求前失败、错误不泄露 key。两个进程同时启动，各自 NUWAX provider/model/key/base/wire/预算及独立 CODEX_HOME/CODEX_SQLITE_HOME，验证实际请求和持久化互不污染。
4. cap 只产生失败终局；截断工具不执行，迟到帧不能改成功。native 已完整 Done 的工具仍按现有调度合同，不能偷改为全部等终局。Chat/Anthropic 负值在 Rig 解码阶段当前为有界失败；Responses 是逐字段容错。实报/缺失/零/overflow 语义需分别注明，不能用 accumulator 单测冒称全部 wire 容错。
5. 补 Responses WebSocket 传输层 cap fixture（只有声明使用该模型 WS 路径时作为本批必验），既有共用 parser 源码不替代实际 transport 测试。普通 SSE 路径不因 WS 扩展尚未覆盖而重复开发。

## 批次 B：一个真实 provider 的 NUWAX 启动闭环

1. 现有 live binary harness 的 `live-tests/src/binary_turns.rs`/`binary_turns/runner.rs` 用临时 config.toml provider 和 experimental_bearer_token；给它加 typed 启动模式，将 NUWAX_MODEL/BASE_URL/WIRE_API/API_KEY 和可选 cap/retry/idle 只传子进程环境，保留现有 config 模式。复用 bounded capture、超时、工件和 source/bin receipt，不另造路由。
2. 先用本地 mock 验证该模式真的走 env→CLI seeds→Rig、准确的请求和工具执行，home/sqlite 隔离；密钥不写配置、收据、日志、rollout 或 git。
3. 用用户实际选择且平台支持的一个 provider/model/wire 验收 marker、一次无害工具往返、usage、取消、resume、compaction；明确平台真实支持的协议，不把不存在的 Responses/Anthropic 网关当产品故障。真实 cap 触顶与“请求接受 cap 字段”分开验收。
4. 本会话已经取得用户提供的测试凭据及有限 live 测试授权，上述验收已执行；它不授权无限重试或任意新增厂商矩阵。后续新批次明确厂商/模型/请求数上限并复用已有证据，不复制凭据进 Git/提示词，不擅自将全部 live 矩阵当默认运行。新会话的凭据和额外费用范围仍由用户提供；缺凭据时继续独立离线工作。

## 批次 C：Linux/容器服务验证和下一阶段边界

1. 有原生 Linux/容器时，运行当前 source/bin 的实际 build 和选定公共链路，校准 bubblewrap/userns，再验证工具、进程取消/退出、数据持久化、重启后 resume。记录镜像/OS/triple/features；Mac mock/交叉编译不代替原生实跑。TLS 证书验证保持开启，代理按当前有效配置；ResolvedDefault 仍 opt-in。
2. 新 SHA CI 的 userns/磁盘/TLS 三修复需重新取实际日志。sysctl 写失败不能当 key 不存在；TLS 需验证真实 rustls backend/source chain。未授权 push/dispatch 时准备验收，不手动触发；取得授权后才用新 run 关项。
3. 对 `projects_persist_and_assign_threads` 和 `guardian_checkpoint_migration_request_history` 先做独立最小复现，区分断言过时/fixture/产品竞争，只修复证据证明的原因。不批量接受 snapshot、不放宽 deadline、不增栈、不 skip 求绿；与 Rig 环境启动的阻断关系单列。
4. 旧 rollout 身份规范化可作单独小切片：恢复后的内存 ledger 也遵守身份上限，保累计值及原 rollout 前缀，不截断 ID。无需同时重构所有恢复存储或新增模型上下文注入。
5. ACP 只读确认：保持 main 用于上游同步、dev 用于使用，不切/改该仓库。通过 CODEX_PATH 固定本次实际二进制；环境继承已存在，显式 ACP provider/model 可能优先于 env。若 Codex 公共路径通过而 ACP 的 usage/provider 展示或失败映射不兼容，写出具体事件与文件行号，留独立任务；本轮不转移开发中心。

## 验收门槛和执行规则

第一阶段可以验收为“某个明确 Linux 镜像、当前二进制、某个真实 provider/model/wire 的 Rig 环境启动可用”，不得泛化为三平台/全厂商/40 项全部完成。至少提供离线三 wire 请求证据、一个授权真实厂商链路、公共工具/失败/取消/resume/compaction/usage 和双进程隔离，以及准确 source/bin/receipt。

D6 Exact/ProvenUpperBound、模型可见 partial 恢复、全部旧 reader/SDK/UI、完整行政与异 OS 矩阵、代理默认迁移和发布工件是后续 roadmap，不能阻止普通 Rig 模型接入闭环，也不能预先标完成。partial 继续是诊断，未经计量证明和 P0 人工门绝不注入模型。

每个确认缺陷先保存首败，做小批本地 commit（复杂 <500、普通 <800 changed lines）。Rust 使用 `just test --locked --offline --retries 0`，实际非零运行，PASS/FAIL/SKIP/TIMEOUT/ABORT/LEAK 分开；注意宿主继承禁网标记时 fixtures 会早退，nextest PASS 不等于测试执行。在有授权的非沙箱 loopback 环境运行时可仅清除测试子进程标记，不改 sandbox 代码。

本轮用户选择完整 workspace 留下一轮/CI，不视为给你本轮无限测试授权；遵守 AGENTS 的完整 suite 确认要求。每批 tests 之后 scoped fix，再 just fmt、diff-check，最终 fix/fmt 后不重跑同批测试。schema/TS/config/Bazel/runfiles 按实际变更同步。不得修改 V8 pin/验证、关闭 TLS、改变默认代理、付费 live、push/dispatch/tag/npm/部署，除非用户另有明确授权。

结果写新的 follow-up 文件并更新 Tasks 进展；按“必须修复 / 当前已验证 / 平台或授权待验 / 独立扩展”分别列，40 项复合 DoD 未全满足不勾完成。最后给阶段提交、实际测试数量、当前可用范围、剩余最短任务，不仅报告 PASS。
