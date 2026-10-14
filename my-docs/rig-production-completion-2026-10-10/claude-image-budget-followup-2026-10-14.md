# MCP 截图与图片预算：分析和 Claude Code 下一轮任务

日期：2026-10-14。目标：国内／第三方视觉模型经 NUWAX_* → Codex → Rig 使用时，MCP 截图保持图片语义，请求大小与上下文成本分别有界，连续截图不会因为本地错误计量导致 turn 失败。

后续用户补充了同步main和考虑升级Rig的要求。执行顺序以 [main/Rig整合交接](claude-main-rig-upgrade-followup-2026-10-14.md) 为准：先本文件第一批的小范围wire预算修复，再独立整合main、独立Rig迁移，最后入口和连续截图策略。本文件“本轮不升级Rig”的边界限定为图片功能批次不同时升级依赖，不排除下一轮独立升级阶段。

## 已核实的版本与范围

- fork：`/Volumes/soddy/git_workspace/fork-nuwax-codex`，`test`，分析时 HEAD `7df7dc8f89ab483315dd6dc8ad8fc02b8b5669df`，工作树干净。开始开发时重新核对，保留并行改动。
- 官方：`/Volumes/soddy/git_workspace/codex`，`main`，本地快照 `cb03f28a4034b6ee901885808d27182ac586830a`。这是本地源码对照，不宣称是网上最新版本。
- Rig 参考仓库：`/Volumes/soddy/git_workspace/rig`，HEAD `9e7cb6000`，workspace `0.44.0`。
- fork 实际依赖：`codex-rs/codex-rust-rig-bridge/Cargo.toml:10`，registry **rig-core = 0.42.0**；Cargo.lock 的 checksum 已核对。其缓存源码位于 `/Users/soddy/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/rig-core-0.42.0`。
- ACP 实际客户端：`/Volumes/soddy/git_workspace/fork-codex-acp-ts`，`dev`。本轮中心是 Codex；官方和 Rig 仓库只读，ACP 入口问题单独给出最小修复和验证，不混入 Codex 提交。
- 本次只读分析，没有修改生产代码、执行测试或调用真实模型。此前文本／工具 live 验证不证明视觉能力。
- artifacts 迁移已由用户处理并取消，不再操作该事项。

## 判断与依据

### 1. 正常图片字段使用正确；最终预算有真实缺陷

`request_content.rs:162–238` 将 data URL 解析为 Rig typed Image（Base64 + MIME），不是 Text。Chat 用户图片是 `messages[].content[].image_url.url`；Anthropic 是 `messages[].content[].source.data`，source 带 `type=base64` 和 `media_type`。Chat 的工具图片放在紧随工具结果的补充 user image 中，保持 call/result 配对；这是对 Rig 0.42 工具图片限制的适配。

Responses 路径见 `codex-rust-rig-bridge/src/responses.rs:197–218`：Codex 请求原样序列化，再用 Rig HTTP 发送，不走 Rig 通用图片转换器。用户图片是 `input[].content[]` 的 `input_image`，工具图片留在 typed `output[]`。因此不能将 Rig 通用 Responses 图片限制直接当作本 fork 透传路径的限制。

**P1：`transport.rs:444` 调用 `wire_budget.check(request.body().len())`；`wire_budget.rs:51` 把整个最终 JSON 的字节数除以 4，连图片 Base64 也计作文本。** 主线 `core/src/client.rs:3438` 会传模型 usable context window，三种协议均受影响。处理后的 1MiB 图片约有 1.33MiB Base64，被误估为约 349,526 个文本 token；128K 上下文可能在网络发送前被误拒。

错误映射为 ContextWindowExceeded。普通 sampling 的 `session/turn.rs:1712` 会返回错误；不能声称所有该错误都会当轮自动压缩恢复。整个请求独立的 **32MiB 字节硬限制应保留**。

### 2. 官方方案值得参考，fork 已继承许多机制

官方和 fork 都采用 `ImageReference::{Inline,File}` 与 `AttachmentStore`。官方 app-server 的 `message_processor.rs:362` 注入 `passthrough_image_store()`；官方 `attachment-store/src/lib.rs:166–172` 的 InlineAttachmentStore 只是返回原 bytes。没有发现可见生产默认路径自动向厂商 Files API 上传；`thread/attachment/*` 是聊天关联工件的存储 RPC，不是模型 Files API。

中央图片准备处理 user、function/custom 工具输出。默认 high：最长边 2048px，32px patches ≤2500；Original：最长边 6000px，patches ≤10000。见 fork `utils/image/src/lib.rs:75–83`、`core/src/image_preparation.rs:390–406`。Original 也有界，不等于无限原图。UnifiedImageBudget 默认关闭并有模型能力 gate，不能当作当前默认方案。

Core 的 `context_manager/history.rs:1039–1056,1267–1288` 对图片单独粗估：普通图约1844 tokens，Original 按尺寸计算32px patches。Base64 长度不作为文本长度。这个规则不是所有国内模型通用的精确视觉 tokenizer。

普通历史会重放仍在历史里的图片；没有通用“仅发送最新截图”。remote-v2 的 CompactionImageBudget 默认开启，64K retained 预算内图片与标签原子保留，但 **本 fork 的 Rig provider 通常走 local compaction**，不能借 remote-v2 的测试声称 Rig 已有全局截图预算。local compaction 保留文本/summary，舍弃媒体；压缩请求自身也需要旧历史，超限后才尝试压缩可能已经太迟。

官方 Guardian 的 `guardian-context/src/images.rs` 有4张/8MiB的局部选择策略，可以参考“数量+字节分别限制”的设计；它不约束普通对话，也不能直接复制为每轮删旧图的策略。

### 3. MCP／ACP 仍有二进制变文字的入口

- `protocol/src/models.rs:2317–2342` 优先非 null structuredContent 序列化为 Text，普通 content 图片可能被忽略；若 structuredContent 本身包含 Base64，它会成为真实文本。这是官方同样存在的合同，不能贸然全局改变优先级。
- 同文件 `2443` 的未知/格式坏内容块会被整个 JSON 转为 InputText；resource blob 也可能走这条路。
- `core/src/tools/handlers/mcp_resource.rs:361–368` 将 read-resource payload 序列化为 JSON 文字后截断；不能把它当已支持图片资源的视觉入口。
- ACP `src/CodexAcpClient.ts:1286–1292`：image MIME 资源是图片；缺少或不匹配 MIME 的 blob 被放进 `<context encoding=base64>` 文字。
- Code Mode 的 `image(result.content[0])` 发 typed image；`text(result)`／JSON.stringify 整个工具结果则发文字。不要自动从任意普通字符串中猜图片。
- `utils/output-truncation/src/lib.rs:162` 对图片不扣文本预算；单次工具多图并未因此有界。输入1GiB sanity、单图缩放、整包32MiB分别有作用，但不是完整视觉预算。

未知模型 metadata 还会 fallback 到272K和Text+Image（`models-manager/src/model_info.rs:157–164`、`protocol/src/openai_models.rs:190`）。这不证明国内主用模型有该窗口或视觉能力；验收须使用真实能力和窗口。

### 4. Rig 的附件能力与合理使用方式

Rig 0.42 已有 typed 图片来源 Base64/URL/FileId，但不同 wire 兼容性不同：Chat image file ID 不通用；Anthropic0.42用户图片支持Base64/URL、工具图片仅Base64。0.44扩大了 Anthropic/Responses file ID 表达能力，仍受厂商 dialect 限制。

没有发现 rig-core 提供可直接替代本项目方案的通用 Files uploader。0.44 cassette 的 Anthropic 上传由测试侧 reqwest multipart 实现；FileLoader 是本地UTF-8文件读取，不是上传。不要把图片改成非PDF Document::Base64绕限制，它可能变为文字。

URL/file ID 可减少重复传输，但引用图片仍计视觉 token，不能解决截图积累本身。当前推荐默认保留有界、缩放后的 typed inline 图片。仅在指定厂商明确支持、生命周期完整时，后续选配 File API。

官方说明：[OpenAI 图片与视觉](https://developers.openai.com/api/docs/guides/images-vision)、[Anthropic Files API](https://platform.claude.com/docs/en/build-with-claude/files)。兼容 Messages/Chat/Responses 不表示兼容厂商 Files API；file ID 不是跨平台通用资源。

## 给 Claude Code 的执行任务

请按以下顺序完成开发和离线验收。每批形成独立、可审查提交，不因第三批较复杂而延迟第一批。先读 AGENTS、上述源码和最新工作树；不要重做已完成的模型接入或混入 CI/Windows 大范围清理。

### 第一批：修最终图片预算误算，优先交付

1. 先写能在当前代码上失败的三协议请求级回归：typed 图片不会按Base64文本算几十万tokens；相同像素内容/尺寸采用不同编码大小时，视觉预算不应仅随Base64长度成比例。覆盖用户图、MCP/function/custom 工具图，Responses 真正执行，不保留不可达的测试分支。
2. 将最终请求的 **wire bytes** 与 **模型内容成本** 分开。保留32MiB硬限制和输出预留/无效配置错误；按实际协议的合法 content 路径识别图片，计文本、工具schema/参数和视觉成本。
3. 不全局删除名为 data/image_url 的字符串，不忽略普通text或工具JSON中真实的Base64文字；这些仍按文字计量。URL/file ID 图片不能按短引用长度计零成本；未知尺寸有保守、具名策略。
4. 优先复用图片准备/尺寸计量，新增代码放合适的小模块。明确estimate的依据与适用模型，不冒称精确provider tokenizer。已有配置/catalog可表达的能力不再造一套路由或模型注册系统。
5. 预算检查只读实际请求，不改变发送的原始bytes、raw整数、工具schema、顺序、call/result、thinking/hosted replay/pause语义。拒绝发生在HTTP发送前，不能伪装Completed，也不能无界重试。

完成标准：合法大图不再因Base64误算失败；真正文本超限、输出预留和HTTP字节超限仍拒绝；三wire全有实际出站断言。

### 第二批：MCP 图片入口、单次结果和诊断硬界

1. 将 typed 图片在 decode 前的encoded bytes、decoded bytes/pixels/内存、encode后单图bytes、单工具图片数量和aggregate bytes分别有界。复用utils/image和中央preparation，校验格式/magic/MIME；保留截图文字可读性和用户明确的高精度选择。
2. 单工具结果的文本+视觉估算合计遵守10K token合同；不能给文本9800后再无限追加图片。先测代表性截图，再在具体policy中写出选定数值与理由；不要只写“适当限制”。超过预算时采用有界缩放/拒绝/可见省略策略，保留call ID、文本说明和失败原因，不能静默成功且丢完截图。
3. 明确 structuredContent/content 图片、resource blob、缺/错MIME、大小写及MIME参数、invalid Base64、坏图片、超大metadata、普通Text里伪造dataURL的合同。支持 typed 的标准入口；不支持二进制不能整包转巨大文本。保留既有structuredContent优先语义中无图片的正常行为，图片混合语义做定向设计和回归。
4. MCP/Code Mode/direct app-server入口复用同一策略，不靠文本token cap假装控住图片。嵌套工具结果的image()、未emit图片的普通JS返回、text()显式输出分别验收，不无声改变Code Mode合同。
5. ACP先给独立最小修复方案和入口测试，避免未知二进制blob直接进context；不要顺手重构ACP或使用main分支。
6. 核对request capture的2MiB body cap与HTTP attempt预算的耦合。合法大图不能因为审计存原始Base64而被误当模型超限；如需调整，在独立小提交中让有界摘要/大小/hash与发请求计数兼容。不能简单关闭attempt预算或无限提高日志上限；摘要不能冒充原始请求证据。

完成标准：单工具多图、错误资源、缺MIME和decode病理输入有明确边界；正常截图保持typed image；所有失败保留原始原因，日志不存无限Base64。

### 第三批：连续截图与正规压缩边界

1. 在现有history/sampling/compaction流程中跟踪视觉估算、图片数量、实际wire bytes和输出预留，定义提前压缩阈值及安全余量。使用主用模型真实窗口；unknown metadata/fallback的能力与估算必须标明，不把272K默认值当厂商实证。
2. 在压缩请求本身超过预算之前触发正规压缩。明确保留最新任务所需截图/用户图的规则；必要时按已有checkpoint/retained-source机制保存来源，在正式压缩边界后重新引入有界证据。不要每次sampling临时删旧图、动态去重已发送历史或无限压缩重试。
3. 区分浏览器连续屏幕、用户上传的参考图、普通MCP图。不能把所有图片按“仅保留最新一张”处理；缓存命中也不表示图像无需视觉tokens。
4. 超限恢复最多使用明确有界次数；无法压缩/最新必要图本身不适配时给出明确错误或调整方案。保存失败证据，禁止错误终局变Completed。
5. 新增模型可见通知必须短小、有界，以core/context的ContextualUserFragment实现；新增可超过1K token的单项按仓库规则标P0并列额外手工审查，不能冒称已经通过。纯计量修复不应借此新增大段模型上下文。

完成标准：连续至少20次合成MCP截图能触发可观察、次数有界的压缩；最新必要图片和call/result语义正确，wire/context均受控；cancel、热resume、冷resume和压缩后的账目/来源正常。更长stress可用离线mock，不耗付费额度。

## 验收矩阵与工作边界

- 三wire × 用户图/直接MCP图/function/custom工具图/Code Mode嵌套图，实际HTTP捕获断言图片路径、MIME、call ID和下一请求。
- 小图、大图、同尺寸不同编码、单次多图、连续20次；文本超限、未知尺寸URL/file ID和传输字节边界。
- structuredContent+content图片、resource图片、未知附件、错MIME、invalid Base64；普通Text伪装图片不能绕预算。
- 正常turn、取消、热/冷恢复、正式compaction；unknown/text-only模型行为。
- Linux为主，在已有实机上跑定向离线场景；Windows验证原生图片路径、大小边界与恢复，不用WSL代验。ACP dev通过CODEX_PATH固定本轮新二进制。
- 选择器非零命中、实际请求非零；skip写未测。模型真正识图只能靠指定vision模型的实测，不能用mock PASS代验。
- 本轮默认离线，不调用付费API。真实识图待指定主用vision模型/endpoint和小批请求预算明确后单列；不重复13个文本协议矩阵，不复制已有密钥。
- 不升级Rig、不改V8 pin、不新增默认云存储/公网截图上传。复杂批尽量<500 changed lines，普通批<800；不要把三批塞进一个大提交。
- Rust测试用just test；完整workspace延续用户此前选择留到CI。按tests→scoped fix→just fmt→diff-check收尾，不在最终fix/fmt后重跑同批测试。配置/API/依赖/编译数据变动同步对应schema/TS/Bazel。
- 阶段git commit保存在test；不push、不dispatch、不发布包。报告具体SHA、二进制、执行数量、首败、修复范围和未验证限制。

## 后置：厂商附件上传的独立设计

只有指定厂商真实支持才实现opt-in File API，复用AttachmentStore边界。需要明确endpoint+auth-domain/账户+文件hash的作用域、TTL/删除、取消上传、冷resume、换provider、失败fallback、上传次数和字节硬预算。远程URL须由厂商实际可达，localhost/本地路径/用户网页登录态不能代替。未知附件按文件类型处理，不把二进制Base64当文字或图片转Document绕限制。

该优化减少重复传输量，仍计视觉预算；不是第一批修复的依赖，不必先造通用上传平台才能正常看截图。
