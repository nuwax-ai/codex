# 给 Claude Code 的提示词

在 `/Users/soddy/Documents/git-rust-work/fork-codex` 继续稳定性开发。基线 checkpoint 是 `ead6f2bcae3e0cac4a8374708e9dc1d1d31296a6`，但工作树还有 Claude R1–R4 修复和 Codex 补充测试，先记录实际 HEAD/diff，保留这些改动及其他人的工作。

先读 AGENTS.md、`my-docs/codex-review-2026-10-04.md`、本目录 `spec.md`、`plan.md`、`tasks.md`。按 A→B→C→D 顺序实施；不要只重复跑旧测试便声称开发完成。每批更新 tasks 中的证据、相关源码摘要、feature、selected/executed/asserted 与失败/跳过项。

优先完成 A：所有 query 值不进入持久化 endpoint 摘要，实际完整 URI/query 与每轮非良性 extra headers 在私有有界 scope 比较，旧来源 fail-closed，保留可见文本及 append-only 历史。不要只合并 denylist 后继续猜测凭据名。明确 source 版本兼容。

随后 B：行政名字查询跨 provider 并拒绝歧义；queue 遵守 owner server、禁止第二 writer；不要仅改 daemon exclusion 就认定完成。UUID enqueue 不代表随后模型执行成功。补同 provider echo resume 的真实 RPC/HTTP 回归。

再完成 C 的进程级 retry/idle 三个环境变量，按现有 EnvSeed 优先级、非 Unicode fail-fast、daemon 剥离和三协议真实 mock 请求计数验收。最后补 D 的 feature 门禁、live capture 字段断言与三协议输出触顶语义测试。

使用隔离 CARGO_TARGET_DIR 和 `just test`，Core 单包桥测试显式 `--features rust-rig`；不直接 cargo test，不例行 all-features。先相关测试，最终 scoped just fix/just fmt/diff-check，fmt 后不再跑测试。完整 workspace 按 AGENTS 另行确认。

真实厂商只用 `.env.local` 中已有配置，密钥不回显、不写工件；先通过离线 gate，按需要做少量已有授权 MiMo/GLM 请求，不盲跑付费扩展矩阵。源码改变后重建 exec，独立核对收据和同一 executable；旧 live 7/7 的 wire_asserted=false，不能转述为鉴权/字段断言完成。mock、live、跨平台、CI 分开汇报。

不要自动 commit/push/发布，不清理运行时数据库或 tmp，不禁用 TLS，不扩大测试 deadline，不改 V8 pin。最终提供实际完成任务、相关测试、未验/阻断项和最小剩余阶段。
