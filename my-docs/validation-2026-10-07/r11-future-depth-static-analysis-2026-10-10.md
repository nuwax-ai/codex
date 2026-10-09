# R11 静态分析:guardian 溢出的 future 嵌套结构与 Box::pin 边界候选(2026-10-10)

> 方法:只读静态代码调查,未修改任何代码、未运行任何测试。所有行号以 2026-10-10 工作区(test 分支)为准。
> 上游输入:`pkg3-failure-signatures-worklist.md` §R11(143–151 行)。

## 0. 结论速览

1. 采样重试的五层循环**全部是"每次迭代创建/销毁内层 future"的替换型结构**,静态上不存在"第 N+1 次重试把第 N 次 attempt 的 future 包进当前 future"的增长点。worklist 中"采样请求重试链疑似每次重试加深 future 包裹"是**静态证伪候选**(待第 5 节实验最终裁决)。
2. "16MiB 仍溢出"**不能**作为增长型证据:溢出线程是测试自建的固定 4MiB 线程,`RUST_MIN_STACK` 对它无效(worklist 已记录此事实,本文补充其推论:因此 16MiB 变体只是复验了同一 4MiB 预算下的失败)。
3. 真正的深度大头是**单次 poll 的固定纵深**:从 spawned task 到 `WebSocketConnector::new` 的同步 rustls 证书加载,中间约 10 层未装箱 async fn + 3–4 层 select/timeout/instrument 包装,全部线性同栈下降,任何一层都没有任务边界。
4. `Box::pin` 只削减 future **体积(状态机尺寸)**,不削减 poll **深度**。唯一能在静态上真正减深度的改法,是把最深的同步尾巴(`WebSocketConnector::new` → 证书构建)挪出 poll 链(`spawn_blocking`,仓库已有先例)或在任务边界切断。
5. 已有的装箱点相当多(guardian review 侧几乎全程 boxed);**采样连接链(turn.rs:2582 → websocket-client/src/lib.rs:97)是唯一整段未装箱的高纵深路径**,也是活体采样观察到的栈底所在。

---

## 1. 已证事实(worklist 实测)与本文静态推导的边界

### 1.1 已证事实(全部引自 worklist §R11,不重复验证)

| 编号 | 事实 | worklist 出处 |
|---|---|---|
| F1 | 溢出用例 `guardian_ephemeral_retry_preserves_parallel_trunk_and_fork_history` 单跑(t1)PASS 2/2;模块 8 并发下 4/4 SIGABRT,恒为同一用例 | 145–148 行 |
| F2 | 该测试线程自建 `.stack_size(4MiB)`,`RUST_MIN_STACK=8/16MiB` 不控制它;16MiB 受控变体仍失败 | 147 行 + 202 行("R11实际线程固定4MiB") |
| F3 | 活体采样(30.1s 复现,sig-s1.txt)栈底:`turn::run_turn → run_sampling_request → try_run_sampling_request → ModelClientSession::stream → stream_responses_websocket → websocket_connection → ModelClient::connect_websocket → ResponsesWebsocketClient::connect → WebSocketConnector::new → build_rustls_client_config_with_custom_ca → rustls_native_certs::load_native_certs → security_framework TrustSettings::iter`,1377/1377 采样阻塞于此 | 149 行 |
| F4 | 证书缓存修复后 SIGABRT 仍复现但 30.1s→15.2s;结论"TrustSettings 是延迟放大器而非栈溢出主因" | 150 行 |
| F5 | 绕过 just 直接 cargo nextest(tokio worker 落回 2MiB 默认栈)时 17 个 `guardian_review*` 测试瞬时 SIGABRT | 149 行 |
| F6 | "~12 次 × 深栈后 8MiB 溢出"的重试次数为观察值,增长型为"疑似" | 149 行("疑似每次重试加深") |

### 1.2 本文静态推导(候选,未动态验证)

- C1:五层重试循环均为替换型,重试不累积 future 嵌套(§2.2)。
- C2:F5 的17个测试瞬时SIGABRT证明该执行条件存在栈敏感性，但未区分 future 构造/移动、状态体积与 poll 栈；没有直接测出单次 poll >2MiB。固定深链仍为待实验候选（§4.3）。
- C3:F2 的 16MiB 变体不构成增长型证据(§4.2)。
- C4:并发才溢出、单跑通过的差异,静态上不能由 future 嵌套解释;剩余候选是"负载下才走到更深的叶子路径/更多次重试把最深处组合点撞出来"或尚未定位的向量(§6.4 开放问题)。

---

## 2. 每次 attempt 的 future 嵌套结构

### 2.1 完整纵深链(自任务 spawn 到同步叶子)

溢出时刻的单条 poll 链(与 F3 采样栈底逐层对上,标注文件:行号):

```
[4MiB 测试线程 tests.rs:3513-3515]
  runtime.block_on(Box::pin(async {...}))            guardian/tests.rs:3520   ← 已装箱(仅最外层)
  └ tokio::spawn 的 reviewer 会话任务               session/mod.rs:936-940(session_loop)
    └ submission_loop → start_task → task_future     tasks/mod.rs:366-403
      └ tokio::spawn(task_future)                    tasks/mod.rs:404        ← 任务边界(同线程调度,不封顶深度)
        └ RegularTask::run 的 loop                   tasks/regular.rs:104-114
          └ run_turn(...).instrument(run_turn_span)  tasks/regular.rs:105-114  [循环 L1:替换型]
            └ run_turn(678 行单体 async fn)          core/src/session/turn.rs:164-841
              └ turn 内 loop                          turn.rs:427              [循环 L2:替换型]
                └ async { ... } 块                    turn.rs:500-536
                  └ run_sampling_request(133 行)      turn.rs:1620-1752
                    └ 重试 loop                       turn.rs:1655             [循环 L3:替换型 ★worklist 称的"重试循环"在此]
                      ├ try_run_sampling_request(...).await   turn.rs:1691-1702
                      │  └ try_run_sampling_request(676 行)   turn.rs:2535-3210
                      │    ├ client_session.stream(...)
                      │    │  .instrument("stream_request")   turn.rs:2593   (+1 层 Instrumented)
                      │    │  .or_cancel(&cancellation_token) turn.rs:2594   (+1 层 select)
                      │    │  └ ModelClientSession::stream     core/src/client.rs:2515-2617
                      │    │    └ stream_responses_websocket   client.rs:2105-2404
                      │    │      └ loop(鉴权恢复)             client.rs:2127 [循环 L4:替换型]
                      │    │        └ websocket_connection     client.rs:1601-1670
                      │    │          └ connect_websocket     client.rs:1262-1346
                      │    │            └ tokio::time::timeout(...)          client.rs:1282-1296 (+1 层)
                      │    │              └ ApiWebSocketResponsesClient::connect
                      │    │                codex-api/src/endpoint/responses_websocket.rs:394-420
                      │    │                └ connect_websocket(自由函数)      同文件 :499-515
                      │    │                  └ WebSocketConnector::new        websocket-client/src/lib.rs:81-107
                      │    │                    └ build_rustls_client_config_with_custom_ca  lib.rs:97(同步)
                      │    │                      └ NATIVE_ROOTS_CACHE 命中/未命中  http-client/src/custom_ca.rs:265,321-337
                      │    │                        └ rustls_native_certs::load_native_certs → TrustSettings::iter(同步 C 栈,F3 栈底)
                      │    └ 事件 loop               turn.rs:2629             [循环 L5:替换型]
                      │      └ stream.next().instrument(...).or_cancel(&preempt).or_cancel(&cancellation_token)
                      │        turn.rs:2645-2650                     (+2 层 select + 1 层 Instrumented)
                      │        └ ResponseStream::poll_next = mpsc recv  core/src/client_common.rs:122-136(到此变浅)
                      └ handle_response_stream_error(...).or_cancel(&preempt).or_cancel(&cancellation_token)
                        turn.rs:1725-1736                            (+2 层 select)
                        └ core/src/responses_retry.rs:57-176(仅 sleep/事件,无递归)
```

命名更正:worklist 把 `try_run_sampling_request` 称为重试循环;静态上**重试循环在 `run_sampling_request`(turn.rs:1655–1751)**,`try_run_sampling_request` 内部是单次 attempt 的事件消费循环(turn.rs:2629)。

### 2.2 逐循环分析:为什么都是"替换型"(非增长)

Rust 语义:loop 体内 `.await` 的 future 是该次迭代的临时值;状态机对同一挂起点只保存一份,下一次迭代重建并替换。增长型只出现在**递归 await**(async fn 在自身未完成时 await 自身/互相 await)或把旧 future 存进新 future 的组合子里。逐一核对:

| 循环 | 位置 | 内层 future | 替换/增长 | 跨迭代存活物 |
|---|---|---|---|---|
| L1 task 级续轮 | regular.rs:104–124 | `run_turn(...).instrument(...)` | 替换 | `next_input`、`prewarmed_client_session`(数据) |
| L2 turn 内步进 | turn.rs:427–… | turn.rs:500–536 `async {}` 块(内含 L3) | 替换 | `world_state`、`next_step_context`(数据) |
| L3 采样重试 ★ | turn.rs:1655–1751 | `try_run_sampling_request(...)`(turn.rs:1691,match 临时值) | 替换 | `retry_state`、`executed_tool_calls_by_output`、`original_input`(turn.rs:1651–1654,纯堆数据) |
| L4 ws 鉴权恢复 | client.rs:2127–2403 | `websocket_connection(...)`(client.rs:2202,match 临时值) | 替换 | `pending_retry`、`auth_recovery`(数据) |
| L5 事件消费 | turn.rs:2629–… | `stream.next()` 的 `Next` 临时值 | 替换 | `stream`(_owned_ 值,非嵌套 future)、`in_flight: FuturesOrdered<Pin<Box<dyn Future>>>`(turn.rs:2600,元素已装箱) |

上层 guardian 侧的两个重试循环同理:
- `run_with_retry`(ext/guardian-reviewer/src/retry.rs:22–66):`loop { run_attempt(deadline).await }`(retry.rs:48),闭包每次新建 attempt future,替换型。**溢出测试走的就是这条生产路径**:扩展 `review`(ext/guardian-reviewer/src/review.rs:76–106)在 :97–105 以 `Box::pin(crate::run_with_retry(..., |deadline| self.host.attempt(...)))` 包住 host attempt → review_request.rs:140 `run` → :196 `run_guardian_review_session_before_deadline`。即测试中"非法 JSON→第 4 个请求"的重试外层**已经装箱**。
- `run_guardian_review_session_with_retry_before_deadline`(core/src/guardian/review.rs:264–291,`#[cfg(test)]`,经 mod.rs:205 别名供**其他**直接单测使用,非溢出测试路径):把 attempt 包进 `async move { attempt.await }` 后交给 `run_with_retry` —— 仍是每次迭代重建。

`handle_response_stream_error`(responses_retry.rs:57–176)本身无递归:只有 `tokio::time::sleep/sleep_until` 与事件发送;两个提前返回分支(unbounded 连接重试 responses_retry.rs:93–118、传输回退 :120–139)只是让 L3 继续 loop,不嵌套 future。

**因此:静态上找不到任何 await 点会把前一次 attempt 的 future 包进当前 future。** L3 挂起在 turn.rs:1691–1702 时,状态机里保存的是"当前这一次" `TryRunSamplingRequest`(676 行函数的全部状态,FuturesOrdered、prompt、解析器等);重试发生后旧的被 drop、新的替换,尺寸取 max 而非求和。

### 2.3 路径上已有的 Box::pin / 任务边界清单

| 位置 | 内容 |
|---|---|
| guardian/tests.rs:3520 | 测试主 future `block_on(Box::pin(async{...}))`(只箱最外层,poll 链照降) |
| core/src/guardian/review.rs:216 | `Box::pin(run_guardian_review_session(...))` |
| core/src/guardian/review_session_setup.rs:197–203 | `Box::pin(run_review_on_session(...))` |
| ext/guardian-reviewer/src/pool.rs:243、248 | `Box::pin(self.review_ephemeral(...))`(trunk/ephemeral 分叉仲裁) |
| ext/guardian-reviewer/src/review.rs:76–77 | `fn review(...) -> ExtensionFuture` = `Box::pin(async move {...})` |
| ext/guardian-reviewer/src/review.rs:97–105 | 溢出测试实际经过的 `run_with_retry(...)` 调用点已 `Box::pin`(review 级重试外层装箱) |
| core/src/guardian/review_session.rs:364、483、852 | `Box::pin(ensure_guardian_node_repl_policy)` / `Box::pin(async{...})` / `Box::pin(async{...})` |
| ext/guardian-reviewer/src/execution.rs:73–74 | `tokio::pin!(timeout)`(栈 pin,非箱) |
| core/src/stream_events_utils.rs:223–224、360 | `InFlightFuture = Pin<Box<dyn Future>>`,工具 future 已装箱 |
| core/src/client.rs:2708 | `intercept_stream(Box::pin(api_stream), ...)`:ResponseStream 以下已装箱 + :2736 mapper 单独 `tokio::spawn` |
| tasks/mod.rs:404 | 整个 turn 任务 `tokio::spawn`(任务边界;但 current_thread runtime 下仍在同一线程栈上 poll,只切断"父链+子链相加",不给新预算) |
| decision.rs:29–37 + async-utils/src/lib.rs:9 | 生产路径的异步审批在专用 16MiB 线程跑 `decide_approval`(`spawn_approval_decision`);**测试直接内联调用 decide_approval(tests.rs:4207),不走此边界** |

---

## 3. review/guardian 路径上最大的未装箱组合子层级

### 3.1 select!/join!/timeout 清单(按 poll 纵深贡献排序)

| 组合子 | 位置 | 层数 |
|---|---|---|
| **采样连接链(§2.1)** | turn.rs:2582→lib.rs:97 | **约 10 层未装箱 async fn 同栈直降 + timeout(client.rs:1282)+ instrument(turn.rs:2593)+ or_cancel(turn.rs:2594)+ 同步 rustls 尾巴。全程无任何装箱/任务边界 → 最大未装箱纵深** |
| `or_cancel`(= 内嵌 `tokio::select!`,async-utils/src/lib.rs:32–38) | turn.rs:237、486、1734–1735(×2)、2594、2648–2649(×2) | 每处 +1 层,最深处在 L5 事件循环里叠加为 ×2 |
| `tokio::join!` | turn.rs:283–308(两支:上下文记录 + 显示根) | +1 层,双支 future 并联(体积翻倍、深度 +1) |
| `run_before_review_deadline` 三臂 select | ext/guardian-reviewer/src/deadline.rs:12–22 | +1 层;被包裹的 future 均已 `Box::pin`(§2.3) |
| `wait_for_guardian_review` 三臂 select 循环 | ext/guardian-reviewer/src/execution.rs:77–161 | +1 层/次 poll;`runtime.next_event()` 是通道接收,下方浅 |
| `decide` 的 pin+select | ext/guardian-reviewer/src/routing.rs:135–149 | +1 层;decision 为 `tokio::pin!` 栈 pin |
| `tokio::time::timeout` | client.rs:1282(ws 连接);tests.rs:3689(测试侧观察窗) | +1 层 |

### 3.2 结论

review 侧存在大量 Box::pin 与任务/通道边界；run_sampling_request 到同步证书加载是采样观察到的深调用候选。静态调用图不能证明它是唯一大栈路径，也未测出约2MiB下降；需分别量构造/移动、future尺寸、poll栈及真正溢出位置。

guardian 专属的额外注意点:`try_run_sampling_request` 对 guardian 会话还会先走 `prepare_guardian_prompt`(turn.rs:1682–1690),在进入深链之前再叠加一段未装箱 await;但它在深链**上方**,只加常数层。

---

## 4. 溢出测试的 4MiB 线程与并发结构(tests.rs:3508–3543)

### 4.1 结构确认

- tests.rs:3511 `const TEST_STACK_SIZE_BYTES: usize = 4 * 1024 * 1024;`
- tests.rs:3513–3515 `std::thread::Builder::new().stack_size(TEST_STACK_SIZE_BYTES).spawn(...)` —— **显式栈尺寸,`RUST_MIN_STACK` 与 tokio worker 默认均不作用于它**。
- tests.rs:3517–3519 线程内构建 `new_current_thread` runtime;tests.rs:3520 `block_on(Box::pin(async{...}))`。**该 runtime 上所有 `tokio::spawn` 的任务(session loop、reviewer turn 任务、SSE mock 服务端任务、3675 的 trunk review)都在这同一个 4MiB 线程栈上被 poll**——任务边界不提供额外栈预算,只切断父子链相加。
- 并发结构:tests.rs:3614–3623 第一次 review 内联 await;tests.rs:3675–3687 第二次(trunk)review `tokio::spawn`(gated 挂起);tests.rs:3689–3697 `timeout(5s)` + `yield_now` 忙轮询观察服务端请求;tests.rs:3726–3736 第三次(ephemeral fork)review 内联 await,其内部第 3 个请求返回非法 JSON 触发 `run_with_retry` 重试(第 4 个请求,tests.rs:3565–3580 的 mock 序列)。tests.rs:3788–3798 gate 放行后 join trunk review。
- 传输:mock 为 SSE 服务器(tests.rs:3543),但 provider 满足 `supports_websockets`(client.rs:1075–1083)时先走 websocket 连接尝试,失败后回退 HTTP SSE——这解释了 F3 栈底出现在 ws 连接链上。

### 4.2 "16MiB 仍溢出"的证据效力(静态修正)

`RUST_MIN_STACK` 只影响未显式设栈的线程(libtest 测试线程、tokio worker/blocking 线程)。justfile:8、95 设 `RUST_MIN_STACK=8388608`,作用于 libtest 线程;而溢出发生在测试**自建的 4MiB 线程**内。因此 16MiB 受控变体的失败与 8MiB 变体失败测的是同一预算,**不构成"提高预算仍失败⇒增长型"的推断**。增长型假说目前唯一的正面依据是 F5(2MiB 瞬时死亡)+ F1(单跑过、并发死),两者均可用"固定深链 ≈ 2–4MiB + 负载下路径/时机差异"解释,无需增长型。

### 4.3 F5 的正确解读

17个guardian_review普通测试在未设RUST_MIN_STACK的默认测试线程中观察到瞬时SIGABRT，可登记该窗口栈敏感。即使早期发生，也不能据此判定溢出只在poll而非future构造/移动，更不能把2MiB作为单次poll的实测下界；正常4MiB显式线程与逐attempt实验仍需执行。

---

## 5. 候选最小 Box::pin 边界与深度削减点

先声明语义:**`Box::pin` 把子 future 状态移到堆,削减的是父状态机体积(以及跨重试循环时循环挂起态持有的体积);poll 深度仍要穿过 `Pin<Box<F>>::poll` 继续下降,深度不变。** 真正减深度只有两类:同步叶子挪出 poll 链(`spawn_blocking`)、任务边界切断(`tokio::spawn` 换栈起点——current_thread runtime 下同栈,但父链不再与子链相加)。

### 5.1 装箱候选(削体积;按最小侵入排序)

| # | 边界 | 位置 | 理由 |
|---|---|---|---|
| P1 | `client_session.stream(...)` 调用点装箱(`Box::pin(client_session.stream(...)).instrument(...).or_cancel(...)` 或等价) | turn.rs:2582–2595 | L3 循环挂起态不再内联整个连接链状态;每次重试的循环状态 O(1)。受益面:采样、warmup、compact 所有调用方(若在 `ModelClientSession::stream` 内部箱则同效) |
| P2 | `stream_responses_websocket` 内部把 `self.websocket_connection(...)` 装箱 | client.rs:2202–2214 | L4 鉴权恢复循环同理;连接建立是循环里最重的未装箱段 |
| P3 | `connect_websocket`(core)整体装箱或箱 `ApiWebSocketResponsesClient::new(...).connect(...)` | client.rs:1282–1296 | 把 timeout+connect 段从 `stream`/`websocket_connection` 状态机里摘出 |
| P4 | `run_turn` 调用点装箱 | regular.rs:105–114 | L1 循环挂起态 O(1);`run_turn` 是 678 行单体 async fn,体积贡献最大 |

### 5.2 深度削减候选(唯一直接针对 SIGABRT)

| # | 改法 | 位置 | 理由 |
|---|---|---|---|
| D1 | `WebSocketConnector::new`(及其同步证书构建)挪出 async poll 链:构建挪到 `spawn_blocking`(仓库已有同型先例:websocket-client/src/dialer.rs:132–143 对代理 TLS 已用 `spawn_blocking(build_rustls_client_config_with_custom_ca)`,注释明说"keep that work off the Tokio worker"),或把 connector 构建提升到连接层之外复用 | codex-api/src/endpoint/responses_websocket.rs:513 → websocket-client/src/lib.rs:81–97 | 同步 rustls/security_framework 尾巴(F3 栈底)从深链末端移到 blocking 线程(独立 2MiB,且其上无 async 链)。注意 custom_ca.rs:265 的 60s TTL 缓存只去掉了重复钥匙串往返(F4:时间 30.1→15.2s),首次加载仍在调用线程同步执行(custom_ca.rs:253–264 注释自认) |
| D2 | (备选)把整个 `connect_websocket` 作为独立任务 spawn | client.rs:1262 | 切断"采样链+连接链"相加;current_thread runtime 下不给新预算,收益=父链高度 |

### 5.3 顺带登记(静态发现,未列入修复)

- responses_retry.rs:93–118:`Feature::UnboundedConnectionRetries` 开启时 `ConnectionFailed` 的重试**无次数上限**(仅退避+取消)。它不加深 future(仍是 L3 替换型),但意味着"attempt 数"在连接失败场景可以远超 `DEFAULT_STREAM_MAX_RETRIES=5`(model-provider-info/src/lib.rs:64)。worklist 的"~12 次"观察值(F6)若发生在该分支,次数本身不是深度证据。
- 测试内联调用 `decide_approval`(tests.rs:4200–4223)绕过了生产路径的 16MiB 专用审批线程(decision.rs:17–42)。把测试改走该线程属于"提高栈预算",按任务约束**不采纳为修复方向**,仅记录差异。

---

## 6. 验证实验设计(证实/证伪"每次重试加深 future")

原则:**不放宽断言、不提高(已提交测试的)栈预算**;一切探针与放大栈仅存在于一次性 scratch 分支/本地测量副本,提交物零改动。

### 6.1 核心:逐 attempt 栈深探针

1. **attempt 计数已现成**:`ResponsesStreamRetryState.retries`(responses_retry.rs:32–36)在每次进入 L3 时可通过 `handle_response_stream_error`(responses_retry.rs:87、142)观测;在 turn.rs:1691 进入 `try_run_sampling_request` 前打点 `attempt = retry_state.retries + 1`。
2. **叶子深度探针**:在 scratch 副本里包一层 `build_rustls_client_config_with_custom_ca`(websocket-client/src/lib.rs:97 调用点),进入时取 `&probe_local as *const _ as usize` 得栈地址;同时在任务根(tasks/mod.rs:366 `task_future` 首行)取一次基准栈地址。**每 attempt 记录 (attempt, SP_leaf − SP_root)**。SP 差即该次 poll 下降的净高度,是对"future 嵌套深度"的直接度量。
3. **对照组**:同法在 `stream.next()`(turn.rs:2645)取一个浅叶子 SP,用于区分"深链只在连接阶段"与"事件循环也深"。
4. **强制 K 次重试**:复用测试自带的 mock 基建——`StreamingSseChunk { gate }`(tests.rs:3543–3589)可门控响应;再叠加连接级失败(断开/拒绝升级)驱动 L3(Stream 错误→`handle_response_stream_error`→retry)与 `run_with_retry`(非法 JSON→Parse,ext/retry.rs:97)两条重试线。K 扫 1..16。
5. **测量栈放大**:scratch 副本里把 tests.rs:3511 的 4MiB 临时改成 64MiB(仅测量副本;提交物不动、断言不动)——目的是让深链活着走完并吐出探针数据,而不是求绿。
6. **判定规则**:
   - `SP_leaf − SP_root` 对 attempt k≈常数(K 变化也不变)⇒ **增长型证伪**,固定深链+预算问题成立 ⇒ 修复方向转 D1 + P1–P4(优先 D1)。
   - 差值随 k 线性增长 ⇒ 增长型证实 ⇒ 回到 §2.2 找漏掉的嵌套点(静态上目前不存在,需据此重审)。
   - 探针自身开销用"attempt 1 也在探针在场时测"抵消。

### 6.2 零侵入旁证(不改代码)

- 重跑活体采样(同 F3 方法),按 attempt 打时间戳归属样本,比对不同 attempt 的叶子栈地址分布是否随 attempt 上移(深度增加)——采样粒度粗,只作旁证。
- 2MiB 复现箱(F5 场景)下,对 17 个 `guardian_review*` 之一做最小通过预算二分:若最小预算与强制重试次数 K 无关 ⇒ 非增长型的第二证据。

### 6.3 与 worklist 待办的对应

worklist 151 行待做"复查 guardian 采样重试 future 包裹是否线性增长(Box::pin 边界)"——本节即其可执行方案;§2.2 的静态结论预测 6.1 将给出"常数"结果。

### 6.4 开放问题(静态无法回答)

- 为何单跑(t1)在同样 4MiB 下通过、模块 8 并发必死:候选=负载下才进入的最深组合点(ws 连接超时→unbounded/普通重试→在更深上层状态时再入连接链?静态上每条路径深度固定,差异只能来自"哪条路径被走到的频率",需 6.1 的 attempt×路径矩阵数据)、或 security_framework 在争用下自身的 C 栈行为。
- debug/release 差异未测(worklist 全部实测在 debug 测试配置);F5 的 >2MiB 单 poll 足迹含 debug 帧膨胀,release 下数值会显著变小,但结构结论(§2、§3)不变。

---

## 7. 引用索引(文件:行号速查)

- 重试/循环:core/src/session/turn.rs:1655(L3)、1691–1702、1723–1750;client.rs:2127(L4)、2202–2237;tasks/regular.rs:104–124(L1);turn.rs:427(L2);turn.rs:2629(L5);ext/guardian-reviewer/src/retry.rs:47–65;core/src/guardian/review.rs:264–291
- 深链:turn.rs:2582–2595;client.rs:2515–2617、2105–2404、1601–1670、1262–1346、1075–1083;codex-api/src/endpoint/responses_websocket.rs:394–420、499–515;websocket-client/src/lib.rs:81–107(:97 同步证书);http-client/src/custom_ca.rs:246–265、321–337;websocket-client/src/dialer.rs:132–143(spawn_blocking 先例)
- 组合子:async-utils/src/lib.rs:9、16–38(or_cancel);turn.rs:283–308(join!);ext/guardian-reviewer/src/deadline.rs:7–23;ext/guardian-reviewer/src/execution.rs:66–162;ext/guardian-reviewer/src/routing.rs:65–159
- 已有装箱:guardian/tests.rs:3520;core/src/guardian/review.rs:216;review_session_setup.rs:197–203;ext/guardian-reviewer/src/pool.rs:243、248;ext/guardian-reviewer/src/review.rs:76–77、97–105;core/src/guardian/review_session.rs:364、483、852;stream_events_utils.rs:223–224、360;client.rs:2708、2736;client_common.rs:122–136
- 任务/线程:tasks/mod.rs:366–404;session/mod.rs:936–940;guardian/decision.rs:17–42(16MiB 审批线程)、45–133;justfile:8、95
- 测试:guardian/tests.rs:3508–3520(4MiB 线程)、3543–3589(mock+gate)、3614–3623/3675–3687/3689–3697/3726–3736/3788–3798(并发结构)、4200–4223(内联 decide_approval)
- 常量:model-provider-info/src/lib.rs:64(DEFAULT_STREAM_MAX_RETRIES=5);responses_retry.rs:93–118(无上限连接重试分支)
