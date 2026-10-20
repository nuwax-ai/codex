# 自定义模型接入与试用（2026-10-18）

核心接入已实现。兼容 OpenAI Chat Completions、OpenAI Responses 或 Anthropic Messages 的平台，可以通过环境变量选择端点和模型；通常无需为厂商增加专用代码。模型自身的工具调用、视觉、上下文长度仍需按其能力配置和验证。

## 四个运行参数

以 GLM Chat 为例（Key 只填自己的值，不提交到仓库）：

```bash
export NUWAX_BASE_URL='https://open.bigmodel.cn/api/coding/paas/v4'
export NUWAX_WIRE_API='chat'
export NUWAX_MODEL='GLM-5.3-Flash'
export NUWAX_API_KEY='填入自己的Token Plan Key'
```

`NUWAX_WIRE_API` 可选 `chat`、`responses`、`anthropic`。换供应商时替换这四项即可；Base URL 使用对应协议的基址。BASE_URL、WIRE_API、API_KEY 需成组提供。显式 CLI/provider 覆盖的优先级高于环境变量，排查时也要检查宿主的 `CODEX_CONFIG`、`MODEL_PROVIDER` 和启动参数。

自定义型号还应按平台公布的真实token窗口设置已有的 `CODEX_MODEL_CONTEXT_WINDOW`（正整数），并配置适合该模型的输出上限。未知型号的默认窗口并非自动探测结果，不能用较大的默认值替代真实限制；图片和输出也会消耗窗口。环境值会进入本地压缩和最终请求预算，显式 `-c model_context_window=...` 的优先级更高。

## 可选第五项：NUWAX_BRIDGE（2026-10-20）

`NUWAX_WIRE_API=responses` 默认经 rig 桥访问。如果中转后面其实是官方 GPT 模型，想让请求走上游原生 Codex Responses 传输，再导出一个变量即可：

```bash
export NUWAX_BRIDGE='native'   # rig（默认）| native | genai
```

`native` 必须与 `NUWAX_WIRE_API=responses` 同时出现（chat/anthropic 两条线只由桥服务，配 `native` 会在启动时直接报错并说明需要哪个变量）。凭据仍来自 `NUWAX_API_KEY`，不依赖 auth.json 或官方订阅登录；执行记录、取消、压缩、冷恢复与三条线共用同一套协议栈。共享 daemon 与 live 子进程会剥离该变量，不会把某个客户端的传输选择带给其他进程。

验证是否真的离开桥：`codex doctor` 的 model routing 检查会打印 `bridge: native transport` 与 `experimental bridge override: native`；设置 `NUWAX_BRIDGE=rig` 或不设置时打印 `bridge: rig (model bridge)`。

## 固定实际运行的 Codex

在 `fork-nuwax-codex` 的 `test` 分支源码中构建包含 rig 的 CLI：

```bash
cd /Volumes/zhitai/git_workspace/fork-nuwax-codex/codex-rs
cargo build --release -p codex-cli --features codex-core/rust-rig
export CODEX_PATH='/Volumes/zhitai/git_workspace/fork-nuwax-codex/codex-rs/target/release/codex'
```

**构建前先确认你在哪个副本上。** 本机存在两个互相独立的 `nuwax-ai/codex` clone，都在
`test` 分支：`/Volumes/zhitai/…` 是本轮收口用的工作副本；`/Volumes/soddy/…` 落后于它，
从那里构建会得到修复前的二进制。`git -C <目录> log --oneline -1` 应当是远端最新提交，
不要用旧 worktree（`.claude/worktrees/merge-main-20261014`）当源码。切换构建后检查二进制
SHA 和路径，避免仍运行 npm 依赖附带的旧版本。

Linux/容器把路径换成相应绝对路径。`CODEX_PATH` 指向 **codex CLI**，ACP 会用它启动 `app-server` 子命令；不要填 codex-exec 或 codex-app-server。

先用同一个 CLI 做文字冒烟，再让现有 ACP 宿主启动 `fork-codex-acp-ts` 的 `dev` 分支：

```bash
"$CODEX_PATH" exec --skip-git-repo-check '请只回复 OK'
cd /Volumes/soddy/git_workspace/fork-codex-acp-ts
node --import tsx src/index.ts
```

ACP 使用stdio协议，由现有ACP宿主完成初始化/认证/会话请求。宿主的启动命令使用 `node --import tsx src/index.ts`，工作目录设为ACP仓库；若使用npm则需 `npm --silent start`，避免npm启动横幅污染协议stdout。确保 `CODEX_PATH` 和四个 `NUWAX_*` 同时进入 **ACP agent进程环境**，其子进程才能继承。沿用你当前宿主的API-key认证配置。

## 本次收口范围

已实测国内模型的三协议主链、工具、图片、取消、压缩、冷恢复和恢复用量。完整结果与具体源码/二进制对应关系以本轮复审执行记录为准。

可以进入Linux/容器ACP主路径的日常试用，不再为核心接入增加一轮泛化功能开发。Windows ConPTY/提权runner握手、跨平台全量CI和正式安装包发布属于后续兼容或交付工作；它们没有被宣称全部完成。

字段和图片的完整保真边界，以及有限的最后收尾项，见[rig-field-image-closure-2026-10-18.md](rig-field-image-closure-2026-10-18.md)。图片走专用内容块，默认内联Base64，并非通用Files API上传；图片仍计视觉token，不能宣称附件引用会使上下文成本归零。
