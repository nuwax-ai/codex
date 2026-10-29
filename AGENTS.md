# AGENTS.md — nuwax-codex fork 工作入口

本仓库是 [openai/codex](https://github.com/openai/codex) 的 fork,目标是让 codex
支持中国厂商模型(GLM / MiMo / StepFun / DeepSeek / Ollama 等)。**动手前先读下面两份
根目录文档**,它们分别是使用侧与行为侧的权威索引:

| 文档 | 回答什么 | 什么时候读 |
|---|---|---|
| [NUWAX-HARNESS.md](NUWAX-HARNESS.md) | 怎么装、怎么配模型、`NUWAX_*`/`CODEX_*` 环境变量、config.toml、ACP 启动 | 任何涉及配置、启动、环境变量、使用方式的问题 |
| [FORK.md](FORK.md) | fork 与官方的行为差异:协议路由、窗口语义、压缩、品牌、已知限制 | 任何涉及"为什么这样实现"/"改这个会不会破坏合同"的问题 |

配套无凭据模板:[nuwax.env.example](nuwax.env.example)(harness CLI)、
[acp.env.example](acp.env.example)(ACP agent)。**API key 只走环境变量,不写进任何
提交文件。**

## 必读约定

- **改 fork 行为必须同步更新 [FORK.md](FORK.md)**;使用侧变化(新环境变量、安装方式、
  配置字段)同步更新 [NUWAX-HARNESS.md](NUWAX-HARNESS.md)。这两份是给人和 agent 看的
  入口,不是附属说明。
- **Fail Fast**:配置非法尽早报错并点名变量,不静默取默认值。跨协议无法表达的字段
  要么 fail-fast,要么显式告警,不静默丢弃。
- **不在引擎里做黑盒兼容**:跨网关差异(某个网关不接受某个字段值)应由环境变量显式
  设置,而不是桥在背后静默改写——否则"用户配了什么"与"线上发了什么"不一致,问题只能
  从抓包反推。实例见 [FORK.md §10](FORK.md) 的 `thinking` 模式一行。
- **生产代码禁用 `unwrap()`/`expect()`**(测试可用);Rust 项目不用 `unsafe`(FFI 除外),
  HTTP 接口用 utoipa 补 OpenAPI;dashmap 并发场景只用 entry API。
- **测试用 `cargo nextest`**;Mac 上跑 core 集成测试需 `RUST_MIN_STACK=33554432`
  (成批 ABRT 先怀疑环境,不是回归)。
- **insta 快照**:不要用宽过滤器 `INSTA_FORCE_UPDATE`(会翻刷大量无关快照);用窄过滤器
  逐个接受,并先目视核对 diff。

## 常用路径

| 内容 | 位置 |
|---|---|
| rig 桥(协议转换核心) | `codex-rs/codex-rust-rig-bridge/` |
| 中立桥接 trait | `codex-rs/codex-api/src/bridge.rs` |
| 协议/provider 定义 | `codex-rs/model-provider-info/src/lib.rs` |
| TUI(含欢迎动画) | `codex-rs/tui/src/`;动画在 `empty_state_animation/` |
| ACP 适配器(另一仓库) | `/Volumes/zhitai/git_workspace/fork-codex-acp-ts` |
| 历史台账与设计文档 | `my-docs/`(入口 `my-docs/ROADMAP.md`) |

## 构建与安装

```bash
# 源码装到本机(不等 npm 发布);V8 env 文件由 prefetch 脚本生成
cd codex-rs && set -a && source /tmp/nuwax-v8.env && set +a
CARGO_TARGET_DIR=~/.nuwax-cargo-target cargo install --path cli --bin codex \
  --root ~/.nuwax-cargo --locked --force
ln -sf ~/.nuwax-cargo/bin/codex ~/.cargo/bin/nuwax-harness
```

装到独立 root 再软链成 `nuwax-harness`,**不要**把 fork 的 `codex` 装进
`~/.cargo/bin`(会与 homebrew 官方 codex 冲突)。源码构建版本恒为 `0.0.0`,
可用来区分 npm 版。发布流程见 [FORK.md §8](FORK.md)。
