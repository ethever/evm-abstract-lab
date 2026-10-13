# Issue #124：未知世界交互的验收与性能

本次正式 release CLI 对照在干净源码 `dd069018c282e9e85e790e3d2f819aadf92f14dc` 上完成，使用加强的返回值、storage 和 closed 检查器。执行器 binary 由 `b25bf2d35477018dd7e156daab8fb96cfe4226fd` 构建，之后只修改测试、Nix 示例检查和验收脚本；两版之间没有生产 Rust 源码变更。ordinary 对照与 SMT probe 使用同一 binary，SHA256 为 `2bd2e8210af788c0b91942f240c39b7c00c4ee904119de508ec679378a99a837`。main 基线为 `a14843430ed975fc71593d52c73fae316a21c385`。

本记录在测量结束后加入。最终干净提交、完整 Nix 命令和真实退出码记录在 PR；以下开发期验收不替代该门禁。教程与可运行 bytecode 见[第 18 课](18-unknown-world.md)。

## 语义验收

[`unknown_world`](../crates/evm-abstract/tests/unknown_world.rs) 的 **42 个测试全部通过**。它们覆盖真实已知入口、零次／多次／间接／嵌套回调、外部 C 的 old/new 与禁止写入反例、分层成功失败组合、静态身份、调用上下文、短 returndata 输出区、预算和报告。基础、嵌套、C、回滚、静态身份和输出区使用库默认策略：共享 work 20,000,000、Z3 rlimit 100,000、表达式节点 1024、约束 128、深度 64。C 与回滚的成功用例还明确要求 Converged、无 frontier、非空且全部 closed 的未知摘要；短返回数据同时检查实际 Return/Revert 路径与未复制后缀。回调成功、未知过程成功和交易成功分别核对，每次调用使用自己的回滚检查点。

完整符号地址空间的四个目标条件测试使用统一显式策略：work 1,000,000,000、约束 2048、节点 65,536、深度 512，其余为库默认。单次 target、排除 C、重复同一 target 的关系闭合；两个独立符号可别名的例子保留全部候选，并以 revm 的 C→C 具体执行核对同一 abstract outcome 的完整效果。该例仍有原生输入缺口 `PrecompileInput`，目标为 1–17 和 256；缺口位于 U 的调用依赖内时，对应 U 关系保持 open。这不属于完整世界闭合。另有相同完整地址空间在默认预算下耗尽的回归，不能用增加功能验收额度代替前沿测试。

创建 owner 后委托 SELFDESTRUCT 的具体见证被覆盖，但相关测试仍保留 `Creation(UnknownEndowment)`，不声称整个生命周期已经闭合。有限 revm 样本用于发现遗漏；一般覆盖依据是入口和全部允许交互的保守变换、整个摘要的单调扩大、依赖更新传播以及未完成部分的显式前沿，详见教程。

六份 [unknown-world 示例](../examples/README.md)均用当前 CLI 和 Nix 安装示例检查器通过：busy-x、busy-x-nested、external-change、external-guard、outer-revert、static-reuse。每例要求 Converged、无 frontier、非空且全部 closed 摘要、完整 verified SSA 和对应数值结果。outer-revert 要求确实存在 64 字节回退数据：old=5、new 覆盖 `{5,9}`，最终 C.slot 0=5；删去全部 64 字节 Revert、仅留下 Failure／空 Revert 和正确最终 storage 的负向样本会被拒绝。

其余开发期检查实际通过：核心库 **581 tests**、CLI **148 tests**、#111 符号状态 **45 tests** 和关系 **23 tests**、工作区 Clippy及离线文档链接。真实 Chromium 使用匹配的新 Wasm 和 server，显式提交 root/caller/value/calldata，验证 closed 摘要、无虚构未知指令、SummaryReturn、完整 SSA、返回 42，并实际选中节点查看 Frame 的抽象交互摘要。测试结果行、示例检查器源码及浏览器构建哈希保存在[机器可读记录](evidence/issue124-unknown-world.json)；这些结果不等于最终 Nix 安装包或完整浏览器门禁已通过。

## 相同覆盖与精度的 release CLI 对照

正式数据来自 `benchmark-final-closed/measurements.json`，源码为上述 `dd069018`；较早的测量批次不用于本表。[JSON 证据](evidence/issue124-unknown-world.json)保留全部 24 个未舍入样本、配置、fixture／binary 哈希、wait4 CPU／峰值 RSS、计费 work、状态／传播／复用、导出体积和序列化 DAG 规模。每例交替开启／关闭复用各运行三次，表中均为中位数。每次必须 Converged、frontiers 为空、未知关系非空且全部 closed，指定返回值和 storage 的数值集合相同；不声称证明所有输入输出关系完全等价。

两侧共同使用 work **3,000,000,000**；其余 CLI 默认值包括 Z3 rlimit 10,000,000、max_constants 512、节点 16,384、约束 2048、深度 256。CLI 默认 work 本为 1,000,000,000,000，这里使用相同明确充分额度。库功能测试的 20M 策略与本表 CLI 策略不同，不能比较其计费 work 来推断实际计算倍数。

| case | 复用 | wall 秒 | user+system CPU 秒 | 峰值 RSS KiB | work | transfers | JSON MB |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| static-reuse | 开 | 0.0523 | 0.0515 | 62,016 | 45,548,063 | 14 | 2.476 |
| static-reuse | 关 | 0.0663 | 0.0662 | 70,072 | 154,716,074 | 38 | 2.476 |
| busy-x | 开 | 0.3704 | 0.3702 | 81,756 | 964,878,643 | 40 | 2.755 |
| busy-x | 关 | 0.3731 | 0.3727 | 81,624 | 964,878,643 | 40 | 2.755 |
| busy-x-nested | 开 | 0.3931 | 0.3927 | 81,892 | 1,049,690,588 | 56 | 3.199 |
| busy-x-nested | 关 | 0.3872 | 0.3868 | 81,788 | 1,049,690,588 | 56 | 3.199 |
| external-change | 开 | 5.0466 | 5.0448 | 896,244 | 1,318,342,766 | 75 | 16.808 |
| external-change | 关 | 5.1451 | 5.1436 | 896,264 | 1,318,152,695 | 75 | 16.096 |

| case | 摘要请求 开/关 | 关系状态 开/关 | 传播 开/关 | reused 开/关 | 序列化节点 开/关 |
| --- | ---: | ---: | ---: | ---: | ---: |
| static-reuse | 4/4 | 32/32 | 14/56 | 7/16 | 1,484/1,484 |
| busy-x | 1/1 | 11/11 | 10/10 | 2/2 | 3,486/3,486 |
| busy-x-nested | 1/1 | 15/15 | 36/36 | 8/8 | 3,614/3,614 |
| external-change | 1/1 | 19/19 | 38/38 | 15/15 | 37,662/36,456 |

四次静态调用节省重复计算，返回值始终为 42，最终 A.slot 0=42。busy 与嵌套例子只有一次外层未知入口，两侧仍须求递归闭合，work 和 transfers 相同；x 覆盖 `{0,1,2}`、返回保存的 busy=1、最终 busy=0。C 变化例子的 old 保持 5，new 与最终 C.slot 0 覆盖 `{5,9}`，C.slot 1=1。本批小幅耗时差异未证明 busy／嵌套／C 有稳定复用收益；C 仍需约 5 秒、0.85 GiB 峰值 RSS 和约 16 MB 报告。`reused` 大于零不能自动解释为请求变快；这组结果不代表 USDC/WETH 已完成或其端到端延迟已修复。

进程耗时包括启动、分析和完整 native JSON 导出；CPU 为 Linux wait4 的 user+system，RSS 不包括 Cargo 编译器。序列化节点数包含不同观察重复编码的根，不是整个运行唯一 DAG 节点或 SMT 公式数。work 包含求解预算预留，不是求解器真实步数或毫秒。

## 原生 SMT 成本

[独立 profile 证据](evidence/native-z3-cost-b25bf2d-review.json)保留 24 次交错 probe 样本、2 次补充 perf 的调用栈摘要，以及 C 探针、runner 和冻结检查器的完整源码与哈希，复查不依赖原 `/tmp` 文件仍存在。测量源码为 `b25bf2d`，与正式表、ordinary 对照的 binary SHA256 一致。全部测量进程实际 exit 0，native UNKNOWN=0、clock failures=0。

探针通过 LD_PRELOAD 拦截动态链接的 Z3 4.16.0 `Z3_solver_check`／`Z3_solver_check_assumptions`，保留原返回值与 errno，避免嵌套调用重复计时。表中是每组 3 次测量的中位数：

| case | checks 开/关 | native elapsed ms 开/关 | native calling CPU ms 开/关 |
| --- | ---: | ---: | ---: |
| static-reuse | 2/8 | 0.327/1.007 | 0.324/1.003 |
| busy-x | 70/70 | 135.061/133.189 | 135.047/133.141 |
| busy-x-nested | 76/76 | 142.437/143.890 | 142.423/143.806 |
| external-change | 87/87 | 4768.502/4769.688 | 4767.117/4768.677 |

external-change 在这批插桩运行中，native calling CPU 占**同批** process CPU 的中位数为 92.78%/92.53%；process CPU 中位数为 5.138/5.154 秒，逐样本扣 native calling CPU 后的残差中位数为 0.381/0.385 秒。残差包括 Rust 分析、编码、context/solver 构建、model 与 JSON，不能直接称为编码耗时。调用线程计时不包含 native worker 线程；覆盖范围也仅是这两个 check API。

补充 perf 每侧一次、99 Hz 用户态 CPU 采样，得到 299/311 个样本、无 lost samples。libz3 self 样本占比 92.31%/91.96%，native `Z3_solver_check` inclusive 占比 90.64%/90.03%；主要 self 热点为 AST hash 扩容、AST 比较、compact_memory、act_cache 与 register_node_core。unsigned division/remainder 的 bit-blasting helper inclusive 占比为 31.44%/30.87%。这些重叠的调用栈采样占比不能相加或转换为精确时长；它们支持主要成本位于 native check 的 AST／位向量处理这一结论。

C 例没有实际普通摘要命中：开启时 hits=0、misses=3、published=3，关闭时均为 0；两侧都是 75 transfers、87 checks。开启额外保存 3 份普通摘要，JSON 增加 711,357 bytes。这个观察解释了复用没有节省这次请求的大部分计算，不能把文件大小差额当成耗时。插桩与 perf 属于独立批次；其 wall 时间不替代正式无 probe 表，也不与其他批次混用分母。

## 普通已知调用回归

另与上述 main 的 Nix release 包对照三个既有 world，每侧交替运行七次，共 **42 次**，原始数据来自 `ordinary-benchmark/measurements.json`。使用相同 CLI 默认配置，包括 work 1,000,000,000,000。逐次比较终止、返回数据和 Store 的完整序列化 outcome 图：身份作用域重命名后相等；新增显式 false 的 created/pending-destruction 默认字段与旧的隐式 false 对齐，没有删除数值或表达式内容来获得相等。完整图的规范化哈希及原 runner 源码保存在 JSON。

| case | 版本 | wall 秒 | CPU 秒 | 峰值 RSS KiB | work |
| --- | --- | ---: | --- | ---: | ---: |
| call-return-branch | main | 0.0661 | 未记录 | 62,764 | 82,116,738 |
| call-return-branch | 本实现 | 0.0667 | 未记录 | 62,860 | 87,082,368 |
| proxy-storage | main | 0.0613 | 未记录 | 56,640 | 29,460,596 |
| proxy-storage | 本实现 | 0.0626 | 未记录 | 56,628 | 29,497,861 |
| summary-reuse | main | 0.0282 | 未记录 | 56,888 | 8,830,935 |
| summary-reuse | 本实现 | 0.0278 | 未记录 | 56,888 | 8,838,399 |

这批 recorder 未保存 user/system CPU，因而不推算或补造 CPU 值。三个小例子没有显示显著的普通调用延迟变化，结论限于这三个 world。`call-return-branch` 的 work 从 82,116,738 增至 87,082,368，新增 metadata 精化有计费开销，不能只报告耗时而忽略这一变化。

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

SMT profile JSON 的 `sources` 内包含 `content` 与 SHA256，可取出 C 探针和 `measure-z3-checks.py`。用 Nix 开发环境中的 cc 按嵌入说明编译共享库，传入本地 `--root`、`--binary`、`--probe` 和新的 `--output`；两侧仍使用相同 3B work。插桩结果单列，不能冒充正式 wall 对照。

完整交付按[本地检查](local-ci.md)在最终干净提交运行全部 Nix 检查，并在 PR 核对远端 head 与真实通过的 SHA 一致。
