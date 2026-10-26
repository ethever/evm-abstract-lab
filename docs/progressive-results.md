# 渐进结果

`AnalysisEngine::run_hybrid_with_publisher`、
`AnalysisSession::run_hybrid_with_publisher` 和
`analyze_rpc_hybrid_with_publisher` 在闭合且当前的 Global 模型可用后调用
`StagePublisher<N>`，然后才准备可选的 Native 分析。回调可保留
`baseline_handle()`；该不可变句柄保留实际数值载体、快照、事实版本和根绑定。
取消后仍可读取保留的模型。回调不代表 Native 已完成。

CLI 可输出 JSON Lines：

```sh
evm-abstract analyze --world examples/worlds/reentry.json \
  --evm.to 0x0000000000000000000000000000000000000101 --staged-json
```

首行带有 `lifecycle: "Running"` 和 `model_closed: true`，完整写入并刷新后才继续
Native。正常返回时再输出 `lifecycle: "Final"` 的结果。每行都有发布版本、Global
和 Native 版本、原始快照与事实版本。RPC 的新增事实及限制链单独保留，原始模型
不会被标为新版本事实下重新执行的模型。现有单份 JSON、文本和 DOT 输出继续使用
各自的格式；`--staged-json` 单独选择流式格式。

服务端的任务状态与模型闭合状态独立。`JobSnapshot.publication` 给出最新发布身份；
`GET /api/tasks/{id}/result` 返回 `JobResultReply`，其中的成功结果包含任务 ID、发布
身份和报告。存在已发布结果时，Running、Cancelling、Cancelled 或 Failed 任务都可
读取它；客户端应同时检查任务状态与结果身份。任务尚无结果时返回类型化 NotReady。
结果随任务保留策略一起过期。

首次报告的 `minimum` 保存实际的 Global 关系、来源资格和数值输入/联合输出界限。
它不需要完整 SSA 或私有状态池，`global`、`native` 可以都是 `null`。
`minimum_version` 标识这个稳定的 Global 模型；`global_version`、`native_version`
分别标识可选的丰富图。只有复用同一个不可变投影对象时才保留对应版本。
`rich_export` 区分尚未请求、完成、拒绝和取消，丰富导出拒绝不会删除已有最小模型。

服务端的 `--postprocess-work` 设置每个任务共享的一份应用后处理预算，默认保持
2,000,000,000。先支付必需最小模型，再支付可选丰富导出；它独立于请求的 EVM 执行
预算。执行及 RPC 重启继续共享原始累计执行预算，各阶段都不会通过新建预算补救
拒绝。最小模型本身无法付费时仍返回明确失败。

可选细节的来源上限由 CLI `--max-detail-regions` 或 HTTP
`limits.max_detail_regions` 设置，默认 1。只有旧请求省略这个新字段时使用默认值；
显式的 0 关闭细节，null、负数、分数等无效值仍被拒绝。记录历史请求时保留原始字节，
另记省略字段的有效值，不能把补上字段后的对象称为原始请求。

`requested_detail_regions` 在早期、延后及最终 coverage 中保存实际请求上限，
`detail_regions` 是实际选择列表，二者分别检查。0 不支付 Native 准备或执行；
Global 已闭合时细节为 `Disabled`，Global 尚未闭合时仍保留其 pending 资格。
正数沿用同一累计执行账本与剩余额度。RPC 的最新 Native 选择可能属于独立的
`hybrid.precision`，而原始 `hybrid.coverage` 继续限定不可变 Global 模型；读取
详细选择时使用 `native_coverage()`，不以原始列表代替后来的实际选择。

Global 的 Process 控制坐标保留实际的已知代码关系。`ProcessFrameKey.location`
区分解码块 `Block(i)`、未知跳转的共享合法目标节点 `JumpTargets`、代码末尾 `End`
和未观测代码的 `Any`。这些都是源代码的 MAY 坐标；不会生成 Native 的
`executed_pcs`、栈或 SSA。`Block(i)` 的索引按该来源 `Program.blocks` 的顺序解释，
导航时从同一 `programs` 目录读取源指令。

`AnalysisReport.process_controls` 按完整 `(epoch, source)` 保存一份不可变控制关系，
其中的 `program` 绑定来源角色、账户、fork 和 hash。每个实际 Body transfer 的
`process.control` 只引用目录 ID，并保留自己的 `entry`、`through` 和 `boundaries`。
`through` 表示在同一 Store/调用输入下组合的纯控制位置，`boundaries` 表示仍需要
真实 Body transfer 的效果、调用、创建或终止义务。普通未知跳转先到共享目标节点；
该节点自身的实际 receipt 才覆盖合法目标集合。目录中的潜在返回位置不构成调用返回
证据，调用及恢复仍由 regional call/delivery 的实际身份、版本和 current 字段表示。

边界 transfer 可以没有 `process.control`。调用者应读取它的真实 location 和
successors，不能为了补齐图而重放控制分析。可选丰富导出复用已发布的最小模型与
控制目录句柄；完整目录的存在、历史 transfer 或数值界限本身都不重新证明 current。
