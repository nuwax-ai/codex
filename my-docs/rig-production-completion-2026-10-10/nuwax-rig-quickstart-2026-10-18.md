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

## 固定实际运行的 Codex

在 `fork-nuwax-codex` 的 `test` 分支源码中构建包含 rig 的 CLI：

```bash
cd /Volumes/soddy/git_workspace/fork-nuwax-codex/codex-rs
cargo build --release -p codex-cli --features codex-core/rust-rig
export CODEX_PATH='/Volumes/soddy/git_workspace/fork-nuwax-codex/codex-rs/target/release/codex'
```

Linux/容器把路径换成相应绝对路径。`CODEX_PATH` 指向 **codex CLI**，ACP 会用它启动 `app-server` 子命令；不要填 codex-exec 或 codex-app-server。切换构建后检查二进制SHA和路径，避免仍运行npm依赖附带的旧版本。

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
