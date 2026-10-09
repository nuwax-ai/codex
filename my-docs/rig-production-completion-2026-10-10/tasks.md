# Tasks：全部剩余工作的执行清单

日期：2026-10-10。规范/技术方案见 `spec.md` / `plan.md`。**40个执行单元、8个包**；这是本方案拆解，不是40个已复现产品bug。状态初始化为开放，已有小切片不可替代完整DoD。

每项完成证据写 `results.md`：当前HEAD/source/bin、完整命令/feature/target、实际非零执行、首败/复验、skip/timeout/abort、工件路径和未验边界。全部历史身份见 `historical-open-tests.tsv`，逐项追加当前状态，不覆盖旧窗口。

## 包3：确认缺陷与稳定性（优先；14项）

- [ ] **T01 P1：统一历史与当前账目。** 恢复旧机still249/signatures/四日志的完整身份联结；裁决relay #224及registration17/15/18/16；补10-10五套件实际收据。DoD：119每个tuple都有当前执行/阻断，SIGABRT和非383失败另列，原始首败保留。
- [ ] **T02 P1：修CI启动与构建依赖。** 正确Bazel安装Action；三平台glib/gstreamer/pkg-config依赖或经审查的合法target/feature门控。DoD：锁定当前SHA四job均实际进入相应build/test，不以删workspace成员或setup绿代替测试绿。
- [ ] **T03 P1：修native-root无损来源键。** 所有平台Option<OsString> FILE/DIR，缺失/空/非Unicode不合并。DoD：本地受控loader及Windows真实A→B新connector拒旧信新；旧connector不变。
- [ ] **T04 P2：缓存与测试隔离。** 私有实例/模块、锁后时钟、加载后TTL、部分解析/空失败、单飞/毒化/RAII，避免全局fixture竞争。DoD：同进程并发测试+真实TLS，不能只用nextest逐进程PASS证明Bazel隔离。
- [ ] **T05 P1：WS代理错误脱敏。** TransportDefault/NO_PROXY/explicit统一InvalidProxyConfig错误边界。DoD：合成非法密码的raw/decoded值在Display/Debug/上层API/日志均不出现，类别与失败关闭不变。
- [ ] **T06 P2：默认HTTPS环境代理可达。** Typed route进入TLS-to-proxy，保留NO_PROXY/协议优先级。DoD：WS/WSS×HTTP/HTTPS/SOCKS×env/explicit/bypass的实际proxy计数/目标/鉴权及错误矩阵。
- [ ] **T07 P1：系统代理兼容性能实现。** 修订默认matcher参考；手动系统loader与PAC策略分离、有界专职线程/TTL/取消/生命周期、接全构建入口。DoD：逐scheme/逐hop双轨等价，原预算冷/并发测量，未证CFRunLoop因果单列。
- [ ] **T08 P1：R9三线独立归因。** Core #161 T0–T4与SQLite/model-build；OTLP #263–268 collector/export；rmcp #270/#282真实恢复。DoD：各身份独立最小复现、证据支持因果、修复与公共路径回归，不能互相代替。
- [ ] **T09 P1：R10冷首请求。** 找回旧四失败身份，无预热进程测TLS/WS/代理/HTTP IMDS。DoD：原deadline下冷/暖同窗对照，build/connect/handshake/header/body分段，不靠warmup/吞EOF关项。
- [ ] **T10 P1：R11真正栈问题。** 区分future构造/移动/尺寸/poll深度，正常4MiB显式线程及同feature精确集合。DoD：取得真实溢出或受控机制证据，最小修复后单跑/模块并发/公共Core路径通过；不增栈求绿。
- [ ] **T11 P1：R1b有限checkpoint重放。** Runtime grant绑定checkpoint/producer/target/session，不用持久bool或任意scope哨兵。DoD：13身份及sync/async/context modes/pool分支阳性wire有checkpoint，普通resume/非法来源阴性仍降级，历史前缀不改。
- [ ] **T12 P1：Luna post-answer终局。** Gate控制审批/tool-start/早退/generation/cache/采样交错。DoD：合法请求发出并终结，正确拒绝不发，取消/Superseded不丢终局，不增加等待期限。
- [ ] **T13 P1：收口其余历史族与新失败。** 按TSV复验TUI45、app-server18、Core31、exec9、HTTP6、install1、otel6、rmcp2、v8-poc1等（部分与前项重叠，不能求和为新用例总数）。DoD：119每行状态齐全，快照逐项解释，完整首次SIGABRT/新增失败未遗漏。
- [ ] **T14 P1：稳定性公共切片闭环。** proxy/TLS/WS/exec-registry/watcher/Guardian/TUI/Core/Rig复验。DoD：实际执行有请求断言；test-only直连、native fixture和warmup的覆盖边界明确，源与二进制匹配。

## 包1：cap partial / usage（6项）

- [ ] **T15 P1/P0：落实7个兼容决策。** 失败载体/记录/旧reader/硬限/usage口径/API与exec/启用降级。DoD：具体类型/样例diff、旧reader实测、固定预算及opt-in语义；>1k fragment经人工门，无未说明默认迁移。
- [ ] **T16 P1：三协议失败usage。** Responses共用SSE/WS与Rig、Chat迟到usage、Anthropicstart/delta；presence/显式0/未知。DoD：已报告计数不丢，无假成功，完整/部分/缺帧/非法各有真实wire回归。
- [ ] **T17 P1：稳定response reducer。** Host key、累积快照替换、多response/retry/pause、重复/缺失provider ID。DoD：无重复累计，unknown/incomplete不会被后续成功抹掉。
- [ ] **T18 P1/P0：partial双存储读写/resume。** 有界采集/持久化/完整render，typed context fragment及授权分类，sidecar或经实测兼容的新格式。DoD：legacy/paginated/crash/export/archive/delete/两次resume/旧reader/模型切换，截断工具零执行；同会话继续与resume投影一致，无计量证明的诊断保存不能关闭模型恢复验收。
- [ ] **T19 P1：查询API与SDK/UI。** ThreadTokenUsage字段、nullable v2、TS/schema、rawResponseItem/*兼容与snapshot；thread/read/attach/重连/分页重放。DoD：同一历史完整性一致，旧历史缺证未知，旧客户端兼容。
- [ ] **T20 P1：exec失败与端到端闭环。** JSON/SDK快照与turn累计、partial在failed之前。DoD：恰一次失败、零Completed/重采样/截断工具，关闭后无delta，无cap旧行为兼容。

## 包2：D6可信预算（3项）

- [ ] **T21 P0：计量证明。** 官方单位/framing、provider/model/tokenizer版本/证明域。DoD：可复核Exact/ProvenUpperBound；缺证明明确unverified，不能用bytes/4/分位数认证。
- [ ] **T22 P0：适配器与边界。** ASCII/CJK/高熵/JSON/签名/引用/完整framing、缓存失效与计量失败。DoD：完整wire/token阈值、未知来源、20KB/41KB/4次续接/取消回归，超限零下一POST。
- [ ] **T23 P0：显式启用与兼容。** LegacyBytes默认保持，KeepWholeOrFail，未知模型错误明确。DoD：不截断签名/删opaque/改写rollout，拒绝旧内容的模式单独opt-in，人工复审未绕过。

## 包4：owner/行政（4项）

- [ ] **T24 P1：完整CLI配置路由。** 四命令/四环境状态/flags/install/daemon/embedded/remote。DoD：真实owner/RPC、连接前fail-fast、配置字节与机密不回显，不能仅枚举单测。
- [ ] **T25 P1：writer生命周期。** cold/loaded/subscribed/running、resume持锁到unload、held/free校准、PID/window/error。DoD：owner存在不新建writer，lock/实际RPC证据同步，轮询局限公开。
- [ ] **T26 P1：crash/restart/持久队列。** Gate注入owner退出/重启与enqueue/execution。DoD：恢复不丢不重执行，名字歧义/分页稳定，执行失败不把enqueue成功冒称模型成功。
- [ ] **T27 P1：环境接线与并行预算。** 双home/有效SQLite/模型/凭据/协议/retry/idle、CLI覆盖/失活/坏值。DoD：真实HTTP attempts、长短idle观测、取消后无新请求，daemon/config不被污染。

## 包5：字段、来源与历史（4项）

- [ ] **T28 P1：当前字段审计。** 三官方协议与厂商文档、锁定SDK、first-party/native/bridge capability。DoD：每字段保留/转换/拒绝/限制对应源码与测试，不能引用旧Responses转Chat结论。
- [ ] **T29 P1：同源与旋转矩阵。** credential/query/header/endpoint/model/provider/wire逐维度正负。DoD：Core/exec/RPC实际URL/auth/model/attempt与完整块/位置，不以config变化代替wire变化。
- [ ] **T30 P1：旧历史与跨进程。** 无来源/旧来源版本、不同credential-instance、visible/opaque、RawValue大数字与字节。DoD：同源阳性保留，负例合法降级，rollout前缀不变，不猜来源戳。
- [ ] **T31 P1：hosted/工具/流终局。** Signed thinking/search/citation、pause配对与顺序、namespace/parallel/results、迟到/断流/timeout/cancel/cap。DoD：公共请求/事件/工具执行断言，首有效终局不可被覆盖，rawResponseItem/*实验性外部事件兼容另验。

## 包6：平台与异OS（4项）

- [ ] **T32 P1：Linux与容器。** 原生build/test、双进程三wire/预算/凭据/根目录、信任源与信号。DoD：真实镜像/平台/版本记录，交叉编译或Mac mock不替代。
- [ ] **T33 P1：Windows。** 原生build/test、FILE/DIR、路径/锁/skills、Ctrl-C三等待阶段。DoD：控制台事件处理到退出，退出后无attempt，不能用Unix kill代替。
- [ ] **T34 P1：macOS冷态/代理/watch。** 系统proxy受控双轨、TLS冷启动、watch/unwatch/drop/就绪与失败恢复。DoD：低载条件和线程/时序分项记录，保持原deadline，原生实际执行。
- [ ] **T35 P1：异OS app/exec/remote。** Auto_env/runfiles/资源路径与客户端服务端配置。DoD：至少实际两OS组合，owner和执行机身份可核，未具备平台明确阻断。

## 包7：真实厂商（2项，需凭据/费用授权）

- [ ] **T36 P1：限定MiMo/GLM/Step矩阵。** 支持的wire/model×marker/tool/reasoning/cap/usage/error。DoD：当前source/bin新receipt、实际字段和状态，协议不存在记限制；密钥不落工件。
- [ ] **T37 P1：真实触顶与opaque能力。** Responses live cap触顶、signed/citation/search/pause等声明能力。DoD：实际触发语义与读写/恢复闭环；接受cap字段≠触顶，旧收据不复用。

## 包8：最终门禁与发布准备（3项）

- [ ] **T38 P1：workspace/Bazel/Python/三平台CI。** 当前SHA完整集合排除live、retries0；真实Bazel三wire/bridges-out、cache同进程并发、skills canary。DoD：非零执行、首次结果完整、skip/timeout/abort单列；setup/build/test分别绿。
- [ ] **T39 P1：独立验收与提交审计。** Final source/git/dependency/bin/receipt、secrets、旧配置/历史、diff规模与上游merge面。DoD：全部40项有证据/明确平台限制且发布范围已裁决，不能把基线同败当接受。
- [ ] **T40 P1：可发布工件与回滚方案。** npm/平台包安装升级/签名/hash/资源/目录与数据回滚演练。DoD：可审制品及checklist完整；真正push/tag/npm发布等待明确授权，准备完成不冒称已发布。

## 执行规则

失败先最小复现并保留原始原因，再修复；不放宽预算、不批量接受快照、不禁用TLS、不变V8 pin求绿。每批相关just test之后才scoped fix/fmt；下一批新增修改可重新验证，但该批最终fix/fmt后不再test。

授权/平台/凭据缺失不能结束所有工作：标明该项阻断与需什么，继续其余独立任务。最后仍按全范围报告，不能只交本批PASS。
