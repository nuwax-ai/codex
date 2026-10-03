# 2026-10-03 直接开发执行记录

## 源码

- [x] 六参 config API 兼容和局部 remote control 禁用。
- [x] v3 响应/片段身份及六类定位/布局回归源码。
- [x] 共享内容指纹收据、严解析、PreparedExec 与验证矩阵源码。
- [x] 完整场景失败保留和可注入 runner 矩阵源码。
- [x] 请求原始字节/多 attempt 记录及容量/I/O/凭据回归源码。
- [x] 最终 wire 硬字节与模型估算预算源码。
- [x] Generic provenance 消费、实际请求来源绑定和 auth-domain 矩阵源码。
- [x] NUWAX cold resume/fork/reload 正向矩阵源码。
- [x] 正常 API Key 的不可变 credential-instance 绑定源码（避免全部 opaque 被 selector 保守清除）。

## 验证（源码完成不等于通过）

- [x] 定向 build/test（记录日志、过滤器、run/pass/fail/skip）。
- [x] fresh exec build + source receipt 验证。
- [x] MiMo/GLM 实际请求断言与失败场景留存。
- [x] lock refresh。
- [x] scoped fix + fmt + diff check。

源码完成与测试通过分别登记；最终证据在此追加。不得用旧报告中的绿灯代替本轮结果。

### 首轮结果（仍在修复验证）

- `/tmp/codex-oct03-local-unit6.log`：744 run / 741 pass / 3 fail / 0 skip；3 个 fixture/预期问题已修，后续 L1 运行 unit 全绿。
- `/tmp/codex-oct03-local-related2.log`：856 run / 852 pass / 4 fail / 0 skip。4 个 wire fixture 含非法 citation schema 或拆分响应未重编号外层 index，已修；strict parser 保留，尚待重跑。
- `/tmp/codex-oct03-core-app-exec.log`：build exit 101，13 个 Core 编译错误（可变 self/import/collect 类型注解）正在修，不是运行时通过。
- 本机限定平台 `cargo metadata --offline --filter-platform aarch64-apple-darwin` 成功；无平台筛选版本缺 WASM 缓存，不代表本机 runtime 失败。
- `just bazel-lock-update` 两次成功（首次沙箱缓存写入权限失败后使用批准的缓存访问）；当前 MODULE.bazel.lock 无差异。API rand 依赖落地后仍需再次刷新。

### 凭据实例实现后

- API/static-model-provider/ Core actual-source 实现已稳定，尚待本轮新增测试通过。缓存持有实际 header/URI/query 的私有副本，仅随机 ID 写入历史；RNG 和锁错误明确返回，不含秘密值。
- account 来源使用实际 selected account headers 与捕获身份匹配，绑定 account/user/可用 workspace-membership；不使用 plan 分类充当 workspace identity。
- 每个 attempt 仅计算一个 source；metadata 与 Rig dispatch 共用它，防止缓存淘汰或并发导致两次注册不一致。
- 本机平台 Cargo metadata（`/tmp/codex-oct03-metadata-credential.json`）exit 0；Core/AppServer/Exec 正在 `/tmp/codex-oct03-core-app-exec2.log` 构建，不得提前记为通过。

### 真实厂商失败后的增补

- [x] GLM `web_search_prime` / full `search_query` input / string result 保留；严格 ID 和非搜索 name 拒绝回归。
- [x] 已完成 pipes/exit 在 posthash 前落盘，identity 仍严格失败；I/O 不覆盖主错误。
- [x] 支撑 lib 第一轮 30/30、exit 0；`/tmp/codex-oct03-live-support-vendor-posthash.log`。
- [x] 只优化 sha2 dev package、摘要一致性的本机隔离 benchmark；拒绝非 regular binary input。
- [x] 双轮 watchdog 720s、单轮 360s；不修改 300s subprocess timeout。
- [x] query `_signature` 识别与 credentialInstance 轮换回归源码。
- [x] stdout/stderr 各 16 MiB cap + capture failure 提前停止子进程及回归。
- [x] 三协议匿名 auth 归一化，移除缺省凭据时 Rig 合成空 header，保留显式 key / gateway header 及 HTTP 回归。
- [x] 最后源码稳定后的定向复验、fresh build、两项真实请求复验。

- 最后定向整合命令已启动：`/tmp/codex-oct03-final-related-validation.log`。本次包括新增 query-signature、auth HTTP 和 pipe-cap 回归；所有 worker 已冻结源码。命令结束前不得登记为通过。

- 最后整合：1083 run / 1075 PASS / 8 FAIL（内部超时）；同构建图、单线程隔离14/14 PASS，包含全部失败项。完整记录见 targeted-validation。不能把两轮写成一次全绿。
- 源码 API 注释补 credentialInstance 后，fresh exec build 进行中：`/tmp/codex-oct03-final-source-exec-build.log`。

## 本轮完成结论

- 直接开发源码与相关验证完成；保留所有失败轮次和隔离补验，不伪装单次全绿。
- required scoped fix / just fmt / final Exec compile / diff-check 均 exit 0。fix 仅两份测试文件三处等价 lint 修正（method ref × 2、json 宏内冗余 clone × 1），其余 70 Rust 文件字节差异为格式化。pause 递归显式 `impl Future + Send` 保留。
- 最终 source hash：`78b80bbe020225678b7c6c8e4bec81a98741b0a6ddb6974eb147eeeb6b69bd4e`；final binary SHA256 `90b1ed802a3545a3c2a6f5185f29d03a4f708c632cb8bb833927fbdbbf2b0738`。最终编译与 source binding 通过；fix/fmt 后未运行测试/厂商请求。
- 真实 MiMo/GLM 三协议 marker 覆盖与 GLM 双轮新搜索闭环已有证据；最后 GLM live 源码摘要是 `18afe06a…5ba39`，不能称为 final formatted SHA 实测。
- citation 能力在最后真实 GLM 场景为 0/0 未验；严格签名/密文/引用保真来自 mock/wire/Core 回归。跨进程 credentialInstance 不承诺旧 opaque 继续回放，普通历史保留。
- 未运行完整 workspace、Linux/Windows、远程 CI/发布验收；未 stage/commit/push。当前是可审查的本地工作树交付，HEAD 仍为 `20898140f`。
- 执行命令、run IDs、counts、原失败与补验见 `direct-development-2026-10-03-targeted-validation.md`。
