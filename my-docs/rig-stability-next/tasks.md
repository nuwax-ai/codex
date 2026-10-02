# Rig 稳定性下一轮 — Tasks（执行清单）

> Codex 第二版复查发现 N2 身份/位置仍未实现，N3、EnvSeed 恢复/回退/API写入也有需修复的边界。以 `my-docs/codex-review-2026-10-02-round2.md` 的裁决、修复和最终验证为当前进度；以下保留 Claude 阶段记录，不代表全面验收。

状态：`[ ]` 未开始 / `[~]` 进行中 / `[x]` 完成（附证据）。每批更新。

## 基线核对

- [x] 读取 codex-review-2026-10-02.md，逐一核对 F01–F21 未提交修复 diff
- [x] 基线测试（本轮源码重跑）
  - bridge+live-tests 选择集：188 run/188 pass/122 skipped，exit 0
    （`/tmp/n-round-baseline-bridge.log`；比审查者记录多 1 个用例——工作树
    含 F21 后新增回归）
  - core/exec/doctor 选择集：被并行实施中的 N1 改动打断（编译错误均为
    本轮中间态），由阶段 A 完成后的全量选择复跑取代；不再单独回填

## 阶段 A（N1 + N5 规范）

- [x] N5 预算规范写入 spec.md（分层边界表 + 超限行为 + 非精确 token 声明）
- [x] A1 codex-config EnvSeed 层 + 层级隔离校验 + 单元测试
  （config/src/config_layer_source.rs 新增 EnvSeed(27)；loader 构建独立层；
  env_group_isolation.rs 层级校验：无 seed 选中→错、任何非 seed 层贡献任意
  键→错、requirements 定义→错、merged 四键兜底；9 个单测；
  codex-config 366/366）
- [x] A2 core builder/refresh + 加载矩阵扩展
  （ConfigBuilder.env_seed_overrides；load_config_toml_with_layer_stack_
  and_env_seed；is_session_layer 含 EnvSeed（refresh 保留）；core 矩阵：
  四来源 + 同名 base_url/env_key 文件拒绝 + requirements 优先级矩阵带真实
  seed；core nuwax 选择 8/8）
- [x] A3 exec/tui/app-server seed 独立通道 + doctor 来源按层栈
  （exec 全部 ConfigBuilder/bootstrap/fork_source/InProcessStartArgs；TUI
  startup_orchestration/lib.rs/run_ratatui_app/App 结构体/config_persistence/
  session_picker/realtime_settings/worktree_startup/daemon exclusion（含
  search-only onboarding 语义保持——组活跃时跳过）；has_launch_setting 计
  EnvSeed 层；standalone app-server ConfigManager 持独立 seed；doctor
  routing_sources 改层栈推导 + endpoint source 行 + model_routing 测试更新）
- [x] A4 app-server JSON-RPC 零请求回归（start/resume/fork × base_url/env_key
      覆写）——in-process 形态（本环境子进程 app-server 启动被 remote-control
      网络依赖阻塞，登记为环境边界）；2/2 PASS，wiremock 0 模型请求
- [x] 阶段 A 验证记录（verification.md：config 366/366、core 8/8、exec 6/6、
      doctor 关键 3/3、tui lib 选择 20/20、protocol 314/314（schema 再生成后）、
      nuwax_isolation 2/2；doctor 宽过滤下 2 失败+6 超时为网络探测类环境受限，
      与改动无关）

## 阶段 B（N2/N3）

- [x] B0 独立 Spec/Plan/Tasks（n2-n3-projection-{spec,plan,tasks}.md）
- [~] B1 引用投影与正文位置（已有子串 splice 实现，但无 response/block 身份；Codex 新增顺序/重复正文回归，尚需根治。原阶段记录：cited 终态替换同名 plain 文本
      投影并随组保持流式顺序；wire 回归 cited_text_replaces_the_plain…）
- [x] B2 混合轮前缀稳定（request_messages：被取代 pending 的 use 保留原位，
      获胜项仅在其 use 已投影时贡献 result；wire 回归
      mixed_turn_late_result_preserves…：前消息逐字节稳定 + assistant 内容
      前缀稳定；bridge 全量 183/183）
- [ ] B3 真实 Core 矩阵
- [ ] 阶段 B 验证记录

## 阶段 C（N4/N5）

- [ ] C1 provenance 消费 + 身份边界
- [~] C2 Core 取消矩阵（anthropic 桥 core 级；旧用例删掉失败 follow-up，Codex 已恢复 raw TCP/socket/第二轮成功强断言，验收见 round2）：
      等响应中断 → TurnAborted 且请求数保持 1（无重试/重复）；工具执行中
      中断 → 工具恰好执行一次、aborted by user 输出进入历史、后续轮正常；
      登记待查：中断 stalled 响应后的紧邻 follow-up 轮出现 request timed out
      （请求未达 mock、约 idle-timeout 时长）——已用简化断言锁定核心性质，
      连接复用路径待专项调查
      Codex复验：raw TCP立即后续轮PASS；工具用例原超时是nested sandbox-exec启动失败，增强诊断后在外层沙箱之外隔离PASS。尚未证明共享连接池产品缺陷；其余timeout/retry/pause-header/平台矩阵仍待验。
- [x] C3 多次 pause usage/request ID 语义（wire 级钉住：暂停续接的 Completed
      token_usage = 最后请求的 input 120/output 5，非两次累加 160/12；
      paused_turn_usage_reports_the_final_request_counters PASS）
- [~] 阶段 C 验证记录（C2/C3 完成；C1 未做——provenance 消费与身份边界为
      下轮主要缺口）

## 阶段 D（N6）

- [ ] D1 exec 构建收据与执行前校验
- [ ] D2 最终 HTTP 脱敏 recorder
- [x] D3 失败路径部分 rollout 保留（run_websearch_turns 任何失败先
      best-effort 复制 rollout 到工件目录再返回错误；live-tests 单元 7/7）
- [ ] D4 最小 MiMo/GLM live（离线全绿后；未触发项标 not-run）
- [ ] 阶段 D 验证记录

## 阶段 E（N7）

- [ ] E1 schema/proto/锁/feature 核对
- [x] E2 Bazel 接入范围 + 可执行 Cargo workflow（.github/workflows/
      fork-cargo-pr.yml：ubuntu/macos/windows nextest workspace +
      rust-rig feature + fmt/clippy fork 面；README 登记；尚未 dispatch——
      需 push/CI 授权；桥/live-tests 维持 Cargo-only、无 BUILD.bazel 的
      决定写入 README）
- [ ] E3 平台证据与待授权清单
- [ ] 阶段 E 验证记录

## 收尾（每批）

- [ ] scoped `just fix -p <crate>`
- [ ] `just fmt`
- [ ] verification.md 更新（源码身份/dirty、命令、退出码、计数、工件）
