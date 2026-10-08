# 调用与状态：返回了什么，留下了什么，又撤销了什么？

[模块目录](../modules.md) · [理论视角](../routes/theory.md#calls-state) · [实现视角](../routes/implementation.md#calls-state) · [两条完整路线](../learning-routes.md)

本模块把一条 A→B 调用拆成三件可观察的事：暂停与恢复哪个帧、把哪些字节交回 caller、保留或回滚哪些交易状态。目标是读懂调用图中的一条继续边，而不是仅看 CALL 的成功位。

## 进入前，准备一个 word 和一个 slot

- 会读操作数栈：[00 的栈表](../00-start.md#第二步先手算这个程序)。
- 知道 MSTORE/MLOAD 操作 32 字节：[14 的第一个实验](../14-memory-model.md#2-第一个实验写入-42再读出来)。
- 知道 slot 按账户归属，写入修改当前 Store：[16 的第一个实验](../16-storage-model.md#2-第一个实验替换-slot-0读取-slot-0-和-slot-1)。

不需要先学 RPC、摘要、CREATE 或完整 SSA。前三个单元形成调用语义的主线，容器实现与性能比较放在最后选读。

<a id="call-return"></a>

## 基础单元：沿 CALL 和 RETURN 把三份字节区连起来

运行[09 的返回值实验](../09-cross-contract.md#1-第一个实验b-返回-1a-写入-1)，输入为 [`call-return-branch.json`](../../examples/worlds/call-return-branch.json)。先只追踪一次成功进入 B 的执行：B 返回 32 字节的数值 1，A 读取复制到自己 memory 的字节，再写自己的 slot 0。

**理论角度：**`[A 活动] → [A 暂停,B 活动] → [A 活动]` 描述调用栈；每帧的 EVM 操作数栈是另一层。成功位、完整 returndata 与 CALL 请求的 memory 输出区也要分别观察。

**实现角度：**沿 [`calls::call` / `finish`](../../crates/evm-abstract/src/analysis/transfer/calls.rs)，查看 [`CallStack`](../../crates/evm-abstract/src/analysis/machine/stack.rs) 与 [`ChildFrame`](../../crates/evm-abstract/src/analysis/machine/frame.rs)。子帧的 `Continuation` 保留 A 的继续位置和输出区；A 的暂停状态没有被 B 的栈与 memory 覆盖。

**观察与停点：**在报告中找到 Call、Return 与 A 后续的 BranchTrue。再按[帧内容与 returndata 实验](../09-cross-contract.md#3-一次调用需要保存哪些东西)检查 [`returndata-copy.json`](../../examples/worlds/returndata-copy.json)：请求输出长度为零，仍可随后复制完整 returndata。报告还可包含模型保留的失败可能，按[为什么还有 2 和 Failure](../09-cross-contract.md#2-为什么结果里还有-2-和-failure)分开解释。

**自检：**为什么 CALL 返回的成功位 1 不等于返回数据中的数值 1？为什么 A 请求零字节输出，不代表 B 没有返回字节？

<a id="owners-reentry"></a>

## 基础单元：代码账户、状态账户和调用帧分别是谁？

先做[09 的代理实验](../09-cross-contract.md#4-代理实验读谁的代码写谁的-storage)，查看 [`proxy-storage.json`](../../examples/worlds/proxy-storage.json) 中两个代理执行同一实现代码时的帧。只追 code address、state owner、caller、value 四项。

**理论角度：**代码相同不表示存储相同。DELEGATECALL 使用实现代码而保留代理的状态身份；重入还说明，同一个账户可以同时对应多个调用帧，各有栈和 memory，但共享当前账户状态。

**实现角度：**对照 [`FrameKey`](../../crates/evm-abstract/src/analysis/machine.rs) 的 `code_address`、`address`、`address_value` 与 [`FrameState`](../../crates/evm-abstract/src/analysis/machine/frame.rs)。S 编号标记整机状态，不能直接当成账户编号；帧内跳转历史也不能代替完整调用栈。

**观察与停点：**代理实验中实现 code address 都是 `0x...0300`，state owner 分别为 `0x...0201`、`0x...0202`。随后做[重入实验](../09-cross-contract.md#6-重入新帧读到当前状态)：成功进入的内层 A 读到外层刚写的 1；抽象报告仍可能把最终 slot 1 合为 `{0,1}`，其中 1 对应该成功路径。

**自检：**如果每次 CALL 都从初始 world 重新建立 Store，重入实验会在哪一次读取出错？两个 A 帧共享 slot，是否表示它们也共享 memory？

<a id="rollback"></a>

## 基础单元：失败要恢复的是哪一刻？

做[09 的 REVERT 实验](../09-cross-contract.md#5-回滚撤销子调用保留之前的修改)，使用 [`revert-rollback.json`](../../examples/worlds/revert-rollback.json)。A 先写 slot 0=3；B 再写自己的 slot 0=7，随后 REVERT 并返回 42。

**理论角度：**回滚撤销 B 及其更深调用的状态效果，同时保留 A 调用 B 之前的写入。REVERT 数据独立交回 caller，因此可以同时看到“B 的写入消失”和“A 得到 42”。

**实现角度：**读取 [`Store::snapshot` / `restore`](../../crates/evm-abstract/src/world/store.rs)，再找 [`calls::finish`](../../crates/evm-abstract/src/analysis/transfer/calls.rs) 使用子帧 `saved_store` 的位置。保存点覆盖整份 Store，包括其他账户、transient storage 和日志，不能只保留 B 自己的 slot。

**观察与停点：**报告的成功入口结果中，A slot 0 为 3，B slot 0 恢复为 4；A 保存的返回数值可能为 `{0,42}`，42 来自 REVERT 数据，零来自其他失败可能。再读[回调后回滚](../16-storage-model.md#9-revert-恢复的是调用入口检查点不是固定初始快照)的具体路径，确认 B REVERT 能撤销更深 A 回调写入的 9。

**自检：**回调 A 已成功返回，为什么它的修改仍可能随祖先 B 一起撤销？如果保存点取自固定初始 world，会错误抹掉 A 的哪次修改？能回答就完成本模块的调用语义主线。

<a id="backends-boundaries"></a>

## 可选单元：保存状态的容器与停止分析的边界

需要修改实现时，读[11 的后端说明](../11-state-backends.md#从同一份-store-切换底层实现)。[`Checkpoint`](../../crates/snapshot-state/src/checkpoint.rs) 保存旧版本，[`OrderedMap`](../../crates/snapshot-state/src/ordered_map.rs) 选择 std 或 imbl 容器；保存与恢复的语义相同，复制和共享的成本可以不同。

**理论角度：**容器支持保留版本，执行器决定恢复哪一个版本；这与 product/constants-only 如何表示数值是两个选择。比较性能之前，应固定输入与分析策略，核对语义一致。

**实现与观察：**按[11 的比较合同](../11-state-backends.md#先核对语义再观察时间)阅读 JSON、退出码与 checksum 的核对范围，再决定是否运行测量脚本。单个更快的检查点实验不能说明全部分析始终更快。

另做[09 的缺代码与调用深度实验](../09-cross-contract.md#7-输入缺失和预算停止也要读出来)。`MissingCode` 与 `CallDepth` 都留下 `Incomplete`、退出 2；它们不是 CALL 正常返回 0。源码中的 [`FrontierReason`](../../crates/evm-abstract/src/analysis/machine.rs) 保存这些未展开原因。

**停点与自检：**能区分“子调用执行失败”“模型保留失败可能”“分析没有覆盖该调用”即可。补查初始事实、闭合摘要、创建和销毁代码属于[快照与复用模块](rpc-summaries.md)，需要时再进入。

最终验收：选一次调用，画出进入前、子帧中、返回后三份状态；逐项标出成功位、返回字节、memory owner、storage owner 与回滚点，并确认报告是否仍有 frontier。
