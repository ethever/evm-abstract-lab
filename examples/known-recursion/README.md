# 已知 bytecode 的递归

`self-return-42.json` 只有一个代码和状态均已知的账户 `0x101`。代码先用 `CALL ADDRESS` 调用自身，然后丢弃 CALL 的成功标志并返回 32 字节的数值 42。它没有先于递归调用的成功返回分支；实际调用拒绝后，父帧仍继续执行 RETURN。

```sh
evm-abstract analyze --world examples/known-recursion/self-return-42.json \
  --evm.to 0x0000000000000000000000000000000000000101 \
  --evm.caller 0x0000000000000000000000000000000000000900 \
  --evm.value 0 --evm.calldata 0x --max-call-depth 32 --max-work 20000000
```

过程摘要必须明确标为 `KnownRecursion`，保持实际 Runtime 代码身份，并记录已经分析的 block。闭合需要有非空的真实返回结果，不能用预算截断或没有终态冒充完成。入口 gas 失败是模型既有的另一种可能；它保持初始 Store 和空返回数据。`--no-summaries` 只关闭跨外层调用的缓存复用，递归过程的闭合仍启用。

`captured/` 保存 issue #123 固定 Ethereum 区块 26180958 的初始观察：USDC 两个账户与两个明确读到的槽，以及 WETH 的代码和四个槽。所有未读到的初始槽仍为未知；这些离线文件不包括后续 RPC 水合轮次，不能直接当作完整 RPC 请求的性能结果。`captured/manifest.json` 记录世界与原请求的 SHA256、代码身份、区块环境及完整离线参数；两个 `*-original-request.json` 保留原始请求字节，caller、value、calldata 仍为 Unknown。

`captured/usdc-100-byte-witness.json` 给出一个独立的具体输入：固定调用者、零 value 和 100 字节 calldata。它属于原 Unknown 输入范围；它不会替代原问题的 Unknown calldata/caller/value 请求。具体 revm 对照为未观察的槽选择零这一种可能，抽象分析仍保留那些槽的未知默认。集成回归实际执行 PC742 的 selfCALL、PC1680 的 selfDELEGATECALL 和 PC2119 的 implementation 写入，并检查父帧发出过日志而最终 Revert 没有保留日志，implementation 恢复初始观察值。

这一完整捕获代码的功能回归读取原请求的 Web 策略：work 上限 1e12、分析深度 1025、内存上限 64 MiB，以及原数值域和 SMT 参数。直接递归、短互递归、调用上下文、回滚和非递归精度回归使用库默认策略，包括深度 32 和 work 上限 20M。两个较复杂的 symbolic fixture 保留同一输入、深度和精度策略，把功能回归的 work 上限明确设为 100M：一个增长 persistent/transient 状态，另一个通过静态互递归传递 128 字节上下文。独立回归检查它们在完整默认 20M 下留下真实 Work 前沿。预算不同的结果分别验收；默认预算的未完成报告不能被描述成过程已闭合。

具体对照由 revm 执行字节码和处理调用、gas、journal、回滚。测试汇编器只解析标签，Inspector 只观察实际步骤、SSTORE 操作数和日志，没有替换执行结果。每个具体 witness 必须被同一个抽象 outcome 同时覆盖：终止类型、整份返回数据、所观察的槽、捕获账户余额和日志都一起比较。调用者选取足够支付 gas 的具体余额，交易 gas 费用不混入捕获账户效果；已知区块环境保持原观察值。有限 witness 检查实际语义，closed 标志、非空 root 输出和已分析的 Runtime block 则独立检查过程关系的闭合。

真实 Unknown USDC/WETH 用例属于性能与覆盖实验，放在 `captured/` 中供基准脚本选择，不应无差别加入每次安装后示例门禁。
