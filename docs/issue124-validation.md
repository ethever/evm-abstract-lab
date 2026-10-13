# Issue #124：未知世界交互的验收与性能

本次正式 release CLI 对照在干净源码 `a657cc334c4ef9532ac19f66bf855d796e3a48f4` 上完成，包含静态未知过程的完整效果上界简化，并使用返回值、storage 和 closed 检查器。ordinary 对照与 SMT probe 使用同一执行器 binary，SHA256 为 `ef66756a39da61684877feb8bce845ae30a78e0dedc0de963052367d4df5f239`。main 基线为 `a14843430ed975fc71593d52c73fae316a21c385`。较早的 `b25bf2d` 测量单独保留为历史，不与本轮样本混用。

本记录在测量结束后加入。最终干净提交、完整 Nix 命令和真实退出码记录在 PR；以下开发期验收不替代该门禁。教程与可运行 bytecode 见[第 18 课](18-unknown-world.md)。

## 语义验收

开发期 [`unknown_world`](../crates/evm-abstract/tests/unknown_world.rs) 的 **43 个测试全部通过**。它们覆盖真实已知入口、零次／多次／间接／嵌套回调、外部 C 的 old/new 与禁止写入反例、分层成功失败组合、静态身份、调用上下文、短 returndata 输出区、预算和报告。新增 revm 见证实际执行 U 对 A、C 的静态回调，读取 42 与 5、处理失败的 SSTORE，并核对完整 outcome；抽象结果闭合且保留世界状态，不为省略的只读回调虚构已执行块。基础、嵌套、C、回滚、静态身份和输出区使用库默认策略：共享 work 20,000,000、Z3 rlimit 100,000、表达式节点 1024、约束 128、深度 64。C 与回滚的成功用例还明确要求 Converged、无 frontier、非空且全部 closed 的未知摘要；短返回数据同时检查实际 Return/Revert 路径与未复制后缀。回调成功、未知过程成功和交易成功分别核对，每次调用使用自己的回滚检查点。

完整符号地址空间的四个目标条件测试使用统一显式策略：work 1,000,000,000、约束 2048、节点 65,536、深度 512，其余为库默认。单次 target、排除 C、重复同一 target 的关系闭合；两个独立符号可别名的例子保留全部候选，并以 revm 的 C→C 具体执行核对同一 abstract outcome 的完整效果。该例仍有原生输入缺口 `PrecompileInput`，目标为 1–17 和 256；缺口位于 U 的调用依赖内时，对应 U 关系保持 open。这不属于完整世界闭合。另有相同完整地址空间在默认预算下耗尽的回归，不能用增加功能验收额度代替前沿测试。

创建 owner 后委托 SELFDESTRUCT 的具体见证被覆盖，但相关测试仍保留 `Creation(UnknownEndowment)`，不声称整个生命周期已经闭合。有限 revm 样本用于发现遗漏；一般覆盖依据是入口和全部允许交互的保守变换、整个摘要的单调扩大、依赖更新传播以及未完成部分的显式前沿，详见教程。

六份 [unknown-world 示例](../examples/README.md)包括 busy-x、busy-x-nested、external-change、external-guard、outer-revert、static-reuse。每例要求 Converged、无 frontier、非空且全部 closed 摘要、完整 verified SSA 和对应数值结果。outer-revert 要求确实存在 64 字节回退数据：old=5、new 覆盖 `{5,9}`，最终 C.slot 0=5；删去全部 64 字节 Revert、仅留下 Failure／空 Revert 和正确最终 storage 的负向样本会被拒绝。六例及该负向检查此前通过的记录归入静态上界简化前的历史；本轮正式 24 次对照覆盖下表四种示例，最终安装包检查仍由完整门禁验收。

本轮开发期核心定向单元检查 **6 tests** 及上述 **43 个集成测试** 实际通过；运行时 checkout 为带相同修改的 `285f83a`，随后形成 `a657cc3`，不是最终干净提交门禁。静态上界简化前的历史检查包括核心库 **581 tests**、CLI **148 tests**、#111 符号状态 **45 tests** 和关系 **23 tests**、工作区 Clippy及离线文档链接。历史 Chromium 使用当时匹配的 Wasm 和 server，显式提交 root/caller/value/calldata，验证 closed 摘要、无虚构未知指令、SummaryReturn、完整 SSA、返回 42，并实际选中节点查看 Frame 的抽象交互摘要。测试结果行、示例检查器源码及构建哈希保存在[机器可读记录](evidence/issue124-unknown-world.json)的当前与 `historical_before_static_envelope` 字段；不据此宣称本轮整套核心库、最终安装包或完整浏览器门禁已通过。

本轮 [Router Web 取证](evidence/issue124-router-static-envelope.json)保持原 21,943 字节 bytecode、196 字节 calldata、symbolic ADDRESS／caller／CALLVALUE 和全部浏览器默认策略。HTTP 请求在 10.764 秒完成，native 状态仍为 Incomplete：261 个状态、5 个 Memory 与 2 个 Relations(ExpressionLimit) 前沿、partial SSA；3 份静态未知摘要全部 closed。前沿来自已知 Router 的 returndata 复制／回退与表达式限制，不是未知摘要未闭合。这份直接 HTTP 取证不替代浏览器截图验收，也不代表真实代币端到端已经修复。旧 CLI 在 1250.365 秒被人为 SIGTERM 中止，峰值 RSS 3.026 GiB、无最终报告；新 CLI 在 12.545 秒因 native JSON 导出超过 64 MiB 而退出 1。两者均没有完整 CLI 报告，不计算完成耗时比；Web 使用不同的协议投影表示。

## 相同覆盖与精度的 release CLI 对照

正式数据来自 `benchmark-static-envelope-closed/measurements.json`，源码为上述干净的 `a657cc3`；较早的测量批次不用于本表。[JSON 证据](evidence/issue124-unknown-world.json)保留全部 24 个未舍入样本、配置、fixture／binary 哈希、wait4 CPU／峰值 RSS、计费 work、状态／传播／复用、导出体积和序列化 DAG 规模。每例交替开启／关闭复用各运行三次，表中均为中位数。每次必须 Converged、frontiers 为空、未知关系非空且全部 closed，指定返回值和 storage 的数值集合相同；不声称证明所有输入输出关系完全等价。

两侧共同使用 work **3,000,000,000**；其余 CLI 默认值包括 Z3 rlimit 10,000,000、max_constants 512、节点 16,384、约束 2048、深度 256。CLI 默认 work 本为 1,000,000,000,000，这里使用相同明确充分额度。库功能测试的 20M 策略与本表 CLI 策略不同，不能比较其计费 work 来推断实际计算倍数。

| case | 复用 | wall 秒 | user+system CPU 秒 | 峰值 RSS KiB | work | transfers | JSON MB |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| static-reuse | 开 | 0.0201 | 0.0196 | 30,112 | 9,131,732 | 7 | 2.113 |
| static-reuse | 关 | 0.0204 | 0.0201 | 30,112 | 9,137,438 | 10 | 2.113 |
| busy-x | 开 | 0.5377 | 0.5372 | 81,912 | 964,878,643 | 40 | 2.755 |
| busy-x | 关 | 0.4301 | 0.4296 | 81,832 | 964,878,643 | 40 | 2.755 |
| busy-x-nested | 开 | 0.4930 | 0.4924 | 81,964 | 1,049,690,588 | 56 | 3.199 |
| busy-x-nested | 关 | 0.4906 | 0.4900 | 81,900 | 1,049,690,588 | 56 | 3.199 |
| external-change | 开 | 7.4685 | 7.4573 | 902,068 | 1,318,342,766 | 75 | 16.808 |
| external-change | 关 | 7.6484 | 7.6472 | 896,492 | 1,318,152,695 | 75 | 16.096 |

| case | 摘要请求 开/关 | 关系状态 开/关 | 传播 开/关 | reused 开/关 | 序列化节点 开/关 |
| --- | ---: | ---: | ---: | ---: | ---: |
| static-reuse | 4/4 | 4/4 | 0/0 | 3/0 | 1,280/1,280 |
| busy-x | 1/1 | 11/11 | 10/10 | 2/2 | 3,486/3,486 |
| busy-x-nested | 1/1 | 15/15 | 36/36 | 8/8 | 3,614/3,614 |
| external-change | 1/1 | 19/19 | 38/38 | 15/15 | 37,662/36,456 |

四次静态调用两侧都直接使用完整状态恒等与三出口上界，返回值始终为 42，最终 A.slot 0=42。两侧约 20 ms、30,112 KiB，未显示额外复用收益；无需重新执行只读回调来闭合。busy 与嵌套例子只有一次外层未知入口，两侧仍须求递归闭合，work 和 transfers 相同；x 覆盖 `{0,1,2}`、返回保存的 busy=1、最终 busy=0。C 变化例子的 old 保持 5，new 与最终 C.slot 0 覆盖 `{5,9}`，C.slot 1=1。本批三次样本的耗时差异未证明 busy／嵌套／C 有稳定复用收益；C 仍需约 7.47／7.65 秒、0.86 GiB 峰值 RSS 和约 16 MB 报告。`reused` 大于零不能自动解释为请求变快；这组结果不代表 USDC/WETH 已完成或其端到端延迟已修复。

进程耗时包括启动、分析和完整 native JSON 导出；CPU 为 Linux wait4 的 user+system，RSS 不包括 Cargo 编译器。序列化节点数包含不同观察重复编码的根，不是整个运行唯一 DAG 节点或 SMT 公式数。work 包含求解预算预留，不是求解器真实步数或毫秒。

## 原生 SMT 成本

[独立 profile 证据](evidence/native-z3-cost-static-envelope-a657cc3-review.json)保留本轮 24 次交错 probe 样本、同批 process CPU，以及 C 探针、runner、独立 smoke 和冻结检查器的完整源码与哈希，复查不依赖原 `/tmp` 文件仍存在。测量源码为干净的 `a657cc3`，与正式表、ordinary 对照的 binary SHA256 一致。全部测量进程实际 exit 0，native UNKNOWN=0、clock failures=0。静态例子两侧六个样本均为零次 native check；同一动态 Z3 库的独立 smoke 确认两次真实 API 查询分别 SAT／UNSAT，避免把未工作探针误记为零成本。

探针通过 LD_PRELOAD 拦截动态链接的 Z3 4.16.0 `Z3_solver_check`／`Z3_solver_check_assumptions`，保留原返回值与 errno，避免嵌套调用重复计时。表中是每组 3 次测量的中位数：

| case | checks 开/关 | native elapsed ms 开/关 | native calling CPU ms 开/关 |
| --- | ---: | ---: | ---: |
| static-reuse | 0/0 | 0.000/0.000 | 0.000/0.000 |
| busy-x | 70/70 | 136.457/133.395 | 136.441/133.313 |
| busy-x-nested | 76/76 | 140.347/136.646 | 140.055/136.629 |
| external-change | 87/87 | 6083.843/6139.585 | 6083.400/6138.827 |

external-change 在这批插桩运行中，native calling CPU 占**同批** process CPU 的逐样本比例中位数为 94.08%/94.25%；process CPU 中位数为 6.461/6.493 秒，逐样本扣 native calling CPU 后的残差中位数为 0.377/0.373 秒。残差包括 Rust 分析、编码、context/solver 构建、model 与 JSON，不能直接称为编码耗时。调用线程计时不包含 native worker 线程；覆盖范围也仅是这两个 check API。上述占比不使用正式 wall 表或较早 profile 的 CPU 分母。

历史 [`b25bf2d` profile](evidence/native-z3-cost-b25bf2d-review.json)的补充 perf 每侧一次、99 Hz 用户态 CPU 采样，得到 299/311 个样本、无 lost samples。libz3 self 样本占比 92.31%/91.96%，native `Z3_solver_check` inclusive 占比 90.64%/90.03%；主要 self 热点为 AST hash 扩容、AST 比较、compact_memory、act_cache 与 register_node_core。unsigned division/remainder 的 bit-blasting helper inclusive 占比为 31.44%/30.87%。这些重叠的调用栈采样占比不能相加或转换为精确时长，也不是本轮 `a657cc3` 的 perf 测量。

C 例没有实际普通摘要命中：开启时 hits=0、misses=3、published=3，关闭时均为 0；两侧都是 75 transfers、87 checks。开启额外保存 3 份普通摘要，JSON 增加 711,357 bytes。这个观察解释了复用没有节省这次请求的大部分计算，不能把文件大小差额当成耗时。本轮插桩、正式无 probe 对照和历史 perf 分别记录，其 wall 时间与分母不混用。

## 普通已知调用回归

另与上述 main 的 Nix release 包对照三个既有 world，每侧交替运行七次，共 **42 次**，原始数据来自干净 `a657cc3` 的 `ordinary-static-envelope/measurements.json`。使用相同 CLI 默认配置，包括 work 1,000,000,000,000。逐次比较终止、返回数据和 Store 的完整序列化 outcome 图：身份作用域重命名后相等；新增显式 false 的 created/pending-destruction 默认字段与旧的隐式 false 对齐，没有删除数值或表达式内容来获得相等。完整图的规范化哈希及原 runner 源码保存在 JSON。

| case | 版本 | wall 秒 | CPU 秒 | 峰值 RSS KiB | work |
| --- | --- | ---: | ---: | ---: | ---: |
| call-return-branch | main | 0.0559 | 0.0557 | 62,176 | 82,116,738 |
| call-return-branch | 本实现 | 0.0561 | 0.0558 | 62,728 | 87,082,368 |
| proxy-storage | main | 0.0610 | 0.0608 | 56,276 | 29,460,596 |
| proxy-storage | 本实现 | 0.0573 | 0.0570 | 55,732 | 29,497,861 |
| summary-reuse | main | 0.0277 | 0.0276 | 56,500 | 8,830,935 |
| summary-reuse | 本实现 | 0.0291 | 0.0289 | 56,500 | 8,838,399 |

这批 recorder 保存 wait4 user/system CPU，表中 CPU 是逐样本相加后的中位数。三个小例子的 wall 中位数有升有降，不能推出普遍的普通调用延迟收益，结论限于这三个 world。`call-return-branch` 的 work 从 82,116,738 增至 87,082,368，新增 metadata 精化有计费开销，不能只报告耗时而忽略这一变化。

## 复现

在干净的待测提交中构建 release，并运行相同额度的正式对照：

```bash
nix develop --no-update-lock-file -c env CARGO_BUILD_JOBS=8 \
  cargo build --release -p evm-abstract-cli -j8
python3 scripts/benchmark-unknown-world.py \
  --binary target/release/evm-abstract \
  --output /tmp/issue124-new-measurement --runs 3 --max-work 3000000000
```

输出目录必须尚不存在。脚本检查测量前后源码和 binary 未变化；调用者须提供刚由该提交构建的 binary。失败和不完整输出不会计作成功耗时样本。普通对照的 binary 路径、完整命令模板与原 runner 在验收 JSON 的 `ordinary_reproduction`；复现时把原脚本的 root、output、main/changed binary 路径改为本地新目录与待比较程序。

当前 SMT profile JSON 的 `sources` 内包含 `content` 与 SHA256，可取出 C 探针、独立 smoke 和 `measure-z3-checks-static-envelope.py`。用 Nix 开发环境中的 cc 按嵌入说明编译共享库与 smoke，传入本地 `--root`、`--binary`、`--probe`、`--smoke-binary` 和新的 `--output`；两侧仍使用相同 3B work。插桩结果单列，不能冒充正式 wall 对照。

完整交付按[本地检查](local-ci.md)在最终干净提交运行全部 Nix 检查，并在 PR 核对远端 head 与真实通过的 SHA 一致。
