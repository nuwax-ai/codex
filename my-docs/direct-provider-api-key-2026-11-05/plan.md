# Plan：直接 API Key 的最小完整实现

状态：待开发；字段/API 扩展均是提案，不表示已经实现。
配套：[Spec](spec.md)、[Tasks](tasks.md)。

## 一、复用现有路径，避免另建凭据平台

已核对的基线入口：

| 层 | 路径与改造点 |
|---|---|
| provider 配置 | codex-rs/model-provider-info/src/lib.rs：ModelProviderInfo、validate、api_key() |
| 主鉴权 | codex-rs/model-provider/src/auth.rs：bearer_auth_for_provider / resolve_provider_auth |
| 鉴权分类 | model-provider/src/runtime_auth.rs、provider.rs、models_endpoint.rs、models_identity.rs |
| 核心来源 | core/src/client.rs、session/token_budget.rs、model_output_projection.rs |
| 后端管理 API | app-server-protocol/src/protocol/v2/model_provider.rs；已有 modelProvider/list 与 modelProvider/config/write |
| 后端写入 | app-server/src/config_manager_service_provider_write.rs、config_manager_service_model_providers.rs |
| TUI | tui/src/app/provider_wizard.rs、chatwidget/provider_wizard.rs、chatwidget/model_popups.rs |
| 配置写入 | core/src/config/edit.rs；既有 config/batchWrite 事务、版本与保存路径规则 |
| 其它入口 | cli/src/doctor.rs、config/src/thread_config/remote.rs、配置 schema/strict 检查、相关生成物 |

不要只在 TUI 给 api_key 赋值。上述鉴权存在性判断和公共配置投影必须一并走查。

## 二、配置与鉴权解析

新增 api_key: Option<RedactedString>，保持旧文件默认 None。字段和现有 api_key()
方法可同名；如调整方法名，必须迁移全部调用而不是留下 env-only 的旁路。

统一解析规则：
1. 配置加载时校验新 api_key 来源的互斥关系、类型、空值，错误不包含原值。
2. 显式 api_key → 返回对应私有鉴权值。
3. 未配置 api_key 而配置 env_key → 走原有环境读取与错误语义。
4. 未配置两者 → 沿用原有 legacy bearer / command / 官方鉴权等路径。

可以增加无副作用的 has_static_api_key_source / credential_source 描述，统一
替换散落的 env_key.is_some() || experimental_bearer_token.is_some() 判定。
它描述配置来源而非探测网络，也不能读取失败后回退到官方登录或另一把 key。

继续由现有 AuthProvider 和 rig/native 传输拼接鉴权头，不在桥里新增厂商特判。
Chat/Responses/Anthropic 均须覆盖；检查 catalog 缓存身份、不可变凭据头快照与
跨厂商 reasoning 来源判定，新增 key 不得错误继承环境中的 OpenAI 身份。

## 三、区分保存序列化与公开投影

RedactedString 的 Debug 已脱敏，但 Serialize 是原值；仅换包装类型不够。
SerdeRedactedString 则始终序列化占位符，不能用于真实写入。

建议将 api_key 的通用 ModelProviderInfo Serialize 设为跳过，磁盘保存由明确的
TOML edit 写入真实值；实施前核验所有依赖该类型序列化的路径。公开元数据只
返回 source/configured 状态，不使用可被保存回去的“******”伪 key。

这个建议不能替代边界审计：raw TOML/JSON 配置层并不经过 ModelProviderInfo。
必须同时处理 config/read、inspect、doctor、报错、反馈导出和原始 RPC tracing。
新增/编辑 API 的真实 key 仅在输入、受限传输、写入端解包，日志记录其操作意图。

thread/start、thread/resume、配置快照和 rollout 不得因为序列化整个 provider
而持久化 key。常规会话请求选择 provider id，由执行后端读取自己的用户配置。
remote proto 不新增明文 key 公共字段；若某条内部执行链确实依赖配置传输，
必须显式设计仅内存的受限通道并验证不落历史，不能静默省略后当成免鉴权执行。

旧 experimental_bearer_token 也不得因新增向导/导出路径被回显，但不要求本轮
把所有旧用户配置迁移成 api_key。

## 四、扩展已有管理 API，形成明确的写入意图

优先复用已有 modelProvider/list 和 modelProvider/config/write，而不是再造一套
credentials 平台。扩展其自定义 provider 参数，以支持 api_key、wire_api、models
及 P4 已有字段，保持其它现有客户端行为。

密钥 patch 采用三态：缺省=Keep、明确新字符串=Set、明确 null/移除动作=Clear。
环境来源与无鉴权选择同样转换成明确的字段删除；沿用或补齐实际 TOML 删除 API。
不能把“省略 env_key”误认为会删除已存在字段——当前 P4 config_edits 恰有此隐患。

写入端统一校验来源冲突、用户层所有权、托管限制、实际生效 provider 和版本，
在一次原子编辑里完成来源切换，保留不相关设置。响应为脱敏摘要和配置版本。
如复用 config/batchWrite 作为内部落盘机制，仍须给敏感字段的请求/错误/输出
建立脱敏边界；不能认为 generic ConfigEdit 的 Debug/JSON 默认就是安全的。

ModelProviderSummary / GatewayAuth 等摘要增加直接 key 的配置状态，不返回原值。
更新 has_external_auth、supported_fields、can_edit 策略：不能把新正规 api_key
误分成不可编辑的外部鉴权。不要绕过原有托管/内置 provider 限制。

旧服务器不支持扩展时明确提示升级或使用旧 env_key；未知 auth mode 不能按 None
处理。第一期不把远程配置写入当成本机凭据自动同步能力。

## 五、保存后刷新与 key 轮换

新增 provider 首次选择：保存→后端重新加载用户配置→列表状态刷新→既有显式
model/provider resume，实际请求读取后端文件中的 api_key，TUI 不回传整份秘密配置。

修改当前 provider 的 key 需要额外处理，不能只刷新菜单：
- 写入成功标记相关 provider 配置版本已改变。
- 空闲会话下一次采样前，重新构建其有效鉴权快照；当前执行中的轮次继续持有旧快照。
- 优先复用现有 idle unsubscribe/re-home/cold-resume，必要时增加受限的 provider
  配置刷新原因/版本，不能通过随便切到别的模型再切回来制造刷新。
- 已加载线程的 resume 比较不能只看 model/provider 名称。当前
  collect_resume_override_mismatches 不比较静态 key，必须识别显式刷新时的
  provider 定义/私有鉴权版本变化，避免“同名所以忽略新 key”。
- 不记录 key/可复用凭据摘要到日志或 rollout；对外版本只表示配置版本。
- 不强制接管其它客户端订阅的线程；无法应用时说明“已保存，当前会话未应用”
  并暂停自动发送需要新鉴权的队列，不能继续发旧 key 却显示应用成功。

跨 provider 的待切换仍复用 PendingModelSwitch，最后一次选择优先，正常完成/
停止完成后应用；新增 key 的写入和模型选择分别处理，不能轮中替换路由。

## 六、TUI 与文件保存

凭据步骤三选一，默认“直接输入 API Key”。专用隐藏输入复用 textarea 的
render_ref_masked 能力；不用普通 CustomPromptView 回显 key，不进聊天历史、
补全、粘贴预览和 TOML 预览；取消/完成后释放输入临时状态。

编辑旧 provider 时显示“已配置密钥”，默认 Keep；替换/移除需要用户明确动作。
预览仅展示普通字段和“保留/更新/删除密钥”。保存成功后不再提示 export $KEY。

/model 使用执行后端的摘要状态，不再只做本地 std::env::var；缺失行可打开配置，
状态可用只代表来源可解析，不能显示为“实网验证通过”。

保存沿用活动用户配置/profile 文件位置，禁止项目级目标；Unix 文件 0600，
临时文件与备份同等级，原子替换，不跟随不安全文件替换；Windows 用户 ACL。
对既有只读/非用户层字段 fail-fast；不修改 shell 配置，不调用 set_var。

## 七、实现顺序与复杂度

1. 配置字段、统一鉴权和序列化边界，先用假 key 跑三个协议的真实 loopback 请求。
2. 类型化管理 API、原子编辑/删除、状态与运行时刷新。
3. TUI 隐藏输入、Keep/Set/Clear、即时列表刷新与已有轮次机制衔接。
4. 定向回归、最小真实模型验收（有已授权凭据时）、文档、构建与本机安装。

属于跨配置/鉴权/API/TUI 的中等改动，主流程不需要新钥匙串或存储 crate。
不能用只改一个输入框或只写文件的演示当作多 provider 已可用。
