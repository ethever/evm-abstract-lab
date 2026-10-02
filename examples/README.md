# 七个可以手算的 runtime bytecode

`.hex` 文件只含十六进制字节与空白，可以直接作为 CLI 的 `--file` 输入。

| 文件 | 要观察的现象 | 关键位置 |
| --- | --- | --- |
| `straight-line.hex` | 2+3、MSTORE、RETURN；单块 SSA | ADD pc=0x04 |
| `diamond.hex` | 两条 calldata 分支汇合；φ 与集合 join | 汇合 pc=0x0e，值为 {1,2} |
| `loop.hex` | 回边、有限域收敛、循环 φ | 循环头 pc=0x02 |
| `dynamic-jump.hex` | calldata 目标为 Top 也要保留后续 SSTORE | JUMP pc=0x03，JUMPDEST pc=0x04，SSTORE pc=0x09 |
| `internal-calls.hex` | 同一 helper 的两次内部调用 | helper pc=0x0e；比较 k=0/1 |
| `stack-heights.hex` | 相同块不同入栈高，不能强行合并 | 汇合 pc=0x0c，height=0/1 |
| `osaka-clz.hex` | 新 opcode 的版本启用与常量跳转恢复 | CLZ(1)=255；计算目标 pc=0x08 |

```bash
nix run . -- explain --file examples/diamond.hex
nix run . -- explain --file examples/internal-calls.hex --context-depth 1
```

具体对照测试使用 32 字节 calldata。diamond/stack-heights 分别测试末字节为 0 与 1；dynamic-jump 测试末字节为 4，确保具体执行经过合法目标并写 storage；其余例子不依赖 calldata。原有六个例子在 Cancun/Prague/Osaka、k=0/1/2 下核对具体入口、边、出栈及 SSA 值；CLZ 例子在 Osaka 下核对，并在两个旧 fork 下验证拒绝启用。见 [`concrete.rs`](../crates/evm-abstract/tests/concrete.rs) 与 [`osaka.rs`](../crates/evm-abstract/tests/osaka.rs)。
