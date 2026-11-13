# 智谱 `/v1/responses` 端点：对话以 `function_call_output` 结尾必报 "messages 参数非法"（完整验证报告）

**日期**：2026-11-13
**影响端点**：`https://open.bigmodel.cn/api/v1/responses`（智谱官方 OpenAI Response 协议端点，模型 `glm-5.3-flash`）
**经女娲代理 `nuwax-api.nuwao.com/api/proxy/model/responses` 100% 复现**（错误体逐字透传，代理转发无嫌疑）

## TL;DR

智谱官方 Responses 端点有一个未写进文档的限制：**`input` 的最后一项不能是
`function_call_output`**——即"工具调用结束、等待模型回答"这一标准多轮形态会被
400 拒绝：

```
{"error":{"code":"invalid_request","message":"messages 参数非法。请检查文档。","type":"invalid_request"}}
```

- **与图片无关**（数组/字符串、带图/不带图都一样）；
- **与 tools 定义无关**（带 `tools` 也拒）；
- **流式/非流式都拒**；
- **唯一解法**：在 `function_call_output` 之后**追加一条 user message**，立即恢复正常
  （且模型能正确看到工具结果里的图片内容）。

客户端（nuwax-harness）行为完全符合标准 OpenAI Responses 协议：`view_image` 工具的
图片结果就是以 `function_call_output`（content parts 数组）收尾的——这恰是智谱拒绝的
形态。重试 5 次均撞同一错误，客户端呈现为
`stream disconnected before completion: API 调用参数有误，请检查文档。<trace>`。

## 验证矩阵（2026-11-13，直连智谱官方端点实测）

| # | `input` 尾部形态 | 结果 |
|---|---|---|
| Z1 | 纯文本 user message | ✅ 正常 |
| Z2 | 图片放 user message | ✅ 正常且正确识图（"红色"） |
| Z3 | `fco` **数组**+纯文本，**对话到此结束** | ❌ 400 `messages 参数非法` |
| Z4 | `fco` **数组**+图片，**到此结束** | ❌ 400 同上 |
| Z5 | `fco` **纯字符串**，**到此结束** | ❌ 400 同上 |
| Z6 | `fco` 字符串 + **带 `tools` 定义**，到此结束 | ❌ 400 同上 |
| Z7 | `fco` 字符串 + **追加 user message** | ✅ 正常 |
| Z8 | `fco` 字符串，**流式**，到此结束 | ❌ 同错误（SSE error 事件） |
| Z9 | `fco` **数组**+纯文本 + 追加 user message | ✅ 正常 |
| Z10 | `fco` **数组**+**图片** + 追加 user message | ✅ 正常且**模型正确看到图片**（"红色"） |
| Z11 | 同 Z10 且 user message 里再放一次同图 | ✅ 正常 |

经代理的对照（`nuwax-api.nuwao.com/api/proxy/model/responses`，错误体逐字透传）：
以 fco 结尾 → 同一错误、同一 trace 格式；fco 字符串 + 图片 user message → 正常
（1136 个事件）。**代理（Java Netty 转发）无嫌疑，错误由上游原样返回。**

## 根因推断

智谱把 `/v1/responses` 转成内部 chat 形态处理时，要求 `messages` 以 user 结尾
（chat 类 API 的常见约束）；`function_call_output` 被映射为 tool/工具结果消息，
落在末尾即触发 `messages 参数非法`。上游未按 OpenAI Responses 标准实现
"fco 收尾即请求模型继续" 的语义。

## 修复建议

**代理侧（推荐，改动小且对客户端透明）**：Netty 转发前检查 `input` 最后一项，
若为 `function_call_output`，追加一条占位 user message 即可，例如：

```json
{"type":"message","role":"user","content":[{"type":"input_text","text":"(tool result above)"}]}
```

矩阵 Z7/Z9/Z10 证明追加后功能完整（含图片识别）。

**引擎侧（nuwax-harness 兜底，可选）**：对 Responses 线在请求收尾是 fco 时自动
追加同样的占位 user turn；或提供 provider 开关。上游修复后可移除。

## 复现脚本（key 用环境变量，勿写死）

```python
import json, os, urllib.request
key = os.environ["ZHIPU_KEY"]
url = "https://open.bigmodel.cn/api/v1/responses"
hdr = {"Authorization": f"Bearer {key}", "content-type": "application/json"}
body = {"model": "glm-5.3-flash", "input": [
  {"type": "function_call", "call_id": "call_1", "name": "view_image", "arguments": "{}"},
  {"type": "function_call_output", "call_id": "call_1", "output": "工具结果正常"},
]}   # 对话以 fco 结尾 → 400
req = urllib.request.Request(url, data=json.dumps(body).encode(), headers=hdr, method="POST")
print(urllib.request.urlopen(req, timeout=60).read().decode()[:400])
# 在 input 末尾追加 {"type":"message","role":"user","content":[{"type":"input_text","text":"继续"}]}
# 即返回 200（对照 Z7）
```
