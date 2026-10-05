# 另一台电脑继续工作的入口（2026-10-05）

仓库：`git@github.com:nuwax-ai/codex.git`；工作分支：`test`。本轮目标是将全部已审查源码/测试/文档 checkpoint 推到 origin/test；精确结果见同目录 `codex-review-push-2026-10-05.md` 与最终聊天回执。

## 安全同步

先 `git status --short`。另一台电脑有修改时先保存、审查，不 reset/clean/stash 覆盖它们。工作区干净时：

```sh
git fetch origin test
git switch test
git pull --ff-only origin test
git log -1 --format='%H %s'
```

若尚无本地 test，用 `git switch -c test --track origin/test`。出现分歧停止同步并检查，不能强制覆盖。文档所属 checkpoint 可用 `git log --diff-filter=A -1 --format=%H -- my-docs/other-computer-handoff-2026-10-05.md` 定位；最新 HEAD 另记录。

## 可直接给 Claude Code 的任务

先读 AGENTS.md、`my-docs/codex-review-push-2026-10-05.md`、`my-docs/rig-stability-followup-2026-10-04/{spec,plan,tasks}.md`、`claude-acceptance-prompt.md`。以实际 HEAD 为基线，复验已经推送的修复，再按下列顺序推进：

1. **新电脑验证基线。** 选择相关模块和公共 CLI/RPC/环境矩阵；Core 单包桥测试明确 `--features rust-rig`，检查实际匹配数。旧 375/604、历史 live4 不代替本机结果。
2. **warm/cold/低载。** 新修复用真实 gate 证明 running，不恢复三个请求的错误 fixture。不扩大 initialize deadline；真低载冷/热对照及 loaded 无订阅/另一连接订阅、running/bare 对照单列。Linux 可用环境的验证与 macOS 分开。
3. **B2/B3/C2 剩余。** 补实际 owner 缺失/embedded/running、不同 active client 环境、禁止第二 writer、四命令 corrupted/inactive、行政启动矩阵；SSE-wait 取消、Windows native Ctrl-C、并行 idle 值的实际观测。
4. **cap/usage/partial 契约。** Core cap 不重采样已有测试；保留 partial text/usage/Done 的产品行为需先更新独立 Spec/Plan 和兼容界面，不能靠合成成功 Completed 解决统计。
5. **D6 方案。** 只接受覆盖整个实际载荷/framing 的 exact 或证明上界；无证明明确 legacy/unverified。未经产品裁决不改变 40,960-byte fail-fast 或删签名、搜索块。
6. **额外门禁。** workspace 全套遵守 AGENTS 单独确认；本地平台、Bazel build、Windows/Linux、CI、Step、加密引用、跨进程回放、真实 live cap 触顶逐项记录，未执行不算通过。不要自动 dispatch、发布或盲跑付费压力矩阵。

开发发现确定局部缺陷直接修复并加公共路径回归；不要只重跑旧测试/修改任务状态便宣布完成。最终将 selected/executed/asserted/pass/fail/skip/timeout、完整命令、源码/二进制身份、首次失败及复验写入 `my-docs/other-computer-validation-results.md`。

## 环境与工件

- 本机 `.env.local`、SQLite、tmp、Cargo target、原始 logs 不通过 Git 传递。新机独立配置凭据，禁止将密钥贴进聊天/报告/commit；每个生产进程独立 CODEX_HOME，确认有效 sqlite_home 来源。
- 历史 live 脱敏元数据在 `live-cap-historical-evidence-2026-10-05.json`，包含四次通过与早期失败、两份源码；不是当前 HEAD 或新机的 live 证明。
- 用隔离 CARGO_TARGET_DIR，保持阶段 package/feature 图稳定。不直接 cargo test，不例行 all-features。缺 helper 二进制按仓库方法构建；不要以测试过滤/运行时跳过冒充覆盖。
- macOS V8 setup 采用仓库 pin 与校验过的配对工件流程，读 `third_party/v8/README.md`；临时路径不能从本机复制使用，不禁用 TLS，不改 pin 求绿。
- 新 live 前重建 exec，核对当前 HEAD/source digest/同一 executable SHA 与 receipt；提交会改变 HEAD/输入集合，提交前收据不能自动升级成提交后证据。
- 相关测试先运行，最后 scoped just fix、just fmt、diff-check，之后不再跑测试。改依赖或 API/config 类型同步相应锁/schema。
- 此交接不额外授权新一轮 commit/push/发布；完成验证后先给出实际结果、剩余项和按依赖划分的提交建议。
