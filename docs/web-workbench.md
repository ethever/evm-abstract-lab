# egui Web 工作台

工作台在浏览器中绘制反汇编、SSA 和 CFG。输入是普通 EVM 运行时字节码，可以选择 Cancun、Prague 或 Osaka 规则。分析仍由原生 Rust 后端完成，浏览器不加载原生 SMT 库。

## 启动

从仓库根目录运行：

```bash
nix run .#web
```

Nix 构建 WebAssembly、匹配版本的 wasm-bindgen JavaScript 绑定和原生服务，并将静态资源与 `/api/analyze` 放在同一来源下。浏览器打开服务打印的地址即可使用。

egui 通过 **wgpu / WebGPU** 绘制到 HTML canvas。Canvas 是页面上的绘图区域，WebGPU 是使用的渲染 API，两者并不冲突。浏览器必须提供 WebGPU 适配器，并从 `localhost`、`127.0.0.1` 或 HTTPS 安全上下文访问；普通 HTTP 局域网地址不满足要求。启动失败会显示错误，当前配置不回退到 WebGL。

字体随 Wasm 打包，通过 egui 的字体列表查找字形，不使用浏览器 CSS 字体回退。比例字体保留原有正文和 emoji 优先级，并追加已有等宽字体，补齐浮窗箭头 `→`、状态圆点 `●` 等符号。

单独构建可分发的静态资源：

```bash
nix build .#web-assets
```

`result/` 包含 HTML、JavaScript 和 Wasm。它仍需要连接同源原生分析服务；单独放到只提供静态文件的服务器上不能执行分析。

## 使用控件

页面首次加载会分析内置分支示例。可以粘贴十六进制字节码，选择规则版本，调整分析限额，再点 **Analyze** 或按 **Ctrl+Enter**。每次提交都会清空旧图，错误信息会出现在页面底部。

- **Workspace / 0** 根据可用宽度切换布局：宽窗口并排显示三个视图，中等窗口将反汇编与 SSA 放入标签组并与 CFG 并排，窄窗口将三个视图放入标签组。代码面板初始按实际内容定宽，剩余空间给 CFG；长内容受可用空间限制，仍可横向滚动。拖动分隔线后保留该档布局的手动宽度，窗口空间不足时临时夹紧，恢复空间后还原。**1 / 2 / 3** 分别展开反汇编、CFG 和 SSA；文本框获得焦点时数字键仍用于输入。
- 点击反汇编指令、SSA 指令行、SSA 块标题或 CFG 节点，在共享状态与 PC 上联动选择。不同调用帧和上下文保持独立；选择子帧时显示该帧的指令。
- 反汇编只显示 PC 和指令／立即数。悬停指令可查看栈规格与解释，例如 `PUSH1: 0 → 1`、`ADD: 2 → 1`、`POP: 1 → 0`、`DUP1: 1 → 2`、`SWAP1: 2 → 2`。数字是正常执行所需的栈顶窗口与执行后的窗口大小，不是整栈高度；DUP 保留原值，SWAP 只交换位置。无效指令和调用尚未返回的情况在浮窗中另作说明。
- CFG 在新分析结果首次显示时排布并适配一次，初始缩放最高 100%。之后改变窗口、分栏或视图，只改变可见范围，保留节点排列、缩放和相对绘图区的平移位置。拖动、双指滚动或鼠标滚轮都用于平移，支持横纵两个方向；捏合或 **Ctrl/Cmd + 滚轮** 缩放并保持指针下的图中位置。**Fit graph**、**F / Shift+F** 或双击背景会把现有节点、边线及标签整体适配并居中，不重新排列节点，也不重新开启持续自动适配。快捷键在 CFG 可见且未编辑文本时生效，字节码中的 `f` / `F` 正常输入，**Ctrl/Cmd+F** 等组合键不触发适配。指针在节点上也能平移和缩放，其他代码面板的滚动独立处理。
- CFG 工具栏的 **Disasm / SSA** 切换节点内容，默认 SSA，选择会保留到后续分析。SSA 预览显示 phi、`%` 定义和参数、μ 效果及未完成阶段；较长的块省略中间行并保留末条指令，完整数据可悬停或在 SSA 面板查看。主动切换内容时会重新测量卡片并更新布局，保留当前缩放及附近节点在绘图区内的位置；需要完整显示时可按 **F**。
- 边标签作为布局层的虚拟节点显示，例如 `A → e0 true → B`。标签拥有独立空间，两段连线连接其边界，箭头仍指向原目标状态。虚拟标签不计入 EVM 状态数，也不改变 CFG/SSA 的原始边 ID；它们与状态节点一起参与首次排布和 Fit，避免文字被节点覆盖。
- SSA 值名统一使用 `%0`、`%1`，与 CLI 一致。定义值、操作数、phi 前驱和 effect 分开着色；普通指令与 effect 使用紧凑单行，部分执行阶段仍显式显示，完整信息可悬停查看。长立即数和 phi 输入可横向滚动。
- 底部显示收敛状态、部分 SSA、诊断和未展开前沿。展开诊断列表后可跳转到对应状态与指令。

分栏、分隔线和标签由 [`egui_tiles`](https://docs.rs/egui_tiles/0.17.1/egui_tiles/) 管理；反汇编与 SSA 使用 [`egui_extras::TableBuilder`](https://docs.rs/egui_extras/0.36.2/egui_extras/struct.TableBuilder.html) 管理列、滚动和可见行。字节码编辑、按钮和菜单使用 egui 通用控件，语义着色及 CFG 节点和边保留专用绘制。

## 布局与空间审查

布局按可用宽度选择：1120 点起为三栏，720–1119 点为代码标签组与 CFG 两栏，720 点以下为三个标签页。采用 egui 的逻辑点，不按固定屏幕像素拉伸文字；每档布局保存自己的手动宽度和活动标签，反汇编和 SSA 使用稳定的控件身份，在布局之间保留横纵滚动位置。尚未手调的代码面板随内容更新自然宽度，放大窗口时多出的空间留给 CFG。

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

`POST /api/analyze` 接收 `AnalyzeRequest`，返回 `AnalyzeReply`。成功结果是 `AnalysisReport`，失败是带 `ApiErrorCode` 的 `ApiError`。传输使用 JSON，但两端均直接序列化或反序列化共享 Rust 类型，不使用无类型 JSON 树，也不解析 CLI、DOT 或 SSA 文本。

反汇编保留 PC、opcode、立即数与基本块边界；CFG 保留原生状态及边 ID；SSA 保留值定义、操作数、phi 的前驱边、effect 引用和指令执行阶段。U256 立即数使用十六进制字符串，避免 JavaScript 数字精度损失。图和 SSA 共用原生状态 ID，同一个 PC 的不同上下文不会被合并为一个节点。

## 如何理解分析结果

当前请求是 **SingleProgram**：一段运行时字节码，calldata、环境和持久状态未知。此入口不加载 world 文件或 RPC 快照；这些输入继续使用[CLI](../README.md#命令与输出格式)。

`Converged` 表示配置范围内的抽象工作表闭合，不表示程序安全，也不表示每条抽象边都可实际执行。`Incomplete` 表示仍有显式前沿。后端保留诊断和前沿的位置与原因，并使用经过验证的部分 SSA 表达已观察到的执行：

- `Unexecuted` 和 `Stale` 状态不展示为当前有效的指令执行体。
- 待处理指令保留实际执行阶段，不生成尚未观察到的结果值。
- phi 和 effect 的输入只引用有执行证据的边；未支持的边保留原因。

这与[部分 SSA 教程](04-ssa.md)和[模型边界](06-boundaries.md)使用同一套语义。

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

完整验证仍运行[本地门禁](local-ci.md)。原有 native workspace 检查保留，另对 Wasm 目标执行构建、Clippy 和 Dylint。动态派发仅对 `evm_abstract_web::framework` 中的 egui 文本编辑、egui_tiles 布局行为与 JavaScript 适配开放，控件、共享协议和原生分析服务继续受 `no_dyn` 约束，见[检查规则](no-dynamic-dispatch.md)。

浏览器回归使用 Chromium 的 SwiftShader 软件 WebGPU 适配器，检查实际 WebGPU 上下文及绘制调用，同时覆盖页面分析、DPR 1/2 和图交互。软件适配器验证渲染路径，不代表硬件 GPU 加速性能。

实现参考：[eframe WebRunner](https://docs.rs/eframe/0.36.2/wasm32-unknown-unknown/eframe/web/struct.WebRunner.html)、[egui Painter](https://docs.rs/egui/0.36.2/egui/struct.Painter.html)。
