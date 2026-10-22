# 晚间批次独立复审（2026-10-22）

## 判断与范围

用户目标是通过环境变量接入国内/自定义 OpenAI 与 Anthropic 协议模型，同时保留
上游原生 Responses 通路。核心协议和模型工作流已有实现及真实使用证据，当前工作
应收敛为明确缺陷修复、部署入口联调和修后验收，不再扩大供应商功能列表。

接手时 `test=origin/test=850a87390`，工作树干净。审查增量为
`a5b25b043..c17b8675b` 首父11笔，以及两笔交接文档。main集成a5已在上一轮复审。
`29e7e59d5..c17b8675b` 还包含300笔上游祖先与原6笔文档，不能与11笔表逐行等同。
本轮未再次同步main、未升级rig或genai、未修改ACP仓库、未push/dispatch/发布，
没有使用真实模型API。修复提交由包含本文的Git提交定位。

固定版本：Rust1.99.1 / V8152.2.0 / rig0.44.0 / 备用genai0.7.0。

## 用户所需接入路径

| 路径 | 环境选择 | 已证范围 |
| --- | --- | --- |
| OpenAI Chat兼容 | `NUWAX_WIRE_API=chat`，默认rig | 环境接线、实际厂商与工具/图片工作流已有收据 |
| Anthropic Messages兼容 | `NUWAX_WIRE_API=anthropic`，默认rig | 同上；具体视觉/搜索/推理仍依模型能力 |
| Responses经rig | `NUWAX_WIRE_API=responses`，默认rig | 同协议透传、cap/usage、GLM真实端点等已有收据 |
| 上游原生Responses | `NUWAX_WIRE_API=responses` + `NUWAX_BRIDGE=native` | 环境→provider→Core调用闭环、exec/app-server真实子进程fixture已证；没有本轮官方GPT/中转native实网收据 |

共同配置 `NUWAX_BASE_URL`、`NUWAX_API_KEY` 和网关的实际 `NUWAX_MODEL`。
密钥以env引用进入临时provider，不放入文档/命令日志。`native+chat/anthropic`
会启动失败，不静默忽略。显式CLI/config provider优先于环境临时provider。

开关证据：`utils/cli/src/nuwax_env.rs` 校验并写 `experimental_bridge`；
`model-provider-info/src/lib.rs::uses_model_bridge` 对native Responses返回false；
`core/src/client.rs` 因此使用上游原生HTTP/可用时WS。已有exec默认rig账本对照和
native无rig账本对照，app-server也有真实子进程用例。不是需要再开发的一整套链路。

**ACP入口要分开。** ACP的 `CODEX_BASE_URL/CODEX_API_PROTOCOL/CODEX_WIRE_API`
会显式选择custom-gateway；`EnvGatewayConfig`目前没有experimental_bridge字段。
因此不能承诺单设NUWAX_BRIDGE就覆盖该provider。若正式部署沿用ACP的CODEX_*
入口，补桥选择映射与一次出站对照是剩余的小开发任务；本轮没有跨仓修改它（只读核对ACP dev@6a76aa7）。

字段保证不能扩大：Responses类型clone与RawValue工具JSON已有保留合同；跨协议
私有历史按来源投影，Chat/Anthropic的不可表达字段有明确限制。备用genai不等同
主rig严格usage-presence合同，也不宣传“任何厂商所有私有字段全无损”。

## 本轮发现与修复

1. **CI真失败：新Rust弃用API。** run39761410573的Ubuntu/macOS均在
   Clippy阶段失败，错误唯一指向共享`exec/src/build_source_identity.rs`中的
   `AtomicUsize::fetch_update`。改用同语义`try_update`，不放宽-D warnings。
   旧交接“按旧失败名单判断”的前提不适用于测试根本没启动的这两条lane。
2. **doctor桥名错误。** genai选择也显示rig。按实际experimental_bridge生成
   诊断，五案覆盖默认rig/显式rig/native Responses/genai Chat/genai Anthropic。
3. **预取脚本并发与路径。** 固定`.part`会让并发调用相互覆盖；未引用的shell
   env导出使空格/单引号/$()路径被拆分或求值；相对cache路径切换cwd后失效。
   改同目录唯一mktemp、仅清理自身临时文件、校验后原子发布；cache绝对化，
   stdout/envfile统一printf %q。保持坏hash删除与离线失败关闭。
   八个无网络合同在旧脚本3红，修后8/8；加入既有CI helper步骤。
4. **O2验收不足导致隔离退化。** 原显式/dev为payload同UID所有的可写tmpfs，
   缺席的/dev/fd可重新ln-s创建；restricted Read/dev还会把宿主树重新绑定，
   使“已省略”结论过期；宿主devpts绑定使宿主终端节点元数据可见。
   真实Linux生产helper三种策略均红→绿；最终只读封闭，保留单独授权的shm写入，
   冲突绑定明确拒绝。特殊分支空pts/无ptmx，不再绑定宿主devpts；普通策略仍可分配
   私有PTY。sentinel证明旧实例可见，没有读取其他终端或证明其内容可读。
   不用单元argv形状或初始ENOENT代替恶意重建检查。
5. **状态文档矛盾。** ROADMAP仍称未push、genai进行中、Windows待决策、self deny
   仍静默可读；实际已推、0.7已合、B+警告已接受、self家族已Fatal。
   更新ROADMAP/FORK与台账当前状态，保留历史收据但不把历史当新待办。

6. **备用genai已有的路径/query损坏。** 此问题在a5已存在，不归因0.7升级：
   原实现把query拼完才加`/`，OpenAI URL join可丢/v1且把/塞进参数值，Anthropic
   直接拼messages会把端点吸进query。现用Url先规范path，再编码追加参数；
   OpenAI真实loopback验证/v1与/v1/、原query、特殊字符/中文完整保留。
   genai Anthropic SDK无法表达这种query基址，现明确报不含参数值的固定错误，
   可用主rig路径；正常无query仍兼容。移除raw URL日志，避免query凭据泄露。
   同时发现按Authorization的值过滤会误删同值secondary头，改按名字识别主头。
   三层header的非文本或同层多值在genai String/HashMap中不能无损表示，改为明确拒绝，
   保留跨层provider→auth→extra覆盖，不再静默省略。

7. **TUI测试接口适配遗漏。** Windows首先暴露七处编译错误，但缺参/已删除函数也影响
   macOS，后者被更早lint截断。四处调用补空env_seed_overrides，一处补BackendId；
   上游已删除server_features，不能恢复旧四项过滤或cfg隐藏，用现有真实WebSocket
   compatibility_warning模拟验证表替换和末值优先，并保留monorepo eligibility。
   这只适配测试，不改变产品后端或放宽已有场景断言。

## Windows与genai审查

Windows legacy产品差分确实只有WARNING，不改变授权/执行；UDS仅固定PowerShell
模块路径，没有降低ACL断言。按用户B+警告决策保留write-delete红，不扩写legacy或
elevated实现。新CI尚未给出UDS/legacy的最终判决时，实机绿不代验runner。

genai0.7适配未发现新增阻断：新增Tool字段None不出wire，fallible build传播错误，
Zero在OpenAI adapter映射none；Heartbeat不产内容但被流泵接收而刷新idle计时。
没有用这几条检查声称所有供应商私有字段已覆盖。

## CI快照与原始证据

run39761410573，head=c17b8675b，attempt1：
Ubuntu job119766223411、macOS job119766223380在Clippy失败，workspace步骤skipped。
Windows job119766223198在构建测试时发现七处TUI旧接口编译错误；整个测试尚未执行，
不是先前六项红测试的新判决。Bazel job119766223370成功。整轮conclusion=failure。
日志由jobs API获取并正确去除ESC/BOM；无Summary即没有测试终态计数。
原始日志保留在外接盘 `/Volumes/zhitai/tmp/codex-evening-review-20261022/`。
机器可读摘要、失败记录与源码指纹见[复审收据](receipts/2026-10-22-evening-audit.json)。
本轮不自动重复CI。

旧M1 run39633345230/mac job119602236907成功仅覆盖29e7e59d5。#24已有Linux
断网暖缓存构建及Windows目标制品校验，不再安排为未开始的大任务。

## 最终验证补记

- CI同范围四包Clippy -Dwarnings已通过（9m08s），修复了两条lane的实际lint原因；
  最终额外纳入genai --all-targets再次严格Clippy **通过，16.98s**，覆盖query/header补丁。
- 环境解析、provider路由、genai及身份/收据定向lib集 **139/139**，retries0，
  4.131s。实际分布：utils-cli59、model-provider-info36、genai31、live-tests13；
  其余41条因filter未运行。包含三个新的query合同和三个header合同；真正消费SSE触发HTTP请求。
- 预取脚本无网络合同 **8/8**，bash -n通过；同组在旧脚本3红，首红保留。
- Linux整个sandbox lib **178/178**；bwrap/WSLg及新集成 **76/76**；
  最后CLOEXEC夹具三个实命令集成 **3/3**；原blocked_descriptor_path **1/1**。
  新helper三种特殊策略+普通私有PTY探针均通过。数量是重叠选集，不相加。
  详见[Linux专项收据](receipts/2026-10-22-linux-dev-audit.md)。
- Linux crate范围 --no-deps --all-targets Clippy -Dwarnings通过。首次未加--no-deps
  在未改依赖history的大枚举lint失败；保留首红，不冒称整个依赖图严格lint全绿。
- doctor一个测试循环五案 **1/1通过**，2.335s。macOS TUI lib测试已完整编译，
  两条受影响的daemon合同 **2/2通过**，0.165s；5836条因filter未执行。
- 追加原有兼容/后端七条选集：**6通过、1失败**，首红保留（EOF处5s deadline）。
  后者abort只提出取消，增加有界join确认真实transport所有者已析构，前移原取消断言，
  保留EOF原5s判据。最终无诊断单例 **1/1通过，28.292s**，不写成一次整组7/7。
  首手加握手版本仍有60s harness超时，不能据后来通过声称所有长尾根因已关闭。
  阶段探针实测取消析构2.446s，join后约1.47ms即Disconnected/EOF，未见产品发送端
  泄漏证据；未改产品后端，未加sleep/retry，所有临时标记已删除。
- Windows原生TUI --tests定向check **通过，8m15s**，七处编译错误消失；
  不是Windows运行测试。最终单文件增量check也通过，40.86s，七份源文件SHA均匹配。
  详见[Windows收据](receipts/2026-10-22-windows-tui-audit.md)；没有执行Windows全workspace。
- cargo fmt --all --check及diff-check通过；最终阶段提交仍只在本地，不push/dispatch/发布。
