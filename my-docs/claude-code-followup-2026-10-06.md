# Claude Code 后续开发任务（2026-10-06）

仓库 `/Volumes/soddygo/git-workspace/fork-nuwax-codex`，分支 `test`。从实际最新 HEAD 和工作树开始；本轮 Codex 已分阶段保存审查修复，准确提交及测试结果见 `other-computer-validation-results.md` 的独立复审节。不要退回 `7b891587`、覆盖其他人的工作或清理 SQLite/tmp/.env.local。

先读根 AGENTS.md、独立复审结果，以及 `rig-stability-followup-2026-10-04/{spec,plan,tasks}.md`。历史执行数和 live 收据仅作线索，不能代替修改后的验收。以下按优先级分批推进，每批保持可独立审查，复杂逻辑尽量少于 500 changed lines，任何普通批次少于 800。

## 1. P1：收敛 workspace 的确定性失败

历史全套为 21,561 run / 21,178 pass / 270 fail / 113 timeout，不是全绿。不能用“新增测试通过”证明其余 383 例与 fork 无关。来源身份的 stable precomputed schema 漂移已由 Codex 修复，先确认该修复仍在，不重复恢复旧产物。

1. 从 `/tmp/codex-oct05-newpc-workspace.log` 提取最终失败及超时清单，按 package/test/feature/平台建表；日志不可用时明确登记，不猜完整命令。
2. 优先单独复现 exec-server `registration_retry`、network-proxy MITM/HTTP/SOCKS、guardian_v2、skills/TUI snapshots 等确定性候选。使用 `just test`、`--retries 0`，隔离 target、home 和 SQLite，保留首轮失败。
3. 对每一类给出准确根因：fork 代码缺陷、上游缺陷、fixture/构建图、平台限制、环境或尚未归因。基线对照应保持同机、同依赖和 feature 图；不要把单包默认 feature 图与 workspace 联合图直接比较。V8-poc 的 sandbox feature 统一差异单独处理，保持 pin 和配对工件校验。
4. 明确局部缺陷直接修复并补公共路径回归。快照逐个读取 diff；先确认归一化或渲染语义，不自动接受整批 `.snap.new`。保留已有被跟踪的 `.snap.new`，不要用清理命令处理。
5. 小批通过后才申请完整 workspace 复验。获授权时显式排除 `exec_live`/`bridge_live`、禁用自动重试并登记 feature 图；不扩大 deadline、不跳过或削弱失败断言。

验收：每类至少一项可重复的负控/复现及修后回归；未定性清单继续开放。完整 workspace 和跨平台 CI 通过以前，不称生产门禁已完成。

## 2. P1：让 Bazel 的桥路径进入长期回归门禁

本轮发现 build 成功并不等于桥可用：Core BUILD 未启用 `rust-rig`/`rust-genai` 时，第三方请求在发网前报缺 bridge。当前已补 feature、两个桥 BUILD、exec build-script source 和 reqwest_rig alias。

- 保留整个接线，不只保留两个新 BUILD。独立构建 CLI/exec，再使用实际 Bazel 二进制测试 Chat `/v1/chat/completions`、Anthropic `/v1/messages`、Responses `/v1/responses`。
- 复用 `exec/tests/suite/nuwax_env.rs` 的 HTTP/SSE fixture 与公共启动契约；断言 model、auth、cap、单 POST、最终 Completed、config 不变。不得仅用 Cargo 产物证明 Bazel 可用。
- 将这组真实执行纳入现有 fork 本地/CI 门禁，报告实际选中和执行数，0 匹配失败。修改 CI 文件可以先提交 review diff；不要自动 dispatch。
- 当前 Cargo 元数据仅有 reqwest_rig 这一个 registry rename。以后若加入 build/dev/platform 条件 rename，要按相应依赖种类和平台验证 alias，避免 phantom edges；不要顺手改整个依赖系统。

验收：Bazel public path 三协议非零执行，且故意关闭 Core bridge feature 的负控会在模型请求之前失败。

## 3. P1：补 owner/行政启动及跨进程 opaque 矩阵

macOS owner 测试覆盖异环境客户端、运行中排队、冷持久化和第二 writer 守卫。它们是请求/持久化层证据，不是进程级 writer 计数。npm_nuwax × 无 daemon 单元格不能等同完整行政矩阵；Queue 仍优先使用现有 owner，不受行政命令的 install-method 排除规则控制。

- 补 install-method × daemon 存在性 × queue/archive/unarchive/delete 的可观察公共 CLI 差异，包含 active/inactive/corrupted group、显式 provider 屏蔽、名字歧义/分页、显式 remote 的客户端/服务端归属。`daemon_startup` 命令矩阵补 archive 等实际遗漏。
- 有 owner 时验证 writer/队列归属，不启动第二 writer。按可观察的进程或锁证据记录 writer 数；不能仅凭两个请求同凭据就宣称进程数唯一。
- 用两个隔离进程和真实 HTTP mock 恢复同一份复制的旧 rollout，轮换 endpoint、query（未知名/重复/空值）、headers 和真实凭据 snapshot。断言 visible 内容保留，不能证明来源相等的 encrypted/signed/hosted 块降级，旧文件字节不改写，秘密不进入来源元数据。
- loaded/订阅测试使用 unsubscribe ACK 和 loaded-list 断言确认状态；当前契约为无订阅 idle cache 可先 Shutdown 后重建，有订阅或 running 保留 owner。不要再次把该既有分支标成未知产品缺陷。

验收：精确 RPC/wire/attempt/队列持久化断言，配置不污染；Linux 实测与 macOS 分开，Windows Unix-socket 不可执行项明确列出。

## 4. P1：cap partial output / usage / Done 的产品契约

Core 三协议 cap 终止已有 mock 覆盖：不重采样、不执行截断工具，不合成成功 Completed。本轮加强的 live cap harness 严格检查 JSONL、捕获完整性、单次 attempt，并保留 accepted failure 的 rollout；它不等于 cap usage/Done 产品能力已经补齐。

先交付一份独立 Spec/Plan，明确失败时哪些 partial text/reasoning/tool items 和 usage 要在 UI、exec JSON、rollout、app-server 中保留，以及失败 terminal 的身份和顺序。评估兼容、持久化和协议 schema，再小批实现和补公共 Core/exec/RPC 回归。不得为了保留统计伪造成功 Completed，也不得执行截断工具。

Responses live 触顶仍缺验收。先完成 mock；真实厂商请求需当次明确授权和本机凭据，并绑定新建 binary/source receipt，限制场景和费用。旧 Chat/Anthropic 历史 live 不能冒充当前提交验证。

## 5. P2：平台取消、冷启动 profiling 和工具链

- Windows 原生控制台 Ctrl-C fixture：覆盖 Retry-After、首 SSE 字节等待、流中等待；应用处理后的退出、连接关闭和直至进程退出的零后续 attempt。不得把 Unix kill test 当 Windows 通过。
- Linux 执行多进程 env/retry/idle/owner 路径；两个 home、SQLite、model、凭据、协议和预算独立，检查运行后的 config 原字节。
- D4 分开记录 spawn-to-ready、initialize、resume/turn 等阶段及负载；已有低载通过并未定位高载失败根因，不能只靠 dyld 栈推断签名成本。
- 当前锁定 Python codegen 依赖在 Python 3.14 下失败；本轮以隔离 Python 3.13 完成生成，锁文件未改。后续定义 codegen 的可支持运行时并验证该脚本，避免系统默认 Python 意外选 3.14；不要为求绿扩大产品 Python 限制或无理由升级依赖。

## 6. P0 人工复审仍开放：D6 token-aware pause 预算

仅设计，尚未实施。先按修订后的 D6 Spec/Plan 完成 provider framing/计量单位和可信 tokenizer/版本清单。只有完整实际载荷的 exact count 或证明上界可以返回 verified；经验 bytes/4、均值/分位数不构成硬限制证明。

当前 40,960-byte fail-fast 和完整签名/搜索块保持。方案是 KeepWholeOrFail，缺证明时 LegacyBytes/unverified。启用 token 硬限会拒绝部分此前可发送内容，必须先明确产品兼容裁决；不引入未经裁决的 DropOpaque/DropAll、不部分截断签名、不改写旧历史。

验收：ASCII/CJK/高熵/JSON/签名/引用/framing 全载荷边界；超限零下一 POST；未知模型/缺证明不能 verified。token-aware 的设计文档、cap 触顶测试和普通字节 fixture 均不能单独关闭 P0。

## 每批交付规则

记录基线及 dirty 文件；测试完整命令、package/feature/target、selected/executed/pass/fail/skip/timeout；首轮失败与复验；source/binary 身份及脱敏 wire 事实。相关测试完成后 scoped `just fix`、`just fmt`、`git diff --check`，之后不重跑测试。改依赖、ConfigToml 或 API 时同步锁/schema/Bazel。

先交付可 review 的 diff、剩余项和下一批范围。当前这份交接不自动授权新的 commit/push、完整 workspace、厂商调用、CI dispatch 或发布；依据下一轮用户授权执行。
