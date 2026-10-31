# 已知代码递归：历史结论与回归入口

历史观测绑定源码 `4212e5addd3672a9becdb7cd01d0ded75d836eb9`、main `f70afa2b383d574649831c3ff10a885947a1126a`（native8 / wire13）。该版本已将已知递归接入共享过程不动点；原 WETH 请求交付 `Incomplete`，原 USDC 请求被外部超时终止，没有最终报告。这些观测不能证明原 Unknown 请求已完成，也不用于判断当前版本性能。

生成运行记录已移出 Git 工作树；完整原记录可从清理前的提交读取：

```sh
git show 3801d1d22a2341d0961b4a9a621771be2de5a375:docs/evidence/known-recursion-selected-validation.json
```

## 保留的输入与语义要求

[输入清单](../examples/known-recursion/captured/manifest.json)、[原 USDC 请求](../examples/known-recursion/captured/usdc-original-request.json)、[原 WETH 请求](../examples/known-recursion/captured/weth-original-request.json)及初始 observed worlds 保留原始字节与 SHA256。它们固定 Osaka、Ethereum chain 1、区块 26180958 / `0x5219691ff60b20005fa597d2bb5e4b11e3ddb68f24fb528a900c8dafbb84b499`；caller、call value、calldata 内容与长度保持 Unknown，未观察的槽保持未知。

[100 字节 USDC 见证](../examples/known-recursion/captured/usdc-100-byte-witness.json)是独立的具体输入，不替换原请求。[捕获回归](../crates/evm-abstract/tests/known_recursion/captured.rs)实际执行 selfCALL、selfDELEGATECALL 和 implementation 写入，并检查外层 Revert 后的 storage 与日志回滚。

[`known_recursion`](../crates/evm-abstract/tests/known_recursion.rs)及[可运行自调用示例](../examples/known-recursion/README.md)保留下列验收边界：

- 活动栈上重复调用边与先后返回的普通调用分别处理；owner、code、static、返回控制和回滚检查点不能混淆。
- 输入增长使依赖结果重新计算；无真实终态、仍有依赖或前沿时，不能把摘要命中称为闭合。
- 终止、返回数据、账户效果和日志由[独立 revm 见证](../crates/evm-abstract/tests/known_recursion/oracle.rs)联合检查。
- 深度与 work 资源不足时保留真实前沿。历史深度 3 的闭合用例与深度 2 的 `CallDepth` 对照分别验收，不能把内部摘要闭合当作外层完成。

数值摘要可能扩大参数到输出、跨帧 storage 的关系；`--no-summaries` 仍保留递归闭合，不是逐层展开的独立对照。旧 Z3 插桩结果仅描述该历史版本，不属于当前执行路径。

## 复查命令

```sh
nix develop --no-update-lock-file -c \
  cargo test --locked --offline -p evm-abstract --test known_recursion
```

固定离线与 RPC acquisition 是不同输入边界。当前 CLI profile 和 HTTP 请求按[固定规模检查流程](../examples/no-smt-scale/README.md)从实际构建源码派生，保留原始 capture；新的运行输出放在仓库外。报告闭合、任务完成、取消和导出结果分别读取，完整提交门禁按[本地检查](local-ci.md)执行。

## 当前 benchmark 消费边界

[`benchmark-known-recursion.py`](../scripts/benchmark-known-recursion.py) 新输出声明 `known-recursion-benchmark-v2`：离线 `analyze --world --format json` 严格读取 native16 的固定 `global.analysis` / `native.analysis` 槽，HTTP 严格读取 wire22 的 `Result<JobResult, ApiError>`，核对任务 u64 ID 和五字段 publication。两种格式不能互换；旧版本输出及其声明留在仓库外，不能用当前格式重标。历史输入清单保留原字节，当前调用方需显式提供与被测源码匹配的命令和请求，不由消费端猜测或改写。

HTTP 在 Running 的状态轮询首次公布模型时立即 GET `/result`。`first_closed_model` 记录首个通过版本、当前性、源目录、snapshot / revision / root、执行与数值策略以及原始 Unknown 输入检查的闭合模型；其 `artifact` 指向捕获的完整回复，保留数值输入和同一结果的 data/store 关联。`metrics.graphs.Baseline` / `Native` 为 `null` 表示没有富图，不能解释为零状态、零前沿或完成 SSA。`task_state` 始终描述任务；后续 Failed/Cancelled 不会抹掉已经收到的模型。

最终 D4 的共享 `process_controls` 目录必须显式存在；primitive body 可以使用空列表。benchmark 通过现有 `web_test_hybrid.job_result_report` 共享严格入口接入目录的类型与来源资格检查，拒绝缺失目录的中间版本 wire21 输出，不能把它们重标为最终架构结果。

D5 的请求资格保留三项记录：`request_identity.policy` 是原始 limits map，`effective_policy` 仅在遗漏 `max_detail_regions` 时增加该字段为 `1`，`applied_policy_defaults` 明确记录这次默认值应用。显式 `0` 禁用 Native，显式 `2` 请求最多两个来源；null、布尔、负数、小数、字符串或超出 u64 的值均不能当作遗漏。原始请求字节、哈希、World、header、Unknown 输入及 work/transfer 预算不改写。富图 metadata 的 limits 必须与有效 map 的键、值和类型完全一致；最小报告只验证它实际回显的执行/数值策略和 precision cap，不声称回显了 RPC transport policy。

当前输出必须显式包含 `coverage.requested_detail_regions`，有独立 RPC precision 时也须回显相同请求 cap。`metrics.precision_policy` 分别保存 cap、实际 `selected_detail_regions` 和数量，实际数量不得超过 cap；零 cap 必须无 Native，已闭合 Global 的 detail 状态须为 Disabled。原生 native16 CLI 使用同一资格记录，并核对实际 `--max-detail-regions`（遗漏默认1），不把选中数量当成请求值。缺少新回显字段的 intermediate1975 wire21/native16 产物不能通过 D5 消费入口，也不会被补默认值或重标；历史 adapter 的资格由冻结版本单独声明。`precision_policy_contract` 标记本次消费约定；默认2B、额外 TOTAL6B 与 regions1 的测量 profile 保持原样。

`submit_to_usable_model_seconds` 是外部观测时间，包含轮询、捕获与解析校验开销。触发 GET 的任务状态和广告版本单独保留，结果可能在请求期间前进到更新版本。`terminal_observed_seconds`、`final_result`、`final_detail`、`final_export` 分别记录最终任务观测、结果 GET、Native 资格及富导出 outcome/backend timings；`first_model_to_terminal_seconds` 包含后续工作与观测等待，不能充当纯 Native 执行时间。`cached-result` 是终态后的同一保留模型 GET，`cached-result-gzip` 另记实际 Content-Encoding、传输字节、解压和解析时间；重复 GET 不代表 HTTP 缓存命中。

原始 request 文件和 POST body 保持完全相同；最小报告没有回显 RPC transport policy，记录会明确区分已核对的执行/数值策略和原样提交的完整请求策略。私有 RPC 配置与回复/日志中的 endpoint 脱敏，HTTP 原始字节长度和哈希保留。离线 World 的耗时与 RPC acquisition 的耗时分组统计，不据此生成性能或等价性结论。源级假 HTTP/严格解析回归不启动引擎或连接 RPC：

```sh
python3 -B scripts/test_benchmark_known_recursion.py
nix run --no-update-lock-file .#check-changes --
```
