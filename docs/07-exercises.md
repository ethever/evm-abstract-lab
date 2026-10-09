# 07：按步骤把知识变成实验

学习入口：[模块：边界、证据与验证](modules/evidence.md) · [理论：按主题核对推导](routes/theory.md#evidence) · [实现：按主题核对实现](routes/implementation.md#evidence) · [选择路线](learning-routes.md)。

先完成 [00 的首个实验](00-start.md)，再按[所选路线](learning-routes.md)在相应主题后做练习。解码与协议实验对应执行规则，有限集合与组合域实验对应数值摘要，SSA 与 φ 实验对应值流，历史分组实验对应敏感性；世界实验配合[第 09 课](09-cross-contract.md)。后面的[SSA 优化](#10-进阶设计消除多余-φ)、[分支约束](#11-进阶设计让-true-分支记住-x5)、[边解释](#12-进阶设计解释一条边为什么存在)设计用来区分当前能力和需要新增的规则。

每个练习都用同一种方法：

1. **预测**：写出将看到的 pc、栈或边，暂时不看提示。
2. **运行**：在仓库根目录执行命令，栈按栈底 → 栈顶读。
3. **解释差别**：指出哪一个输入、合并或规则造成结果。
4. **验收**：按折叠提示核对具体事实；不以“命令没报错”代替理解。

## 1. 区分 PUSH 数据与真正的 JUMPDEST

字节码 `605b005b00` 含有两个 `5b`。先列出全部指令地址，再回答：哪一个是合法跳转目标？它一定可达吗？

```bash
nix run . -- disasm --hex 605b005b00
nix run . -- cfg --hex 605b005b00
```

<details><summary>提示与验收</summary>

| pc | 内容 |
| --- | --- |
| `0x00` | PUSH1 0x5b；后面一个字节是数据 |
| `0x02` | STOP |
| `0x03` | 真正的 JUMPDEST |
| `0x04` | STOP |

disasm 有 B0、B1 两个块；CFG 只有入口 S0，没有边。`0x03` 是合法目标，但本程序入口在 STOP 处结束，没有跳到它。验收要同时解释解码结果和可达图。

</details>

## 2. 先弹出的操作数是谁

先画出每条 PUSH 后的栈，再预测 SUB 与 DIV 的结果：

```bash
nix run . -- cfg --hex 6002600303
nix run . -- cfg --hex 6003600203
nix run . -- cfg --hex 6002600304
nix run . -- cfg --hex 6003600204
```

这四段代码只交换两个输入或把末尾 opcode 从 SUB 改为 DIV。为什么不能简单按 PUSH 的先后顺序写算式？

<details><summary>提示与验收</summary>

| 程序 | 运算前栈 | 实际算式 | 出栈 |
| --- | --- | --- | --- |
| 前两个 PUSH 为 2、3，SUB | `[2,3]` | 3−2 | 1 |
| 前两个 PUSH 为 3、2，SUB | `[3,2]` | 2−3，模 `2^256` | U256::MAX |
| 前两个 PUSH 为 2、3，DIV | `[2,3]` | 3/2，整数除法 | 1 |
| 前两个 PUSH 为 3、2，DIV | `[3,2]` | 2/3，整数除法 | 0 |

U256::MAX 显示为 64 个十六进制 `f`。源码里的 `args[0]` 是先弹出的栈顶；读 [`domain.rs`](../crates/evm-abstract/src/domain.rs) 时要按这个顺序代入。若修改运算实现，应使用独立 revm 对照，防止测试和实现复制同一个顺序错误。

</details>

## 3. 改容量：同一程序为何变得不精确

预测 [diamond](../examples/diamond.hex) 在 `pc=0x0e` 的入口值，以及加 10 后的值：

```bash
nix run . -- cfg --file examples/diamond.hex --domain constants-only --context-depth 0 --max-constants 1
nix run . -- cfg --file examples/diamond.hex --domain constants-only --context-depth 0 --max-constants 2
```

记录两次的 `status`、汇合点 `stack in`、`stack out`。增加容量改变了程序还是分析表示？

<details><summary>提示与验收</summary>

容量 1：`{1} ⊔ {2}=Top`，加 10 后仍为 Top。容量 2：入口 `{0x1,0x2}`，出口 `{0xb,0xc}`，即 `{11,12}`。两次都为 `Converged`，字节码和合法行为没有改变。

集合容量增大能保留更多常量，但逐槽集合仍不保存槽位间的配对关系。若两条路径的两个槽位分别是 `[1,10]` 和 `[2,20]`，逐槽合并会允许 `[1,20]` 这种额外组合；容量足够也无法自动消除它。

这里显式使用 `constants-only`，以便直接检查有限集合的基石规则。默认 `product` 在常量组件不能枚举时仍可能保留范围、位和同余约束；其区别在[练习 8](#8-组合域位信息与相等关系各自改善什么) 中观察。

</details>

## 4. 用名字追踪 DUP、SWAP 和 φ

先手写栈中的值名字，再运行：

```bash
nix run . -- ssa --hex 60018060029000
nix run . -- ssa --file examples/diamond.hex --context-depth 0
```

回答：最终有三个栈槽位，为什么只有两个值定义？[diamond](../examples/diamond.hex) 的 `%0` 为什么不是第一条 PUSH？从 S1 进入汇合点时，φ 接收哪个名字？

<details><summary>提示与验收</summary>

第一个程序 `values=2`，出栈 `[%0,%1,%0]`。DUP 复制引用，SWAP 改变位置，都不产生新值定义。

[diamond](../examples/diamond.hex) 为入口槽位先分配 `%0`，所以第一条 PUSH 是 `%1`。汇合点是 `phi(S1: %4, S2: %5)`；从 S1 来取 `%4`，从 S2 来取 `%5`。`slot 0` 表示入口栈底位置，`%0` 表示定义身份，`abstract {0x1,0x2}` 表示可能数值；三者应分别解释。

</details>

## 5. 未知跳转后，副作用不能消失

运行 [dynamic-jump](../examples/dynamic-jump.hex)，定位 `pc=0x09` 的 SSTORE：

```bash
nix run . -- explain --file examples/dynamic-jump.hex
```

手算一个具体输入：32 字节 calldata 的前 31 字节为零，末字节为 `04`。JUMP 去哪里？SSTORE 写哪个 slot、什么值？随后给原程序末尾追加 `JUMPDEST; STOP`，预测图的变化：

```bash
nix run . -- cfg --hex 600035565b602a600055005b00
```

<details><summary>提示与验收</summary>

具体输入使 CALLDATALOAD 得到 4，跳到 `pc=0x04`，随后写 slot 0=42。单段分析的 calldata 未知，所以目标是 Top；原图保留到 `0x04` 的边和 `UnknownJump`，SSA 保留 SSTORE 的两个参数，即使它不产生返回值。

追加后，真正的 JUMPDEST 是 `0x04`、`0x0b`，图有两条 Jump 边，还保留可能异常的诊断。不要把 PUSH 数据中的 `5b` 算作目标，也不要因为目标未知就删掉正常后继。

</details>

## 6. 观察调用来源怎样被遗忘

先比较原示例：

```bash
nix run . -- cfg --file examples/internal-calls.hex --context-depth 0
nix run . -- cfg --file examples/internal-calls.hex --context-depth 1
nix run . -- cfg --file examples/internal-calls.hex --context-depth 2
```

再比较下面的变体。它在 helper 中加入 JUMPI，true 和 false 都进入 `pc=0x15`；这个安排保持返回逻辑，专门观察一次额外分支如何占用历史：

```bash
nix run . -- disasm --hex 6005600e565b50600c600e565b005b6000356015575b602a9056
nix run . -- cfg --hex 6005600e565b50600c600e565b005b6000356015575b602a9056 --context-depth 1
nix run . -- cfg --hex 6005600e565b50600c600e565b005b6000356015575b602a9056 --context-depth 2
```

记录 `pc=0x0e` 和 `pc=0x15` 各有哪些 context，以及入口返回地址集合。

最后检查默认值、更大的合法深度，以及资源边界：

```bash
nix run . -- cfg --file examples/internal-calls.hex --format json | jq '.config.context_depth, .status'
nix run . -- cfg --file examples/internal-calls.hex --context-depth 256
nix run . -- cfg --file examples/internal-calls.hex --context-depth 256 --max-states 2
```

深度 256 会被配置拒绝吗？实际历史长度会立即变成 256 吗？最后一次实验缺少的边能否证明不可达？

<details><summary>提示与验收</summary>

原示例 k=0 合并返回地址 `{0x5,0xc}`，k=1 已能分开；k=2 在这个小例子中保留更长历史，但不进一步改善返回地址精度。

变体的 JUMPI 所在块从 `0x0e` 开始。更新历史后，k=1 的 `pc=0x15` 只有 `context=[14]`，两次输入合成 `{0x5,0xc}`。k=2 则有 `[0,14]` 与 `[5,14]`，入口分别为 `{0x5}`、`{0xc}`。

context 的数字是十进制来源块起始 pc；14 即 `0x0e`。验收需解释对应状态与返回边，而不只是“k=2 的状态更多”。已有具体对照见 [`concrete.rs`](../crates/evm-abstract/tests/concrete.rs)；给变体增加覆盖测试时也应让 revm 真正执行两次调用。

JSON 的默认深度为 128、状态为 `Converged`。深度 256 合法，分析不会预先建立长度 256 的历史；历史随实际跳转增长。`--max-states 2` 使结果为 `Incomplete`、退出码 2，留下 `States` 前沿。配置接受任何可表示为 `usize` 的非负深度，但实际执行仍受状态、transfer 和工作预算限制。

</details>

## 7. 分辨“值未知”和“分析没做完”

```bash
nix run . -- cfg --file examples/diamond.hex --domain constants-only --context-depth 0 --max-constants 1
nix run . -- cfg --file examples/loop.hex --context-depth 0 --max-transfers 1
nix run . -- cfg --file examples/loop.hex --context-depth 0
```

比较状态、诊断和 frontier。预算只有 1 时，能否把未执行状态的 `stack out []` 当作程序结果？

<details><summary>提示与验收</summary>

[diamond](../examples/diamond.hex) 的 Top 是精度扩大，分析为 `Converged`。限制 transfer 的 [loop](../examples/loop.hex) 为 `Incomplete`、退出码 2，留下 `Transfers` 前沿；默认预算则完成传播。未执行状态的临时出口不代表真实效果，缺少后续边也不能用于证明不可达。

额外运行 `ssa --file examples/loop.hex --context-depth 0 --max-transfers 1`，应拒绝构建 SSA。解释拒绝是因为图未完成，而不是循环本身不允许 SSA。

</details>

再比较局部交换精度和累计工作：

```bash
nix run . -- cfg --hex 5f355f0200 --max-facts 1
nix run . -- cfg --hex 5f355f0200 --reduction-rounds 1
nix run . -- analyze --world examples/worlds/call-return-branch.json --evm.to 0x0000000000000000000000000000000000000101 --evm.caller 0x0000000000000000000000000000000000001000 --evm.value 0 --evm.calldata 0x --max-work 1
```

<details><summary>局部上限与工作前沿的验收</summary>

前两段都是 `x*0`，仍能证明出口值为零，整体为 `Converged`。事实容量 1 留下 `FactExchangeLimited(FactLimit)`，文本以区间 `u[0x0,0x0]` 和 `bits=0x0000000000000000000000000000000000000000000000000000000000000000` 表示零；一轮上限留下 `FactExchangeLimited(RoundLimit)`，出口为 `{0x0}`。表示不同不意味着具体结果不同，也不能把局部报告当作数值矛盾。

世界实验为 `Incomplete`、退出码 2，留下 `Work` 前沿。CLI/Web 的根工作预算默认 1000000000000，域运算、事实交换、状态处理和所有调用帧共同使用；它衡量分析工作，不是 EVM gas。完整结果中要同时检查 `status`、diagnostics 和 frontiers，不能只看其中一个数字。

</details>

## 8. 组合域：位信息与相等关系各自改善什么

第一段读取未知 x，计算 `(x AND 254) OR 1`，然后以结果作为 JUMPI 条件。先手算它可能有多少个值、是否可能为零，再运行：

```bash
nix run . -- cfg --hex 5f3560fe16600117600c57005b600200 --context-depth 0 --max-constants 1
nix run . -- cfg --hex 5f3560fe16600117600c57005b600200 --domain constants-only --context-depth 0 --max-constants 1
```

第二组比较“复制同一个值”与“分别读出两个值”。先预测 XOR 的结果和分支，再运行：

```bash
nix run . -- cfg --file examples/copy-identity.hex --context-depth 0
nix run . -- cfg --file examples/independent-inputs.hex --context-depth 0
```

记录各图的 `status`、BranchTrue/BranchFalse 边和条件来自哪个定义。增大历史深度，能否代替位信息或复制身份？

<details><summary>提示与验收</summary>

第一段条件是 1 到 255 的奇数，共 128 个值，容量 1 无法列完。默认 product 仍证明最低位为 1，所以仅有 BranchTrue；constants-only 保留两边。两次都是 `Converged`。这说明不能列完常量不等于不能证明非零。

第二组第一段读取根 calldata word，固定输入符号支持 `x XOR x=0`；product 与 constants-only 都仅保留 BranchFalse。第二段读取不同偏移的两个 word，输入身份不同，不能证明相等，所以两边都保留。要比较临时复制身份，用[第 03 课的 `x+1; DUP1; XOR` 变体](03-cfg.md#5-数值精度怎样改变候选边)。

验收要分别说明数值性质、固定输入符号和临时复制关系。临时复制身份在基本块、汇合、调用和摘要边界失效；固定输入身份一致时可跨块保留，但不能据此推出任意数组别名或完整路径相关性。提高 `--context-depth` 只改变分组，不能生成这些缺失的规则。事实交换细节见[第 12 课](12-product-domains-facts.md)。

</details>

## 9. 进阶实验：快照值、强更新和未知别名

读过[第 09 课](09-cross-contract.md)后，在 `/tmp/storage-experiment.json` 保存以下离线世界。它声明 slot 0 初始为 4，代码依次读取、写入 7、再次读取：

```json
{
  "fork": "osaka",
  "provenance": "offline:exercise-storage:v1",
  "accounts": [{
    "address": "0x0000000000000000000000000000000000000101",
    "code": "0x600054600760005560005400",
    "storage": {"0x0": "0x4"},
    "storage_unknown": false,
    "balance": "0x0"
  }]
}
```

```bash
nix run . -- analyze --world /tmp/storage-experiment.json --evm.to 0x0000000000000000000000000000000000000101 --evm.caller 0x0000000000000000000000000000000000001000 --evm.value 0 --evm.calldata 0x
```

接着修改这份临时文件：把 `code` 改为 `0x60005460076001545560005400`，把 `storage_unknown` 改为 true，并更新来源说明。新程序从未知的 slot 1 读取一个值，用它作为 SSTORE 的目标 slot。预测第二次读取 slot 0 的结果。

<details><summary>提示与验收</summary>

原程序的块出栈是 `[{0x4},{0x7}]`；Return outcome 的 slot 0=7。新程序出栈是 `[{0x4},{0x4,0x7}]`：未知目标可能写中 slot 0，也可能没写中，不能继续只保留 4，也不能一律替换为 7。

同时会保留模型允许的 Failure outcome，回滚后 slot 0=4。把它与成功结果分开读，不能拿一个失败结果否定成功路径的写入。

省略 CLI 的 `--evm.calldata` 时，长度和内容均为符号输入。显式 `--evm.calldata 0x` 提供已知空字节，其他 hex 提供具体字节。前一个程序使用常量 slot 0，后一个从 storage slot 1 取写入目标；两者都不读取 calldata，因此只改变 calldata 不会改变这个实验。若将代码改为 `0x60005460075f355560005400`，写入目标才来自 `CALLDATALOAD(0)`：保留 `--evm.calldata 0x` 时目标为零，最终 slot 0=7；省略这项时目标未知，slot 0 保留 `{4,7}`。Rust API 中，`Entry::new(to)` 使用符号默认值，环境中的 calldata 也可设置为 `ByteArray::unknown()`。

进一步运行 [`proxy-storage.json`](../examples/worlds/proxy-storage.json) 和 [`revert-rollback.json`](../examples/worlds/revert-rollback.json)，分别核对 DELEGATECALL 的状态账户、子帧回滚后的 slot。验收应包括来源假设、`storage_unknown`、写入前后值与正确回滚，而不只是最终一个数。

</details>

## 10. 进阶设计：消除多余 φ

设计一个生成新 SSA 的优化，先处理无环、单前驱案例：只有一个有效来源的 φ 可以替换为来源值。画出替换前后的入口、uses（使用位置）与 exit_stack，再考虑 φ 自引用和循环。

<details><summary>提示与验收</summary>

需要 ValueId 替换映射并处理传递替换；只删除 φ 会留下未定义引用。当前 verifier 假设每个入口槽位都有 φ，因此采用别名入口参数时，还需同时设计表示与验证规则。

验收至少包含：所有引用和出口同步更新、每个使用仍有唯一且符合支配规则的定义、循环不无限递归、优化前后具体结果一致。遇到环状 φ 时不能草率递归替换；经典支配边界插入是另一种 SSA 构建路线，可作为后续比较。

</details>

## 11. 进阶设计：让 true 分支记住 x=5

从 `x → EQ(x,5) → JUMPI` 画一张数据流图。设计沿 true 边怎样把 x 收窄为 5：状态需要保存什么？怎样找到原值？如果条件只保存 `{0,1}`，还能恢复比较关系吗？

<details><summary>提示与验收</summary>

需要保留原值身份与谓词关系，例如基于 SSA 的假设或额外约束域。条件结果 `{0,1}` 只说明可能真假，不能告诉分析器哪个变量应收窄。当前组合域已有同块复制和局部运算事实，但不会把这个比较的含义作为路径假设传到后继。

验收既要显示新规则改善了哪个分支，也要证明已检查的具体轨迹仍被覆盖：不能把未知条件猜成某一边；分支汇合时还需正确合并不同假设。提高 context_depth 和分配 SSA 名字都不会自动完成这项能力。

</details>

## 12. 进阶设计：解释一条边为什么存在

设计一个查询：某个 SSTORE 是经过哪些抽象边到达的？对每条边注明常量目标、有限目标集合、Top 目标或零/非零条件，并保留 pc、context 和未完成前沿。

<details><summary>提示与验收</summary>

先用 [dynamic-jump](../examples/dynamic-jump.hex) 检查 Top 目标解释，再用 [diamond](../examples/diamond.hex) 检查条件边，用受限 [loop](../examples/loop.hex) 检查 Incomplete。查询不得遗漏 `UnknownJump` 或 frontier，也不能把抽象图可达写成“这笔交易一定成功”。

这些标签说明边的推导依据，不是整条路径的可满足性证明。若未来要报告真实可执行路径，还需给出固定输入下的具体执行证据。

</details>

## 13. 判断部分 SSA 究竟记录到了哪里

打开 [partial-ssa-memory.json](../examples/partial-ssa-memory.json)，先手写五条指令的正常栈变化，再按 [04 的实验命令](04-ssa.md#从块中途停下的实际输出开始)分别使用 1 和 32 字节内存上限。不要先看答案，预测四件事：MLOAD 是否有结果名字；ADD 是否出现于 SSA 正文；记录处的栈还剩什么；分析状态与退出码是什么。

接着运行 [partial phi 示例](04-ssa.md#部分-φ-的输入覆盖哪些边)，找到一条 `T` 边及接收它的入口 φ，回答：输入列表只有一项，能否证明以后不会再发现别的入边？整张图 Incomplete，是否意味着每个块都只执行到中间？

<details><summary>提示与验收</summary>

| 内存上限 | MLOAD 和后续指令的证据 | 栈与完成状态 |
| --- | --- | --- |
| 1 | MLOAD 为 OperandsConsumed，消费偏移名字 `%1`，没有结果；ADD、STOP 未分析到 | `recorded prefix stack: F0: [%0]`，其中 `%0` 是之前的常量 1；Incomplete、退出 2 |
| 32 | MLOAD 定义 `%2`，ADD 定义 `%3`，STOP 已记录 | 块末栈为 `[%3]`；Converged、退出 0 |

`recorded prefix stack` 是当前记录阶段的栈，不承诺最后一条指令已经正常完成。φ 的 `T: %value` 项只说明沿该条已支持边传入哪个名字；当前实现未为 Incomplete 图中的每个块分别证明入边完整。单输入 φ 也可能只是未简化的入口命名。已记录到块末尾的路径可以与其他路径的前沿同时存在。

最后分别从[理论路线](routes/theory.md#ssa)解释这里的覆盖限制，从[实现路线](routes/implementation.md#ssa)定位 `InstructionProgress`、`exit_frames` 与 φ 输入的构造。能把这些字段对应到实际输出，才算完成这一题。

</details>

继续当前[阅读路线](learning-routes.md)，或用[第 08 课](08-forks.md)检查协议规则。世界实验配合[第 09 课](09-cross-contract.md)、[第 10 课](10-snapshots-summaries-creation.md)；组合域实验配合[第 12 课](12-product-domains-facts.md)。
