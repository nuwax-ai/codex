# Plan：厂商凭据的技术改造

> 已被用户 2026-11-05 的新决定替代：采用用户级配置直接保存 `api_key`。
> 请以 [新方案](../direct-provider-api-key-2026-11-05/claude-code-prompt.md) 为准；本文件仅保留历史分析，不再作为开发要求。

状态：设计建议，类型、字段和 RPC 名称均为拟新增，不代表当前已支持。
配套：[Spec](spec.md)、[Tasks](tasks.md)。

## 已核对的复用点与缺口

| 位置 | 当前情况 / 改造方向 |
|---|---|
| tui/src/app/provider_wizard.rs | 逐步校验和 config/batchWrite；新增凭据方式、独立提交步骤 |
| tui/src/chatwidget/provider_wizard.rs | 现为普通 CustomPromptView；添加专用隐藏输入，预览只显示存储方式 |
| tui/src/bottom_pane/textarea.rs | 有 render_ref_masked，可复用渲染能力；不能仅给现有普通提示框改标题 |
| keyring-store/src/lib.rs | 已有 KeyringStore load/save/delete 和 MockKeyringStore |
| keyring-store/Cargo.toml | 已有 macOS/Windows/Linux 后端；Linux 无可用桌面服务需显式文件选项 |
| model-provider-info/src/lib.rs | api_key() 只读取环境；保持纯配置职责，不在这里做磁盘/钥匙串 I/O |
| model-provider/src/auth.rs | bearer_auth_for_provider 是主凭据拼装点，应接入统一的已解析凭据 |
| model-provider/src/models_identity.rs | 缓存身份也读取旧 api_key，必须随解析链一起改，防止换 key 仍用旧缓存 |
| model-provider/src/models_endpoint.rs | 模型发现也经 resolve_provider_auth，必须与推理共用凭据 |
| model-provider/src/shared_state.rs | 已有按运行时配置共享鉴权管理器的模式，可参考而不混用官方登录记录 |
| tui/src/chatwidget/model_popups.rs | std::env::var 置灰判断替换为后端状态 |

## 1. 配置只保存引用，来源显式互斥

拟新增字段示例（无真实凭据）：

```toml
[model_providers.deepseek]
name = "DeepSeek"
base_url = "https://api.deepseek.com/v1"
wire_api = "chat"
credential_ref = "keyring:opaque-id"
# models 按用户账号实际可用模型填写
```

环境模式继续使用 env_key；文件模式引用可为 file:opaque-id。
引用前缀明确选择后端，避免查不到钥匙串记录就自动回退到明文文件。
不要允许任意文件路径或将引用字符串视为访问授权。

credential_ref 与 env_key、experimental_bearer_token、命令主鉴权等互斥；
沿用现有模型鉴权组合校验，对官方/云平台保留 id 不扩展这条自定义 key 路径。
同时维护 strict config、schema、remote proto/转换及 round-trip 测试；只传
引用和能力，不传密钥。旧后端不支持引用时明确报兼容错误。

## 2. 统一 ProviderCredentialManager

建议作为独立模块，复用 codex-keyring-store 的抽象；不为本功能重写跨平台后端。
底层可替换为 KeyringCredentialStore / FileCredentialStore；测试注入内存实现。

内部职责：
- resolve(scope, source) → 私有凭据快照及版本。
- status(scope, source) → Ready / Missing / Unavailable，不返回 key。
- stage / commit / discard、replace / delete → 原子或可补偿的管理流程。

scope 至少包括后端用户/NUWAX_HOME 身份、provider、规范化目标端点和协议。
密钥记录保存此绑定；配置把 base_url 改到别的目标时，引用不能自动放行。
model_catalog_url 等附加接收凭据的端点也必须纳入显式绑定，避免只验证推理地址。

存储 I/O 放到阻塞任务或专用工作线程，不持有会话锁跨越 I/O。
解析后的 key 不塞回 ModelProviderInfo/Config，也不通过 set_var 或 env header
制造旁路；使用私有、Debug 脱敏的运行时快照传给已有 AuthProvider。

推理、原生 Responses、桥接协议、模型发现、doctor、状态查询共用来源与错误
分类。记录修改后失效相关缓存；活动轮次继续持有旧快照，下一轮读取新版本。
删除后新请求不得因陈旧缓存继续拿到旧 key。

## 3. 后端管理入口，TUI 不越过后端直接写凭据

拟新增受限的 app-server 能力：
- provider 凭据状态查询：仅返回状态/来源/不可访问原因。
- provider 保存或更新：一次业务操作提交 provider 定义与新凭据。
- provider 凭据替换/删除：有明确目标和版本，不提供完整 key 读取接口。

名称可采用 modelProvider/credentials/*，最终遵守仓库现有 RPC 命名与版本约定。
状态能力与写入能力分开协商；可信同用户本机通道才支持第一期写入。
远程连接、云后端、旧后端返回明确 capability，不因是 WebSocket 就自动允许。

普通 provider-only 编辑继续走 config/batchWrite；携带 key 的新增/更新走专用
入口。运行时解析下沉，使 CLI exec 和 ACP 不依赖是否经过 TUI。

## 4. 保存与失败恢复

钥匙串和配置文件没有跨存储事务，需要显式补偿：
1. 校验 provider、端点、来源、权限与配置预期版本。
2. 写入新的凭据记录/id，不覆盖旧的有效记录。
3. 用 CAS/已有配置版本机制原子提交仅包含新引用的 provider 定义。
4. 配置提交失败，删除新暂存记录；保留旧引用/旧凭据，报告清理失败而不吞错。
5. 成功后刷新后端配置/凭据状态并通知 TUI，再清理确认无人引用的旧记录。

钥匙串暂不可访问与记录不存在必须区分。锁定/权限拒绝不触发自动文件回退。
文件后端创建受限目录与文件、拒绝宽松权限和不安全的路径替换，原子 rename；
Windows 用用户 ACL。复用仓库已有安全文件写入工具，具体 API 实施时再核验。

## 5. 密钥输入与保密边界

使用专用 SecretInput 状态和掩码显示，不进入普通聊天 composer、输入历史、
粘贴预览、模型提示词和 TOML 预览。取消/保存后释放临时秘密状态。

特别注意 utils/redacted-string：RedactedString 只遮盖 Debug，其 Serialize
仍输出原值；SerdeRedactedString 会固定输出占位符，又不能用于真实提交。
因此专用写入 RPC 的线格式需真实传递 key，但 tracing、原始 JSON 记录、
错误上下文、反馈打包、快照路径必须在各自边界剥除该字段，不能仅依赖 Debug。
使用假 marker 测试所有输出面；响应绝不回显完整 key。

## 6. 列表和向导交互

- 向导默认“直接输入密钥”，环境变量收进高级选项。
- 缺少 key 的模型行可进入“配置密钥”，不会直接切换/发送请求。
- 后端凭据保存成功后推送状态或重新查询；当前 TUI 不重启即可刷新。
- 当前轮在执行时，凭据保存和模型选择是两个动作：保存不会打断当前轮，
  选择继续使用最后一次意图、轮次结束后应用的既有机制。
- Ready 只表示凭据可解析。测试连接单独执行与展示，不以 /models 发现失败
  判定推理接口不可用。

## 推荐交付顺序

A. schema + 统一凭据解析 + 存储与隔离测试（先做底座）。
B. 受限后端管理入口 + 保存补偿 + 缓存/轮次快照（贯通执行路径）。
C. TUI 隐藏输入 + 状态/修复入口 + 保存后可用（完成用户体验）。
D. CLI/daemon/ACP 验收与文档；远程管理另行设计，不随第一期默认开放。

不建议只做“向导自动改 agent.env + 多线程 set_var”：它不能一致更新别的进程，
且引入环境修改、生命周期与凭据管理的第二条路径。也不建议把 key 写进现有
experimental_bearer_token，借配置序列化和预览链路传递。
