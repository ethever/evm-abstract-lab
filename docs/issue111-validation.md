# Issue #111：符号状态的历史结论与回归入口

历史验收绑定源码 `6a95223b12248b0e615c2fd7e49cfcfd7f969373`：当时的 SMT 实现通过了 45 个测试、128 次分析，并为预期的资源边界保留 `Incomplete`。这些结论属于该版本；当前实现的范围见[符号状态教程](17-symbolic-state.md)。逐例运行记录已移出 Git 工作树，原文件可从清理前的提交读取：

```sh
git show 3801d1d22a2341d0961b4a9a621771be2de5a375:docs/evidence/issue111-symbolic-state.json
```

## 保留的输入与语义要求

[`symbolic_state.rs`](../crates/evm-abstract/tests/symbolic_state.rs)及其子模块保留真实 bytecode fixture。账户、余额、storage 和交易环境由输入显式设置；未知 calldata、槽和地址继续保持未知，不能把某个具体模型的取值写回初始观察。

- [字节](../crates/evm-abstract/tests/symbolic_state/bytes.rs)、[memory](../crates/evm-abstract/tests/symbolic_state/memory.rs)与[传输](../crates/evm-abstract/tests/symbolic_state/transfers.rs)：大端 word、部分覆盖、重叠复制、零大小、不回绕的范围和完整未知 calldata 转发。
- [调用](../crates/evm-abstract/tests/symbolic_state/calls.rs)与[创建](../crates/evm-abstract/tests/symbolic_state/creation.rs)：CALL-family 的 owner/caller/value/static 区别、重入、CREATE/CREATE2 生命周期及分层回滚。
- [storage](../crates/evm-abstract/tests/symbolic_state/storage.rs)、[日志](../crates/evm-abstract/tests/symbolic_state/logs.rs)与[收敛](../crates/evm-abstract/tests/symbolic_state/convergence.rs)：别名、历史读取、日志捕获旧 memory，以及循环的覆盖和明确前沿。

[revm 对照](../crates/evm-abstract/tests/symbolic_state/oracle.rs)独立执行具体 fixture，检查同一个抽象 outcome 对终止、完整返回数据和账户效果的联合覆盖，并核对实际帧、指令与块入口栈。日志另由上述日志回归检查；有限见证不代替一般覆盖论证。未执行的指令不能算作 SSA 执行证据，未完成路径须保留 pending 与 frontier。

Multicall3 的原 bytecode、aggregate3 calldata 和具体返回见证继续保存在[运行时 fixture](../crates/evm-abstract-web/tests/fixtures/layout/ethereum-multicall3.json)。历史版本曾交付带前沿的部分图；图规模和某次 UI 回放不能证明原请求已完成。独立图压力回归与具体合约的执行精度分别验收。

## 复查命令

在待检查的源码版本运行真实测试；当前策略与断言由该版本的测试定义：

```sh
nix develop --no-update-lock-file -c \
  cargo test --locked --offline -p evm-abstract --test symbolic_state
```

新的日志、报告和资源测量放在仓库外，记录源码、构建身份、完整输入、命令和实际退出码。最终提交的完整门禁按[本地检查](local-ci.md)执行并记录于 PR。
