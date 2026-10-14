# 已知代码递归：验收与性能复现

本验收保留 [Issue #123](https://github.com/ethever/evm-abstract-lab/issues/123) 的原始未知请求和默认策略。目前已测到接入 MRU 上下文的开发候选 6：原 USDC 请求的一次连续 600 秒尝试仍未交付报告；WETH 在约 42.70 秒通过独立 HTTP API 交付报告，但分析状态仍为 `Incomplete`。原 USDC 性能问题尚未解决。小型返回地址精度回归与下述 CPU 诊断用于定位机制，不能代替最终干净提交的交替对照或完整门禁。

## 冻结输入

[复现清单](../examples/known-recursion/captured/manifest.json)保存命令、raw request/world 哈希、代码身份和观测范围。原 Ethereum 区块为 **26180958**，hash 为 `0x5219691ff60b20005fa597d2bb5e4b11e3ddb68f24fb528a900c8dafbb84b499`。USDC 入口是 `0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48`，额外对照 WETH 是 `0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2`。

两份原 request 的 caller、call value、calldata **内容与长度**全部为 `Unknown`。Web 的 calldata 留空保留这一含义；原性能请求没有改成精确空数据 `0x` 或具体 witness。USDC raw request SHA256 为 `df1a7eae163963b18e83c7413eb77d6da65381d19b3d2232f02d2f0ad90c6c4e`，WETH 为 `48f304b0790176ae9549ae8c183dbc09a9a1b47a36889caefb3b4a1c78cc28ae`。

| 入口 | 离线初始观测 | 未观测事实 | 原 RPC 请求 |
| --- | --- | --- | --- |
| USDC | proxy 2186 bytes、implementation 23464 bytes，2 账户、2 storage 槽 | `storage_unknown=true`，其他未知账户不补造成空代码或零余额 | 原 raw bytes、固定 block hash，允许正常 RPC 获取后续事实 |
| WETH | code 3124 bytes，1 账户、4 storage 槽 | 同上 | 同上 |

离线 world 来自旧报告的初始 `Store::new(native.world())` 观测，不使用 outcome 写入后的终态。它预加载已捕获事实，并显式保留区块环境；world 使用 CLI 所需的小写 `osaka`，原 protocol request 保持 `Osaka`，执行 fork 相同。离线模式排除 RPC 水合轮次、网络和新增事实获取，因此必须与原同块 RPC/API 模式分别比较。

[100 字节 admin witness](../examples/known-recursion/captured/usdc-100-byte-witness.json)是原 Unknown 请求允许的一个具体子集：管理员更新 implementation 为代理自身，随后 self CALL 和 self DELEGATECALL。该具体输入仅作递归语义见证，不代替原 Unknown 请求。

## 策略与实验保护

两侧采用相同输入、同一 fork 和原默认策略：work `1,000,000,000,000`、call depth `1025`、context depth `128`、states `100,000`、transfers `10,000,000`、memory `64 MiB`、constants `512`、constraints `2048`、expression nodes/depth `16384/256`、Z3 rlimit `10,000,000`；完整 RPC 限制保留在原 request。没有通过缩小预算、固定 calldata、排除 target 或减少代码覆盖取得性能结果。

外部守护每秒采样 owned process 的 RSS 和主机 `MemAvailable`，达到本轮外部时间上限、RSS 8 GiB，或主机可用内存低于 20 GiB 时，仅终止本 runner 创建的进程组，保留实际退出码；必要时 5 秒后升级为 SIGKILL。affinity 为 CPU48–55，最多使用 8 个 CPU并至少留下 5 个可用 CPU。首轮与候选 3 的时间上限为 120 秒；候选 4、5、6 原 USDC 各一次连续运行至 600 秒，120 秒仅保存进度而不停止或重试，WETH 仍为 120 秒。这些保护属于实验条件，不能记录成引擎 frontier、solver timeout 或产品默认限制。

## 首轮诊断与来源

[机器证据](evidence/known-recursion/preliminary.json)保留 raw artifact 哈希、命令、进度、统计与来源。各 case 本轮仅一个样本，运行期间存在实现修改、构建或短测试；USDC 候选还接受过一次 0.465 秒只读线程栈捕获。这批数据不作为最终隔离延迟分布，也不计算取消基线与新表示之间的加速比。

| 产物 | 实际来源与用途 | SHA256 |
| --- | --- | --- |
| 首轮旧 CLI | a657cc3 release；生产源码与 eb86cb5 的 git 对象相同，preliminary 使用 | `ef66756a39da61684877feb8bce845ae30a78e0dedc0de963052367d4df5f239` |
| 首轮旧 server | 285f83a dirty production build start；先前 a657cc3 clean 测量的同 ELF，preliminary 使用 | `a31db0598f0c8b6b2d7c0b1d5669cde8637032909939375c1ed0b7ada87ba104` |
| 首轮候选 CLI | eb86cb5 HEAD 上的开发工作树，`dirty_build=true`，真实 release build exit 0 | `0fbb53472d759db27d975eacf3578b95224dac722ed00af6383f88bef846e979` |
| 首轮候选 server | 同上；完整 build metadata 与 production hashes 保留 | `6faa17349ef062a598318645c9d03ee1619e43ff6b2811c0fb0626adbdd0e6b8` |
| 正式可用基线 CLI | 固定 clean eb86cb5 的 Nix default release package，已命中缓存，未用于下表 preliminary 样本 | `d452e5e8ce0489ec962d6fc7e4db7c995aca5de974bf59cbd0706a985b8ecfb8` |
| 正式可用基线 server | 同一 clean Nix package | `94193fb04bdca91142fae3c6d0dcce3d821b0cb33c5531ef87b0b41dd5a1f01a` |

固定 eb86cb5 表达式的实际输出为 `/nix/store/45gq0cd80b9xp604ij8b782jighv29hh-evm-abstract-0.4.0`，deriver 为 `/nix/store/cjn6h9lls3c55avwqmjfihhx4xm4ir7p-evm-abstract-0.4.0.drv`。这是固定源码表达式已有的不可变产物，不宣称旧 cargo ELF 是 eb86cb5 重编译；Nix 的源码过滤可让生产输入相同的提交共享输出。

| case/模式 | 版本 | 真实结束 | wall/阶段时间（秒） | user+system CPU（秒） | peak RSS KiB | 交付/分析边界 |
| --- | --- | --- | ---: | ---: | ---: | --- |
| USDC/offline CLI | 首轮旧 CLI | external timeout，exit -15 | process 121.096，guard 120.095 | 120.196 | 1,702,140 | stdout 0 bytes，无终态 JSON |
| WETH/offline CLI | 首轮旧 CLI | native JSON export error，exit 1 | process 28.023 | 27.646 | 243,668 | 触及 native 64 MiB 导出限制，无终态 JSON |
| USDC/RPC API | 首轮旧 server | external timeout，exit -15 | process 121.087，guard 120.085 | 120.251 | 1,710,376 | 最后 Running，709 进度 states / 570 transfers / 18.308B work |
| USDC/RPC API | 首轮候选 server | external timeout，exit -15 | process 120.233，guard 120.180 | 120.251 | 1,855,960 | 最后 Running，719 进度 states / 1581 transfers / 47.523B work |
| WETH/RPC API | 首轮旧 server | HTTP200 / Completed | processing 45.635，analysis 45.485 | 45.879 | 256,552 | Incomplete，29 frontier，partial SSA |
| WETH/RPC API | 首轮候选 server | HTTP200 / Completed | submit→body 43.193，processing 42.769，analysis 42.646 | 43.042 | 249,924 | Incomplete，29 frontier，partial SSA |

WETH 的旧/候选 API 分别返回 11,803,061 / 11,490,775 bytes，报告有 136 / 133 个 native states、484 / 484 transfers、53.271B / 53.723B work、1 / 2 个 summary requests。两侧各有 11 个 `StateJoin` 和 18 个 `Relations` frontier；原始终态保留具体原因。RPC 水合过程中进度 states 可累计不同分析轮次，不能与最终报告的 states 数混作同一指标。

CLI 原错误为 `native JSON export requires at least 67108866 bytes, limit 67108864 (retained 67108864)`；其 exit 1 不是成功交付时间，也不能据此猜测终态 status/frontier/summary。API 的 `Completed` 表示报告已交付，仍需读取报告中的 `Incomplete`。报告取得之后 server 的 `capture_complete` SIGTERM 是 owned process 清理，与任务失败分开记录。

USDC 的一次线程栈快照落在 Z3 `bit_blaster::mk_udiv_urem` / `mk_urem` 与 AST 表扩容，取消观察线程在等待。它说明该时刻正在 native 位向量余数求解，并不是完整统计 CPU profile。本轮未插桩，SMT check 次数和 native 累计成本不可得，不能填成 0；正式探针测量须使用独立两查询 smoke 验证，并与无 probe 的 wall 分开。

原 loopback RPC8545 已通过固定 hash 的五次只读 probe：chain ID、header、proxy code、admin 槽、implementation 槽与原 capture 一致。API 诊断只启动私有随机端口、一个 worker、一个队列位的 owned server，未访问或重启用户的 1111 服务。

## 后续开发候选的实际结果

[候选 3/4 机器证据](evidence/known-recursion/candidate-fourth-rpc.json)逐项保留实际 build 来源、两个版本的完整 production source hashes 与六个变化文件的 hash、原样 sample records 和 raw measurement 摘要哈希。两者都来自 eb86cb5 HEAD 上的 dirty 开发构建，不能合并称为最终版本；候选 4 server SHA256 为 `6c4f90af156f2e4dca9eff537f8b66f04eea71ffe8c17a5c89a5e38f70e4383c`。

候选 3 加入独立过程 widening、numeric fact widening 和递归返回第一 block 的 CALL status partition；候选 4 扩展到所有 CALL 返回第一 block，并用短生命 Reader 复用相同 immutable facts/domain 下的 word 字节并集。两轮都使用原始 RPC request、同块和同 policy；运行时存在开发测试负载。候选 4 没有 native probe、gdb、perf 或同时运行的微基准，未访问用户 1111 服务。

[候选 5 机器证据](evidence/known-recursion/candidate-fifth-rpc.json)保存完整 dirty build metadata、production source hashes、两个原样 sample records、源码/输入/ELF 前后校验及连续运行的 120 秒检查点。实际构建 HEAD 仍为 `eb86cb5661b77b5be2960ac4b1909c1b36cf6871`，包含开发修改；构建期间 production hashes 未变。CLI SHA256 为 `623701c6529ec0b2d16a3418ee33da9adfa0d65540d0bb06ddc36138db57c9f8`，server 为 `29ec1ecb72ab1020d2dc0555b10334a6f94b12b907716d969311146a44fd458a`。候选 5 仅在检测到过程内反馈边或递归依赖时启用相应 widening；运行期间存在短开发测试，未开启 probe、gdb、perf 或微基准。

[候选 6 机器证据](evidence/known-recursion/candidate-sixth-rpc.json)保留两份原样 sample、120 秒检查点、完整 dirty build metadata 与 197 个 production file hashes。它在候选 5 上接入有界 MRU 跳转来源上下文；实际 HEAD 仍为 eb86cb5 上的开发工作树。CLI SHA256 为 `6481d544bd01a7320a80120ba71699ba37ba97748f207c8a4687077fdd59754e`，server 为 `7cacc837501418d57a3dc88f0895835d2e25786b8088cc34a6a2bc113c36ddce`。这两次原请求测量未开启 probe、gdb、perf 或微基准。

| case | 版本 | 实际结束与阶段 | CPU（秒）/peak RSS KiB | 报告或最后进度 |
| --- | --- | --- | --- | --- |
| USDC | 候选 3 | external timeout，process 120.289 秒，exit -15 | 120.580 / 845,676 | 无报告；最后 1,264 累计进度 states / 3,874 transfers / 114.128B work |
| USDC | 候选 4 | external timeout，guard 600.932、process 601.110 秒，exit -15 | 602.650 / 1,441,916 | 无报告、尚未进入 SSA/导出；最后 3,765 累计进度 states / 14,593 transfers / 444.575B work |
| USDC | 候选 5 | external timeout，guard 600.942、process 601.066 秒，exit -15 | 602.600 / 1,052,056 | 无报告、尚未进入 SSA/导出；最后 2,255 累计进度 states / 8,878 transfers / 333.395B work |
| USDC | 候选 6 | external timeout，guard 600.930、process 601.018 秒，exit -15 | 604.716 / 787,036 | 无报告、尚未进入 SSA/导出；最后 round4 / 2 accounts / 2 slots / 29 RPC requests，3,830 累计进度 states / 4,640 transfers / 859.908B work |
| WETH | 候选 3 | HTTP200 / Completed；submit→body 43.217 秒 | 43.190 / 250,044 | Incomplete，133 native states / 484 transfers；29 frontier（11 Shape、18 Relations ExpressionLimit），partial SSA |
| WETH | 候选 4 | HTTP200 / Completed；submit→body 42.691、analysis 41.980、processing 42.107 秒 | 42.380 / 268,644 | Incomplete，134 native states / 488 transfers / 53.456B work；26 frontier（8 Shape、18 Relations ExpressionLimit），partial SSA；11,475,721 response bytes |
| WETH | 候选 5 | HTTP200 / Completed；submit→body 43.699、analysis 43.002、processing 43.134 秒 | 43.400 / 256,072 | Incomplete，134 native states / 488 transfers / 53.456B work；26 frontier（8 Shape、18 Relations ExpressionLimit），partial SSA；11,475,722 response bytes |
| WETH | 候选 6 | HTTP200 / Completed；submit→body 42.700、analysis 42.410、processing 42.580 秒 | 42.813 / 262,304 | Incomplete，134 native states / 486 transfers / 53.454B work；26 frontier（8 Shape、18 Relations ExpressionLimit），partial SSA；11,475,790 response bytes |

USDC 的不同外部时间上限与未完成报告不能构成延迟加速比较；WETH 的单样本与不同开发负载也不建立性能分布或覆盖改进。两个版本各自的终态、frontier 与输入/ELF 哈希前后校验均保留。候选 4 的两个 owned server 进程均已回收，两个 runner 实际 exit 0；USDC 的外部取消与 WETH 交付后的 `capture_complete` 清理分开记录。

候选 5 的两次 runner 及顺序 wrapper 也均实际 exit 0，两个 owned PID 均已退出。输入与 frozen ELF 前后校验通过；并发开发测试改变了工作树，所以两批 `source_unchanged=false`、`valid_frozen_batch=false`，只作开发诊断。表中 RSS 来自 `wait4`；USDC 守护采样另观测到 `VmHWM=1,054,956 KiB`，两种读数原样保留。其 119.814 秒检查点仍为 Running，984 进度 states / 2956 transfers / 95,813,197,291 work；600 秒计数并不代表最终 native 报告，frontier、summary 和 SMT 次数仍不可得。

候选 6 的顺序 wrapper 与两次 runner 均实际 exit 0，两个 owned PID 已退出。原输入和 ELF 未变，结束后的 197 个 production hashes 仍匹配构建记录；USDC 期间测试修改使 `source_unchanged=false`，WETH 期间为 true，但两者均使用 `--allow-dirty`，所以 `valid_frozen_batch=false`。119.915 秒检查点已到 round4，2,342 累计进度 states / 2,458 transfers / 204,911,049,514 work。不同 RPC 轮次的进度计数不能直接与候选 5 的 round2 计数作等量比较。USDC 未返回报告，终态 frontier、summary、SMT 次数不可得；WETH 交付的两个摘要 `closed=[true,false]`。后续格式化发生在测量和哈希复核结束之后，不能用后来工作树的字节变化改写这些 frozen build 来源。

## 候选 6 的独立 CPU 诊断

在上述两次无插桩尝试全部退出后，单独对同一 frozen server、同块原 Unknown USDC request 做一次外部 90 秒诊断。[CPU 证据](evidence/known-recursion/candidate-sixth-cpu-profile.json)保存原样记录、独立两查询 smoke、探针及采样源码、可见栈分类规则和原始 artifact 哈希。使用 49Hz `cycles:u`、16 KiB DWARF callchains，实际 perf record/report 均 exit 0，11,800 个 raw samples、lost samples 0；server 在外部时间保护下退出，未交付分析报告。输入、ELF、197 个生产文件及统计模块在该轮前后校验一致。

| 可见调用栈路径 | 加权 user-cycle 样本比例 |
| --- | ---: |
| native `Z3_solver_check` | 48.84% |
| Z3 context 创建 | 17.48% |
| `Z3_solver_assert` | 12.12% |
| context 清理 | 1.31% |
| summary projection | 1.46% |

表中来自显式优先级的可见栈分类，详细规则与其它类别保存在 JSON；它不是精确阶段计时。普通 children 调用栈会重叠，不能相加。`embedded_smt::check` 的 children 比例为 84.72%，`relations::refine` 为 71.91%；最高 self 行包括 `ast_manager::compact_memory` 11.48% 和两种 AST manager 构造 9.36% / 5.70%。可见过程符号或类型约占 1.51%，Main 外层栈可能被截断，不能据此宣称全部 table/Main 开销为零。当前这段采样直接显示 SMT check、context 与断言构造的主要开销，未据未归类的残差作推断。

外置探针记录 15,175 次 completed native checks：15,045 SAT、130 UNSAT、0 UNKNOWN，clock failures 0；native thread CPU 为 43.789592 秒，进程 user+system 为 92.412031 秒，比例约 47.39%。这是已完成查询 CPU 的观测下界，排除编码、model、context/solver 清理及最后 live snapshot 后的活动；分母是整个 owned process CPU，不是 wall。最后进度仍为 Analyzing round4、2,271 累计 states / 2,340 transfers / 192,983,859,335 work，SSA/导出为 0。单次插桩诊断不能与前述无插桩 wall 混合，也不构成 USDC 解决或加速结论。

[Reader 投影微测](evidence/known-recursion/numeric-reader-projection-cost.json)包含可审查源码、rlib/可执行文件哈希、6 个交替原始样本和完整摘要相等检查。固定 256 字节、一个模糊 WriteWord 的 cell 投影，BYTE 应用从 8192 降到 32；实际 library-default 数值域、debug 库与 opt2 harness 下中位数为 1.503904 / 0.013495 秒。旧侧是通过 root/facts 形状检查的原有 BYTE/join 循环专门化，新侧调用生产 `ByteArray::summary_projection`；这项局部对照只确认计算复用，不作为 release 或 USDC 加速证据。缓存最多 32 项和 8K logical retained work，facts/domain 不跨调用复用，逻辑 Work 收费保持原值；6 个数值、来源、alias/overflow、facts/domain 与真实投影回归全部 exit 0。

## 私有 helper 返回地址的精度回归

最初按更新次数直接 widening，会把并行四个 caller 的有限返回 PC 扩大到不可达目的地。候选 5 的反馈图修复了这组并行 caller，但四次顺序调用同一个 helper 会在合并 CFG 中形成环，过程摘要仍可能返回 `{42,99}`。继续保留前缀并擦除循环的原型修好了直接顺序调用；Outer helper 再调用 Inner helper 时，重复的 Outer 来源又会抹掉最近 caller。MRU 改为保留每个来源 PC 最后出现的顺序，例如 `[a,b,c,b]` 变为 `[a,c,b]`。

[上下文原型证据](evidence/known-recursion/context-prototype.json)保留输入 raw bytes、报告哈希、原样结果、候选 5 来源及原型 source delta。prefix/MRU 使用同一 debug ELF，SHA256 为 `4a7021be4091f911fbeabc6494ac1df4e259f91dba58eccdcc0faaa048f6993a`，仅在独立原型进程中选择策略；生产实现不含该环境开关。小型输入采用 work `1,000,000,000`、relations disabled、constants `512`、context depth `128`，不是原 USDC 的 RPC/策略或性能测量。

| 版本/策略 | parallel 四个 caller | sequential 四次 helper 调用 | nested Outer→Inner |
| --- | --- | --- | --- |
| 候选 5 反馈图 | 过程只返回 42，无 PC336 | 过程含 `{42,99}`，覆盖 PC336 | 本批未单独测量 |
| prefix 原型 | 过程只返回 42，无 PC336 | 过程只返回 42，无 PC336 | 过程含 `{42,99}`，覆盖 PC336 |
| MRU 原型 | 过程只返回 42，无 PC336 | 过程只返回 42，无 PC336 | 过程只返回 42，无 PC336 |

原型 10 次 CLI 均实际 exit 0、`Converged`、无 frontier；MRU 的 context128 parallel/sequential/nested/counter 四项均逐过程检查完整 word42，并排除不可达 PC336。nested 为 54 transfers / 26,949,253 logical work；这是执行计数，不是性能收益。仅检查外层 Return=42 会漏掉上述摘要污染。context0 nested 真实允许 `{42,99}`，外层与过程历史均为空；counter0 仍精确返回 42，保持上下文不敏感策略的合法粗化。

MRU 只保留同一帧的跳转来源，新 CALL 帧历史仍为空，Unknown 帧仍清空历史；不把物理递归深度带回过程 key。规范化预付 work，保留的历史长度计入后续成本。复杂循环的不同来源顺序可能产生多种有限 key 排列，仍受原 context/state/work 上限与 typed frontier 约束，不承诺线性状态数。MRU 原型与迁入生产后的 29 个 procedure 单元、18 个 known-recursion、43 个 unknown-world focused 回归均 exit 0；候选 6 的真实原请求结果已单独列出，完整 Nix 门禁仍需最终记录。

## 可复用对照命令

[`benchmark-known-recursion.py`](../scripts/benchmark-known-recursion.py)支持 offline/RPC 两种模式、baseline/candidate 两侧和重复交替运行。它冻结并前后复核 binary/input/source 身份，保留每次实际退出、资源、task progress、phase timing、response bytes、native status/frontier、summary 和图导出统计。源码默认必须干净；开发诊断必须显式传 `--allow-dirty`，其结果不会标作有效 frozen batch。

准备固定基线和最终干净候选的同类 release 产物，最多使用 8 个构建核心：

```sh
nix build --no-update-lock-file --max-jobs 1 --cores 8 --out-link /tmp/known-recursion-baseline 'git+file:///home/ethever/projects/evm-abstract-lab-worktrees/known-recursive-summaries-20261014?rev=eb86cb5661b77b5be2960ac4b1909c1b36cf6871#packages.x86_64-linux.default'
nix build --no-update-lock-file --max-jobs 1 --cores 8 --out-link /tmp/known-recursion-candidate .#packages.x86_64-linux.default
```

停止其他编译、基准和 probe 负载后，在已记录最终源码 SHA 的干净工作树运行；`KNOWN_RECURSION_CANDIDATE_SOURCE` 和 `KNOWN_RECURSION_ASSETS`须指向实际构建来源与 matching 资产：

```sh
python3 scripts/benchmark-known-recursion.py \
  --manifest examples/known-recursion/captured/manifest.json \
  --output /tmp/known-recursion-final \
  --mode both --runs 3 \
  --baseline-cli /tmp/known-recursion-baseline/bin/evm-abstract \
  --baseline-server /tmp/known-recursion-baseline/bin/evm-abstract-server \
  --baseline-build-source eb86cb5661b77b5be2960ac4b1909c1b36cf6871 \
  --candidate-cli /tmp/known-recursion-candidate/bin/evm-abstract \
  --candidate-server /tmp/known-recursion-candidate/bin/evm-abstract-server \
  --candidate-build-source "$KNOWN_RECURSION_CANDIDATE_SOURCE" \
  --assets "$KNOWN_RECURSION_ASSETS" --rpc-config http://127.0.0.1:8545 \
  --cpus 48-55 --timeout-seconds 120 --rss-limit-gib 8 \
  --host-available-floor-gib 20 --poll-seconds 1 \
  --environment-note 'No concurrent build, tests, other performance or probe batch'
```

每轮反转 baseline/candidate 顺序，进程顺序运行；不把 offline 和 RPC 的不同获取/导出边界混合求中位数。无完整报告的样本仍保留在 attempts 和真实 termination 统计中，不计作成功报告。报告中的 status、frontier 和闭合情况是独立指标，速度或 states 数不能证明覆盖一致。

runner 的 20 次小型 subprocess/HTTP smoke 已验证实际 timeout/native export/typed task failure 分类、交替顺序、owned PID 回收、RPC 配置脱敏和大于 `2^53` 的 u64 task ID。它们只验证实验工具，不作为 EVM 功能或性能样本。

## 已取得的语义与 UI 证据

matching protocol11 Wasm、首轮 release server 的真实 Chromium focused 检查已 exit 0。已知递归 fixture 只提交一次分析，取得 `Converged`、SSA complete、3 个 native states，以及闭合 `KnownRecursion` / `Runtime` summary；Frame inspector 保留真实入口和闭合信息。[已检查截图](evidence/known-recursion/summary-inspector.png)与 [第 18 课](18-unknown-world.md)描述对应能力。这是该 fixture 的语义/UI 验证，不能代替原 USDC 性能验收。

最终冻结性能、真实字节码/revm 回归与完整 Nix 门禁尚待最终产物；PR 必须记录实际 clean source SHA、binary SHA、运行命令和真实退出码，不提前宣称全门禁通过。
