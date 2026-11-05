# Tasks：直接 API Key 开发与验收

全部待实施。具体约束以 [Spec](spec.md) 为准，做法参考 [Plan](plan.md)。

## T1：配置与鉴权

- [x] ModelProviderInfo 增加 api_key；新 api_key 的互斥校验不回显值，空值 fail-fast。
- [x] 保留 env_key、旧 bearer 和现有二级网关鉴权的兼容语义。
- [x] 接入统一解析，更新 runtime auth / provider / catalog / core provenance 等
  所有“是否显式鉴权”判断，原生与桥接均使用相同来源。
- [x] 更新严格配置、schema、必要的转换和初始化；不把明文 key 扩散到公开 proto。

验收：假 key 不依赖环境变量即可通过三个协议的 loopback 服务；检查实际鉴权头，
显式来源缺失/冲突不落到另一把 key；两个 provider 不串 key。

## T2：保存、脱敏、状态

- [x] 扩展已有 modelProvider/config/write 与 modelProvider/list，补齐 wire_api/models。
- [x] 实现 Keep/Set/Clear；来源切换在同次编辑中真正删除旧互斥字段。
- [x] 维护托管/只读/内置 provider 限制和其它配置项，不覆盖并发写入。
- [x] 用户级文件受限权限、原子保存；失败保留原文件与原 key。
- [x] 公开配置、摘要（config/read 层视图脱敏）统一脱敏；doctor/反馈导出经同一 read 路径。
- [x] 验证通用序列化、thread/start/resume 和 rollout 不携带 key；不能仅测试 Debug。

验收：假 secret marker 只出现在预期私有文件/请求中，公开输出与历史均不出现；
Keep 不写掩码字符串，Clear 不残留旧来源；保存故障、配置竞争和托管层冲突可恢复。

## T3：生效与运行时更新

- [x] 新 provider 保存后后端配置/列表可见，当前 TUI 无需重启即可切换。
- [x] 同名 provider 换 key 可识别版本变化（provider_definition_drifted），下一轮真正使用新 key，而非仅变更菜单。
- [x] 执行中保持原快照（复用既有轮次边界），完成/停止后才应用；切模型多次只保留最后一次选择。
- [x] 复用 PendingModelSwitch 既有抑制依赖新路由/新 key 的排队输入。
- [x] 同后端共用（daemon/ACP 走同一 app-server 配置）旧/远程后端不静默假成功。

验收：mock 记录第一轮头为 key-A，轮中保存 key-B 后第一轮仍为 A，下一轮为 B；
同时检查模型/provider、历史、草稿与队列，不能只断言底部文字。

## T4：TUI

- [x] 默认直接 API Key；环境变量高级选项保留，另有显式无鉴权。
- [x] 新增专用隐藏输入（masked '*'）；编辑经 GatewayAuth::DirectApiKey 状态显示已配置，Keep/Set/Clear 意图清楚。
- [x] 预览为圆点状态行；错误不回显；保存后刷新内存配置。
- [x] /model 缺失行打开配置沿用 P4 入口，已配置与已验证连接分别呈现。

验收：无任何相关环境变量，从 TUI 新增第二 provider→保存→切换→最小请求成功；
退出重开仍可用。存在旧 env_key 时切来源能正常完成，不留冲突字段。

## T5：验证与交付

- [x] 新测试统一以 direct_api_key 命名（loopback + service + 轮换），便于窄过滤；覆盖真实 loopback HTTP，
  不以仅构造字段对象替代请求合同。
- [x] 覆盖空 key/冲突/多 provider/无鉴权/替换删除/权限/层视图脱敏（写失败恢复沿用 wizard 重试路径）
  并发与写失败、受限文件权限、日志/错误/导出泄漏、活动轮次和旧配置兼容。
- [x] cargo nextest 定向运行；Mac core 测试设置 RUST_MIN_STACK=33554432。
- [ ] 编译实际 CLI；若有用户已授权实网凭据，只做最小请求并单独报告来源/版本/结果，
  不将 key 复制到仓库或把额度/网络错误伪报成鉴权失败。
- [x] 更新 AGENTS.md、FORK.md、NUWAX-HARNESS.md 与本记录。
- [x] 定向验证通过后按独立 root 流程更新本机源码安装（见下），备份旧二进制并验证实际路径；
  不覆盖官方 codex。不自动发布 npm 或推送。

## 第六轮复审修复批与定向复验（2026-11-05）

修复提交：`d5b4da1c39`（续接前五轮的复审修复）、`c5500a3adf`（补交 DirectApiKeyGatewayAuth 的
python codegen 产物）。三处行为/边界变化：

1. **刷新"已生效"判据**：`widget_generation` 只在 `replace_chat_widget`（唯一 swap 点）自增；
   resume 包装函数入口的自增已删——它在终端清除失败时先自增后失败，会给出假的"已生效"。
   判定恢复为 `result.is_ok() && bump && thread 匹配`（执行中排队路径与保存后空闲路径共用）。
2. **类型错误脱敏收口**：`format_config_error` 与 `ConfigLoadError::Display` 两处都遮
   （后者经 `io::Error::to_string()` 进 app-server 的 invalid-config 告警）；规则改为
   "`, expected` 之前、受引号包裹的第一个片段"，覆盖 serde 的字符串双引号渲染与
   `unknown variant`，不再误遮 `expected one of \`a\`, \`b\``。
3. **闸门与杂项**：本批范围内 config/app-server/tui 的 deny 级 clippy lint 清零（含
   `then_some`、needless borrow、redundant closure、`expect_used`、`print_stderr`），
   并删除模型切换测试里遗留的 `eprintln!` 计时输出；bedrock picker 快照按 P4 向导补行。

### 定向验证（只跑改动模块，未跑全量 workspace）

| 命令 | 结果 |
| --- | --- |
| `cargo nextest run -p codex-config -p codex-model-provider-info`（整包） | **481/481** |
| `cargo nextest run -p codex-app-server -E 'test(/direct_api_key|provider_write|config_warn|invalid_config/)'` | **16/16** |
| `cargo nextest run -p codex-tui -E '…wizard/refresh/replace_chat_widget/model_picker/rehome/config_errors/preference…'` | **51/51** |
| `cargo clippy -p codex-config -p codex-model-provider-info -p codex-app-server -p codex-tui --all-targets` | **0 error** |
| `rustfmt --check`（本批改动的全部 .rs） | **0 diff** |

新增测试：`config/src/diagnostics_tests.rs` 9 例（整数/布尔/双引号字符串/`\"` 转义/
`unknown variant`/期望值保留/无值原样/幂等/`ConfigLoadError::Display`）；
`tui/src/app/tests.rs::replace_chat_widget_bumps_generation_and_clears_credential_refresh`
（每次替换恰好 +1、替换清 pending）。

**排除项（均非本批引入，同样不能算本批通过）**：

- `app::tests::session_lifecycle_requests::model_picker_rehomes_displayed_thread_and_updates_route`
  仍超时：即文末"基线环境注意"记录的 macOS FSEvents 真集成阻塞；改动前的 16:57 全量过滤跑
  同样超时，有基线对照。
- `app::tests::backend_sessions::backend_sessions_poll_three_sources_and_isolate_closed_or_paused_streams`
  在 nextest 60s×2 预算下超时；去掉时限以 `cargo test` 单跑 **90.48s 通过**，属本机负载
  （load avg≈11）下的慢测。

**仍未做**：T5 最后一条（编译实际 CLI + 可选实网最小请求）保持未勾。本轮未构建 CLI、
未做任何厂商实网调用；上面的数字全部是 nextest/clippy 的本地结果，不能当成实网或
"凭据已在真实网关生效"的证据。

基线环境注意：此前真实 app-server 恢复测试曾阻塞于 macOS FSEvents 注册。
若再次遇到，采样确认并单独报告；协议模拟、真实 app-server、TUI 和厂商实网
证据分别列出，不相互冒充。不要扩大快照更新范围去吞掉其它工作区变更。
