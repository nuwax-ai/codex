# Codex 独立复审：Claude 二十个提交

日期 2026-10-13，分支 `test`。起点 `abc9b0071`，工作树干净；审查范围 `8aefb5a6f..abc9b0071`，20 提交，32 文件，+2639/-420（3059 changed lines，另有二进制 schema）。fetch 后 origin/test 为 `b497795f2`，本地领先两个提交。用户本轮明确授权修复、commit、push；没有授权发布制品、变更 V8 pin 或默认代理。

## 发现及修复

以下旧行号按审查起点，修复后的代码行号可能移动。每项均独立核对源码或复现，不把 Claude 自查结论作为验证证据。

1. **P2，非 UTF-8/读取失败被算作“干净”。** `live-tests/src/binary_turns.rs:559` 的 read_to_string(...).unwrap_or_default() 会跳过 SQLite 等二进制文件及不可读工件。新增两条回归在原源码上均 FAIL：三个扫描根目录里的非 UTF-8 密钥文件、不能读取的条目。修复为分块字节扫描并保留跨块重叠，读取错误失败关闭；非普通文件不沿链接逃出扫描场景。新增 clean binary 和 Interrupted/PermissionDenied 回归。最后独立复核还确认带引号/反斜杠的自定义 key 以 JSON 转义持久化时漏检（新回归首败）；已把 JSON 字符串内转义形式也纳入字节匹配。成功后才扫描、场景原始错误优先的行为保留。
2. **P2，恢复记录仍会复制超长身份。** `core/src/state/session.rs:107` 仅限制 join key；合法 host key 配 1 MiB provider ID 仍在 `session/mod.rs:1843,1864` 被整对象 clone 到 latest/ledger。新增 state/token_usage_record.rs，复制前规范化：有效 host 保留、超长 provider ID 清空；超长 host 及其 advisory provider ID 都清空，避免下一检查点恢复时意外退回 provider join。计数与所有权保持，原 rollout 不重写。状态测试加公共两次 resume/compaction 回归，覆盖三类旧记录、累计 30+40=70、原历史字节前缀。
3. **P2，32 条路径不是收据字节上限。** `exec/src/build_source_identity.rs:160` 的树状态可能因转义超过 32 KiB receipt 读取预算；真实临时 Git 长引用路径反例得到 tree_status 约 51 KiB。现在最多 32 条、每条最多 192 escaped bytes、状态包络 <8 KiB；缩短/遗漏计数明确，UTF-8/JSON 转义不切断。新增完整 receipt <32 KiB、长 Git 路径和短记录兼容测试。首次长路径 fixture 超过 macOS PATH_MAX，改为四层、更多引用字符后仍复现原转义超限，保持平台路径合法。
4. **P1，Bazel 缓存变量没有展开并制造冒号 sysroot。** `.bazelrc:8` 的 `%{BAZEL_CACHE_ROOT:-~/.cache}` 被 Bazel 9 原样接收。本机 `bazel --batch info output_base repository_cache` 实际输出 `/repo/%{BAZEL_CACHE_ROOT:-~/.cache}/bazel-repo-cache`。现有 Rust repository 链接及其 `--print sysroot` 同样指向这个含冒号的物理目录。Rust 1.95 在构造链接器 PATH 时会 unwrap env::join_paths，Unix 不允许单段路径含冒号，正好对应 JoinPathsError；因此用旧 rc 但沿用旧 repository 链接的复现不能排除缓存改动因果。恢复合法 `~/.cache` 默认，使用仓库已有且 gitignored 的 user.bazelrc 显式覆盖；修复后路径正确，无冒号新 output root 的禁缓存 bootstrap 实编通过。未改 Rust/V8 pin、未清理用户缓存，新 Linux CI 因果关闭仍待新 run。实现依据：[Rust linker](https://github.com/rust-lang/rust/blob/59807616e1fa2540724bfbac14d7976d7e4a3860/compiler/rustc_codegen_ssa/src/back/linker.rs) 与 [sysroot selection](https://github.com/rust-lang/rust/blob/59807616e1fa2540724bfbac14d7976d7e4a3860/compiler/rustc_session/src/config.rs)。
5. **P2，工具历史断言允许假阳性。** `live-tests/tests/nuwax_env_offline.rs:203` 用历史任意位置 marker+任意 tool role；marker 本来就在 user prompt。`app-server/tests/suite/v2/nuwax_positive_wires.rs:443` 类似允许空/错 call_id 输出。现要求 call ID 精确匹配，marker 必须在对应 tool content/function_call_output 内；Chat 也补 tool_call_id。
6. **P2，usage 通知存在不证明账目正确。** 同文件 `:419` 只看非空通知集合。现比较正确 thread/turn IDs、整个 last=4/2/6 与 total=8/4/12 的对象及 cache/reasoning 零值；真实两次 wire fixture 断言累计不会丢失或重复。
7. **P2，父进程 idle 变量污染新测试。** 同文件 `:179` 的 child reset 漏 NUWAX_STREAM_IDLE_TIMEOUT_MS，继承坏值或极短时限会破坏受控 fixture。已补完整启动组隔离，不改变产品环境优先级。
8. **P2，marker 场景只要求最终答复非空。** `live-tests/src/binary_turns/scenarios.rs:187` 会把无 marker 的回答计入成功，且不验 turn/usage。现要求执行输出 marker、最终 marker、恰一次完成/无失败、非零 input/output usage；更新真实离线响应及模拟 CLI JSONL，新增错误注入。首轮另外两个旧假二进制 fixture 因新合同失败，已更新为合法多事件终局数据。
9. **P2，typed live helper 仍采用默认重试。** `live-tests/src/binary_turns.rs:394` 三个控制为 None，而 runner 清除父进程 NUWAX 控制。现明确 request/stream retry=0、idle=45s。这仅约束重试，不是 Anthropic pause continuation 或工具多次 sampling 的绝对请求预算；绝对预算闸门仍列为后续任务。不会将捕获的 64-attempt 审计上限冒称费用预算。

未发现新的模型上下文注入或 schema/Windows `.exe` 兼容回归；没有恢复诊断 partial 的模型注入。Startup 导出与 extra overrides 的当前调用点不构成本轮产品缺陷。

## 验证与边界

macOS arm64；网络禁用/沙箱标记均不存在，mock 服务及实际子进程回归真实执行。没有付费模型请求、完整本地 workspace 或平台模拟冒充原生验证。

- 凭据回归首败：2/2 FAIL，随后有界扫描回归 4/4 PASS。
- `cargo build -p codex-exec --bin codex-exec --locked --offline`：成功；源码更新后重建保证 offline receipt 一致。
- `just test -p codex-core -p codex-app-server --features codex-core/rust-rig --locked --offline --retries 0 -E 'test(state::session) + test(suite::token_usage_rollout) + test(suite::resume) + test(suite::compact) + test(nuwax) + test(suite::v2::projects)'`：141/141 PASS。含三个新增旧身份公共恢复场景、NUWAX 三 wire/工具/取消/恢复和 usage。
- `just test -p codex-live-tests --locked --offline --retries 0 -E 'not binary(exec_live) & not binary(bridge_live)'`：首次 50 PASS/3 FAIL（新合法 marker 需要同步两处老 fixture、一个新长路径 fixture 超过 macOS 文件系统上限）；修正后 52 PASS/1 FAIL，收据正确拒绝构建期间修改的 fixture 源；停止源码修改、重建后53/53通过。随后新 JSON key 扫描回归首败并修复，最终 **54/54 PASS，0 skipped**。不把 paid binaries 的 runtime skip 加进离线通过数量。与上一命令合计 **195 个定向测试通过**。
- Bazel 修复前后真实 info 路径如第 4 项。普通新 output root bootstrap 通过后，为排除暖 action cache，再执行 `bazel --batch --output_base=/private/tmp/codex-bazel-cold-review-2026-10-13 build @rules_rust//util/process_wrapper:process_wrapper --disk_cache= --remote_cache= --remote_executor= --noshow_progress`，**成功，1177 actions/580 darwin-sandbox，Critical Path 1.29s**。它证明 macOS 本地 cold bootstrap，不是完整 Bazel bridge gate 或新 Linux CI 全绿。

日志位于本机 `/private/tmp/codex-review-2026-10-13-*.log`，跨电脑可读证据以上述命令/数量/首败为准。最后 tests→scoped fix→just fmt→diff-check；最终 fix/fmt 后不再跑本批测试。完整 workspace 按用户此前选择留新 SHA CI。

最终 scoped fix 无告警、`just fmt` 与 `git diff --check` 通过。测试发生在最终 lint/格式化前；没有修改配置类型、依赖或 API shape，无 schema/锁文件额外更新。Bazel graph 的既有版本解析 warning 未通过改 pin 求绿。

| 本地阶段提交 | 内容 |
|---|---|
| `98d59ede8` | 恢复 usage 记录复制前限身份，公共两次恢复回归 |
| `47a0a733e` | receipt escaped 状态预算与动态长路径回归 |
| `c0666944f` | 二进制/JSON key 工件扫描、失败关闭、typed retry 默认 |
| `c02b88a9c` | call/result、完成、final marker、准确 usage 与环境隔离 |
| `e0dd75647` | 正常 Bazel cache 路径与既有 user.bazelrc 覆盖 |

五个代码阶段均 <500 changed lines，随后文档提交不改运行代码。本轮 push 同时携带原先两个未推提交 `2cfb7da53`、`abc9b0071`；不 force push，不发布。最终远端 SHA 和新 run 以提交后报告为准，不能复用旧 run 验收新修改。

Core/exec/live-tests/app-server 的 scoped fix 已通过无告警；最后新发现 JSON 扫描问题作为独立小批先验证再 scoped fix。lint 等价整理、测试 enum 命名及 opaque literal 注释由编译/lint确认，不冒称最终格式化后重新测试。

## 当前 CI 与后续范围

读取实际 run `38286839054`，head=`b497795f2`：Bazel、Ubuntu、macOS、Windows 四 job **均 failure**。它支持某些指定修复已生效，不能表述为当前 CI 全绿；“与基线同败/轮转”也不能替代根因证据或稳定性验收。

下一阶段的第一目标仍是 Linux/容器 Codex app-server，经 Rig 支持自定义模型；ACP dev 只作为消费入口。13 个模型/协议基础组合及有限真实工具证据已存在，不重复跑完整付费矩阵。

优先安排：新 push SHA 的精确 CI 归因，验证修正后的无冒号 Bazel 冷构建（本机机制已定，不重复盲目归为纯工具链 bug）；Guardian 栈溢出机制与 Linux shell snapshot/skills 公共路径；启用 bwrap 的 Linux 服务工具、取消、重启/两次 resume 与两进程隔离；绝对请求预算与直接 HTTPS 深链路。Windows 保持可编译和协议兼容，已复现自有用例故障不标“已通过”，单独列平台任务。

当前修改按可独立审查的 resume 身份、receipt 字节预算、凭据扫描、工具/usage 验证、Bazel 配置阶段保存，避免将 3059 行范围合成一个逻辑提交；生成 schema/receipt 与 snapshot 另列。复杂批目标 <500，常规批 <800，原历史提交不为尺寸而重写。
