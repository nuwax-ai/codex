# Codex 独立复审与阶段修复（2026-10-11）

基线：`test` / `070098774`，工作区开始无未提交改动。主要审查 `303ad8960..070098774`；回查 `877b02758` 等必要祖先。四个 code-review 子技能分别检查兼容/上下文/测试/规模。原53/55提交均为历史检查点，当前修复是否通过以本报告末尾实际验证为准。

## 1. 确认发现与本轮处理

位置指审查基线；代码修复均需相关测试，不能由历史绿灯代替。

| 编号/级别 | 位置 | 问题与处理 |
|---|---|---|
| F01 P1 | codex-rs/core/src/model_output_projection.rs:243–254 | Compaction只比较auth_domain_kind，普通resume跨账号/API key/私有query或header仍发旧密文。补实际auth_domain相等；同授权域模型切换保留，跨进程credentialInstance缺证仍降级。 |
| F02 P1 | codex-rs/core/src/guardian/replay_grant.rs:90–95 | grant仅比较kind，共享可变AuthManager不证明凭据连续。补剥固定reviewer头后的basis与producer实际域相等、已知来源/可信kind；新增同kind轮换负控；客户端遇到已要求的grant不能验证时提前失败，禁止缺必要上下文继续审批。账号重启fixture补固定account/user claims。 |
| F03 P1 | codex-rs/codex-rust-rig-bridge/src/convert_response.rs:282；stream_pump.rs:250–257 | cap普通flush发FunctionCall/CustomToolCall Done，Core错误后仍drain工具队列；合法JSON可执行。移除cap flush接线，保typed错误及已显示delta；补Chat合法工具及Anthropic已完成block的公共零执行回归。 |
| F04 P0人工上下文门 | convert_response.rs:214–235；codex-rs/core/src/context_manager/history.rs:533–595 | partial升级为无截断标记/硬限的普通Message/Reasoning历史，随后进模型；dead诊断构造器无法保护。恢复cap不新增Done的安全默认，durable typedpartial重新列为未实现，不冒称闭环。 |
| F05 P2 | codex-rs/core/src/state/session.rs:270–281 | 旧response转移进新turn时误减其他response的usage。保存每response的完整record及turn归属，只有原贡献在当前turn才减；保持现有“全snapshot转移”语义。新增B=10/A重报45及rollout/resume公共回归；stable host key仍开放。 |
| F06 P2 | codex-rs/codex-rust-rig-bridge/src/usage.rs:37–61；convert_response.rs usage helper；codex-rs/codex-api/src/sse/responses.rs:173 | 缺thinking子字段被写成0、u64强转负值、负usage被接受；归一化派生total还能误标完整报告。保presence/显式0、checked转换、拒无效计数，SDK fallback total保持未知；原始Chat usage仍待接线。 |
| F07 P2 dormant | codex-rs/core/src/session/cap_partial.rs:126–169 | 最后prefix未计total_bytes，无ID等长片段key碰撞。补精确字节与位置key及回归；构造器仍未用于生产发射，也不提供token证明。 |
| F08 P1 | codex-rs/http-client/src/outbound_proxy/default_proxy_matcher.rs:106–116 | 生产构造遗漏CGI REQUEST_METHOD防护。共用raw presence构造，补empty/Unicode/nonUnicode子进程负控。 |
| F09 P1 | codex-rs/http-client/src/outbound_proxy.rs:337–369 | ResolvedDefault读RespectSystemProxy的system-first缓存，覆盖env/NO_PROXY。新策略不读该缓存。 |
| F10 P1 | codex-rs/http-client/src/outbound_proxy.rs:456–459及matcher非HTTP分支 | Windows系统字段遗漏，WS/WSS变Direct绕过SDK代理。两者委托TransportDefault；SOCKS/特殊凭据同样委托，未优化不等于不支持。 |
| F11 P1 | codex-rs/http-client/src/outbound_proxy/macos/system_settings_store.rs:413–424 | 空RunLoop立即Finished再recv_timeout(0)，常驻busy-spin。Finished分支正等待，新增真实空闲循环回归。依据Apple RunLoop规范；不以此证明CFRunLoop能消除全部冷态耗时。 |
| F12 P2 | codex-rs/http-client/src/native_roots_cache.rs:124–131；system_settings_store.rs:369–374 | poison恢复未clear，后续不停重载；spawn失败却进入Loading，永久等待。清poison并验证TTL复用；spawn失败清sender进入Failed短TTL。spawn OS失败注入仍待补。 |
| F13 P2 | codex-rs/http-client/src/outbound_proxy/default_proxy_matcher.rs:152–202 | 多@/空userinfo经Url再解析改变参考SDK认证语义。特殊输入委托原transport；普通Basic Auth仍缺真实header双轨断言，不能叫逐字段认证等价。 |
| F14 P1 | codex-rs/http-client/src/client_builder.rs:177–188 | unmanaged公开Factory固定起点route，自动redirect及复用到其他URL丢NO_PROXY/env规则。ResolvedDefault进入已有RouteAwareClientPool；内部resolved builder不递归。新增公开入口双向redirect/复用四格真实wire回归。 |
| F15 P1验收 | .github/workflows/fork-cargo-pr.yml:95–97,156 | 排除code-mode-host却选需其二进制的tests，产生missing binary。未伪造host或删tests求绿；由下轮补配对V8/host工件及消费者集合一致性。 |
| F16 P1证据 | results.md:171–175；handoff:26–31 | 剩余CI全族定性被日志反驳：23旧BOUNDARY在mac CI全PASS；缺host有大量明确失败；Root SIGABRT也存在。撤错误终局归因，按真实状态/身份生成failure ledger。 |
| F17 P2账目 | current-test-ledger.tsv:42/121,78/122,79/123,124,126 | 三组别名重复、两条聚合不是identity，119与126口径混用。保旧126记录，加canonical映射与新CI窗口；126=119历史+3alias+2summary+2new，121完整canonical。 |
| F18 P2测试 | core/tests/suite/rig_output_cap.rs:156–162；state/session_tests.rs:56,128 | None可过的optional断言不证明durable；替换ledger缺公共集成。修复后cap强制零完成项；补真实重复快照/rollout/resume回归。T18真正durable仍需新集成，而非恢复危险flush。 |
| F19 P2证据 | otel/tests/suite/otlp_http_loopback.rs:365；results.md:212 | 替代fixture用RespectSystemProxy，生产默认仍ReqwestDefault。收窄为该fixture通过，默认迁移/冷态闭环未做。 |
| F20 P2可审规模 | 489c43c85/081b6b0b6/5ac058769等 | 本批净7125行，7批>500、3批>800；测试计入阈值。旧历史不改写，本轮修复按功能/消费方/回归的小批保存。 |

已发现但不属于本轮新增：IPv6 HTTPS代理SNI括号路径；本轮不宣称修复。后续归入WS协议矩阵。

## 2. CI与账目准确状态

run `38065399719` 实际SHA **e02dc3771**，不是bac940d34；Bazel job成功，三nextest失败。T02仅“进入build/test”达成；完整T38未达。车道排除了voice/V8相关成员，不能称完整无排除workspace。

| 平台 | 执行 | PASS | FAIL桶 | TIMEOUT | SKIP | FAIL桶细分 |
|---|---:|---:|---:|---:|---:|---|
| Ubuntu | 21561 | 21151 | 408 | 2 | 47 | 407FAIL+1SIGABRT |
| macOS | 21514 | 21292 | 217 | 5 | 68 | 217FAIL |
| Windows | 20400 | 20205 | 176 | 19 | 138 | 174FAIL+1FAIL+LEAK+1ABORT |

共827非PASS终态（410/222/195）；明确missing host的严格分类60/60/36，Ubuntu明确bwrap105，其余仍需逐身份归因。RTM_NEWADDR EPERM只证明该操作被拒，不能直接证明全部需自托管runner。Ubuntu和Windows的guardian ephemeral栈失败仍开放；旧mac安静选集无abort不是跨平台修复证据。

119历史集合为95PASS/1FIXED-linux/23旧BOUNDARY；全部126记录旧分类98/3/23/2。23旧BOUNDARY追加macCI PASS，保原窗口、不把波动写成不可修的本质属性。证据见canonical-ledger-map.tsv和ci-run6-failure-ledger.tsv，均不含原始请求/日志正文。

## 3. 仍需开发的主线

1. CapExhausted在api_bridge映为InvalidRequest时丢结构化usage；CapPartial无生产发射、stable host response key、completeness reducer、双模式读/resume/API/exec/旧reader均未闭环。
2. D6仍无完整载荷/framing token证明；诊断byte/count限不代表模型注入硬限。默认legacy行为保留，P0人工门开放。
3. 默认策略迁移、Basic Auth/CONNECT实际header、Windows/WS/SOCKS/IPv6、冷态原预算和runloop因果实验未完成。
4. V8配对工件/host、827CI失败逐身份、正常4MiB栈问题、Bazel/三平台/容器/异OS等验收仍需完成；不能仅用BOUNDARY标签关闭。
5. owner/行政完整矩阵、三协议来源/旧历史、真实厂商cap触顶/opaque能力、安装升级回滚仍按原40项DoD推进。

具体后续顺序与Claude任务见 `claude-followup-2026-10-11.md`。

## 4. 本轮验证（结果及格式化收尾）

首轮当前修复切片：API/Rig/http-client全量与Core定向，just test、rust-rig、offline/locked、retries0、threads2；688run/685PASS/3FAIL/4976filtered，279.535s，构建7m12s。三失败为Default pool子进程用例：managed_redirect_failure_is_tracked_before_proxy_retry、direct_and_routed_clients_build_equivalent_requests、legacy_custom_ca_fallback_is_limited_to_reqwest_default。首败保留，不按历史标签自动判环境。

首轮后新增Factory公开入口/SDK派生total保护，并清理unused event_index；正在用同source重建app-server辅助binary，重跑HTTP/Rig及Core和R1b定向。使用已有target保留本机V8配对构建缓存；不把它当clean runner/V8工件证明。宿主network guard未设置，实际mock请求断言执行，未运行paid live。

`just bazel-lock-update` exit0，MODULE.bazel.lock无漂移；有既有platforms/rules_cc直接版本提示，不等于Bazel build/test通过。最终scoped fix/fmt及提交结果以最终追加节与回执为准；不自动push。

### 最终定向复验

- 第二批：先重建当前app-server，新增测试初始化漏SDK tool_use_prompt_tokens导致compile exit101，补Default后重跑。492run/478PASS/14FAIL/7034filtered，360.033s：13R1b重启病例+1usage旧期望。
- R1b13来自不完整账号fixture：无account/user ID→进程内随机credentialInstance，重启变化；已补固定测试claims/header。保留真实域隔离，不能拿这些假账号测试作为同kind授权理由。
- usage旧期望把SDK派生total当报告值；改为未知，保持无假Complete。
- 最后一批：重建app-server helper exit0，同五package/feature图定向73/73，exit0，107.054s/7453filtered，含完整16参数R1b、3首败HTTP、Factory四case、Core合法工具cap、真实rollout/resume、presence/range/derived-total。原两批数字不求和成unique总数。
- 完整命令见/tmp/codex-oct11-review-tests.log、review-rerun.log、review-final-tests.log、review-focused-tests.log各阶段及最终回执。原始日志仅本机，不提交敏感正文；源码验证未升级为完整workspace/新CI/跨平台/live。

### 可复验命令

均在codex-rs，用CARGO_BUILD_JOBS=4；第一批选API/Rig/HTTP/Core，后两批增加app-server。严格retries0；第一批threads2、后两批threads1。

```sh
cargo build -p codex-app-server -p codex-core --bin codex-app-server --features codex-core/rust-rig --offline --locked
just test -p codex-api -p codex-rust-rig-bridge -p codex-http-client -p codex-core --features codex-core/rust-rig --offline --locked --retries 0 --test-threads 2 -E 'package(codex-api) | package(codex-rust-rig-bridge) | package(codex-http-client) | (package(codex-core) & test(/model_output_projection|replay_grant|rig_output_cap|token_usage_rollout|cap_partial|record_token_usage|repeated_response|ledger_survives/))'
just test -p codex-api -p codex-rust-rig-bridge -p codex-http-client -p codex-core -p codex-app-server --features codex-core/rust-rig --offline --locked --retries 0 --test-threads 1 -E 'package(codex-http-client) | package(codex-rust-rig-bridge) | (package(codex-api) & test(/reported_usage_tests/)) | (package(codex-core) & test(/model_output_projection|replay_grant|rig_output_cap|token_usage_rollout|cap_partial|record_token_usage|repeated_response|ledger_survives/)) | (package(codex-app-server) & test(/guardians_retain_evidence_after_compaction_and_resume/))'
just test -p codex-api -p codex-rust-rig-bridge -p codex-http-client -p codex-core -p codex-app-server --features codex-core/rust-rig --offline --locked --retries 0 --test-threads 1 -E '(package(codex-api) & test(/reported_usage_tests/)) | (package(codex-rust-rig-bridge) & test(/stream_lifecycle_tests|usage::tests/)) | (package(codex-http-client) & test(/resolved_default_factory_tests|managed_redirect_failure_is_tracked_before_proxy_retry|direct_and_routed_clients_build_equivalent_requests|legacy_custom_ca_fallback_is_limited_to_reqwest_default/)) | (package(codex-core) & test(/model_output_projection|replay_grant|rig_output_cap|token_usage_rollout|cap_partial|record_token_usage|repeated_response|ledger_survives/)) | (package(codex-app-server) & test(/guardians_retain_evidence_after_compaction_and_resume/))'
```

73的最后命令对应最终源码；前两批记录不同阶段，不承诺在今后HEAD恢复旧失败。最终scoped fix/fmt后按AGENTS没有重跑tests。

### lint / 格式与证据边界

Scoped just fix（api/rig/http/core、rust-rig）exit0；自动修复两处system-store测试和一处custom CA测试的等价表达。发现ApiError扩大导致realtime私有enum的large_enum_variant，机械装箱并同步sideband与测试消费方；初次严格检查分别报告未同步消费方与一个cloned_ref切片lint，均已纠正。最终同四包 `just clippy ... -- -D warnings` exit0、零告警（1m21s）。

按AGENTS，测试均在fix/fmt之前；机械装箱及最后等价lint清理有严格编译/lint验证，未在其后重跑tests，也没有单独声称realtime运行验收。完整workspace、新CI、clean V8、原生Windows/Linux、live与发布均未执行。

最终just fmt exit0，git diff --check exit0；44个明确路径与计划完全相等，credential-shape扫描0。五代码阶段135/405/205/69/441changed，各<500；CI827终态表单独生成数据提交（828行），正文/映射约711行<800。只有最终完整WIP经过上述相关测试，分批中间树未逐个重新执行；不将提交拆分称为每批独立实测。
