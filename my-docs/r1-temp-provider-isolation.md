# R1：临时 provider 配置隔离与 fail-fast（Spec / Plan / Tasks）

日期：2026-10-01。需求来源：`codex-review-2026-10-01.md` R1（条目 1–3 与
OSS 诊断边界）。本文件为实施层规划；验收以审查文档 T01 为准。

## Spec（做什么）

1. **来源冲突硬错误（P1）**：当有效配置选中的 provider 为保留 id
   `nuwax_env` 时，其定义必须**只**来自环境组种子（name/base_url/
   wire_api/env_key 四键）。任何其他来源（配置文件 / profile / 项目配置 /
   `-c`，含 `-c model_providers.nuwax_env.<子键>`）贡献的字段（如
   http_headers、env_http_headers、auth、aws、query_params 等）在**有效
   配置加载阶段**产生明确错误：列出冲突字段名（永不列出值），说明来源与
  补救（移除字段或取消环境组）。不得追加覆盖、不得静默清空用户配置、
   冲突时零请求、磁盘配置不被改写。
2. **正规 URL 校验（P2）**：`validate_base_url` 改用 `url::Url` 解析：拒绝
   无 host、非法端口/IPv6、非 http/https scheme、带凭据 userinfo 的值；
   错误不包含值。依赖变化同步 Cargo/Bazel 锁（可行时）。
3. **验收缺口补齐**：doctor 对模型/provider/endpoint 逐字段来源说明
   （环境组 / `-m`、`-c` / 配置文件）；共享 daemon 的双客户端隔离子进程
   验证（两个不同凭据的客户端，daemon 子进程环境零残留）。
4. **OSS 诊断边界**：普通 `--oss`（未显式 local provider）时 doctor 与
   TUI/exec 的本地 provider 解析对齐——能无副作用复用则复用，否则登记
   边界与原因。

### 非目标

- 不改变环境组优先级语义（F05/F06 已修复的部分不动）。
- 不为 doctor 重建全字段 provenance 体系（仅上述关键字段）。

## Plan（如何实现）

- **常量归一**：`NUWAX_ENV_PROVIDER_ID` 单一来源移至
  `codex-protocol::config_types`；utils-cli 改为 re-export（外部 API 不变）；
  codex-config 引用同一常量。
- **加载阶段校验**：codex-config 新模块 `env_group_isolation.rs`：
  `validate(merged: &TomlValue) -> Result<(), io::Error>` ——当
  `model_provider == NUWAX_ENV_PROVIDER_ID` 且
  `model_providers.nuwax_env` 表存在时，键集必须 ⊆ 四个种子键，否则
  `InvalidData` 错误（排序后的冲突键名 + 来源说明 + 补救）。挂接点：
  core `config_toml_from_layers`（初始加载与 refresh 共用），全部四个
  入口（exec/tui/app-server/doctor）经此获得；utils-cli 的 exact-key
  `-c` 预检保留（更早、更具体的报错）。
- **URL 校验**：utils-cli 加 `url = { workspace = true }`；
  `validate_base_url` 用 `Url::parse` + scheme/host/userinfo 检查。
  Cargo.lock 变化提交；`just bazel-lock-update` 在 bazel 可用时执行，
  否则登记 not-run。
- **doctor 来源说明**：doctor 已自算 seeds（`model_cli_overrides`）——
  将"组是否激活 / model 是否来自 NUWAX_MODEL"传给 model_routing check，
  输出 `provider source:`/`model source:` 行；OSS 对齐项评估
  TUI/exec 的 provider 解析函数可否无副作用复用。
- **双客户端隔离**：`pid_start_tests` 增加
  `two_clients_with_different_temporary_credentials_do_not_leak_into_shared_daemon`
  ——两次隔离子进程启动（不同 NUWAX_API_KEY），断言两条 daemon 子进程
  env 文件均为 unset。

### 测试

- codex-config 单元：合并 TOML 层面的键集校验（合法四键 / 各类冲突键 /
  未选中不报错 / 无表不报错）。
- core `config_loader_tests`：真实 ConfigBuilder + 私有临时 HOME，四种
  来源各一例 → Err 且错误含字段名与保留 id；加载后 HOME 内文件字节不变。
- exec 端到端：`nuwax_env.rs` 新增冲突用例——HOME 写入冲突
  `model_providers.nuwax_env.http_headers`，完整环境组启动 → 非零退出、
  stderr 命名冲突、本地服务器零请求、配置文件字节不变。
- utils-cli 单元：URL 校验合法/非法表（`http://@`、无 host、带 userinfo、
  非 http(s)、IPv6 合法、端口非法）。

## Tasks

- [x] T1 常量归一 + codex-config 校验模块与单元测试（`config/src/env_group_isolation{,_tests}.rs`，7 用例；常量在 `protocol/src/config_types.rs::NUWAX_ENV_PROVIDER_ID`，utils-cli re-export）
- [x] T2 core builder 挂接（`config_toml_from_layers`，初始加载与 refresh 共用）+ 四来源回归 + 原配置未改写断言
- [x] T3 exec 端到端零请求回归（`nuwax_env_reserved_provider_conflict_fails_before_any_request`：非零退出、stderr 含保留 id 与字段名且不含值、服务器零请求、配置文件字节不变）
- [x] T4 URL 正规校验（`url::Url`：无 host/非法端口/userinfo 凭据全部拒绝，错误不含值）+ Cargo.lock 同步（+1 行）；`just bazel-lock-update` exit 0、MODULE.bazel.lock 无变化（url 已在依赖图中）
- [x] T5 doctor 来源说明（`model source:`/`provider source:` 行 + plain `--oss` 边界说明行）；OSS 对齐：无副作用部分仅 `resolve_oss_provider`（显式或 config 值），交互式本地发现（lmstudio/ollama 检测）不复制，报告内明示边界
- [x] T6 双客户端 daemon 隔离测试（`two_clients_with_different_temporary_credentials_do_not_leak_into_daemon_children`：两组不同凭据的隔离子进程，两条 daemon 子路径 env 全 unset）

## 实施发现与偏差

- **项目配置来源结构性排除**：上游 loader 对项目 `.codex/config.toml` 的
  `model_providers` 一律忽略并出 startup warning（"Ignored unsupported
  project-local config keys … model_providers"），该来源不可能给保留
  provider 贡献字段。回归改为断言此边界（加载成功 + warning + 文件未改写），
  实际会冲突的来源是：用户配置文件 / profile / `-c`（含子键）三种，全部
  由加载期校验覆盖并各有真实 ConfigBuilder 用例。
- profile-v2 选择必须同时给 `user_config_path`（profile 文件路径）与
  `user_config_profile`（名称）——与 exec 入口一致；仅给名称不会加载
  profile 层（首版回归因此误判，已修正）。
- 未选中保留 id 且带外键的陈旧定义：不报错（无请求可到达，无泄漏面），
  为登记边界；utils-cli 对 `-c` 显式定义的既有预检不变。

## 验证证据（2026-10-01，cwd codex-rs，隔离 target）

环境：`CARGO_TARGET_DIR=/tmp/codex-stability-20260930-target`、
`CARGO_BUILD_JOBS=4`、`--offline --locked --retries 0`。

| 命令 | 结果 |
|---|---|
| `just test -p codex-config -p codex-utils-cli -p codex-cli --test-threads 4 -E 'test(env_group_isolation) | test(nuwax_env) | test(doctor_) | test(model_routing) | test(model_config)'`；`/tmp/r1-tests-1.log` | exit 0；34 run / 34 pass（含 7 隔离单元、URL 新用例、doctor 来源行） |
| `just test -p codex-core --features rust-rig --test-threads 1 -E 'test(nuwax_env_group)'`；`/tmp/r1-four-source-5.log` | exit 0；2 run / 2 pass（四来源 + 干净加载）；此前失败轮的定位记录在 `/tmp/r1-four-source-fail.log`（case 标签误配与 profile 路径缺失，均已修正） |
| `just test -p codex-exec -p codex-app-server-daemon --test-threads 2 -E 'test(nuwax_env) | test(do_not_capture_client_model_seeds) | test(two_clients_with_different)'`（允许 ps 的执行环境）；`/tmp/r1-tests-3.log` | exit 0；7 run / 7 pass（exec 三线 + 部分组 + 冲突零请求；daemon 单/双客户端擦除） |

未运行：完整 workspace、Linux/Windows。fix/fmt 后按 AGENTS.md 不重跑测试。
