# nuwax-harness 厂商模型元数据目录 — 实施 Plan

日期:2026-11-02 · 状态:待用户评审后交实施
需求来源:`vendor-model-catalog-plan-2026-11-02.md`(Spec,含 2026-11-02 四项决策)
核实分析:`vendor-model-catalog-analysis-2026-11-02.md`(14 项差异逐条核实)
社区对照:opencode(构建期快照 `snapshot.txt` + 运行时刷新;`mergeOverlay`
递归逐字段合并;选择器只显示已配置 provider 的模型)——fork 采其快照与合并
语义,运行时刷新按企业需求刻意不做。

---

## 0. 决策基线(2026-11-02 用户拍板)

1. models.d 对 overlay/目录:**逐字段深合并**(未设字段保留目录值);
2. 档位表初始覆盖:**仅实证过先上**(首批 GLM-5.3 系 low/high/max);
3. 模型列表可见性:**维持隐藏**(目录只影响元数据行为);
4. 快照体积:**≤200KB**(保留 models.dev 原始字段,不裁剪)。

## 1. 数据与产物

### 1.1 快照生成(刷新工具)

- 新 bin:`models-manager/src/bin/refresh-vendor-catalog.rs`
  (`cargo run -p codex-models-manager --bin refresh-vendor-catalog`,幂等):
  1. 拉取 `https://models.dev/api.json`(开发机执行,产物入库);
  2. 白名单过滤 provider(键名以实测为准;`ollama` 实为 Ollama Cloud 的实际
     键名,脚本里先校验白名单命中率并报缺失);
  3. 原样保留字段(决策 4),仅做白名单裁剪与排序(可复现 diff);
  4. 写出 `models-manager/vendor/catalog.json`(≤200KB,含 `_meta`:
     来源 URL、拉取时间、上游 commit 可得则记);
  5. 刷新后自动跑锚点测试,失败即退出非零。
- `.gitignore` 不忽略该文件(它是发行物的一部分,`include_str!` 打包)。

### 1.2 overlay(仓库内,`models-manager/vendor/overlay.toml`)

```toml
_version = 1

# 推理档位表(仅实证数据;Spec §5.4 不发明)
[effort."glm-5.3-flash"]
levels = ["low", "high", "max"]
default = "high"

# 条目别名:同 slug 多条目的显式消解(Spec §3.4)
[alias."glm-5.3-flash"]
provider = "zai-coding-plan"

# 按厂商的有效窗口百分比(Spec §3.5-3;缺省 95)
[provider."zhipuai"]
effective_context_window_percent = 95
```

### 1.3 models.d(用户扩展,`$NUWAX_HOME/models.d/*.toml`)

- schema 与 overlay 同构(同 `_version`),支持子集字段;
- 加载:fail-fast——解析失败/字段类型错,**加载即报错并点名文件与字段**
  (复用 config 层错误风格);非 `.toml` 文件忽略;
- 合并:目录值 ← overlay ← models.d,**逐字段深合并**(对象递归、标量新者胜,
  对齐 opencode `mergeOverlay` 语义);
- 优先级:provider 显式配置 > models.d > overlay > 目录(models.dev)。

## 2. 代码级落点

### 2.1 WS-A(先行,独立可发版)

| 改动 | 文件 | 形状 |
|---|---|---|
| A1 审阅预算钳制加条件 | `guardian-context/src/budget.rs:95-99` | `used_fallback_model_metadata && configured_window.is_none()` 才 `min(supported, 128k)`;显式窗口按配置算(Spec §6.1) |
| A2 fallback 占位告警 | `models-manager/src/model_info.rs:169`(fallback 构造) | 进程内按 slug 去重(`OnceLock<Mutex<HashSet>>>`)的 `warn!`:slug、272_000 占位、建议设 `model_context_window`(Spec §6.2) |

### 2.2 WS-B(目录解析层,核心)

| 改动 | 文件 | 形状 |
|---|---|---|
| B1 vendor 目录模块 | 新 `models-manager/src/vendor_catalog.rs` + `vendor/catalog.json` | `include_str!` 解析(懒加载 OnceLock);查询接口 `lookup(slug) -> Option<VendorModel>`;**同 slug 多条目消解**:alias(overlay)→ 条目名含 `coding-plan`/`token-plan` 优先 → 窗口最大者 + `warn!` 留痕(Spec §3.4) |
| B2 解析链插入 | `models-manager/src/model_info.rs`(fallback 构造前) | 官方目录未命中 → B1 查 vendor → 命中则构造完整 `ModelInfo`(`used_fallback=false`;窗口=entry `limit.context`、输出上限=`limit.output`、modalities、`reasoning` 布尔仅记不造档)→ 未命中才走 fallback |
| B3 来源标记 | `protocol/src/openai_models.rs::ModelInfo` | 新增带默认值字段 `metadata_source: MetadataSource`(枚举 `Official|Vendor{provider}|Fallback`;上游合并面:一个字段,记 FORK.md)。B 类差异的各分支按 `Fallback` 判定改为 `metadata_source == Fallback` |
| B4 锚点测试 | 新 `models-manager/src/vendor_catalog_tests.rs` | 断言:`zai-coding-plan` 存在且 `glm-5.3-flash.limit.context == 1_000_000`;`minimax-cn-coding-plan`/`xiaomi-token-plan-cn`/`stepfun-step-plan` 存在;白名单 provider 数 ≥ 14;体积 ≤ 200KB。上游 schema 漂移 → 解析或锚点红(Spec §9.8) |

### 2.3 WS-C(窗口语义与可观测)

| 改动 | 文件 | 形状 |
|---|---|---|
| C1 显式窗口双向生效 | `models-manager/src/model_info.rs:20-47` | 按 `metadata_source` 分支:`Vendor` → 显式值全额生效(不 min 钳制),且显式值 > 目录值时 `warn!` 点名(变量名、目录值、生效值);`Official` → 维持 upstream `min()` 钳制不动;`Fallback` → 维持现状(全额生效,不告警——占位非权威) |
| C2 doctor 来源标注 | `cli/src/doctor.rs`(model_routing 行) | 窗口输出三态:`catalog (zai-coding-plan) 1,000,000` / `override NUWAX_MODEL_CONTEXT_WINDOW=200,000 (catalog 1,000,000)` / `placeholder 272,000 (未配置)`(Spec §4 可观测性) |

### 2.4 WS-D(overlay/models.d/档位目录化)

| 改动 | 文件 | 形状 |
|---|---|---|
| D1 overlay/models.d 加载与深合并 | `models-manager/src/vendor_catalog.rs` | TOML 解析(fail-fast 报错点名);深合并函数(对象递归/标量胜出);合并顺序 §1.3 |
| D2 档位默认值接入 | provider 装配处(config 层构建 `ProviderInfo` 后) | `reasoning_effort_levels` 为 `None` 时,查合并后的目录档位表填充——**复用 0.18.24 档位门管道,机制零改动**(Spec §5.1);覆盖关系:provider 显式 > models.d > overlay |
| D3 逃生口 | `model-provider-info` + config schema | 新配置键 `model_providers.<id>.ignore_catalog_effort_levels = true`:按 provider 关闭目录档位、回退宽松(不占用空列表语义,Spec §5.3);`doctor` 可见其生效状态 |
| D4 覆盖链告警 | D2 处 | models.d/overlay 档位表与 provider 显式配置并存时,启动 `debug!` 记录实际采用来源 |

### 2.5 文档(随发版)

FORK.md §10(目录层与窗口语义新行)、NUWAX-HARNESS.md(§4 目录来源说明 +
models.d 使用节)、usage-guide(ACP 仓,models.d 一段)、台账 §6.75。

## 3. 测试设计

1. **单测**:vendor_catalog 解析/消解(alias/套餐优先/最大窗口+warn)/深合并
   (嵌套对象、标量、数组)/体积上限;
2. **ModelInfo 构造**:vendor 命中模型的窗口、modalities、`metadata_source=Vendor`、
   `used_fallback=false`;effort 布尔不造档(§5.4);
3. **窗口语义四态**(C1):vendor 向下/向上(告警)/official 钳制不变/fallback 不告警;
4. **A 类**:显式窗口后审阅预算不钳;fallback 未配置告警出现且同 slug 只一次;
5. **B 类翻转组合**(§9.5,目录命中模型):轮内转向放行、审阅预算按模型窗口、
   审阅线程图片按 modalities 准入、search tool 按 tool_call 能力开放;
6. **档位门**:零配置 GLM-5.3-Flash 选 `medium` 发送前被拒(§9.4);provider
   显式覆盖;`ignore_catalog_effort_levels` 逃生口回退宽松;
7. **models.d**:扩展生效/坏文件报错点名/优先级(provider > models.d > overlay);
8. **e2e 真网**(§9.1):GLM/MiMo 零配置跑通,doctor 显示目录 1M,溢出压缩行为
   与显式配置一致;Mac 跑 core 集成 `RUST_MIN_STACK=33554432`,compact 套件
   `-j 8`。

## 4. 任务拆分与排期(单人,估)

| # | 任务 | 量 |
|---|---|---|
| T1 | WS-A 两项 + 测试(独立提交,可随 0.18.26 先行) | 0.5 天 |
| T2 | 刷新 bin + catalog.json 入库 + overlay 骨架 + 锚点测试 | 0.5 天 |
| T3 | vendor_catalog 模块 + 解析链插入 + metadata_source + 消解 | 1 天 |
| T4 | 窗口语义分支 + doctor 标注 | 1 天 |
| T5 | models.d 加载/深合并 + 档位默认值接入 + 逃生口 | 1 天 |
| T6 | §9 全项验收 + 真网 e2e + 文档四件 | 1 天 |

发版:0.18.26(T1 可视进度先随任何一次发版走)。

## 5. 风险与对策

| 风险 | 对策 |
|---|---|
| models.dev schema 漂移 | 锚点测试 + 解析仅取已知字段(未知字段忽略不炸,对齐 opencode"skip not fail") |
| ModelInfo 加字段的 upstream 合并面 | 单字段带默认值,FORK.md 登记;`..test_model()` 兼容 |
| 同 slug 消解歧义 | alias 显式消解优先;否则套餐条目优先;再否则最大窗口 + warn 留痕 |
| 快照滞后于厂商上新 | overlay 热修(models.d/overlay 不等发版);逃生口回退宽松 |
| 目录命中带来的行为变化面(轮内转向解锁等) | §9.5 组合验收逐一钉住;FORK.md 明示"目录命中=目录模型待遇"是目标行为 |
| Ollama Cloud 键名不确定 | 刷新脚本白名单命中率校验,缺失即报错(不静默) |

## 6. 与既有工作的衔接

- effort 档位门(0.18.24):目录档位表成为 `reasoning_effort_levels` 的默认值
  来源,门机制零改动;
- 溢出两面修复(0.18.25):pending 投影与事后恢复随目录真实窗口自动更准,
  GLM/MiMo 零配置即得 1M 窗口与正确阈值;
- 压缩阈值 80%(0.18.25):显式值解封语义与 Spec §4 同构,无需返工。
