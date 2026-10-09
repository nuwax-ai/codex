# 2026-10-10 批:P1 阻塞项修复(代理绕过 / 证书缓存生命周期 / 跨平台 import)

基线:本机 `/Users/soddy/Documents/git-rust-work/fork-codex`,分支 test,HEAD = origin/test = `877b02758d090ef87f40f5e5c342519f371e41f6`(含必需祖先)。开工时工作区仅 `.gitignore` 一处本地改动(新增 sqlite/tmp 忽略),未动。本批未 commit/push。

依据:`my-docs/codex-pending-review-2026-10-09.md`(优先)与 `my-docs/other-computer-development-prompt-2026-10-09.md`。**本批是 WIP 检查点之上的修复批,不宣称"全部通过"**;旧机日志只作历史证据,本机实际执行见 §4。

## 1. 修复内容

### 1.1 P1 #2/#3/#6:代理/重定向绕过(最小兼容修复)

- `http-client/src/outbound_proxy.rs`:`ReqwestDefault` 策略下恢复无条件 `TransportDefault`——删除 `destination_host_skips_system_proxy` 及其两处调用(async + sync)。字面量 IP/IPv6/localhost/大小写/尾点目的地不再被特判为 `Direct`,因此:
  - env(HTTP(S)_PROXY/ALL_PROXY)、系统代理、builder 显式代理与 NO_PROXY 的完整优先级回到 reqwest 原生语义;
  - `Direct → builder.no_proxy()` 造成的"客户端所有代理被清除且随自动重定向扩散到域名 hop"的绕过路径消失(P2 #6 的 IPv6 方括号解析缺陷随函数删除一并移除)。
- `route_aware_client_pool.rs`:`(ReqwestDefault, LegacyTransportDefault)` 构建臂的注释更新为真实契约(该臂现在只对显式注入的 Direct 生效);逻辑不变。
- WS dialer 的 `Direct` 分支保留(Left 传输/nodelay/调用方 TLS/解析后 loopback-only 边界不动);`connect_loopback_direct` 显式回环路径不动。

**新增回归(红→绿验证)**:
- `reqwest_default_route_preserves_transport_proxy_behavior`(表:IPv4/IPv6/127.0.0.1/localhost/LOCALHOST/尾点/域名 × 同步 `resolve_proxy_route`,断言 TransportDefault 且不触系统解析)。
- `reqwest_default_async_route_preserves_transport_proxy_behavior`(异步路径同表)。
- `transport_default_literal_ip_and_redirect_hops_follow_environment_proxies`(子进程隔离 env:HTTP_PROXY 指向本地代理,请求字面量 IP 起点 → 302 → 域名 hop;断言代理收到两条绝对形请求。此测试在 WIP 代码上红:起点直连闭端口)。
- `transport_default_routes_literal_ip_websockets_through_environment_proxies`(websocket-client 子进程:HTTP_PROXY + ws://127.0.0.1,断言 CONNECT 到代理后握手/回显成功)。
- 回退 WIP 对 `managed_request_timeout_covers_queued_transport_construction` 的断言改写(恢复 TransportDefault 键断言)。

### 1.2 P1 #1:非 macOS 预热 import

- `route_aware_tls_fallback_tests.rs:476` 的裸 `use super::test_warmup::warm_secure_transport_once;` 移到文件顶部并加 `#[cfg(target_os = "macos")]`(定义与调用点本就有 cfg)。aws-auth 的本地副本核对过已正确 cfg。

### 1.3 P1 #4/#5:根证书缓存生命周期(源键 + TTL + 失败不缓存)

`custom_ca.rs` 的进程级 `LazyLock<RootCertStore>`(首次快照永久缓存,撤根/换文件/SSL_CERT_FILE 变化后新连接器仍信任旧根;空/部分失败永久缓存)替换为**源键 + 有界过期缓存**:

- 键:Unix = `SSL_CERT_FILE`/`SSL_CERT_DIR` 值(openssl-probe 实际读取的源);macOS/Windows = 平台身份。键变化立即失效重载。
- TTL:完整加载 60s;**部分失败(有错误也有证书)5s**(对齐系统代理缓存的成功/不可用双 TTL 模式);**零证书加载不缓存**(当前连接器 fail-closed,下一个连接器重试 → keychain/文件故障恢复)。
- 单飞:加载持锁串行;bundle 证书只进私有 clone,绝不进缓存。
- 契约注释如实说明:这限制的是陈旧度并摊薄加载,**不消除**底层同步系统调用(首次每源仍要付)。

**新增测试**(缓存语义 5 个单元 + 3 个真实 rustls 握手,全部持测试锁并恢复全局缓存):
- TTL 内共享/过期重载、Unix 源键切换立即重载、空加载不缓存、部分失败短 TTL、并发单飞(恰一次加载);
- 同进程 A→B:TTL 过期 + 文件替换后,新连接器信 B 拒 A(`UnknownIssuer` 类失败)、旧配置保持旧信任;
- Unix SSL_CERT_FILE 切换不等 TTL 即换信任;
- 自定义 bundle 不污染缓存(有 bundle 的连接器信 X,无 bundle 的不信 X 只信平台根)。

### 1.4 附带:阻塞 offload 与 R7 权衡处置

- `websocket-client/src/dialer.rs`:https 代理隧道的 TLS 配置构建(内含平台根加载)从 Tokio worker 移入 `spawn_blocking`(注释如实:系统调用仍阻塞一个 blocking 线程,本变更只挪位置)。
- **R7 权衡(重要,如实登记)**:恢复 TransportDefault 后,本机 exec-server `registration_retry` 家族在 18 并发下复现 13/18 FAIL(~0.7-1.4s 预算),单跑通过(0.42s)——与 worklist R7 的 SCDynamicStore 辅助线程阻塞机制一致。按评审处方"测试路径显式直连":`exec-server/src/remote.rs` 的 `#[cfg(test)] EnvironmentRegistryClient::new` 改为显式直连 hermetic 夹具(`with_legacy_direct_proxy_and_custom_ca_fallback`,这些测试只对回环 mock 说话,从不验证代理行为)。改后 **18/18 PASS(6.2s)**。
- **产品侧跟进设计已交付**:`my-docs/scdynamicstore-compat-spec-2026-10-10.md`(专职 CFRunLoop 线程 + 进程缓存接管系统代理解析;含双轨对照验收标准与 hyper-util env-先于-system 的逐 scheme 复刻要求)。未实施。

## 2. 本机实际执行(just test,--offline --locked --retries 0)

| 范围 | 结果 | 首败/备注 |
|---|---|---|
| codex-http-client 全量 | **135/135 PASS** (3.2s) | 首轮 76/79(作用域过滤)3 失败 = 新握手测试缺 CryptoProvider 安装 + 两个 CA 同 DN 导致 BadSignature;修复后全绿,复验 18/18(custom_ca 过滤)与全量 |
| codex-websocket-client 全量 | **20/20 PASS** (0.6s) | 含两个新子进程测试 |
| codex-exec-server 全量 | **616/616 PASS** (35.5s,1 slow>30s 仍 PASS) | 恢复 TransportDefault 后 registration_retry 首轮 13/18 FAIL(上述 R7 机制),hermetic 夹具后 18/18,再跑全量全绿 |
| codex-aws-auth 全量 | **10/10 PASS** (6.0s) | real_imds 0.9s(warmup 生效) |
| codex-app-server-transport | **157/157 PASS** | 可读 `/private/tmp/p1-app-server-transport.log` 支持 0 skipped/EXIT=0；source/bin 收据仍待补 |

## 3. 未执行 / 环境阻断(如实)

1. **Linux/Windows 真实编译验证(P1 #1 的收尾)**:本机装有 `x86_64-unknown-linux-musl`,但 `cargo check --tests --target` 被 `openssl-sys`(native-tls 经 reqwest 默认特性)阻断——musl 无 OpenSSL 且 vendored 未启用,启用需要 musl C 交叉工具链(本机无)。cfg 修复与定义处 cfg 逐字对齐,风险低;真实跨平台编译由 CI 三平台覆盖,或需装 musl 工具链后重跑。
2. **冷态 TLS 4 失败的逐项收口**:旧机 `dialer-fix-verify.log`(139/143)不在本机,4 个失败用例名无法本地列举;本轮 http-client 全量在本机窗口全绿(含 `default_pool_does_not_retry_a_native_tls_protocol_failure` 0.83s),不能据此宣称冷态问题关闭。维持 worklist R10 开放;生产冷首请求(独立进程、原预算、无预热)的测量未在本轮执行——需要低载窗口,且预期在本机冷态证据(SSLCreateContext 8.33s)下不满足原预算,登记为调查项而非可过测试。
3. **完整 workspace / Bazel / 远程 CI / live**:按授权边界未申请未执行。
4. 并行调查代理(R1b 剥离点 / R11 栈深 / R9 SQLite / 历史119账目)的分析文档另行交付,见各自文件;本批不含其结论的采纳。

## 5. 并行调查交付(只读分析,修复未实施)

- `validation-2026-10-07/r1b-delivery-strip-point-analysis-2026-10-10.md`:R1b 剥离点定位于 fork 自有的 `core/src/model_output_projection.rs:217-222`(provenance 对 guardian 复核请求必然不匹配→Compaction 整项移除),13 项判定为 fork 回归,含最小修复候选与待做①答案。
- `validation-2026-10-07/r11-future-depth-static-analysis-2026-10-10.md`:"每次重试加深 future"被静态证伪(全部重试循环为替换型);深度大头为单次 poll 固定纵深,含 `WebSocketConnector::new` 同步证书构建;给出装箱候选与逐 attempt SP-delta 实验设计。
- `validation-2026-10-07/r9-sqlite-establish-static-analysis-2026-10-10.md`:遥测不写 SQLite(前提修正);flume 唤醒丢失候选源码级排除;新强候选 C1 = 模型传输在运行时线程同步急切构建 reqwest 客户端 → SCDynamicStore 阻塞(与 R7 同源、不经路由解析故 R7 早退从未覆盖);含三层复现方案。C1 预测"当前树单跑 connection_failures_increment_retry_telemetry 仍 ~10.4s 失败",本机诊断运行见 §6。
- `validation-2026-10-07/historical-119-ledger-recheck-2026-10-10.md`:119 在标注文件层三途径复算吻合;final19 分解获本地旁证;登记 D2/D3 两处口径疑点;身份级确认需旧机 still249.tsv。

## 6. 已知失败用例的诊断性单跑(非门禁)

按 R9 C1 预测,对 `codex-core ... connection_failures_increment_retry_telemetry_without_consuming_retry_budget`(119 开放集成员,本会话此前从未通过)做单用例诊断运行(发生在 fix/fmt 之后,目的是检验预测,不作为任何修复的验证门禁):

**结果:PASS,31.832s(标记 slow,>30s)。** C1 的"当前树单跑仍 ~10.4s 失败"预测在本窗口不成立——内部 10s 遥测期限这次被满足,但 31.8s 的总时长(load≈31)显示路径上仍有显著延迟成分,与 C1 的"SCDynamicStore 拖慢但不一定每次撞穿期限"的负载相关性解释兼容。C1 维持"强候选、未定案";R9 文档 §4 的五点分段计时(T0–T4)仍是定案手段。该单跑通过不改变其在 119 开放集中的身份(与"单跑过/并发败"既有边界模式一致)。

## 4. 变更清单

产品代码:`outbound_proxy.rs`、`custom_ca.rs`、`route_aware_client_pool.rs`(注释)、`websocket-client/src/dialer.rs`、`exec-server/src/remote.rs`(cfg(test) 夹具)。
测试:`outbound_proxy_tests.rs`、`outbound_proxy_redirect_coverage_tests.rs`、`custom_ca.rs`(内联)、`custom_ca_tls_tests.rs`、`route_aware_tls_fallback_tests.rs`、`route_aware_client_pool_tests.rs`(回退 WIP 改写)、`websocket-client/src/dialer_tests.rs`。
文档:`scdynamicstore-compat-spec-2026-10-10.md`、本文件。
无依赖、锁文件、schema 变更。

## Codex 2026-10-10 独立复审补充

上述全量数字为Claude原批报告。本次未复跑；可读HTTP日志仅首轮79/76PASS/3FAIL/56skipped，其他最终135/20/616/10缺可独立绑定的完整收据，缺日志不证明未执行。app-server-transport尾可确认157/157。当前远端run37967668519三平台workspace与UbuntuBazel四job均failure，不能称CI已补跨平台验收。报告§6的fix/fmt后诊断不作为该批门禁；下轮按AGENTS恢复测试在最终fix/fmt之前的流程。详细确认发现、119账目与全部后续任务见 `rig-production-completion-2026-10-10/review.md` 与 Spec/Plan/Tasks。
