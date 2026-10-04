# 06：知道结果能证明什么

本课目标：区分模型内收敛、精度下降、语义/资源前沿和具体证据，理解固定世界怎样提高跨合约分析精度。

## 三句话分别在说不同事实

“工作表收敛”说明已经算完本抽象模型内的传播闭包。“图覆盖某条 revm 轨迹”说明那个具体样本没有被遗漏。“合约不会出现某种行为”需要针对实际 EVM 状态空间、假设和目标性质的证明。

本仓库提供前两类可检查证据，没有声称第三类。一次成功的抽象调用和一个完成的世界分析，也不等于所有实际交易都成功。

## 本模型如何看待 EVM 的各部分

| 部分 | 当前世界模型 | 影响 |
| --- | --- | --- |
| 输入 | 多账户 JSON 或显式固定链/区块 hash RPC，明确入口环境，固定 fork、typed identity 与事实 fingerprint | 无隐式 RPC；未知事实保持未知；RPC 提供者受信任，不验证 Merkle proof；不支持 EOF |
| 栈与纯运算 | 高度 ≤1024、有限常量集合或 Top；U256 算术、补码、布尔运算与 Osaka CLZ | 集合容量和逐槽独立性影响精度，没有完整关系/路径约束 |
| 调用身份 | 每帧 code address、state address、caller、call value、static 与继续位置分别保存 | 共享代码不代表共享 storage；CALLCODE/DELEGATECALL 保留各自的环境规则 |
| 内存与数据 | 每帧抽象字节数组、MSTORE/MSTORE8/MLOAD、copy、calldata/returndata 与返回区传播 | 未知偏移/字节会降低精度；有类型的内存前沿限制追踪范围 |
| storage / transient storage | 按账户保存，单一 slot 强更新，多目标/未知别名弱更新 | 不把 SLOAD 固定替换为初始快照；后续读取考虑写入与重入；transient 状态只属于本次分析 |
| 余额 | 初始抽象余额与 CALL 值转移，不足余额的调用失败分支 | 不计交易 gas 费用；未知余额不可以当作零 |
| CALL / CALLCODE / DELEGATECALL / STATICCALL | 读取当前代码 overlay，暂停 caller、传入 calldata、返回成功位和字节 | 非有限目标、未知代码留下 `Incomplete`；已完成关系满足精确前置条件时可复用完整图 |
| 成功 / REVERT / 故障 | 成功保留状态，REVERT/故障恢复调用前整份 Store，REVERT 保留返回数据 | 子调用及其更深调用的效果一起回滚；caller 先前的效果保留 |
| static 限制 | 子调用继承限制，禁止写入和有值 CALL 的异常继续返回 | 失败不能当作一次成功的无副作用执行 |
| EIP-7702 | 固定世界提供目标代码时解析标记，保留委托账户的状态身份 | 不处理授权交易列表、nonce 或授权签名 |
| JUMP/JUMPI | 有限目标验证，Top 覆盖真正 JUMPDEST；按条件零/非零剪枝 | 未知跳转可能产生伪边；帧内历史不等于内部函数恢复 |
| 环境 / hash / gas | 已知帧环境和代码信息传播，其余未知环境、hash 和 gas 保守抽象 | 不精确计 gas、EIP-150、out-of-gas 或经济成本；`Converged` 只在这个模型内成立 |
| LOG | 按指令来源保存可能日志的账户、topics 和数据，随 Store 回滚；static 中发日志故障 | 不精确恢复顺序和次数；未知效果由 logs_unknown 标记 |
| CREATE / CREATE2 | 有限 nonce/endowment/salt、可表示 initcode、明确碰撞事实；新帧执行 initcode，验证后安装 runtime | 未知 nonce、碰撞状态、initcode/runtime 或 salt 保留 typed `Creation` 前沿；失败及祖先 REVERT 恢复适当 checkpoint |
| SELFDESTRUCT | 所有支持的 fork 使用 EIP-6780；转移余额，同交易创建的账户在最外层完成时删除 | 事务中代码仍可读/调用；预先存在的账户保留代码/storage；不知道 beneficiary 时保留边界 |
| 预编译 | 按 fork 调用固定 `revm-precompile`，传播具体有限输入和返回/失败，执行前预留工作 | 未知输入或不可表示的长度为 `PrecompileInput`；资源不足为 `Work`；不声称精确 gas |
| 完整调用摘要 | exact input equality，固定世界/fork/代码 hash、完整帧/Store/ORIGIN/精度及深度策略；保留认证图与全部输出关系 | callee 输入、overlay、nonce、生命周期、storage/transient/balance/logs 的变化导致 miss；未闭合或被中断的关系不发布 |
| 资源预算 | 所有账户共用状态、transfer 和累计工作预算，另有限深度及内存预算 | `Incomplete` + typed frontier，退出 2；跨合约 SSA 拒绝未完成图 |

已知空代码是已完成的无代码执行；缺少代码是待分析区域。二者必须区分。`OpaqueResult` 说明某项数据精度被抽象；`UnknownJump` 说明跳转目标精度不足；模型内异常和无法覆盖的语义前沿同样需要分别阅读。

## 世界事实与执行状态是两种东西

世界快照不会在执行期间被改写。结果保存原始 fork、typed identity、fingerprint、provenance 和入口假设，另由 transaction Store 保存后续 storage/transient、余额、nonce、代码与创建/待销毁状态。调用时保存 checkpoint；成功提交，REVERT/故障恢复；最外层成功完成才提交延迟删除。

输入余额与 nonce 属于执行帧入口状态；入口 `value` 只决定 CALLVALUE。外层交易转账、手续费、发送方交易 nonce 增加与授权列表不由本模型执行，也不会被重复施加。CREATE/CREATE2 引起的合约 nonce 变化属于本次执行的 Store。

```text
固定世界 + 入口假设
        ↓
初始抽象 Store
        ↓
写入更新 / 调用子帧 / 当前状态上的重入
        ↓
成功保留或失败回滚
        ↓
返回 caller 的字节、成功位和最终 Store
```

在某个固定区块，已知 slot 0 的值可以收窄 SLOAD。随后 SSTORE、DELEGATECALL 或重入可能覆盖它，因此只把每次 SLOAD 都替换成节点采集时的值会删掉真实行为。未知 slot 写入要保守扩大可能别名；缺少代码时要保留未完成区域，不能猜测外部没有副作用。

CLI 的 `storage_unknown` 默认 true。只有声明 false 的账户才把未列出的初始 slot 当作零。示例的完整合成状态与真实部分采集快照有不同的假设，输出必须保留这一区别。

## 工程证据覆盖什么

- 域性质测试核对 join 的交换、结合、幂等与覆盖；纯运算对照 revm 的边界值和随机 256 bit 输入。
- 单账户字节码回归检查 PUSH 边界、JUMP、栈故障、SSA φ、支配和 def-use；具体轨迹核对 block 入口、边、出栈与 SSA 值。
- [`cross_concrete.rs`](../crates/evm-abstract/tests/cross_concrete.rs) 在 Cancun/Prague/Osaka 对照离线 CALL 返回分支、共享实现代理、CALLCODE、returndata copy、REVERT、static 写入故障、日志回滚和重入。它要求 oracle 实际访问入口及子帧，核对指令和帧身份、block 入栈，以及同一抽象 outcome 对返回数据、storage、合约余额和日志的联合覆盖。
- 跨合约 SSA 验证器检查与实际完整机器图的一致性；回归测试使损坏的转移与效果流被拒绝。
- [`summaries.rs`](../crates/evm-abstract/tests/summaries.rs) 比较摘要开启/关闭后的联合 outcome 关系、实际图/SSA 与独立 revm 轨迹，并覆盖输入失配、未完成关系和共享预算中断。
- [`creation.rs`](../crates/evm-abstract/tests/creation.rs) 让 revm 实际进入 initcode 和后续 runtime，核对 CREATE/CREATE2、nonce、碰撞、回滚、EIP-3860/代码限制及 EIP-6780 的事务内可见性和最终删除。
- [`precompiles.rs`](../crates/evm-abstract/tests/precompiles.rs) 与固定 native 实现核对各 fork 的返回/失败、identity 调用返回区及输入/工作前沿。RPC 回归使用真实 localhost HTTP，检查固定 selector、缺失/冲突事实、超时和无 moving-tag 回退。
- 真实 CLI 测试核对 JSON/文本/DOT、输入错误和退出码，安装后的 Nix 二进制运行离线世界并渲染图。

有限测试能发现反例，不能证明全部合约、calldata、所有 256 bit 参数或完整 EVM 规则均被覆盖。增加精度时应同时展示哪个例子改变了、新假设是什么，以及是否仍覆盖独立具体轨迹。

## 继续研究时保持证据范围

后续可以加入更强路径约束、关系域、内部函数恢复、精确 gas、经过密码学验证的区块状态和跨交易不变量。现有创建/销毁、原生调用与固定 hash 采集的假设见[第 10 课](10-snapshots-summaries-creation.md)，继续扩展时应保持事实、状态与未完成前沿的边界。
