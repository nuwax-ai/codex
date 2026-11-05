# Claude Code 开发交接：TUI 直接保存多 Provider API Key

请在 /Volumes/zhitai/git_workspace/fork-nuwax-codex 开发并完成下面的最小完整能力。
本交接由用户明确要求生成，目标是让用户先正常使用多 provider，不再被手工
环境变量配置阻塞。请实际实现、验证并交付，不只给分析。

## 先读与范围

依次读取：
1. 根目录 AGENTS.md、NUWAX-HARNESS.md、FORK.md。
2. 本目录 spec.md（做什么与边界）、plan.md（技术改法）、tasks.md（执行验收）。
3. 当前源码与适用的子目录 AGENTS.md。文档记录的初始基线为 test@dc2fdbd5b4，
   不保证你开始时未变化，先看 git status / log，保留其它任务的改动和快照。

用户已经选择：允许真实 API key 明文保存在用户级配置；TUI 默认直接输入 key，
环境变量为可选高级模式。AGENTS.md 旧“只走环境变量”条款与此次指令冲突，
此次用户指令优先，实施时同步更新条款，不要再次要求确认这个已确定的选择。

旧目录 ../provider-credential-management-2026-11-05 是已被替代的钥匙串方案。
本轮不做 credential_ref、独立凭据库或系统钥匙串，不自动修改 shell/env。

## 必须交付的用户闭环

1. /model → Manage providers → 新增/编辑 provider。
2. 选择直接 API Key，在隐藏输入框粘贴 key；保存进该后端的用户级 config.toml。
3. 配置使用正式 api_key 字段，预览与所有公开输出只显示已配置状态。
4. 保存后当前 /model 刷新，无需 export/重启即可切换；重启后继续可用。
5. 多 provider 各用自己的 key；执行中多次选择只保留最后一次，正常完成或停止
   完成后再切换。换 key 也不能改动当前轮的鉴权快照，下一轮使用最新配置。

## 容易漏掉的实现点

- 已有 experimental_bearer_token 可参考鉴权接线，但新向导写正式 api_key，
  不把旧实验字段当成新的产品配置名；旧配置兼容保留。
- 统一 api_key()/AuthProvider 和所有显式鉴权判断；不只改 TUI 置灰或 HTTP 某一协议。
- 已有 modelProvider/list、modelProvider/config/write 可扩展复用。P4 当前仍走
  config/batchWrite，需把隐藏输入、管理写入和脱敏状态贯通，避免两套语义。
- Keep/Set/Clear 必须区分；从 env_key 改为 api_key 要真正删除旧字段，省略不会删除。
- 不能覆盖不相关 provider 设置，也不能绕过托管/只读层或旧服务器能力限制。
- RedactedString 仅遮盖 Debug，Serialize 仍会输出原值。仅换该类型不等于不会
  从 config/read、raw JSON、错误摘录、反馈包、会话参数或 rollout 泄漏。
- 磁盘保存允许真实 key，普通配置/协议/历史投影必须脱敏或剥离；不能把掩码
  占位符再写回磁盘，也不能因剥离而在执行路径静默丢失鉴权。
- 同名 provider 的 key 变化不能被 loaded-thread 的“名称没变”判断忽略；检查
  collect_resume_override_mismatches 与实际运行时配置刷新，确保下一轮用新 key。
- 密钥输入不进普通聊天历史、粘贴预览或 TOML 预览。Unix 配置/临时文件/备份
  权限受限；原子写失败不破坏旧配置。

## 验证与交付要求

遵守 SOLID / Fail Fast；生产代码不引入 unwrap/expect/unsafe，不使用 set_var
作为多线程凭据注入捷径。优先 cargo nextest；Mac core 集成用
RUST_MIN_STACK=33554432；不宽过滤更新 insta 快照。

请用假 key 和 loopback HTTP 测试验证实际鉴权头、三协议、多 provider、保存与
重启、来源切换、key 轮换与缓存、轮次边界、错误/日志/导出无泄漏。
如实际环境阻塞，明确列出已通过和未通过，不能用模拟结果替代真实验收。

定向测试通过后编译 CLI，按 AGENTS.md 独立 root 安装流程更新本机 nuwax-harness，
保留旧二进制备份并验证 command -v / 版本 / 新二进制来源，不覆盖官方 codex。
源码构建版本仍为 0.0.0，不能仅凭版本号宣称已更新。
不要自动发布 npm、部署远程服务或 push。不要把用户实际 key 写进测试、文档或提交。

完成时报告：改动文件与行为、测试命令/结果、构建与本机安装状态、未完成项和
需要用户知道的真实限制，并同步更新本目录 tasks.md。
