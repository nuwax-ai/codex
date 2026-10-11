# 默认代理策略迁移裁决材料（2026-10-11）

状态：**未裁决，未实施**。按 follow-up 第二批第 4 项准备的具体 diff 与兼容证据；`OutboundProxyPolicy::ResolvedDefault` 目前仍为 opt-in，本文仅供决策。

## 迁移内容（具体 diff）

主改动是 config 两个工厂选择点各一行（`else` 分支）：

```diff
--- a/codex-rs/core/src/config/mod.rs (Config::http_client_factory, ~L1729)
         let outbound_proxy_policy = if self.respect_system_proxy {
             OutboundProxyPolicy::RespectSystemProxy
         } else {
-            OutboundProxyPolicy::ReqwestDefault
+            OutboundProxyPolicy::ResolvedDefault
         };
```

同型第二处在 `factory_from_config`（同文件 ~L3126，CLI 侧）。其余显式构造 `ReqwestDefault` 的产品调用点（各自一行）：

| 调用点 | 说明 |
|---|---|
| otel/provider.rs、otel/metrics/config.rs | OTLP 出口（替代 fixture 的 RespectSystemProxy 切换不等于本迁移，见边界） |
| codex-api/files.rs、login revoke/manager | 文件上传与登录流 |
| codex-api realtime_websocket/methods.rs | WS 目的地：ResolvedDefault 本就委托 TransportDefault，行为不变 |
| core/environment_selection.rs | 环境探测 |
| chatgpt_cloudflare_cookies.rs、ollama、code-mode grpc_session | 其余出口 |

## 兼容证据（已具备）

1. matcher 与锁定 hyper-util 0.1.20 逐字段等价：15 个 parity 向量（批 K，含上游自身测试向量）。
2. 路由解析向量 8 个 + 双轨路由等价 11 个子进程用例（批 N）。
3. **真实 wire 字节等价**（`7922659b7`）：plain/percent-encoded userinfo 的 `Proxy-Authorization`、CONNECT 隧道（含 TLS SNI 于 origin 实测）、407 同状态零重试风暴、IPv6 代理字面量——参考 0.12 默认 client 与 ResolvedDefault 池逐字节一致。
4. **跨版本等价**（本日新提交）：0.12 参照 / 0.13 bridge 形态 / ResolvedDefault 池三栈在 env、NO_PROXY、CGI 直连、重定向保持代理六场景同侧。
5. 公开 Factory 双向 redirect/复用四格（Codex `4e15b884c`）。
6. 委托安全性：Windows 手动项、SOCKS scheme、多 @/空 userinfo 凭据、ws/wss 目的地——全部委托 TransportDefault（行为按构造保持）。

## 需要裁决的点

1. **SystemProxyFallback 组合**：现 fallback 仅在 `policy == ReqwestDefault` 时挂接。迁移后该 gate 命中不到——要么保留 gate（fallback 功能对默认路径失效，需另行声明），要么把 gate 改为 `!= RespectSystemProxy`（fallback 与本地解析组合的语义需要一次专项验证）。**不能无声丢失。**
2. **CFRunLoop 因果未证明**（Q04）：ResolvedDefault 的收益前提（消除 reqwest 同步系统读取的停顿）仍是推断；冷/暖原预算测量待安静窗口。迁移不依赖该证明（行为等价已证），但收益叙述依赖。
3. **行为差异面**：迁移后默认路径不再触发 reqwest 的同步系统读取（macOS）。这是有意的性能差异，不是行为等价差异；其余行为按 1–6 等价。
4. OTLP loopback 夹具早前切到 RespectSystemProxy 的 PASS **不构成本迁移的证据**（不同策略）；若迁移，OTLP 出口随之走 ResolvedDefault，其并发窗口（批 J 记录的 ≥7 路失败）需要复测。

## 建议裁决选项

- A. 迁移（两处 config 主干 + 各调用点），fallback gate 同批改为 `!= RespectSystemProxy` 并补组合验证。
- B. 仅迁移 config 主干（模型主路径），固定出口（OTLP/login/files）暂留 ReqwestDefault，分批观察。
- C. 暂不迁移，保持 opt-in，等待冷/暖预算与 runloop 因果实验数据。

任一选项都不改变 RespectSystemProxy（系统/PAC 优先）与 NO_PROXY/env 语义本身。
