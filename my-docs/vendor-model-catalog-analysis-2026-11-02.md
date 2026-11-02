# vendor-model-catalog Spec 核实与可实施性分析

日期:2026-11-02 · 性质:**需求(Spec)的核实分析**,非实施 Plan
被核实文档:`my-docs/vendor-model-catalog-plan-2026-11-02.md`(需求方执笔)
结论:**Spec 与代码现状高度吻合,需求成立,可直接进入实施 Plan 阶段**;本文
给出核实证据、三处小出入、实施形状(四条工作流)与 Plan 出稿前建议先定的点。

---

## 1. 核实明细(逐论断对代码)

| Spec 论断 | 核实结果 | 证据 |
|---|---|---|
| #2/#5/#13 fallback 占位:272k 窗口、95% 有效百分比、web_search 固定 Text、10k 截断、search tool 关 | ✅ 全中 | `models-manager/src/model_info.rs:185-215`(fallback 构造一处全含) |
| #7 审阅预算钳 128k 且**无视显式窗口配置** | ✅ | `guardian-context/src/budget.rs:95-99`:`used_fallback_model_metadata` 为真即 `min(supported, 128k)`,显式 1M 同样被砍 |
| #8 轮内换模型被拒(轮间可用) | ✅ 原文即如此 | `core/src/session/step_activation.rs:95-100`:两端任一 fallback 元数据即 `Err` |
| #3 官方目录模型的显式窗口被目录上限钳制 | ✅ | `models-manager/src/model_info.rs:44` `context_window.min(max_context_window)` |
| `used_fallback` 驱动全库分支(文档称 11 处) | ✅(口径微差) | 携带该分支的文件 **12 个**(child_config / review_session / review / code_mode_warning / turn_context / step_activation / guardian-v2 / models_endpoint / budget / openai_models / model_info / manager);按分支点数计与文档相符 |
| models.dev 覆盖到**套餐端点粒度** | ✅ | 实测 235 个 provider;`zai-coding-plan`、`alibaba-coding-plan-cn`、`xiaomi-token-plan-cn`、`stepfun-step-plan`、`minimax-cn-coding-plan` 全部存在 |
| models.dev 数据与实测一致 | ✅ | `zai-coding-plan/glm-5.3-flash.limit.context = 1,000,000`——与我们 2026-11-02 GLM 探针(81.5 万过 / ~98 万拒)一致 |
| models.dev 只有 reasoning 布尔、无档位表 | ✅ | 故 §5.4"档位表只能来自 overlay、不发明"是数据事实约束,非设计取舍 |

## 2. 三处小出入(不阻塞,实施 Plan 时修正)

1. **体积预算**:白名单 15 个 provider 的原始 JSON 子集实测 **173KB**,略超
   Spec §8 的 ≤150KB。两个解法:裁掉 `description`/`canonical_model_id` 等
   长文本字段后预计 60-90KB(推荐);或预算放宽到 200KB。
2. **白名单键名**:`ollama` 在 api.json 中不存在(Ollama Cloud 的实际键名待
   确认,疑为 `ollama-cloud`),刷新脚本白名单需按实际键名修。
3. §2 表"11 处分支"按文件数为 12——纯口径差异,建议 Spec 回写时统一口径。

## 3. 实施形状(四条工作流,建议顺序)

### WS-A:§6 两项 A 类修正 —— 先行、最小、可独立发版(~几十行)

- `effective_input_token_limit` 的 128k 钳制加条件:仅"无目录元数据**且无
  显式窗口**"时生效(§6.1);
- fallback 解析点加一次性告警,点名 slug / 272k / 应设的配置项名(§6.2)。

### WS-B:目录快照 + 解析层 —— 核心

- 刷新脚本(`cargo xtask refresh-catalog` 之类,幂等可测):拉 api.json →
  白名单 → 裁剪 schema → 生成 `models-manager/src/vendor_catalog.json`
  (upstream 不存在的路径,`include_str!` 打包,**零运行时网络**);
- 解析链在 fallback 前插一层:官方目录未命中 → 查 vendor 目录 → 构造完整
  ModelInfo(`used_fallback=false`)→ §2 的 B 类差异(窗口/百分比/模态/
  工具面/轮内转向/审阅准入/search tool)自动消失;
- 同 slug 多条目消解:overlay 别名优先 → 套餐条目优先 → 窗口最大者 + 留痕
  告警(Spec §3.4);
- **锚点测试**:断言 glm-5.3-flash context=1M、zai-coding-plan 存在等已知
  事实——上游 schema 漂移立即测试红(§9.8)。

### WS-C:窗口语义(§4)

- 显式向上 = 生效 + 启动告警(点名变量/目录值/生效值);**作用域仅 vendor
  目录模型**,gpt-\* 保持 upstream 钳制行为不动(合并面最小);
- doctor 窗口行加来源标注(目录来源/显式覆盖/占位,双侧数值)。

### WS-D:overlay + models.d + 档位目录化(§3.5/§5/§7)

- TOML schema(档位表/条目别名/按厂商百分比);models.d fail-fast 加载
  (坏文件报错点名文件与字段);
- **档位表接进现有 `provider.reasoning_effort_levels` 管道当默认值**——直接
  复用 0.18.24 已落地的档位门机制,目录只是把"要用户配"变成"开箱即对";
  逃生口单独配置键(不复用空列表语义,Spec §5.3);
- 覆盖关系:provider 显式配置 > models.d > overlay(§5.2)。

**验收主战场**(§9.5):目录命中后 11+ 处 `used_fallback` 分支**全部翻转**
的组合验证(轮内转向、审阅预算、图片准入、search tool)——这是行为变化最
大、最需要 e2e 盯的部分。

## 4. Plan 出稿前建议先定的点(Spec §11 之外;2026-11-02 已决,见 Spec §11 决策记录:
逐字段深合并/仅实证档位/维持隐藏/体积放宽 200KB;接入点与刷新形态按本文建议执行)

1. 快照 schema 的裁剪字段清单与体积预算(150KB 裁剪达成 vs 放宽 200KB);
2. 档位默认值的接入点:**config 加载时填充 provider info(推荐,一处生效)**
   vs stream 层查询——影响 §5.2 覆盖链的实现复杂度;
3. "向上告警"对 fallback+显式组合的边界:占位 272k 不是目录权威,建议
   **只有目录命中的模型**向上覆盖才告警,fallback+显式维持现状静默生效;
4. 快照刷新形态:文档化手动步骤 vs xtask 命令化(倾向后者,幂等可测);
5. models.dev 数据滞后于厂商上新的窗口期由 overlay/逃生口兜底(Spec 已有
   设计,Plan 里落成操作步骤)。

## 5. 与既有工作的衔接

- **effort 档位门(0.18.24)**:provider 级 `reasoning_effort_levels` 成为
  目录档位表的默认值来源,机制零改动;
- **溢出两面修复(0.18.25)**:pending 投影与事后恢复的判定随目录真实窗口
  自动更准——GLM/MiMo **零配置**即拿到 1M 窗口与正确阈值(§9.1);
- **压缩阈值 80%(0.18.25)**:显式值解封语义与 Spec §4 同构,目录落地后
  无需返工。

## 6. 结论

Spec 可直接进入实施 Plan 阶段:**WS-A 可立即先行**;核心价值(GLM/MiMo
零配置拿到真实窗口与档位)的数据源已实测可用;实施 Plan 按 Spec 附录的
移交说明产出(代码级落点、快照脚本与锚点测试设计、overlay/models.d 的
TOML schema、任务拆分与排期),任何偏离须回写 Spec。
