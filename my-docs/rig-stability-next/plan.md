# Rig 稳定性下一轮 — Plan（如何实现）

按可独立构建的小批次；复杂批次 <500 改动行。所有批次 tests → scoped fix
→ fmt 收尾。统一 `cwd=codex-rs`，
`CARGO_TARGET_DIR=/tmp/codex-stability-20260930-target`，`just test`。

## 阶段 A（N1 来源隔离 + N5 预算规范）

### A1 codex-config：EnvSeed 层与层级隔离校验
- `config_layer_source.rs`：新增 `ConfigLayerSource::EnvSeed`（precedence
  27：Project 25 < EnvSeed < SessionFlags 30）；`format_config_layer_source`。
- 全部穷尽 match 补臂（state.rs `config_folder`、diagnostics.rs、mcp_ema.rs、
  loader/local.rs、loader/mod.rs、ext/skills/host_roots.rs、
  hooks/engine/discovery.rs、tui/debug_config.rs、
  app-server-protocol v2/config.rs 镜像枚举 + schema 再生成）。
- `ConfigLoadOptions` 增 `env_seed_overrides: Vec<(String, TomlValue)>`；
  `load_config_layers_state` 用 `build_cli_overrides_layer(env_seed)` 构造
  EnvSeed 层（经 strict 校验与相对路径解析，与 CLI 层同规则），按 precedence
  插到 SessionFlags 层之下。
- `env_group_isolation.rs` 重写为层级校验：选中保留 id 时 (1) EnvSeed 层必须
  贡献保留表；(2) 其他任何层（文件/profile/system/managed/project/CLI/
  thread/legacy managed）贡献保留表任意键 → 硬错误（报层来源与键名，不报
  值）；(3) requirements 定义保留表 → 硬错误；(4) merged 表键 ⊆ 四 seed 键
  （防御性兜底）。
- 单元测试：同名字段（base_url/env_key）各来源、requirements、无 seed 选中、
  非 adopted 组、seed-only 通过。

### A2 core：seed 独立通道 + `is_session_layer` + 加载矩阵
- `ConfigBuilder.env_seed_overrides(...)`；build_inner 注入
  ConfigLoadOptions；`is_session_layer` 含 EnvSeed（refresh 保留）。
- config_loader_tests 扩展：文件同名 base_url、requirements 定义表、
  model-only seed 选中、-c 同名子键（现有）保持。

### A3 二进制与 doctor
- exec/tui/app-server：seed 不再并入 cli vec；独立传给
  ConfigBuilder/ConfigManager（`ConfigManager::new` 增参或 builder）。
  TUI 内嵌 app-server（in_process `InProcessStartArgs`）同步。
- doctor `routing_sources`：按层栈事实（EnvSeed 层 + required/typed/-c 推
  导）判定 provider 来源；endpoint（base_url）来源用层栈 origins。删除重复
  推导 seeds 的旁路。
- exec 端到端零请求回归（已有）保持；补 app-server 子进程 JSON-RPC 回归见
  A4。

### A4 app-server JSON-RPC 隔离回归
- TestAppServer 子进程 `with_env_overrides(NUWAX_*)` 指向本地 mock HTTP；
  `thread/start` config 覆写 `model_providers.nuwax_env.base_url`/`env_key`
  → JSON-RPC 错误 + mock 收到 0 请求；resume/fork 同参数同样错误。

## 阶段 B（N2/N3，先独立 Spec/Plan/Tasks 文档）
- B0 `my-docs/rig-stability-next/n2-n3-*.md` 三文档。
- B1 response/block 边界载体与正文去重（投影层）。
- B2 混合轮 result-only 追加投影（request_messages）+ 前缀稳定。
- B3 真实 Core 工具闭环矩阵（多搜索/多文本/多次 pause/迟到结果/保存恢复/
  fork）+ 深比较连续请求。

## 阶段 C（N4/N5）
- C1 provenance 消费进投影 + 身份边界定义。
- C2 Core 取消矩阵（Interrupt/超时/重试/续传响应头取消/工具不重复）。
- C3 多次 pause 的 usage/request ID 语义实现与验收。

## 阶段 D（N6）
- D1 codex-exec 构建收据（exec 自身提供并自校验：source SHA、dirty、
  features、target/profile、binary hash 绑定）。
- D2 最终 HTTP 层脱敏 recorder（RigHttpClient 测试通道）。
- D3 失败路径部分 rollout 保留。
- D4 最小 MiMo/GLM live（离线全绿后）。

## 阶段 E（N7）
- E1 schema/proto/锁文件/feature 组合核对与再生成。
- E2 桥/live-tests Bazel 接入范围决定 + 可执行 Cargo workflow。
- E3 平台/npm/remote exec 证据登记与待授权清单。
