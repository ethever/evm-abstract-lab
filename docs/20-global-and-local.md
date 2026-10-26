# 全局模型与详细路径

CLI 的 `analyze`、world/RPC `explain` 与 Web 默认先运行全局保守过程，再把剩余预算用于详细路径。两部分保留原始 world、caller、value、calldata 与固定快照，使用同一累计执行账本；RPC 的补读、事实细化和入口重跑也计入该账本。`cfg`、`ssa` 与单程序 `explain` 保留原生指令检查入口，Rust 的 `AnalysisEngine::run` 也继续可用。

全局模型保存可能的调用、静态回调、委托调用、构造过程、部署、回滚和出口。详细部分可以仍有 pending 路径。组合结果的 `status` 来自全局模型的闭合与来源资格；详细部分的 `Incomplete`、frontiers 和部分 SSA 独立保留。三个终止备选项只属于可选 native 辅助记录，不会闭合过程关系或清除这些义务。

Web 的 **Global model** 与 **Detailed paths** 分别查看两套图。渐进任务先安装可读的全局发布；后续 Native 到达时保留当前命名空间、相机和有效选择，可以手动切换。切换时状态、程序和 SSA 的本地 ID 按所属图重新解释，数字相同不能跨图引用；用户显示偏好保留。符号根的详细视图会明确列出额外的 `root ADDRESS = …` 条件，全局模型仍覆盖其他允许的根地址。

## 控制关系怎样闭合

假设 A 的已知代码调用未知地址 U，U 可以再调用 A，随后 A 返回或回退。全局模型保留 A、U 的过程节点、CALL 类动作、真实 subscriber 和返回续点；U→A 的回调继续参与同一个 Body／Resume 固定点。新增输入会更新 callee 的汇合输入，结果变化会重新投递给等待的 caller。每个 subscriber 独立持有 caller 状态及回滚检查点。

过程解释忘掉私有栈、内存和跳转历史，保留代码来源、owner 类、static 模式及 ADDRESS／CALLER／value 的汇合输入。已知代码按真实解码块选择本次能力和控制后继，只有到达的效果／调用边界才执行相应动作。`STOP; CALL` 中不可达的 CALL 不进入过程关系，PUSH 数据里的 CALL 字节也不是指令；它们不会因为出现在同一份代码中而变成任意可调用动作。

[`ProcessLocation`](../crates/evm-abstract/src/analysis/process/frame.rs)将 `Block(i)`、未知跳转的 `JumpTargets`、`End` 与未知实现的 `Any` 分开。块内确定的跳转或条件可以排除后继；未知跳转经共享 hub 覆盖全部合法 JUMPDEST，并保留异常 Failure。CALL／CREATE 的 caller continuation 位于后续控制位置，真实子调用的 Return／Revert／Failure delivery 才恢复该续点及检查点；资金或深度拒绝也保留其实际继续关系。

纯控制区域使用同一 kernel 中的[宏步关系](../crates/evm-abstract/src/analysis/process/control.rs)：共享 source／epoch 限定的解码关系，用 `through` 记录经过的纯 MAY 位置，用 `boundaries` 记录仍需数值解释的效果／调用／出口边界，不为每个纯块复制一份数值 Body。未知实现继续使用有限 owner／source 类下覆盖任意外部实现的操作过程。局部效果、重复、出口与 CALL／STATICCALL／DELEGATECALL／CALLCODE／CREATE／CREATE2 共同参与同一相对固定点。

启用合作组件时保留完整的逐块 hook 调度；实际控制步骤选定后才消费该位置的 `ProcessFacts`。一个块的 readonly 保证不能变成整个 callee 子树的证明。没有动作的 Process 步骤可直接操作付费数值摘要；调用／创建边界及未满足紧凑表示前提的输入仍走原有适配路径，不另建求解器。

Return 提交子帧共享效果；Revert／Failure 使用对应的保存点。constructor 的 allocation guard、nonce、零结果、部署、runtime 返回和外层 rollback 分开记录。进入帧后的 Failure 包含有限 gas 下的异常终止，纯循环也保留此分支。work 耗尽仍是分析 frontier。

过程图的 `Block(i)` 是来源限定的 MAY 块位置，宏步的 `through` 也是保守控制证据。它们不填入 Native `executed_pcs`、`visited_block` 或 SSA。`transfer.process.control`、实际调用和 `delivery` 分别保留宏步关系、交互与依赖投递；原生细节中的访问证据使用独立的 ID 空间。

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

例如两个位置进入同一个带返回目标的子程序，选中区域的不同 history 可以让两份栈值分别汇合，从而保留各自的 Jump 目标。实际准备状态和相对位置先完成 history 处理，再交给原有 key／join 策略；owner、来源、static、caller 与 rollback 仍按原表示保留。

未选中 callee 的相对准入合并可选的调用者跳转历史；caller continuation 和回滚检查点仍由各自 subscriber 保留。原始环境 calldata 与子帧独立 calldata 的来源标志仍区分私有过程身份，不能因为公开 FrameKey 相同而强行做不兼容的形状 join。细节阶段仍执行未选中的 Native 代码并支付其费用，全局过程的闭合不依赖这些详细路径全部完成。

## 输入、物理账户与来源

当前[原生格式](../crates/evm-abstract/src/analysis.rs)的 `entry.binding` 与 [Web wire](../crates/evm-abstract-protocol/src/lib.rs) 的 `metadata.root_binding` 在没有接纳任何状态时也存在。库的 `root_binding()` 不依赖已执行帧；`declared_root_source(...)` 单独支付原始声明来源的读取，不构成 PC 访问证据。

| binding／owner | 表达的事实 |
| --- | --- |
| `Physical { address }` | 原始 To 是这个实际物理地址 |
| `Namespace { coordinate }` | 原始 To 未固定；coordinate 保存所提供的程序／状态模板 |
| `RootNamespace` | 该模板对应的逻辑根 owner；可以与任意物理 owner 别名 |
| `Named(address)` | 冻结目录中的物理地址 |
| `Residual` | 其余物理地址，包括恰好等于 coordinate 的数值地址 |

私有 coordinate 不参与物理 CALL 的代码解析。模板程序的来源为 `InputProgram { hash, fork }`，其 wire 程序目录项 `code_address=null`；已提供委托标记、目标实现未观测时，来源为 `InputDelegation { target, hash, fork }`，可执行种类仍是 Unknown。确实观测到的委托实现保留 `Runtime { source, hash, fork }`。namespace 与物理地址数值相同不证明相同或不同的账户。

可写效果双向弱化可能别名的 persistent／transient storage、nonce 和生命周期事实。CALL 的 static value guard 与 CALLCODE 分开；static CALLCODE 仍允许非零 value，DELEGATECALL 保留 caller／value 继承。只有具备 guard 的物理 Named 创建目标可以强重置。内部固定点保留 namespace 检查点；公开的过程 Store 与全局 outcome Store 把私有模板投影进独立的物理 outside bound，包含外层 Failure。原始 World 的模板作为带 qualifier 的输入展示。

程序比较先支付共享不可变 capture 的正向快捷路径；独立分配的程序继续支付完整解码数据的值比较。hash 相同不能替代这次比较，拒付与取消仍返回对应错误。构造、复制、native 查询与结果保留各自遵循实际载体的报价。

## 实现中的闭合条件

1. owner／代码词汇冻结在完整 source epoch：物理 names、可选单个 namespace、一个 residual，以及有限的已捕获／opaque／boundary source 类。已知控制位置来自有限的解码块、共享跳转 hub 与结束位置；未来创建的代码继续由 opaque complement 覆盖。精确纳入新来源需要来源版本变化及重新分析。
2. scalar、Store、nonce 等保存在汇合输入中，使用现有 join／widen 契约和 feedback 策略；变化会使依赖的 Body／Resume／subscriber／source 义务重新有效。内置 prepared Process 表示可复用有限数值坐标的覆盖 join 与付费完整相等检查；自定义策略及尚未准备的 Native 输入保留自己的路径，不据此宣称任意自定义域都会终止。
3. 能力扫描、效果、copy、native 构造／比较／query、admission 和发布走原有账本。拒付、取消、未覆盖的 catalogue、foreign premise 或不能解释的 native／process continuation 保留类型化失败或 frontier。原生数值 carrier 的实际收费和契约仍适用。
4. `Converged` 还要求相关队列和依赖义务全部完成。World 闭合检查自身输入 revision、输出 generation，以及实际保留的 Body／Resume 求值与来源 receipt。相同的数值行或共享目录不能代替这些 guard：外部前提退役时新快照失效，同数值重新证明也会唤醒 World；旧不可变模型保留自己的发布依据。可选根终止覆盖只属于原生细节；过程 baseline 始终关闭该选项，其闭合由自身控制关系建立。

这套实现条件说明所支持的有限抽象如何覆盖局部步骤与外部交互；有限 revm 样本用于发现遗漏。具体测试见 [已知控制与独立 revm 包含性](../crates/evm-abstract/tests/process_route_control.rs)、[全局控制与 namespace](../crates/evm-abstract/tests/global_process.rs)、[来源 currentness](../crates/evm-abstract/tests/process_source_currency.rs)、[实际增长／退役与重新证明](../crates/evm-abstract/src/analysis/engine/cooperation/tests/assembled.rs)、[混合选择／预算／局部精度](../crates/evm-abstract/tests/hybrid.rs)。foreign cooperative process 输入和未支持的混合 continuation 仍有显式边界。

## RPC 与交付格式

RPC 保留原始闭合基线的 source guards。新增的同快照精确事实只证明当前初始状态集合是原集合的子集，不能把旧 source-current 标记改写到新 revision。wire 分别保存原始、当前和 detail revision、成功提交的 restriction chain、累计采集及精度失败。后来的重启没有新 native 图时，之前有用的图与前缀仍按自己的 revision 保存。取消仍返回类型化取消错误。

CLI JSON 的外层 `status`、`work`、`transfers` 描述组合资格与累计计数。`.global.analysis` 和可选 `.native.analysis` 始终位于固定字段，`selected_view` 只表示初始展示选择。每张图保留自己的 status、work、transfers、snapshot 和 revision；Native 图旁的 `ssa` 或 `partial_ssa` 使用同一 Native 状态 ID。`.hybrid.local_detail` 显式导出 Native receipt 和该图的 frontier 索引；不按选择递归嵌套另一张报告。

Web wire 使用独立的有类型投影。封套保存 `.global`、`.native` 和唯一的 `.programs` 目录；每个 `ProgramInfo` 保留 source account、runtime/initcode 类型、fork、hash、字节码与完整反汇编。CFG 节点通过 `program` 与 `basic_block` 引用目录，根反汇编通过 `metadata.root_program` 查询。相同源码的多个机器状态共享指令，但每个状态的 executed PC、SSA、输入和覆盖仍独立；不同源账户的相同字节码不会只按 hash 合并。每图 `metadata.source_snapshot` / `metadata.revision` 与 Hybrid 声明相互校验，任务 publication version 另行表示交付更新。CLI 原生数值载体和 Web wire 分别保留自己的格式。

`--no-summaries` / `use_summaries=false` 关闭完成调用的缓存复用，仍运行实际的全局 Process 模型。原有终止覆盖选项传给可选 native 阶段；基线始终关闭该辅助包络。可选阶段未开始或预算不足时，可以没有辅助记录。该记录不决定组合闭合、native 替换或前缀完整性。

CPU／wall／RSS、完整原生 JSON 文件、Web pooled JSON 与真实 HTTP gzip 是各自的测量边界。比较时固定输入、来源、策略和源码／实体二进制，分别记录状态、frontier、实际字节与耗时；逻辑 work 不等于 CPU 时间。完整本地门禁按 [local-ci](local-ci.md) 执行。
