# R9 静态分析:sqlx establish 卡点与 retry 遥测链路(2026-10-10)

状态:**只读静态调查**,未修改任何产品/测试代码。基线为 `pkg3-failure-signatures-worklist.md` R9 节及其引用的实测(r9-sample.txt、r10-verify4.log)。原始采样/日志工件在 `/Volumes/soddygo/git-workspace/codex-tmp/pkg3/`,本机当前未挂载,**本文只能引用 worklist 的转述,未复核原始采样文件**。

核心结论(TL;DR):

1. **遥测链路本身不写 SQLite**。retry 遥测是进程内 tracing event(`codex-client/src/retry.rs:72-81`),由测试自装的 subscriber 层直接观测(`core/tests/suite/retry_after.rs:137-184`),中间没有 SQLite 落盘环节。otel crate 亦不依赖 sqlx(`otel/Cargo.toml` 无 sqlx)。R9 采样中的 sqlx establish 属于**状态持久化车道**(turn 启动前置的线程元数据/rollout 对账),它卡住的是"首个模型请求发出之前"的阶段,而非遥测导出。
2. **establish 的唤醒原语不是 flume**。`ConnectionWorker::establish` 的握手用 `futures_channel::oneshot`(worker 发送,sqlx-sqlite `connection/worker.rs:106/131/347`);flume 只用于建立后的命令流(worker.rs:111/146/372-386)。"worker 线程在 flume recv 等待"是**已建立连接的空闲常态**(也是未 close 连接的泄漏态),不构成唤醒丢失证据。
3. **静态发现一个与全部实测观测吻合、但未被 R9 采样直接捕获的强候选(C1)**:模型传输在默认策略(ReqwestDefault、非托管)下**在运行时线程上同步急切构建** reqwest 客户端(`build_api_transport` → `build_default_client` → `builder.build()`),构建时经 `TransportDefault` 路由加载 macOS 系统代理(SCDynamicStore)。本机实测同原语 12.28s(R7 已证)。在 `current_thread` 单线程运行时上,该同步调用**冻结整个运行时**(含 10s 计时器)→ 首个模型请求未发出 → `next_retry` 期限在解冻后首次 poll 时判过期 → ~10.4s 失败。这与 r9-sample 的"该窗口模型请求未发出"直接吻合。
4. **边界警示**:R7 的字面量 Direct 早退**当前工作树已回退**(HEAD 有 `destination_host_skips_system_proxy`,`git show HEAD:codex-rs/http-client/src/outbound_proxy.rs:290/359/378`;工作树 diff 删除了它,理由是绕过 env/system/显式代理)。注意:该早退只作用于路由解析,**对模型传输的默认急切路径本就不生效**(`default_client.rs:293-298` 不经路由解析)——所以 C1 在 HEAD 与工作树上都成立,回退只影响池形态调用方(exec-server 等)。

---

## 1. sqlx 使用清单与池配置

### 1.1 谁在用 sqlx

- workspace 依赖:`codex-rs/Cargo.toml:491-505`,`sqlx = "=0.9.0"`(features 含 `sqlite-bundled`、`runtime-tokio`、`tls-rustls`),另有 `sqlx-macros`/`sqlx-sqlite` =0.9.0。Cargo.lock:`sqlx 0.9.0`、`flume 0.12.0`、`futures-channel 0.3.34`。
- 使用方 crate:`state`(状态库)、`thread-store`(本地线程存储)、`app-server`、`cli`、`ext/agent-message-board`。core 自身不直接依赖 sqlx,经 `codex-state` 间接使用(`core/Cargo.toml:73`)。
- `CODEX_SQLITE_HOME`(`SQLITE_HOME_ENV`,`state/src/lib.rs:128`)只决定 sqlite home 目录;core 集成测试用临时目录(`core/tests/common/test_codex.rs:784` → `init_state_db`),不读该 env。

### 1.2 池的建立与配置(全部收口在 state/src/sqlite.rs)

`SqliteConfig::open_read_write_pool`(`state/src/sqlite.rs:310-365`)是所有可写库的唯一入口:

- `SqliteConnectOptions`:`create_if_missing(true)`、`synchronous(Normal)`、`busy_timeout(5s)`、`log_statements(Off)`(sqlite.rs:311-316)。
- `SqlitePoolOptions`:**仅设置 `max_connections(5)` 与 `after_connect`**(sqlite.rs:320-359)。未覆盖的项走 sqlx 默认(sqlx-core `pool/options.rs:149-162`):`min_connections=0`、`acquire_timeout=30s`、`idle_timeout=Some(10min)`、`max_lifetime=Some(30min)`、`test_before_acquire=true`。
- `after_connect` 逐条执行:`PRAGMA auto_vacuum` 读、空库时 `PRAGMA auto_vacuum = INCREMENTAL`、`PRAGMA journal_mode = WAL`(sqlite.rs:325-347)。注释明确 WAL setter 会取写锁(sqlite.rs:329-330)。
- **错误短路**:`tokio::select! { biased; init_error_rx.recv() => Err(e), connect_with => … }`(sqlite.rs:360-364)。存在的原因(sqlite.rs:317-318 注释):sqlx 对 after_connect 错误会**重试到 acquire_timeout 耗尽再报 PoolTimedOut**(见 §3.3 inner.rs connect 循环),该 shim 让首个初始化错误直接返回。
- 只读池:`open_read_only_pool`(sqlite.rs:368-385),`max_connections(1)`。

`StateRuntime::init_inner`(`state/src/runtime.rs:125-282`)**串行**打开 5 个库(state/logs/goals/memories/queue),每个先 `open_runtime_db`(sqlite.rs:263-307:open + migrate 两阶段)再进下一个;随后 `ensure_backfill_state_row_in_pool`(runtime.rs:206)、post_init 查询(runtime.rs:226-232)、启动 reclamation worker(runtime.rs:258)与 30min 日志维护(runtime.rs:280)。`SqlitePoolOptions::connect_with`(sqlx-core `pool/options.rs:537-559`)会**立刻 acquire 一次再释放**——所以每个池在 build 阶段就建立第一条连接(经历完整 establish + after_connect PRAGMA)。

入口链:core 测试 fixture `test_codex().build_with_auto_env` → `build_with_home_and_base_url` → `build_from_config`(`core/tests/common/test_codex.rs:718-773` → `:784` `init_state_db`)→ `core/src/state_db_bridge.rs:6-8` → `rollout/src/state_db.rs:45-128`(`StateRuntime::init` + backfill gate;非 test 编译下 poll 1s / 总超时 30s,`rollout/src/state_db.rs:31-38`)。

### 1.3 ConnectionWorker 的启动与通道(sqlx-sqlite 0.9.0 源码)

每条 SQLite 连接一个专属 OS 线程(`sqlx-sqlite-0.9.0/src/connection/worker.rs:25-29`)。`ConnectionWorker::establish`(worker.rs:105-348)流程:

1. async 侧创建 `futures_channel::oneshot`(worker.rs:106),`thread::Builder::spawn` 起线程,线程名 `sqlx-sqlite-worker-{id}`(options/mod.rs:212,采样时可按名区分)。
2. worker 线程内:建 `flume::bounded(command_channel_size=50)` 命令通道(worker.rs:111;options/mod.rs:213),执行 `params.establish()`(worker.rs:113 → `connection/establish.rs:159-197`:**`sqlite3_open_v2` + `sqlite3_busy_timeout`,不执行任何 SQL**;SQLite 打开本身惰性,加锁/读文件发生在首条语句)。
3. 成功后 `establish_tx.send(Ok(Self{..}))`(worker.rs:131-139),随即进入命令循环 `for (cmd, span) in command_rx`(worker.rs:146)——**flume 阻塞 recv 即"空闲等命令",这是已建立 worker 的常态**。
4. async 侧 `establish_rx.await`(worker.rs:347)——`ConnectionWorker::establish` 唯一的挂点。

**唤醒责任**:worker → async 用 futures-channel oneshot(send 即唤醒已注册 waker,值先入槽,不存在"接收方未睡先发"丢失问题);async → worker 用 flume `send_async`(worker.rs:374-386、447-450);Execute 结果流 worker → async 用 flume bounded rx(worker.rs:372)。特殊命令:`UnlockDb` 让 worker 在自己线程上 `futures_executor::block_on(shared.conn.lock())`(worker.rs:328-331,服务于 `lock_handle`,core 仅 `sqlite_integrity_check` 用到,`state/src/runtime.rs:409-425`);`Shutdown` 由 `SqliteConnection::close` 发送并等待线程退出(worker.rs:335-341;connection/mod.rs:220-234)。

### 1.4 池的运行期行为(sqlx-core 0.9.0)

- acquire(`pool/inner.rs:246-322`):deadline = now + acquire_timeout(30s);先弹空闲连接,`test_before_acquire=true` 时先 `ping`(inner.rs:469-479 → worker.rs:412-413 一轮 flume 往返);无可弹则 `connect(deadline, guard)`。
- connect(**在调用方任务上内联执行,不 spawn**;inner.rs:325-396):`connect_options.connect()` 即 `SqliteConnection::establish`(connection/mod.rs:185-192);after_connect 失败 → 记日志 → close_hard → **退避重试直到 deadline → PoolTimedOut**。
- 维护任务(`spawn_maintenance_tasks`,inner.rs:507-560):period = min(max_lifetime, idle_timeout) = **10 分钟**;测试 10s 窗口内 reaper 不会醒来,排除其对窗口内行为的影响。
- 释放/关闭:`PoolConnection::drop` → `spawn(return_to_pool)`(connection.rs:199-208;注释明言"dying runtime 中该 future 可能不执行",issue 1396);`PoolInner::drop` 仅 `mark_closed()`(inner.rs:441-450)。
- **codex 侧 reclamation**:`SqliteReclamationWorker`(`state/src/runtime/reclamation.rs:37-76`)首睡 `IDLE_INTERVAL=60s` 后才碰文件锁/开独立连接——10s 窗口内完全休眠。

## 2. 失败测试的完整链路与各步阻塞点

失败代表:`connection_failures_increment_retry_telemetry_without_consuming_retry_budget`(`core/tests/suite/retry_after.rs:1663-1732`),`#[tokio::test(flavor = "current_thread")]`——**单线程运行时,所有任务共用一条线程;任何同步阻塞都会饿死全部任务**。这里仅分析 Core 历史 #161；旧 R9 六项实际为 OTLP #263–268，另有 rmcp #270/#282。这些身份不属于本文件同构家族，必须分别复现，不能共享本节归因。

时序链(箭头后是阻塞点):

```
[build] test_codex().build_with_auto_env (test_codex.rs:784)
  └─ init_state_db → StateRuntime::init (runtime.rs:125)
       └─ 5×(open_read_write_pool + migrate)  ← B1: 每池首条 establish + WAL/auto_vacuum PRAGMA(写锁);
          串行 5 次;busy_timeout 5s 兜底;临时目录无跨进程争用
       └─ backfill gate (rollout/src/state_db.rs:130-192) ← B2: 新 home 秒过
[turn] submit_user_input (retry_after.rs:1684)
  └─ run_turn (core/src/session/turn.rs:164)
       └─ run_hooks_and_record_inputs → rollout JSONL 追加 + 元数据对账
            └─ LocalThreadStore::update_thread_metadata (thread-store/src/local/update_thread_metadata.rs:111-232)
                 └─ state 池 acquire(空闲 ping 往返 / 池扩容 establish)← B3: r9-sample 观测点
       └─ run_sampling_request (turn.rs:525→1620) → try_run_sampling_request (turn.rs:2535)
            └─ stream_responses_api (core/src/client.rs:1888,每次尝试循环内 :1919)
                 └─ build_api_transport (core/src/client.rs:1227-1249) ← B4: ★同步、急切、在运行时线程上
                      └─ create_client_for_route (login/src/auth/default_client.rs:258-306)
                           └─ 默认策略分支(非托管 network_policy + ReqwestDefault,default_client.rs:293-298)
                              → build_default_client (default_client.rs:425-432)
                              → build_with_transport_default_proxy_and_custom_ca_fallback
                                (client_builder.rs:263-269 → 298-345)
                              → build_reqwest_client_with_custom_ca (custom_ca.rs:182-186)
                              → reqwest ClientBuilder::build()
                                 ← B5: ★reqwest 构建加载 macOS 系统代理 → SCDynamicStore(R7 实测 12.28s;
                                    另叠加本进程首个 Security 框架初始化,R10 实测秒级)
            └─ TCP 连接 unavailable 地址 → TransportError::Connection → CodexErr ConnectionFailed
  [retry] handle_response_stream_error (core/src/responses_retry.rs:57-176)
       └─ UnboundedConnectionRetries 分支 (responses_retry.rs:93-118)
            └─ record_retry! (responses_retry.rs:112 → codex-client/src/retry.rs:63-82)
  [观测] tracing event(target=codex_otel.trace_safe, name=codex.retry)
       └─ RetryTelemetryLayer::on_event (retry_after.rs:137-184) → mpsc unbounded → next_retry 10s (retry_after.rs:228-233)
```

**B4 的两种形态(重要区分)**:
- **本测试(默认策略)**:network_policy 非托管(config/mod.rs:4457 `Default::default()`,is_managed=false,network_policy.rs:218)+ `OutboundProxyPolicy::ReqwestDefault`(config/mod.rs:1730-1735,`respect_system_proxy` 默认 false)→ `create_client_for_route_with_builder` 走 default_client.rs:293-298 → `build_default_client` **立即同步构建** reqwest 客户端并包成 `HttpClientBackend::Direct`(client_builder.rs:263-269)。构建发生在调用线程 = current_thread 运行时的唯一线程;**且 stream_responses_api 的循环每次尝试都调 build_api_transport(client.rs:1919),该形态无任何客户端缓存**。
- **托管/RespectSystemProxy 形态(R7 的 exec-server 上下文)**:`RouteAwareClientPool` 路径,构建推迟到首个请求,在 `tokio::task::spawn_blocking` 内执行(route_aware_client_pool.rs:505-552,外层 JoinHandle await **无超时**),路由解析 `resolve_proxy_route_async` 对 ReqwestDefault 立即返回 TransportDefault(工作树 outbound_proxy.rs:341-346),`configure_builder_for_resolved_route(TransportDefault) → Ok(builder)` 原样(outbound_proxy.rs:505-517)→ 同样落到加载系统代理的 `builder.build()`。两种形态最终撞同一个 SCDynamicStore 原语,区别只在冻结哪条线程(运行时线程 vs blocking 线程)。

**关键纠正**:从 record_retry! 到测试观测之间**没有 SQLite 写入、没有 otel 导出网络**——是纯进程内 tracing 派发(current_thread 运行时下 event 与 subscriber 同线程,set_default 的线程局部派发器可见,retry_after.rs:210-218)。worklist 中"otel/retry 家族"的 otel 部分是失败签名归类(otel-collector-timeout 2 项),与 core retry 测试不共享导出路径;`otel` crate 无 sqlx 依赖。

阻塞点小结:B1/B3 是 sqlx 车道的等待(采样所见);B5 是 http 车道的等待(R7 已证原语)。B4→B5 的发生频率按形态而异:**默认(急切)形态每次尝试都构建、无缓存**(client.rs:1919 循环内直调);**池形态只在每进程每路由第一次**(route 缓存,route_aware_client_pool.rs:484-487/540-548)。nextest 每测试一进程,两种形态的首个构建都必然是冷的。

## 3. 候选根因分析

### (a) sqlx establish 在 macOS 辅助线程的文件锁/IO 阻塞 —— 不确定(对本窗口的直接致因:弱)

- 源码上 establish 阶段被观测到的真实 IO 只有 `sqlite3_open_v2`(打开/建文件,不加锁)与 after_connect 的 WAL/auto_vacuum PRAGMA(取写锁,受 busy_timeout=5s 约束,sqlite.rs:315/325-347)。
- 采样转述"worker 线程在 flume recv 等待":若该 worker 即 establishing 连接的 worker,则 establish **已成功**(worker.rs:131 已发送)且在等命令——与"IO 阻塞在 open"互斥;若该 worker 是其他空闲连接的 worker(5 池至少 1 条/池,空闲态一律 flume recv),则转述无法定位 establishing worker 的真实状态。**原始采样未挂载,无法按线程名(`sqlx-sqlite-worker-{id}`,options/mod.rs:212)对号**。
- 结论:静态**不能确认也不能排除**;但即便发生,busy_timeout(5s)与 acquire_timeout(30s)都给出有限上界,单独难以精确制造 10.46s 的确定性失败。

### (b) flume recv 唤醒丢失(发送方在接收方睡眠前发送)—— 基本排除(源码级)

- establish 握手**不用 flume**:worker.rs:106 的 `futures_channel::oneshot`,send 先写槽再唤醒(worker.rs:131),接收首次 poll 时值已在,不存在"先发后睡丢唤醒"的形态。flume 0.12 的 bounded 通道同理(值先入队再唤醒)。
- 采样所见 flume recv 是 worker 命令循环空闲态(worker.rs:146),不是唤醒点;establish 之后若命令送达,flume 会唤醒 worker。
- 真正与"async 侧长期停在 establish"兼容的解释只剩:**等待者所在任务从未被再次轮询**(单线程运行时被同步调用占死)——这归入 C1 的从属形态(见下),而非通道原语缺陷。
- 注意形态限制:suspended 的异步任务**不占用任何线程栈**;`sample` 能看到 `ConnectionWorker::establish` 帧,只可能是某线程正处在其 poll 链内(current_thread 测试的主线程 block_on park,或某 block_on 桥)。这一形态本身是"采样瞬间快照"的正常样貌,worklist 也已声明"不能据此确认确切等待原语"。

### (c) 池满/串行化 —— 基本排除(作为本次直接卡点)

- 每池 max=5、fair、acquire_timeout 30s;池满的表现是**等信号量**(inner.rs:131 acquire_permit),不会出现在 establish 栈里;采样所见恰是 establish(说明池还有名额、正在开新连接)。
- current_thread 单线程运行时没有真并发,池扩容需要"一条连接跨 await 被占用 + 另一查询"(如 update_thread_metadata 的事务/对账),量级为 1-2 条,远不及 5。
- SQLite 写锁串行有 busy_timeout=5s 上界;且各库分文件(runtime.rs:110-111 注释),无跨库锁。

### (d) shutdown 泄漏 —— 作为 10s 卡点排除;作为采样样貌成立

- 失败测试从不调用 `shutdown_and_wait`/`StateRuntime::close`(TestCodex 无 Drop;test_codex.rs 全文仅 `Drop for TestEnv`:251)。测试结束 drop StateRuntime 时:`PoolConnection::drop → spawn(return_to_pool)`(sqlx-core connection.rs:199-208)在**正在关闭的 current_thread 运行时上可能不执行**(sqlx 注释引 issue 1396);`PoolInner::drop` 仅 mark_closed(inner.rs:441-450);被 drop 的 `SqliteConnection` **不发 Shutdown 命令**(close 才发,connection/mod.rs:220-234;无 Drop impl)→ worker 线程永驻 flume recv。
- 因此"worker 在 flume recv"有两种含义:测试**进行中**= 正常空闲;测试**结束后**= 泄漏线程(每进程 ≤ ~6 条,nextest 隔离,不跨测试放大)。它解释采样样貌,但解释不了窗口内的 10s 停滞。
- 次生风险(登记,非本因):reclamation worker 的 close 等待(runtime.rs:303-313 → reclamation.rs:65-70)在无序关闭下不触发;泄漏线程持有 sqlite handle 直到进程退出。

### C1(补充候选,静态新发现):模型传输的 reqwest 客户端同步构建触发 SCDynamicStore 冻结 —— 机制强度:高;直接证据:缺

- 链路(全部源码可引,见 §2 B4/B5):默认策略下 `stream_responses_api` 每次尝试在**运行时线程上同步**执行 `build_api_transport`(client.rs:1919→1241)→ `build_default_client`(default_client.rs:425-432)→ `build_with_custom_ca_fallback(TransportDefault)`(client_builder.rs:263-269/298-345;`reqwest_builder` 对 TransportDefault 返回原样 builder,client_builder.rs:349-356)→ `builder.build()`。reqwest 侧:0.12.28 的 `ClientBuilder::build()` 在 `auto_sys_proxy`(默认 true,仅 `no_proxy()` 关闭)时**每次构建都** `proxies.push(ProxyMatcher::system())`(reqwest-0.12.28/src/async_impl/client.rs:417-420 → proxies.rs:514-518 `Matcher::system()`)→ hyper-util `matcher::Matcher::from_system()`(hyper-util-0.1.20/src/client/proxy/matcher.rs:109)→ `SCDynamicStoreCreateWithOptions`。**reqwest/hyper-util 均无进程级缓存**——即使测试进程里更早有组件建过客户端,模型请求的每次构建仍要再付一次系统代理加载费(这使 C1 对"早前已暖"稳健)。
- 本机实测(R7,exec-server 的池/spawn_blocking 形态):**12.28s**;对照实验证明仅无 CFRunLoop 的辅助线程如此。**对本测试的形态更尖锐**:current_thread 运行时只有一条线程,B4 的同步构建会把整个运行时(含 `next_retry` 的 10s 计时器、含所有 sqlx 等待任务)冻结到构建完成。计时器基于 wall-clock:解冻后 timeout future 首次 poll 即判 Elapsed → 失败时刻 ≈ 构建结束时刻(约 0.4-0.5s 前置 + 构建时长)。构建时长可变(R7 一次测得 12.28s;本进程首建还叠加 Security 框架初始化,R10 实测秒级),**10.463s 失败 = 前置 ~0.5s + ~10s 构建**,落在该原语的量级区间内。
- 与已证观测的吻合点:①"该窗口模型请求未发出"——客户端未建成则请求无法发出,逐字吻合;②R6(file-watcher)补丁无效——不同车道;③单例 `--test-threads 1` 仍失败、与负载无关——同步系统调用,与调度无关;④按进程确定性复现——每进程首个此类构建必冷(nextest 每测试一进程;且该路径**每次尝试都重建**客户端,无进程内缓存兜底)。
- **未闭合点**:①r9-sample 未(被转述)捕获 SCDynamicStore 栈——若采样窗口落在 B4 冻结期,主线程栈应显示 `build_api_transport → … → SCDynamicStore`;实际显示 establish,说明采样时刻在 B4 之前的元数据/sqlite 阶段(B3),或当时 B4 未发生/已过去。采样是快照,一次未捕获不构成排除;②r10-verify4 运行时树上是否含 R7 直连早退(HEAD 有、工作树已回退)无从判定——若当时含早退,127.0.0.1 字面量在**池形态**下会走 `Direct → builder.no_proxy()`(client_builder.rs:353-355);但注意本测试的默认形态根本不经过路由解析(default_client.rs:293-298 直接 `build_default_client`),**R7 早退对默认形态本来就不生效**,这使得"r10-verify4 时已带 R7 修复却仍失败"与 C1 完全兼容;③采样归属(哪条 worker)未定。
- 附注:R10 的 `warm_secure_transport_once`(http-client/src/test_warmup.rs:10-56)只暖 SSLCreateContext,不暖 SCDynamicStore,也解释不了本项;`test_codex` 构建期对 bootstrap mock(127.0.0.1)若有任何 HTTP 拉取,同样会先撞一次该原语(取决于 fixture 是否发请求)。

## 4. 最小同窗复现方案(不改断言、不放宽 deadline)

原则:nextest 每测试一进程已提供确定性隔离;所有诊断探针为**临时 eprintln(按 env 门控,如 `CODEX_R9_DIAG=1`),采证后移除**(沿用 worklist R1b/R7 的探针纪律)。

### 4.1 三层复现(由隔离到全链)

**R-A|sqlx establish 独立复现(隔离 (a)/(b)/(c))**——state crate 集成测试(或 core suite 内新建,复用 `SqliteConfig::new_for_testing`):
1. 计时 `StateRuntime::init`(分段:每池 open/migrate,可临时注入 `DbTelemetry`——现成钩子 `init_with_telemetry_for_tests`,runtime.rs:116-123;core 测试默认走 `init` 无注入)。
2. 构造确定性池扩容:持有一条连接跨 await(开事务)并发第二条查询 → 强制第 2 条 establish;断言各段耗时(预期 <100ms 量级;**断言上限另定,如 1s,不触碰原 10s 断言**)。
3. 若任一 establish >1s:同窗用 `sample` 抓包区分 worker 栈:`sqlite3_open_v2`(establish.rs:160,真 IO)vs flume recv(空闲/泄漏)vs `sqlite3_step`(PRAGMA 写锁,busy 5s)。

**R-B|client 冷构建复现(隔离 C1)**——login 或 http-client 测试:
1. 计时冷态 `create_client_for_route(&factory, "http://127.0.0.1:1/", ClientRouteClass::Api, ClientRedirectPolicy::Default)`(login/src/auth/default_client.rs:258)首次完成时长——这是模型传输的**真实同构调用**;同窗对照 `builder.no_proxy()` 变体与暖态二次调用(注意 reqwest 无进程缓存,二次调用仍付系统代理费;暖态对照应改为"先在主线程建一次暖 SCDynamicStore 动态存储",R7 对照:主线程建仅 7ms)。
2. 预期(若 C1 成立):冷构建 ≈ 10-13s(R7 minirepro 同量级),no_proxy 变体 <100ms。
3. 变量分离:`SC_DYNAMIC_STORE` 无官方开关;对照臂可用主线程预建(reqwest Client)或 `no_proxy()`。

**R-C|全链复现(原测试 + 分段计时)**——对 retry_after.rs:1664 原用例加 5 个计时点(eprintln,env 门控):

| 计时点 | 位置 | 目的 |
|---|---|---|
| T0 | retry_after.rs:1668(install 后) | 基准 |
| T1 | test_codex.rs:784 返回处(init_state_db 后) | B1 总耗时 |
| T2 | thread-store `update_thread_metadata.rs:123`(require_sqlite_write 判定后)/ 各 acquire 前后 | B3 池等待与 establish |
| T3 | core/src/client.rs:1241(build_api_transport 进入/返回)——同步段 | B4+B5 客户端构建时长(核心判别) |
| T4 | responses_retry.rs:112(record_retry! 处) | 事件产生时刻 |

判定矩阵(T3 时长 × 结果):

- T3 ≈ 10-13s 且失败 → **C1 证实**(R7 同原语、不同调用方;进入修复讨论:模型传输的客户端缓存/构建超时/预热——注意工作树已因代理绕过问题回退 R7 字面量早退,且该早退本就不覆盖默认急切路径(default_client.rs:293-298 不经路由解析),修复需另行设计,不在本文范围)。
- T3 <100ms 且失败 → C1 排除;看 T2:若 establish 段 >1s → 转向 (a);若 T2 亦快 → 看 T4 是否从未到达(卡在 TCP 连接/更早),重新采样。
- 全部快但仍失败 → 链路重审(event 派发线程/feature 门控 `Feature::UnboundedConnectionRetries` 是否默认开,可用 RUST_LOG=trace 旁证)。

### 4.2 采样规格(同窗)

`sample -file r9b.txt -mayDie <pid> 10` 于 R-C 运行窗口内持续;读栈时按线程分类:主线程(当前 runtime block_on——**若 C1 成立,冻结期应显示 `build_api_transport → … → SCDynamicStoreCreateWithOptions` 帧而非 establish 帧**)、`sqlx-sqlite-worker-N`(options/mod.rs:212)、tokio blocking 线程(池形态下找 `matcher::from_system`)。**要求把"async 侧 establish 帧"与"各 worker 帧"做同帧配对**(按连接序号/文件路径),这一步是区分 (a)/(b)/(C1-时序) 的唯一采样手段;原始 r9-sample 若能重新挂载,先做同样配对复盘。

## 5. 已证 vs 候验清单

**已证(worklist 实测,本文引用不新增测量)**:
1. otel/retry 家族 6 项 t2 全败;代表单例本窗复现 FAIL 10.5s,`timed out waiting for retry telemetry`(worklist §2)。
2. r9-sample:async 侧停在 `sqlx_sqlite::ConnectionWorker::establish`,worker 线程 flume recv;窗口内模型请求未发出,10s 到期(worklist R9 节)。
3. R6 补丁后仍在 10.463s 失败(r10-verify4;worklist R9/R10 节)——file-watcher 车道与本项无关。
4. 本机 SCDynamicStore 在无 CFRunLoop 线程创建可阻塞 ~12.28s;reqwest 默认构建无条件加载系统代理(worklist R7 节,exec-server 上下文四层取证)。
5. 本机 Security 框架冷态秒级(SSLCreateContext 首建、TrustSettings 迭代;worklist R10/R11 节)。

**候选(本文静态分析,待 4.1/4.2 判别)**:
- C1 模型传输同步构建 reqwest 客户端触发 SCDynamicStore(机制强;每次尝试构建、无缓存;与全部已证观测吻合;缺直接采样证据)。
- (a) establish 阶段文件 IO/锁(不确定;受 busy/acquire 超时约束;采样归属未定)。
- (b) 通道唤醒丢失(源码级排除;残余形态=任务未被轮询,归 C1 从属)。
- (c) 池满/串行(基本排除;采样形态相反)。
- (d) shutdown 泄漏(作为卡点排除;作为"worker 在 flume recv"样貌与资源问题成立,登记)。

**边界事实(影响复现解释)**:工作树已回退 R7 字面量 Direct 早退(HEAD `outbound_proxy.rs:290/359/378` 有 `destination_host_skips_system_proxy`,工作树删除;回退理由:短路会绕过 env/system/显式代理与 redirect 跳数,见工作树注释)。**当前树上 C1 暴露应重新生效——预测:当前工作树单跑 retry_after.rs:1664 在本机仍会以 ~10.4s 失败;这本身就是 R-C 的第一组数据。**

## 6. 关键源码索引

codex-rs:
- `Cargo.toml:491-505`(sqlx =0.9.0 工作区依赖)
- `state/src/sqlite.rs:310-365`(唯一可写池入口;max 5;busy 5s;after_connect PRAGMA;biased select);`:368-385`(只读池 max 1)
- `state/src/runtime.rs:125-282`(5 库串行 init + backfill + reclamation/维护启动);`:302-313`(close)
- `state/src/runtime/reclamation.rs:37-76`(60s 空闲起睡)
- `state/src/telemetry.rs:32-46`(进程级 OnceLock sink,core 测试未装 → 遥测落点静默)
- `rollout/src/state_db.rs:29-128`(init + backfill gate 常量)
- `core/tests/common/test_codex.rs:784`(init_state_db);`core/src/state_db_bridge.rs:6-8`
- `core/tests/suite/retry_after.rs:1663-1732`(失败用例);`:196-233`(capture/install/next_retry 10s);`:133-194`(layer 观测)
- `codex-client/src/retry.rs:63-82`(record_retry! 事件);`core/src/responses_retry.rs:23-24,93-118`(5s/60s 无界连接重试与 record_retry 调用点 :112)
- `core/src/client.rs:1227-1249`(build_api_transport,同步);`:1888-1923`(stream_responses_api 每次尝试建 transport);`login/src/auth/default_client.rs:258-306`(create_client_for_route:默认策略→急切构建);`:416-419`(default_http_client_builder);`:425-432`(build_default_client);`core/src/config/mod.rs:614,1729-1746,4457`(策略选择与默认非托管);`http-client/src/network_policy.rs:218`(is_managed)
- `http-client/src/client_builder.rs:196-222,263-269,288-345,349-356`(build_for_resolved_route / 急切构建 / TransportDefault→原样 builder);`http-client/src/custom_ca.rs:182-186`(最终 builder.build());`http-client/src/route_aware_client_pool.rs:454-554`(池形态:路由解析→缓存→spawn_blocking 构建,:505-552 无超时 await);`http-client/src/outbound_proxy.rs:341-346,505-517`(工作树 ReqwestDefault→TransportDefault;HEAD 含 Direct 早退 :290/:359/:378);`http-client/src/test_warmup.rs:10-56`(仅 TLS 暖启,不覆盖本项)
- reqwest/hyper-util(注册表源):`reqwest-0.12.28/src/async_impl/client.rs:409-421`(build() 每次推入 system matcher,无缓存);`reqwest-0.12.28/src/proxy.rs:514-518`(Matcher::system);`hyper-util-0.1.20/src/client/proxy/matcher.rs:109`(from_system → SCDynamicStore)
- `thread-store/src/local/mod.rs:152,291-305`(thread_history 池惰性 OnceCell);`thread-store/src/local/update_thread_metadata.rs:111-232`(turn 侧 state 池写入)

sqlx(注册表源 `~/.cargo/registry/src/index.crates.io-*/`):
- `sqlx-sqlite-0.9.0/src/connection/mod.rs:185-192`(SqliteConnection::establish);`:220-240`(close/close_hard)
- `sqlx-sqlite-0.9.0/src/connection/worker.rs:105-348`(worker 建立;:111 flume bounded(50);:131 oneshot send;:146 命令循环;:328-331 UnlockDb block_on;:335-341 Shutdown;:347 establish_rx.await)
- `sqlx-sqlite-0.9.0/src/connection/establish.rs:159-197`(sqlite3_open_v2;无 SQL)
- `sqlx-sqlite-0.9.0/src/options/mod.rs:199-214`(默认值;线程名 :212;通道 50 :213)
- `sqlx-core-0.9.0/src/pool/options.rs:149-162`(默认 max 10/acquire 30s/idle 10min/lifetime 30min/test_before_acquire true);`:537-559`(connect_with 先 acquire 一次)
- `sqlx-core-0.9.0/src/pool/inner.rs:246-322`(acquire);`:325-396`(connect 内联重试);`:441-450`(PoolInner::drop 仅 mark_closed);`:469-479`(ping);`:507-560`(维护任务 period=10min)
- `sqlx-core-0.9.0/src/pool/connection.rs:199-208`(drop→spawn return_to_pool;dying runtime 警示)
