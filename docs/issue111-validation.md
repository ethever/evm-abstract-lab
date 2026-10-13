# Issue #111：集成验收与资源记录

本次专项验收在源码 `ae41fcc791c15cd2b6629b79a682570b42c95f74` 上通过 **45 个测试、128 次分析**。逐例记录见 [JSON 证据](evidence/issue111-symbolic-state.json)，测试入口是 [`symbolic_state.rs`](../crates/evm-abstract/tests/symbolic_state.rs)。这是有明确输入和资源策略的实际执行证据；最终 PR 的干净提交、main 基线及完整 Nix 门禁结果另外记录在 PR 描述中。

## 输入与验证配置

测试使用离线构造的字节码世界。账户、代码、余额、初始 storage 和交易环境由 fixture 显式设置；未知 calldata、未知槽和符号地址保留为符号输入。没有把求解器的某个模型赋值当作链上观察，也没有依赖代理标准或固定实现地址槽。

专项分析使用 Product 数值域和原生 Z3，`rlimit=100000`、表达式/编码节点额度 8192、深度 128、约束额度 256、共享 work 额度 100000000。通常最多 512 个状态、4096 次 transfer，逻辑 memory 额度 4096 字节；循环测试另用 64 个状态和 64/128 次 transfer，范围测试另用 128 字节或 1 MiB memory 额度。逐次实际配置均写入 JSON；这些是验收配置，不能称为所有库或 CLI 默认配置。

Fork 包含 Cancun、Prague、Osaka。调用上下文、创建和部分字节语义测试在三个 fork 分别执行；其余符号关系测试主要使用 Osaka，具体循环范围以对应测试为准。[revm 对照](../crates/evm-abstract/tests/symbolic_state/oracle.rs)独立执行支持的具体 fixture，检查终结、返回数据、账户余额/nonce/storage/代码及调用帧轨迹。有限具体轨迹为符号输入提供见证，不能替代对整个符号范围的 SMT 断言。

专项 128 次分析使用 Z3。Bitwuzla、cvc5 的 Array/UF、结果分类、资源和取消能力还由 `embedded-smt` 与关系编码测试验证，并纳入完整 Nix 检查；这不等于把本表每个 EVM 场景都在三个 provider 下重复测量。

## 实际覆盖与结果

| 测试组 | 测试 / 分析次数 | 主要验证 |
| --- | ---: | --- |
| bytes | 6 / 26 | word 大端、部分覆盖、MCOPY 重叠、零大小、最大偏移不回绕 |
| calls | 7 / 25 | 四种调用的账户/caller/value/static、同代码不同 owner、嵌套委托、重入及回滚 |
| convergence | 4 / 6 | 稳定 MSTORE 循环、持续变化循环的前沿、摘要来源独立与历史值 |
| creation | 5 / 18 | 委托执行中的 CREATE/CREATE2、creator/nonce/余额、部署后代码可见性及分层回滚 |
| logs | 2 / 2 | 符号长度 LOG 保留旧 memory、后续覆盖不改历史、完整数值事实与有界 JSON |
| memory | 5 / 5 | 未知非对齐地址、历史读取、分支地址配对、复制视图与 hash/storage |
| storage | 6 / 7 | 同一未知槽、混合写序、相等/不等别名和符号值的后续消费 |
| transfers | 10 / 39 | `366000803700`、未知长度复制/委托/返回/REVERT、子输入窗口、独立来源与哈希 |

45 个测试均返回 0。128 次分析中，124 次 Converged、4 次按测试要求保留 Incomplete：最大偏移的非空访问、持续变化的 memory 循环、跨越 memory 策略的复制、没有长度上界的最小复制示例。测试分别核对准确前沿及仍可执行的路径；没有把所有输入改成具体值或统一退化为未知来通过验收。

例如，完整未知 calldata 转发保留内容与长度，子帧在真实指令中消费已有字节条件。哈希测试从实际第二次 KECCAK 的状态读取关系，再以原生查询证明同一 storage 槽读值唯一为 7；长度覆盖 0、32、33、64、512 等见证及范围外反例。LOG 的历史 word 保持 7，后续 memory word 为 9，两者分别由实际终结路径中的关系证明。

普通回归另外覆盖默认预算下的嵌套调用、回滚保存点和报告。读取可以保留数值集合及路径条件：例如失败/成功 CALL 的汇合产生字节 `{0,1}`，成功 RETURNDATACOPY/RETURN 的实际路径仍可证明 word 唯一为 1。报告保留抽象字节，测试同时要求路径 SAT 和唯一值证明，避免靠不可达路径获得真值。

## 测量方法与规模

测量时间为 UTC 2026-10-13 00:01:39 至 00:02:11，平台为 Linux x86_64。使用未优化的 Cargo test profile。先复制编译完成的专项测试可执行文件，再为每个测试启动独立进程，参数为 `--exact --nocapture --test-threads=1`。冻结二进制 SHA-256：

```text
02e4386a348378e0b037515cb0e1c68f8c8b76e62f310028203888c267359275
```

45 个进程的累计 wall time 为 **31.814 秒**；单进程最大峰值 RSS 为 **114668 KiB，约 112.0 MiB**。RSS 使用 Linux `wait4`，不含 Cargo/编译器；wall time 包含进程启动、fixture、SMT 查询和 revm 对照。主机还有其它开发任务，因此这些数字是此次运行记录，不是 release 吞吐承诺，也不是与旧实现的公平性能对照。

| 每次分析的指标 | 此次观测范围 | 含义 |
| --- | ---: | --- |
| states | 1–15 | 实际图状态数量 |
| transfers | 1–21 | 实际执行变换次数 |
| work | 15794–94818226 | 累计共享计费工作，包含预留的求解预算 |
| max_stack_expr_nodes | 0–45 | entry/exit 栈表达式的最大展开计数；不含全部隐藏状态根 |
| max_memory_work | 29–37303 | 活动帧 memory 表示的最大工作规模 |
| max_relation_work | 1–185 | entry/exit 关系表示的最大工作规模 |
| max_store_work | 97–675 | entry/exit store 表示的最大工作规模 |
| summary_hits | 0–1 | 每次分析实际摘要命中次数 |

节点展开计数、唯一 DAG 节点数、逻辑字节范围、计费 work 和实际 RAM 是不同指标。上述 work 包含求解预算预留，不能解释成 native solver 实际执行步数；provider 的 rlimit 也不是毫秒。本记录没有声称测得精确的 native query 总次数或仅求解器 wall time。单次分析 `elapsed_us` 及每个测试的 wall/RSS/CPU 时间均保存在 JSON 中。

## 复现

在已进入项目 Nix 开发环境、依赖已准备的干净源码上构建专项测试：

```bash
cargo test --locked --offline -p evm-abstract --test symbolic_state \
  --no-run --message-format=json > /tmp/issue111-build.jsonl
```

从输出中 `target.name == "symbolic_state"` 的 `compiler-artifact` 记录取得 `executable` 路径，再运行：

```bash
python3 scripts/measure-symbolic-state.py \
  /path/to/symbolic_state-test-binary "$PWD" /tmp/issue111-new-measurement
```

输出目录必须尚不存在。脚本拒绝脏源码，保存 HEAD 和冻结二进制摘要，逐例记录实际退出码；失败测试不会被计为通过。调用者须提供刚由该源码构建的二进制。提交的 JSON 省略本地主机文件路径，保留配置、原始数值、源码/二进制摘要及每例结果。

完整交付还必须按 [本地门禁](local-ci.md) 在最终干净提交执行 `nix flake check`，覆盖默认/imbl/全部 features、ignored 测试、原生 SMT、CLI、协议、Wasm 和真实浏览器。专项测量不能替代该门禁。

## 支持边界

逻辑 memory 策略不等于物理分配；超策略输入保留条件前沿。未知目标、未知代码、需要具体化的 initcode/runtime 或预编译输入继续保留准确边界。任意符号创建地址的完整账户生命周期和任意 lazy hash 视图的内容比较没有被本记录宣称为已支持。来源不同不能证明内容或哈希不同，UNKNOWN/超时/错误不能当作 UNSAT。

循环 cases 按有界 widening 策略扩大集合；持续变化的效果可能停止于明确前沿。摘要仍按完整输入匹配，数值同为 Top 不会抹掉新 storage 根或复制视图来强行命中。原生 schema 6、Web 协议 9、导出预算、路径与来源身份的语义见 [符号状态教程](17-symbolic-state.md)。

## 独立原生投影压力与真实 Multicall 前沿

独立的 `long-linear-chain-600` fixture 使用 `bytes.fromhex("5b600160020150") * 600 + b"\x00"`，共 4201 字节。在原生源码 `6a95223b12248b0e615c2fd7e49cfcfd7f969373` 上，实际 Chromium 验证了 600 个原生状态、600 个 Source/SSA 块和 599 条 Fallthrough 边，报告为 Converged、SSA complete、无 frontier。浏览器在 Blocks、States 和 Source SSA 预览间切换，检查全部 600 状态、无隐藏/边界、原生选择与 SSA 指令、缩放和没有新增分析请求。原 128 块用例仍保留。此压力由独立 fixture 提供，不依赖真实合约的共享上下文数量。

原 Multicall3 字节码、1604 字节 aggregate3 calldata、八个输入 item、具体 root 地址和 CHAINID=1、全部默认预算、历史 eth_call 的 1344 字节 ABI 返回见证均保持原样。当前抽象执行的完整机器上下文共享和资源计费会影响捕获图的规模。root 已复核的 6a952 原生基线是 267 状态、286 transfers、56 个唯一 Source 块，停止于三条 Relations / ExpressionLimit 前沿；250 原生状态最低线来自此实际独立测量，Source 最低线继续为 50。本次定向 Chromium 请求取得的真实 API 为 267 原生状态、274 边、286 transfers、56 个 Source 块和 267 个 Current SSA 块；图仍为 Incomplete、SSA partial。三条前沿来自状态 244、248、266，pc 与 range 为空，各引用完整 incoming payload 0、1、2。三份完整 pending payload 的数值/来源、typed DAG、byte view 和 store 引用校验已对同一原始结果实际通过。

这些前沿保留尚未并入图的完整 incoming machine、frames、stack 的完整 U256 数值与来源、path、memory/calldata/returndata 和 transaction/rollback store 引用；SSA 保持 incomplete，不把 pending incoming 当作已完成指令。States 投影的节点和 native edges 必须分别等于实际 CFG 和 edges，原 Source 投影、聚焦、选择、SSA 预览及无额外 POST 检查继续执行。

历史 main 报告的 704 状态、1194 transfers、66 个 Source 块、8 条 Memory 前沿和 25 个 root return 记录，并未覆盖 1344 字节目标返回 ABI。历史返回记录可能包括 widening 路径，不能据此把当前无 root return 认定为业务语义回归，也不能把图由 704 变成 267 宣称为通用性能提升。七条唯一 Call 边不等于七次动态调用；本次不声称原生分析完成八项成功返回。

独立 600 定向浏览器真实退出 0。原 Multicall 首次正常浏览器执行取得完整真实 API 结果后，由新增 consumer 的 View 类型读取错误退出 1；错误已修正并对同一份未改动报告离线通过完整 typed payload 校验。第二次正常浏览器重试曾启动，随后被终止，实际退出 1；外部 page.route transport replay 首次也在进入 Multicall UI 前退出 1。root 核对终止原因后，明确授权一次仅 UI 的外部 HTTP transport replay；该次真实 Chromium 和前后来源复核均退出 0，完整首次 API 结果的原始字节及 SHA-256 未变，零分析 POST 转发到原生 server。Source 56 节点、States 267 节点 / 274 native edges、原生选择与实际 SSA 指令预览、无额外分析请求和真实 WebGPU 均通过。以上失败记录仍保留；transport replay 不代表正常的 fresh-native/browser E2E 已通过。后续 diagnostics、完整浏览器及最终 37 项 Nix 门禁仍待 root 在最终冻结提交运行，局部验证不替代最终门禁。
