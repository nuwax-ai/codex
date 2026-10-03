# Claude Code 独立复查与验证提示词

请对本次 Codex checkpoint 做独立逻辑审查和测试验证。工作目录：
`/Users/soddy/Documents/git-rust-work/fork-codex`，分支 `test`。

## 1. 固定审查对象

基线为 `20898140f2b3638aac6ab328b24d2f604871bfc8`。
目标为本提示词首次进入 Git 的 checkpoint commit。用
`git log --diff-filter=A -1 --format=%H -- my-docs/claude-checkpoint-review-2026-10-03.md`
确定目标，核对它与用户提供的 Codex 回执 SHA 一致；不要将后续 HEAD 或旧二进制当作目标。
先记录 status、branch、SHA、diff 范围；保留其他人的修改和未跟踪运行数据。

依次阅读：

1. 仓库及子目录 AGENTS.md。
2. `my-docs/codex-review-prompt.md`：使用其审查重点；其中旧版本/每轮新建 client 等历史叙述以当前源码为准。
3. `my-docs/rig-stability-next/direct-development-2026-10-03-{spec,plan,tasks,targeted-validation}.md`。
4. `my-docs/container-multiprocess-env-review-2026-10-03.md`。
5. `my-docs/provider-request-controls-2026-10-03/{spec,plan,tasks}.md`。
6. `my-docs/field-mapping-audit.md`、`my-docs/FORK.md`，核对其现状是否准确。

这是多轮相互依赖修改的 checkpoint，不是全面功能/平台验收完成的声明。

## 2. 审查重点

### 协议与请求控制

- 默认 Rig：第三方 Responses 是同协议直传；Chat/Anthropic 才跨协议转换；第一方 OpenAI/Bedrock native 和显式 native 仍正确。Genai 已搁置，本轮相关改动主要是构造器迁移。
- 对照实际锁定的 rig-core 0.42.0 和官方 OpenAI/Anthropic 文档，核对请求/回执/工具/usage/reasoning/错误字段。不要用本地 Rig 最新 main 替代锁定版本。
- `max_output_tokens` 的 HTTP、WS、Core builder、Rig body、预算采用一致数值；请求值优先 provider fallback；未设置不增加字段；cap 变更影响 WS reuse。检查 Chat 的模型特定 `max_completion_tokens` 映射。
- Responses 类型复制与 RawValue 保真：大整数、原始工具 schema、键序、签名/密文、投影后的 null 形态不得因 Value 往返改变。
- HTTP retries=0 仍发送一次；429/5xx/transport flags、Retry-After、取消、最后错误、每次尝试采集正确。HTTP 与 Core sampling 分别计数，明确组合上限；成功取得 SSE 后桥不自动重放。
- pool/custom CA 都关闭 reqwest 内层重试。实际 H2 REFUSED_STREAM 的默认策略 negative control 应 wire3/capture1；生产三组应 1/1、1/1、2/2。补查 GOAWAY、其他可恢复失败及计数。
- `incomplete/max_output_tokens` 不重复同 cap 重采样；Native HTTP、Rig、WS 的错误不能被后续 pending、disconnect、completed 覆盖。检查 usage/部分输出的保留及有无新的字段遗漏。

### 历史、凭据与容量

- v3 hosted replay 的 response/segment 身份、完整/部分布局、去重、pair 顺序、pause continuation 和硬上限；不得重写已保存历史或注入重复片段。
- provider/model/wire/bridge/endpoint/auth-domain 的实际请求来源绑定、credential rotation、匿名和非 Bearer 鉴权；凭据实例仅私有比较，随机 ID 持久化，真实 key/hash 不进历史或诊断。
- 跨进程恢复不保证旧 opaque 可回放，但普通可见历史保留；不要误标全面兼容。
- raw request/all-attempt capture、最终 wire budget、pipe/probe 限额、失败场景首轮历史与证据保留、错误优先级及取消无后台遗留。
- 保留 pause 递归函数的显式 `impl Future + Send` 和必要的 manual_async_fn lint 例外，不要为修 lint 改坏 future 形状。

### 多进程环境配置与集成面

- 同容器不同 vendor/model/key 的进程隔离；独立绝对 CODEX_HOME、SQLite 路径优先级、`.env` 前缀过滤、EnvSeed 来源隔离、CLI/profile/project/managed 的优先级。
- NUWAX max output cap 已支持三协议；高级 retries/timeout/headers 的直接 env 开关尚未开发，不能标为完成。
- active NUWAX 排除共享 daemon，后台清理启动环境，remote host 的环境归属，config-only provider resume 覆盖正确。
- 同 home 的 `resume --last`、并发 config 写和跨 PID namespace 的 runtime 仍有已登记边界，不能用 SQLite 独立或 writer lock 推导全部隔离。
- 官方合并后的 CLI、app-server API、rawResponseItem、config 和旧 rollout 兼容性；保留六参 config loader wrapper 的外部调用兼容。

## 3. 验证要求

1. 不直接运行 `cargo test` / 裸 nextest；遵循 AGENTS 使用 `just test`，保留仓库 stack/default profile。不要使用 routine `--all-features`。
2. 新建隔离 CARGO_TARGET_DIR，重建当前目标提交的 Exec 和测试 helper。旧 `/tmp` binary 的 receipt 绑定旧 SHA，不能用于提交后的验收。核对 build receipt/source identity、二进制 SHA 和实际运行路径。
3. 先相关 crate 的完整或有解释的定向验证：api/rig/config/utils-cli/model-provider/history、Core/Exec、app-server/daemon/TUI、受类型迁移影响的消费者。live-tests 的离线 library、replay 与付费 live 明确分开。
4. 至少复验请求控制 tasks.md 的 native pending/error/completed、H2 negative control、输出 cap、不重采样、cancel、RawValue、WS reuse，以及容器文档的双进程真实 HTTP 用例。
5. 独立进行 MiMo/GLM 三协议的正常文本、工具往返、旧会话切换、上下文/输出边界与错误场景验证；Step 仅使用已配置协议。凭据取 gitignored `.env.local`，禁止复制密钥到聊天/文档/fixtures。限制厂商并发和调用量，不盲跑付费完整矩阵。
6. 记录 selected/executed/asserted/pass/fail/skipped/timed_out、实际 HTTP path/model/字段/事件/历史、命令、退出码、source/binary identity。非空文本、schema 被接受、进程 exit0 各自不等于功能服从或全覆盖。
7. 原失败轮次保留，串行补验分别登记；先排除缺 helper、目录 IO、资源争用，不直接放宽 deadline、snapshot、字段校验来制造绿灯。
8. 完整 workspace 及额外高成本矩阵按 AGENTS 先取得用户授权；未执行的门禁明确列出。最终 scoped fix/fmt 的顺序遵循仓库要求。

### 本机 V8 helper 准备

`codex-code-mode-host` 的默认 upstream sandbox archive 地址在上轮返回404，Python默认CA也缺失。
按 `third_party/v8/README.md` 与当前 pin 下载对应 Codex archive/binding pair，验证仓库 checksum 后设置 `RUSTY_V8_ARCHIVE`、`RUSTY_V8_SRC_BINDING_PATH` 再构建。不得关 TLS 校验、升级 pin 或使用不匹配版本。必要时阅读 `update-v8-version` skill；这是本地准备，不是 hosted V8 canary 验收。

## 4. 修复与交付

确定的小问题直接修复，增加能复现的回归；保留并适配其他人的工作，不扩大到无关功能或全局环境改动。
大改动先按 Spec/Plan/Tasks 拆阶段，明确剩余任务；不能仅为提交删掉必要测试或降低严格语义。
不要自动 commit/push/发布，完成后交给用户确认。

最终报告保存为 `my-docs/claude-checkpoint-review-results-2026-10-03.md`，按以下顺序：

- 目标 SHA 与实际验证树；P0/P1/P2 问题、路径/行号、影响、复现、修复。
- 当前字段支持与协议限制表，新增/残余功能遗漏。
- 逐层验证证据，明确历史记录与本轮实测的区别。
- 原失败、环境准备、隔离补验，未验/阻断门禁。
- 是否可提交修复、后续任务与每项完成标准。

不要沿用 Codex 上轮数字作为本轮通过证据，也不要把重叠批次的测试数相加为唯一用例数。
