# 10：固定事实、复用关系与改变代码

本课先运行四个小世界，再解释为什么缓存命中、代码部署和销毁都需要明确的事实边界。四个例子都以 `0x...0101` 为入口，使用合成离线状态，默认预算即可完成。

## 同一个 callee 怎样被执行两次

```bash
nix run . -- analyze --world examples/worlds/summary-reuse.json --entry 0x0000000000000000000000000000000000000101 --format json > /tmp/summary-on.json
nix run . -- analyze --world examples/worlds/summary-reuse.json --entry 0x0000000000000000000000000000000000000101 --format json --no-summaries > /tmp/summary-off.json
jq '.summary_stats, [.summaries[] | {source_state, state_count, edge_count, reused_at}]' /tmp/summary-on.json
```

A 两次 CALL B；B 只读取自己的 slot 0=7 并返回 32 字节。第一次 CALL 不复制输出，第二次复制 32 字节。你应看到 `hits > 0`，并在某个证书的 `reused_at` 找到第二次 callee 入口。关闭摘要后 `hits=0`，仍然完成图和 SSA。比较联合最终关系时去掉图中的状态编号：

```bash
jq -S '[.outcomes[] | {kind,data,store}] | unique' /tmp/summary-on.json > /tmp/summary-on-relations.json
jq -S '[.outcomes[] | {kind,data,store}] | unique' /tmp/summary-off.json > /tmp/summary-off-relations.json
cmp /tmp/summary-on-relations.json /tmp/summary-off-relations.json
```

缓存保存的是“抽象输入 → RETURN/REVERT/失败、返回字节与 Store 效果”的完整关系。证书来自普通工作表已经闭合的 callee 图，包括更深调用；复用时把这些指令状态、内部边和返回边接到新的暂停 caller 上。因此命中之后图里仍有实际 callee 指令，`--ssa` 仍能核对每条调用与返回。

命中要求精确输入相等，不能只比较地址或函数 selector。键包含固定 fork、typed snapshot identity、初始事实 fingerprint、当前可执行代码 hash、callee 帧、ORIGIN、完整 Store、剩余调用深度和精度策略。完整 Store 覆盖 storage/transient、余额、日志、代码 overlay、nonce 和生命周期；任一相关事实变化都导致 miss。callee 的 caller/static/value/calldata 等环境也要相等。

暂停 caller 和 caller 所拥有的输出复制 continuation 不属于 callee 的输入，因此两个调用位置或输出长度可以不同。其余帧事实与 rollback checkpoint 仍是前置条件。当前缓存属于一次固定世界分析，没有跨分析的持久缓存。改变输入、join 导致输入扩大、未完成 callee 或预算中断，都不能发表一个可复用的完整证书。

查找比较、快照 hashing、认证、复制和图导入消耗同一个 `max_work` 账本；复用图的状态也计入全局状态预算。命中不会获得第二份预算。`SummaryWork` 表示关系查找/认证/导入未完成，此时状态为 `Incomplete`，SSA 不接受未闭合图。实现与回归见 [`summary.rs`](../crates/evm-abstract/src/analysis/summary.rs)、[`summaries.rs`](../crates/evm-abstract/tests/summaries.rs)。

## CREATE 的结果是新代码和新状态

```bash
nix run . -- analyze --world examples/worlds/create-runtime.json --entry 0x0000000000000000000000000000000000000101 --format json --ssa > /tmp/create.json
jq '[.analysis.states[] | .key.frames[-1] | {address,mode,code_hash}], [.analysis.outcomes[].store.account_observations[] | select(.address == "0xea53a153a9a04fd632b2486d84732feb3b71afb7")]' /tmp/create.json
```

A 的 nonce 初始为 0。CREATE 地址为 `0xea53a153a9a04fd632b2486d84732feb3b71afb7`；输入显式声明它 `existence:"absent"`、空代码、nonce/余额为零、完整零 storage。缺少这个目标账户会留下 `Creation(UnknownCollision)`，不能凭“JSON 没有列出来”猜它不存在。例子也声明零地址为空且不存在，覆盖模型允许的创建失败后 CALL 零地址的分支。

```mermaid
flowchart LR
    A[creator nonce +1] --> B[InitCode 帧与新账户]
    B --> C[RETURN 得到 runtime]
    C --> D[Store 安装代码 hash]
    D --> E[随后 CALL 执行新 runtime]
```

图中的 `mode=InitCode` 与 `mode=Runtime` 分别标识构造阶段和部署后的执行。新 runtime 为 `602a5f5260205ff3`，8 字节，返回数值 42；hash 为 `0x30962a84ef989ca0f724a5b2ec94f9cbf6a731752ce2c0be5333bf96e460c9fd`。成功路径中，creator nonce 和新账户 nonce 都为 1。原始 world 中的目标仍是 absent；最终 Store 则能出现 present 的 runtime。这是代码 overlay 的意义。

CREATE 需要有限且能表示的 creator nonce/endowment、具体 initcode 和明确的碰撞事实；CREATE2 还需要有限 salt，用 initcode hash 决定地址。未知 nonce、salt、initcode、runtime 或碰撞事实都有 typed `Creation` 前沿。initcode 在新帧中执行，起始 storage/transient 归属新账户；通过代码长度、前缀和解码检查后才安装 runtime。失败 initcode 保留本次创建前已经增加的 creator nonce；祖先 REVERT 则通过更早 checkpoint 恢复它。静态限制、深度、EIP-3860 和代码限制同样影响结果。

gas 仍是保守模型，所以抽象结果同时覆盖创建/调用失败；不能把“存在返回 42 的 outcome”读成“所有路径都返回 42”。这些关系由 [`creation.rs`](../crates/evm-abstract/tests/creation.rs) 的独立 revm 轨迹核对。

## SELFDESTRUCT 先转账，最后删除

```bash
nix run . -- analyze --world examples/worlds/created-selfdestruct.json --entry 0x0000000000000000000000000000000000000101 --format json > /tmp/destroy.json
jq '[.states[] | select(.entry.store.pending_destruction["0x2fd1832070091785c7e2aa8b7d3464a3e23a4eeb"] == true) | .id], [.outcomes[].store.account_observations[] | select(.address == "0x2fd1832070091785c7e2aa8b7d3464a3e23a4eeb")]' /tmp/destroy.json
```

A 用 CREATE2、salt=5、endowment=7 部署 8 字节 runtime `60015f55610200ff`，地址为 `0x2fd1832070091785c7e2aa8b7d3464a3e23a4eeb`。它写入自己的 slot 0，再把余额转给 `0x...0200` 并 SELFDESTRUCT。caller 恢复后，EXTCODESIZE 仍能读到 8，EXTCODEHASH 仍是 runtime hash；之后还可以再次 CALL 这些代码。caller 把这两个观察保存到自己的 slot 1/2。

所有支持的 fork 都采用 [EIP-6780](https://eips.ethereum.org/EIPS/eip-6780)：同交易创建的账户延迟到最外层成功完成才删除。在完整销毁路径的最终 Store 中，新账户为 absent，代码/nonce/余额/storage 为零，受益人得到 7。执行中的 `created`、`pending_destruction` 与最终 account observations 是不同时间点；事务结束会清除这些临时标志。祖先 REVERT 恢复余额、代码、slot 和待删除状态。

预先存在的账户 SELFDESTRUCT 时不删除代码或 storage；向不同受益人转移余额，向自身转移则保留预先存在账户的余额。把它与同交易新账户的删除合并成“立刻把代码设为空”，会漏掉恢复 caller 后的真实调用。

## 原生预编译也有调用与返回边

```bash
nix run . -- analyze --world examples/worlds/identity-precompile.json --entry 0x0000000000000000000000000000000000000101 --format json --ssa > /tmp/native.json
jq '[.analysis.states[] | .key.frames[-1].mode], [.analysis.outcomes[] | {kind,data}]' /tmp/native.json
```

A 在 memory[0..32] 放置数值 42，CALL identity 地址 `0x4`，把输出写到另一个 memory[32..64] 区域再返回。成功路径返回 42；保守 gas 失败分支仍能保留未填充输出区的零。图中有 `Precompile(0x4)` 模式和真实 Call/Return 边，完整图可构建 SSA。预编译由所选 fork 的注册表决定，不需要伪造普通账户 runtime；未提供的账户 presence/balance 事实仍保持未知。

原生密码学复用固定 [`revm-precompile`](https://docs.rs/revm-precompile/43.0.3/revm_precompile/)，分析器负责具体输入资格、返回流与执行前的保守工作预留。未知字节/长度或无法表示的 modexp 长度留下 `PrecompileInput`；累计工作不足留下 `Work`。原生失败与无法调用后端的模型边界分别处理。它没有补齐精确 gas 或全部环境关系。

## 固定快照的名字不能代替身份

旧 JSON 的 `provenance` 仍保存描述，但被标为 offline；新增离线输入可以明确写：

```json
{"kind":"offline","label":"synthetic-demo:v1"}
```

链上导出把它替换为 `identity:{"kind":"chain","chain_id":"0x1","block_hash":"0x…32字节…"}`，仍单独选择 `fork`。每项已观察代码必须带 matching `code_hash`；运行时代码按真实原始字节计算 Keccak，委托标记按原始 23 字节计算，确认 absent 的账户 hash 为零。单个 hash 不绑定全部 storage/balance/nonce，因此 `fingerprint` 另外绑定 fork、identity 和完整初始事实。输入可附带预期 fingerprint，解析器拒绝失配、重复地址/slot、冲突代码 hash 或不合法 absence 事实。

`existence` 为 `unknown`、`present` 或 `absent`；空代码与不存在是两个事实。省略 nonce/presence/balance 时未知，省略 slot 时默认未知。`storage_unknown:false` 是完整初始 storage 的声明，只适用于明确知道所有未列 slot 为零的输入；RPC 采集只取得请求的 slot，其余保持未知。

要使用网络输入，先明确选择提供者、chain id、exact block hash 和所需账户/slot。以下变量必须由你设置为已经选定的快照；命令不会替换成 latest：

```bash
nix run . -- analyze --rpc "$LAB_RPC_URL" --chain-id 0x1 --block-hash "$LAB_BLOCK_HASH" --fork osaka --entry 0x0000000000000000000000000000000000000101 --account 0x0000000000000000000000000000000000000200 --slot 0x0000000000000000000000000000000000000200:0x0 --format json
```

入口自动加入采集账户；`--account` 和 `--slot ADDRESS:SLOT` 可重复，slot 会加入其所属账户。loader 在分析前校验返回 chain id 和 exact block hash，所有 code/balance/nonce/proof/storage 请求都使用 [EIP-1898](https://eips.ethereum.org/EIPS/eip-1898) 的 `{blockHash,requireCanonical:true}`。不支持这个 selector、缺少区块/结果、identity/代码 hash/state 冲突、JSON-RPC/HTTP/网络/超时错误都会带着 chain/block/method/account/slot 来源退出 1；不会改查 block number 或 moving tag，也不会把缺失响应补成空代码或零余额。没有被预先选择的后续 CALL 目标保留 `MissingCode`。

默认每个 RPC 请求超时 15 秒，响应最多 4 MiB；库 API 可显式选择有界限制。当前边界信任选定的 RPC 提供者，交叉校验 `eth_getProof` 与其余观察，但不验证 [EIP-1186](https://eips.ethereum.org/EIPS/eip-1186) Merkle proof。typed identity 和代码 hash 消除混合/冲突输入，不能把受信任的 RPC 数据提升为密码学状态证明，更不能把 `Converged` 提升为任意合约安全证明。

text/JSON/DOT 都显示 snapshot identity/fingerprint、frame 模式/hash 与摘要信息。JSON 保留完整 initial world、证书输入/输出、transaction Store overlay 和便于查看的 `account_observations`；DOT 的蓝色证书节点标记认证来源与复用位置。源码分别位于 [`world/snapshot.rs`](../crates/evm-abstract/src/world/snapshot.rs)、[`world/rpc.rs`](../crates/evm-abstract/src/world/rpc.rs)、[`transfer/create.rs`](../crates/evm-abstract/src/analysis/transfer/create.rs) 和 [`transfer/precompile.rs`](../crates/evm-abstract/src/analysis/transfer/precompile.rs)。
