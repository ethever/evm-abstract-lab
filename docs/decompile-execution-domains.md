# 反编译表示的执行域与返回数据上界

`Generic` 是默认执行域。它保留通用原语 host 的语义，不给 `RETURNDATASIZE` 添加交易资源上界。

`EthereumTransactions` 声明表示适用于当前所选 Ethereum fork 下，已纳入有效 Ethereum 区块的交易及其内部调用帧。这是调用方声明的适用范围；选择它不会查询链状态或验证某笔交易。该范围不涵盖任意 `eth_call` 模拟、协议系统调用、使用不同规则的链，或不遵守此范围的测试 host。

执行域记录在配置和 IR 中，IR 的 fork 绑定到 artifact 的输入。producer 与独立 source checker 从同一模块读取上界；更改或移除声明后必须重新验证表示。下面的论证支持 `Cancun`、`Prague`、`Osaka`，新增 fork 需要重新检查 gas 规则和 precompile 集合。

## Gas 前提来自有效区块

Ethereum 的区块 gas 上限不超过 `2^63−1`：geth 的共识 header 校验检查 `params.MaxGasLimit`，该常量定义为 `0x7fffffffffffffff`。处理区块时，交易 gas pool 从区块 gas 上限初始化，交易执行前从中扣除交易声明的 gas。因此，一个被有效区块接纳的交易的初始 gas 也不超过这个值。[共识校验](https://github.com/ethereum/go-ethereum/blob/v1.16.7/consensus/beacon/consensus.go)、[协议常量](https://github.com/ethereum/go-ethereum/blob/v1.16.7/params/protocol_params.go)、[区块处理](https://github.com/ethereum/go-ethereum/blob/v1.16.7/core/state_processor.go)、[交易预扣 gas](https://github.com/ethereum/go-ethereum/blob/v1.16.7/core/state_transition.go)

内部调用从父帧的剩余 gas 中获得预算。转账调用的 2,300 gas stipend 需要放在其已收取的 9,000 gas value-transfer 成本中考虑：即使保守地忽略其他成本和 63/64 限制，子帧也至多获得 `parent_gas−9000+2300 < parent_gas`。gas 不足以支付调用成本时，不会成功建立这个子帧。CREATE 类调用也从父帧剩余 gas 分配预算。执行后的退款不向运行中的帧提供超出原有预算的新 gas。[EIP-150](https://eips.ethereum.org/EIPS/eip-150)、[调用 gas 计算](https://ethereum.github.io/execution-specs/src/ethereum/forks/osaka/vm/gas.py.html#calculate_message_call_gas)

因此，所有相关帧可采用共同的保守预算 `G ≤ 2^63−1`。Osaka 的普通交易还有更严格的 `2^24` 上限；共同论证保留较宽的 header 上限，以覆盖三个已支持的 fork。[EIP-7825](https://eips.ethereum.org/EIPS/eip-7825)

这些前提不来自宿主机器的整数位宽、分析工作预算、某次观测到的 gas，或 `ExecutionLimits::max_memory_bytes`。具体执行 oracle 的默认 1 MiB 内存限制属于测试资源策略。

## 普通 EVM 字节码的输出

三个 fork 的内存扩展费用均包含以下累计成本，其中 `w` 是已扩展的 32 字节 word 数：

```text
Cmem(w) = 3w + floor(w² / 512)
```

若 `w ≥ 2^36`，则仅二次项就至少为 `2^63`，已超过上述帧预算。因此帧内存长度严格小于 `32·2^36 = 2^41` 字节。使用整数不等式即可证明这个保守界，无需模拟整段程序的 gas。[内存费用及扩展计算](https://ethereum.github.io/execution-specs/src/ethereum/forks/osaka/vm/gas.py.html#calculate_memory_gas_cost)

普通字节码通过 RETURN 或 REVERT 产生的非空输出是其已付费内存的切片，其长度也严格小于 `2^41`。空输出直接满足该界。异常退出产生空返回数据；CREATE 成功时的 caller return-data buffer 也为空，CREATE 失败时的 REVERT 数据适用相同的内存论证。[RETURN 实现](https://ethereum.github.io/execution-specs/src/ethereum/forks/osaka/vm/instructions/system.py.html#return_)、[EIP-211](https://eips.ethereum.org/EIPS/eip-211)

## Precompile 输出审计

precompile 的结果不一定经过 callee 的 EVM 内存扩展，因此需要单独覆盖。三个 fork 的完整激活集合可在固定版本的 [geth precompile 表与实现](https://github.com/ethereum/go-ethereum/blob/v1.16.7/core/vm/contracts.go) 中核对。

| 输出来源 | 返回长度依据 |
|---|---|
| IDENTITY，地址 `0x04` | 输出等于调用输入。对于结果可被 EVM caller 的 RETURNDATASIZE 观察的内部调用，输入来自 caller 的已付费内存切片，因此长度小于 `2^41`。直接以 precompile 为顶层交易目标的输出没有外围 EVM caller 来执行 RETURNDATASIZE。 |
| Cancun、Prague 的 MODEXP，地址 `0x05` | 返回长度为 modulus length `n`。EIP-2565 的成本至少为 `floor(ceil(n/8)²/3)`，因为迭代数至少为 1。若 `n ≥ 2^41`，该成本至少为 `floor(2^76/3) > 2^63−1`，无法成功产生该长度输出。 |
| Osaka 的 MODEXP | EIP-7823 将 modulus length 限制为至多 1,024 字节。 |
| Cancun 的其他 precompile：`0x01–0x03`、`0x06–0x0a` | 最大固定输出分别为 32、32、32、64、64、32、64、64 字节。 |
| Prague 新增 BLS12-381：`0x0b–0x11` | G1 结果 128 字节，G2 结果 256 字节，pairing 检查结果 32 字节；最大为 256 字节。 |
| Osaka 新增 P256VERIFY：`0x100` | 成功输出 32 字节；无效签名返回空输出。 |

MODEXP 的输出长度定义与其价格公式见 [EIP-198](https://eips.ethereum.org/EIPS/eip-198)、[EIP-2565](https://eips.ethereum.org/EIPS/eip-2565)；Osaka 长度限制见 [EIP-7823](https://eips.ethereum.org/EIPS/eip-7823)。新增固定长度输出见 [EIP-2537](https://eips.ethereum.org/EIPS/eip-2537) 与 [EIP-7951](https://eips.ethereum.org/EIPS/eip-7951)。上述 precompile 的错误返回为空，满足同一上界。

这覆盖了 EVM caller 可观察的返回数据来源，因而公共模块给出保守的包含式上界：

```text
0 ≤ RETURNDATASIZE ≤ 2^41−1
```

实现从 gas 上限和二次内存公式计算保守的二次幂边界。它没有把 `u64::MAX`、宿主 `usize` 或编译器约定当作原语公理。

## 该界怎样帮助指针证明

例如 Pair 的分配表达式为 `P + ((R + 63) & ~31)`。如果 `R` 保持完整的 256 位范围，模加法确实可能回绕，不能仅凭“free-memory pointer”名称保证结果仍位于低端保留区之外。声明的返回数据上界提供了真实的有限范围；AND 的区间传递、内存内容 phi 与后续 source 检查还需分别证明，才能简化最终错误 payload。

即使 RETURNDATACOPY 的目的端加法不回绕，也不能单独替代分配器证明。令 `M=2^256`、`P=292`、`R=M−325`，则分配后的指针是 4，而复制目的端 `324+R=M−1` 仍未发生 256 位加法回绕。这种数值选择被声明的交易资源域排除；`Generic` 不借用该前提。

测试覆盖域选择、三个 fork、内存与 MODEXP 的临界不等式、固定输出集合及严格字符串传输。fork 匹配没有通配分支；新增 fork 会要求显式更新此处的审计。
