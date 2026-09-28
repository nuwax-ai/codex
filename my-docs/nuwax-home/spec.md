# nuwax-codex 分层默认配置目录 — Spec（规范）

日期：2026-09-28　状态：已实现（证据见 tasks.md）

## 1. 背景与问题

fork（nuwax-codex）与官方 codex 共享 `~/.codex/config.toml`。fork 需要官方 schema 不认识的配置字段（`experimental_bridge`、`wire_api = "anthropic"`、`experimental_bearer_token` 等），写入共享文件后官方 codex（0.157.1，严格校验）启动即失败。2026-09-28 已发生过一次该事故；根因是机制性的：两个二进制没有各自的默认配置目录。

## 2. 需求

nuwax-codex 在未显式指定目录时，自动优先使用 fork 专属目录，同时保持与官方默认兼容：

| 优先级 | 来源 | 行为 |
|---|---|---|
| 1 | `CODEX_HOME` 环境变量（非空） | 现行语义完全不变：必须存在、是目录、canonicalize |
| 2 | `~/.codex-nuwax` 存在且是目录（符号链接指向目录也算） | 使用它：config.toml、`<profile>.config.toml`、auth.json、sessions/、history.jsonl、log/、`.env`、MCP OAuth keyring 命名空间与锁、sqlite、installation_id、app-server socket/daemon 全部隔离 |
| 3 | 否则 | 回落 `~/.codex`（上游默认，不校验存在性）：开箱即用，可复用官方 ChatGPT 登录与既有配置 |

"额外目录"需求由优先级 1 覆盖：`CODEX_HOME` 可指向任意目录，两二进制语义一致。不新增 `NUWAX_*` 环境变量（fork 惯例沿用 `CODEX_` 前缀）。

官方 codex 的目录解析零改动：它始终解析自己的 `~/.codex`，`~/.codex-nuwax` 的存在与否对它无影响——隔离与互不干扰由机制保证。

## 3. `.codex-nuwax` 状态语义（Fail Fast）

| 状态 | 结果 |
|---|---|
| 不存在 | 回落 `~/.codex` |
| 目录（含指向目录的符号链接） | 选中；返回拼接路径本身，不 canonicalize（与上游默认分支处理符号链接 home 的语义一致） |
| 普通文件 | 硬错误，信息含 `.codex-nuwax` 与修复指引 |
| 悬空符号链接 | 硬错误，同上 |

半配置状态必须响亮报错，不静默回落——否则用户的隔离配置会悄悄失效。

## 4. 非目标

- 不做多目录配置合并/叠加（避免三层覆盖语义，控制与上游 diff）
- 不做自动迁移（复用官方登录：`cp ~/.codex/auth.json ~/.codex-nuwax/`，文档说明即可）
- 不改动 `CODEX_HOME` 显式分支、doctor、`tui/external_editor.rs` 候选目录回落、上游 install.sh
- 不加新依赖、不改 config schema

## 5. 验收标准

1. `~/.codex-nuwax` 不存在时行为与上游完全一致（`.codex` 默认）
2. 存在时全部 fork 状态落隔离目录，官方 `~/.codex` 一个字节不改
3. 官方 codex 在 `~/.codex-nuwax` 存在时照常工作（事故场景机制性回归）
4. hermetic 单测覆盖全部状态分支；`codex-config`、`codex-core` 回归通过（两者测试均注入 home）
5. fork 补丁面登记到 `my-docs/codex-review-prompt.md` §二
