# 可手算的离线世界与 runtime bytecode

主要入口使用 `worlds/` 的多账户 JSON。所有例子入口为 `0x...0101`，默认 caller=`0x...1000`，calldata 为空、value=0。`storage_unknown:false` 声明完整合成初始 storage；没有 RPC 依赖。

| 世界 | 要观察的现象 | 具体成功轨迹中的结果 |
| --- | --- | --- |
| `worlds/call-return-branch.json` | B 的 RETURN 字节决定 A 的分支 | A slot 0=1 |
| `worlds/proxy-storage.json` | 两代理 DELEGATECALL 同一实现，storage 分离 | P1/P2 slot 0=6/10；实现仍为 99 |
| `worlds/callcode-context.json` | CALLCODE 的 state address、caller、value | caller 为代理，value=3 |
| `worlds/returndata-copy.json` | CALL 输出区为空，随后 RETURNDATACOPY | 返回 32 字节数值 1 |
| `worlds/revert-rollback.json` | 回滚子帧写入，同时传回 REVERT 数据 | B slot 0 保留 4，A slot 1=42 |
| `worlds/static-write.json` | 静态子帧在 SSTORE 处故障 | CALL 成功位 0，B slot 0 保留 4 |
| `worlds/reentry.json` | 重入读取当前交易写入 | A slot 0=2，slot 1=1 |
| `worlds/log-rollback.json` | 子帧日志随 REVERT 回滚 | 只保留 A 的事件，B 的事件消失 |
| `worlds/summary-reuse.json` | 相同只读 callee，输出复制范围不同 | `hits > 0`；开启/关闭后的最终关系一致 |
| `worlds/create-runtime.json` | nonce=0 的 CREATE 后 CALL 新 runtime | InitCode/Runtime 两个代码 hash；成功路径返回 42，新账户 nonce=1 |
| `worlds/created-selfdestruct.json` | CREATE2 salt=5 后销毁并再次调用 | caller slot 1=8、slot 2=runtime hash；完整销毁路径最终目标 absent、受益人余额=7 |
| `worlds/identity-precompile.json` | CALL identity，把返回写到另一内存区 | 成功路径输出 42，图中有原生帧及 Call/Return |
| `worlds/missing-code.json` | 缺少被调用账户事实 | `Incomplete`、`MissingCode`、退出 2 |

```bash
nix run . -- analyze --world examples/worlds/proxy-storage.json --entry 0x0000000000000000000000000000000000000101 --format json --ssa
nix run . -- analyze --world examples/worlds/reentry.json --entry 0x0000000000000000000000000000000000000101
```

抽象图还保留不精确 gas 模型允许的失败分支，因此其最终值集合可能大于表里的具体成功结果。[`cross_concrete.rs`](../crates/evm-abstract/tests/cross_concrete.rs) 在三个 fork 对照完整具体轨迹与账户效果；[跨合约一课](../docs/09-cross-contract.md) 解释事实、帧和回滚。[第 10 课](../docs/10-snapshots-summaries-creation.md) 给出摘要开启/关闭、initcode/runtime、延迟删除与原生帧的可检查输出；其新增 fixture 显式声明 nonce、presence 和已确认的 absent 目标。所有账户都是合成离线假设。

以下 `.hex` 示例用于单账户局部指令学习。

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
