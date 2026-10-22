# 给 Codex 的检查提示词(2026-10-22 晚)

你接手检查 nuwax fork `test` 分支今日第二轮(晚间)的全部产出。唯一源码仓库
`/Volumes/zhitai/git_workspace/fork-nuwax-codex`;本轮已 push 到
`origin/test@ c17b8675b`(29e7e59d5..c17b8675b,含 main 集成 merge `a5b25b043`
与其后 11 笔提交)。台账权威:`claude-rig-final-acceptance-2026-10-18.md`
§6.54-§6.57;路线图 `my-docs/ROADMAP.md`;fork 边界 `my-docs/FORK.md`。

## 待检查的 11 笔提交(全部在 origin/test 上)

| 提交 | 内容 | 验收证据 |
| --- | --- | --- |
| `a5b25b043` | main@342ed69a1 集成(前一轮,已审) | 复审记录+JSON 收据 |
| `5d91ca221` | rusty_v8 本地预取脚本(#24) | 缓存命中/篡改拒绝/错目标三路径 + .45 断网暖缓存构建 8m28s |
| `05b1abcfb` | cap×post-model-hook 交界回归 + hooks helper 提取 | 离线 215/215 之内 |
| `188d87360` | genai 0.6.5→0.7.0(用户授权;rig 冻结 0.44.0) | check+fmt+桥 25/25 |
| `bc04e7b47` | 第一批验收文档 | 台账 §6.55 |
| `32308f77e` | hooks.rs 格式修正 | fmt |
| `9b92ff4e4` | linux-sandbox O2/O3:deny 自建 /dev 符号链接→显式构建;netns conf 缺席拒绝 | .45 241/241;blocked_descriptor_path 红转绿;strace 28×bwrap 零 /dev/fd;shell_snapshot 48+3 已知噪声 |
| `bb6312cde`/`d88ffb3d4` | O4:/proc/net→全 self 家族 deny 改响亮 Fatal | 同上 241/241;无连带 |
| `e7c56d06b` | uds 探针钉 PSModulePath(实机 10/10;runner 待 CI) | CI run 39761410573 将验 |
| `5fe476d7d` | Windows 分诊+write-delete 根因文档 | diag1-7 证据链 §6.57 |
| `c17b8675b` | legacy 删除不设防的运行时 WARNING(用户决策 B+警告) | 实机 check+会话通过 |

## 检查建议入口

1. `git log --oneline 29e7e59d5..c17b8675b` 核对与上表一致;`git diff a5b25b043..c17b8675b --stat` 看补丁面。
2. 源码级重点:`linux-sandbox/src/bwrap.rs`(显式 /dev 构建 + self 家族 Fatal,§6.56)、
   `windows-sandbox-rs/src/unified_exec/backends/legacy.rs`(仅加 WARNING,零行为改动)、
   `codex-rust-genai-bridge/`(0.7 机械适配)、`scripts/prefetch_rusty_v8.sh`。
3. 台账一致性:§6.55/§6.56/§6.57 的每个计数/首败归因是否与提交内容自洽;
   ROADMAP §1-§4、FORK.md §10 的更新是否与台账一致。
4. CI:run `39761410573`(Fork Cargo PR,push 触发,合并树首跑)结果判读——
   预期基线:旧树已知的 ubuntu 1 红(O2 修复前是 blocked_descriptor_path,本轮应转绿)
   与 windows 6 红(实机 5/6 绿;uds 两条经 e7c56d06b 硬化后待判;write-delete 预期仍红,
   决策为 B+警告不修)。**红名单若超出该基线即为新回归,须分诊。**

## 边界(不要顺手扩)

- Windows write-delete = 上游继承的内核级限制,已按用户决策收口(B+警告),
  不修 legacy、不追 elevated runner pipe 问题。
- 不追新上游、不扩泛化功能(Files/PDF/多模态输出不在范围)。
- genai 已按用户要求升 0.7.0;rig 冻结 0.44.0 不动。
