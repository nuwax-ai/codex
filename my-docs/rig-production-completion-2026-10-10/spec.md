# Spec：Rig 三协议 fork 的剩余功能与生产验收

日期：2026-10-10。审查基线：`test` / `6fea37c4c9384b52e7fbfb2055c6506c74e7f1f9`。本文件规定目标和边界；实现方式见 `plan.md`，执行项见 `tasks.md`。本轮交付是独立审查与完整后续方案，不是这些功能已经实现的声明。

## 1. 当前状态与总范围

已实现的主线：Rig 三协议路由、类型层请求投影、进程级 NUWAX 配置、请求预算、部分 owner/行政路径及历史来源隔离。第三方 `responses/chat/anthropic` 分别请求 Responses/Chat Completions/Messages；官方 OpenAI/Bedrock 和显式 native 的边界按源码保留。

剩余范围仍分 **8 包：2 个实质功能包 + 6 个稳定性/验收包**。拆成 **40 个执行单元**，不把包数、测试数换算为完成百分比。

| 原包 | 目标 | 当前边界 |
|---|---|---|
| 1 | cap 失败时保留 partial、实际 usage 与完整性 | 具体提案存在，功能尚未实施；7 个表示/兼容决策需要落成实际契约 |
| 2 | D6 pause 的可信 token 预算 | 只有设计；现行 40,960-byte 边界与 4 次续接保留；P0 人工门开放 |
| 3 | 产品缺陷、历史失败、冷态/并发稳定性 | 六批修复只收口部分路径；本次新增确认问题见 `review.md` |
| 4 | owner/行政命令完整生命周期 | 已有局部公共路径，剩余安装、启动、运行状态与崩溃矩阵 |
| 5 | 三协议字段/来源/历史兼容 | 已有投影与局部证据，缺完整跨协议、跨凭据与旧历史验收 |
| 6 | Linux/macOS/Windows、容器、异 OS | 当前本机切片不能替代原生平台与异 OS 验证 |
| 7 | 真实厂商三协议验收 | 历史收据不能升级为当前源码证据；Responses 真实触顶仍缺 |
| 8 | CI/Bazel/workspace 与发布准备 | 当前远端四个 job 均失败；未达到生产发布门禁 |

历史 **119** 是原 249 失败集合中未标恢复的不同测试身份，不是当前实测失败数。部分身份已有新窗口 PASS。另有 SIGABRT 和不在历史 383 表内的失败；都须纳入最终门禁。机器可读入口为 `historical-open-tests.tsv`。

## 2. 产品必须满足的契约

### S1：传输与证书

- `ReqwestDefault` 保持锁定依赖的显式 builder、HTTP/HTTPS/ALL_PROXY、NO_PROXY 和系统代理语义，包括 IP、localhost、IPv6、逐 hop 重定向。`RespectSystemProxy` 是另一策略，不能混用。
- 系统查询、证书加载不得阻塞 Tokio worker；阻塞任务有界、单飞、可取消等待。移动线程不能自动算作消除了系统调用的耗时。
- WS/WSS 支持声明范围内的 HTTP/HTTPS/SOCKS 代理；默认环境路径与显式路径使用一致的安全错误边界。错误类别保留，密码、代理 userinfo 和原始失败配置不进入 Display/Debug/日志。
- 新连接器的 native-root 来源在所有平台用加载器真实输入区分，覆盖 FILE/DIR、缺失/空值和非 Unicode。来源变化立即刷新，同路径变化有明确 TTL；旧连接器的不可变信任快照不追溯修改。
- 空加载不缓存；部分失败与解析拒绝有明确短 TTL；缓存失效/锁异常不得造成生产 panic 或永久信任污染。自定义 bundle 不进入共享根缓存。

### S2：guardian 与普通来源隔离

- R1b 必须恢复合法 guardian checkpoint 的投递，同时保留普通 resume/跨来源 opaque 降级。
- 许可由可信运行时产生，绑定 checkpoint、producer、目标 reviewer、目标 endpoint/auth/wire 和会话/请求生命周期。持久化 bool、通用哨兵或把 guardian 头加入全局 benign 白名单都不能授予无限重放权限。
- Luna post-answer 采样依赖工具开始、配置、checkpoint、授权与 generation 状态；工具开始不是请求必发的充分条件。失败/取消/替换必须有明确终局。

### S3：cap partial / usage

- 保持现行 cap 触发与不可重采样语义；截断工具不执行。每个 turn 恰一次失败关闭，不能合成成功 Completed。
- 提取厂商实际报告的计数，缺失不填 0；显式零、部分计数、完整计数和未知分别表示。多 response、retry、pause 的累计用稳定 host response 身份去重。
- legacy/paginated 均能保存、读取、resume 可见 partial；工具参数只作不可执行诊断文本。跨后续成功、attach、重连、压缩与恢复仍能查询 usage 完整性。
- 模型可见片段在 core/context 定义 struct，实现 ContextualUserFragment 并注册识别器/授权分类；不冒充人类指令。采集、序列化与完整 render 均有 byte/count/token 硬限。
- 新格式必须有实际旧读取器测试、开关和升级/降级规则。预算无计量证明时不得声称 token 限额 verified。

### S4：D6

- 新模式只允许 Exact 或可复核的 ProvenUpperBound，并绑定 provider/model/tokenizer/协议版本与完整实际载荷/framing。
- LegacyBytes 明确 unverified；未知模型、证明域外载荷或计量失败不得自动认证。
- KeepWholeOrFail；不得截断 signature、删除 opaque/search block 或改写旧 rollout。新增拒绝旧可发送内容的模式必须显式启用。
- 新fragment及VerifiedTokens模式的单项 >1k tokens 必须保留 P0 人工复核，>10k 禁止；保留的LegacyBytes仍unverified，不能声称已满足该token门，P0继续开放。40,960 bytes、bytes/4、样本分位数都不是 token 硬上限证明。

### S5：多进程、owner 与行政命令

- 每个生产 worker 使用独立 CODEX_HOME，并核对实际 sqlite_home 优先级；模型、凭据、协议、预算不串用，不污染 daemon 默认或 config.toml。
- queue 经实际 owner 写入；loaded 持 writer 至 unload。owner 存在时不启动第二 writer；崩溃/重启、队列恢复、名字歧义和分页有确定行为。
- 四命令、环境组四状态、CLI flags、安装来源、daemon/embedded/remote 和 cold/loaded/subscribed/running 的公共路径矩阵完整；客户端不把凭据配置隐式写到远端。

### S6：字段和能力

- 对照官方协议与 Cargo.lock 实际 SDK，逐字段记录保留、转换、拒绝、协议不支持及 SDK 限制。
- 覆盖 reasoning、工具定义/choice/parallel/results、cap、usage、错误、hosted tools、pause、citation、RawValue 字节/大数字及 terminal 顺序。
- first-party 专属能力、native WS 与 bridge GuardianV2 等能力差异明确表达。测试 fixture 选 native 不能证明 Rig 提供该能力；不支持不得静默假成功。

## 3. 兼容实施边界

推荐先实现可选择的新 cap 详细记录模式和 D6 可信计量模式，默认保留既有行为；具体可审表示和旧读端结果在编码前落实到 Plan。转交本方案后的开发可以自主推进安全的 opt-in 实现；默认迁移、扩大重放授权或发布必须单独裁决。

每个阶段完整可构建、可审查，普通非机械 diff <800、复杂逻辑 <500。旧提交不改写；新缓存功能先拆私有模块，core 只保留必要编排。

## 4. 全范围完成标准

1. 40 项都有真实实现/验收证据；无证据项保持开放，不靠修改 checkbox 关闭。
2. 历史119逐身份与 SIGABRT/新增失败有当前状态及归因；最终 workspace 零失败，skip/abort/0-match 单列。
3. 三平台原生测试、Bazel 实际三wire/bridges-out、容器和异 OS 通过；环境无法提供时列明阻断，不能算通过。
4. 当前源码与同一二进制的新 live 收据覆盖声明的厂商能力；端点不存在记录限制。
5. CI 实际执行通过；签名/打包/安装/升级/回滚和 secrets 检查满足发布范围。未执行发布操作不宣称已发布。

本轮未授权 push/发布或新增付费请求；完整离线测试、平台执行和所需额外授权的具体安排见 `claude-prompt.md`。
