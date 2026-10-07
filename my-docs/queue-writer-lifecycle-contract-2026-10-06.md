# Queue writer 生命周期观测契约（2026-10-06 第三轮）

交接要求：审查“任何时候无第二 writer”需要真实 writer 注册/锁持有生命周期证据。本文件定义公共路径回归的观测范围与限制；轮询结果不能单独关闭“任何时刻唯一”的验收项。

## 契约

### 1. writer 的定义

应用 writer 通过 `WriterLockCoordinator::acquire` 持有 thread 的排他文件锁：
`CODEX_HOME/thread-writer-locks/{thread_id}.lock`（见 `codex-rs/rollout/src/writer_lock.rs`）。创建/删除锁文件由同目录的 `.coordination.lock` 串行化。

公共 `thread/resume` 会在 `thread-store/src/local/live_writer.rs` 获取 writer 锁，并把锁保存在 live recorder 中；线程空闲、一次 turn 完成或 enqueue 客户端退出都不会释放它。`thread/unsubscribe` 移除最后一个订阅者后，线程在配置的空闲期限到达时卸载，shutdown 完成并移除 live recorder 才释放锁。`thread/queue/list` 只读取未加载线程的元数据，不等于 resume；未加载线程通常没有 writer 锁。

测试探针在 `.coordination.lock` 下打开现有 thread 锁，检查 `try_lock()`；锁文件缺失直接记录空闲，不创建 thread 锁文件。`WouldBlock` 表示已有持锁者；成功表示探测时没有其他持锁者。成功的探针锁先于 coordination 锁释放，使合法 writer 不会与它争抢，也避免探针检查已被删除重建的旧 inode。

### 2. 可观测事件

| 事件 | 观测方式 | 证据强度 |
|---|---|---|
| writer 启动（注册） | resume 前探针为空闲，公共 `thread/resume` 成功后为 `WouldBlock` | 校准真实加载路径与跨进程持锁观测 |
| loaded writer 保留 | 客户端运行阶段样本均为 `WouldBlock`；客户端退出及 queued turn 完成后仍持锁 | 采样时存在活跃持锁者，符合 live recorder 生命周期 |
| writer 结束（释放） | 公共 `thread/unsubscribe` 后收到该线程的 `thread/closed`，再探测为空闲 | 校准真实卸载路径，没有观察到遗留持锁者 |
| 探针失效 | 打开/加锁失败或采样 task 出错 | 回归失败，不能用此前成功样本继续判定通过 |

### 3. 客户端窗口与短暂 writer 的可检测性

后台探针的首个成功样本通过 ready 屏障确认，随后才启动真实 CLI。每次采样记录开始、结束的单调时刻。测试还在启动客户端后立即采样，并用采样后的 `child.try_wait() == None` 确认该样本发生在客户端仍运行时。

客户端窗口从 spawn 返回后的时刻开始，到最后一次 `try_wait() == None` 的时刻结束；只有完整落在此保守窗口内的样本参与运行阶段断言，而且集合必须非空。启动前和退出后的样本不能代替运行期间样本。结束时发送 stop 并等待采样 task 返回，传播错误；CLI spawn/wait 失败也会先收敛探针。

后台探针每轮采样后等待 50ms。实际采样间距还包含文件 IO、coordination 等待及调度时间，不能承诺 ≤50ms。样本全部为 `WouldBlock` 只证明这些采样时有持锁者，不能证明采样间没有锁释放或第二 writer，也不能识别持有者 PID。短暂转移只在采样碰到空闲窗口时可见；即使转移保持了连续持锁，探针也无法归属它。

### 4. 与其他测试隔离

锁命名空间以 `CODEX_HOME` 为根；每个测试使用独立临时 home，探针只操作该 home。owner 使用本地 Responses mock；子进程清理继承的 provider/auth/install 环境，测试不调用厂商。

### 5. 本契约不能证明的（如实登记）

- `WouldBlock` 无法直接归因于 owner 或第二进程。真实 resume/卸载校准、运行窗口样本及既有 CLI 拒绝路径提供组合回归证据，不能据此宣称“任何时刻绝无第二 writer”。若该验收项要求无遗漏的注册/退出和持有者归属，仍需覆盖全部 writer 入口的生命周期事件或其他可归属观测。

## 最小公共路径回归

`cli/tests/queue_writer_lifecycle.rs`：
1. 未加载 thread 校准为空闲；公共 `thread/resume` 后校准为持锁。
2. 探针 ready 后启动真实 CLI enqueue；确认实际客户端运行窗口内有样本，且这些样本全部持锁。
3. 客户端退出后仍持锁；本地 mock 上的 queued turn 成功关闭后仍持锁，owner 队列已消费该提交。
4. 公共 `thread/unsubscribe`、`thread/closed` 后校准为空闲。
5. 独立负控让探针在 ready 后失败，确认停止/等待操作把错误传回调用者。

## 未覆盖（登记）

- Windows 文件锁语义差异（unix gate）。
- owner 进程内部多线程争抢（进程内锁另有序列化）。
- 采样之间的短暂 writer、持有者 PID 与连续锁转移；不能以配置的 50ms 等待时间推断检测下界。
- “任何时刻唯一”的完整验收，以及远程 app/exec 异 OS 的生命周期行为。
