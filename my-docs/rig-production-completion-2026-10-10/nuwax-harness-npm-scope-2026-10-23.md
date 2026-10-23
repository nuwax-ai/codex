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

待实际 CI 发布、registry 元数据以及新组织包隔离安装验证回填。
原生制品验收与三平台安装、四路径 ACP 28/28 见
[原生 0.18.14 收据](receipts/2026-10-23-harness-release.json)；不能将旧包安装直接称为新包已安装。
