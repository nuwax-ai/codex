# Codex 稳定性复审与集中收口

复审起点：Codex `8e7c5904b..477782126`，ACP dev `2b4ae13`。
用户授权直接修复、commit、push，并安排较完整的后续稳定验收。
候选位于 `.claude/worktrees/merge-main-20261014`，最终合回 Codex `test`；
ACP 修复保存在其 `dev`。不改两仓main，不调用付费模型，不手动dispatch。

## 1. 已确认的问题与处理

1. **P1：图片预算改造误伤纯文本配置。** 上轮Codex的`9e4a4276b`
   无条件把纯文本、结构化MCP、Code Mode输出压到9,744 tokens，覆盖
   用户明确配置的30K/100K。不是已经证实的Linux取数分叉。现仅图片
   承载结果或图片省略标记保留10K共享硬限；纯文本沿用调用方预算。
   图片数量、单图／聚合字节、引用元数据及未知original保守计费仍保留。
   三个文本预算新回归先全红；修后utils全40/40、0跳过。
2. **测试缺陷：fs_watch用noop验证成功注册。** 新的FileWatcher合同明确
   noop.registration_succeeded=false，四个owner/duplicate测试却用noop。
   仅测试helper改用真正FileWatcher，生产降级／失败语义不变。
3. **合同冲突：cap的累计花费和context baseline混淆。** SpendingOnly
   的total必须包含cap usage，但last保持最近已安装模型输出的context
   baseline。首轮cap是total14/last0；成功33后cap是total47/last33。
   不能直接改持久化last=14，也不能只发临时TokenCount——该事件会写入
   rollout，冷恢复直接重建同一baseline，错误覆盖会破坏下一轮context估算。
   现测试完整比较字段、失败回合及冷恢复通知，仍禁止假Completed／完整
   agentMessage；没有修改产品计费代码或放弃累计消费断言。
4. **ACP验收假红：两个“产品缺陷”撤回。** mock把guardian引用的截图
   指令当主指令，在无tools的结构化审查请求里发tool_call，造成unsupported；
   又拼接所有user text parts，先匹配已取消旧essay而忽略真正的compact／
   follow-up。旧tmp驱动也共用此mock，不能当独立产品复现。修正schema
   分流、最新text part、真实广告目录与全局请求上限；工具完成通知通过
   toolCallId关联开始通知的名称。未改Codex namespace/router兼容逻辑。
5. **P2：ACP自动标题仍硬编码Luna。** 主会话第三方模型已正确，隐藏
   标题请求却仍发gpt-5.6-luna。TitleGenerator现同步捕获provider及活动
   base model，临时线程继承provider；OpenAI与两个内置Bedrock保留原
   preferred。两个入口解析ACP的model[effort]，不把组合ID发给供应商。
   新RPC回归和六场景S2真实HTTP同时检查标题model及dummy auth。
6. **ACP严格检查遗漏。** 归档TS驱动在src外，根tsconfig默认include它却
   rootDir=src，破坏typecheck。现根项目明确include src，驱动使用独立
   strict tsconfig并纳入npm typecheck；SDK参数／必需env变量正确建模。
7. **CI可重复性与磁盘。** 两个Cargo lane由浮动stable固定到仓库pin1.95.0；
   Windows TEMP/TMP移到workspace同盘的专属兄弟目录，避免本run C盘
   临时数据库写满，并打印各盘空间。不删除预装工具或用户数据，不加栈／
   timeout，不把换工具链当作证明旧失败全是工具链造成。
8. **中断夹具审查。** `e6ab13a18`的审批应答与实际spawn握手正确。
   ExecCommandBegin证明启动成功，不能证明shell内部sleep已抵达；
   源码注释收窄，不改产品执行路径。

9. **CI统计工具静默漏行。** `nextest_log_summary.py`原RESULT和CANDIDATE
   都不接受`FAIL + LEAK`，导致漏掉复合终态且不报告。现识别并规范化该
   状态，未知复合及retry仍报unparsed；新增回归先红，修后Python12/12。
   parser只证明结果行完整性，不能以缺最终Summary的行集合冒充进程成功。

## 2. CI证据更正

统计同一crate+完整test身份，包含FAIL、FAIL+LEAK、TIMEOUT、SIGABRT。
旧账本只数FAIL，不能称完整失败总数。原始日志：
`/tmp/rig044-ci-firstfail/*-clean.log`。

| 平台 | run38782462743 | 基线38528171490 |
|---|---|---|
| Ubuntu | 27：25FAIL、1SIGABRT、1TIMEOUT | 4：3FAIL、1TIMEOUT |
| Windows | 中断前94：76FAIL、2FAIL+LEAK、15TIMEOUT、1ABORT | 49：24FAIL、2FAIL+LEAK、23TIMEOUT |

Ubuntu最终Summary：22,837 run、22,810 pass、26failed（含SIGABRT）、
1timeout、48skipped。Windows无最终nextest Summary，runner磁盘写失败
中断。集合对照Ubuntu重叠3／当前独有24／基线独有1；Windows为24／69／25。
以上集合对照先按FAIL/FAIL+LEAK/TIMEOUT计算，当前93个身份；另有1个
ABORT使完整失败/超时/中止为94。ABORT是19:58:39Z的
`retained_tool_output_survives_compaction_and_resume`，先于空间不足，不能
用后半场disk full解释。另有30个LEAK（基线272个）属通过但泄漏，单列
资源诊断，不与FAIL+LEAK混为失败；parser的non_pass包括这些LEAK。
基线早于整个main合并，这些集合差异不能定位单一提交。

Windows首个明确空间不足证据为2026-10-15T20:24:43.9559098Z，日志第19180行，
`os error112`；之前57个失败／超时及1个ABORT，之后36FAIL，随后大量SQLite code14，
最终runner自身日志也写不下。时间边界不证明更早磁盘健康，也不能解释
之前全部sandbox／doctor问题。初版独立解析也漏FAIL+LEAK的91/47已撤回。

详细独立审计：`/private/tmp/codex-oct16b-ci-audit.md`。

## 3. 本轮实际验证

本机macOS arm64，codex-rs内pin rustc1.95.0；集成执行前确认禁网／sandbox
跳过变量不存在，保留仓库默认8MiB栈。全部模型请求为loopback mock。

- utils output-truncation：40/40，含新回归RED→GREEN。
- ACP：strict typecheck通过；TitleGenerator9/9；mock contracts8/8。
- 修正后的ACP同旧binary六场景6/6，9次HTTP；日志
  `/tmp/codex-acp-review-title-final.log`。该binary SHA256为
  `7fba393f9cf5549dbcd14a78d85636dc8d3e2da13f4ad7d45a841bc3f6635d75`，
  只作隔离mock缺陷证据，不能冒充本轮最终Rust源码产物。
- Mac Core／app-server首跑229项，227过、2个新旧夹具失败；修正当前
  turn通知关联及可容纳图片的origin预算后，usage两例2/2、图片相关18/18
  均过并覆盖原失败。原229项已逐项收口，不声称整个229选择器又完整跑过。
  日志：`/private/tmp/codex-oct16b-{baseline-selected,usage-fixed,image-final}.log`。
- Linux校准后原选择器211/211全过（164.732s，7159因过滤未选）；含
  纯文本配置、CodeMode、图片限额、fs_watch和两例usage冷恢复。日志
  `/private/tmp/codex-oct16b-linux-calibrated-v8.log`。此前211跑83过128失败，
  缺host的unsupported簇已在同选择器关闭；第一次host构建又碰默认V8
  下载缺失，改按CI既有artifact pair+仓库可信manifest校验，不改V8 pin。
  host SHA256 `9f019b51589388095676edaa651607aad87b7f4cb2ff81ecb8d80090b45dd093`。
- scoped Clippy通过（既有large enum／未用live helper告警仍在）；CI同款
  bridge/live-tests/config/utils-cli全targets `-D warnings`通过。`just fmt`
  与最终diff-check通过。未跑整个workspace，不把局部检查称全仓零告警。
- 原始CI日志去掉时间戳后由修复的parser再次核对：完整状态分列，泄漏
  与ABORT不吞；保留missing_results／缺Summary边界，结果在
  `/private/tmp/codex-oct16b-ci-parser-real-input.json`。
- 最终交付采用新提交重建CLI／exec／Code Mode host，再以expected SHA
  跑ACP六场景。机器收据写`/private/tmp/codex-oct16b-build.log`与
  `/private/tmp/codex-oct16b-acp-final.log`，最终产物SHA由同轮答复报告；
  上述旧binary只用于隔离mock原因。

新usage测试第一轮发现夹具自身跨回合缓存污染：成功回合的item/completed
被后续cap断言误消费。现所有相关通知均按当前cap turnId筛选，缺ID仍报错，
不放宽本回合断言；首败保留在`/private/tmp/codex-oct16b-baseline-selected.log`。

旧图片预算夹具A100/B400却要求保留1844/4096视觉费图片，不满足共享预算。
现A/B为2000/4000、序列化预算2400/4800，预扣视觉费后分别精确比较556/704
文本额度；完整raw／图片尺寸／跨模型／持久化／恢复断言全部保留。

Linux使用独立`/tmp/codex-review-20261016b`源码目录，共享仅构建缓存；
初次因无just未运行，随后按just recipe显式设8MiB／local profile执行，
不把缺命令或缺栈当产品失败。初次日志
`/private/tmp/codex-oct16b-linux-selected-retry.log`保留缺host首败；校准后
构建所用V8 archive+binding严格匹配仓库sidecar manifest，且CODEX_REPO_ROOT、
工具链和profile齐全。两者的区别不能归咎代码或额外打开不相关feature。

## 4. Windows旧SHA补录

读取原生机器`C:\tmp\rig044-val-e6ab13a18\run-*.log`：

| 套件 | nextest报告 |
|---|---|
| bridge | 301/301 |
| core rig／cap／stress | 52/52 |
| utils／models | 111/111 |
| env | 59/59 |
| live replay | 189PASS（含内部提前返回，不能当189场景实跑） |

五轨已于本地时间2026-10-16 05:45结束，原“后三轨仍跑”已补齐。
收据`/private/tmp/codex-oct16b-windows-receipt.log`；当时C盘仍有约173GB。
这些只补`e6ab13a18`旧账，不证明本轮新修复已在Windows跑过。

## 5. 后续集中收口

后台记忆正常Shutdown仍被记为failed_agent并延后一小时，是独立P2：
需带ownership的取消释放、先停子agent、保留watermark／失败预算、真实
错误仍backoff。不能一行把所有Shutdown／Errored／NotFound当干净取消。
ACP长驻期间与关闭时分别验收，不声称完全不受影响。

下一轮执行[十项稳定收口任务](claude-stability-closure-plan-2026-10-16.md)：
先冻结当前依赖、验证Linux／容器ACP主门，再补18场景三wire、Code Mode、
生命周期／公共读取重连、50轮资源压测、记忆退出、Windows和可运行产物。
不能把非目标官方全部22K测试或新File平台设成主门的无限前置任务；
也不能因旧基线同败而忽略真实日常路径阻断。真实付费／视觉另取授权。

## 6. 配套ACP提交

- `7e45c8f`：第三方活动模型自动命名。
- `c9e0ab2`：验收mock、toolCallId关联及严格typecheck。

保留原`2b4ae13`归档提交；已有未跟踪vendor缓存不纳入提交、不删除。
