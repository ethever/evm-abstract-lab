# 11：把状态快照与容器实现分开

`A → B → C` 的状态回滚需要两层约定：容器能保留旧版本，执行器知道哪一层调用失败。仓库把前一层放入本地 crate [`snapshot-state`](../crates/snapshot-state)，后一层仍由 [`Store`](../crates/evm-abstract/src/world/store.rs) 和[调用处理](../crates/evm-abstract/src/analysis/transfer/calls.rs)负责。

本课的 std / imbl 选择改变状态怎样保存和复制；[第 12 课](12-product-domains-facts.md)的 product / constants-only 选择改变一个值保存哪些性质。比较容器时固定同一份输入和域策略，才能把结果差异或时间差异归到正确的原因。

## 从同一份 Store 切换底层实现

`OrderedMap<K, V>` 是仓库拥有的有序映射包装。默认使用标准库 `BTreeMap`；启用 Cargo feature `imbl` 后使用 `imbl::OrdMap`。两种实现暴露同一组读取、插入、范围更新、遍历与序列化操作。键保持排序，因此更换底层不会改变分析输出的顺序。

`Checkpoint<T>` 保存一个不透明的旧版本。`Store::snapshot()` 创建覆盖整个事务状态的检查点，`Store::restore(snapshot)` 消费检查点并替换当前状态。覆盖范围包括 persistent/transient storage、余额、代码、nonce、账户存在性、创建与待删除标记，以及日志。容器不决定调用是否成功，也不负责抽象值的 `join`。

```rust,ignore
let before_b = store.snapshot();
// B 及更深的 C 修改 store。
if b_failed {
    store.restore(before_b);
}
// B 成功：丢弃 before_b，保留当前 store。
```

A 在调用 B 前保存自己的事务状态版本，B 在调用 C 前再保存一份。C 成功后，其效果留在当前版本；如果 B 随后失败，恢复 B 的入口检查点会一起撤销 C 的效果。REVERT 的返回字节单独传给 caller，不属于被恢复的 Store。[第 09 课](09-cross-contract.md)解释调用帧与返回边。

标准库实现复制映射中的键和值。`imbl` 实现共享未修改的树节点；保留旧版本时，后续修改复制需要变化的部分，见 [`OrdMap` 官方 API 文档](https://docs.rs/imbl/7.0.0/imbl/struct.GenericOrdMap.html)。具体成本还取决于键和值、修改范围、旧版本的存活时间和释放成本；两者的语义相同不意味着性能相同。

在仓库根目录进入开发环境后，可以分别运行同一个实际分析输入：

```bash
nix develop
cargo run --release --locked -p evm-abstract-cli -- \
  analyze --world examples/worlds/revert-rollback.json \
  --evm.to 0x0000000000000000000000000000000000000101 --evm.caller 0x0000000000000000000000000000000000001000 --evm.value 0 --evm.calldata 0x --format json
cargo run --release --locked -p evm-abstract-cli --features imbl -- \
  analyze --world examples/worlds/revert-rollback.json \
  --evm.to 0x0000000000000000000000000000000000000101 --evm.caller 0x0000000000000000000000000000000000001000 --evm.value 0 --evm.calldata 0x --format json
```

feature 在一次构建中统一选择后端；同一个进程里的 Store 使用同一实现。并行比较使用独立 target 目录，避免后一次构建覆盖前一次的二进制。

## 先核对语义，再观察时间

[`scripts/compare-state-backends.sh`](../scripts/compare-state-backends.sh) 构建两份 release 二进制并完成以下比较：

1. 对所有单账户 `.hex` 例子比较 disasm、CFG 和 SSA 的完整 JSON。
2. 对所有 world 例子比较分析与 SSA 的完整 JSON，以及关闭摘要后的复用例子。
3. 要求 `missing-code.json` 保持退出码 2、`Incomplete` 和 `MissingCode` 前沿；其他例子要求退出码 0。
4. 运行实际 Store 操作的多次测量，核对观察值与最终状态的 checksum，全部一致后输出中位数与比值。

JSON 比较只忽略空白，保留对象字段顺序和数组顺序。输入错误、退出码差异、完整输出差异或测量 checksum 差异都会停止脚本，不产生性能结论。这些固定例子是回归证据；完整测试与本地门禁仍见[检查流程](local-ci.md)。

```bash
# 默认：64 / 4096 个显式 slot，每项 100 次操作，5 次独立进程重复。
bash scripts/compare-state-backends.sh

# 快速核对，或者按需要放大状态与操作次数。
STATE_BACKEND_SLOTS=64,4096 \
STATE_BACKEND_ITERATIONS=20 \
STATE_BACKEND_REPEATS=3 \
  bash scripts/compare-state-backends.sh
```

构建默认最多使用 `min(8, nproc−5)` 个并行任务，至少使用一个。`STATE_BACKEND_JOBS` 可以进一步降低并行数；脚本拒绝超过该 CPU 预留上限的值。slot 数限定为 1–65536，单项操作次数为 1–10000，重复次数为 1–25。`STATE_BACKEND_OUTPUT` 可更换输出根目录，默认是 `target/state-backend-comparison`。每次运行创建一个带 UTC 时间和随机后缀的独立子目录，并打印完整路径；失败重跑不会混入先前成功运行的聚合结果。

## 测量边界

[`state-backends` example](../crates/evm-abstract/examples/state-backends.rs) 构造一个拥有完整初始 storage 的账户，使用真实 `Store` 与默认抽象域。输入、写入值、join 的另一个 Store，以及每项开始前的状态复制都在计时外准备。

| 工作负载 | 一次计时操作包含什么 |
| --- | --- |
| `checkpoint_drop` | 建立整个 Store 检查点并释放它 |
| `sparse_write_restore` | 检查点、最多 8 个已存在 slot 的精确写入、读取一个观察值、恢复检查点并释放被替换的状态 |
| `unknown_alias_write_restore` | 检查点、未知 slot 写入对该账户全部显式 slot 和默认值的弱更新、读取一个观察值、恢复与释放 |
| `join_drop` | 合并两个 Store、读取一个观察值、释放合并结果 |

计时内使用 `std::hint::black_box` 保留操作和观察结果。计时外检查每项结束后 Store 与原输入相等；另行计算一次写入后或合并后完整 Store 的 checksum，覆盖所有 slot 的效果。完整 Store 序列化、checksum 与输出都在计时外。每次重复交替 std / imbl 的进程启动顺序；它能减少固定顺序带来的偏差，但不消除操作系统调度、频率和缓存变化。

输出保留每个工作负载的后端、slot 数、操作次数、纳秒耗时、观察值 checksum 和最终状态 checksum。每次运行目录里的 `metadata.json` 记录 Git 提交、工作区状态、已跟踪修改和未跟踪源码的 hash、工具链、主机、测量参数与两个后端二进制的 hash；全部比较完成后才写入 `completed:true`。脚本将 CLI 结果写入 `parity.json`，原始测量写入 `samples.jsonl`，聚合写入 `medians.json`。`std_over_imbl > 1` 表示该项 std 中位数耗时更长；它只适用于这次机器、输入和工作负载。

未知 slot 更新与 `join` 要处理大量抽象值；检查点建立只观察保留旧版本的成本。把这几项分开，才能看出某个后端对当前分析器的收益和代价。这个小测量不包含完整分析工作表的内存峰值，也不能据此断言某个后端始终更快。
