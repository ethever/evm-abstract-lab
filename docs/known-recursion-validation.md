# 已知代码递归：历史结论与回归入口

历史观测绑定源码 `4212e5addd3672a9becdb7cd01d0ded75d836eb9`、main `f70afa2b383d574649831c3ff10a885947a1126a`（native8 / wire13）。该版本已将已知递归接入共享过程不动点；原 WETH 请求交付 `Incomplete`，原 USDC 请求被外部超时终止，没有最终报告。这些观测不能证明原 Unknown 请求已完成，也不用于判断当前版本性能。

生成运行记录已移出 Git 工作树；完整原记录可从清理前的提交读取：

```sh
git show 3801d1d22a2341d0961b4a9a621771be2de5a375:docs/evidence/known-recursion-selected-validation.json
```

## 保留的输入与语义要求

[输入清单](../examples/known-recursion/captured/manifest.json)、[原 USDC 请求](../examples/known-recursion/captured/usdc-original-request.json)、[原 WETH 请求](../examples/known-recursion/captured/weth-original-request.json)及初始 observed worlds 保留原始字节与 SHA256。它们固定 Osaka、Ethereum chain 1、区块 26180958 / `0x5219691ff60b20005fa597d2bb5e4b11e3ddb68f24fb528a900c8dafbb84b499`；caller、call value、calldata 内容与长度保持 Unknown，未观察的槽保持未知。

[100 字节 USDC 见证](../examples/known-recursion/captured/usdc-100-byte-witness.json)是独立的具体输入，不替换原请求。[捕获回归](../crates/evm-abstract/tests/known_recursion/captured.rs)实际执行 selfCALL、selfDELEGATECALL 和 implementation 写入，并检查外层 Revert 后的 storage 与日志回滚。

[`known_recursion`](../crates/evm-abstract/tests/known_recursion.rs)及[可运行自调用示例](../examples/known-recursion/README.md)保留下列验收边界：

- 活动栈上重复调用边与先后返回的普通调用分别处理；owner、code、static、返回控制和回滚检查点不能混淆。
- 输入增长使依赖结果重新计算；无真实终态、仍有依赖或前沿时，不能把摘要命中称为闭合。
- 终止、返回数据、账户效果和日志由[独立 revm 见证](../crates/evm-abstract/tests/known_recursion/oracle.rs)联合检查。
- 深度与 work 资源不足时保留真实前沿。历史深度 3 的闭合用例与深度 2 的 `CallDepth` 对照分别验收，不能把内部摘要闭合当作外层完成。

数值摘要可能扩大参数到输出、跨帧 storage 的关系；`--no-summaries` 仍保留递归闭合，不是逐层展开的独立对照。旧 Z3 插桩结果仅描述该历史版本，不属于当前执行路径。

## 复查命令

```sh
nix develop --no-update-lock-file -c \
  cargo test --locked --offline -p evm-abstract --test known_recursion
```

固定离线与 RPC acquisition 是不同输入边界。当前 CLI profile 和 HTTP 请求按[固定规模检查流程](../examples/no-smt-scale/README.md)从实际构建源码派生，保留原始 capture；新的运行输出放在仓库外。报告闭合、任务完成、取消和导出结果分别读取，完整提交门禁按[本地检查](local-ci.md)执行。
