# 已知代码递归与验收记录

已知 bytecode 递归已接入共享过程不动点；原 Unknown USDC 的完成时间目标仍开放。以下观测绑定代码 `4212e5addd3672a9becdb7cd01d0ded75d836eb9`、main `f70afa2b383d574649831c3ff10a885947a1126a`。关键数据见[选定证据](evidence/known-recursion-selected-validation.json)。

| 原请求 | 实际交付 | 单次观测与边界 |
| --- | --- | --- |
| WETH | HTTP 200，`Incomplete` | body 32.233947 秒；analysis 31.728704 秒；137 states、486 transfers、32 outcomes；26 frontiers：8 `StateJoin(Shape)`、18 `Relations(ExpressionLimit)`；partial SSA，开放 `UnknownEnvironment` |
| USDC | 无报告，`external_timeout` | process wall 900.607233 秒；CPU 902.173378 秒；RSS 峰值 2,294,704 KiB；最后仍为 `Running/Analyzing` round 4；最终 native status、frontier 和摘要闭合未知 |

WETH 与 348 的完整导出比较只排序无序 `PathAtoms` 和完整 symbolic/storage/transient fact 键值对，排除 `metadata.work`/`metadata.timings`；其余字段一致，包括表达式身份、roots、entry/exit 和 pending machines、CFG、诊断及摘要。work 实际同为 52,254,064,022；348 body 为 32.681550 秒。单样本不建立统计加速，导出一致也不泛化为数学关系等价。

USDC 最后进度为累计 4227 states、4898 transfers、823,722,197,022 work、2 accounts / 2 slots / 29 requests；它们不是最终图规模、最终 work 或实际槽成员清单。与选定历史 348、8d、5048 的同 900 秒 NoReport 记录一样，本轮没有可比较的最终精度，不能据此称 Work 耗尽或性能修复。

## 实现与精度边界

- 已知递归按活动栈上重复的结构调用边准入；已返回的相同 callsite 不构成递归。共享输入增长触发重算，输出 generation 通知订阅者；闭合要求队列为空且无 frontier，没有真实终态时保留 `RecursiveNoTerminal`。调用身份、owner、static 状态、checkpoint 和回滚参与求解。
- 过程内实际循环及递归反馈触发 widening，有界 MRU 保留跳转来源。有限 stack alias 保留 must-equality；persistent storage observations 限于已知代码活动帧、具体逻辑 owner、栈顶 16 个位置，root ≤64 nodes、key ≤32 nodes、fact work ≤512、Members ≤8。任意世界关系和挂起父帧 read 模板不在此保证中。
- branch/state joint 只安装已证明结论，branch 上限 16 targets / 4096 bits，未决项保留原求解。较强共同前缀的 Unique/Infeasible 不写入较弱 Fi 缓存。typed canonical-key cache 使用完整 Eq、有界保留和付费维护；literal fold、pointer HashMap 与专用线性维护计费一并保留。
- Z3 候选族复用仅用于已付款 chunk 中 3–32 个同宽 syntactic literals。完整 base→target→after 和 UF lemmas 在 push 外，首次真实 cache miss 惰性构造，每项 native literal EQ push/check/pop；每项原 fee、rlimit、cancel、未决顺序及完整 Fi 缓存保持。unique/pair、非 literal 和其他 provider 保持原路径。

单个真实捕获候选族的 native check CPU 为 2.438→0.143 秒，两侧均 32 SAT；C/D/E 控制显示策略与断言顺序也贡献差异，不能归因于纯复用或外推为 USDC 整体收益。选定测试日志为 364 个唯一测试（134 embedded + 124 domain relational + 42 transfer relations + 21 known + 43 unknown），均 actual 0；query_pair 19 是 embedded 的子集。fmt、两 crate all-target Clippy、标准 Nix release 和 matching assets 构建也为 actual 0。

同代码基线的私有诊断在 USDC 第 4 轮记录到约 71.48 秒的后段窗口：1212 次 native 检查累计 60.53 秒，其中 fresh Solver 为 55.48 秒、literal family 为 5.06 秒，均在过程表外。Context 外围配对差约 1.90 秒，取消监控配对差约 0.075 秒；这些是已完成的 wall spans，嵌套跨度不能相加为 CPU 分解。该窗口说明剩余热点仍是多个精化消费者的独立求解；诊断构建与单窗口不能代替标准请求的完成时间对照。

数值积摘要仍会忘记部分参数→输出及跨帧 Store 关系，有界 MRU、深度包络和 SMT/资源限制仍影响精度。`--no-summaries` 仍执行强制递归不动点，不能当作逐层展开对照，该独立对照验收仍开放；显式增加预算的功能例子不证明默认策略或原请求完成。

CLI 的未知调用目标回归分别检查深度 3 与深度 2：前者进入重复活动边的递归摘要后收敛、无 frontier；后者在准入前受分析深度限制，保留 `Incomplete` 和 `CallDepth`，不能把环境摘要闭合误当作整个请求完成。两者均验证不猜测远端账户，也不额外读取 storage。

后续工作分为两条路线：按消费需求精化符号值，减少普通分支上的重复求解；以及在资源边界生成可解释的部分报告。前者必须保证摘要形成、widening 和报告快照不会提前丢掉尚未使用的关系；后者须区分预算停止、用户取消与后端错误，并为报告生成保留资源。本次不引入默认时间预算，也不采用已证实在摘要忘掉关系时造成额外数值精度损失的放宽投影实验。

## 复现与报告契约

[输入清单](../examples/known-recursion/captured/manifest.json)、[原 USDC](../examples/known-recursion/captured/usdc-original-request.json)和[原 WETH](../examples/known-recursion/captured/weth-original-request.json)固定 Osaka、区块 26180958 / `0x5219691ff60b20005fa597d2bb5e4b11e3ddb68f24fb528a900c8dafbb84b499`；caller、call value、calldata 内容与长度均 Unknown。100 字节 witness 是不同范围。使用[测量脚本](../scripts/benchmark-known-recursion.py)及 matching native/Web assets；本批 WETH/USDC 外部护栏分别为 120/900 秒、CPU48–55、RSS 8 GiB、主机可用内存下限 20 GiB。

native schema 8，wire schema 13。#126/#127 后 export work 为诊断计数，native/HTTP JSON 不再有整份输出字节 cap；typed 节点数、深度、exact byte payload、StateRoot、取消和序列化校验仍保留。RPC response limit 属于事实获取。最终完整门禁的干净提交、命令、实际退出码与远端 head 核对单列于 PR。

[Issue #123](https://github.com/ethever/evm-abstract-lab/issues/123) 的原请求完成时间验收仍开放。
