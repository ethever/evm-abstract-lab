# 07：按步骤把知识变成实验

前七个练习只需运行命令、手算和比较结果；后四个涉及世界输入或实现设计。先完成 [00 → 06 的基础阅读](00-start.md)，世界练习等读过[第 09 课](09-cross-contract.md)后再做。

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

预测 diamond 在 `pc=0x0e` 的入口值，以及加 10 后的值：

```bash
nix run . -- cfg --file examples/diamond.hex --max-constants 1
nix run . -- cfg --file examples/diamond.hex --max-constants 2
```

记录两次的 `status`、汇合点 `stack in`、`stack out`。增加容量改变了程序还是分析表示？

<details><summary>提示与验收</summary>

容量 1：`{1} ⊔ {2}=Top`，加 10 后仍为 Top。容量 2：入口 `{0x1,0x2}`，出口 `{0xb,0xc}`，即 `{11,12}`。两次都为 `Converged`，字节码和合法行为没有改变。

集合容量增大能保留更多常量，但逐槽集合仍不保存槽位间的配对关系。若两条路径的两个槽位分别是 `[1,10]` 和 `[2,20]`，逐槽合并会允许 `[1,20]` 这种额外组合；容量足够也无法自动消除它。

</details>

## 4. 用名字追踪 DUP、SWAP 和 φ

先手写栈中的值名字，再运行：

```bash
nix run . -- ssa --hex 60018060029000
nix run . -- ssa --file examples/diamond.hex
```

回答：最终有三个栈槽位，为什么只有两个值定义？diamond 的 `%0` 为什么不是第一条 PUSH？从 S1 进入汇合点时，φ 接收哪个名字？

<details><summary>提示与验收</summary>

第一个程序 `values=2`，出栈 `[%0,%1,%0]`。DUP 复制引用，SWAP 改变位置，都不产生新值定义。

diamond 为入口槽位先分配 `%0`，所以第一条 PUSH 是 `%1`。汇合点是 `phi(S1: %4, S2: %5)`；从 S1 来取 `%4`，从 S2 来取 `%5`。`slot 0` 表示入口栈底位置，`%0` 表示定义身份，`abstract {0x1,0x2}` 表示可能数值；三者应分别解释。

</details>

## 5. 未知跳转后，副作用不能消失

运行 dynamic-jump，定位 `pc=0x09` 的 SSTORE：

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

<details><summary>提示与验收</summary>

原示例 k=0 合并返回地址 `{0x5,0xc}`，k=1 已能分开；k=2 在这个小例子中保留更长历史，但不进一步改善返回地址精度。

变体的 JUMPI 所在块从 `0x0e` 开始。更新历史后，k=1 的 `pc=0x15` 只有 `context=[14]`，两次输入合成 `{0x5,0xc}`。k=2 则有 `[0,14]` 与 `[5,14]`，入口分别为 `{0x5}`、`{0xc}`。

context 的数字是十进制来源块起始 pc；14 即 `0x0e`。验收需解释对应状态与返回边，而不只是“k=2 的状态更多”。已有具体对照见 [`concrete.rs`](../crates/evm-abstract/tests/concrete.rs)；给变体增加覆盖测试时也应让 revm 真正执行两次调用。

</details>

## 7. 分辨“值未知”和“分析没做完”

```bash
nix run . -- cfg --file examples/diamond.hex --max-constants 1
nix run . -- cfg --file examples/loop.hex --max-transfers 1
nix run . -- cfg --file examples/loop.hex
```

比较状态、诊断和 frontier。预算只有 1 时，能否把未执行状态的 `stack out []` 当作程序结果？

<details><summary>提示与验收</summary>

diamond 的 Top 是精度扩大，分析为 `Converged`。限制 transfer 的 loop 为 `Incomplete`、退出码 2，留下 `Transfers` 前沿；默认预算则完成传播。未执行状态的临时出口不代表真实效果，缺少后续边也不能用于证明不可达。

额外运行 `ssa --file examples/loop.hex --max-transfers 1`，应拒绝构建 SSA。解释拒绝是因为图未完成，而不是循环本身不允许 SSA。

</details>

## 8. 进阶实验：快照值、强更新和未知别名

读过第 09 课后，在 `/tmp/storage-experiment.json` 保存以下离线世界。它声明 slot 0 初始为 4，代码依次读取、写入 7、再次读取：

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
nix run . -- analyze --world /tmp/storage-experiment.json --entry 0x0000000000000000000000000000000000000101
```

接着修改这份临时文件：把 `code` 改为 `0x60005460076001545560005400`，把 `storage_unknown` 改为 true，并更新来源说明。新程序从未知的 slot 1 读取一个值，用它作为 SSTORE 的目标 slot。预测第二次读取 slot 0 的结果。

<details><summary>提示与验收</summary>

原程序的块出栈是 `[{0x4},{0x7}]`；Return outcome 的 slot 0=7。新程序出栈是 `[{0x4},{0x4,0x7}]`：未知目标可能写中 slot 0，也可能没写中，不能继续只保留 4，也不能一律替换为 7。

同时会保留模型允许的 Failure outcome，回滚后 slot 0=4。把它与成功结果分开读，不能拿一个失败结果否定成功路径的写入。

CLI 的 `--calldata` 提供具体字节，默认为空，不能用它表达未知 calldata。若要让未知 calldata 决定写入目标，可在 Rust API 的 Entry 中使用 `ByteArray::unknown()`，再增加相应对照。

进一步运行 `proxy-storage.json` 和 `revert-rollback.json`，分别核对 DELEGATECALL 的状态账户、子帧回滚后的 slot。验收应包括来源假设、`storage_unknown`、写入前后值与正确回滚，而不只是最终一个数。

</details>

## 9. 进阶设计：消除多余 φ

设计一个生成新 SSA 的优化，先处理无环、单前驱案例：只有一个有效来源的 φ 可以替换为来源值。画出替换前后的入口、uses（使用位置）与 exit_stack，再考虑 φ 自引用和循环。

<details><summary>提示与验收</summary>

需要 ValueId 替换映射并处理传递替换；只删除 φ 会留下未定义引用。当前 verifier 假设每个入口槽位都有 φ，因此采用别名入口参数时，还需同时设计表示与验证规则。

验收至少包含：所有引用和出口同步更新、每个使用仍有唯一且符合支配规则的定义、循环不无限递归、优化前后具体结果一致。遇到环状 φ 时不能草率递归替换；经典支配边界插入是另一种 SSA 构建路线，可作为后续比较。

</details>

## 10. 进阶设计：让 true 分支记住 x=5

从 `x → EQ(x,5) → JUMPI` 画一张数据流图。设计沿 true 边怎样把 x 收窄为 5：状态需要保存什么？怎样找到原值？如果条件只保存 `{0,1}`，还能恢复比较关系吗？

<details><summary>提示与验收</summary>

需要保留原值身份与谓词关系，例如基于 SSA 的假设或额外约束域。条件结果 `{0,1}` 只说明可能真假，不能告诉分析器哪个变量应收窄。

验收既要显示新规则改善了哪个分支，也要证明已检查的具体轨迹仍被覆盖：不能把未知条件猜成某一边；分支汇合时还需正确合并不同假设。提高 context_depth 和分配 SSA 名字都不会自动完成这项能力。

</details>

## 11. 进阶设计：解释一条边为什么存在

设计一个查询：某个 SSTORE 是经过哪些抽象边到达的？对每条边注明常量目标、有限目标集合、Top 目标或零/非零条件，并保留 pc、context 和未完成前沿。

<details><summary>提示与验收</summary>

先用 dynamic-jump 检查 Top 目标解释，再用 diamond 检查条件边，用受限 loop 检查 Incomplete。查询不得遗漏 `UnknownJump` 或 frontier，也不能把抽象图可达写成“这笔交易一定成功”。

这些标签说明边的推导依据，不是整条路径的可满足性证明。若未来要报告真实可执行路径，还需给出固定输入下的具体执行证据。

</details>

基础实验做完后，读[第 08 课](08-forks.md)，检查同一字节码在不同协议规则下的区别。世界实验和扩展设计继续配合第 09、10 课阅读。
