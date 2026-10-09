# egui Web 工作台

工作台在浏览器中显示反汇编、SSA、跨合约 CFG 和机器状态。输入可以是普通 EVM 运行时字节码，也可以是后端配置的 RPC 提供者与链上地址；执行规则可选 Cancun、Prague 或 Osaka。原生 Rust 后端直接调用核心分析库，不启动 CLI 子进程，浏览器不加载原生 SMT 库。

## 启动

从仓库根目录运行：

```bash
nix run .#web
```

Nix 构建 WebAssembly、匹配版本的 wasm-bindgen JavaScript 绑定和原生服务，并将静态资源与 `/api/tasks` 放在同一来源下。浏览器打开服务打印的地址即可使用。

默认不配置 RPC 提供者，字节码分析可直接使用。需要分析链上账户时，在后端准备 JSON 配置文件，例如 `rpc-providers.json`：

```json
{
  "providers": [
    {
      "id": "mainnet",
      "name": "Ethereum mainnet",
      "endpoint": "https://rpc.example.org/ethereum"
    },
    {
      "id": "local",
      "name": "Local development chain",
      "endpoint": "http://127.0.0.1:8545"
    }
  ]
}
```

将示例地址替换为实际 RPC 地址，然后启动：

```bash
nix run .#web -- --rpc-config /absolute/path/to/rpc-providers.json
```

`id` 是请求使用的稳定标识，须唯一，由 1–64 个 ASCII 字母、数字、`-` 或 `_` 组成；`name` 是浏览器下拉框显示的名称，须非空、不含控制字符且不超过 128 个字符。`endpoint` 须为有主机名的 HTTP(S) URL，不含空白、控制字符或 fragment。配置文件保留在服务器上；浏览器只取得 `id`、`name`，不会取得 endpoint 或其中的凭据。配置在启动时读取，更新后重启服务生效；无法读取或无效的配置会使启动失败。省略 `--rpc-config` 或配置空 `providers` 时，RPC 表单提示未配置并禁止提交。

若 RPC 报错显示 `eth_chainId` 收到 HTTP `502`，表示后端在取得链 ID 时收到了网关错误，此时尚未开始 CFG 或合约分析。先检查配置的 endpoint 服务及后端网络、代理连接；公网 RPC 的这类错误不需要通过修改前端地址输入来修复，也不能仅凭 `502` 判断一定是代理导致。

后端的 reqwest 客户端继承服务进程的环境代理设置。访问本地或内网 RPC 时，将对应主机加入 `NO_PROXY` / `no_proxy`；下面的 loopback 例子合并并保留两个变量原有的名单：

```bash
rpc_no_proxy="127.0.0.1,localhost,::1${NO_PROXY:+,$NO_PROXY}${no_proxy:+,$no_proxy}"
NO_PROXY="$rpc_no_proxy" no_proxy="$rpc_no_proxy" \
nix run .#web -- --rpc-config /absolute/path/to/rpc-providers.json
```

内网 RPC 同样加入其实际主机名或 IP。更改配置或代理环境后重启服务，再重试分析。

egui 通过 **wgpu / WebGPU** 绘制到 HTML canvas。Canvas 是页面上的绘图区域，WebGPU 是使用的渲染 API，两者并不冲突。浏览器必须提供 WebGPU 适配器，并从 `localhost`、`127.0.0.1` 或 HTTPS 安全上下文访问；普通 HTTP 局域网地址不满足要求。启动失败会显示错误，当前配置不回退到 WebGL。

字体随 Wasm 打包，通过 egui 的字体列表查找字形，不使用浏览器 CSS 字体回退。比例字体保留原有正文和 emoji 优先级，并追加已有等宽字体，补齐浮窗箭头 `→`、状态圆点 `●` 等符号。

单独构建可分发的静态资源：

```bash
nix build .#web-assets
```

`result/` 包含 HTML、JavaScript 和 Wasm。它仍需要连接同源原生分析服务；单独放到只提供静态文件的服务器上不能执行分析。

## 使用控件

通过 **New analysis** 打开弹窗表单，选择字节码或 RPC 链上账户输入。规则、调用环境、区块选择和预算集中在可滚动表单中；提交后收起输入，让主工作区留给结果。未指定 calldata 表示内容和长度未知，显式空字节串才表示空调用；value 和 caller 也区分未知与指定值。RPC 的区块参数默认 `latest`，每次任务只解析一次。

RPC 表单在 **RPC provider** 下拉框中显示后端配置的名称，默认选择第一项，再填写 **Root account**。名单加载中、加载失败或为空时，**Analyze** 和 **Ctrl+Enter** 都不会提交 RPC 任务；失败或为空时可点击 **Retry** 重新获取名单。切回 **Bytecode** 仍能分析字节码。

**Execution budget**、**Precision and solver**、**RPC acquisition budget** 保留全部预算与精度设置。Web 使用下表中的较大默认值；可以在十进制文本框中继续调大，前端与后端都不另设业务最大值。输入、请求与报告保留原始整数，超过 `2^53` 的值也不会经过浮点数舍入。

| 参数 | Web 默认值 |
| --- | --- |
| States / `max_states` | 100,000 |
| Transfers / `max_transfers` | 10,000,000 |
| Logical work / `max_work` | 1,000,000,000,000 |
| Call depth / `max_call_depth` | 1,025 |
| Memory bytes / `max_memory_bytes` | 67,108,864（64 MiB） |
| Jump context depth / `context_depth` | 0 |
| Constants per value / `max_constants` | 512 |
| Reduction rounds / `reduction_rounds` | 16 |
| Scalar facts / `max_facts` | 4,096 |
| Constraints / `max_constraints` | 2,048 |
| Expression nodes / `max_expression_nodes` | 16,384 |
| Expression depth / `max_expression_depth` | 256 |
| Resource limit / `smt_rlimit` | 10,000,000 |
| Accounts / `rpc_max_accounts` | 4,096 |
| Requests / `rpc_max_requests` | 1,000,000 |
| Response bytes / `rpc_max_response_bytes` | 67,108,864（64 MiB） |
| Timeout (ms) / `rpc_timeout_ms` | 120,000（120 秒） |

这些仍是用户指定的有限预算，达到执行、精度或采集边界时仍会停止相应工作并报告原因，不会自动改为无限。除 `context_depth` 可以为 0（不保留内部跳转历史）外，表中参数都必须是正整数。SMT 默认使用 Z3，额度计量的是所选求解器的资源单位，不是毫秒；默认启用关系推理、Product 数值域和完整被调用者摘要复用。RPC 的连接与整个请求（包括响应体）都遵循配置的 timeout，不再叠加固定 5 秒连接上限；账户、响应大小及初始槽位也不再有独立的 4096 / 64 MiB / 1024 硬上限。

数值仍受类型可表示范围约束：`smt_rlimit` 为 `u32`，最大 4,294,967,295；其余表中字段为 `u64`，最大 18,446,744,073,709,551,615。后端还检查需要转换为原生 `usize` 的字段是否能在运行平台表示。超出类型范围的输入不会被夹紧或近似接受。CLI 的默认参数保持原样；本表只描述 Web 默认值。

任务运行期间保留上一份完整结果，并显示新任务的状态与离散进度；新任务成功后才原子替换结果。输入草稿的编辑不会改变正在查看的报告。取消、失败或迟到的旧请求也不会把旧报告冒充新任务结果。

- **Workspace / 0** 根据可用宽度切换布局：宽窗口并排显示三个视图，中等窗口将反汇编与 SSA 放入标签组并与 CFG 并排，窄窗口将三个视图放入标签组。代码面板初始按实际内容定宽，剩余空间给 CFG；长内容受可用空间限制，仍可横向滚动。拖动分隔线后保留该档布局的手动宽度，窗口空间不足时临时夹紧，恢复空间后还原。**1 / 2 / 3** 分别展开反汇编、CFG 和 SSA；文本框获得焦点时数字键仍用于输入。
- 点击反汇编指令、SSA 指令行、SSA 块标题或 CFG 节点，在共享状态与 PC 上联动选择。不同调用帧和上下文保持独立；按实际程序身份切换完整源代码，也能区分根代码委托和 storage 所属账户。
- 反汇编只显示 PC 和指令／立即数。悬停指令可查看栈规格与解释，例如 `PUSH1: 0 → 1`、`ADD: 2 → 1`、`POP: 1 → 0`、`DUP1: 1 → 2`、`SWAP1: 2 → 2`。数字是正常执行所需的栈顶窗口与执行后的窗口大小，不是整栈高度；DUP 保留原值，SWAP 只交换位置。无效指令和调用尚未返回的情况在浮窗中另作说明。
- CFG 在新分析结果首次显示时排布并适配一次，初始缩放最高 100%。之后改变窗口、分栏或视图，只改变可见范围，保留节点排列、缩放和相对绘图区的平移位置。拖动、双指滚动或鼠标滚轮都用于平移，支持横纵两个方向；捏合或 **Ctrl/Cmd + 滚轮** 缩放并保持指针下的图中位置。**Fit graph**、**F / Shift+F** 或双击背景会把现有节点、边线及标签整体适配并居中，不重新排列节点，也不重新开启持续自动适配。快捷键在 CFG 可见且未编辑文本时生效，字节码中的 `f` / `F` 正常输入，**Ctrl/Cmd+F** 等组合键不触发适配。指针在节点上也能平移和缩放，其他代码面板的滚动独立处理。
- CFG 工具栏的 **Disasm / SSA** 切换节点内容，默认 SSA，选择会保留到后续分析。SSA 预览显示 phi、`%` 定义和参数、μ 效果及未完成阶段；较长的块省略中间行并保留末条指令，完整数据可悬停或在 SSA 面板查看。主动切换内容时会重新测量卡片并更新布局，保留当前缩放及附近节点在绘图区内的位置；需要完整显示时可按 **F**。
- 边标签作为布局层的虚拟节点显示，例如 `A → e0 true → B`。标签以紧凑的灰色小字显示，不绘制彩色外框，边类型颜色保留在线路上。标签拥有独立空间，两段连线连接其边界，箭头仍指向原目标状态。虚拟标签不计入 EVM 状态数，也不改变 CFG/SSA 的原始边 ID；它们与状态节点一起参与首次排布和 Fit，避免文字被节点覆盖。
- SSA 值名统一使用 `%0`、`%1`，与 CLI 一致。定义值、操作数、phi 前驱和 effect 分开着色；普通指令与 effect 使用紧凑单行，部分执行阶段仍显式显示，完整信息可悬停查看。长立即数和 phi 输入可横向滚动。
- 可收合的账户／程序目录列出取得的代码与地址；程序选择与状态选择分别保留，尚未执行的源码也可以查看。
- 底部检查器按所选状态、entry/exit 和调用帧显示上下文、stack、memory、storage、transient storage、calldata、returndata。稀疏字节与槽位保留默认值、未知信息和精度，不把未观察位置一律画成零。
- Outcomes 保留每个返回／回滚结果的字节和事务状态；Acquisition 显示固定快照、采集轮次、账户／槽位与类型化失败证据。诊断和未展开前沿仍可跳转到对应状态与指令。

分栏、分隔线和标签由 [`egui_tiles`](https://docs.rs/egui_tiles/0.17.1/egui_tiles/) 管理；反汇编与 SSA 使用 [`egui_extras::TableBuilder`](https://docs.rs/egui_extras/0.36.2/egui_extras/struct.TableBuilder.html) 管理列、滚动和可见行。字节码编辑、按钮和菜单使用 egui 通用控件，语义着色及 CFG 节点和边保留专用绘制。

## 布局与空间审查

布局按可用宽度选择：1120 点起为三栏，720–1119 点为代码标签组与 CFG 两栏，720 点以下为三个标签页。采用 egui 的逻辑点，不按固定屏幕像素拉伸文字；每档布局保存自己的手动宽度和活动标签，反汇编和 SSA 使用稳定的控件身份，在布局之间保留横纵滚动位置。尚未手调的代码面板随内容更新自然宽度；宽屏时单个自动代码面板最多占工作区可用宽度的 28%，尽量给 CFG 保留至少 40%（最低 320 点）。长代码可以横向滚动，避免 PUSH 地址和长 SSA 把图挤成细栏；手动宽度偏好仍保留。

| 区域 | 原来的问题 | 当前处理 |
| --- | --- | --- |
| 顶部导航与输入 | 额外的上下留白、独占一行的说明；窄窗口导航换行后可能被遮挡 | 6×2 点边距，窄窗口使用视图菜单；示例和限额收进菜单，说明保留在悬停详情 |
| 输入和诊断详情 | 长字节码、长错误或大量诊断可能挤占图表区域 | 编辑器及详情独立滚动并按视口高度限高；默认文字大小保持不变 |
| 工作区 | 代码面板初始按固定比例占用空间，短内容仍有大片空白 | 根据实际字体和内容测量代码宽度，CFG 使用余量；保留分隔条、手动宽度和响应式标签组 |
| 视图标题 | 每个标题固定 47 点高 | 24 点单行标题，完整说明可悬停查看；SSA 优先显示完整或部分覆盖状态 |
| 反汇编 | 28 点行高和固定列位置；部分面板必须先横向滚动才能看到栈列 | 22 点虚拟表格行，按内容分配 PC 和 opcode/operand；栈规格及解释放在指令悬停详情 |
| SSA | 每条普通指令固定 46 点双行，块标题 42 点，内容至少占 480 点宽 | 22 点虚拟表格行，指令与 effect 同排；按实际内容宽度滚动，`%` 值名和不完整阶段仍明确显示 |
| CFG 节点与布局 | 固定节点尺寸留下空白，浮动边标签可能落在节点下面 | 按内容测量状态卡片，将边标签作为独立虚拟节点参与初次分层和避让；真实状态 ID 不变 |
| CFG 视口 | 窗口或分栏变化会自动重排、缩放，打断正在查看的位置 | 新结果首次排布并适配，之后固定布局和视角；用户可平移、缩放或主动 Fit，Fit 只适配现有场景 |
| CFG 边线 | 折线落在不同亚像素位置，横竖边覆盖率不一致；浮动标签遮挡线路或被状态卡片覆盖 | 按设备像素比对齐折线路径，线宽不随图缩放变细；连线在标签节点边界断开并继续，文字留在预留矩形内 |

验证覆盖桌面宽屏、普通笔记本、中等宽度、390 点窄窗口以及横向短窗口；在同一个运行中的页面连续改变尺寸，不重新分析来重置状态。检查代码面板的自然宽度与手动偏好、CFG 初次适配及后续布局稳定性、显式 Fit、长 phi/PUSH32 横向访问、长输入和大量诊断。窗口缩小时图可能超出可见范围，可主动平移、缩放或 Fit；代码长内容仍可滚动。

触控板遵循浏览器输入语义：普通 wheel 事件的两轴位移用于平移，浏览器捏合转换成独立缩放事件。实现沿用 [eframe 的 wheel/gesture 接入](https://github.com/emilk/egui/blob/0.36.2/crates/eframe/src/web/events.rs) 和 [egui 的滚动/缩放增量](https://docs.rs/egui/0.36.2/egui/struct.InputState.html#method.zoom_delta)。原生回归覆盖指针在背景、节点及邻接代码面板时的行为；浏览器检查在 Chromium 中覆盖 DPR 1/2、真实 wheel 与 Ctrl+wheel、捏合格式事件，以及 Safari 格式的 gesture 事件。这些是 Linux 浏览器自动化与输入协议验证，不表示已经连接 macOS 实机触控板测试。

## 数据边界

三个 crate 分别承担以下职责：

| crate | 职责与依赖方向 |
| --- | --- |
| [`evm-abstract-protocol`](../crates/evm-abstract-protocol) | 请求、响应、枚举及数据 ID；只依赖 Serde，不依赖 egui 或执行引擎 |
| [`evm-abstract-server`](../crates/evm-abstract-server) | 将协议请求转换为引擎输入，验证 SSA，并投影为结构化响应；提供 HTTP 和静态资源 |
| [`evm-abstract-web`](../crates/evm-abstract-web) | 依赖共享协议；通过 Web API 获取结果，用 egui Painter 绘制各视图 |

共享协议版本为 4。传输使用 JSON，但请求、结果、进度、任务状态和错误都使用具体 Rust 结构与枚举；不使用无类型 JSON 树，也不解析 CLI、DOT 或 SSA 文本。`ApiError.details` 保留具体错误类别与参数，`message` 只补充人类可读说明。

| 请求 | 返回 |
| --- | --- |
| `GET /api/rpc-providers` | `RpcProvidersReply`：`{"result":{"Ok":[{"id":"mainnet","name":"Ethereum mainnet"}]}}`；无配置时列表为空 |
| `POST /api/tasks`，body 为 `AnalyzeRequest` | `202` 与 `JobReply`，包含任务 ID |
| `GET /api/tasks/{id}` | 小型 `JobSnapshot`：状态、阶段和累计计数 |
| `GET /api/tasks/{id}/result` | 完成后的 `AnalyzeReply`；未完成返回类型化 `TaskNotReady` |
| `DELETE /api/tasks/{id}` | 请求取消后的状态，仍通过状态接口确认终态 |

RPC 输入使用 `provider_id`，不接受浏览器提供的 `endpoint`。例如 `AnalyzeRequest.input` 为 `{"Rpc":{"provider_id":"mainnet","address":"0x0000000000000000000000000000000000000101","block":"Latest","accounts":[]}}`；其余 `fork`、`environment`、`limits` 字段仍按共享请求类型提供。后端在任务入队前按 ID 查找配置，未知 ID 返回 `InvalidRequest`。此变更属于 Web 共享协议，CLI 的显式 `--rpc` 参数保持其原有用法。

版本 4 将 `AnalysisLimits` 中除 `smt_rlimit: u32` 外的数值统一为 `u64`，避免 Wasm 的 32 位 `usize` 限制后端预算。累计 work / transfers 遥测也使用 `u64`。浏览器在 Rust 中直接处理 JSON 文本，保留整数精度；其他客户端处理这些数字时也应使用可精确保留 64 位整数的解析方式。报告的 `metadata.limits` 保存实际提交的完整策略，不用默认值覆盖用户输入。

报告包含代码目录、完整帧上下文、有效环境、快照身份、账户事实、入口／出口状态和终结结果。不可变 byte-array 与 store 使用报告内索引复用，每个 byte-array 的完整抽象字节值也用局部字典复用；符号表达式用扁平 DAG 表达，保留结构并避免 JSON 嵌套深度随表达式深度增长。状态轮询不重复下载整份报告。

反汇编保留 PC、opcode、立即数与基本块边界；CFG 保留原生状态及边 ID；SSA 保留值定义、操作数、phi 的前驱边、effect 引用和指令执行阶段。U256 立即数使用十六进制字符串，避免 JavaScript 数字精度损失。图和 SSA 共用原生状态 ID，同一个 PC 的不同上下文不会被合并为一个节点。

## 如何理解分析结果

字节码输入标为 **SingleProgram**；没有显式提供的环境和持久状态仍是未知。RPC 输入标为 **RpcWorld**，固定 chain ID 与 block hash 后按需补查具体调用目标和有限 storage 槽位。区块环境默认来自同一快照；显式覆盖与原始快照观测分别保留，交易输入不会被猜测为空调用。离线 world 文件仍可使用[CLI](../README.md#命令与输出格式)。

CLI 的 world/RPC SSA 与 Web 共享核心 `ssa::build_world` 和 `ssa::build_partial_world`。Web 对收敛图先执行完整验证，再用同一部分图表达结构生成统一展示数据；原生状态、边和值 ID 均保留。

`Converged` 表示配置范围内的抽象工作表闭合，不表示程序安全，也不表示每条抽象边都可实际执行。`Incomplete` 表示仍有显式前沿。后端保留诊断和前沿的位置与原因，并使用经过验证的部分 SSA 表达已观察到的执行：

- `Unexecuted` 和 `Stale` 状态不展示为当前有效的指令执行体。
- 待处理指令保留实际执行阶段，不生成尚未观察到的结果值。
- phi 和 effect 的输入只引用有执行证据的边；未支持的边保留原因。

这与[部分 SSA 教程](04-ssa.md)和[模型边界](06-boundaries.md)使用同一套语义。

## 任务执行与取消

CPU 分析由通道调度到有限原生线程池，每项分析在一个工作线程上完成。默认工作线程数为 `max(1, available_parallelism / 2)`；`--workers` 可覆盖，`--queue-capacity` 控制额外排队任务，`--retained-jobs` 控制保留多少终态结果。HTTP 使用独立的有界 I/O 线程，状态查询和静态资源不等待分析结束。Tokio 的单线程 reactor 只负责 RPC 网络等待，CPU 分析不会提交给 Tokio executor。

状态为 `Queued → Running → Completed/Failed`。取消排队任务会直接释放其请求；运行中的任务先进入 `Cancelling`，然后停止后续工作并丢弃结果，确认工作线程释放任务资源后才进入 `Cancelled`。RPC 请求／响应 future 会被丢弃并关闭 reactor；Z3 与 Bitwuzla 接入原生中断，执行预算检查点、SSA 构建和报告投影也检查取消。cvc5、预编译或系统 DNS 等无法主动中断的当前调用可能需要先返回，这段时间仍显示 `Cancelling`，不声称固定毫秒内停止。每个工作线程由只等待退出的监督线程持有，panic 和停机都在 join 完成、线程本地原生上下文析构之后确认；监视原生中断的辅助线程也只等待信号，不执行第二份分析。

进度显示实际阶段和累计 work、transfer、状态、账户、槽位、请求计数，不把开放工作表伪装为百分比。结束时以最终报告校正可丢弃的中间进度事件。初始采集失败是类型化失败；后续补查失败仍可产生带 RPC 证据的 `Incomplete` 报告。工作线程异常退出有明确错误状态，调度器补充工作线程继续服务。

## 构建与检查

Rust、Wasm target、wasm-bindgen 和浏览器验证工具由 Nix 管理；依赖由 `Cargo.lock` 和 `flake.lock` 固定。浏览器包不引入桌面窗口依赖。

修改前端后可在开发环境中直接重建：

```bash
nix develop
cargo build --locked --release --target-dir target -p evm-abstract-web --target wasm32-unknown-unknown
wasm-bindgen --target web --out-name evm_abstract_web --out-dir dist target/wasm32-unknown-unknown/release/evm_abstract_web.wasm
cp crates/evm-abstract-web/index.html dist/index.html
cargo run --locked -p evm-abstract-server -- --assets dist
```

服务默认监听 `127.0.0.1:8080`；需要其他本地端口时传入 `--bind 127.0.0.1:8081`。`nix run .#web -- --bind 127.0.0.1:8081` 也可指定端口。

本地开发服务同样支持 `cargo run --locked -p evm-abstract-server -- --assets dist --rpc-config /absolute/path/to/rpc-providers.json`。

完整验证仍运行[本地门禁](local-ci.md)。原有 native workspace 检查保留，另对 Wasm 目标执行构建、Clippy 和 Dylint。动态派发仅对 `evm_abstract_web::framework` 中的 egui 文本编辑、egui_tiles 布局行为与 JavaScript 适配开放，控件、共享协议和原生分析服务继续受 `no_dyn` 约束，见[检查规则](no-dynamic-dispatch.md)。

浏览器回归使用 Chromium 的 SwiftShader 软件 WebGPU 适配器，检查实际 WebGPU 上下文及绘制调用，同时覆盖页面分析、DPR 1/2 和图交互。软件适配器验证渲染路径，不代表硬件 GPU 加速性能。

RPC 回归先启动本地 JSON-RPC fixtures，再用真实配置文件启动分析服务，覆盖提供者名单、第二项选择与 `provider_id` 提交、跨合约结果和取消等待中的 RPC。另检查无配置时字节码可用、RPC 禁止提交，以及名单加载失败后的重试。

预算回归在独立页面操作真实输入控件，填写超过旧上限及 `2^53` 的整数，再分析小字节码，核对原始 POST 和报告 `metadata.limits` 精确一致；同时检查 Web 默认值与协议版本。

实现参考：[eframe WebRunner](https://docs.rs/eframe/0.36.2/wasm32-unknown-unknown/eframe/web/struct.WebRunner.html)、[egui Painter](https://docs.rs/egui/0.36.2/egui/struct.Painter.html)。
