# nuwax-codex 分层默认配置目录 — Tasks（执行清单）

统一使用隔离 `CARGO_TARGET_DIR=/tmp/codex-nuwax-home-target`、仓库根 `just test`（8 MiB 测试线程栈）。日期：2026-09-28。

## T1 代码

- [x] T1.1 新增 `codex-rs/utils/home-dir/src/nuwax_default.rs`：`find_default_codex_home(user_home)` 承载默认目录决策（探测 `.codex-nuwax`：目录选中/不存在回落 `.codex`/文件与悬空链接硬错误；选中路径不 canonicalize）
- [x] T1.2 `codex-rs/utils/home-dir/src/lib.rs`：None 分支委托 fork 模块（`// Fork:` 标记），`find_codex_home()` doc 注释更新；上游 None 测试改为委托断言（原测试在存在 `~/.codex-nuwax` 的机器上必挂）
- [x] T1.3 新增 `codex-rs/utils/home-dir/src/nuwax_default_tests.rs`（`#[path]` 兄弟文件，TempDir 作 user home，hermetic）

## T2 测试

- [x] T2.1 定向：`CARGO_TARGET_DIR=/tmp/codex-nuwax-home-target just test -p codex-utils-home-dir` → **10/10 passed, 0 skipped，exit 0**（6 个新 fork 用例：回落/目录选中且不 canonicalize/普通文件硬错误/悬空链接硬错误/符号链接选中原路径/并存时胜出；3 个上游 env 用例；1 个委托用例）
- [x] T2.2 预构建测试辅助二进制（隔离 target 冷目录缺产物会让大量用例失败）：`cargo build -p codex-rmcp-client --bin test_stdio_server --bin test_streamable_http_server --offline`、`cargo build -p codex-cli --bin codex --offline` → 均 exit 0。`codex-code-mode-host` **本机无法构建**：rusty_v8 v150.4.0 的 `librusty_v8_ptrcomp_sandbox_release_aarch64-apple-darwin.a.gz` 在 GitHub release 返回 HTTP 404（归档不存在，非网络问题）；源码编译 V8 不可行。该限制与本改动无关（历轮本地验证均只跑定向子集，从未全量跑过 codex-core）
- [x] T2.3 回归：`just test -p codex-config -p codex-core --offline --retries 0` → **5059 项：4855 过 / 200 败 / 4 超时 / 27 跳过**。**codex-config 全绿（0 失败）**。200 个失败全部位于 codex-core，逐类归因：
  - 147 个用例名含 code_mode、其余 53 个经深查同样依赖 `codex-code-mode-host`（guardian/skills/scenarios/cli_stream 等经 code-mode host 启动）——全部根因是上述 V8 归档 404 无法构建二进制
  - 因果排除实验（stash 基线对照）：53 个非 code_mode 名用例 + 3 个超时项，**基线两次完全一致（56 run：17 过/38 败/1 超时）**，带改动 12-13 过；差异的 4 个用例（guardian review_session、shell_snapshot、cli_stream oauth refresh、tool_parallelism）**全部为时序敏感型**（断言并行耗时阈值、60s 超时），在低负载单独运行时**带改动 4/4 通过（复跑两次）** → 批量差异为负载抖动，非本改动回归
  - 其余 38 个失败在基线与带改动两侧完全一致（预存在：依赖缺失或分支既有失败），完整对照日志 `/tmp/nuwax-batch-{baseline,with-change}.log`
- [x] T2.4 收尾：`just fmt` → exit 0；`just fix -p codex-utils-home-dir` → exit 0（1 处外观修复，零告警）；`git diff --check` → exit 0（fix 后按 AGENTS.md 未重跑测试）

## T3 手动端到端（`/tmp/codex-nuwax-home-target/debug/codex`，HOME 仅作用于子进程，tmp 家目录）

- [x] T3.1 无 `.codex-nuwax` → doctor state 段 `CODEX_HOME ~/.codex (dir)`、`config.toml ~/.codex/config.toml`
- [x] T3.2 `mkdir $TMP/.codex-nuwax` + `model = "gpt-5.1-codex"` 标记 → doctor state 段 `CODEX_HOME ~/.codex-nuwax (dir)`，config 段 `config.toml ~/.codex-nuwax/config.toml · parse ok`、**`model gpt-5.1-codex · openai`（标记生效）**，log/sqlite 全部指向 `.codex-nuwax`（全隔离）
- [x] T3.3 `CODEX_HOME=$TMP/custom` → doctor 显示 canonicalize 后的 custom 路径（显式优先，含 config.toml 路径）
- [x] T3.4 `.codex-nuwax` 为普通文件 → `codex exec` **硬失败**：`Error finding codex home: .../.codex-nuwax exists but is not a directory; remove it or make it a directory to isolate the nuwax-codex config home`，exit 1。注：arg0 的 PATH 别名创建是上游设计的尽力而为路径，任何错误降级为 WARNING 后继续（错误信息原样展示）；主配置路径不受影响、按设计 Fail Fast
- [ ] T3.5 官方 codex 不受影响：机制上成立（官方二进制不读 `.codex-nuwax`、本改动不影响其代码）。真机最终验收待用户确认创建 `~/.codex-nuwax` 后与 GLM 配置一起做（见下）

## T4 文档与提交

- [x] T4.1 `my-docs/nuwax-home/{spec,plan,tasks}.md` 三件套
- [x] T4.2 `my-docs/codex-review-prompt.md` §二登记补丁面（`utils/home-dir`）
- [ ] T4.3 提交分组：代码+测试一个提交、文档一个提交（等用户确认后执行，不自动 commit/push）
