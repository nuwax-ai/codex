# 给 Codex 的检查提示词(2026-10-22 晚,终版)

你接手检查 nuwax fork `test` 分支今日全部产出。唯一源码仓库
`/Volumes/zhitai/git_workspace/fork-nuwax-codex`;**已 push**:
`origin/test@850a87390`(复审接手时核实;代码区间
`29e7e59d5..c17b8675b`,含 main 集成 merge `a5b25b043` 与其后 11 笔提交)。

权威索引:台账 `claude-rig-final-acceptance-2026-10-18.md` §6.54-§6.57;
路线图 `my-docs/ROADMAP.md`;fork 边界 `my-docs/FORK.md`。

> 后续独立复审：本页是推送时交接。run39761410573现已结束，三平台在lint/测试编译
> 阶段失败（无workspace测试计数），Bazel通过；修复及最终证据以
> [晚间复审](codex-review-2026-10-22-late-audit.md)、台账§6.58与ROADMAP为准。

## 原推送时状态快照

- **主线(国内模型接入:OpenAI Chat/Responses/Anthropic 三协议)全绿**:三协议
  实网 9 用例 + acp-workflow 12/12(首败=预算饥饿,已同断言重跑关闭)、rcoder
  容器腿 3/3(无残留进程)、离线补跑 215/215、双平台二进制收据在册。
- **合并树首个 CI**:run `39761410573`(Fork Cargo PR,push 自动触发,c17b8675b),
  后台监控中。判读基线:旧 ubuntu 红(blocked_descriptor_path)本轮应转绿;
  windows 六红按 §6.57 判(uds 两条经硬化待验;write-delete 预期仍红,用户决策
  B+警告不修);**超出该名单的任何红=新回归,须分诊**。
- **CI 费用核查(用户要求)**:fork-cargo-pr/release/live-tests 全在标准 hosted
  runner;仓库 PUBLIC → 免费。唯一含付费规格(macos-15-xlarge/intel)的
  `rust-release-provisioned-macos.yml` 是孤儿 workflow(仅 workflow_call 且无
  调用方),不会自动触发——将来接入发版链路前须先改免费规格。

## 待检查的 11 笔提交(全部在 origin/test 上)

| 提交 | 内容 | 验收证据 |
| --- | --- | --- |
| `a5b25b043` | main@342ed69a1 集成(前一轮,已审) | 复审记录+JSON 收据 |
| `5d91ca221` | rusty_v8 本地预取脚本(#24) | 缓存命中/篡改拒绝/错目标三路径;.45 断网暖缓存构建 8m28s;Windows 目标亦验证 |
| `05b1abcfb` | cap×post-model-hook 交界回归 + hooks helper 提取 | 离线 215/215 之内 |
| `188d87360` | genai 0.6.5→0.7.0(用户授权;rig 冻结 0.44.0) | check+fmt+桥 25/25 |
| `bc04e7b47` | 第一批验收文档(台账 §6.55) | — |
| `32308f77e` | hooks.rs 格式修正 | fmt |
| `9b92ff4e4` | linux-sandbox O2/O3:deny 命中自建 /dev 符号链接→显式构建;宿主专属 netns conf 缺席拒绝 | .45 241/241;blocked_descriptor_path 红转绿;strace 28×bwrap 零 /dev/fd;shell_snapshot 家族 48+3 已知噪声 |
| `bb6312cde`/`d88ffb3d4` | O4:/proc/net→全 self 家族 deny 改响亮 Fatal(探针证明 payload pid≠1,pid 翻译方案被否决) | 同上 241/241;无连带 |
| `e7c56d06b` | uds 探针钉 PSModulePath(实机 10/10;runner 待 CI 判) | run 39761410573 将验 |
| `5fe476d7d` | Windows 分诊+write-delete 根因文档(diag1-7 内核级证据链) | §6.57 |
| `c17b8675b` | legacy 删除不设防的运行时 WARNING(用户决策 B+警告,零行为改动) | 实机 check+会话通过 |

## 检查建议入口

1. 新增11笔用 `git log --first-parent --oneline a5b25b043..c17b8675b` 与上表核对;
   `29e7e59d5..c17b8675b` 另含main的300笔祖先及原6笔文档，不应与11笔表逐项等同;
   `git diff a5b25b043..c17b8675b --stat` 看补丁面是否符合"最小改动"。
2. 源码级重点:`linux-sandbox/src/bwrap.rs`(显式 /dev 构建+SYNTHETIC_DEV 清单
   对齐 vendored SETUP_MOUNT_DEV;self 家族 Fatal)、
   `windows-sandbox-rs/src/unified_exec/backends/legacy.rs`(应只有 WARNING 一处)、
   `codex-rust-genai-bridge/`(0.7 机械适配:Tool 新字段/fallible build/Zero 改名/
   Heartbeat)、`scripts/prefetch_rusty_v8.sh`(校验与离线 fail-closed)。
3. 台账一致性:§6.55/§6.56/§6.57 的计数、首败归因、测试前后置与提交内容自洽;
   ROADMAP §1-§4、FORK.md §10 与台账一致;O4 决策(B+警告)已写入 FORK 边界。
4. CI run `39761410573` 终态按上文基线判读;runner 标签抽查确认为标准免费机。

## 边界(不要顺手扩)

- Windows write-delete = 上游继承的内核级限制(WRITE_RESTRICTED 不覆盖 DELETE
  权位),已按用户决策收口(B+警告),不修 legacy、不追 elevated runner pipe。
- 用户方向校准:fork 主线=国内模型接入(OpenAI/Anthropic 协议),不为上游平台
  问题扩大补丁面;不追新上游、不做泛化功能(Files/PDF/多模态输出)。
- genai 已按用户要求升 0.7.0;rig 冻结 0.44.0 不动。
- 发版(release.yml 打 tag)未授权;若将来发版,先处理 provisioned-macos 的
  付费规格问题。
