# Nuwax Harness 0.18.13 发布记录

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

当前Release正在运行，npm0.18.13尚未宣称发布。已有完整core CI39774635523（head8ed1cbfbd）
仍在跑，Bazel已通过；其余Summary尚未取得，后续终态单独回填。
本次纯收据文档使用[skip ci]并推进test，不额外启动重复workspace CI，tag的Release仍跟原tag SHA。
