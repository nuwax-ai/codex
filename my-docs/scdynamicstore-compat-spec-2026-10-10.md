# Spec:macOS 系统代理解析的兼容性能设计(SCDynamicStore 阻塞跟进)

日期:2026-10-10
状态:**设计提案,未实施**。本文回答 codex-pending-review-2026-10-09.md 第 2 项中"若保留生产性能优化,先给具体兼容 Spec/Plan"的要求;同日已先落地的是最小兼容修复(恢复 `ReqwestDefault → TransportDefault`,见 `my-docs/claude-p1-fixes-2026-10-10.md`),本设计是其后续。

## 1. 问题陈述

产品事实(均已在 pkg3 worklist R7 节实测登记):

- reqwest 0.12.28 默认客户端构建会经 hyper-util `matcher::Matcher::from_system()` 读 macOS 系统代理配置;环境变量(`HTTP_PROXY` 等)已设置时跳过系统查询。
- `SCDynamicStoreCreateWithOptions` 在**没有 CFRunLoop 的辅助线程**上可阻塞十余秒(本机 minirepro:主线程 7ms,辅助线程 12.28s,30s 窗口 100% 采样命中);慢的程度随系统服务争用波动。
- 影响面:`RouteAwareClientPool` 在 `spawn_blocking` 里构建客户端(线程无 runloop),生产上首个客户端构建、以及测试并行时每个测试进程的首次构建都会撞上。
- 2026-10-09 WIP 曾用"字面量 IP/localhost → Direct"短路规避(让 127.0.0.1 目标跳过系统查询),但该短路同时**清除了 env/系统/builder 显式代理与 NO_PROXY**(Direct → `builder.no_proxy()`),并随自动重定向扩散到域名 hop——这是 P1 代理契约缺陷,已于 2026-10-10 恢复默认并补回归。

因此需要一个**不改变代理语义**的系统性规避:完整保留 系统 > env / builder 显式 > NO_PROXY 优先级(hyper-util 实际语义:env 先于 system;builder 显式先于两者),只消除"无 runloop 线程上的阻塞系统查询"。

## 2. 目标与非目标

目标:
1. `ReqwestDefault` 语义逐字恢复(已达成,本设计不回退它)。
2. macOS 上系统代理解析不再在无 runloop 的线程上发生。
3. 解析结果按进程缓存 + TTL(对齐 `SYSTEM_PROXY_SUCCESS_CACHE_TTL=60s` / `UNAVAILABLE=5s` 的既有模式),键与失效语义明确。
4. 可测试:注入时钟/环境/系统应答,不需要真实 SCDynamicStore。

非目标:
- 不改变 `RespectSystemProxy` 策略的现有优先级(system → env fallback,与 reqwest 的 env → system 相反是有意为之的既有产品语义)。
- 不在本设计里处理 trust-store 缓存(另行见 custom_ca 的源键 TTL 设计,已实施)。
- 不承诺消除底层系统调用本身的耗时——只把它移到正确的线程并限频。

## 3. 方案(候选 A,推荐)

**A. 专职 CFRunLoop 线程 + 进程级缓存,替换 reqwest 内建系统查询。**

核心思路:不让 reqwest 在构建时自己调 `from_system()`(不可控、在调用线程执行),而是在 http-client 内接管"系统代理决策"的获取:

1. 新模块 `http-client/src/system_proxy_loader.rs`(macOS-only 实现,其他平台返回 `Unavailable`):
   - 进程启动惰性创建**一条专职线程**,该线程在首次查询前 `CFRunLoopRun`(或 `SCDynamicStore` 要求的等价 runloop 服务),所有 `SCDynamicStore` 访问只发生在这条线程。
   - 线程用请求通道接收 `(request_url)` 查询,在 runloop 内执行 `macos::resolve`(复用现有 `outbound_proxy/macos.rs` 逻辑),应答走 oneshot 返回。
   - 结果写入既有 `SYSTEM_PROXY_CACHE`(60s/5s TTL,单飞),查询本身串行天然单飞。
2. 池构建客户端时,`ReqwestDefault` 不再让 reqwest 查系统:
   - 若缓存/专职线程给出 `Proxy{url}` → `builder.proxy(explicit)`(显式配置,reqwest 不再自查系统);
   - 若 `Direct`/`Unavailable` 且 env 无代理 → `builder.no_proxy()`(等价于"系统与 env 都说直连");
   - env 有代理 → `builder.proxy(env_url)` + `NO_PROXY` 语义(优先级:builder 显式 > env,与 hyper-util 一致;system 仅在 env 未设时被咨询,顺序也对齐 hyper-util `from_system`)。
3. 关键约束(兼容性验收标准):
   - 对任意目的地(含字面量 IP/localhost/域名/大小写/尾点/IPv6),路由结果必须与 `reqwest` 默认行为一致——用双轨对照测试证明:同环境下,接管构建的客户端与 `reqwest::Client::builder().build()` 在同一组请求上产生相同的"是否经代理/经哪个代理"判定(本地 proxy 计数断言)。
   - NO_PROXY 语义逐 hop 保持(reqwest 自身重定向时按 hop 重新匹配)。
   - builder 显式代理仍最高优先(接管只发生在"未显式配置"时)。

## 4. 方案(候选 B,备选)

**B. 上游修复推动 + 短期旁路。**
向 hyper-util/reqwest 报告"SCDynamicStore 在无 runloop 线程阻塞"并推动其将系统查询移入内部专用线程或提供 `Proxy::system_offloaded()`;在合入前,方案 A 作为本地桥接。B 单独不可交付(时间不可控),只作为 A 的退出路径。

## 5. 测试计划(实施批必须包含)

1. 单元(注入):专职线程返回 Proxy/Direct/Unavailable × env 有/无代理 × builder 显式有/无 → 期望的 builder 配置形态。
2. 双轨对照(真实环境,macOS):本机系统代理手动开/关两组子进程(隔离 env),断言接管客户端与 reqwest 默认客户端行为一致(直连/经代理计数)。
3. 回归钉子:字面量 IP + HTTP_PROXY 的重定向逐 hop 走代理(已落地:`transport_default_literal_ip_and_redirect_hops_follow_environment_proxies`);localhost/IPv6/尾点解析表(已落地)。
4. 性能验收:并行 N 进程首建客户端不再出现 >2s 的构建段(对照 R7 的 12.28s 基线),在低载窗口测。
5. 不放宽任何 deadline;失败原样登记。

## 6. 风险与未决

- 专职线程的生命周期(进程退出前 join?守护?)与 panic 恢复(线程崩溃后降级为 Unavailable + 重建)。
- hyper-util `from_system` 在 env 半设(仅 HTTP_PROXY 设、HTTPS 未设)时按 scheme 分别决定;接管实现必须逐 scheme 复刻,对照测试要覆盖该矩阵。
- Windows(WinHTTP 已在 `outbound_proxy/windows.rs` 有类似同步查询)是否同构受益,实施时一并评估。
