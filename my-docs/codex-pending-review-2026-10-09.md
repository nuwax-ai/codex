# Claude 开发批待修复检查点：Codex 初步复核（2026-10-09）

用户最终指令为先 git commit/push 当前阶段、再到另一台电脑继续。按此指令，本轮不再展开产品修复或重新运行测试，保存 Claude 的现有源码。**这是 WIP 检查点，不是审查通过或可发布结论。**以下为三个独立审查及变更大小审查的全部已得到意见；后续未完成验证明确保留。

## 基线与保存范围

仓库 `/Volumes/soddygo/git-workspace/fork-nuwax-codex`，分支 test，HEAD/origin/test `b8cf0ba7b69e35c37088c3e8db6dce264e6d9669`；开始10 modified+1 untracked，index空。保存 http-client 六个文件（含新test_warmup）、websocket-client/dialer、四份Claude文档及本次交接文件。根AGENTS、四个code-review子技能已读；不reset/clean/stash，不清理SQLite/tmp/配置/凭据/快照。

本轮仅只读审查与脱敏日志计数，**没有运行Rust测试、fix、fmt、真实厂商或完整workspace，也没有修复以下产品问题**。Claude报告的fix/fmt与测试是前轮执行，不能冒充Codex独立通过。没有依赖或配置类型变更；Bazel新增文件收集需另机核实。

外部卷在只读审查中反复返回No such file/Device error。一条读取tracked astra_kickoff_remote_compaction_windows.snap.new的hash命令失败；重新从/private/tmp使用git -C访问时仓库仍可读，git status未报告该文件变化。未覆盖或恢复该文件，不能声称本轮独立确认其字节完整。不要在异常卷上盲目清理锁、数据或重建目录。

## 全部待处理发现

| #/级别 | 位置 | 触发与影响；下一步 |
|---|---|---|
| 1 P1 | codex-rs/http-client/src/route_aware_tls_fallback_tests.rs:476；test_warmup.rs:9 | 无条件use仅macOS存在的函数；Linux/Windows测试编译会unresolved import。同步cfg，并实际做跨平台编译。源码保存时尚未修。 |
| 2 P1 | codex-rs/http-client/src/outbound_proxy.rs:290、359；:530 | ReqwestDefault对IPv4/localhost无条件Direct，后续no_proxy清除环境、系统与原builder显式代理。数字地址服务和要求经代理的localhost请求绕过用户设置。锁定reqwest0.12.28/hyper-util0.1.20并无“所有IP天然不走代理”契约。最小兼容修复是恢复TransportDefault、测试路径显式直连；生产优化另需完整代理语义方案和真实请求回归，不能只靠registration转绿批准。 |
| 3 P1 | codex-rs/http-client/src/route_aware_client_pool.rs:520、555 | 未托管默认pool仍自动redirect；初始IP/localhost的no_proxy客户端继续直连后续域名，没有逐hop重新选路。恢复默认路由可消除此回归；如保留目的地特殊路由，须逐hop重新决策并保留鉴权/网络策略边界。静态路径已确认，真实回归未跑。 |
| 4 P1 | codex-rs/http-client/src/custom_ca.rs:249、271 | LazyLock永久保留首次信任根。系统撤根或SSL_CERT_FILE/SSL_CERT_DIR变化后新connector仍信任旧根；原生loader不只读取不可变平台证书，自定义bundle重新加载也不会移除缓存A。先恢复原加载语义，或给出来源键、失效/刷新及撤根负控的明确缓存设计。 |
| 5 P2 | codex-rs/http-client/src/custom_ca.rs:253-260 | 首次读取错误仅warn，空/部分store永久缓存；keychain/文件故障恢复后不能自动恢复。不可永久缓存失败/部分失败，需重试/失效；缓存是否接受必须和刷新契约一起判断。 |
| 6 P2 | codex-rs/http-client/src/outbound_proxy.rs:381-383 | Url.host_str的IPv6有方括号，IpAddr解析失败，[::1]仍TransportDefault。解析缺陷已确认，延迟影响未本轮验证。若保留特殊判断用Url.host枚举；先修第2项，不能只补IPv6扩大代理绕过。 |
| 7 P1证据 | my-docs/validation-2026-10-07/pkg3-failure-signatures-worklist.md:147-151；codex-rs/core/src/guardian/tests.rs:3511、3515 | 溢出测试明确创建4MiB线程，RUST_MIN_STACK=8/16MiB不控制它。采样只见一条sampling调用链，turn.rs:1691是重试循环，未证明每次future加深。删除“16MiB仍爆佐证增长”“增长型是根因”“任何全套必abort”；保留显式4MiB线程并发失败、证书阻塞和一次30.140→15.190s观察。2MiB多例失败是栈敏感证据，不是future尺寸直接测量。 |
| 8 P2证据 | my-docs/validation-2026-10-07/pkg3-failure-signatures-worklist.md:90；guardian_v2_history_tests.rs:535；core/src/guardian/review.rs:221 | parent_input_types取断言时最后父请求；review历史在另一捕获点，未关联同一history版本/checkpoint/attempt。snapshot优先、Independent策略、trunk复用与provenance仍可能解释缺项。只能收窄候选，不能定案下游剥离；internal_model_context未读到剥离compaction的代码证据。不得猜来源戳或直接追加opaque envelope求绿。 |
| 9 P2证据 | my-docs/validation-2026-10-07/pkg3-failure-signatures-worklist.md:125 | 六次复现/计时/SCDynamicStore采样支持本机client-build慢路径，不证明与负载无关；主线程7ms未单独控制CFRunLoop。AWS共享地址不够证明历史失败同根；保留测量，因果推断单列。 |
| 10 P2账目 | tasks.md新增最终组合验证段；historical-failures-per-item-labels.md #204-218 | final-verify2实际19=core1+exec-server16+relay2；不是registration19，也不是17+2+1。134-15=119是账面数，本轮未把原249集合与全部恢复记录完整identity联结，不能写独立复算确认。累计115+15与此前重复通过9分开，旧19/19无HTTP/WS覆盖。 |
| 11 P2验收 | route_aware_client_pool_tests.rs:176；test_warmup.rs:1 | warmup复用不验证生产冷首请求，139/143仍有4失败；缺代理/重定向/撤根/失败恢复真实回归。不要增加deadline、吞连接、放松断言或以选择器0-match算通过。 |

WebSocket Direct分支(dialer.rs:58)保留TcpStream未装箱Left、TCP_NODELAY、调用方TLS及loopback_direct解析后回环筛选；当前未发现独立新增传输回归，但它仍受代理绕过和信任缓存问题影响。普通Direct不等于loopback-only。

## Claude日志独立计数（元数据，只读）

日志在兄弟codex-tmp/pkg3-r2。按(binary,test_name)及nextest结果序号去除Summary重复；不输出原请求、stdout/stderr或密钥。

| 日志 | 唯一执行 | PASS | FAIL |
|---|---:|---:|---:|
| final-verify2.log | 19 | 19 | 0 |
| dialer-fix-verify.log | 143 | 139 | 4 |
| regression-run.log | 428 | 419 | 9 |
| cache-regression.log | 143 | 137 | 6 |

原始结果行19/147/437/149不等于执行数；四日志合并512不同身份。final19含guardian1，不能把19都计原249恢复。dialer批HTTP111PASS/4FAIL+ca_env10PASS+WS18PASS；缓存批WS16PASS/2FAIL。regression批exec16、relay2、watcher29PASS/1FAIL、AWS1、HTTP53PASS/7FAIL、network-proxy314PASS/1FAIL、proxy binary4PASS。完整workspace/跨平台/live/CI未由这些切片关闭。

## 大小与阶段

tracked230 changed lines+新增预热约56，共约286，低于500复杂/800总量门槛。代理resolver+pool+dialer与相关断言同批，证书缓存独立，warmup文件+注册+调用同批；文档另批。本次按用户要求保存WIP，不将分commit等同独立验证。具体阶段SHA由git log查看，推送确认在最终回复；不发布tag，不force push。

完整后续并行工作令：other-computer-development-prompt-2026-10-09.md。本文件优先于Claude新文档中的“全部通过/根因已证实/修复完成”表述；那些为待复核的历史陈述。
