# 图片工具输出复审修复边界

单条**含图片或图片省略标记的**工具结果按现有模型可见估算共享最多 10,000 tokens：正文和媒体最多
9,744，另外 256 为受限工具头部、合并的省略说明和截断提示预留。调用方的
更小额度仍约束正文和图片，装不下时可以全部省略，但必须保留明确说明。
纯文本（包括结构化MCP文本）沿用调用方明确配置的预算，不强制降到10K；
否则会破坏30K/100K exec与Code Mode输出合同。标准图按每张 1,844 计费，最多保留最新五张；调用方提高额度不会提高硬上限。

`original` 不再免费，也不会暗改为 high。工具入口和 bridge 共用受限的
header-only 尺寸估算，按 32px 网格计费，最多 10,000 patches。可识别且能
放入共享额度的小原图保留 original detail（2×2 PNG 为一个 patch）。多张
原图共享剩余额度，超额从最旧开始省略；用满 10,000 patches 的单图无法
同时容纳控制说明，也会明确省略。

尺寸估算先限制 data URL 为 16 MiB、其头部为 8 KiB、Base64 解码结果为
12 MiB，然后为 header decoder 配置 1 MiB 分配额度；不执行 raster decode。
PNG 预算仅仅读取 IHDR，不解析或解压 ICC/EXIF 等后续元数据；其他格式仍
先受解码输入大小约束。这不是全图准备路径的 metadata 解压防护。损坏、
未知、超界、无法在该额度内读取头部的图片，以及没有尺寸的
File 引用，保守按 10,000 计费，因此可能被省略。`UnifiedImageBudget` 默认
关闭；主动开启时，准备后变为 original 的工具图片也按相同规则收费。
用户消息图片仍走独立准备路径，最终 wire guard 对可识别原图同样使用
header patch 估算，未知图仍收取保守上限。

准备完成的工具 original 图片保留 inline，避免上传成无尺寸的 File 引用后
在下一轮 history 准入被误判为未知原图；这些图片不会进行无用上传。该策略
通过 ImageOrigin 的空 message_role 识别工具结果，用户消息和非 original
工具图片继续采用 attachment store 的上传策略。原本已经是未知 File 引用
的 original 图片仍按保守上限处理，不建立跨线程的隐式尺寸缓存。

文件引用 ID 和 data URL 的头部各有 8 KiB 上限；整个 inline URL 仍受
16 MiB 单图及 24 MiB 聚合上限约束。这些是引用元数据护栏，不能代替 ICC
等图片内部元数据的解压限制。现有 `EncryptedContent` 协议豁免未在本次改动。

准备产物按 Base64 膨胀后的 data URL 长度检查，并为 32 MiB 整包限制预留
具名 1 MiB 包头和文本空间。该检查保证单张图片不会独自占满整包；多图片
和其他请求内容的总大小仍由最终 wire guard 检查。
