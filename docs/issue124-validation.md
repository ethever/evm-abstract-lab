# Issue #124：未知世界交互的历史结论与回归入口

历史验收绑定源码 `a657cc334c4ef9532ac19f66bf855d796e3a48f4`、main 基线 `a14843430ed975fc71593d52c73fae316a21c385`。当时实现通过了 43 个 unknown-world 集成测试；静态未知摘要闭合仍不等于已知 Router 继续执行完成，报告保留 `Incomplete` 和真实前沿。旧 SMT profile 分别绑定 `b25bf2d35477018dd7e156daab8fb96cfe4226fd` 与 `a657cc3`，不能用于判断当前引擎的成本或性能。

四份生成运行记录已移出 Git 工作树：`issue124-unknown-world.json`、`issue124-router-static-envelope.json`、`native-z3-cost-b25bf2d-review.json`、`native-z3-cost-static-envelope-a657cc3-review.json`。原始记录及嵌入的旧 probe/runner 源码可按文件名从清理前的提交读取，例如：

```sh
git show 3801d1d22a2341d0961b4a9a621771be2de5a375:docs/evidence/issue124-unknown-world.json
```

## 保留的输入与语义要求

[`unknown_world`](../crates/evm-abstract/tests/unknown_world.rs)通过真实已知入口和具体 U bytecode 检查零次、多次、间接、嵌套回调、CALL-family 上下文、静态身份、短 returndata、账户生命周期和分层回滚。[独立 revm 对照](../crates/evm-abstract/tests/unknown_world/oracle.rs)要求一个 abstract outcome 同时覆盖具体终止、完整返回数据、账户状态和日志。

[六个可运行示例](../examples/README.md)保留在 `examples/unknown-world/`：

| 输入 | 必须保留的语义观察 |
| --- | --- |
| [busy-x](../examples/unknown-world/busy-x.json)、[busy-x-nested](../examples/unknown-world/busy-x-nested.json) | 零次与多次回调、嵌套交互，返回保存的 busy，最终恢复 busy |
| [external-change](../examples/unknown-world/external-change.json)、[external-guard](../examples/unknown-world/external-guard.json) | 外部账户的旧值、新值与禁止写入条件，不能混淆不同历史读取 |
| [outer-revert](../examples/unknown-world/outer-revert.json) | 非空回退数据与最终 storage 恢复须同时覆盖；只留下 Failure/空 Revert 不足以通过 |
| [static-reuse](../examples/unknown-world/static-reuse.json) | 静态交互的完整效果上界、返回值和闭合，不能虚构已执行回调块 |

Router 的 21,943 字节[代码](../crates/evm-abstract-web/tests/fixtures/layout/ethereum-uniswap-v2-router02.hex)及 196 字节 getAmountsOut calldata 保留在[fixture](../crates/evm-abstract-web/tests/fixtures/layout/ethereum-uniswap-v2-router02.json)。该入口保留 symbolic ADDRESS/caller/CALLVALUE；当前验收须读取 fixture 的语义要求和实际 typed pending，不能沿用旧报告的节点数或前沿数量。

完整地址空间、创建、未完成依赖和资源拒绝分别检查。较大功能策略与默认额度下的前沿测试都保留；摘要的 closed 标志不能关闭外部尚未完成的工作。当前实现与覆盖条件见[未知世界教程](18-unknown-world.md)。

## 复查命令

```sh
nix develop --no-update-lock-file -c \
  cargo test --locked --offline -p evm-abstract --test unknown_world
```

新的 CLI 对照使用[固定输入与命令生成流程](../examples/no-smt-scale/README.md)，并把生成 profile、完整报告、失败记录和资源测量放在仓库外。比较时保留完整输入与覆盖要求，区分摘要复用、递归闭合、报告交付和可选导出。最终完整门禁见[本地检查](local-ci.md)。
