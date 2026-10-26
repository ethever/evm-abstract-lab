# Rust ELK 选型与 CFG 几何边界

CFG 使用锁定的 `elkrs = "=0.1.1"` 生成分层布局。布局输入是显式范围内已测量的卡片、边标签和有类型的拓扑；输出是叶节点矩形、完整边折线、标签矩形和空间分组。ELK 接口不接收窗口、平移或缩放；前端首次测量卡片时按实际图区选择尺寸与预览密度。实际接口见 [`evm-abstract-layout`](../crates/evm-abstract-layout/src/lib.rs) 与[几何数据类型](../crates/evm-abstract-layout/src/model.rs)。

这里分开记录三类证据：两套上游移植的选型探针、本仓库当前实现，以及尚未解决的限制。选型探针中的小图是用于隔离几何行为的合成输入，不是链上合约分析结果；原生运行和 Node Wasm 运行也不能替代真实浏览器验收。

## 两套 Rust 移植的实测

2026-10-10 的选型实验使用项目 Nix 环境中的 Rust 1.99.0，Cargo 并发为 4。两套候选都直接调用 Rust 图模型与布局引擎，未通过 Java、JavaScript 布局实现或命令行文本转换取得几何。

| 候选与固定版本 | 实际运行结果 | 本次覆盖的输入 |
| --- | --- | --- |
| [depetrol/elkrs 0.1.1](https://docs.rs/elkrs/0.1.1/elkrs/)，发布包源码提交 `344c18cc1992ef39a622d1ca9f97cf89e159cb36`；另测 main `265115909878e4a54efc86f5da9a53d945303b13` | 两版原生运行退出 0；0.1.1 的 Wasm check、完整 cdylib build 退出 0；两版 Wasm 均在 Node `WebAssembly` 中执行，七例均无失败、退出 0。两版在这些输入上输出相同几何 | 三节点旁路、128 节点长链、跨程序容器并含返回环、普通环、自环、平行边、单边多个标签 |
| [openedges/elk-rs](https://github.com/openedges/elk-rs/tree/2191680292b9565592223c22a28b5ef33c3acbad)，提交 `2191680292b9565592223c22a28b5ef33c3acbad` | 使用 graph/core/layered Rust crates 的原生探针、Wasm build 和 Node Wasm 执行均退出 0，七例完成；另一个 mixed-direction `SEPARATE_CHILDREN` 探针因斜线违反正交检查，退出 101 | 跨容器、环与自环、32 节点长链、混合方向 `INCLUDE_CHILDREN`、嵌套容器、平行边、三节点旁路 |

成功案例检查了原叶节点与边身份、原 source/target、有限的正尺寸矩形、完整且轴对齐的折线、真实叶节点边界上的起终点、叶节点互不覆盖、边不穿过无关叶节点，以及边标签不覆盖叶节点。elkrs 还核对了每条原边的标签数量。Wasm 探针的 imports 为空，确实执行了编译后的 Rust 布局代码；这项证据只证明 CPU Wasm 路径可运行。

openedges 的额外几何审计发现部分跨层线路穿过容器顶部的居中标题。`INCLUDE_CHILDREN` 的混合方向输入仍沿父容器方向排布；改用 `SEPARATE_CHILDREN` 后，一个跨层边段从 `(924, 76.5)` 到 `(284, 89)`，成为斜线。它的 32 节点向下长链仍为 `184 × 4320`，容器本身不会自动改善长宽比。

这不是两库的完整对照基准：案例集合和尺寸不同，单次计时不能支持性能优劣结论。本次没有运行两库完整上游测试，也没有独立复核上游声称的 Java ELK 全量 parity；七类案例通过不构成对任意 CFG 的几何证明。

## 选择 elkrs 0.1.1 的理由

两套候选都有真实的 Rust compound layered 实现。当前需求在 elkrs 的发布版本上已验证：跨层边保留真实叶节点端点、返回边保留方向、旁路避开中间节点，边与标签共同参与布局。发布版和所测 main 在这些输入上的几何相同，因此使用有固定版本和锁文件记录的发布包，无需依赖浮动 Git main。

elkrs 是一个 crate，可通过 `ElkGraph`、`NodeId`、`EdgeId`、`ShapeId`、有类型的 `Property<T>` 与 `RecursiveGraphLayoutEngine::layout` 直接适配。本仓库把这些上游类型封装在[私有适配器](../crates/evm-abstract-layout/src/engine.rs)内，公开边界使用自己的 Rust 结构与枚举。没有公开 ELK JSON 接口；Web 获取分析报告所用的 JSON 与布局引擎的调用边界是两件事。

这一选择基于当前所需能力、发布版本实测和适配范围。openedges 的成功案例同样证明它可用于 Rust/Wasm 布局；其 npm/NAPI 包、完整 workspace 与其它算法未在本次选型中验收。

## 当前实现

### 分组保持分析身份

[分组规则](../crates/evm-abstract-layout/src/groups.rs)只增加空间容器，不生成分析状态、SSA 合并或替代边。每个容器的 `GroupId` 与输入叶节点、边 ID 分开；输入边始终连接原叶节点，不改成连接程序框或 Chain 框。

- **Program**：显示图包含至少两个已知程序时，为拥有至少两个叶节点的程序建立容器；单程序图不会仅为包框增加这一层。
- **Cycle**：在程序相同的普通控制流边上计算强连通分量，含自环的单节点也可成组。CALL、RETURN、Failure、Revert 不作为内部普通连接。
- **Chain**：每个内部连接的源只有一条出边、目标只有一条入边；平行边按实际数量计数。还要求普通控制流、相同程序、已知且相同的帧深度、两端无前沿或隐藏关联边，并排除 Cycle 成员。

这些条件作用于提交给布局的显示图。默认来源 CFG 使用[来源级投影](../crates/evm-abstract-web/src/widgets/graph/minimum/control/projection.rs)：源码直接邻接、调用／异常边界与区域宏关系分别保留，宏关系可在 Graph → Model relations 或 Model overview 检查；没有捕获代码的保守上下文使用抽象边界。Detailed paths 的 States 保留范围内原生状态及边，Blocks 先按[Native 显示投影](../crates/evm-abstract-web/src/widgets/graph/projection.rs)聚合，并提供 Instances、Local 和可选 Overview。布局只保持所提交投影的身份；来源点不等于 Native 已执行状态，聚合后的 A→B→C 也不能证明来自一条可接续的执行路径。空间 Cycle 同样不是新的分析结论。

### 固定世界坐标与联合边布局

适配器固定使用 `layered`、`DOWN`、`ORTHOGONAL`、`INCLUDE_CHILDREN` 和 `RANDOM_SEED=1`。叶节点卡片与边标签由 egui 的实际字体和内容测量，标签作为原边的 ELK label 参与布局，不再单独建立一层虚拟标签节点。短容器标题预留顶部空间；若返回线路仍穿过标题位置，适配器只把标题移到空闲标题区或图外边距，保留线路和真实端点。

ELK 节点坐标相对于父节点，边 sections 与边标签坐标相对于边的 `containing_node`。适配器累加祖先偏移转换为世界坐标，保留每个 section 的起点、折点和终点，并按原 source→target 拼接全部 sections。导出前核对原端点、标签身份、测量尺寸、容器包含关系、有限坐标、正交性和完整性；失败以 `LayoutError` 返回。Web 的[转换层](../crates/evm-abstract-web/src/widgets/graph/layout.rs)另检查坐标能否安全转换为 Painter 使用的 `f32`。

布局输入不含 viewport。默认 Entry 窗口取实际锚点及全部真实前向一跳备选，含调用边界和实际交付的调用者续点，不沿入边扩展，也没有固定点数上限。Graph 菜单的 Nearby 选择有限邻域，Full source CFG 选择当前上下文全部已准入保留的来源点，含断开、待展开和历史部分；Model overview 另行展示区域关系。这里的来源总数不等于全部静态字节码块或 Native 已执行状态数。

仅因窗口范围省略的入边／出边保存在不参与节点布局的有类型边界元数据中，保留原边种类、层级、来源与精确限定端点，仍可通过目标入口和菜单检查。首次布局先取得实际图区，再选择卡片宽度和预览行数，按入口及其直接备选的完整矩形框定镜头。[范围缓存](../crates/evm-abstract-web/src/widgets/graph/minimum/scene.rs)保留所选窗口与布局偏好；普通 resize、分栏变化和 Back 保留几何与镜头，显式切换窗口可重新布局。Fit 只适配当前几何。

### 语义缩放只改变绘制内容

[语义缩放](../crates/evm-abstract-web/src/widgets/graph/paint.rs)按比例选择轮廓、身份与详细预览，并使用滞回避免闪烁。低倍或过小卡片不绘制不可读文字，文字裁剪在各自卡片边界内；详细预览仍提供测量尺寸，缩放切换不会重新布局、收缩节点矩形或更改 ID。选择与检查继续使用原有来源／上下文或 Native 实例资格，不因可见性获得额外的执行或 SSA 证据。

当前 `layout(&Input)` 是同步计算，Web 在更新布局时同步调用它。拥有型输入输出为后台执行提供了类型边界，但当前实现没有把 ELK 任务提交到浏览器 Worker 或 Rayon 池；引入 Rust ELK 本身不能证明大图布局期间 UI 不阻塞。

## 真实合约 fixture 的范围

仓库另保存三份 Ethereum 主网实际 runtime code 及采集来源：chain ID 为 1，固定区块为 `26162721`，hash 为 `0x249b266a4b3e71abf93278a2e96938a49099ac2afafa6f8193e28c21872d57dc`。普通可信 RPC 的 `eth_getCode` 使用该 hash 和 `requireCanonical=true`；各 JSON 元数据保留地址、code hash、代码长度和精确 ABI calldata。重复布局检查使用保存的代码与输入，不依赖链上 RPC 在线可用。

| 实际代码与输入来源 | 原生 CLI CFG 协议图规模 | 分析结果与边界 |
| --- | --- | --- |
| WETH9 [runtime](../crates/evm-abstract-web/tests/fixtures/layout/ethereum-weth9.hex) 与[元数据](../crates/evm-abstract-web/tests/fixtures/layout/ethereum-weth9.json)：精确 `transferFrom(address,address,uint256)` 输入 | 20 个原生状态、20 条边、18 个已捕获源码块 | `Converged`，无前沿；余额、allowance 与 caller 仍为抽象输入，只覆盖该 ABI 入口 |
| Uniswap V2 Router02 [runtime](../crates/evm-abstract-web/tests/fixtures/layout/ethereum-uniswap-v2-router02.hex) 与[元数据](../crates/evm-abstract-web/tests/fixtures/layout/ethereum-uniswap-v2-router02.json)：`getAmountsOut(1000000, [WETH, USDC, DAI])` | 43 个原生状态、42 条边、38 个已捕获源码块 | `Incomplete`，保留外部 pair 的 `MissingCode` 前沿；这是缺失被调用者代码的单入口检查，规模不作为大图证据 |
| Multicall3 [runtime](../crates/evm-abstract-web/tests/fixtures/layout/ethereum-multicall3.hex) 与[元数据](../crates/evm-abstract-web/tests/fixtures/layout/ethereum-multicall3.json)：`aggregate3` 中八次自身 `getChainId()` 调用，root 为 `0xca11bde05977b3631167028862be2a173976ca11`，显式 `CHAINID=1` | 648 个原生状态、653 条边、61 个已捕获源码块 | `Incomplete`，保留 8 个 `Memory` 前沿；覆盖范围仍是这一精确入口和 standalone 抽象账户环境 |

Multicall3 的同一 calldata 已通过同一 canonical block 上的只读 `eth_call` 对照，成功返回 1344 字节，结果摘要保存在元数据中。这证明 ABI 输入可供链上合约执行；原生抽象报告仍保留其 `Memory` 前沿，不能据此改称全入口覆盖或完整的链上调用分析。

<a id="已通过的浏览器布局验收"></a>

### 浏览器中的 fixture 阅读

完整报告中的状态数不等于默认来源窗口的卡片数。阅读以上 fixture 时先从入口的限定范围开始，再按需打开 Detailed paths、Overview 或 Local，并选择实际实例；全图 Fit 可能把长链文字缩小，Entry 可恢复可读定位。首次卡片尺寸与预览密度按实际图区选择，resize／Back 的稳定性适用于同一显式范围及其已保留的布局偏好。当前操作说明见[Web 工作台](web-workbench.md#阅读结果)。

[浏览器检查脚本](../scripts/test-web-browser.py)使用保存的 runtime、calldata、root address 和环境字段提交任务；完整检查要求匹配当前源码的服务与 Wasm，按[完整本地门禁](local-ci.md)执行。fixture 的既有报告计数、合成布局探针或图上可见卡片数均不能替代当前提交的检查结果。

## 已知限制与验证范围

[elkrs 发布版的上游说明](https://github.com/depetrol/elkrs/blob/344c18cc1992ef39a622d1ca9f97cf89e159cb36/README.md#known-divergences)仍列出以下限制。本次使用节点端点、DOWN 和调用方测量的标签，不覆盖这些外部端口相关路径：

- 一个合并的 external port 连接至少四个容器内部节点时，子节点顺序可能与 Java ELK 不同。
- `direction=UP` 下 `nodeLabels.placement` 的回显选项有差异；上游将其描述为几何不变的显示差异，本次未测试 UP。
- `restoreDummy` 的外部端口 `PORT_LABELS` 分支会返回 `Err`，上游说明其测试输入未到达该分支。
- `topdownLayout=true` 引擎模式报错；未配置 `ILabelManager`，CENTER/END 标签管理处理器为空操作。当前适配器提供测量尺寸并使用 CENTER placement，不依赖该管理器动态调整标签，也没有请求 topdown 引擎模式。

上游图属性内部使用动态属性值，不能把 ELK 对象当作可跨线程的公开数据。未来执行器应在所选线程内建立并销毁 ELK 图，仅传递本仓库自己的输入输出。引擎 API 没有进度或取消回调；丢弃迟到结果可以阻止旧图覆盖新图，却不能提前中止计算。依赖仍可能 panic，默认 Wasm panic-abort 也不能靠原生 `catch_unwind` 获得恢复保证。

当前 Chain 容器仍沿固定 DOWN 方向布局，没有蛇形折行或独立方向的紧凑链算法；全图 Fit 在很长的链上仍可能把文字缩小。语义缩放和包框提供阅读层次，不等于解决长链长宽比。换用其它层级方向、external ports 或取消机制，需要新增适配与相应验证。

本仓库的[分组回归](../crates/evm-abstract-layout/src/groups/tests.rs)、[几何回归](../crates/evm-abstract-layout/src/engine/tests.rs)、[Web 转换回归](../crates/evm-abstract-web/src/widgets/graph/layout/tests.rs)和[相机回归](../crates/evm-abstract-web/src/widgets/graph/tests.rs)分别检查这些规则和不变量。测试代码描述检查范围，实际通过状态应由当前提交的测试输出与[完整本地门禁](local-ci.md)确认。浏览器验收只覆盖本节列出的输入与交互，不能由合成选型案例或这些浏览器案例推导任意 CFG 的几何、性能或覆盖保证。
