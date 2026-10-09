# 独立复审：六批提交与全部后续范围

日期：2026-10-10。仓库 `nuwax-ai/codex`，分支 `test`；基线 `6fea37c4c9384b52e7fbfb2055c6506c74e7f1f9`。主要 diff：`877b02758..6fea37c4c`，必要处回查 WIP `56606963b`。开始工作区仅用户已有 `.gitignore` 修改。

本轮使用 code-review 的 compatibility/context/testing/change-size 四子技能独立复审。**未运行 Rust 测试、完整workspace、Bazel或live；未修改产品代码。**本轮提交审查、纠正文档、完整Spec/Plan/40项Task、119身份账目和runtime忽略规则。下面“确认”是源码/数据核对结论，不能称这些问题已经修复。

## 1. 当前结论

- 三协议Rig主线已实现；六批不是完成全量生产验收。仍有 **2功能包+6稳定性/验收包**，见Spec表。设计提案和静态定位不能算实现。
- HTTP `ReqwestDefault→TransportDefault` 恢复消除了此前字面量目的地no_proxy及重定向扩散回归；cfg import修复与定义一致。WS显式Direct的TLS/回环解析/TCP_NODELAY未发现本批新增回归。
- 证书缓存来源在Windows/非Unicode上仍有缺陷，WS默认代理的密码错误边界有现存缺陷；默认HTTPS环境代理有现存兼容缺口。
- R1b、R9、R10、R11均未完成实现/动态归因；本次新调查中还有不宜直接实施的方案和过强证据措辞。

## 2. 全部确认发现（位置为本轮修改前）

1. **F01 P1：Windows信任来源键不完整。** `codex-rs/http-client/src/custom_ca.rs:303–314` 非Unix固定platform，但锁定rustls-native-certs0.8.3 `src/lib.rs:118–124,198–205` 所有平台先读SSL_CERT_FILE/DIR。切换A→B后TTL内新connector仍信A/拒B；自定义bundle不会移除A。T03处理，真实Windows验证尚缺。
2. **F02 P2：Unix来源键有损/有碰撞。** 同文件`:306–309,564` 用String，缺失与空合并，非UTF8经env::var变缺失；loader实际var_os。两种非Unicode目录可共享key，字符串分隔符还不是结构化身份。T03统一Option<OsString>，测试missing/empty/path碰撞。
3. **F03 P1（现存）：默认WS代理错误泄露密码。** `websocket-client/src/dialer.rs:46–55,104` 直接返回tungstenite错误；锁定`tungstenite/proxy.rs:249–266` 将无效百分号/非UTF8密码放进InvalidProxyConfig，`codex-api/src/endpoint/responses_websocket.rs:523,591` 再记录/返回。显式parse已redacted，default/NO_PROXY路径未统一。T05保类别并脱敏所有边界。
4. **F04 P2（现存）：默认HTTPS环境代理未支持。** `dialer.rs:49` TransportDefault进入tokio-tungstenite `connect.rs:110`；tungstenite `proxy.rs:197` 不接受https，直接UnsupportedProxyScheme，本地TLS-to-proxy实现不可达。T06补真实env WS/WSS路径，不用重试吞错误。
5. **F05 P2：缓存fixture同进程竞争。** `custom_ca.rs:280–293` wrapper只串行自身；`:950,963` 真实root调用没共锁，可覆盖/继承fixture；observed panic也不恢复。nextest逐用例进程不检验Bazel同进程问题。T04优先私有cache实例；禁止在loader再取得wrapper非重入锁。
6. **F06 P2（设计）：默认代理Spec与SDK不等价。** `my-docs/scdynamicstore-compat-spec-2026-10-10.md:10,38,43` 原称env已设跳过系统并复用macos::resolve。hyper-util0.1.20 `matcher.rs:588–613` 先读系统、逐scheme补缺失env；`:313–317` ALL_PROXY最后fallback。现有`outbound_proxy/macos.rs:97–103,221–247`执行PAC/CFNetwork例外，默认matcher只手动设置。必须修订参考和独立loader；本轮纠正文档，T07实现尚未做。
7. **F07 P1（设计）：R1b任意scope哨兵扩大重放授权。** `r1b-delivery-strip-point-analysis-2026-10-10.md:109–112` 持久guardian_replay bool和任意scope许可可扩散到普通resume/跨endpoint/auth/wire；`history/src/lib.rs:128` metadata可序列化。本轮改为未实施的受信请求级bounded grant要求；T11实际实现及13身份回归仍开放。
8. **F08 P2：采样充要条件错误。** R1b分析`:136` 将无tool-start写为二次采样不发的充要条件，但`ext/guardian-v2/src/async_scorer/observation.rs:45–53,222–249` tool-start后仍可早退。本轮收窄到入口/后续条件，T12动态竞态未解决。父producer必为account也只能在有相应凭据的场景成立。
9. **F09 P2：R11静态推断升级为栈测量。** `r11-future-depth-static-analysis-2026-10-10.md:32,141,163` 没量future构造/移动/poll却声称唯一~2MiB路径。`core/src/guardian/tests.rs:3511–3515`固定4MiB，RUST_MIN_STACK16MiB不影响它。循环替换事实不能证明实际溢出位置；worklist旧“增长型/全套必abort/深future主因”保持历史候选，本轮纠正新的定案措辞；T10真实实验仍缺。
10. **F10 P2：R9家族错联。** `r9-sqlite-establish-static-analysis-2026-10-10.md:57` 把其余5项称同文件；历史#263–268实际为OTLP，Core retry #161和rmcp #270/#282另算。本轮纠正，T08三线分别复现，不能共享SCDynamicStore归因。
11. **F11 P2：新批收据未齐。** `claude-p1-fixes-2026-10-10.md:49–57` 缺完整命令/target/features/source/bin/log引用。可读p1-http-client-tests.log仅79run/76PASS/3FAIL/56skipped；不能独立支持最终135。app-server-transport尾支持157/157、0skipped、exit0，但报告仍写后台。musl日志明确openssl构建失败，尾exit0不能算编译过。缺工件不证明Claude没执行；T01补证，本轮修文案边界。
12. **F12 P1验收：当前CI确实未过。** [run37967668519](https://github.com/nuwax-ai/codex/actions/runs/37967668519)，对应本轮HEAD：四job均completed/failure。Ubuntu Bazel setup找不到`bazel-contrib/setup-bazelisk`；Windows glib-sys缺pkg-config，Ubuntu缺glib-2.0，macOS缺gstreamer-1.0。均不能作为三平台编译/测试通过；T02修准备、T38最终实跑。
13. **F13 P2可审规模：两批过大/文件继续增长。** TLS提交647changed（实现+unit384，真实TLS263），超过复杂500；文档963超过800。custom_ca.rs836→1156，测试前主体至854；原inline模块不是新模块规则的直接违规，但新增功能应拆私有模块。已提交历史不重写，T04及新阶段按依赖拆分。
14. **F14 P0既有上下文门仍开放。** `rig-bridge/src/stream.rs:45,267` 的40960-byte限制及`stream_pump.rs:25`的4次续接不证明10K-token；cap具体提案`:172`的2000-token fragment跨>1k人工线，且尚未实现/裁决。本次无新模型可见fragment，T15/T18/T21–23不得靠普通byte单测关闭人工门。

## 3. 待实验/合同确定的疑点（不当作已复现新bug）

- Q01：`custom_ca.rs:332` 用锁外旧now判TTL，超长锁等待可能接受实际已过期条目；用受控时钟/锁等待证明，T04处理。
- Q02：ignored>0/errors空仍给60s；明确是否按部分失败5s处理并测试，T04处理。
- Q03：生产cache锁毒化分支调用panic；异常策略应结构化失败或明确恢复，不能在用户进程产生panic。T04检验策略/错误传播。
- Q04：主线程7ms/辅助线程12.28s定位慢路径，不单独证明CFRunLoop是原因；AWS纯HTTP与TLS冷因果、R9与R7共同根因仍需实验。

## 4. 119账目与证据边界

独立解析383个唯一身份；原t2仍败249，其中恢复标记130，未标119（119唯一tuple）。累计标签139含此前已过9，不是249恢复139。R1b13不计恢复；relay#224和registration17/15/18/16仍需旧机工件联结。

| binary | 历史未标恢复 |
|---|---:|
| app-server（unit+all） | 18 |
| core（unit+all） | 31 |
| exec-server（unit+accepted_websocket+relay） | 9 |
| http-client | 6 |
| install-context | 1 |
| otel | 6 |
| rmcp（两个binary） | 2 |
| tui | 45 |
| v8-poc | 1 |
| 合计 | 119 |

源：`validation-2026-10-06-round2/historical-failures-per-item-labels.md`，120097bytes，SHA256 `28ceb8dede2597a1b1d7efd35991cfe5887a413606561958fec63e451e11e730`。TSV保留编号/binary/test/旧状态，当前状态统一needs_current_evidence；HTTP六项已有可读新窗口PASS，不能再叫当前失败119。SIGABRT/非383新增失败单列。

Claude自报新机135/20/616/10/157套件通过；本轮仅从可读日志独立确认157/157和http首轮76/79，未复跑，所以不发布Codex独立“全绿”结论。正常Bazel/完整workspace/live仍未由本轮验证。

六批精确规模：1855新增/81删除=1936changed，代码973/文档963。每批185、647、122、13、6、963。阶段拆分建议和依赖详见Plan。

## 5. 本轮落盘与下一步

- 整理已有.gitignore：覆盖整个codex-rs/tmp，不固定随机arg0路径，保留用户SQLite及sidecar忽略意图；无运行时数据清理。
- 纠正未实施的SCDynamicStore/R1b方案与R9/R11证据措辞；历史原观察保留、当前验收边界补明。
- 新建独立Spec/Plan/Task/Claude提示词，全部40项和119完整身份；测试与生产修复由下轮按任务实施。本轮不自动push/发布，不读取.env.local。

最终本地commit与format/diff-check结果以本次最终回执为准。另一台电脑使用此方案前需要同步本目录所属新提交；旧origin/test只有6fea37c4c不能读取尚未推送的新方案。

## 6. 本轮实际检查

- 40个T编号唯一且完整，119个TSV完整身份唯一，全部current_state为needs_current_evidence；核对属于数据审查，不是产品测试。
- `just fmt` 首次因沙箱无法写uv缓存失败；取得宿主缓存访问后重跑exit0。实际工作区未出现codex-rs/scripts/sdk/.github源码改动；`git diff --check`通过。
- `git check-ignore --no-index`确认不同随机arg0路径、SQLite及WAL被runtime规则覆盖。无运行时文件清理。
- 六份旧文档修订与六份新交付合计约630changed，低于800；`.gitignore`单独提交。新文档全部明确未实施、未运行Rust验收，未来每批按Plan独立验证。
