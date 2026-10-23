# Nuwax Harness npm 组织包（2026-10-23）

用户决定：开发阶段统一使用 `@nuwax-ai/nuwax-harness`，安装命令为
`npm install -g @nuwax-ai/nuwax-harness`，只导出 `nuwax-harness` 主命令。
不提供旧包卸载或迁移流程。数据目录仍为 `~/.nuwax-harness`，显式 `CODEX_HOME` 优先。

## 本轮范围

- npm 包名与发布配置：`@nuwax-ai/nuwax-harness`，public、官方 npm registry。
- 安装入口与环境变量文档同步；历史发布收据保留原名，不改写旧事实。
- stable/beta 后续发布使用新包名。
- stable workflow 增加 `workflow_dispatch` 的 `npm_only` 开关：从指定 workflow revision
  发布 npm 包装，检查已有公开 stable release 的七个制品及 OSS 下载；不重新构建原生程序。
- 首次组织包版本仍为 `0.18.14`，复用 `v0.18.14` 原生制品
  `f8273097515a97dea54d4ffe4e7a37e51bd30de1`。npm 包装提交与原生源码分别记录，不移动旧 tag。

## 已验证

- 本地 npm launcher/postinstall/pack 合同 **12/12**，包含新作用域名和仅主命令导出的断言。
- 两份 workflow YAML 与全部 shell block 语法检查通过。
- 发布前新组织包 registry 查询为 404，未占用既有同名版本。

## 发布与安装结果

组织包 `@nuwax-ai/nuwax-harness@0.18.14` 已发布，latest=0.18.14；
[package-only CI39888836661](https://github.com/nuwax-ai/codex/actions/runs/39888836661) 成功，原生构建与 GitHub release 创建按设计跳过。
Mac ARM64 全新 prefix/cache/HOME 的实际 scoped npm 安装通过，只导出新命令；版本、全部文件校验和、真实 wrapper App Server initialize/thread-start/read/EOF、nuwax_env/artifact-probe 和零模型请求均通过。
[组织包机器收据](receipts/2026-10-23-harness-scoped-npm.json) 记录包装 gitHead 与原生 f827 的不同绑定。

检查又发现原生 TUI/doctor 的版本查询和自动更新命令写死旧包名，已同步源码地址与提示。
此原生改动不能通过 npm 包装覆盖，已提交 `2d216c175` 并打 `v0.18.15`，
[Release39890467055](https://github.com/nuwax-ai/codex/actions/runs/39890467055) 正在构建；模型/协议实现未改。
更新动作、registry metadata、doctor 更新标签及 daemon 安装提示的定向测试 **15/15**（无重试）；
两个实际 registry 地址均读取成功。已有 CLI test 的 cloud_config unused-variable 与 macOS大unwind警告保留，非本轮引入。
0.18.14 组织包已可正常安装启动，但其原生更新检查仍是旧通道，不把源码修正冒称已进入旧二进制。
原生制品验收与三平台安装、四路径 ACP 28/28 见
[原生 0.18.14 收据](receipts/2026-10-23-harness-release.json)；不能将旧包安装直接称为新包已安装。
