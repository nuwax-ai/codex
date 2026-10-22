# Nuwax Harness 0.18.13 发布记录

**状态更正（2026-10-22）：此候选的 Release 已由维护者主动取消，从未发布到 npm。**
原 `v0.18.13` tag 保留、不改写；后续改为 [v0.18.14 发布记录](nuwax-harness-release-0.18.14.md)。
下面保留该候选的前置验证，不能作为已发布收据。

用户授权：新增环境变量指南，将最终主二进制命名nuwax-harness，默认数据目录改为
~/.nuwax-harness，无旧目录兼容/迁移；验证后打tag并触发正式npm发布。

- 发布源码：b6080e11531c1a62d273cd81c49c8c1b119857cd。
- tag：v0.18.13，已推送；[Release run39779558934](https://github.com/nuwax-ai/codex/actions/runs/39779558934)。
- npm包名保留nuwax-codex，新主命令nuwax-harness，旧命令/归档文件别名兼容已有ACP。
- 数据：默认~/.nuwax-harness，CODEX_HOME仍优先且显式路径必须存在；不读取或迁移旧目录。
- 缓存：~/.nuwax-harness-cache；主程序、Code Mode host与平台helper齐全后才认为安装完成。
- 文档：[环境变量指南](../../docs/nuwax-harness/README.md)，附NUWAX与ACP两套无凭据模板。

## 发布前验证

home lib12/12；npm安装合同12/12（含实际并发修复与恶意ZIP负控）；七目标归档合同4/4；
改名后的实际ACP→Codex→HTTP fixture14/14（Chat默认rig、Responses native）；实际CLI
在旧目录含无效配置时仍成功使用新默认，CODEX_HOME显式覆盖也成功。没有真实模型请求。
[机器收据](receipts/2026-10-22-harness-release-preflight.json)保留SHA与结果，不把debug probe
冒称优化后release制品；最终制品须由发布CI和npm安装再给出收据。

发布链修复：读取仓库Rust1.99.1，所有target使用受信V8 archive/binding配对校验，
所有七目标带Code Mode host，Linux带bwrap、Windows带双sandbox helper；精确checkout tag；
OSS全部目标可下载后才发布npm（beta也修正旧的先publish后上传顺序）。

Release 39779558934 当前为 cancelled，Publish to npm 未执行（job 终态 cancelled），npm 0.18.13 未发布。
机器收据中的 in_progress 是原打 tag 时快照；本段按实际取消结果更正。
Core CI39774635523 的 Ubuntu/Windows Summary 已取得，新回归与后续修复见
[0.18.14 记录](nuwax-harness-release-0.18.14.md)。原 tag 仍绑定 b6080e115，不改写历史。
