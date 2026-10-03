# Provider 请求控制执行记录

- [x] A1 HTTP/WS 字段、Core builder/reuse 与构造器迁移
- [x] A2 Rig cap/预算与 Responses 环境变量开放
- [x] A3 HTTP/WS/raw tools、Core 与真实 Exec env 回归
- [x] B1 最终 request 的有界 HTTP retry、分类、Retry-After 与取消
- [x] B2 三协议尝试数/原始字节/状态配置/流断开回归
- [x] V1 相关模块验证，记录 selected/executed/pass/fail/skip
- [x] V2 scoped lint、fmt、diff-check 与新增依赖 lock 同步

起始 HEAD：20898140f2b3638aac6ab328b24d2f604871bfc8；工作树包含之前两轮开发，所有验证针对当前树重新执行。

## 第一批验证

- `cargo check -p codex-rust-rig-bridge -p codex-api --tests --offline`：exit 0，4m33s。
- `just bazel-lock-update`：exit 0，MODULE.bazel.lock 无需内容更新；Cargo.lock 本轮增加既有 `codex-client` 依赖边。
- API HTTP/WS、utils-cli NUWAX 与 Rig 全套：268 selected / 268 executed / 268 PASS，0 FAIL，231 为过滤未执行；测试时间 577.553s，exit 0。
- 日志 `/tmp/codex-oct03-controls-api-wire-tests.log`；新增 6 个 wire retry 场景和 local/build 错误分类均执行；三协议 cap 与 raw tools 字节回归通过。
- Core/Exec、新二进制和真实厂商验证正在执行，不能使用前轮数字替代。

## 真实厂商 cap 验证

- 当前构建副本 SHA256 `c97351398c5238cf03a72c9fae882a7a1fdca4c13b3a26919b2195c985df2af6`，505326168 bytes；当前树 `cargo build -p codex-core -p codex-exec --bin codex-exec --offline --locked` exit 0，10m12s。
- MiMo Responses：exit 0，1 HTTP attempt，29.88s；GLM Responses：exit 0，1 HTTP attempt，10.78s。
- 两者均接收并返回 `PROVIDER_CAP_LIVE_OK`；所有捕获请求的 cap=4096、path=/responses，凭据未进入 stdout/stderr/capture。
- 记录 `/tmp/codex-oct03-controls-live-cap-result.json`；凭据仅取 gitignored `.env.local`，原始材料未写入仓库。
- 这是字段接收与正常响应验证，不是实际耗尽 cap 或完整厂商能力/费用验收；local incomplete 回归单独覆盖输出耗尽语义。

## 验证中发现并修复的边界

- `response.incomplete(reason=max_output_tokens)` 原为 generic Stream error，Core retry_delay 会允许重复同一 cap 的采样。
- 共享 Responses decoder 将这个终止原因映射为既有的不可重试 InvalidRequest/budget 配置错误，并提示增加 cap；content_filter、interrupted、未知 reason 语义保留。
- 新增 decoder→Core retry classification 回归和 Core HTTP 计数回归，配置 HTTP/stream retries 均为3仍须仅请求1次。正在补验，第一批268不覆盖这个后续修复。

## Core/Exec 与 cap 终止边界

- 第一轮 Core/Exec：22/22 PASS，119.220s，exit 0；不覆盖之后加入的 cap 耗尽修复。
- 修复后 API decoder/api_bridge/HTTP/WS、Core、Exec 和相关 retry/cap wire：103 selected / 103 executed / 103 PASS，0 FAIL，5499 为过滤未执行；198.339s，exit 0。
- `responses_bridge_output_cap_exhaustion_does_not_resample` 实际执行：HTTP/stream retries=3、重复 incomplete mock，仍仅1次 HTTP；decoder→Core retry_delay=None。
- 日志 `/tmp/codex-oct03-controls-final-boundary-tests.log`；使用仓库 just test、8 MiB stack、隔离 CARGO_TARGET_DIR、--retries 0。
- TUI 单项快照与受类型迁移影响的消费者离线 library 测试、最后 scoped lint/fmt 正在收尾。

## 消费者与快照

- Guardian v2 / genai / live-tests（仅离线 library）/ TUI 单项快照：151 selected/executed，148 PASS + 3 FAIL，5619过滤未执行，716.585s。
- 初始失败：缺 codex-code-mode-host；一个内部 deadline；新快照尚无基线。没有修改 Guardian 生产逻辑或放宽断言。
- helper 默认构建先受 Python CA 缺失阻断；上游默认 archive URL 实际404。按现有 V8 README，用 Codex 150.4.0 的 archive/binding pair，校验仓库锁定 SHA256 后构建，exit 0，2m38s；无 V8 pin/manifest 改动。
- 已审查并接受唯一新增快照；相同消费者 feature graph 串行复验上述3项，3/3 PASS，29.278s，exit 0。
- 日志 `/tmp/codex-oct03-controls-consumers-tests.log`、`/tmp/codex-oct03-controls-helper-build-verified.log`、`/tmp/codex-oct03-controls-consumers-rerun.log`。
- 这是148初轮通过+3隔离复验通过，不能表述成一轮151全绿；完整 workspace、Docker/Windows/Linux矩阵和远程CI未运行。

## 后续范围（未在本轮宣称完成）

- 临时 NUWAX provider 的 request/stream retries、idle timeout、自定义 headers 等直接 env 开关。
- 共用 home 的最近会话按完整来源身份过滤、跨进程 config 更新锁、跨 PID namespace 的 daemon 所有权。
- 本机 code-mode-host 新构建须使用已校验的现有 Codex V8 pair；默认 upstream sandbox URL 404 和 Python CA 路径属于已记录的开发环境问题，未改变 V8 pin 或全局机器配置。
- cap 的真实厂商验证仅验证字段接收与正常回执；没有做付费截断压力测试、全部模型/功能矩阵、容器或远程 CI 验收。

## 独立复查追加的两项修复

- Native SSE 曾暂存 budget InvalidRequest，pending/断网/later completed 可覆盖它。该不可恢复错误现立即终止；新增三种尾部状态回归。
- reqwest 0.13 默认 HTTP/2 protocol-nack retries 可绕过 provider次数和capture。pool/customCA共用禁止底层retry的builder；新增真实 H2 REFUSED_STREAM 次数/捕获回归。
- 前一轮 scoped fix/fmt exit0（21m50s）只对应追加修复之前；将在新测试后重新收尾。

## 独立复查补强验证

- API/rig --tests check exit0，2m01s；新dev h2依赖已有版本，Cargo.lock增加依赖边，Bazel lock-update exit0。
- Native SSE三种尾部状态、HTTP/2 REFUSED_STREAM及相关decoder/HTTP/cap/retry：104 selected / executed / PASS，0FAIL，346过滤未执行；170.526s，exit0。
- H2 negative control实际观察默认reqwest wire3/capture1；生产never策略三组为wire/capture=1/1、1/1、2/2，说明零重试和transport开关已管住实际发送。
- Native预算错误保持连接打开/后续断网/后续completed均立即返回同一个InvalidRequest并关闭事件通道。
- 日志 `/tmp/codex-oct03-controls-monotonic-tests.log`；之前成功的Core/Exec+消费者与此处补强分开登记。

## 最终收尾

- 本轮主范围 scoped `just fix`（API/rig/utils-cli/core/exec/genai/guardian/live-tests/tui）exit0，21m50s；fmt exit0。
- 最后追加修复的 API/rig 再次 scoped fix exit0，4m26s，无 warning/error/自动修复；fmt exit0。
- 与最后 lint 前快照相比仅2文件格式变化；无语义修改，`git diff --check` exit0。按AGENTS，最后fmt后未重跑测试。
- `h2`仅dev依赖，codex-client为已有workspace运行依赖；Cargo lock边已同步，第二次Bazel lock-update exit0，MODULE.bazel.lock无内容漂移。
- 测试分批且有覆盖重叠，勿把268、22、103、151、104求和作为唯一用例数；各批证据如上。
- 修改仍在工作树，未commit/push。真实厂商验证对应追加native/H2修复前的固定构建副本；这两处之后仅本地边界回归验证，未重复付费调用。
