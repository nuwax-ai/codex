# TUI 模型切换与终端状态标识修复（2026-11-04）

## 问题与证据

用户在 rcoder 项目内通过 /model 选择 Step，界面先显示 Switching、随后显示
Already viewing，底部仍是 MiMo。对应会话的 17:57 turn_context 也仍为
mimo-v2.6-flash，证明首次故障是实际路由未切换。

本次定位到三层问题：

1. 同线程导航去重提前返回，阻止显式 model/provider resume。
2. app-server 对仍有订阅者的已加载线程按 rejoin 处理，忽略路由覆盖。
3. 保留草稿时完整恢复 ThreadInputState，连旧 collaboration-mode 模型一起恢复，
   即使 resume 已确认新路由也可能把底部与下一轮本地设置改回旧模型。

## 最终行为

- 空闲时选择其他 provider，先解除本客户端订阅，再以明确的 model/provider
  恢复同一线程；服务端返回的路由必须匹配。不会强制接管其他客户端仍订阅的线程。
- 执行中不重建或打断当前轮：暂存选择，重复选择只保留最后一次；处理正常完成
  或停止完成通知后再自动切换。
- 等待时暂停后续队列自动发送；选择器关闭不提前解除暂停，成功切换后恢复。
- 后来的同 provider 选择沿用官方设置更新路径，并覆盖此前跨 provider 待切换意图。
- 离开会话取消待切换；成功保留历史、草稿和队列，恢复输入时采用新确认的模型设置。
- provider 从磁盘删除等准备失败发生在解除订阅之前，保留原选择和输入。
- 终端 OSC 7501 应用标识改为 app=nuwax-harness。Cargo bin/crate 不改名，
  不以此承诺 Qoder 按进程名生成的标签一定变化。

## 验证范围

定向 cargo nextest：11/11 通过。

- model_picker_rehome_protocol_preserves_route_and_draft：协议代理模拟服务器对
  已订阅线程的 rejoin 合同，历史读取仍走真实 app-server。覆盖 MiMo→Step→MiMo、
  正常完成/停止完成、重复选择取最后一项、同 provider 后续选择取消旧意图、
  选择器关闭继续暂停、请求次序、确认模型与 provider、草稿历史、磁盘默认不变、
  provider 缺失时原状态不变。
- ignore_same_thread_resume 三项已有导航回归通过。
- terminal_program_status 七项通过，七态快照仅改应用标识。

真实 app-server 集成 model_picker_rehomes_displayed_thread_and_updates_route 和
已有 same_thread_retry_keeps_subscription_and_restores_draft 在本机超时。
单独串行给到 180 秒仍未完成首次 resume。原生采样明确显示：

SkillsWatcher.register_thread_config → FileWatcher.register_paths →
notify FsEventWatcher.run 等待 runloop 通道；其原生线程停在
FSEventStreamStart → register_with_server → mach_msg。

这属于本次本机验证的环境/文件监听阻塞证据，尚未修复该阻塞，也不据协议测试
通过宣称真实集成、Qoder 实机 UI、厂商网络请求或全 workspace 回归已通过。

## 本地安装

最终 release 构建及本地安装结果见 progress.md；源码构建版本仍为 0.0.0。
未发布 npm、未推送仓库。
