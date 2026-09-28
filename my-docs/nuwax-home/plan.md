# nuwax-codex 分层默认配置目录 — Plan（技术方案）

## 1. 唯一收口点

目录解析集中在 `codex-rs/utils/home-dir/src/lib.rs` 的 `find_codex_home()` → `find_codex_home_from_env(Option<&str>)`。全部二进制（cli/exec/app-server/arg0/daemon/sandboxing/rmcp/install-context）直接或经 `codex_core::config::find_codex_home`（纯转发）汇入；`ConfigBuilder::build_inner` 未注入 home 时也调它。改 None 分支一处即全覆盖，无需逐入口修改。

选中的 home 由 `Config.codex_home` 字段向下游传递，配置加载（`load_config_layers_state` 以参数接收 home）、profile 文件（`resolve_profile_v2_config_path`）、auth、sessions、日志等全部跟随，无第二套解析。

## 2. 模块划分

- **fork 拥有**：`utils/home-dir/src/nuwax_default.rs`，`pub(crate) fn find_default_codex_home(user_home: &Path) -> io::Result<AbsolutePathBuf>` 承载整个默认目录决策（探测 `.codex-nuwax` → 选中或回落或报错）。纯函数、无环境依赖、hermetic 可测。
- **上游最小 diff**：`lib.rs` 的 None 分支取 `home_dir()` 后委托上述函数（`// Fork:` 标记，约 4 行），doc 注释同步更新。
- **测试**：`nuwax_default_tests.rs`（`#[path]` 兄弟文件惯例），TempDir 作 user home。

探测语义：`symlink_metadata` 区分"路径上没有东西"（`NotFound` → 回落）与"有东西但不可用"（报错）；`fs::metadata`（跟随符号链接）判定是否目录；非 NotFound 的探测错误原样传播。选中路径不 canonicalize——`config/src/codex_home_symlink.rs` 的别名保留只读原始 `CODEX_HOME` 环境变量，fork 分支行为与"`~/.codex` 本身是符号链接"的既有上游语义一致。

## 3. 上游测试适配

原 `find_codex_home_without_env_uses_default_home_dir` 用真实 `~` 断言 `.codex`——机器上存在 `~/.codex-nuwax` 后必挂。改为委托测试：`find_codex_home_from_env(None)` 必须等于 `find_default_codex_home(home_dir())`，任何机器上恒成立；`~/.codex` 回落契约由新纯函数的 hermetic 测试承载。

## 4. 明确不改（已审计）

- `CODEX_HOME` 显式分支；`cli/src/doctor`（显示生效路径、磁盘检查兜底走收口点，自动正确）；`tui/src/external_editor.rs:185`（编辑器候选目录回落，外观性）；`scripts/install/install.sh`（上游安装脚本，npm 路径不用）；config schema 描述文案。
- Bazel：`defs.bzl` 用 `glob(["src/**/*.rs"])` 收录源码，新文件无需改 BUILD.bazel；无依赖变更 → 无需 `bazel-lock-update`；不改 `ConfigToml` → 无需 `write-config-schema`。

## 5. 后续阶段（另行设计）

自动迁移助手（引导复制 auth.json/初始化配置）、`--global` 类命令的目标目录提示、npm 包 README 用户文档。
