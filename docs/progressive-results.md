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

Global 和 Native 使用各自的图版本。只有复用同一个不可变投影对象时才保留 Global
版本；替换模型会更新它。没有 Global 图时，`global_version` 为 `null`。执行及 RPC
重启共享原始累计执行预算，首次和最终报告共享一份应用后处理预算。最终导出拒绝
不会删除先前已发布的结果，也不会取得新的执行或后处理预算。
