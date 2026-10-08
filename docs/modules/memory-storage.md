# 内存与 storage：写入之后，究竟从哪里读回来？

[模块目录](../modules.md) · [理论视角](../routes/theory.md#memory-storage) · [实现视角](../routes/implementation.md#memory-storage) · [两条完整路线](../learning-routes.md)

本模块围绕一件事：把栈上的 word 写进某个位置，再解释后续读取为什么得到这个结果。先分别掌握帧内字节数组和账户 slot，随后才增加地址不确定、调用归属与回滚。

只想读懂普通 MSTORE/MLOAD、SSTORE/SLOAD，完成前两个单元即可。它们使用离线输入，不要求先掌握 CALL、RPC、SSA 构建或求解器。

## 进入前，只补这几项

- 能按栈底到栈顶手算 PUSH 与弹栈：[00 的栈表](../00-start.md#第二步先手算这个程序)。
- 能读单个确定值与多个候选：[02 的抽象值含义](../02-domain.md#2-抽象值保存的是允许的可能性)。暂时不需要循环和固定点。
- 运行实验时沿用正文的环境参数；省略 calldata 与空 calldata 的区别只需查[13 的默认输入](../13-evm-environment.md#1-默认输入与目标账户)。

| 位置模型 | 一次寻址给出什么 | 一个保存单元是什么 |
| --- | --- | --- |
| memory | 当前帧内的字节偏移 | 一个 byte；MLOAD/MSTORE 连续处理 32 byte |
| storage | 状态账户与 256 位 slot 编号 | 一个 256 位 word；相邻 slot 不重叠 |

<a id="memory-basics"></a>

## 基础单元：用 42 看懂字节、word 和偏移

范围只到一个调用帧。先读[14 的字节与 word](../14-memory-model.md#1-先把字节word-和偏移分开)，再做[写入 42 后读取](../14-memory-model.md#2-第一个实验写入-42再读出来)的 [`memory-word.hex`](../../examples/memory-word.hex)。画出偏移 0 到 31 的 32 个字节：末尾是 `2a`，前面都是零；MLOAD 将它们按大端顺序组装回 42。

**理论角度：**word 的数值和它占用的字节位置是两项信息。偏移 1 表示向后移动一个字节，不能把它当成下一个独立 word。新帧的 memory 读零，写入和访问范围又会改变后续观察。

**实现角度：**打开 [`ByteArray`](../../crates/evm-abstract/src/world/bytes.rs)，只追 `memory`、`write_word`、`read_word`、`expand`；结构中的默认字节解释了为什么没有显式表项也能读零。然后用[覆盖一个字节的实验](../14-memory-model.md#在一个已写入的-word-中覆盖一个字节)理解 `write_byte`。

**观察与停点：**第一个实验的 `stack out` 为 `[{0x2a}, {0x20}]`，分别是读取结果与 MSIZE。能解释“值是 42，内存规模是 32 字节”，即可进入 storage；COPY、未知长度和跨帧复制可留到需要时。

**自检：**MSTORE(0,42) 后，这个 word 从哪个偏移开始，非零字节 `0x2a` 在哪个偏移？先读出的 word 会不会因后续 MSTORE8 而自动改变？

<a id="storage-basics"></a>

## 基础单元：区分初始 slot 与当前 Store

只分析一个账户，暂不涉及调用。读[16 的账户与 slot](../16-storage-model.md#1-storage-的位置由两个数确定)，再运行[替换 slot 0 的实验](../16-storage-model.md#2-第一个实验替换-slot-0读取-slot-0-和-slot-1)。[`storage-basic.json`](../../examples/storage-basic.json) 初始给出 slot 0=5、slot 1=7，代码先写 slot 0=42，再读取两个 slot。

**理论角度：**指令按执行顺序改变当前状态。已知唯一 slot 的写入替换旧值，称为强更新；读取 slot 1 不受这次写 slot 0 影响。初始 world 是输入事实，当前 Store 是执行到此处的状态。

**实现角度：**沿 [`Store::new` → `write` → `read`](../../crates/evm-abstract/src/world/store.rs)，找到 `Plane` 的显式项、账户默认值、全局默认值；结合[三层查找](../16-storage-model.md#当前-store-的三层查找)阅读。再做[未列出 slot 的对照](../16-storage-model.md#没列出的-slot-是零还是未知)：`storage_unknown:false` 是输入的完整性声明。

**观察与停点：**基础实验的栈是 `[42,7]`；完整零默认与未知默认的对照分别是 `[5,0,0]` 和 `[5,⊤,⊤]`。这些离线实验都可以 `Converged`，未知值本身不表示分析未完成。到这里已完成本模块的基础目标。

**自检：**为什么再次 SLOAD(0) 读到 42，而不是 JSON 中的 5？如果 JSON 只列一个 slot，能否只凭文件很短推断其他 slot 都是零？

<a id="aliases"></a>

## 进阶单元：地址有候选时，为什么读取会多出值？

本单元额外需要[02 的 join](../02-domain.md#3-join把两条路径的信息合在一起)。先列具体选择，再合并，范围仍限制在单帧、单账户。

做[14 的有限地址写入](../14-memory-model.md#6-地址确定时覆盖地址有几个候选时合并)与[16 的两候选 slot](../16-storage-model.md#亲手制造两个候选)。后者使用 [`storage-finite-alias.json`](../../examples/storage-finite-alias.json)，保留未知 calldata，并按正文使用 `context-depth=0`，让 slot 0 和 1 在同一状态汇合。

**理论角度：**k∈{0,1} 时写入 9，每个 slot 都可能没有被写到，必须保留旧值。两条具体结果 `(9,7)`、`(4,9)` 被逐位置摘要为 `({4,9},{7,9})`；摘要额外允许 `(9,9)`，不等于找到了一次同时写两个位置的执行。

**实现角度：**[`ByteArray::write_values`](../../crates/evm-abstract/src/world/bytes.rs) 对有限偏移分别写临时副本再合并；[`Plane::write`](../../crates/evm-abstract/src/world/store.rs) 对有限 slot 保留各位置的旧值。读两个分支的差异，不必先研究所有数值组件。

随后只选一个未知索引实验：[相同 memory 偏移写后读](../14-memory-model.md#同一个未知偏移写后再读也未必恢复原值)或[相同 storage 键写后读](../16-storage-model.md#6-无法列出-slot-候选刚写入再读为什么是-07)。两者的结果分别为 Top 和 `{0,7}`；标量身份仍在，数组却没有保存通用符号索引更新关系。

**观察与停点：**能在输出中定位弱更新保留的旧值，并解释一个多出的组合即可。完整 word 的表达式往返、字节间关联和延后合并，可再查[14 的相关性实验](../14-memory-model.md#8-为什么刚存进去再读出来会多出候选)。

**自检：**把两个候选 slot 都直接改成 9，会漏掉哪种具体结果？只知道两次 k 相同，是否足以让当前实现证明 SLOAD(k)=7？

<a id="ownership"></a>

## 可选单元：换了调用帧，位置的主人有没有换？

只有读跨合约报告时才进入此单元。先补[09 的帧内容](../09-cross-contract.md#3-一次调用需要保存哪些东西)，再读[16 的状态归属](../16-storage-model.md#8-调用时谁拥有-storage谁拥有-memory)：两个帧可以共享同一个 storage owner，同时各有独立 memory。

**理论角度：**地址归属和状态版本要分别确定。DELEGATECALL 可以执行实现账户的代码、写代理账户的 slot；REVERT 恢复调用入口检查点，包含更深调用造成的效果。

**实现角度：**对照 [`FrameState::memory` / `saved_store`](../../crates/evm-abstract/src/analysis/machine/frame.rs) 和 [`calls::finish`](../../crates/evm-abstract/src/analysis/transfer/calls.rs)。用[嵌套回滚实验](../16-storage-model.md#9-revert-恢复的是调用入口检查点不是固定初始快照)的 [`storage-callback-revert.json`](../../examples/storage-callback-revert.json)追踪 `(A[0],B[0])` 从 `(1,4)` 到 `(9,7)` 再回到 `(1,4)` 的成功进入子调用路径。

**观察与停点：**能解释恢复后 A[0] 是 1，而不是初始 5 或回调写入的 9。更完整的调用、返回字节和后端比较交给[调用与状态模块](calls-state.md)；它们不影响前两个基础单元的完成。

## 按需要继续

| 新问题 | 下一入口 |
| --- | --- |
| 已证明 k=0，为什么寻址能变精确？ | [关系模块的边界单元](relations.md#limits) |
| 瞬态 slot 为什么只属于本次交易？ | [16 的 transient storage](../16-storage-model.md#10-transient-storage同样按账户和-slot-寻址但只属于本次交易) |
| 节点补查的是旧值还是刚写的值？ | [16 的初始依赖](../16-storage-model.md#11-rpc-补查的是初始事实不是当前交易值)，再到[快照与复用模块](rpc-summaries.md) |
| MLOAD 尚未完成，为什么仍显示剩余栈？ | [04 的块内停止实验](../04-ssa.md#从块中途停下的实际输出开始)，再到[SSA 模块](ssa.md) |

最终自检只需挑一个位置：说出它的单位、owner、初始默认值、最近写入、读取结果与精度限制。完整逐题验收见[16 的自检](../16-storage-model.md#自检先手算再核对)和[14 的六个问题](../14-memory-model.md#15-六个自检问题)。
