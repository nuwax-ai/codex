# Spec：macOS 默认系统代理查询的兼容性能边界

日期：2026-10-10。状态：**修订后的设计，未实施**。技术实现统一见 `rig-production-completion-2026-10-10/plan.md` §2.3，执行为T07。本文件回答做什么与边界，不把线程猜测或旧方案算成已验证实现。

## 1. 已确认事实与未定因果

- 本批已恢复 ReqwestDefault → TransportDefault，撤销字面量IP/localhost强制Direct；环境/系统/显式代理及重定向契约不能再次回退。
- 锁定reqwest0.12.28/hyper-util0.1.20的macOS默认matcher会先创建SCDynamicStore、读取手动HTTP/HTTPS设置，再分别补HTTP_PROXY/HTTPS_PROXY缺失值；ALL_PROXY最后fallback。环境代理已经设置并不意味着当前实现跳过系统查询。
- 默认matcher不执行PAC/CFNetwork目的地例外。现有RespectSystemProxy的macos::resolve会执行它们，不能直接复用来冒称默认等价。Rig使用的reqwest版本另核，不能假定同构。
- 旧窗口client build测12.28s、主线程对照7ms、采样命中SCDynamicStore支持系统查询为慢路径；未控制runloop/负载/线程全部变量，不能确认无CFRunLoop是唯一原因。
- test-only registry直连只能修hermetic夹具，不能证明生产默认建client的慢路径已经解决。

## 2. 功能与兼容要求

1. 默认模式保持锁定参考客户端的完整逐scheme/逐hop代理行为：HTTP/HTTPS/ALL_PROXY、大小写/空值/部分配置、NO_PROXY、显式builder和system手动设置。
2. RespectSystemProxy的PAC/例外和优先级继续独立；默认模式不因优化额外执行PAC或CFNetwork例外。
3. 系统配置读取不得阻塞Tokio worker；采用有界、单飞、正负TTL的加载与缓存机制。线程/runloop选型须通过受控实验验证，不能仅移动位置便承诺延迟消失。
4. 等待可取消、后台刷新有界；系统不可用、线程退出/异常、重建与缓存失效都有明确策略，不无限积累任务/线程，不无声绕过用户代理。
5. 重定向后重新按目的地判断，保留网络与鉴权安全。不得把起点路由固定给全部hop而丢NO_PROXY。
6. 核全部消费者：route-aware pool、Core/SDK急切client构建、Rig0.13、普通WS与HTTPS-proxy；某条pool通过不能替代其他入口。

## 3. 验收

- 受控参考client与优化client双轨：手动系统HTTP/HTTPS、PAC-only、例外列表、env半设/全设/ALL_PROXY、显式builder、NO_PROXY命中/未命中。
- 域名、IPv4/IPv6、localhost/尾点/大小写、HTTP/HTTPS/WS/WSS和多次redirect：本地proxy/target记录真实计数和目标，不记录凭据。
- 成功、认证拒绝、TLS、连接失败、取消、过期/不可用/恢复各有用例；成功/失败不靠扩大deadline。
- 同负载/同线程条件控制runloop；无预热独立进程、并发N进程分别计route/build/connect/header/body。性能目标依照原消费者预算；未达标原样记录，不预设必能小于2s。
- Linux/Windows编译与策略负控，正常Bazel及公共三wire验证；默认成功与普通Direct/loopback-only边界分别覆盖。

## 4. 非目标与交付边界

不改变TLS trust合同、OPAQUE来源隔离或上游第一方能力；不调整系统代理配置来让测试变绿。macOS系统访问若确需FFI unsafe，说明理由并局限在已存在封装或最小适配器，不扩散生产unsafe。

旧“专职CFRunLoop线程+复用macos::resolve”候选不满足完整默认等价，已撤为待重设方案。是否runloop线程最优、系统变化订阅、Windows类似优化均由Plan实验裁决。完成须有实际对照证据，不能只提交本Spec后标T07完成。
