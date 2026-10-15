# Codex 集成复审：Rig、图片预算、后台模型与证据更正

审查起点 `914a400d7`，本轮开发范围 `29d55354c..914a400d7`。复审在
`/Volumes/soddy/git_workspace/fork-nuwax-codex/.claude/worktrees/merge-main-20261014`
的 `codex/rig-044-20261014` 进行；用户授权修复后合回主工作区 `test` 并 push。
本地 main 快照 `cb03f28a4` 的合并及 Rig 0.44.0 已在候选内，不再重复升级。

## 1. 确认并修复的问题

下列路径相对 codex-rs；位置引用原候选 `914a400d7` 的具体文件／函数，
本轮提取模块后请以函数名定位最终实现。

1. **P0（context 规则）：original 免费和调用方大 policy 绕过单项上限。**
   `utils/output-truncation/src/lib.rs:337,425`。8 张 original 可贡献约80K
   视觉 token，还保留完整文本预算；大 policy 也可留下8张 high（14,752）。
   现正文／音频估算／视觉共用10K估算硬限，内含256控制开销。high/auto
   每张1,844；original 共用有界 header-only 32px patch 估算，不做像素
   解码，小合法原图保留 detail，未知／File／超界收费10K并明确省略。
   同步最终 Chat/Responses wire 估算，避免入口放行的小原图又被误拒。
   本轮已人工复核该 P0 的格式与上限，不把“减少了旧风险”当成合同完成。
2. **P2：二次截断会丢图片省略说明和运行 cell ID。**
   `core/src/tools/context.rs:196`、`core/src/context_manager/history.rs:576`。
   第一次截断后的 header 和图片省略标记，在 history 的第二次预算预扣后
   被当普通文字删除。现只保留一个受长度约束的生成头部，合并三类省略
   标记并计入有界控制开销；相似的大段文本照常收费。测试穿过真正的
   AnyToolResult→history 和 Code Mode yielded output，不仅比对中间值。
3. **P2：引用元数据缺少独立限额。**
   `utils/output-truncation/src/lib.rs:383` 的 File 分支不检查 file_id 长度。
   现 File ID 与 data URL 头部各限8KiB；图片整体仍有16MiB单图、24MiB
   聚合与数量限额。JSON引用元数据限不冒充所有图片内部 metadata 的解压限。
4. **P2：prepared 二进制32MiB不等于最终 JSON32MiB。**
   `utils/image/src/lib.rs:254`。24–32MiB二进制经Base64后仍可突破整包限。
   现先算 data URL 膨胀，并在32MiB中留具名1MiB余量，使用无分配算术
   测试边界及溢出；多图片／其他正文的总量仍由最终 wire guard 检查。
5. **P2：启动 Config.model 被误当运行时活动模型。**
   `memories/write/src/phase1.rs:167`、`phase2.rs:348`、
   `ext/guardian-v2/src/async_scorer/classification.rs:332`。`thread.config()`
   返回启动配置，切模或目录默认值并未写回它。现 memory startup 从
   config_snapshot 捕获一次活动模型供两阶段共用；async reviewer 政策用
   已捕获的 parent ModelInfo。新增实际两阶段请求和 WS 政策请求捕获。
6. **P2：内置 Bedrock 的专用后台模型被覆盖。**
   `core/src/guardian/reviewer_config.rs:147`、`memories/write/src/runtime.rs:483`。
   原规则只豁免字符串openai，使两个Bedrock provider的preferred被活动
   模型覆盖。现复用实际 ModelProviderInfo.is_first_party，保留OpenAI与
   两个Bedrock的专用默认；第三方用活动模型；显式override仍优先。
   未发现配置 key 与 provider_id 不一致：加载及远端恢复均有身份写回。
7. **P2：v2 cassette 未保存 cap usage 的 wire presence。**
   `codex-rust-rig-bridge/src/cassette.rs:176`。live有原始total／显式零／
   部分计数，replay只读normalized RigUsage而丢掉presence。TerminalRecord
   现可兼容缺省地保存wire_reported_usage，live snapshot后录制，replay
   恢复；完整比较Chat cap三种usage的错误、前缀和磁盘字段。旧v1/v2仍可读。
   另恢复一份2,364字节的真实旧v1 golden，并同步Bazel compile_data。
8. **P2：图片生命周期测试接受失败的 TurnComplete。**
   `core/tests/suite/rig_image_history.rs:313`。旧helper未检查error，冷恢复
   发出正确请求后终态失败仍能PASS。现检查error，并加单发cap失败注入。
9. **验证缺口：20张截图测试仅由人工usage增量触发。**
   `core/tests/suite/rmcp_client.rs:5085`。现每张PNG和call ID独立，用实际
   捕获请求的文本＋视觉估算报告usage，设置真实40K窗口／30K压缩阈值及
   输出预留；检查每个采样和压缩请求均能容纳、每张图到达typed image字段、
   普通采样配对完整、持久化保留20组call/result、正式压缩及冷恢复只追加。
   压缩请求会按设计折叠工具消息成压缩文本，不强求其保持普通tool消息形状。
10. **证据与规范：PASS 数不是执行数。**
    `codex-review-request-2026-10-16.md:55`、migration notes第十一轮中的
    “84/84真实付费绿”撤回为nextest报告数；独立可核的是50个turn captures
    （42组Chat/Anthropic events，8组Responses SSE）。不推断84次都付费。
    两个本批新建inline测试模块也已移入专门的 *_tests.rs。
11. **P2（复核新增）：已知小原图上传成File后丢失计价尺寸。**
    `core/src/image_preparation.rs::prepare_image` 与 history二次截断相交。
    工具original现保留已验证的inline表示，不先上传再丢弃，也不改detail；
    用户original和工具high继续使用附件存储策略。RecordingFileStore回归
    检查零无用上传、原图真正入history，以及其余两类正常上传。已存但无
    可信尺寸的File original仍走保守限额，不能从全局裸file_id缓存猜成本。

大小审查：原三条非机械提交分别542／1,487／932行，存在可拆的独立机制；
本轮不重写已形成的历史。大fixture数据已独立提交，按机械数据例外处理。
本轮修复按机制分组，图片helper提取与行为修复分开保存。

## 2. 撤回上轮的假通过

`/private/tmp/codex-rig044-review-core-cap.log` 原30项共0.212s；
`core/tests/common/lib.rs:605` 的 skip_if_no_network 宏检测变量存在即return。
因此2026-10-15 Codex记录的Core cap 30/30不能算验证，明确撤回。
Claude的 `6c2d2c562` 修复本身经本轮实际执行复核；未修改禁网常量或跳过宏。

本轮所有集成运行先确认 CODEX_SANDBOX_NETWORK_DISABLED、CODEX_SANDBOX
均不存在，允许本机loopback，未加载live凭据执行模型请求。`--offline`
只限制Cargo取依赖，不意味着HTTP mock未运行。默认OpenAI的281个通用
resume/compact仍只作普通回归；显式Rig与原生对照分列，不混为同一证据。

## 3. 本轮验证收据

环境：macOS arm64，pin rustc1.95.0（59807616e），nextest local，retries=0。
仅本地mock／文件回放，未调用付费API。完整workspace仍留给push自动CI。

下列过程保留首败，不用重跑结果抹掉失败：

- 原始Bridge＋utils：343跑342过，proxy parity超时；同一用例隔离8.7s过，
  后续组合运行29.6s过。未放宽测试deadline，不把此单次超时宣称产品回归。
- 原始显式Core＋stress：51/51实际执行全过（含真实cap与原生对照）。
- 图片新边界RED：5/5失败，见 `/private/tmp/codex-oct16-image-red.log`。
- 首次组合在新memory测试遇到assert_eq宏导入歧义，补显式pretty_assertions。
  随后359跑357过，两处新夹具断言失败；修正max_completion_tokens字段
  差异与text-only上游移图说明。再跑485中484过，余下一处错误要求压缩
  请求保留tool消息形状；改为分别检查采样配对与完整rollout，不削弱账目。
- 随后组合495跑489过、6失败，涉及测试runtime、ICC预期和并发编译时序。
  修正后在quiet串行安排下定向重验347/347全过，日志
  `/private/tmp/codex-oct16-quiet-validation.log`（253.504s）。这347项的
  分项如下；nextest另报7,563项skipped，不计入本轮已执行覆盖。

| 分项 | 实际执行／通过 |
|---|---|
| app-server公共接口 | 9/9 |
| core lib | 168/168 |
| core all集成 | 15/15 |
| guardian-v2 | 1/1 |
| live cassette roundtrip | 8/8 |
| env启动 | 3/3 |
| memories | 56/56 |
| bridge | 29/29 |
| utils image | 21/21 |
| utils output-truncation | 37/37 |
| 合计 | 347/347 |

- 最终离线replay：nextest报告84 PASS，日志
  `/private/tmp/codex-oct16-replay-final.log`（0.272s）。其中包含测试函数
  内部early-Ok跳过；runner的“0 skipped”不能识别这些内部返回，不能解释
  为84个协议场景实际执行，更不等于84次真实或付费API调用。
- Cargo只增加两条既有workspace图片工具依赖边，未升级版本；已运行
  `just bazel-lock-update`，完成后`MODULE.bazel.lock`无漂移。默认Python3.9
  不兼容脚本，改用Python3.13执行
  `.github/scripts/run_bazel_with_buildbuddy.py mod deps --lockfile_mode=error`
  通过。
- scoped `just fix`通过；遗留的测试helper lint已机械修正。随后按CI命令
  `cargo clippy -p codex-rust-rig-bridge -p codex-live-tests -p codex-config
  -p codex-utils-cli --all-targets --locked --offline -- -D warnings`通过，
  日志 `/private/tmp/codex-oct16-ci-clippy.log`。`just fmt`及最终diff-check通过。
  scoped fix仍报告既有ThreadSpawnError大枚举／未用live helper警告，不能
  把局部CI严格检查通过表述为整个workspace零警告。

测试发生在候选`HEAD=914a400d7`＋本轮dirty源码，尚非最终提交SHA。
当时源码补丁为`/private/tmp/codex-oct16-tested-source.patch`，SHA256：
`57d6fa23f92c1fbeb2640130bff277673a784a5dd624680af4a35fd086e0dc25`。
该导出工件在无末尾换行的legacy JSON后缺少patch边界换行；规范化版本
`/private/tmp/codex-oct16-tested-source-normalized.patch`只补标准patch边界，
SHA256 `04c13883a8d3c1a39a4ad52d95544f4a7c4b71d68b1fcbab684c0e6395597cc2`。
已应用至独立临时index并核对golden字节一致；与最终源码的差异仅为格式、
等价lint改写、注释与测试helper错误传播，没有新增生产行为变化。
二进制来自本候选worktree的`codex-rs/target/debug/`：

| 二进制 | SHA256 |
|---|---|
| codex | `09f2f46ed735b1bcdd5179ae31d72a49b44b1c0f07b488ab0b7a1004fcd11f21` |
| codex-exec | `9779fc261fe3de3ee0beba1eb944c73aafa8b44ac1f7de93448bf0d7539541fd` |

收尾按tests→scoped fix→just fmt→diff-check；最终fix/fmt之后不再重跑
测试，不拿上述二进制或测试补丁冒充最终提交的构建收据。下一轮按实际
`test` HEAD重新构建。代码修复保存至`0ef8b8ff7`，其codex-rs子树对象为
`dbff83cba1740bb282939ae164603f908f10bf25`；以上测试针对整体修复树，并非
逐条中间提交各自运行。交付采用候选→test快进合并并推送origin/test，
最终文档提交SHA、远端一致性及自动CI链接由本轮Codex最终回复提供。

## 4. 独立fixture与平台边界

150个变动录制文件：50 final-request、50 converted rig、42 rig-events、
8 raw Responses SSE；JSON可解析，42 events均items+terminal。认证头、
常见key／JWT／私钥／URL凭据模式和3个本地真实credential的exact search
均零匹配；不输出凭据。root另独立扫描151个文件（含golden），143个JSON
可解析，3个实际credential exact值与各类敏感模式均0匹配。

旧Linux／Windows收据对应 `4ca0e4fa0`，不当成本轮后续修复的三平台证明；
最终以推送SHA的CI及必要原生定向回归补齐。旧ledger不能直接把当前guardian
失败归为无关；需要同平台、工具链、features与栈的准确对照。

保留的限制：Rig-event cassette只作单attempt证据；多attempt pause仍用
wire测试。guardian-v2 async sampler固定Luna且默认bridge provider整体禁用
是基线行为，本轮改的是政策选择，不声称第三方async scorer已启用。
40MP是新批次的源图安全上限，并不证明所有合法原照片都低于36MP。
真实识图收据仅证明当时MiMo小红图场景，不认证所有模型／图片／新提交。
ACP驱动仍在外仓gitignored tmp，需归档无凭据版本才能成为可重复验收。

下一轮以 [CI与使用路径验收提示词](claude-next-ci-validation-2026-10-16.md)
收口，先验收当前功能，避免继续扩展通用附件上传平台。

## 5. 修复提交索引

| 提交 | 内容 |
|---|---|
| `2abe215db` | 图片准入helper机械提取 |
| `1f2993878` | 原图header估算与Base64整包大小 |
| `9e4a4276b` | 共享硬限、控制头部与省略标记 |
| `9ea6c860b` | 图片重复截断／原图边界回归 |
| `4a0dd66d5` | 工具原图准备与history边界 |
| `be7de603a` | 活动模型及OpenAI/Bedrock默认优先级 |
| `f8cae3190` | memory两阶段运行时模型快照 |
| `2e1f62e73` | cassette wire presence与旧v1 golden |
| `c2a602c1c` | 图片生命周期失败断言与20图真实预算stress |
| `0ef8b8ff7` | scoped Clippy机械修正 |
