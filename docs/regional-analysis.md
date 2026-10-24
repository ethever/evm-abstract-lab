# 全局过程闭合与按区域保留的原生精度

混合分析先求一个保守的全局控制／效果关系，再把剩余预算用于原始输入的原生细节。两次执行共享原有 work 账本、状态计数和来源身份。`baseline` 与 `detail` 分别保存各自的图和 frontier；`coverage` 记录实际选择的关系与支出。库的 `AnalysisEngine::run` 保留原来的原生入口，`run_hybrid` 执行这个组合。

## 从一次回调看模型

假设 A 的已知代码调用未知地址 U，U 可以再调用 A，随后 A 返回或回退。全局模型保留 A、U 的过程节点、CALL 类动作、真实 subscriber 和返回续点；U→A 的回调继续参与同一个 Body／Resume 固定点。新增输入会更新 callee 的汇合输入，结果变化会重新投递给等待的 caller。每个 subscriber 独立持有 caller 状态及回滚检查点。

过程解释忘掉私有栈、内存和跳转历史，保留代码来源、owner 类、static 模式及 ADDRESS／CALLER／value 的汇合输入。已捕获字节由付费的能力扫描决定动作集合，PUSH 数据里的 CALL 字节不构成动作。未知实现使用覆盖任意外部实现的操作过程。局部效果、重复、出口与 CALL／STATICCALL／DELEGATECALL／CALLCODE／CREATE／CREATE2 共同构成控制关系。

Return 提交子帧共享效果；Revert／Failure 使用对应的保存点。constructor 的 allocation guard、nonce、零结果、部署、runtime 返回和外层 rollback 分开记录。进入帧后的 Failure 包含有限 gas 下的异常终止，纯循环也保留此分支。work 耗尽仍是分析 frontier。

过程图的节点没有指令 PC、基本块或 SSA 访问证据。`transfer.process`、实际调用和 `delivery` 给出的是采用的控制规则与依赖投递；原生细节中的访问证据使用独立的 ID 空间。

## 原始输入与物理账户

`Entry.binding` 在没有接纳任何状态时也存在：

| binding／owner | 表达的事实 |
| --- | --- |
| `Physical { address }` | 原始 To 是这个实际物理地址 |
| `Namespace { coordinate }` | 原始 To 未固定；coordinate 保存所提供的程序／状态模板 |
| `RootNamespace` | 该模板对应的逻辑根 owner；可以与任意物理 owner 别名 |
| `Named(address)` | 冻结目录中的物理地址 |
| `Residual` | 其余物理地址，包括恰好等于 coordinate 的数值地址 |

私有 coordinate 不参与物理 CALL 的代码解析。模板程序的来源为 `InputProgram { hash, fork }`；已提供委托标记、目标实现未观测时，来源为 `InputDelegation { target, hash, fork }`，可执行种类仍是 Unknown。确实观测到的委托实现保留 `Runtime { source, hash, fork }`。namespace 与物理地址数值相同不证明相同或不同的账户。

可写效果双向弱化可能别名的 persistent／transient storage、nonce 和生命周期事实。static 规则保留其限制，CALLCODE 的非零 value 与 DELEGATECALL 的 caller／value 继承仍单独处理。只有具备 guard 的物理 Named 创建目标可以强重置。内部固定点保留 namespace 检查点；公开的过程 Store 与全局 outcome Store 把私有模板投影进独立的物理 outside bound，包含外层 Failure。原始 World 的模板作为带 qualifier 的输入展示。

## 局部精度怎样改变实际状态

`HybridConfig` 默认选择一个已捕获代码区域，从根 executable 开始。选择器在真正的 `MachinePartition::prepare*` 中先处理输入，再产生 key：选中区域保留调用者策略允许的 bounded jump history，其他区域的准备状态和相对位置 history 都先清空。owner、代码来源、static、caller 和 rollback 继续参与各自原有表示。

例如两个位置进入同一个带返回目标的子程序，选中区域可以让两份栈值在不同 history 下分别汇合，因此保留各自的 Jump 目标。这种差异存在于实际求解输入和状态中。详细图仍有待处理工作时，可以展示当前 retained input 的已执行指令、阶段、已安装 successor 和本地 frontier。

这些记录的 scope 为 `RetainedInputPrefix`；尚未处理的前驱可能继续扩大其输入。原始 To 为 Namespace 时，付费复制出的原生 entry 额外施加 `To = namespace` 条件，记录使用 `NativeOwnerNamespacePrefix` 和明确 binding。该条件没有收窄原始 baseline 的输入，也不能用来替代其所有别名行为。

只有原生关系自身闭合、来源有效且覆盖同一原始 owner binding 时，`chosen=Native` 才成立。否则 `ProcessWithDetail` 保留闭合的过程关系和当前局部观察，`Process` 保留过程关系。`detail_current` 单独不能授权替换原始符号输入。

## 闭合依赖哪些实现条件

1. owner／代码词汇冻结在完整 source epoch：物理 names、可选单个 namespace、一个 residual，以及有限的已捕获／opaque／boundary source 类。未来创建的代码继续由 opaque complement 覆盖；精确纳入新来源需要来源版本变化及重新分析。
2. scalar、Store、nonce 等保存在汇合输入中，使用现有 join／widen 契约和 feedback 策略；变化会使依赖的 Body／Resume／subscriber／source 义务重新有效。闭合读取检查当前输入 revision、输出 generation 与来源 guard。
3. 能力扫描、效果、copy、native 构造／比较／query、admission 和发布走原有账本。拒付、取消、未覆盖的 catalogue、foreign premise 或不能解释的 native／process continuation 保留类型化失败或 frontier。原生数值 carrier 的实际收费和契约仍适用。
4. `Converged` 还要求相关队列和依赖义务全部完成。可选根终止覆盖只属于原生细节；过程 baseline 始终关闭该选项，其闭合由自身控制关系建立。

这套实现条件说明所支持的有限抽象如何覆盖局部步骤与外部交互；有限 revm 样本用于发现遗漏。具体测试见 [全局控制与 namespace](../crates/evm-abstract/tests/global_process.rs)、[来源 currentness](../crates/evm-abstract/tests/process_source_currency.rs)、[混合选择／预算／局部精度](../crates/evm-abstract/tests/hybrid.rs)。foreign cooperative process 输入和未支持的混合 continuation 仍有显式边界。

## 来源扩展和交付

RPC 混合入口先保留原始 baseline，再由同一固定 snapshot 的 Session 获取代码／storage 事实并重启可选原生细节。只有实际 fact restriction 可以把原 baseline 关系用于更窄的当前输入；原 baseline 的 revision 和 source-current 记录保持其原始含义。报告分别保存 original／current／detail revision 与较早仍有用的局部观察。获取、重启和可选额度共享同一账本，取消从公开 API 返回类型化错误。

CPU／wall／RSS、完整原生 JSON 文件、Web pooled JSON 与真实 HTTP gzip 是不同测量边界。记录状态、frontier、输入／源码／二进制 hash 和实际导出字节后才能比较；逻辑 work 不等于 CPU 时间。完整本地门禁仍按 [local-ci](local-ci.md) 执行。
