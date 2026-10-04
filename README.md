# evm-abstract-lab

一个用 **Rust + Nix** 学习和实现跨合约 EVM 抽象分析的实验室。主要输入是包含多个账户代码、初始 storage 和余额的离线世界，以及一次明确的入口调用。抽象执行直接维护调用栈、每帧内存、账户状态和返回数据，产出跨合约执行图与 SSA。代码注释和教程以中文为主，按“观察结果 → 跟踪实现 → 理解理论 → 动手改变精度”的顺序展开。

这里的“执行”是在可能值的摘要上计算。例如，两条路径带来的 `1` 和 `2` 合成 `{1,2}`；无法继续精确表示时升到 `⊤`，表示任意 256 bit 值。循环用工作表计算固定点，动态跳转目标未知时覆盖所有真正的 `JUMPDEST`。

```mermaid
flowchart LR
    A[多账户世界 + 入口调用] --> B[按 fork 解码账户代码]
    B --> C[调用帧栈 + 内存 + 账户 Store]
    C --> D[CALL / RETURN / REVERT 的全局工作表]
    D --> E[跨合约执行图 + SSA + 状态效果]
    E --> F[返回分支 / 代理 / 回滚 / 重入证据]
```

## 开始运行

需要启用 flakes 的 Nix。所有命令在仓库根目录执行。

```bash
nix develop
cargo run --locked -p evm-abstract-cli -- analyze --world examples/worlds/call-return-branch.json --entry 0x0000000000000000000000000000000000000101
cargo run --locked -p evm-abstract-cli -- analyze --world examples/worlds/proxy-storage.json --entry 0x0000000000000000000000000000000000000101 --format json --ssa
```

也可以直接使用 Nix 打包的二进制：

```bash
nix run . -- analyze --world examples/worlds/reentry.json --entry 0x0000000000000000000000000000000000000101
nix run . -- analyze --world examples/worlds/proxy-storage.json --entry 0x0000000000000000000000000000000000000101 --format dot > /tmp/proxies.dot
nix develop -c dot -Tsvg /tmp/proxies.dot -o /tmp/proxies.svg
```

`analyze` 支持 text/JSON/DOT；`--ssa` 构建并验证完整执行图的 SSA。`--caller`、`--calldata`、`--value` 和 `--static` 指定入口环境；调用深度、累计工作量、状态数、transfer 和每帧内存都有显式预算。缺少代码、未知调用目标、未支持的创建/销毁/预编译或预算耗尽产生 `Incomplete`、保留原因和前沿、退出 `2`。输入错误退出 `1`。分析从不隐式访问 RPC。

世界 JSON 的 `fork`、`provenance` 和 `accounts` 是必填项。账户地址是 20 字节 hex；代码是 runtime hex；storage slot、storage value 和余额是 `0x` 开头的 256 bit 数。缺少账户或代码表示未知，`"code":"0x"` 才表示观察到空代码。未列出的余额和 slot 默认未知；只有显式 `"storage_unknown":false` 才把未列出的 slot 视为零。[跨合约一课](docs/09-cross-contract.md) 展示完整输入和状态流。

单段字节码仍可以用 `disasm`、`cfg`、`ssa` 和 `explain` 的 `--hex` / `--file` 学习；它们是主要世界分析入口的单账户视图。文本栈按 **底到顶** 显示，单账户 SSA 同样拒绝未完成的图。

默认使用 **Osaka（Fusaka 的执行层）**。世界文件固定 `fork: "cancun" | "prague" | "osaka"`；单段字节码命令接受 `--fork`。选择结果记录在文本、JSON 和 DOT 中，同一世界不能混用 fork。协议规则与模型支持范围需要分别阅读，见[协议版本一课](docs/08-forks.md)。

```bash
nix run . -- explain --file examples/osaka-clz.hex
nix run . -- explain --file examples/osaka-clz.hex --fork cancun
```

## 从哪一课开始

| 阅读顺序 | 你要回答的问题 | 实验与代码 |
| --- | --- | --- |
| [00：环境与第一眼](docs/00-start.md) | 世界、调用帧和返回边是什么？ | `call-return-branch.json`、CLI |
| [09：跨合约执行](docs/09-cross-contract.md) | 代理共享哪些东西？失败和重入怎样传递状态？ | `world.rs`、`analysis/machine.rs`、离线世界 |
| [01：字节码与基本块](docs/01-bytecode.md) | 为什么 PUSH 内部的 `5b` 不是跳转目标？ | `bytecode.rs` |
| [02：抽象执行与域](docs/02-domain.md) | 一个集合怎么替代许多具体执行？为什么合并用并集？ | `domain.rs`、`diamond.hex` |
| [03：CFG 与固定点](docs/03-cfg.md) | 跳转边未知时怎么继续？循环为什么会停？ | `analysis/`、`loop.hex`、`dynamic-jump.hex` |
| [04：栈 SSA](docs/04-ssa.md) | φ 选择什么？DUP/SWAP 怎么保留值身份？ | `ssa/`、分支与循环 |
| [05：敏感性与精度](docs/05-sensitivity.md) | 流、路径、上下文、栈高敏感分别保留什么？ | `--context-depth`、`internal-calls.hex` |
| [06：模型边界与证据](docs/06-boundaries.md) | 收敛能证明什么？链上状态能增加哪些信息？ | revm oracle、诊断和预算 |
| [07：练习与提示](docs/07-exercises.md) | 如何亲手扩展这个分析器？ | 由易到难的练习和验收方法 |
| [08：协议版本与升级](docs/08-forks.md) | 为什么同一字节码在不同 fork 下会产生不同 CFG？ | `fork.rs`、`osaka-clz.hex`、EIP-7702 |

每课都有真实字节偏移、栈变化或图，以及对应源码位置。无需先掌握 Datalog、SMT 或编译器理论。

## 已实现的学习材料与能力

- 多账户固定世界、显式入口环境、区分缺少账户与已知空代码；一个工作表分析整段嵌套调用。
- CALL / CALLCODE / DELEGATECALL / STATICCALL 的执行帧、代码地址与状态账户分离、调用参数与返回数据传播、成功/REVERT/失败返回边。
- 每帧抽象内存、calldata、returndata；按账户保存 persistent/transient storage 与余额，写入更新、嵌套回滚与重入观察当前状态。
- 跨合约图和 SSA；调用边、返回边及状态效果进入 IR，完整结果经过验证器核对。
- Cancun/Prague/Osaka legacy 解码、按 fork 检查指令启用、PUSH0/PUSH1..32、截断 PUSH 右侧补零、真实 JUMPDEST 索引和基本块划分。
- 256 bit 有限常量集合域、保守 Top、纯算术/位运算、逐槽 join、循环工作表。
- 常量与计算得到的跳转目标、条件分支剪枝、未知跳转的保守展开。
- 相同块按栈高区分；可选 `k=0..3` 的有限跳转来源历史，观察内部调用的合并与分离。
- CFG 上的栈 SSA、循环 φ、DUP/SWAP 别名、保留 SSTORE 等副作用指令，以及结构验证器。
- Osaka `CLZ` 的常量集合传播与 SSA；识别 EIP-7702 委托标记并报告目标，避免生成虚假的终止 CFG。
- 单账户字节码与多账户离线世界、三个 fork 的 revm 具体执行对照、性质测试、真实 CLI 测试和 Nix/CI 检查。

模型针对世界内账户的 **legacy runtime bytecode**。gas 不精确计量，未知环境和 hash 保守抽象，CREATE/CREATE2、SELFDESTRUCT、预编译和无法取得的代码保留未完成前沿；没有完整路径约束或跨交易不变量证明。`Converged` 表示本抽象模型的工作表完成，不能据此判断合约安全。[详细边界](docs/06-boundaries.md) 列出信息处理方式和证据范围。

## 工具链与依赖

创建时核验并固定 Rust **1.99.0**（2026-10-01 稳定发布）。[`rust-toolchain.toml`](rust-toolchain.toml) 是 Nix 和 rustup 共用的版本来源，锁定 cargo、rustfmt、clippy、rust-src 和 rust-analyzer。它不会跟随浮动的 `stable` 自动变化。

[`flake.lock`](flake.lock) 固定 nixpkgs、rust-overlay、crane 的提交和内容哈希；Nix 2.34.8、Graphviz、cargo-nextest、nixfmt、lychee 随 nixpkgs 固定，CI 也明确使用 Nix 2.34.8。直接依赖用精确版本，[`Cargo.lock`](Cargo.lock) 固定全部传递依赖和校验和。升级必须显式修改并重新验证。环境、测试与打包始终使用同一个 Rust 工具链。

| crate | 负责什么 | 为什么复用 |
| --- | --- | --- |
| `revm-bytecode` | opcode 名称、立即数和栈 I/O 元数据 | 避免重复维护 opcode 表 |
| `alloy-primitives` / `ruint` | U256、模运算、快速幂 | 避免自己写大整数 |
| `petgraph` | 图、DOT 导出、dominators | 避免重复写图格式和支配算法 |
| `serde` / `serde_json` | 可检查的结构化输出 | 不手写 JSON |
| `clap` / `thiserror` | 参数与有类型的错误 | 语义代码保持聚焦 |
| `revm` / `proptest`（测试） | 具体执行 oracle、代数性质与随机案例 | 给自研抽象语义独立的核对依据 |

抽象 transfer、工作表的状态划分和栈到 SSA 的转换是本仓库的学习主题，保留为有注释的 Rust 实现。通用基础设施使用现有 crate。

## 验证与源码导航

```bash
nix flake check --print-build-logs --option max-jobs 1 --option cores 8
```

这个门禁运行构建、工作区测试与 doctest、Clippy、Rust 格式、Rustdoc、离线文档链接检查和打包后二进制的例子/图检查。在开发环境中也可单独运行：

```bash
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo fmt --all -- --check
cargo doc --workspace --no-deps --locked
```

文档链接使用 [lychee](https://lychee.cli.rs/)；版本由同一份 `flake.lock` 固定，规则见 [`lychee.toml`](lychee.toml)。从仓库根目录运行：

```bash
nix develop --command lychee --offline --root-dir "$PWD" -- README.md '**/*.md'
```

离线门禁检查所有 Markdown 的本地文件、引用式链接和锚点，包括隐藏目录中的文档；跳过 `target`、`result*`、`.git`、`.direnv` 中的生成文件。它不访问网络，也不验证 HTTP(S) 外链。需要联网检查外链时显式运行：

```bash
nix develop --command lychee --offline=false --scheme http --scheme https --include-fragments=none -- README.md '**/*.md'
```

这条命令只检查 HTTP(S) 状态，不检查远端锚点；外站可用性和限流会影响结果，因此它不属于可复现的 Nix 门禁。失败链接会以非零状态退出，不把超时或 HTTP 错误当作通过。

库位于 [`crates/evm-abstract/src`](crates/evm-abstract/src)，CLI 位于 [`crates/evm-abstract-cli`](crates/evm-abstract-cli)。源码模块顶部解释职责与不变量，算法关键处解释“为什么”。[参考资料](docs/references.md) 指向 EVM 规范、抽象解释原始论文、SSA 教学材料和依赖文档。

MIT license。
