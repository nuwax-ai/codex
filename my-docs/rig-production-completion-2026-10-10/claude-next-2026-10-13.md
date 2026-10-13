# Claude Code 下一轮：收敛到 Linux Codex/Rig 服务可用

主仓库 `/Volumes/soddy/git_workspace/fork-nuwax-codex`，`test` 分支。用户目标是环境变量配置国内或自定义第三方模型，通过 Rig 可靠使用 fork Codex；首个稳定目标 Linux/容器 app-server，实际客户端 `fork-codex-acp-ts` 的 dev。中心保持 Codex，ACP 不混入本轮重构。先读 AGENTS、`codex-review-2026-10-13.md`、当前 Tasks/Spec 与最新 results；以远端 test 最新 checkpoint 为起点，尊重并行改动，不 reset/clean/强推或复用旧电脑 target 冒称新二进制。

本轮独立复审已经修复：二进制/读取失败的凭据扫描，旧账目恢复的 identity 克隆，receipt 转义字节预算，Bazel 无效环境表达式，工具 call/result 与 usage 假阳性，测试 idle 污染，marker 终局/usage 合同，typed live 的默认重试。不要重复这些工作。Bazel 缓存覆盖用既有 `user.bazelrc` 显式路径；`%{BAZEL_CACHE_ROOT:-~/.cache}` 不会自动展开。

## 第一批：新 SHA CI 与最小阻断修复

1. 核对这次 push 的 exact HEAD/run/jobs/attempt，保存首次 FAIL/TIMEOUT/ABORT/LEAK 和完整 `(platform,binary,test)` 身份。旧 run38286839054 四 job 仍失败；只能验收指定修复，不能称全绿。未取日志的轮转失败不能统称 flaky；最小复现后才判重试是否合理。
2. **Bazel bootstrap JoinPathsError 新 CI 关闭**：Codex 已确认错误 rc 导致物理 sysroot 路径含冒号，Rust join_paths 拒绝；修正配置后独立无缓存 macOS bootstrap 实编通过。先验证新 Linux run 的完整 gate。若仍失败，抓 process_wrapper 编译环境/命令和真实 sysroot/native lib/canonical cache 路径，区分旧 repository 链接残留、路径和工具链机制。旧 rc+旧 symlink 的复现不能排除缓存改动；不要重做已完成机制或先改 pin。冷 action 必须实际执行，不用暖缓存 PASS 关项，不禁 TLS、不下载未校验工件。
3. **Guardian SIGABRT**：以原 test/feature 在 Linux 明确4MiB线程复现，区分 future 构造/移动/尺寸/poll/递归路径，做最小有因果证据的栈占用修复。不得增栈、放宽 deadline 或 skip；装箱 ToolError 不是已证明栈修复。修后单跑/模块并发/正常 Core 公共路径对照。
4. **Linux shell_snapshot 与 skills**：按失败身份实际执行沙箱/remote snapshot 模块，核对 env/cwd/用户、引号、shell版本、Git项目标记和fixture隔离。不要把 userns 新解锁后暴露的失败当作默认可接受。Windows 自有 NUWAX 工具/跨进程恢复挂起单独保留，需要原生事件/进程证据，不用 Unix 或 timeout 放宽替代。

一批只处理一个可复现机制，复杂变更 <500、普通 <800 changed lines；不要一口气重写全部4000行历史。可先完成最小独立阶段并本地提交，再继续其它机制。

## 第二批：Linux 实际部署路径

1. 选一个明确 Linux 镜像/架构，构建当前源对应的 CLI/app-server 与测试 helper；记录源 SHA/dirty/content、features、triple、binary hash/receipt。用户实际 ACP 路径通过 CODEX_PATH 固定候选，不能用安装的旧 nuwax-codex 包代验。
2. 安装匹配 bwrap，分别校准宿主 AppArmor userns 与容器 seccomp；在预期实际部署策略下执行 sandbox-enabled 公共工具链路。过去 danger-full-access 容器 mock PASS 不能代验沙箱。只为校准暂时解除限制的环境单列，不宣称其就是最终安全运行配置。
3. 三 wire 离线先走真实 app-server JSON-RPC start/turn/一次无害工具/取消/热及进程重启冷 resume/compaction/usage；真实 URL/auth/model/cap 与 call ID 配对均断言。两个进程拥有独立 provider/key/model/wire/retry/idle、CODEX_HOME/CODEX_SQLITE_HOME，实际请求和SQLite互不污染。取消返回后零新 POST，终局恰一次，失败不是 Completed。
4. 保存失败和首败，不批量接受快照，不把现有 baseline 同败当作稳定验收。复合 DoD 未全满足不勾 Tasks。

## 第三批：live 请求预算和直接 HTTPS 深链路

1. 给现有 typed live harness 加**绝对实际外发请求预算**（每场景/总批次），须覆盖 providerHTTP retry、Core stream retry、工具sampling与Anthropic pause continuation。`retry=0` 和捕获64次上限都不是硬请求预算。预算耗尽明确失败、零下一 POST，不能合成 Completed。配置 typed/self-documenting，复用现有 transport/request capture，不新造 provider 路由。
2. 离线以可计数本地服务验证预算边界和取消 race；credential 扫描继续成功后执行并覆盖 home/cwd/artifacts 的字节内容，读取失败不能视为空，原场景错误保留。每个 marker 验收必须包括真实执行、配对工具输出进后续 request、最终 marker 和实际 usage；不得仅检查字符串任意出现或通知非空。
3. 已有13个基础模型/协议与三个主模型有限工具链路证据，不重复免费率未知的全矩阵。选一个用户实际主用 provider/model/wire，做直接 HTTPS（不经上一轮本地费用闸门）、app-server/ACP 消费、真实取消、两次 resume、压缩和真正触顶。把接受cap字段与真实触顶分开，signed/search/citation/pause 仅对平台声明能力验收。新的付费批次明确 provider/model/请求上限并取得该批授权；密钥只由用户私密环境提供，绝不复制进提示词/工件/Git。

## 先验收什么、哪些后置

首阶段目标：明确 Linux 镜像 + 当前源二进制 + 明确 provider/model/wire，可通过环境启动、工具、取消/恢复/压缩、usage 与双进程隔离稳定使用。仍有失败的路径必须标限制；不泛化成所有厂商、所有平台、所有40项发布验收。

D6可证明tokenizer预算、partial模型可见恢复、全套旧reader/SDK/UI扩展、代理默认迁移及完整发布/异OS矩阵保持独立roadmap。缺证明partial仅诊断，未经P0门不能注入模型；不要让这些扩展阻止正常Rig模型接入，也不能提前关项。

Rust 全部用 `just test --locked --offline --retries 0`；确认实际非零执行和请求断言，paid binaries必须精确选择，不隐式扫 live matrix。完整本地workspace按AGENTS另行确认；已授权的push自动CI可承担全量，不手动dispatch。tests后scoped fix、just fmt、diff-check，最终fix/fmt后不重跑同批测试；实际config/API/dependency/compile_data变动同步schema/TS/Bazel lock。

这次用户授权 Codex commit+push 不视为授予你无限push/发布或pin迁移权限；新会话按用户当时授权执行，任何发布先完成可审制品再审批。最后交付每批提交、精确 source/bin/测试收据、新run状态、已修/当前验证/未验平台/剩余最短任务；结果写新的follow-up，旧事实保留。
