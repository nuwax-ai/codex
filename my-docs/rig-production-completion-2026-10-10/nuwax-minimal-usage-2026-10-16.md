# NUWAX Codex app-server + ACP dev 最小使用清单（2026-10-16 收口轮）

一页可审查清单：如何在隔离环境启动 Linux/容器 Codex app-server，由 ACP dev
驱动第三方模型主路径。所有密钥均为占位符；真实凭据按环境变量名注入，值不落盘。

## 本轮验证过的产物（源 2f2ddc157，见收据表）

| 产物 | 平台 | sha256 |
|---|---|---|
| codex CLI（含 `app-server` 子命令，ACP 的 CODEX_PATH） | Linux x86_64 | `4c2414218aeedb1723ccb8b4682319cc64cda2730f957cc9a530ab860ca4ae99` |
| codex CLI | macOS arm64 | `3fe73883561bd42eddda054e466db423d8adfd4408eb333f8af274fe08eb1f96` |
| codex-code-mode-host | Linux | `9a601dde4bf1d318e6a0cbc476cfc853189a1f4d40634fe6e8211c70b59c78c2` |
| codex-app-server（独立二进制，非 ACP 路径） | Linux | `aea2b9aa6672856b9f047aa6a8fc1ce8452753bcb04d961c0f298408d1937393` |
| codex-exec | Linux | `ac3e7b494f6841addfec2fb2ef69919b3e8b6f69fdeb5cd355c07aa96e33e118` |
| test_stdio_server（测试夹具） | Linux | `82542cfffaafc781fd429c1b4b606a776f2d4757e05b29587841913858faa721` |

构建条件：rustc 1.95.0（仓库 pin）、`--locked`、V8 150.4.0
`ptrcomp_sandbox_release` artifact 对（按 `third_party/v8/rusty_v8_150_4_0_
release_manifests.sha256` 校验后经 `RUSTY_V8_ARCHIVE`/`RUSTY_V8_SRC_BINDING_PATH`
注入）；ACP 验收用 CODEX_PATH 指向 **codex CLI**（ACP 以 `$CODEX_PATH app-server`
子命令方式启动），不是独立 app-server 二进制。

## 环境启动（NUWAX）

1. 隔离 home：`export CODEX_HOME=$PWD/codex-home`（全新目录，不写全局配置）。
2. 供应商配置写入 `$CODEX_HOME/config.toml`：

```toml
model = "<第三方模型ID>"
model_provider = "vendor"

[model_providers.vendor]
name = "third-party"
base_url = "http://<endpoint>/v1"   # 三协议形状见下
env_key = "NUWAX_VENDOR_API_KEY"    # 只写变量名；值从环境注入
wire_api = "chat"                   # chat | responses | anthropic
```

3. 三协议 base URL 形状（相对 `{base}` 的请求路径）：
   - `wire_api = "chat"`：`POST {base}/chat/completions`，鉴权 `Authorization: Bearer $KEY`；
   - `wire_api = "responses"`：`POST {base}/responses`（同协议 passthrough），
     鉴权 `Authorization: Bearer $KEY`；MCP 工具以 namespace 条目
     （`{"type":"namespace","name":"mcp__<server>"}`）广告，回调用扁平名
     `mcp__<server>__<tool>`；
   - `wire_api = "anthropic"`：`POST {base}/v1/messages`（base 不含 `/v1`，
     rig 会追加），鉴权 bearer 被转译为 `x-api-key: $KEY`。
4. 启动：ACP dev 侧由 `CODEX_PATH` 指向上表 CLI；或直接
   `codex app-server`（stdin/stdout JSON-RPC）。
5. 停止：向进程发 SIGTERM（正常退出 ≤15s，见压测收据）；ACP 侧
   `session/cancel` 取消进行中的回合。

## ACP dev 指向方式

```bash
CODEX_HOME=$PWD/codex-home CODEX_PATH=/path/to/codex \
  node --import tsx src/index.ts     # ACP agent（stdio）
```

验收器（无凭据、本地 mock）：
`scripts/acp-validation/run-all-wires.sh /path/to/codex <期望sha256>`（18 行
场景表，出 `summary-18.json`）；压测：`run-stress.sh <codex> <sha> ALL`。

## 工具与截图限制

- 单次工具输出图片硬上限 8 张（超出合并为一条 `[omitted N image(s) …]` 标记）；
  媒体字节/视觉 token 共享限额不随更大文本预算放大；纯文本沿用调用方显式预算。
- 截图经 MCP `take_screenshot` 返回合成红 PNG；typed image 以
  `image_url`（chat）/ `input_image`（responses）/ base64 source（anthropic）
  进入下一请求；`text()` 的 base64 永不猜成图片。
- Code Mode `image()` 只产出一个 typed 图像；普通 JS `return` 不产生内容。

## 恢复路径与日志

- 冷恢复：进程被杀后，新进程 `session/load {sessionId}` 从 rollout 恢复，
  召回依赖持久化历史真实在场（S6/压测 cold-restart 轮断言请求里确有 token）。
- 日志：`$VAL_ROOT/appserver-logs/app-server.log`（JSON-RPC 通知流）；
  mock 请求录制 `$VAL_ROOT/mock-record.jsonl`（含 wire/鉴权/kind/body）。

## 回滚步骤

1. 停止：结束 ACP agent / app-server 进程（SIGTERM；压测实测 ≤20ms 退出，
   工具子进程随后 ≤10s 内全部退出）。
2. 部署回滚：CODEX_PATH 指回上一已知良好 CLI（sha256 收据各轮记录于
   `ci-validation-ledger` 与本目录执行记录）；`$CODEX_HOME` 为隔离目录，
   直接删除即回滚会话/rollout，不触碰全局配置。
3. 数据：rollout 全部在 `$CODEX_HOME` 内（append-only；remote compact 亦
   不改写既有字节），无外部状态需要清理。

## 能力表（分层，不得混写）

| 层 | 证据 |
|---|---|
| mock 合同（本轮） | Linux 18/18 三 wire 六场景（summary-18.json）；51 轮压测（stress-results.json）；mock 单测 17/17 |
| 旧真实服务收据 | 10-15 晚 MiMo 付费轮工具往返成功（历史）；live-replay 196/189 报告数（cassette 回放） |
| 当前未授权 live | 真实厂商/真实视觉验证本轮未运行，结论不外推 |

## 边界

- Responses wire 的 Step 配置端点本轮未接真实服务（现有 Step 只提供
  Chat/Anthropic 端点）；未验证 ≠ 永久不支持。
- Windows 为扩展支持门（见复审 §4），不与 Linux 主门混写。
