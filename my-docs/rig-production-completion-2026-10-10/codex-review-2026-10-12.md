# Codex 独立复审（2026-10-12）

## 范围与结论

受审 HEAD `bfee9dd6a`，从前次 checkpoint `d92053516` 起共 18 个提交、49 个文件、+4107/-1274，包含 cap/usage、proxy 验证、配置与 CI。不是只检查最后两个 V8/bwrap 提交。初始工作树干净；修复前 fetch 后 `origin/test...HEAD` 为 `0 24`，没有覆盖远端新增提交。

rig 三协议路由保持，V8 pin `150.4.0` 保持；默认 proxy 未迁移。下列确认的局部问题已修。metadata 总界、模型可见恢复及新三平台 CI 仍开放，不能宣称整个 fork 已最终验收。

## 修复清单

位置指受审 HEAD；最终行号会随格式化移动。

| 编号 | 级别 | 源码/证据位置 | 问题与修复 |
|---|---|---|---|
| F1 | P1 | `core/src/session/session.rs:671,1852` | 同 turn 恢复后 sequence 归零导致 host usage key 与持久化记录相撞、替换已花费 token。加入 fresh runtime UUID namespace，保留 turn/sequence，公共 suspend/recover 回归验证两次采样累计。 |
| F2 | P2 | `core/src/session/turn.rs:2714` | 中途模型切换后的 cap usage 用初始设置归因。改用实际 step settings 记账，并保留 TokenCount 发射；mock wire+metrics 检查模型 B。 |
| F3 | P2 | `core/src/session/cap_partial.rs:175–200` | UTF-8 截断后的剩余空位可追加未来 ASCII，破坏原文前缀。首次 fragment 截断冻结该片段，首次 turn 截断关闭整个 capture。 |
| F4 | P2 | `core/src/session/turn.rs:3202` / capture | 同一 item 的 done-only summary 共用 key，后续 section 丢失。引入 section key，防 whole Done 和 delta+summary Done 重复捕获。 |
| F5 | P2 | `core/src/session/turn.rs:2924` | Added 已含初始文本，capture 只保存后来 delta。接入现有 seed 路径，公共 cap 回归检查完整前缀。 |
| F6 | P2 | capture whole renderer / `turn.rs:2613` | feature 默认关闭仍采集；whole Done 先 clone/join/format 全文再裁切。仅启用时构造 capture，借用文本分段追加，关闭后不做全文 render。 |
| F7 | P2 | `config/src/schema.rs` / `core/config.schema.json:797,6876` | Rust 支持 bool 和 `{enabled=true}`，schema 仅声明 bool。补实际生成器 typed feature 分支、深比较回归、重新生成 schema。 |
| F8 | P2 | `rig-bridge/src/proxy_version_parity_tests.rs` | NO_PROXY 的 hit.test 不匹配 env-hit.test；重定向判断在占位替换后永不命中。改为真实可达 localhost 绕过及域名边界阴性，重定向三客户端均验证两跳与请求行，不把连接错误当成功。 |
| F9 | P2 | `.github/workflows/fork-cargo-pr.yml` / setup action | 硬编码 runner 架构可能与 native Cargo target 不一致。action 支持 host，从 rustc -vV 解析；不支持 target/重复信任条目在下载前拒绝，保留两层 checksum。 |
| F10 | P2 | `scripts/bwrap_capability_check.sh` | 普通模式丢弃诊断，使 `-v` 改变分类；没有 timeout，且宽泛失败可误报 userns 拒绝。始终捕获、分类不依赖 verbosity；有界探针、显式二进制、精确 permission/timeout/unknown 分类。fake Linux 测试不证明 Linux 内核能力。 |
| F11 | P2 | `core/tests/suite/rig_output_cap.rs:588,598` | 用未注册 shell 得 exec=0，假称完整 tool Done 也被 cap 禁止。改注册真实 UnifiedExec/Direct，检查执行及输出恰一次、一次 POST、cap 仍失败、诊断不重复执行。未改变 native 调度。 |
| F12 | P2 | results-followup / run6 attribution | 184 个 macOS 样本被扩大为 567 三平台历史身份已绿，正文统计仍415/140/63。收窄覆盖，更新487/79/53，并登记80条 explicit 缺逐行首败定位。 |
| F13 | P3 | `session/mod.rs:4780` / `state/session.rs:84` | 最后 lint 的 question_mark 与 redundant_guard：等价改成 `reported?` 和空字符串 pattern。剩余两条结构告警交后续 typed 参数/装箱边界验证，不用 allow 隐藏。 |

F11 的真实合同：未完成/截断工具不执行；完整 native tool item 在 terminal 之前即可按既有调度执行，诊断自身不执行。若要所有 native 工具等待有效终局，须另做 Spec/并行/取消/延迟兼容方案。旧提示词“所有 cap 工具零执行”不能作为未经证明的现状。

## 验证记录

所有 Rust 测试通过 `just test`，local profile 固有 `RUST_MIN_STACK=8MiB`，retries=0。未增加栈或 deadline。平台为 macOS arm64；下述 focused 结果不能外推 Linux/Windows 或完整 workspace。

已完成：

- `just test -p codex-config --locked --offline --retries 0`：369/369，0 skip。
- `just write-config-schema`：退出0，生成物21增/2删；bool/table 正确映射。
- Python existing V8 consumer 12/12、新 setup action 6/6、bwrap fake 分类12/12。bwrap 初次在 macOS `/bin/true` 不存在导致夹具失败，改临时副本 availability 路径后通过；实际 Linux 参数保持 `/bin/true`。
- `bash -n scripts/bwrap_capability_check.sh`：退出0。workflow YAML 可解析，两个 action target 为 host；结构校验不替代 GitHub action 执行。
- 实际执行 setup action host shell，官方 aarch64-apple-darwin manifest 原字节 SHA 与仓库 trust list 一致，archive/binding 两个资产 checksum 均 OK。临时工件 `/tmp/codex-oct12-v8/rusty_v8/`，不入库。
- `python3 .github/scripts/rusty_v8_bazel.py check-module-bazel`：150.4.0 checksums match。
- `just bazel-lock-update`：退出0，无 lock diff；Bazel 报既有 platforms/rules_cc resolved version 警告，没有更改依赖。它不是 Bazel 全量 build/test 收据。

Core/bridge 首轮编译出现两个新测试类型错误（Option call_id 比较、remove_thread Option 误用 ?），已修。首轮实际288个身份中286 PASS/2 FAIL：模型A暂停阶段合法产生零计数但测试漏算；恢复夹具未等第二请求到达即 suspend，违背自身 mock 的两请求期望。修正夹具后6/6复验，再跑完整 Core 相关选集31/31。保留首败，不把首轮写全绿。

| 本轮最终相关验证 | 实际结果 | 临时收据 |
|---|---|---|
| rig bridge 完整默认 suite | 259/259 PASS；与首次 Core 两个 FAIL 分开统计 | `/tmp/codex-oct12-core-bridge-2.log` |
| Core capture/cap/usage/model-switch/code-mode 选集 | 31/31 PASS；4999未选，不是workspace通过 | `/tmp/codex-oct12-core-final.log` |
| 完整工具 native/rig 双路新增对照 | 2/2 PASS；替换上述选集中的单路测试，两份执行数不相加为独立身份数 | `/tmp/codex-oct12-responses-routes.log` |
| http-client resolved-default/loader 选集 | 15/15 PASS；1 slow，167未选；原deadline未变 | `/tmp/codex-oct12-http.log` |
| sandbox V8 | 6/6 PASS，0 skip | `/tmp/codex-oct12-v8-tests.log` |
| fresh code-mode-host | build退出0，真实Core code-mode两样本已运行 | `/tmp/codex-oct12-host-build.log` |

host SHA256：`b17c99387088b2ffcbfa54fc0e2dd7a7cd13950d5afa5d5d1f072841a9ba3368`。本机目标 `aarch64-apple-darwin`；官方配对工件是真实下载/校验，Windows/Linux仅 synthetic consumer control 测试，未原生编译。本轮没有付费厂商请求、完整 workspace 或手动 dispatch，未将 runtime/mock 判定冒称厂商验收。原始日志含 mock 请求，只留临时路径，不入库。

Rust 仓库实际编译器为1.95.0（59807616e，LLVM22.1.2）；root action解析使用全局1.99.0，但两者host triple相同。本轮首次 scoped `just fix` 退出0，发现4条既有新提交告警，无自动改动；两条等价写法已修。余下 `ToolError` large_enum_variant（240 bytes）与 `record_token_usage` too_many_arguments（8/7）保留为结构整理任务，不能称零告警或把尚未测量的装箱当栈故障修复。最后 lint/fmt 不再运行 tests。

最后 `just fix -p codex-core -p codex-config -p codex-rust-rig-bridge --features codex-core/rust-rig --locked --offline` 退出0（`/tmp/codex-oct12-fix-final.log`），仅上述2条不同告警，lib test重复报告不另计；没有自动源码改动。两个最后等价改写由编译/lint验证，不冒称其后重跑过tests。

最后 `just fmt` 退出0（`/tmp/codex-oct12-fmt.log`），仅改本轮文件格式，`git diff --check` 通过；没有重跑tests。阶段产品/验证提交：`9f72b0a95` schema、`94f3b78e7` usage/prefix、`5811c971d` capture、`516241387` tools对照、`134125bd8` proxy、`d8c06c6bf` V8、`a3ce9ac1d` bwrap。每批分别74/304/340/141/94/254/449增删行，均小于500；文档另批。后续在另一台记录最新HEAD及自动CI run，不猜本文件自己的提交hash。

## 明确未完成与下一步

1. **metadata 和最终事件总界（P2）**：fragment text 16KiB/turn text 64KiB/32片段有硬限；item/call ID、usage response ID、JSON 元数据及序列化额外字节没有对应总界。不能把“文本预算”描述为完整事件最大内存/大小证明。先定义超限身份语义与可观测性，再实现硬限，详见新提示词。
2. **模型可见恢复/D6（P0 验收门）**：partial 只作诊断，token evidence=Unverified；render/framing tokenizer 证明、typed context 与人工门未落地。不注入模型、不关验收。
3. **usage/公共客户端**：Chat 迟到 usage chunk、WS cap fixture、compaction legacy provider-ID key、exec 专用端到端、app-server 完整投影、旧 SDK/UI/readers 仍有缺口。已有 crash/双 resume/materialization 的存在不等于全矩阵闭环。
4. **CI 与平台**：567 分类身份的最新三平台逐行结果、bwrap runner、guardian Linux/Windows 栈问题、79 assertion-open/14 no-evidence/53 deadline、Windows311 PASS+LEAK、voice-host 供给仍开放。新 push 自动触发 CI；手动 dispatch 未执行。
5. **代理默认迁移**：ResolvedDefault opt-in 的对照测试不验收仍使用 ReqwestDefault 的默认 cold/并发性能；默认迁移、PAC/runloop因果仍须具体证据与裁决。
6. 原 T21–T40 的 owner/队列、双进程隔离、字段/来源旋转、旧历史、原生/异OS、付费 live、安装升级与发布准备继续按 Spec/Plan/Tasks；不得把历史日志、分类或本机选集替代原 DoD。

代码规模审查：历史 `78acb6df4` +912/-336（生产531行）超复杂500/普通800指导，不改历史。本轮修复分独立 schema、usage、capture、native测试、proxy测试、V8、bwrap、docs 小批保存；最终 split 见 git log。

跨电脑直接使用 `claude-followup-2026-10-12.md`；它包含仓库目的、更新分支、排序任务、命令、验收与权限边界。
