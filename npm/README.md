# Nuwax Harness

通过环境变量接入自定义 OpenAI Chat、OpenAI Responses 和 Anthropic Messages 模型。
默认使用 rig，也可选择上游原生 Responses 通路。

```bash
npm install -g @nuwax-ai/nuwax-harness@latest
nuwax-harness --version
```

npm 包名为 `@nuwax-ai/nuwax-harness`，主命令和下载的主二进制为 `nuwax-harness`。
该组织包只导出 `nuwax-harness` 命令；Rust 内部程序名保持不变。


```bash
export NUWAX_BASE_URL='https://your-provider.example/v1'
export NUWAX_WIRE_API='chat'
export NUWAX_MODEL='your-model-id'
export NUWAX_BRIDGE='rig'
# 从你的受保护环境注入 NUWAX_API_KEY。
nuwax-harness
```

Anthropic 使用 `NUWAX_WIRE_API=anthropic`；原生 Responses 使用
`NUWAX_WIRE_API=responses` 和 `NUWAX_BRIDGE=native`，并换成相应的基址及模型。

默认数据目录为 `~/.nuwax-harness`；显式 `CODEX_HOME` 优先且须为已存在目录。
不读取或迁移旧数据目录。下载缓存为 `~/.nuwax-harness-cache`，包含同版本的配套辅助程序，
请勿只复制主二进制而丢弃 helper。

完整参数、ACP 启动、Linux/容器、Windows 与排查示例见
[环境变量启动指南](https://github.com/nuwax-ai/codex/tree/test/docs/nuwax-harness)。
