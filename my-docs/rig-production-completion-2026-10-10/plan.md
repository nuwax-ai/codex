# Plan：实现路径、依赖和验证设计

日期：2026-10-10。范围见 `spec.md`；每个实施批的基线用实际 HEAD，不能硬退回 `6fea37c4c`。本文件是技术方案，不是已运行的验证记录。

## 1. 依赖顺序与并行所有权

执行：**包3安全缺陷/CI准备 → 包3 R1b与时序 → 包5来源/包4 owner → 包1 cap → 包2 D6 → 包6平台 → 包8离线/CI → 包7 live → 包8发布准备**。

- 缓存组拥有 http-client 私有缓存及其测试；WS组拥有 websocket-client；guardian组拥有 checkpoint/projection 相关文件，先协商 core/client.rs 所有权。
- owner组拥有行政/queue 生命周期；usage组拥有失败载体/usage reducer/消费方，不与 guardian 组同时编辑 core/client.rs。
- CI/账目组拥有 workflows、验证脚本、任务证据。不同平台 worker 使用独立 target/home/SQLite。
- 并行人员明确“不是唯一开发者”，不撤销他人改动。重叠文件顺序实施或明确交接，注册/调用方/schema/BUILD 不遗漏。

## 2. 包3：先解决确认的安全与稳定性缺陷

### 2.1 native roots 缓存（T03/T04）

从 custom_ca.rs 提取私有 native_roots_cache 模块，原 inline 老测试保持，新增测试放独立 *_tests.rs。生产只持私有 cache instance，测试可创建独立实例，避免 fixture 改全局缓存。

- key 是结构化 `Option<OsString>` FILE/DIR 元组，在所有平台读取 `var_os`；缺失与空值不折叠，不使用带分隔符的字符串拼接。以锁定 rustls-native-certs 的输入为参考。
- 在取得锁后读时钟检查有效性，加载结束后计算 expires_at。注入时钟与 loader，验证等待锁超过 TTL 的情况。
- 明确空加载不缓存/清除该失败来源的条目；loader errors 或 add_parsable 的 ignored>0 使用短 TTL。bundle 仅扩展私有 clone。
- 锁毒化返回结构化错误或经明确策略清空后重载，不能 panic；日志不输出环境值。测试恢复用 RAII，不能把 wrapper 的非重入锁再放进 loader 造成死锁。
- unit：缺失/空/两种非 Unicode、FILE/DIR 切换、同路径替换、清洁/部分/空失败恢复、并发单飞、锁等待/毒化。wire：A→B，新 connector 拒 A/信 B，旧 connector 不变，bundle 不污染；Windows 真实环境变量/路径必须另验。

### 2.2 WS 代理安全与支持（T05/T06）

- 在所有 tungstenite 错误返回边界统一处理 InvalidProxyConfig，保留类别，把敏感 payload 换为固定占位；TransportDefault、Proxy+NO_PROXY、显式路径均覆盖 Display、Debug、上层 API 及日志。
- 子进程设置合成密码尾 `%` / 非 UTF-8百分号编码，断言 raw/decoded 密码均不存在于任何输出；有效密码和407/连接/TLS错误对照避免过度改写类别。
- 默认环境选出的 `https://proxy` 交给现有 TLS-to-proxy 路径。优先修清晰的类型化 route 决策，不能等 UnsupportedProxyScheme 后盲目重发或把全部错误吞掉。
- 明确 HTTP(S)_PROXY、ALL_PROXY、NO_PROXY 在 ws/wss 的锁定 SDK 规则；用 HTTP/HTTPS/SOCKS × env/explicit × bypass × IP/域名/IPv6 的本地代理矩阵断言目标、握手、auth和真实次数。

### 2.3 macOS 默认代理兼容优化（T07）

先纠正旧 SCDynamicStore Spec，建立锁定 reqwest0.12/hyper-util0.1 与 Rig reqwest0.13 的两份实际参考，不能假定版本规则相同。

- 默认 hyper-util先读手动系统HTTP/HTTPS，再为缺失的逐scheme env值补充，ALL_PROXY最后fallback；即使env已设，当前macOS实现也会先创建store。明确空值、大小写、CGI、NO_PROXY和显式builder顺序。
- 默认模式不执行PAC/CFNetwork目标例外；RespectSystemProxy现有PAC行为独立保留。不得直接复用macos::resolve改变默认语义。
- 为默认模式实现专门的手动系统设置 loader。专职线程/runloop是待验证实现选择：需真实服务runloop并用source/timer唤醒，不能先CFRunLoopRun把接收请求的代码永远挡住。
- 队列有界、并发查询合并、加载单飞、正/负TTL、退出和失败恢复明确；取消调用者不取消共享刷新，失联线程不得无限重建。回调内不持会被调用者重入的锁。
- 传输保留逐hop路由：不能把某个起点的Proxy固定到整client并丢后续NO_PROXY。尽量使用SDK可用的custom matcher/callback与完整环境快照；若必须重定向接管，复用现有route-aware重定向安全/鉴权规则。
- 跨reqwest版本用私有中立 route 类型分开实现adapter，不跨版本透传client/header类型。HTTP、Rig三wire、普通WS均查清构建入口，优化不能只覆盖pool或https-proxy分支。
- 双轨对照基准是未优化的锁定默认client：手动/PAC-only/系统例外、半设env、ALL_PROXY、显式builder、NO_PROXY与redirect。记录真实代理计数/目标和无泄漏。CFRunLoop因果用同线程/负载条件实验，性能比较不扩大原deadline。

### 2.4 R9/R10/R11 与历史族（T08–T14）

R9分 **Core retry #161、OTLP #263–268、rmcp #270/#282** 三条线；不共享未证明的根因。

- Core T0 build进入、T1 state init完成、T2 submit、T3 model client构建开始/完成、T4首次HTTP/telemetry；SQLite establish、pool acquire、migration/PRAGMA另计，模型build独立计。仅写有界时长/阶段/attempt，不记录prompt/凭据。
- OTLP测collector bind/readiness、连接、export、flush/shutdown和runtime；rmcp分别断言恢复请求/会话身份/重试次数。Tracing没有写SQLite不代表其他启动车道没有SQLite。
- R10找回四个旧失败身份；无预热独立进程对比native-tls/rustls、HTTP/HTTPS代理、WS和纯HTTP IMDS，分build/connect/handshake/header/body。mock关闭/EOF/超时保持原错误语义。
- R11记录future尺寸、构造/移动与poll栈分别的证据；显式4MiB线程、同feature图/精确集合、单跑/并发、冷/暖交错。正常Rust接口与profiler优先；不新增unsafe测地址。Box::pin只解决状态体积，spawn任务或offload才可能截断poll深度，需单变量验证。
- 优先检查WebSocketConnector::new同步根加载及Core/Rig client急切构建；保留线程/超时预算。一次单跑PASS不能删除SIGABRT，暖态成功不能关闭冷态。
- TUI45按reconnect/启动默认/lifecycle/resume/history/guardian快照/动态工具分组；app-server剩余account/residency/detached-review/derive、Core approvals/MCP/realtime/remote-env/parallel/unified-exec、exec9、install1、v8-poc1分别建立最小复现。计数以TSV完整身份为准。

### 2.5 R1b bounded replay 与 Luna（T11/T12）

先用同一 history version/reset/checkpoint/attempt/context mode 关联种子、历史、prompt和最终transport，只采类型/计数/有界ID。最后父请求不能代表所有review attempt的历史版本。

许可放在可信运行时请求上下文，不从rollout里一个bool授予。包含checkpoint稳定ID/内容身份、producer来源、guardian目标和允许的模型变换，期限限当前review/session；用户可编辑历史不得伪造许可。批准仅允许预定reviewer模型与固定可信guardian header变换；必须证明实际凭据、endpoint/query和wire具有合法对应，不能从名称猜。producer与target的auth_domain可因该头不同，不能仍强制二者相等；绑定target后任何未批准的凭据/endpoint/header/wire变化均拒绝。匿名/缺证来源不能被补成account。普通reasoning规则不放松。

先单独实现最小许可与projection消费，再回归13个历史身份及Legacy/ThreadOwned/Independent、trunk/busy-fork、同步/异步和旧rollout；阳性wire保留checkpoint，阴性消失且可见内容不丢。

Luna用受控gate覆盖审批、tool-start、score_tool早退、generation替换/Superseded、缓存、checkpoint失败和发请求交错；同步deny后的无请求是正确状态，真正的未发/未终局另修。

## 3. 包1：cap完整实现（T15–T20）

先落成7个决策：失败载体、存储表示、旧读端策略、fragment预算、usage口径、API/exec兼容、启用/降级。现有提案只是候选，不直接复制未验证类型。

推荐实现合同：默认legacy行为保留；新增typed opt-in。partial与usage独立失败记录，host response key稳定且持久化，未知计数保持Option/presence；不生成成功item/Completed。旧usage数值口径保留，新增已知小计与completeness，不把整turn算成最后response。

存储优先评估与rollout关联的版本化failure sidecar，使旧reader继续读原rollout并明确忽略新能力；若采用新event/item，必须逐个旧reader证明兼容后才写。两条路线只能选一条明确实现，写入由既有owner单writer负责；crash、导出/迁移、删除/归档与resume的sidecar/rollout一致性都纳入验收。

预算推荐每fragment完整render≤1k verified tokens、最多8片段、turn完整render≤8k verified tokens，并独立限制UTF-8 bytes、序列化bytes与累计采集bytes。具体byte数在T15固定并加入schema/边界测试；计量没有证明时可有界保存诊断但不注入模型。不能用整response usage给局部片段定硬限。诊断保存本身不能关闭模型resume可见partial验收；同会话继续与resume投影须一致。若需要>1k单片段，先交P0人工复审。

实施顺序：共用ReportedUsage/CapFailure与consumer迁移 → 三wire实际提取 → 稳定reducer → 有界partial与存储/read/resume → ThreadTokenUsage v2/schema/TS → exec/SDK/UI。新概念优先私有模块或合适独立crate，Core只编排；加crate/dependency同步Bazel lock。

验收：有/无/部分/显式0/非法usage、Chat finish后usage、Anthropic start/delta、Responses SSE/WS与Rig；多response/retry/pause/provider ID重复；legacy/paginated、两次resume、旧二进制、crash恢复；终局前partial，恰一次failed，零重采样/截断工具执行，关闭后无delta。

## 4. 包2：D6（T21–T23）

明确计量对象是整message/block/完整请求中的哪一项及framing；tokenizer/model映射与版本/证明域作为证据对象，不能只用模型名称猜。

做Exact/ProvenUpperBound adapter与独立边界测试，结果含verified/unverified及理由；未知/异常fail-fast或明确LegacyBytes，禁止自动fallback后声称verified。保留40960与4次续接及KeepWholeOrFail。只在显式选择新模式后拒绝旧可发送内容。

ASCII/CJK/高熵/JSON/签名/密文/citation、wrapper/ID/framing全部计入；超限零下一POST，同源完整字节不变。没有厂商可核计量契约的组合登记限制，不能宣布全模型硬限证明完成。

## 5. 包4/5：公共矩阵（T24–T31）

- 四命令 × active/inactive/incomplete/corrupted × UUID/名字/歧义/分页；flags覆盖no-daemon、strict、profile、oss、显式provider/local，安装来源按源码枚举。
- daemon存在/缺失、embedded/remote、cold/loaded/subscribed/running；通过真实RPC和writer锁生命周期观测，probe正负校准且遵守coordination，不另开writer。owner kill/restart、队列持久化和配置字节/SQL根隔离都断言。
- 双进程不同auth/model/protocol/retry/idle，实际服务端断言；短/长idle要有真实等待差异，立即SSE完成不能证明。
- credential-only同源正负、query/header/endpoint/model/provider/wire旋转、跨进程及legacy缺证，在Core/exec/RPC各有代表。same-source保完整opaque；negative保visible并降级opaque，rollout原前缀不变。
- 工具namespaces、signed thinking/search/citation、hosted工具/pause、encrypted reasoning、大数字RawValue与三协议terminal/cancel完整覆盖；字段审计用官方当前文档和锁定SDK，注明支持能力/协议限制。

## 6. 包6/7/8：平台、live、最终门禁（T32–T40）

CI已知先修setup：Bazel Action仓库不存在；Windows缺pkg-config/glib，Ubuntu缺glib，macOS缺gstreamer。核Cargo target/feature图及官方依赖安装流程，补正确平台依赖或合法feature/target门控，不能随意删除成员求绿。Action选择实际存在的官方仓库并固定可核ref。

Linux验证容器双进程、信任源、信号、home/SQLite；Windows原生Ctrl-C覆盖Retry-After/首SSE/流中，并观察退出后无attempt；三平台watcher install/unwatch/drop/失败恢复、默认代理、锁和skills根canary都实际跑。

异OS app-server/exec-server用remote-tests的auto_env与runfiles资源定位，分别断言客户端/服务端配置归属。

相关套件绿后完整workspace排除exec_live/bridge_live、retries0；Bazel实际build/test三wire、bridges-out拒绝、同进程cache并发及Python gate/parser负控。0匹配、cache复用、Skipping不可称请求验收；不修改sandbox常量/guard。

live只在对应凭据/费用授权下运行MiMo/GLM/Step支持矩阵，先marker+tool再reasoning/cap/usage/error，再真实Responses触顶与encrypted引用。source digest、HEAD、依赖图、同一exec SHA/receipt逐批绑定；不支持端点明确限制，不扩大paid压力。

发布只交可审checklist/脚本：npm/原生包安装升级、平台资源、默认目录、配置/历史迁移、签名/制品hash、回滚、无秘密。push/CI dispatch/tag/npm发布是另一个明确授权步骤。

## 7. 每批证据和提交

完整命令、target/features、source/git/bin身份、每个(binary,test_name)首败/复验和PASS/FAIL/SKIP/TIMEOUT/ABORT记录到本目录results.md及ledger。旧机工件缺失列缺失，不用新通过伪造旧根因。

每批先相关just test，Core桥显式rust-rig，最后scoped just fix、just fmt、diff-check；该批最后fix/fmt后不再test。测试/消费方/schema/BUILD形成同批完整依赖；复杂<500、普通<800，机械拆分标注无行为变化。
