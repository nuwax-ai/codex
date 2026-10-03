# 2026-10-03 直接开发实现计划

对应规范：`direct-development-2026-10-03-spec.md`。

## 实现分工

- Config：旧六参 loader wrapper、新具名 env-seed API、NUWAX 子进程局部隔离。
- Bridge identity：capture、envelope 验证/预算、request side owner ranges、transport splice，各自私有模块；ID 持久化在现有 ResponseItem.id，不改变 wire API shape。
- Generic provenance：history metadata → Prompt item-ID provenance → 实际 client setup 投影 → 当次 stream 来源快照；不可用 authorization identity 时 fail closed opaque replay。
- Receipt：exec/live harness 共享源内容摘要算法；严格版本化解析；有界 subprocess；PreparedExec 绑定 canonical path 和文件摘要。dev 的 sha2 backend 单包 opt-level 3；保持 SHA-256/全部字节校验，不换弱 hash、不缓存后置检查。
- Capture：独立 request_capture 模块，append attempts，最终 body rewrite 后记录；pause chainer clone recorders；Responses 保持 typed clone 序列化。
- Budget：独立 wire_budget 模块，最终 transport 检查，context 类型错误供 Core compact 识别；硬字节超限为 InvalidRequest，避免无意义 compact 循环。
- Harness：runner/scenarios/rollouts 私有模块，真实可注入 runner 驱动实际 scene，不只测试错误 helper。

## 验证顺序

1. 所有源码编辑稳定后一次性构建定向测试；不在多个 worker 写文件时运行 formatter 或 rustfix。
2. Bridge unit + wire、config 兼容、history provenance、live harness/receipt unit、Core hosted/source、AppServer isolation/remote control、Exec receipt。
3. 重建当前 source-bound exec，最少 MiMo/GLM 三协议 marker 与 Anthropic hosted two-turn；保存 capture、receipt、status 和原始错误。断言 bytes/历史内容，分清未执行能力。
4. Cargo 依赖变动刷新 Cargo.lock 和 Bazel lock；最后 scoped `just fix`、`just fmt`、diff 检查。若 rustfix 改生产逻辑，先人工处理再安排需验证变更；最终 fix/fmt 后不再跑测试。

本次未授权 push、远程 workflow 或发布。用户的直接开发指令不自动等于提交本轮新增工作。

## 真实验证发现的补修

- GLM 搜索 family predicate 与现有生产桥同步；保留完整 input/result shape 和严格配对回归。
- runner 的 pipes/exit 落盘提前到 posthash 前，哈希失败追加 exit 诊断；错误优先级保持 process/capture/identity → persistence。
- 双轮 compact/search 外层 deadline 720s，单轮保持 360s；内层每 turn 300s、每 pipe drain 5s。
- query 凭据识别补 `_signature`，Core 静态匿名头 + signed query 也必须是 credentialInstance；query 轮换只改变私有凭据身份，endpoint digest 不包含签名值。
