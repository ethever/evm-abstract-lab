# egui Web 工作台

工作台在浏览器中绘制反汇编、SSA 和 CFG。输入是普通 EVM 运行时字节码，可以选择 Cancun、Prague 或 Osaka 规则。分析仍由原生 Rust 后端完成，浏览器不加载原生 SMT 库。

## 启动

从仓库根目录运行：

```bash
nix run .#web
```

Nix 构建 WebAssembly、匹配版本的 wasm-bindgen JavaScript 绑定和原生服务，并将静态资源与 `/api/analyze` 放在同一来源下。浏览器打开服务打印的地址即可使用。

单独构建可分发的静态资源：

```bash
nix build .#web-assets
```

`result/` 包含 HTML、JavaScript 和 Wasm。它仍需要连接同源原生分析服务；单独放到只提供静态文件的服务器上不能执行分析。

## 使用控件

页面首次加载会分析内置分支示例。可以粘贴十六进制字节码，选择规则版本，调整分析限额，再点 **Analyze** 或按 **Ctrl+Enter**。每次提交都会清空旧图，错误信息会出现在页面底部。

- **Workspace / 0** 根据可用宽度切换布局：宽窗口并排显示三个视图，中等窗口将反汇编与 SSA 放入标签组并与 CFG 并排，窄窗口将三个视图放入标签组。分栏宽度按比例随窗口变化，也可以拖动分隔线调整。**1 / 2 / 3** 分别展开反汇编、CFG 和 SSA；文本框获得焦点时数字键仍用于输入。
- 点击反汇编指令、SSA 指令行、SSA 块标题或 CFG 节点，在共享状态与 PC 上联动选择。不同调用帧和上下文保持独立；选择子帧时显示该帧的指令。
- CFG 的自动适配随视口尺寸重新布局和缩放；横向短窗口中，必要时改为从左到右排列，避免在有横向空间时仍缩小文字。拖动、双指滚动或鼠标滚轮都用于平移，支持横纵两个方向；捏合或 **Ctrl/Cmd + 滚轮** 缩放并保持指针下的图中位置。手动操作后保留视角；**Fit graph** 恢复自动适配。指针在节点上也能平移和缩放，其他代码面板的滚动独立处理。
- CFG 工具栏的 **Disasm / SSA** 切换节点内容，默认 Disasm，选择会保留到后续分析。SSA 预览显示 phi、`%` 定义和参数、μ 效果及未完成阶段；较长的块省略中间行并保留末条指令，完整数据可悬停或在 SSA 面板查看。切换会重新测量节点，自动视角重新适配，手动视角保留缩放及附近节点的位置。
- SSA 值名统一使用 `%0`、`%1`，与 CLI 一致。定义值、操作数、phi 前驱和 effect 分开着色；普通指令与 effect 使用紧凑单行，部分执行阶段仍显式显示，完整信息可悬停查看。长立即数和 phi 输入可横向滚动。
- 底部显示收敛状态、部分 SSA、诊断和未展开前沿。展开诊断列表后可跳转到对应状态与指令。

分栏、分隔线和标签由 [`egui_tiles`](https://docs.rs/egui_tiles/0.17.1/egui_tiles/) 管理；反汇编与 SSA 使用 [`egui_extras::TableBuilder`](https://docs.rs/egui_extras/0.36.2/egui_extras/struct.TableBuilder.html) 管理列、滚动和可见行。字节码编辑、按钮和菜单使用 egui 通用控件，语义着色及 CFG 节点和边保留专用绘制。

## 布局与空间审查

布局按可用宽度选择：1120 点起为三栏，720–1119 点为代码标签组与 CFG 两栏，720 点以下为三个标签页。采用 egui 的逻辑点，不按固定屏幕像素拉伸文字；每档布局保存自己的分隔比例和活动标签，反汇编和 SSA 使用稳定的控件身份，在布局之间保留横纵滚动位置。

| 区域 | 原来的问题 | 当前处理 |
| --- | --- | --- |
| 顶部导航与输入 | 额外的上下留白、独占一行的说明；窄窗口导航换行后可能被遮挡 | 6×2 点边距，窄窗口使用视图菜单；示例和限额收进菜单，说明保留在悬停详情 |
| 输入和诊断详情 | 长字节码、长错误或大量诊断可能挤占图表区域 | 编辑器及详情独立滚动并按视口高度限高；默认文字大小保持不变 |
| 工作区 | 固定左右面板宽度；窄屏直接退回 CFG，切换回来无法按比例利用空间 | egui_tiles 的相对份额、分隔条和标签组；连续缩放时自动切换布局 |
| 视图标题 | 每个标题固定 47 点高 | 24 点单行标题，完整说明可悬停查看；SSA 优先显示完整或部分覆盖状态 |
| 反汇编 | 28 点行高和固定列位置；部分面板必须先横向滚动才能看到栈列 | 22 点虚拟表格行，按内容分配 PC、opcode/operand 和栈列 |
| SSA | 每条普通指令固定 46 点双行，块标题 42 点，内容至少占 480 点宽 | 22 点虚拟表格行，指令与 effect 同排；按实际内容宽度滚动，`%` 值名和不完整阶段仍明确显示 |
| CFG 节点与布局 | 252×148 点固定节点、296×216 点固定步距；少量指令也留下大块空白 | 按实际文字和预览指令数计算节点大小，节点间距 22 点、纵向层间距 30 点；横向层间距为边标签预留宽度 |
| CFG 视口 | 初次适配后不响应窗口尺寸变化；底部说明遮挡节点 | 自动模式按新视口重新排列和缩放，计入边与标签边界；操作说明放在工具栏悬停提示 |
| CFG 边线 | 折线落在不同亚像素位置，横竖边覆盖率不一致；标签背景遮挡水平段 | 按设备像素比对齐屏幕中的折线路径，线宽不随图缩放变细；用实际标签尺寸预留空隙，避免标签遮住自身边线 |

验证覆盖桌面宽屏、普通笔记本、中等宽度、390 点窄窗口以及横向短窗口；在同一个运行中的页面连续改变尺寸，不重新分析来重置状态。还覆盖手动拖动分隔线、图的自动/手动视角、长 phi/PUSH32 横向访问、长输入和大量诊断。完整数据不以缩小到难以阅读的字号来塞入视口，长内容仍可滚动。

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

实现参考：[eframe WebRunner](https://docs.rs/eframe/0.36.2/wasm32-unknown-unknown/eframe/web/struct.WebRunner.html)、[egui Painter](https://docs.rs/egui/0.36.2/egui/struct.Painter.html)。
