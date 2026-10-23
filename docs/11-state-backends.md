# 11：把状态快照与容器实现分开

学习入口：[模块：调用、状态归属与回滚](modules/calls-state.md) · [理论：状态与保存点](routes/theory.md#calls-state) · [实现：容器后端与回滚](routes/implementation.md#calls-state) · [选择路线](learning-routes.md)。

`A → B → C` 的状态回滚需要两层约定：容器能保留旧版本，执行器知道哪一层调用失败。仓库把前一层放入本地 crate [`snapshot-state`](../crates/snapshot-state)，后一层仍由 [`EvmStore<N>`](../crates/evm-abstract/src/world/store.rs) 和[调用处理](../crates/evm-abstract/src/analysis/transfer/calls.rs)负责。

本课的 std / imbl 选择改变状态怎样保存和复制；[第 12 课](12-product-domains-facts.md)的 product / finite-set 选择实际使用 `NumericValue` 或 `FiniteConstantSet`，来源、值身份和表达式属于共用的 `EvmValue<N>`。比较容器时固定同一份输入、数值策略和关系模式，才能把结果差异或时间差异归到正确的原因。

## 从同一份 Store 切换底层实现

`OrderedMap<K, V>` 是仓库拥有的有序映射包装。默认使用标准库 `BTreeMap`；启用 Cargo feature `imbl` 后使用 `imbl::OrdMap`。两种实现暴露同一组读取、插入、范围更新、遍历与序列化操作。键保持排序，因此更换底层不会改变分析输出的顺序。

`EvmStore::snapshot()` 返回 [`SharedCheckpoint<EvmStore<N>>`](../crates/snapshot-state/src/shared_checkpoint.rs)，覆盖整个事务状态，包括 persistent/transient storage、余额、代码、nonce、账户存在性、创建与待删除标记，以及日志。`capture` 调用一次 `EvmStore::clone` 保存旧版本；随后复制检查点句柄只共享这份不可变状态。`EvmStore::restore(snapshot)` 消费检查点：唯一持有者直接移出旧 Store；还有其他检查点共享它时，先克隆 Store，再替换当前状态，让恢复后的写入与其他检查点隔离。容器不决定调用是否成功，也不负责抽象值的 `join`。

原有 [`Checkpoint<T>`](../crates/snapshot-state/src/checkpoint.rs) 仍保存独占的泛型状态；`from_state` 与消费式 `restore` 不要求 `T: Clone`，恢复时直接移动状态。它与 Store 使用的共享检查点是两个不同的类型。

```rust,ignore
let before_b = store.snapshot();
// B 及更深的 C 修改 store。
if b_failed {
    store.restore(before_b);
}
// B 成功：丢弃 before_b，保留当前 store。
```

A 在调用 B 前保存自己的事务状态版本，B 在调用 C 前再保存一份。C 成功后，其效果留在当前版本；如果 B 随后失败，恢复 B 的入口检查点会一起撤销 C 的效果。REVERT 的返回字节单独传给 caller，不属于被恢复的 Store。[第 09 课](09-cross-contract.md)解释调用帧与返回边。

容器后端的差异仍体现在 `capture`、共享检查点恢复时的 Store 克隆，以及后续修改的成本。标准库实现克隆映射时复制键和值；`imbl` 实现共享未修改的树节点，后续修改复制需要变化的部分，见 [`OrdMap` 官方 API 文档](https://docs.rs/imbl/7.0.0/imbl/struct.GenericOrdMap.html)。具体成本还取决于键和值、修改范围、旧版本的存活时间和释放成本；两者的语义相同不意味着性能相同。

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

以上两个命令把 caller、value 和 calldata 明确限定为同一具体输入。省略这些参数会分析符号输入；比较两种后端时也必须保留相同范围。结果 JSON 的 `schema_version` 为 13，调用环境在 `.entry.environment`，帧键 `.states[].key.frames[].caller` 用 `{"Concrete": "0x..."}` 或 `{"Symbolic": "Caller"}` 表示；`.address_value` 是逻辑 ADDRESS，`.address` 是状态账户键。每次环境的私有编号不进入 JSON；同名符号不构成不同分析之间的相等证明。

feature 在一次构建中统一选择后端；同一个进程里的 Store 使用同一实现。并行比较使用独立 target 目录，避免后一次构建覆盖前一次的二进制。

## 先核对语义，再观察时间

[`scripts/compare-state-backends.sh`](../scripts/compare-state-backends.sh) 构建两份 release 二进制并完成以下比较：

1. 对所有[单账户 `.hex` 例子](../examples/README.md#单账户先把栈和图读懂)比较 disasm、CFG 和 SSA 的完整 JSON。
2. 对所有 [world 例子](../examples/README.md#多账户观察调用怎样影响返回值与状态)比较分析与 SSA 的完整 JSON，以及关闭摘要后的[复用例子](../examples/worlds/summary-reuse.json)。
3. 对 [`missing-code.json`](../examples/worlds/missing-code.json) 额外检查非空、闭合的未知过程关系；另用表达式限制用例检查退出码 2 与保留的部分 SSA。
4. 运行实际 Store 操作的多次测量，比较受测存储观察的 checksum，并要求每个进程内部完整 Store 恢复相等，全部通过后输出中位数与比值。

CLI JSON 比较会对整份报告中的原生图 `identity_scope` 作一致的 producer 命名替换，保留其他身份、对象字段顺序和数组顺序。输入错误、退出码差异、完整输出差异或测量 checksum 差异都会停止脚本，不产生性能结论；Store 存储观察的比较范围见[测量边界](#测量边界)。这些固定例子是回归证据；完整测试与本地门禁仍见[检查流程](local-ci.md)。

```bash
# 默认：64 / 4096 个显式 slot，每项 100 次操作，5 次独立进程重复。
bash scripts/compare-state-backends.sh

# 快速核对，或者按需要放大状态与操作次数。
STATE_BACKEND_SLOTS=64,4096 \
STATE_BACKEND_ITERATIONS=20 \
STATE_BACKEND_REPEATS=3 \
  bash scripts/compare-state-backends.sh
```

构建使用 Cargo 的默认并发，没有额外上限或 CPU 预留。显式设置正整数 `STATE_BACKEND_JOBS` 时，脚本将其传给 Cargo 的 `--jobs`；未设置时，报告的 `build_jobs` 为 `null`，表示由 Cargo 选择。slot 数限定为 1–65536，单项操作次数为 1–10000，重复次数为 1–25。`STATE_BACKEND_OUTPUT` 可更换输出根目录，默认是 `target/state-backend-comparison`。每次运行创建一个带 UTC 时间和随机后缀的独立子目录，并打印完整路径；失败重跑不会混入先前成功运行的聚合结果。

## 测量边界

[`state-backends` example](../crates/evm-abstract/examples/state-backends.rs) 构造一个拥有完整初始 storage 的账户，使用真实 `EvmStore<N>` 与显式构造的 `ValueAlgebra<ProductAlgebra>`。输入、写入值、join 的另一个 Store，以及每项开始前的状态复制都在计时外准备。

样例使用 `new_checked`、`write_checked` 和 `read_checked`，任何表示失败都会停止测量。它按实际负载显式建立有限的 `RelationLimits`：令 S 为初始槽数，W 为 `min(S,8)`，则 `max_nodes=max(1024,6S+3W+6)`，`max_depth=max(64,S+W+3)`，保持关系推理开启和默认约束容量 128。零根占一个节点；每个常量键/值写入最多增加三个节点、一层深度；baseline 与最多 W 次写入的状态合并再增加 ITE 和 selector，检查读取还需临时的读节点与键。每轮恢复原检查点，因此次数不累加图深度。输出的 `relation_limits` 记录实际配置。

| 工作负载 | 一次计时操作包含什么 |
| --- | --- |
| `checkpoint_drop` | 建立整个 Store 检查点并释放它 |
| `sparse_write_restore` | 检查点、最多 8 个已存在 slot 的精确写入、读取一个观察值、恢复检查点并释放被替换的状态 |
| `unknown_alias_write_restore` | 检查点、未知 slot 的有序根更新与显式 slot/default 数值弱更新、读取一个观察值、恢复与释放 |
| `join_drop` | 合并两个 Store、读取一个观察值、释放合并结果 |

计时内使用 `std::hint::black_box` 保留真实操作和读取结果。每项结束后先检查完整 Store 与同进程原输入相等，再在计时外核对恢复后的存储观察。初始化、完整观察采集、序列化和 checksum 均不计入操作耗时。

存储观察读取全部显式槽 `0..S-1`，再读取确定未列出的槽 S，覆盖默认区域。每格必须保留完整有限候选，并允许全部预期候选；缺少有限视图或约束排除预期值都会报错。checksum 保存这些读取的实际数值载体，包括 product 的全部非默认组件；来源、表达式和进程身份不属于这份明确的数值投影，不对原始 Store JSON 作身份改写。

| 验证对象 | 显式槽的预期完整候选 | 未列出的槽 S |
| --- | --- | --- |
| baseline / 恢复后 | 槽 i 为 `{i+1}` | `{0}` |
| sparse write | 前 W 槽为 `{1000000}`，其余保持原值 | `{0}` |
| unknown alias write | 每格为 `{i+1,1000000}` | `{0,1000000}` |
| baseline 与 sparse write 的 join | 前 W 槽为 `{i+1,1000000}`，其余保持原值 | `{0}` |

三个有状态效果都必须与 baseline 的受测观察不同。样例输出 `baseline_storage_observation_checksum`、`effect_storage_observation_checksum`、`restored_storage_observation_checksum`、观察数量和未列槽坐标；`restore_verified` 只在同进程完整 Store 相等检查通过后输出。脚本比较这些观察及有限配置，接受的新样例格式不包含旧的原始整 Store hash 字段。

这些检查覆盖受测存储观察与恢复；它们不证明完整世界、来源关联、所有账户或所有路径语义等价，完整语义仍由回归 suite 检验。样例选择内置 `ProductAlgebra`；泛型采集 helper 保留实际数值载体，不能把某个自定义载体的有限视图当作它的全部语义。

每次重复交替 std / imbl 的启动顺序。运行目录里的 `metadata.json` 记录源码、工具链、主机、参数与两个后端二进制的 hash；全部校验完成后才写入 `completed:true`、`storage_observations_match:true` 和 `restore_verified:true`。CLI 报告比较写入 `parity.json`，操作样本写入 `samples.jsonl`，中位数写入 `medians.json`。`std_over_imbl > 1` 只表示这次输入、配置和机器上 std 中位数耗时更长。此前未检查初始化错误而得到的空操作计时不能用作性能证据。

未知 slot 更新与 `join` 要处理大量抽象值；检查点建立只观察保留旧版本的成本。把这几项分开，才能看出某个后端对当前分析器的收益和代价。这个小测量不包含完整分析工作表的内存峰值，也不能据此断言某个后端始终更快。
