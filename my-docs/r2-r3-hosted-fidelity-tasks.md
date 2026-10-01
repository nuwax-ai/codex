# R2+R3 Tasks：hosted 搜索历史保真与暂停续接（任务文档）

对应 Spec：`r2-r3-hosted-fidelity-spec.md`；Plan：`r2-r3-hosted-fidelity-plan.md`。
每批独立可构建、附精确命令与证据；完成打勾并链接证据。

## 批次

- [ ] **B1 envelope 载体与来源门禁**：`hosted_replay.rs`（解析/门禁/上限/
  形状/去重的纯函数 + 单元测试）；`web_search_call_events` 带 source 写
  envelope；`request_messages` 走 envelope，不可回放对跳过。回归：旧版
  裸数组降级、source 不符降级、65 对/超字节/坏形状请求侧拦截、降级后
  assistant 非空无悬空。
- [ ] **B2 暂停原样续接**：`raw_assistant_content` 全块重组；暂停分支
  `(items, raw_content)`；transport 替换式 splice；`assembled_text` 移除。
  回归：thinking+签名→搜索→结果→带引用文本→pause 的请求 2 content 深度
  相等；仅 thinking pause；工具数组不变。
- [ ] **B3 跨响应配对与去重**：completed 注入的未配对 result 回配请求输入
  的 in_progress call（追加完成条目）；请求构建同 id 去重；pump 跨 attempt
  暂存。回归：两请求混合轮（client result 先回）、乱序 id、多对。
- [ ] **B4 引用持久化**：citations 捕获进 envelope `cited_text`（completed
  轮 + 暂停轮）；回放按位拼回。回归：带引用文本的保存与回放深度相等；
  无对可挂时丢弃并警告。
- [ ] **B5 core/resume 回归与断言加固**：envelope 格式下 resume/fork 请求
  副本清空、rollout 前缀保持；既有 pause/hosted wire 测试的 contains 断言
  升级为整体相等；`hosted_tools_tests` 自比较改为独立期望值。

## 完成标准（每批通用）

- `just test -p codex-rust-rig-bridge [--offline --locked --retries 0
  --test-threads 4]` 全绿（新增用例列出实际执行数）；涉及 core 时加跑
  `just test -p codex-core --features rust-rig ... -E '<选择集>'`。
- `just fix -p codex-rust-rig-bridge`（core 变更时加 `-p codex-core`）后
  `just fmt`；fix/fmt 后不重跑测试（AGENTS.md 顺序）。
- 每批 <500 行、独立提交（已获用户授权的阶段性提交）；提交信息注明批号。
- 完成后更新本文件勾选与 `codex-review-2026-10-01.md` 的 T02/T03 状态；
  B5 后按 T06 做最小 GLM hosted live 复验（envelope 格式 turn2 接受性）。

## 已知边界（登记，不在本簇解决）

- 未知 delta 类型的块无法忠实重建（按基底降级 + 警告）。
- citations 的用户可见映射（协议/TUI 表达）为后续工作。
- bytes/4 仍是估算 token 上限（不称精确）；64 对与 40,960 字节为请求侧
  硬上限的真实语义。
