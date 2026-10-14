# 已知代码递归与验收记录

已知 bytecode 递归已接入过程级不动点；原 Unknown USDC 请求的完成时间验收仍开放，本轮在 900 秒外部护栏结束时未交付报告。以下原请求结果绑定代码 `348bb8cc629d37411af299c75649d08a623c2667`、main `f70afa2b383d574649831c3ff10a885947a1126a`，不代替后续候选的测量。关键数据和原始产物摘要见[选定证据](evidence/known-recursion-selected-validation.json)。

## 原请求结果

| 请求 | 实际交付 | 单次观测 | 分析边界 |
| --- | --- | --- | --- |
| WETH | HTTP 200，`Incomplete` | body 32.681550 秒；analysis 32.077572 秒 | 137 states、486 transfers、32 outcomes；26 frontier：8 `StateJoin(Shape)`、18 `Relations(ExpressionLimit)`；partial SSA，开放 `UnknownEnvironment` 摘要 |
| USDC | 无报告，`external_timeout` | wall 900.531215 秒；CPU 902.158705 秒 | 最后仍为 `Running/Analyzing`，round 4；最终 native status、frontier、SSA 和摘要闭合未知 |

348 WETH 与 8d、5048 的完整报告比较，只排序无序合取项和完整 fact 键值对：除 work/timings 外全部导出字段一致，包括 32 outcomes、137 entry/exit states、10 pending machines、CFG、定位 frontier、完整表达式身份和摘要。348 的 work 为 52,254,064,022，比 8d 多 31,352,394；两次 body 分别为 32.681550、33.174817 秒。单样本不建立统计加速。

348、8d 与 5048 的同原输入 USDC 请求均在 900 秒外部护栏结束时没有报告；没有可比较的最终 native status、frontier 或摘要。348 最后记录为累计 4159 states、4810 transfers、791,464,786,804 work；这些是未完成的进度，不能据此声称加速、覆盖等价或 Work 耗尽。WETH 与小例子也不能代填原 USDC 的结果。

## 已实现的机制

- 已知递归按**活动调用栈上重复的结构调用边**准入过程表；已返回的相同 callsite 不算递归。过程 key 不随物理递归深度或任意 calldata 值增长，未完成的过程保留 typed frontier。
- 过程内仅实际循环反馈触发 widening；保留有界最近跳转来源，避免将顺序 helper 的有限返回地址当作任意数值增长。
- 有限 stack alias 保留栈位置之间的 must-equality；join 取共同等价关系，实例化重新命名。persistent storage observations 仅覆盖已知代码活动帧、具体逻辑 owner、栈顶 16 个位置；root ≤64 nodes、key ≤32 nodes、fact work ≤512、Members ≤8。任意世界关系和挂起父帧的 read 模板不在此保证中。
- 冻结关系上的 branch joint proof 同时处理最多 16 个目标 / 4096 bits。完整模型与 AND/OR 排他检查只安装已证明结论；AND UNSAT/普通 Unknown 不证明唯一，未决目标保留原逐项求解。每次 fresh Solver 使用原 rlimit，最多 N+2 checks；Work/Cancelled 停止。共同前缀比单值 Fi 更强，其 Unique/Infeasible 不写入较弱 Fi 缓存。
- state guard 使用原只读 wrapper walker，最多规划 16 个不同条件；仅实际 ISZERO 使用 Boolean joint 准备，原 metadata/Fresh length 路径保留。共同前缀证明只进本次 truth map；未计划或未决条件仍走原 callback。storage observation 的安装按当前 alias 类、owner 和完全相同数值界去重，逐位置保证仍参与 join，兼容但不同的界不被合并。
- literal fold、canonical key 的 pointer HashMap、完整 key Eq 校验和有界 cache maintenance 一并保留。算法工作与逻辑费用分别核对，不以减小默认预算或丢弃路径换速度。

实际 EVM branch 小例子中，全 Unique 的 native checks 为 33→4，全 Multiple 为 33→3，数值和 instruction 结果一致；state guard consumer 小例子为 12→3。这些只证明各 consumer 的机制，没有证明原 USDC 的实际分组规模或 native 次数相同下降。默认 20M work 的 16 个相同 storage alias / 20 JUMP 例子，在去重后保持 exact 7、closed、无 frontier；原逐位置保证和预算保留。

## 复现与报告契约

[输入清单](../examples/known-recursion/captured/manifest.json)、[原 USDC](../examples/known-recursion/captured/usdc-original-request.json)和[原 WETH](../examples/known-recursion/captured/weth-original-request.json)固定 Osaka、区块 26180958 / `0x5219691ff60b20005fa597d2bb5e4b11e3ddb68f24fb528a900c8dafbb84b499`；caller、call value、calldata 内容与长度均为 Unknown。100 字节 witness 是不同实验范围。使用[测量脚本](../scripts/benchmark-known-recursion.py)和匹配该代码的原生程序/Web assets；本批 WETH/USDC 外部护栏分别为 120/900 秒、CPU48–55、RSS 8 GiB、主机可用内存下限 20 GiB。

native schema 为 8，wire schema 为 13。#126/#127 后 export work 是诊断计数，native/HTTP JSON 不再有整份输出字节 cap；typed 节点数、深度、exact byte payload、StateRoot、取消和序列化校验仍保留。RPC response limit 属于事实获取，不能当作报告输出 cap。

[Issue #123](https://github.com/ethever/evm-abstract-lab/issues/123) 的原请求性能目标仍未达成。最终干净提交按[本地门禁](local-ci.md)执行完整检查；精确提交、命令、实际退出码和远端 head 的核对记录在 PR 描述。旧实验原始产物与失败记录保留在 Git 历史和独立证据归档，不作为本页选定结果。
