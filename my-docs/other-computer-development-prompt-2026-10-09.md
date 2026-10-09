# 另一台电脑 Claude Code 工作令（2026-10-09 开发批检查点）

下面正文可直接粘贴。当前源码是用户要求先保存的WIP，尚有已确认P1，不可发布。先修阻塞项，再按文件所有权并行收口；不要继续沿用“全绿/根因已定”的旧结论。

---

请继续改造 nuwax-codex，git@github.com:nuwax-ai/codex.git，分支test。先定位实际路径、记录branch/HEAD/status和新文件。先fetch origin test，干净且可安全快进才同步；保留dirty工作，不reset/clean/stash，不清理SQLite/tmp/config/.env.local/tracked .snap.new。新增WIP包含2026-10-09开发批，至少包含b8cf0ba7b祖先；最新checkpoint以origin/test和git log为准，不强制退回旧SHA。

先完整读根/子AGENTS、my-docs/codex-pending-review-2026-10-09.md、other-computer-validation-results、pkg3-failure-signatures-worklist、claude-code-completion-plan-2026-10-07、Rig spec/plan/tasks、cap具体提案和D6 Spec/Plan。新pending审查优先于旧“完成/全绿”描述。以当前源码、Cargo.lock、实际执行和来源/bin身份为依据。旧电脑日志只作历史证据。

路由契约：第三方responses/chat/anthropic默认Rig分别请求/responses、/chat/completions、/messages；OpenAI/Bedrock默认保留native特殊能力；native/显式bridge/第一方识别按源码。genai是备用，不能以它替代Rig主验收。“Responses转Chat”旧说法过时。

总目标是生产可用和可复核，不只是降低失败数字。下列12包全部登记推进；每包复杂diff<500、普通<800，依赖注册/消费者/测试/锁/schema同批。并行时一个文件只归一个agent，共享core/turn等修改按依赖串行合并。

0. 启动卫生：外部卷先前反复Device error/消失，先确认仓库与git对象可读、目标磁盘稳定和足够空间。不在异常盘上盲清锁/覆盖快照。每个进程隔离CODEX_HOME/SQLITE/target，留源码/bin/hash/feature/平台。先修TLS import：route_aware_tls_fallback_tests无条件use引用macOS-only函数，补相同cfg，Linux/Windows真实编译验证。

1. P1代理契约（agent A，http-client/outbound_proxy/route pool+WS dialer+对应tests）：当前ReqwestDefault对所有IPv4/localhost Direct会绕过HTTP(S)/ALL_PROXY、系统手动与builder显式proxy，且no_proxy随自动redirect扩散到域名。不接受“系统从不代理IP”假设。优先恢复默认TransportDefault并将mock直连留测试构造；若保留生产性能优化，先给具体兼容Spec/Plan，完整保留系统/env/explicit/NO_PROXY优先级，不能只是改成loopback就宣称兼容。IPv6用Url.host enum；public/private/v4/v6/localhost/domain、大小写/尾点、每个redirect hop、proxy auth、Direct/TransportDefault/Proxy、两种Legacy fallback及同步/异步都验证。子进程隔离env，本地proxy与target同时计数，要求实际路径、headers/model/attempt，不修改本进程env。核对取消、预算在client-build前后的效果；不扩大deadline。Direct WS保留Left/nodelay/TLS/解析后loopback-only边界。

2. P1证书生命周期（agent B，custom_ca+独立模块/tests；避免碰A的pool）：LazyLock全局根store不只含平台根，还受SSL_CERT_FILE/SSL_CERT_DIR影响；撤根/文件替换后新connector仍信任旧A，首次空/部分失败永久缓存。先恢复原加载语义，或制定来源键/刷新/失效/错误重试且不降低信任的设计。测试同进程A→B，B可用、旧A拒绝；首次读取失败→恢复；并发初始化与cancel；自定义bundle互不污染。同步系统加载离开Tokio poll线程需真正offload，并保留new-connector刷新契约；只加spawn_blocking/缓存不能声称底层系统阻塞消失。

3. P1 R11真实栈（agent C，core guardian tests/必要采样边界）：该溢出测试自身.stack_size固定4MiB，8/16MiB RUST_MIN_STACK实验不改变它；不能用此佐证增长future。获取实际stack_size、poll栈、future尺寸/嵌套和逐attempt深度证据。采样现在只展示一条sampling链，try_run_sampling_request是循环。受控实际线程栈/并发/冷暖/feature/同窗配对，定位后再给最小Box边界/生命周期修复；不得提高栈或预热求绿、笼统给所有future装箱。最终必须模块并发及公共Core路径通过，单跑不关闭SIGABRT。

4. P1 R1b历史（agent D，guardian review/setup/context/app-server history tests；不要并行碰C的文件）：parent_input_types是最后父请求，review捕获历史是另一时点。关联history/reset版本、checkpoint envelope稳定身份/来源可用性、review attempt、context mode、snapshot/trunk/fork，并在capture/初始化/prompt/transport各阶段记录有界脱敏类型计数。区分策略排除、provenance合法降级、时序、重用与真正丢项。不要猜来源戳/追加opaque包求绿，不改持久化前缀。定位后Spec/Plan+最小修复，覆盖sync/async、resume、兼容/不兼容checkpoint、Independent模式、并行fork、Luna二次采样。

5. R9 SQLite（C/D空闲后独立agent）：sqlx establish/waker pending仍开放。先最小同窗复现，跟踪实际CODEX_SQLITE_HOME、connect/worker/reply/cancel/shutdown，是否阻塞/通道/锁/资源生命周期；仅采样停在flume recv不能定丢唤醒。分别调查R7/R9/AWS，不因共同慢或地址相同就同根。补明确公共失败/取消/恢复回归。

6. 冷/暖TLS与平台系统服务：共享预热仅暖态假设，生产冷首请求另用独立进程真实路径与原预算。SCDynamicStore主线程7ms vs辅助慢没有单独控制CFRunLoop，AWS历史同根未证。精确记录init/build/connect/handshake耗时与attempt、CPU/load仅观察。warm、cold、NativeTls/rustls、HTTP IMDS分别测；139/143的4失败不得归为已知就关闭；不吞空ClientHello，不放宽deadline。

7. 历史119账目与workspace稳定性：原249身份集合固定，新增15标签与原集合/后续PASS按(binary,test_name)联结，输出新增恢复/重复通过/未覆盖/仍开放。119是账面数待独立复算；此前124=115原集合恢复+9重复已过保留。final19=core1+exec16+relay2，不是registration19。包含FAIL/TIMEOUT/SIGABRT/skip/0-match，去Summary重复，交错同窗配对才谈树回归。剩余TUI快照、watcher/网络族、guardian并发逐项收口；别以baseline同败等于可发布。全workspace先完成小批再按AGENTS确认授权，显式排除exec_live/bridge_live、retries0。

8. 行政/多进程/owner全矩阵：补env组active/inactive/incomplete/corrupted/nonunicode/overflow、CLI/env/file/remote owner优先级、cold/loaded/subscribed/running，profile-v2/strict/oss/local-provider/no-daemon/安装方式、archive/unarchive/delete/queue、UUID/name歧义、owner crash/restart。真实writer lock跨resume到unload、coordination/ready/error传播。断言最终model/wire/path/auth/budget/attempt，config字节无污染；进程隔离home与SQLite实际路径，remote配置归属不串用。

9. 三协议字段/历史全矩阵：Responses/Chat/Anthropic public Core、exec和RPC mock；output limits/reasoning/tool definitions/choice/parallel/results、Added/Delta/Done/terminal/迟到帧、cancel/timeout/HTTP/断流/触顶精确区分；首有效terminal不可覆盖，cap无重采样/截断工具零执行。补credential-only、endpoint/query/header/model/wire轮换与legacy缺戳，RawValue字节/精度、encrypted reasoning/signed thinking/server tools/search/citation/pause顺序。阳性完整保留、阴性可见保留opaque降级，旧rollout只追加。compaction辅助入口/schema/Bazel/SDK也覆盖，锁定Rig源码及官方厂商文档核对。

10. cap partial/usage实质功能：7决策点未批准，先交具体类型、状态转换、预算和双history/旧读端兼容方案供裁决，不能凭这份任务书自动启用新语义。准备离线测试可自主做；满足前置后按小批实现三协议presence/显式0、稳定response key去重及多sampling累计、终止帧usage失败载体、legacy/paginated保存/读/resume、unknown跨成功/重连保持、app-server failed/exec turn.failed关闭。fragment在core/context实现trait+matcher/harness/授权分类，模型partial不得当人类授权；全render和serialized caps，delta采集期即有界。不合成成功Completed，不执行截断参数。

11. D6 token-aware实质功能：现40,960-byte兼容限不等于10K-token证明。真实model/tokenizer/framing与全载荷Exact/ProvenUpperBound先于verified；bytes/4、经验分位数、provider整response usage不能作fragment证明。缺证明LegacyBytes/unverified，KeepWholeOrFail，不截签名、不删opaque、不写旧历史。>1k片段P0人工门，>10k禁止，2k候选未批准；改变产品拒绝语义先明确Spec/Plan兼容裁决，未实施不勾选完成。

12. 跨平台/真实厂商/CI发布：Linux/macOS/Windows原生，Ctrl-C/锁/skills root/异OS app-exec-remote；本地mock→供应商限定授权场景→CI→可发布工件/rollback。真实厂商须当次凭据与明确费用授权，CI dispatch/push/发布按对应用户授权；准备具体review结果后再问。密钥/鉴权/敏感原始请求不进报告或commit。不存在的厂商协议列能力限制，不伪造支持；bridge GuardianV2/native WS限制透明记录，不用native fixture冒充Rig能力。

一律just test，Core桥显式rust-rig并非零匹配/真实执行，隔离CARGO_TARGET_DIR，不改sandbox常量/guard；成功输出核对Skipping，必要正常Bazel环境独立执行。Bazel/module/compile_data、依赖锁、ConfigToml/schema、API/TS/SDK相应同步。相关测试后scoped just fix、just fmt、diff-check，之后不再test。测试失败原样留证，不能删失败/改deadline求绿。

每包更新本机基线、完整命令/feature/实际run/pass/fail/skip/timeout/abort、首败/根因/复验、来源/bin身份和剩余项。标清旧机与新机、本地mock与live、切片与完整workspace/CI；不要只交“自查通过”。自主推进上述所有可离线开发和可审方案，不反复因例行可逆动作停下；最后提供小阶段提交与剩余任务。未批准产品语义/付费调用/发布按明确前置处理，不能将已保存WIP当生产完成。
