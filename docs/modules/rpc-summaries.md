# 快照与复用模块：同一份事实，何时可以继续使用？

[模块目录](../modules.md) · [两条路线](../learning-routes.md) · [理论入口](../routes/theory.md#rpc-summaries) · [实现入口](../routes/implementation.md#rpc-summaries)

本模块回答：**补查一个账户或复用一次调用时，怎样确认事实和结果仍属于当前分析？** 目标是分清区块初始事实、交易内 Store、调用摘要及当前代码版本，并知道它们各自的有效范围。

前两单元先补读 [16 的初始快照与当前状态](../16-storage-model.md#3-初始快照与当前状态是两份不同的东西)。进入第三单元的摘要复用时，再补 [09 的一次调用所需信息](../09-cross-contract.md#3-一次调用需要保存哪些东西)，分清 CALL 返回字节、结果位和 Store；无需预先读完创建、销毁和预编译。

前三单元组成核心部分，可用离线 world、源码和现有 localhost HTTP 测试完成，也可以只选择当前需要的一项。公网 RPC 是可选对照；第四单元按需选一个生命周期实验，不影响前面的 RPC 学习。

## 单元一：固定的到底是哪个身份？

**起点与范围：**打开 [`summary-reuse.json`](../../examples/worlds/summary-reuse.json)查看离线 identity，再读 [10 的身份、代码 hash 与指纹](../10-snapshots-summaries-creation.md#身份代码-hash指纹各负责什么)。先不运行本节依赖的 `/tmp/summary-on.json` 查询，第三单元会生成它。

写出三种绑定关系：chain ID/block hash 选择区块，code hash 绑定代码字节，fingerprint 绑定本次初始事实。来源描述相同不证明事实相同；固定区块也不会自动把 caller、calldata 等输入变成某笔确定交易。

然后读[固定区块补查规则](../10-snapshots-summaries-creation.md#为什么补查也必须固定区块)，在 [`rpc/tests.rs`](../../crates/evm-abstract/src/world/rpc/tests.rs) 找 `latest_number_and_hash_pin_once_across_incremental_acquisition`。测试使用本地 HTTP 服务，断言启动选择只解析一次，后续请求使用同一个 `{blockHash, requireCanonical:true}`；无需自己的链节点。

从实现看，跟踪 [`Session`](../../crates/evm-abstract/src/world/rpc/session.rs) 的身份解析和后续请求。从理论看，这是保证观察来自同一声明快照的一致性条件；当前实现信任普通 RPC 返回值，不使用状态证明来验证提供者。

**停在这里的产物：**画出“启动选择 → 固定 hash → 多次账户/槽请求”，并从上述测试指出支持每条箭头的断言。能够解释离线 label、链身份和状态真实性的区别，即可进入下一单元。

## 单元二：补查为什么从入口重跑？

**起点与范围：**读 [16 的 RPC 初始依赖](../16-storage-model.md#11-rpc-补查的是初始事实不是当前交易值)，再读 [10 采集实验](../10-snapshots-summaries-creation.md#可选实验从固定区块采集)中的 A→B→C 轮次图和 `rpc_acquisition` 字段表。暂不执行需要真实 endpoint 的命令。

把一次分析画成两条线：区块事实加入采集缓存；指令从入口重建图、Store 与调用检查点。已强写的 slot 直接读取交易值；仍依赖初始值的有限槽才需要补查。新查到的初始值不能覆盖此前的弱写、转账或回滚关系。

离线核对 [`rpc_discovery.rs`](../../crates/evm-abstract/tests/rpc_discovery.rs) 的 `discovery_replays_unknown_slot_alias_writes_before_delegatecall`：它自建 `127.0.0.1` HTTP 服务，对比按需发现、预先加载和关闭摘要的结果；重跑后 slot 候选仍覆盖 7 和 9。继续到 `compare` 的断言即可，不必一次阅读全部回归。

实现只追踪 [`analyze_rpc`](../../crates/evm-abstract/src/analysis/rpc.rs) 的收集前沿、补入事实、重启轮次，以及 [`Store::missing_initial_slots`](../../crates/evm-abstract/src/world/store.rs) 的初始依赖判断。理论问题是如何保持同一次事务的先后关系；累计预算不能因为重跑而重新获得一份额度。

**停在这里的产物：**能指出最后一轮的图与跨轮累计计数分别在哪里，并解释为何 `MissingStorage` 可能在最终报告中消失。有真实 RPC 时，再选做[关闭发现的同 hash 对照](../10-snapshots-summaries-creation.md#rpc-without-discovery)；保持两次调用输入相同。

## 单元三：命中摘要，复用的是哪个关系？

**起点与范围：**按 [10 的摘要开关实验](../10-snapshots-summaries-creation.md#第一步分别开启和关闭摘要)生成两份 JSON，再做[最终关系比较](../10-snapshots-summaries-creation.md#第二步确认复用没有改变最终关系)。这是完整离线实验，也会生成第一单元提到的快照报告。

只选一个摘要解释 `published`、`hits`、`source_state`、`reused_at`，并检查返回数据和 Store 是否属于同一个 outcome。本例两次请求复制的输出长度不同；callee 结果可复用，恢复 caller 时仍须采用当前继续信息。

理论上，复用要求结果的前提继续成立，而不只是代码地址相同。读[命中条件](../10-snapshots-summaries-creation.md#什么条件下允许命中)，从 [`summary.rs`](../../crates/evm-abstract/src/analysis/summary.rs) 找输入资格与闭合认证，再到 [`summary/replay.rs`](../../crates/evm-abstract/src/analysis/summary/replay.rs) 看证据怎样接回 caller。

**停在这里的产物：**开启时找到命中与导入状态，关闭时命中为零，正文 `cmp` 比较一致；同时说明这只是该例的核对。能回答“输入扩大后旧证书为何失效”和“为何不能只缓存一个返回 word”，就完成本模块的核心部分。

## 单元四：可选——事实没变，执行代码为何能变化？

这一单元连接同一身份问题下的三个扩展，选一个即可。先按表里的起点进入正文，到列出的可观察产物为止；不必从第 10 课开头全部重读。

| 小题 | 正文与既有实验 | 停在什么产物 |
| --- | --- | --- |
| CREATE 安装代码 | [区分 InitCode 与 Runtime](../10-snapshots-summaries-creation.md#第二步运行并区分两种帧)，`create-runtime.json` | 同一地址出现两种 mode/hash，说明后续 CALL 为何使用 Store 中新 runtime |
| SELFDESTRUCT 延迟删除 | [中间标记与最终账户](../10-snapshots-summaries-creation.md#3-selfdestruct转账和删除发生在不同时间)，`created-selfdestruct.json` | 区分 pending_destruction、中途仍有代码和最终 outcome 的账户事实 |
| 预编译原生执行 | [没有普通字节码的调用帧](../10-snapshots-summaries-creation.md#4-预编译没有普通字节码也有调用帧)，`identity-precompile.json` | 找到真实调用帧、返回字节和继续边，说明没有普通指令体仍有执行效果 |

实现按所选题分别看 [`create.rs`](../../crates/evm-abstract/src/analysis/transfer/create.rs)、[`Store::selfdestruct` / `finalize_transaction`](../../crates/evm-abstract/src/world/store.rs)、[`precompile.rs`](../../crates/evm-abstract/src/analysis/transfer/precompile.rs)。理论上要检查代码身份、生命周期或执行模式有没有进入当前判断的前提，不能仅凭账户地址复用旧结论。

## 怎样判断整个模块已经完成

给一份报告标出四项：初始事实的身份、当前 Store 的修改、摘要的输入前提、仍未补齐的前沿。再指出一个由本地测试支持的重跑规则和一个摘要开关实验的观测结果；将样本证据与普遍保证分开。

需要补足调用检查点时进入[调用与状态模块](calls-state.md)；要分析报告能支持什么结论，进入[证据与验证](evidence.md)。公网 RPC 的真实查询和可选生命周期实验，都可以留到相应问题出现时再做。
