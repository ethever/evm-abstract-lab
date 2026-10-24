# 全局模型与详细路径

CLI 的 `analyze`、world/RPC `explain` 与 Web 默认先运行全局保守过程，再把剩余预算用于详细路径。两部分保留原始 world、caller、value、calldata 与固定快照，使用同一累计执行账本；RPC 的补读、事实细化和入口重跑也计入该账本。`cfg`、`ssa` 与单程序 `explain` 保留原生指令检查入口，Rust 的 `AnalysisEngine::run` 也继续可用。

全局模型保存可能的调用、静态回调、委托调用、构造过程、部署、回滚和出口。详细部分可以仍有 pending 路径。组合结果的 `status` 来自全局模型的闭合与来源资格；详细部分的 `Incomplete`、frontiers 和部分 SSA 独立保留。三个终止备选项只属于可选 native 辅助记录，不会闭合过程关系或清除这些义务。

Web 的 **Global model** 与 **Detailed paths** 分别查看两套图。切换会重置状态、程序和 SSA 的本地选择，数字相同的状态 ID 不互相引用。存在当前详细前缀时，初始视图展示详细路径；全局关系始终可切回。符号根的详细视图会明确列出额外的 `root ADDRESS = …` 条件，全局模型仍覆盖其他允许的根地址。

## 当前前缀的范围

`hybrid.local_prefixes` 中的每项引用 **Native** 命名空间内某个状态的保留入口，携带 snapshot、fact revision、真实 frame/context/source、根地址条件，以及这次 receipt 实际访问的 PC、阶段和已安装后继。其范围是 `RetainedInputPrefix`，或带额外根地址条件的 `NativeOwnerNamespacePrefix`。待处理的前驱仍可能扩大这个入口；某条 receipt 当前有效，不表示整个区域、调用分派或完整 native 图已完成。

| 阶段 | 实际含义 |
| --- | --- |
| Started | 到达指令，操作数尚未消费 |
| OperandsConsumed | 已消费操作数，正常结果尚未完成 |
| Completed | 本地计算和普通指令效果已完成 |
| Dispatched | 已进入调用或终止分派；继续检查已安装和待处理路径 |
| Faulted | 无效指令或栈错误 |

每项的 frontier 索引属于 Native 图；没有当前前缀的待处理状态仍保留在该图的完整 frontiers 中。详细图未访问到某条边，不会删除全局模型的对应路线。增加 work 可以支付更多 native 执行、当前性检查和前缀保留。默认选择一个来源明确的代码区域保留有限跳转上下文；其他区域在实际输入与 key 准入前合并可选历史，仍可继续执行。

## 输入、物理账户与来源

native 15 的 `entry.binding`、wire 20 的 `metadata.root_binding` 明确区分 `Physical {address}` 与 `Namespace {coordinate}`。后者的原始 World 行是提供给符号根的程序/状态模板；坐标不声明物理账户。wire 中对应 runtime 的 `ProgramInfo.code_address=null`，来源为 `InputProgram {hash,fork}`。这在尚未准入任何状态时也成立。

`RootNamespace` 是过程 owner 类，可能与物理 Named 或 Residual 类别名，标签不同不证明物理地址不同。`InputDelegation {target,hash,fork}` 保留已知输入标记及其真实指针，目标执行代码仍未观测。实际已捕获的委托实现 B 继续使用 `Runtime {source:B,…}`。构造代码、部署后 runtime、空执行、预编译与无效委托分别保留自身种类。过程没有私有 PC、栈、内存或 SSA 指令证据。

## RPC 与交付格式

RPC 保留原始闭合基线的 source guards。新增的同快照精确事实只证明当前初始状态集合是原集合的子集，不能把旧 source-current 标记改写到新 revision。wire 分别保存原始、当前和 detail revision、成功提交的 restriction chain、累计采集及精度失败。后来的重启没有新 native 图时，之前有用的图与前缀仍按自己的 revision 保存。取消仍返回类型化取消错误。

CLI JSON 的外层 `status` / `work` 描述组合结果，未请求 SSA 时保留平铺的原生图字段，选中的 Native 关系带 SSA 时，`.analysis` 保留该关系，`.hybrid.baseline` / `.hybrid.detail` 各保留另一套图一次；`.hybrid.local_detail` 显式导出 native receipt，包含独立 frontier 索引。全局 Process 不生成指令 SSA；请求的 Native 明细 SSA 位于 `.hybrid.detail_ssa`，并携带自身的完成状态。`.hybrid.selected_graph` 单独保留所选图的原始 status、work、transfers，外层组合计数不能覆盖这些测量。原生图格式为 15，wire 为 20，域格式仍为 3、逻辑成本仍为 8。历史捕获与原始 profile 保持原版本。

`--no-summaries` / `use_summaries=false` 关闭完成调用的缓存复用，仍运行实际的全局 Process 模型。原有终止覆盖选项传给可选 native 阶段；基线始终关闭该辅助包络。可选阶段未开始或预算不足时，可以没有辅助记录。该记录不决定组合闭合、native 替换或前缀完整性。
