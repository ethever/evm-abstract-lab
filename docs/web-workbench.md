# egui Web 工作台

工作台在浏览器中显示反汇编、SSA、跨合约 CFG 和机器状态。输入可以是普通 EVM 运行时字节码，也可以是后端配置的 RPC 提供者与链上地址；执行规则可选 Cancun、Prague 或 Osaka。原生 Rust 后端直接调用核心分析库，不启动 CLI 子进程，浏览器只负责请求、校验与展示。

## 启动

从仓库根目录运行：

```bash
nix run .#web
```

Nix 构建 WebAssembly、匹配版本的 wasm-bindgen JavaScript 绑定和原生服务，并将静态资源与 `/api/tasks` 放在同一来源下。浏览器打开服务打印的地址即可使用。首次打开时先读取 RPC 提供者目录：有提供者则使用第一项自动分析下面的 Ethereum 主网 USDC 地址；目录为空则运行原有的分支字节码示例。目录加载失败时仍可使用字节码入口，并可在 RPC 表单点击 **Retry** 后手动提交分析。

默认不配置 RPC 提供者，字节码分析可直接使用。需要分析链上账户时，直接传入 RPC HTTP(S) URL：

```bash
nix run .#web -- --bind 0.0.0.0:1111 --rpc-config http://127.0.0.1:8545
```

`--rpc-config <URL_OR_FILE>` 接受 RPC URL 或 JSON 配置文件路径。传入 URL 时，后端自动创建一个 ID 为 `default`、显示名称为 **Default RPC** 的提供者；配置保存在服务内存中，无需编写或生成 JSON 文件。浏览器的 **RPC provider** 显示名称和实际 URL，例如 **Default RPC (http://127.0.0.1:8545)**。这与手动配置一个提供者等价，分析请求仍只提交提供者 ID。

需要多个 RPC 提供者或自定义名称时，再准备 JSON 配置文件，例如 `rpc-providers.json`：

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

`id` 是请求使用的稳定标识，须唯一，由 1–64 个 ASCII 字母、数字、`-` 或 `_` 组成；`name` 是浏览器下拉框显示的名称，须非空、不含控制字符且不超过 128 个字符。直接传入的 URL 与配置文件中的 `endpoint` 使用相同规则：须为有主机名的 HTTP(S) URL，不含空白、控制字符或 fragment。配置文件保留在服务器上；浏览器取得 `id`、`name` 和完整 `endpoint` 值，用于显示配置的名称和地址。配置在启动时读取，更新后重启服务生效；无效 URL、无法读取的配置文件或无效 JSON 会分别报告对应错误并使启动失败。文件不存在时，错误会明确指出读取配置文件失败，并提示 `--rpc-config` 也可直接接受 HTTP(S) URL。省略 `--rpc-config` 或配置空 `providers` 时，RPC 表单提示未配置并禁止提交。

若 RPC 报错显示 `eth_chainId` 收到 HTTP `502`，表示后端在取得链 ID 时收到了网关错误，此时尚未开始 CFG 或合约分析。先检查配置的 endpoint 服务及后端网络、代理连接；公网 RPC 的这类错误不需要通过修改前端地址输入来修复，也不能仅凭 `502` 判断一定是代理导致。

后端的 Alloy RPC client 通过有界 `reqwest` 传输适配访问节点，并继承服务进程的环境代理设置。访问本地或内网 RPC 时，将对应主机加入 `NO_PROXY` / `no_proxy`；下面的 loopback 例子合并并保留两个变量原有的名单：

```bash
rpc_no_proxy="127.0.0.1,localhost,::1${NO_PROXY:+,$NO_PROXY}${no_proxy:+,$no_proxy}"
NO_PROXY="$rpc_no_proxy" no_proxy="$rpc_no_proxy" \
nix run .#web -- --rpc-config /absolute/path/to/rpc-providers.json
```

内网 RPC 同样加入其实际主机名或 IP。更改配置或代理环境后重启服务，再重试分析。

egui 通过 **wgpu / WebGPU** 绘制到 HTML canvas。Canvas 是页面上的绘图区域，WebGPU 是使用的渲染 API，两者并不冲突。浏览器必须提供 WebGPU 适配器，并从 `localhost`、`127.0.0.1` 或 HTTPS 安全上下文访问；普通 HTTP 局域网地址不满足要求。启动失败会显示错误，当前配置不回退到 WebGL。

字体随 Wasm 打包，通过 egui 的字体列表查找字形，不使用浏览器 CSS 字体回退。比例字体保留原有正文和 emoji 优先级，并追加已有等宽字体，补齐浮窗箭头 `→`、状态圆点 `●` 等符号；小型派生字体补齐上下标并保持正文的字形来源。状态、程序、块、帧、边和效果统一写作 `σᵢ`、`Pᵢ`、`Bᵢ`、`fᵢ`、`eᵢ`、`μᵢ`，使用 egui 原生小字号下标排版，`%编号` 保持原样。控件文本保留 Unicode，辅助阅读、复制和导出使用相同符号；实例搜索同时接受数字、`σ₁₂₈`、`sigma_128` 和旧输入 `S128`。完整记号与编号空间见[统一符号](notation.md)。

单独构建可分发的静态资源：

```bash
nix build .#web-assets
```

`result/` 包含 HTML、JavaScript 和 Wasm。它仍需要连接同源原生分析服务；单独放到只提供静态文件的服务器上不能执行分析。

## 使用控件

通过 **New analysis**（窄屏为 **Input**）打开表单，显式选择 **Address**（RPC 链上账户）或 **Bytecode**（粘贴运行时字节码）。规则、调用环境、区块选择和预算集中在可滚动表单中；提交后收起输入，让主工作区留给结果。calldata、value、caller 默认保留 **Unknown**，未指定输入不改成零值或空调用。显式选择 calldata 的 **Exact** 后，空文本或 `0x` 表示空调用；value 的 Exact 和 caller 的 Address 则必须填写有效值，空文本会提示校验错误。RPC 的区块参数默认 `latest`，每次任务只解析一次。

RPC 表单在 **RPC provider** 下拉框的当前选项与每个候选项中同时显示后端配置的名称和 RPC URL，默认选择第一项；**Root account** 预填 Ethereum 主网 USDC 地址。长名称或 URL 会按可用宽度省略显示，悬停可查看完整名称和地址。名单加载中、加载失败或为空时，**Analyze** 和 **Ctrl+Enter** 都不会提交 RPC 任务；失败或为空时可点击 **Retry** 重新获取名单。切回 **Bytecode** 仍能分析字节码。

入口示例下拉框提供 **USDC**（默认）、**WETH**、**USDT**、**Multicall3**、**Uniswap V2 Router02**、**Uniswap V2 Pair (USDC/WETH)**、**Uniswap V2 Factory**、**Uniswap V4 PoolManager** 与 **Custom**。八个预置地址均标记为 **Ethereum mainnet**，应搭配 Ethereum 主网 RPC：

| 示例 | Root account | 地址来源 |
| --- | --- | --- |
| USDC | `0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48` | [Circle 支持的 USDC 合约](https://help.circle.com/support/en/usdc-supported-blockchains-minting-redemption-faqs?id=kb_article_view&sysparm_article=KB0010590) |
| WETH | `0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2` | [ethereum.org 的 canonical WETH 实现](https://ethereum.org/wrapped-eth/) |
| USDT | `0xdAC17F958D2ee523a2206206994597C13D831ec7` | [Tether 支持的协议与合约地址](https://tether.to/en/supported-protocols/) |
| Multicall3 | `0xcA11bde05977b3631167028862bE2a173976CA11` | [Multicall3 官方部署清单，chainId=1](https://github.com/mds1/multicall3/blob/main/deployments.json) |
| Uniswap V2 Router02 | `0x7a250d5630B4cF539739dF2C5dAcb4c659F2488D` | [Uniswap V2 官方部署](https://developers.uniswap.org/docs/protocols/v2/deployments) |
| Uniswap V2 Pair (USDC/WETH) | `0xB4e16d0168e52d35CaCD2c6185b44281Ec28C9Dc` | 以主网 Factory、USDC、WETH 地址按官方 [UniswapV2Library.pairFor](https://github.com/Uniswap/v2-periphery/blob/master/contracts/libraries/UniswapV2Library.sol) 推导 |
| Uniswap V2 Factory | `0x5C69bEe701ef814a2B6a3EDD4B1652CB9cc5aA6f` | [Uniswap V2 官方部署](https://developers.uniswap.org/docs/protocols/v2/deployments) |
| Uniswap V4 PoolManager | `0x000000000004444c5dc75cB358380D2e3dE08A90` | [Uniswap V4 官方部署，Ethereum: 1](https://developers.uniswap.org/docs/protocols/v4/deployments) |

V2 Pair 选用具体的 USDC/WETH 池子，Router02 与 Factory 则分别是路由和创建池子的合约。V4 的[池子状态与执行逻辑集中在单例 PoolManager](https://developers.uniswap.org/docs/protocols/v4/concepts/poolmanager)，池子通过 PoolKey / PoolId 区分，因此 V4 示例以 PoolManager 作为 Root account。选择某个具体 V4 池子的调用还需在 calldata 中提供相应池子参数。

选择预置示例只把对应地址写入草稿；也可选择 **Custom** 手工编辑地址，重新打开表单会保留草稿。提供者、区块、环境和预算设置不变。切换下拉选项不会自动提交，点击 **Analyze** 或按 **Ctrl+Enter** 才开始新的分析。

**Execution budget**、**Precision**、**RPC acquisition budget** 保留全部预算与精度设置。CLI 的 `cfg`、`ssa`、`analyze`、`explain` 与 Web 共用下表中的较大默认值；可以在十进制文本框中继续调大，前端与后端都不另设业务最大值。输入、请求与报告保留原始整数，超过 `2^53` 的值也不会经过浮点数舍入。

| 参数 | CLI / Web 默认值 |
| --- | --- |
| States / `max_states` | 100,000 |
| Transfers / `max_transfers` | 10,000,000 |
| Logical work / `max_work` | 2,000,000,000 |
| Call depth / `max_call_depth` | 1,025 |
| Memory bytes / `max_memory_bytes` | 67,108,864（64 MiB） |
| Detailed code sources / `max_detail_regions`（analyze / explain / Web） | 1 |
| Jump context depth / `context_depth` | 128 |
| Numeric model / `numeric_model` | Product（可选 Finite set） |
| Constants per value / `max_constants` | 512 |
| Reduction rounds / `reduction_rounds` | 16 |
| Scalar facts / `max_facts` | 4,096 |
| Constraints / `max_constraints` | 2,048 |
| Expression nodes / `max_expression_nodes` | 16,384 |
| Expression depth / `max_expression_depth` | 256 |
| Accounts / `rpc_max_accounts` | 4,096 |
| Requests / `rpc_max_requests` | 1,000,000 |
| Response bytes / `rpc_max_response_bytes` | 67,108,864（64 MiB） |
| Timeout (ms) / `rpc_timeout_ms` | 120,000（120 秒） |

这些仍是用户指定的有限预算，达到执行、精度或采集边界时仍会停止相应工作并报告原因，不会自动改为无限。数值模型是枚举选择；`context_depth` 可以为 0（不保留内部跳转历史），`max_detail_regions` 可以为 0（只运行 Global 模型），其余数值参数必须为正整数。默认启用局部关系推理、Product 数值域和完整被调用者摘要复用。RPC 的连接与整个请求（包括响应体）都遵循配置的 timeout，不再叠加固定 5 秒连接上限；账户、响应大小及初始槽位也不再有独立的 4096 / 64 MiB / 1024 硬上限。

**Precision → Detailed code sources** 设置额外局部精度最多选择多少个已捕获的代码来源。默认 1；2 或 3 可以让更多已捕获的被调用者保留局部细节，这些来源继续共享 Global 之后的剩余执行预算。报告分别显示请求上限和实际选中的来源数量，达到预算时保留实际终止状态。省略 HTTP 字段时使用默认 1，Web 表单始终提交明确的有效值。

Web 协议的数值仍受类型可表示范围约束：表中字段均为 `u64`，最大 18,446,744,073,709,551,615。后端还检查需要转换为原生 `usize` 的字段是否能在运行平台表示。超出类型范围的输入不会被夹紧或近似接受。CLI 使用相同的默认配置，显式参数仍可覆盖；对应选项见[CLI 精度与预算表](05-sensitivity.md#7-实验时分别调整精度与预算)。直接使用 Rust 库的 `Config::default()`、`ExecutionConfig::default()`、`RelationLimits::default()` 和 `RpcInput::new()` 时，仍保留各类型原有的较小默认值；它们不会隐式采用 CLI/Web 配置。

当前 wire 请求必须提供 `limits.numeric_model: "Product"` 或 `"FiniteSet"`。后端只在应用入口选择一次实际数值代数，报告的 `metadata.limits` 保留这个选择；终止覆盖输入还携带独立的模型标签和实际 `domain_spec`。数值限制与执行预算分别检查，标签不代替来源或策略资格。

新任务运行期间保留上一份结果，并标记 **Previous result**。状态栏分别显示可复制的 **Current Job ID / Result Job ID**；任务与结果相同时显示 **Job ID**。收到当前任务通过校验的最小 Global 模型后即可切换，详细路径和富报告可以随后到达；这些补充保留仍有效的来源、上下文、视图和镜头。编辑输入草稿不会改变报告，迟到的旧任务或旧发布版本也不能替换当前结果。

**Model ready** 与 **Detail running / partial / ready / refused / cancelled** 分别说明已有模型和可选细节的状态；全局模型可用时，详细路径仍可能未完成。**Cancel** 请求取消当前任务，确认前保留任务状态；轮询连接中断会显示原因并重试，可用结果继续保留。表单校验失败可直接修正；任务错误可通过 **Edit inputs** 返回草稿，悬停错误可查看类型化详情。没有结果时工作区显示加载、运行或输入提示。

默认先支付并发布可用的最小全局模型，再把剩余执行预算用于详细路径。**Global model** 保留来源限定的 MAY 控制、调用与返回交付关系、数值输入和联合输出边界；**Detailed paths** 查看独立 Native 图、指令与 SSA。最小模型没有完整机器状态或 SSA 时也可导航和检查；富导出被拒绝或取消不撤销已有模型。符号根的详细视图仍明确显示额外根地址条件，当前前缀不表示整个区域已完成。任务完成、全局闭合和富报告导出分别显示，详见[全局模型与详细路径](20-global-and-local.md)。

**Execution budget** 保留默认勾选的 **Prepare conservative root terminal coverage**。这个可选辅助属于 native 阶段；未开始或预算不足时可没有记录，三个 tuple 不替代全局控制闭合。关闭完成调用缓存复用仍运行实际 Process 模型。[根终止覆盖](19-root-terminal-coverage.md)说明辅助 tuple 的联合数据/Store 与回滚资格。

## 分析耗时

非紧凑布局的状态栏在提交、排队、运行和获取结果时显示 **Elapsed**。这段时间从提交分析开始，直到当前结果的首次视图准备完成；失败或取消确认后也会冻结。完成后的 **Total wait** 是用户实际等待的时间，切换视图、缩放或查看其他状态不会重新计时。点击状态栏的时间按钮可打开检查器的 **Timing** 页；紧凑布局通过 **More → Timing** 查看完整分解。

后端的 **Backend processing** 从工作线程实际开始处理计到结束，**Queue** 单独记录等待工作线程的时间。阶段时间由后端的单调时钟直接累计，前端轮询延迟或丢弃的进度事件不会改变它。完成结果保留这些时间，开始新任务时，上一份结果继续保留自己的计时记录。

| 时间项 | 范围 |
| --- | --- |
| Validation | 后端验证与输入准备 |
| RPC | 固定链与区块、初始读取、全部补读账户和存储 |
| Abstract analysis | 全部分析轮次的抽象执行，包含局部关系推理 |
| SSA construction | 构建、规范化与验证 SSA |
| Report projection | 整理原生状态、值和效果，生成结构化展示报告 |
| Frontend preparation | 获取并解析报告、安装结果以及当前视图的首次准备 |

RPC 精化可能反复经历“抽象执行 → 补读数据 → 从入口重跑”。显示的 RPC 和抽象执行时间累计所有轮次，最终图只保留最后一轮也不会丢掉此前耗时。局部关系推理属于抽象执行内部工作，不重复加入总时间。

前端准备还可查看 **Result transfer**、**JSON parsing** 和 **First view setup**。首次准备只测实际打开的视图；不会为了统计而提前生成其他视图的 CFG 布局。用户总等待也包含提交和状态轮询等往返延迟，不能把后端处理、排队与前端准备当作覆盖全部等待的精确加总。这里统计实际耗时，`work` 仍表示逻辑工作量；区块 `Timestamp` 和 RPC timeout 保留各自原有含义。

## 阅读结果

新结果默认打开入口来源的 **CFG**，以可读比例定位入口附近。顶部 **Code / CFG / SSA / Compare** 选择阅读方式；**1 / 2 / 3 / 0** 分别切换 Code、CFG、SSA、Compare，编辑文本时数字键正常输入。Compare 按空间并排或用标签组合代码、图和可用的 SSA；它不是默认入口。

**Sources** 按“代码来源 → 上下文 → 块／状态／指令”导航。共享相同代码的来源仍保留各自的账户别名；代码账户、storage owner、caller、static 和根地址条件共同限定证据。选择来源即可读完整解码 **Code**，不要求存在执行上下文；查看 CFG 或 SSA 时再选择相容上下文。**Global model / Detailed paths** 打开对应模型的上下文选择器，选择经校验的确切条目后才切换；数字相同的状态或程序 ID 不会自动跨图对应。**All contexts** 扩大列表范围，包含未捕获代码或不属于当前来源的上下文，本身不改变当前选择。

沿图上的调用／边界目标，或 **Sources → Context calls** 中的调用条目，可进入被调用代码及其 owner 上下文。未知代码、未知跳转和范围外目标仍保留可检查的边界信息。**Back / Forward** 恢复先前的来源、上下文、Code／CFG／SSA 等视图与镜头；**Entry** 返回入口位置。紧凑布局的 Forward、Entry 位于 **More**。

**SSA** 查看选定相容 Native 上下文中实际保留的 SSA 记录，包含仅有边界效果的记录：没有捕获代码、`program=None` 或没有 opcode PC，仍可能有真实的 `Current / Stale` SSA；Code 的可用性独立判断。MAY 控制、只有源码、没有 SSA 记录和尚未执行的状态各有可用性说明，可通过 **Choose Native context** 继续选择。部分或较早输入条件下的 SSA 仍带覆盖和条件说明展示，不因此成为当前完整图，也不会把只有来源的选择补成 Native State 0。

反汇编只显示 PC 和指令／立即数，提供 **Copy code / Copy bytecode**。悬停指令可看栈规格，例如 `PUSH1: 0 → 1`、`ADD: 2 → 1`、`DUP1: 1 → 2`；数字表示正常执行所需的栈顶窗口，不是整栈高度。无效指令和未返回调用另有说明。SSA 使用共享核心的常量传播、折叠和基础代数简化，以 `%0`、`%1` 等值名显示指令，保留 φ、原 PC／opcode 来源、效果和未完成阶段；长立即数与 φ 输入可横向滚动。**Selected state / All source states** 选择当前状态或该来源的逐状态列表，**Copy SSA** 复制所选范围的完整详情，各状态仍保留独立上下文。点击值可在 **SSA value** 中查看原定义、规范化规则、数值事实及其资格，**Go to source** 返回匹配的状态和 PC。规范化规则见 [SSA 基础规范化与值事实](ssa-normalization.md)。

**SSA details** 同时控制 Native SSA 面板与 CFG 的 SSA 预览；紧凑布局在 SSA、Compare 或 Native CFG 的 SSA 预览中直接显示，其他视图从 **More** 打开。默认收起正常完成且无故障的 `DUP1…16`、`SWAP1…16`、`POP` 及普通效果链注释；打开后恢复完整行、效果链和 `Completed` 标记。值定义、调用、控制流、存储／内存操作、故障与未完成阶段仍可见；选中隐藏 PC 会临时显露对应行。该偏好跨视图、窗口与后续分析保留，切换只更新展示。

### CFG 范围与图层

默认 **SOURCE CFG** 按当前来源与上下文显示直接控制关系，并在真实调用点保留调用与交付边界。区域宏步的 `through`、跨块关系和来源收据可从 **Graph → Model relations** 或可选 **Model overview** 检查，不作为每个源码块的直接后继。没有捕获代码的保守上下文显示 **ABSTRACT CONTROL**，不虚构 opcode 或指令位置；**MODEL OVERVIEW** 则展示区域关系及独立上下文。

**Graph → Entry view** 默认显示当前锚点及全部真实的前向一跳备选，包括实际调用边界与已有交付的调用者续点，不沿入边扩展，也没有固定三点上限。**Nearby** 显式探索有限邻域；**Full source CFG** 显示当前所选上下文内已准入保留的全部来源点，包含断开、待展开和历史部分。**Model overview** 另行查看区域关系。

来源卡片标题显示字节范围，下一行单独给出末条指令及其 PC。例如 `Block 0x0000–0x000c` 与 `JUMPI @ 0x000c` 分别说明块范围和跳转指令的位置；正文是有限行解码预览，长块提示 **open Code**。owner／static 等限定集中在范围标题中。紧凑卡片可只显示 `Block 0x0000`，末条指令位置保留在详情行，完整指令可在 Code 中阅读。

图上显示窗口类型及已显示／总来源点数；总数是所选上下文已准入的保留点，不是全部 Native 已执行状态或字节码的全部静态块。**Receipts (N)** 选择该点的独立来源收据，**Targets →** 打开实际目标或解码候选；解码候选本身没有执行收据。未知跳转保留完整合法目标集，仅因窗口范围省略的入边／出边以不参与节点布局的边界元数据保留，通过目标入口、**Graph → Incoming links / Hidden control** 检查。原边种类、限定端点与来源，以及收据的 current／complete、输入条件和独立输出资格继续保留。

**Detailed paths** 中仍可使用 **Blocks / States / Local**：Blocks 将同一程序、同一基本块的可见 Native 实例合成导航卡片；States 保留逐状态卡片；Local 查看实际邻接状态，Hops 可调 1–4，默认 2。默认范围保留所选调用上下文，其他调用和范围外目标作为可导航边界；**Overview** 可展开跨调用范围，Local 总览最多显示 200 个邻接状态。非 Local 的程序下拉框保留 **All programs** 与单程序筛选，筛选不重新分析。

Blocks 卡片不会合成跨实例的 φ、值或 Store；**Instances** 打开实例列表，选择具体 `σᵢ` 后查看它的 SSA、栈和效果。聚合边可能来自不同实例，不能把拼接路径当作可行轨迹。**Disasm / SSA** 切换 Native 卡片预览；单实例可读实际 SSA，多实例保留成员摘要与解码代码。主动改变内容或投影可能更新布局，**SSA details** 与缩放级别切换则保留已测量几何。

Native 终止背景按实际证据区分正常结束（深绿）、REVERT（深紫）和异常（深红），混合、继续执行或证据不足时保持中性。**Root end / Frame return** 分别说明根结果和子帧返回，前沿或非当前覆盖另有琥珀色虚线。**End colors** 或缩放提示提供图例；颜色不从当前显示出度推断覆盖。空间分组、聚合、可见范围和缩放均不改变分析结果、预算或闭合状态，也不提供求解精度、安全性或性能保证。

### 检查器的观察范围

**Inspect** 按 **State / Data / Effects / Evidence / Timing** 分组选择详情。先确认观察对象，再读数值：

| 观察对象 | 内容与资格 |
| --- | --- |
| Original／**Original observations** | 原始环境及账户观察，独立于过程或 Native 状态；从 Sources 的根输入／Accounts 也可打开 |
| **Procedure input** | 当前限定过程的合并输入边界，不是某次 Native 指令前状态 |
| **Joint outcome** | 同一相对出口配对的数据与 Store，保留该出口资格；回滚由调用者应用 |
| **Native · before block / after block** | 所选 Native 抽象状态与调用帧的块边界快照；没有出口时明确显示不可用，历史出口保留标记 |
| Source only | 来源与解码信息，没有上下文时不补造机器状态 |

Native 中选择 PC 只联动指令位置，memory／storage 仍是块边界观察。检查器保留上下文、stack、memory、calldata、returndata、persistent／transient storage、balance、code、nonce 和账户效果；未知或未观察位置保留原默认值与精度。**Outcomes** 查看返回／回滚数据及事务状态，**Acquisition** 查看固定快照、采集与失败原因，**Coverage / Diagnostics** 保留前沿和可定位的状态／指令证据。

**Storage / Transient** 的账户、**Account facts in this store**、默认槽值与显式槽列表共用主要滚动区域，长列表只绘制可见行。槽键默认显示完整 U256，数值右对齐，可选择和复制完整文本；用户可按需调整列宽或启用保留后缀的前导省略。

**Inspect** 也支持 Windows/Linux 的 **Ctrl+J** 与 macOS 的 **⌘J**；工作台获得焦点时生效，弹窗与输入法组合输入优先，长按不重复开关。拖动检查器上边缘调整高度，**Maximize / Restore / Hide** 控制面板；隐藏后重开保留活动页、观察位置和高度偏好。调整面板不重新分析。

## 布局与空间审查

宽度低于 680 或高度低于 560 逻辑点时使用紧凑布局，新结果入口 CFG 的 **Sources / Inspect** 初始都关闭。二者一次只占用一个位于导航下方的锚定内容区；选择来源／上下文后收起 Sources，回到所选位置。**Input** 表单优先占用交互空间，暂时收起其他面板内容，阻止点击穿透。主导航保留 **Back** 与带限定的来源身份，**More** 收纳模型选择、Forward、Entry、Timing、Coverage、Diagnostics、Terminal coverage 及当前未直接显示的 SSA details；页脚保留当前／结果 Job ID、Copy 和模型／细节状态。桌面 Sources 可停靠侧栏，Inspect 可调整高度或最大化。

入口窗口在各设备上都采用真实前向一跳范围。首次卡片宽度与预览行数按导航、工具栏和页脚之后的实际图区选择，镜头围绕锚点及其直接备选定位；窗口外连接不靠增加独立节点拉大布局。普通 resize／Back 保留当前范围的布局偏好、几何与镜头，显式切换窗口范围可以重新布局。

入口／当前范围以可读比例定位。**Fit graph**（来源图位于 **Graph** 菜单）、**F / Shift+F** 或双击背景只适配现有范围的节点、边和标签，不自动展开全模型，也不重排节点；**Model overview** 是独立的可选范围。拖动或双轴滚动用于平移，捏合或 **Ctrl/Cmd + 滚轮** 缩放，保留指针下的图中位置。快捷键要求 CFG 可见且没有编辑文本；相邻代码区独立滚动。Compare 的分栏与标签在空间变化时保留阅读和手动宽度偏好，长代码可横向滚动。

卡片和边标签按实际内容测量，由 Rust ELK 联合布局。语义缩放在远景仅绘制轮廓，中景显示身份，放大后恢复预览；使用滞回避免闪烁，文字裁剪在各自卡片内，低倍隐藏不可读的预览与标签。节点、边、标签和分组框保持同一世界坐标；可见区域内的线路保留方向与连续性。布局目前仍在浏览器主线程同步计算，范围限制与绘制裁剪不代表任意大图都不会阻塞，详见[层次 CFG 布局](elk-layout.md)。

触控板的普通 wheel 两轴位移用于平移，浏览器捏合使用独立缩放事件。实现沿用 [eframe 的 wheel/gesture 接入](https://github.com/emilk/egui/blob/0.36.2/crates/eframe/src/web/events.rs) 和 [egui 的滚动/缩放增量](https://docs.rs/egui/0.36.2/egui/struct.InputState.html#method.zoom_delta)。分栏与标签使用 [egui_tiles](https://docs.rs/egui_tiles/0.17.1/egui_tiles/)，SSA 表格使用 [TableBuilder](https://docs.rs/egui_extras/0.36.2/egui_extras/struct.TableBuilder.html)；反汇编与 SSA 的长内容按可见行绘制。按钮、编辑与菜单使用 egui 通用控件，语义着色及图的节点和边保留专用绘制。

## 数据边界

各 crate 分别承担以下职责：

| crate | 职责与依赖方向 |
| --- | --- |
| [`evm-abstract-notation`](../crates/evm-abstract-notation) | 身份的 Unicode 格式与状态搜索解析；不依赖 egui、Serde 或执行引擎 |
| [`evm-abstract-protocol`](../crates/evm-abstract-protocol) | 请求、响应、枚举及数据 ID；只依赖 Serde，不依赖 egui 或执行引擎 |
| [`evm-abstract-server`](../crates/evm-abstract-server) | 将协议请求转换为引擎输入，验证 SSA，并投影为结构化响应；提供 HTTP 和静态资源 |
| [`evm-abstract-web`](../crates/evm-abstract-web) | 依赖共享协议和符号库；通过 Web API 获取结果，用 egui Painter 绘制各视图 |

报告封套固定保存 `minimum`、`rich_export`、`global`、`native`、`selected_view`，以及共享的 `programs` 指令目录和 `process_controls` MAY 控制目录。首份可用发布可以只有 `minimum`，两张丰富图均为 `null`；此时从最小模型的来源关系导航，不要求 `metadata.root_program` 或 Native 状态存在。机器状态与 SSA 的数字 ID 只在所属图中解释；切换展示不会搬移图。`ProgramInfo` 按来源账户、runtime/initcode、fork 和 hash 区分；丰富图的 CFG 用 `program` + `basic_block` 查指令，根反汇编用该图的 `metadata.root_program` 查同一目录。多个上下文不会重复携带同一指令列表，也不会因为共享指令而合并机器状态。

`process_controls` 的目录项按完整来源与 epoch 限定，`ProcessFrameKey.location` 和实际 transfer 的 `process.control` 在该关系中解释。`Block(i)` 是来源的解码块序号，`JumpTargets` 是共享合法跳转目标集合，`Any` 与 `End` 没有指令位置。导航须核对来源角色、账户、fork、hash、epoch 和块范围；目录或潜在后继的存在不证明 Body 已求值或调用已返回。实际 regional transfer 与 delivery 保留各自的 current、complete、输入 revision 和输出 generation，详见[渐进结果](progressive-results.md)。

每张图的 `metadata.source_snapshot` 与 `metadata.revision` 保留精确输入身份；`snapshot` 另含可用的链上区块头。组合声明、Native 前缀与图来源交叉校验。Process 的 MAY 控制不填入 Native executed PC 或指令 SSA。组合 status/work/transfers、累计 acquisition、后处理实耗和 timings 位于封套，图内保留原始统计；CLI 的原生数值格式独立于此 wire 投影。

协议 9 加入 report-local mixed symbolic DAG。scalar refs、byte views、persistent/transient roots 和完整 path guards 在同一目录中共享；range/join 的待处理 payload 引用有限 pending machine pool。前端校验 node sort、依赖顺序和 pool refs，按可见行显示来源、写入和复制范围。typed export error 保留策略与实耗；MemoryPolicy 的未展开输入与 EVM Failure 分开显示。详情见[第 17 课](17-symbolic-state.md)。

当前 [Web 协议](../crates/evm-abstract-protocol/src/lib.rs)版本为 21，[原生分析 JSON](../crates/evm-abstract/src/analysis.rs)为 16，[域策略格式与成本版本](../crates/evm-abstract/src/domain/spec.rs)为 3 与 9。当前 wire 使用最小模型、独立丰富导出结果和共享控制目录；旧消费者必须迁移完整字段与身份校验，不能只改版本号。此前协议 16 新增 `WorkLimit`、`Cancelled`、`KernelFailure` reduction reason，分别保留共享工作耗尽、取消与 kernel 失败，与局部 `RoundLimit` 区分。跨成本版本的原始 work 不是同一单位。

此前版本 15 新增必填布尔 `AnalysisLimits.root_terminal_coverage` 与必填可选 `AnalysisReport.root_terminal_coverage`（无记录为 null）；当前用户默认请求显式设置前者为 true，保留旧关闭策略的对照请求显式设置为 false。`StoreSnapshot` 在所有报告中增加全局 storage/transient base refs、nonce/code/existence/created/pending-destruction defaults，旧字段继续保留。`InputIdentity.scope` 和新增 snapshot namespace/revision/input scope 均为规范十进制字符串，数字编码、前导零、溢出及未知字段会被拒绝；work、transfer 等计数仍是原整数类型。旧报告不能只改版本号。迁移细节见[wire 15 身份与 Store 契约](19-root-terminal-coverage.md)。

此前共享协议版本为 15。版本 14 删除 `AnalysisLimits.smt_provider`、`smt_rlimit` 与原生 solver 诊断枚举；旧请求中的这些字段会在任务入队前明确拒绝，旧报告也不能作为版本 14 安装。表达式、byte/slot 根、路径、scalar facts 和真实精度边界继续保留。 LOG 的 `code_hash` 和 `source_kind` 为必填来源字段，配合 emitter、code account 与 pc 区分初始化和部署后代码；前端面板使用完整身份。传输使用 JSON，但请求、结果、进度、任务状态和错误都使用具体 Rust 结构与枚举；不使用无类型 JSON 树，也不解析 CLI、DOT 或 SSA 文本。`ApiError.details` 保留具体错误类别与参数，`message` 只补充人类可读说明。

版本 12 的 `ExportLimits` 保留节点、依赖深度与 exact payload 上限，整份报告 JSON 不设置字节上限；`metadata.export_usage.work` 保存累计 projection work，仅作诊断统计，work 总量不会导致导出失败。导出继续检查取消，错误展示相应策略和实耗，编码取消另保留已写字节。执行分析仍遵守 `AnalysisLimits.max_work`，界面的 Logical work 输入继续控制分析预算。

| 请求 | 返回 |
| --- | --- |
| `GET /api/rpc-providers` | `RpcProvidersReply`：`{"result":{"Ok":[{"id":"mainnet","name":"Ethereum mainnet","endpoint":"https://rpc.example.org/ethereum"}]}}`；无配置时列表为空 |
| `POST /api/tasks`，body 为 `AnalyzeRequest` | `202` 与 `JobReply`，包含任务 ID |
| `GET /api/tasks/{id}` | `JobReply` 包装的小型 `JobSnapshot`：状态、阶段、累计计数、实时／冻结的 `timings` 与可选 `publication` |
| `GET /api/tasks/{id}/result` | `JobResultReply`：成功项含 `id`、`publication` 和 `report`；已有发布即可读取，包括任务仍在运行或已取消时；尚无发布返回类型化 `TaskNotReady` |
| `DELETE /api/tasks/{id}` | 请求取消后的状态，仍通过状态接口确认终态 |

`publication` 的 `version` 是本任务的发布序号；`minimum_version`、`global_version`、`native_version` 分别标识实际保留的最小模型与两张丰富图，`model_closed` 表示来源限定的模型闭合。版本使用规范十进制字符串，任务 ID 仍是整数。可选 Native 或丰富导出可以产生新发布而不改变 `minimum_version`；同版本保留的最小模型、已发布的来源与控制目录项保持原内容。客户端同时核对任务、发布及图命名空间，迟到响应不能替换当前结果。

每个任务的应用后处理账本由服务端 `--postprocess-work` 配置，默认 2,000,000,000，先支付必需最小模型，再支付可选丰富导出。最小模型无法付费会明确失败；已有模型之后的丰富导出拒绝或取消记录在 `rich_export`，不会删除模型。该账本与累计 EVM 执行预算分开，重新发布或 RPC 重启不会刷新任何一份预算。

版本 8 为 `Diagnostic` 增加类型化的 `invalid_opcode` 证据。普通指令保留实际 `opcode`、可选 `mnemonic` 与无效原因，区分 `0xfe (INVALID)`、未定义的 `0xef` 和当前 fork 未启用的 `0x1e (CLZ)`；字节来自诊断状态的当前捕获程序。委托入口单独表示为 `DelegationEntry`，无法核验字节时说明缺失原因，不跳到根程序的合成 PC 0。Diagnostics 保留同 PC 不同抽象状态的每条记录及其跳转；记录数不是不同指令数或失败交易次数。原生 CLI 分析格式独立演进，字段路径见[第 08 课](08-forks.md#从诊断区分原始操作码)。

版本 7 为 `JobSnapshot` 和报告的 `metadata` 增加 `AnalysisTimings`。字段均为 `u64` 纳秒：`queue_ns`、`processing_ns`，以及 `validating_ns`、`pinning_ns`、`acquiring_ns`、`analyzing_ns`、`building_ssa_ns`、`projecting_ns`。阶段耗时互不重复，合计为 `processing_ns`；RPC 展示值为 `pinning_ns + acquiring_ns`。直接调用分析服务函数时队列时间为零。成功、失败或取消后，终态快照的计时冻结；成功报告保留同一份后端计时。用户等待与前端准备属于本次浏览器观察，单独保存，不以浏览器与后端的时钟起点相减计算。

版本 6 为 SSA 增加规范化 `operation`、按最终 ValueId 索引的 `values`、带范围的事实和来源关系；原始 opcode、PC、效果与执行阶段仍保留。版本 5 的提供者名单新增展示用的 `endpoint`；RPC 分析输入继续使用 `provider_id`，不接受浏览器在请求中指定 `endpoint`。例如 `AnalyzeRequest.input` 为 `{"Rpc":{"provider_id":"mainnet","address":"0x0000000000000000000000000000000000000101","block":"Latest","accounts":[]}}`；其余 `fork`、`environment`、`limits` 字段仍按共享请求类型提供。后端在任务入队前按 ID 查找配置，未知 ID 返回 `InvalidRequest`。这些变更属于 Web 共享协议，CLI 的显式 `--rpc` 参数保持其原有用法。

`AnalysisLimits` 的数值均使用 `u64`，避免 Wasm 的 32 位 `usize` 限制后端预算。累计 work / transfers 遥测也使用 `u64`。浏览器在 Rust 中直接处理 JSON 文本，保留整数精度；其他客户端处理这些数字时也应使用可精确保留 64 位整数的解析方式。报告的 `metadata.limits` 保存实际提交的完整策略，不用默认值覆盖用户输入。

报告包含代码目录、完整帧上下文、有效环境、快照身份、账户事实、入口／出口状态和终结结果。不可变 byte-array 与 store 使用报告内索引复用，每个 byte-array 的完整抽象字节值也用局部字典复用；符号表达式用扁平 DAG 表达，保留结构并避免 JSON 嵌套深度随表达式深度增长。状态轮询不重复下载整份报告。

反汇编保留 PC、opcode、立即数与基本块边界；原始 Native CFG 保留状态及边 ID，SSA 按同一 Native 状态 ID 保留值定义、操作数、phi 的前驱边、effect 引用和执行阶段。U256 立即数使用十六进制字符串，避免 JavaScript 数字精度损失。同一 PC 的不同上下文在原始报告中保留独立状态；显示卡片可以分组这些实例，原始 ID 与各自的 SSA 仍独立。

## 如何理解分析结果

字节码输入标为 **SingleProgram**；没有显式提供的环境和持久状态仍是未知。RPC 输入标为 **RpcWorld**，固定 chain ID 与 block hash 后按需补查具体调用目标和有限 storage 槽位。区块环境默认来自同一快照；显式覆盖与原始快照观测分别保留，交易输入不会被猜测为空调用。离线 world 文件仍可使用[CLI](../README.md#命令与输出格式)。

CLI 的 world/RPC SSA 与 Web 共享核心 `ssa::build_world` 和 `ssa::build_partial_world`。Web 对收敛图先执行完整验证，再用同一部分图表达结构生成统一展示数据；原生状态、边和值 ID 均保留。

`Converged` 表示配置范围内的抽象工作表闭合，不表示程序安全，也不表示每条抽象边都可实际执行。`Incomplete` 表示仍有显式前沿。后端保留诊断和前沿的位置与原因，并使用经过验证的部分 SSA 表达已观察到的执行：

- `Unexecuted` 和 `Stale` 状态不展示为当前有效的指令执行体。
- 待处理指令保留实际执行阶段，不生成尚未观察到的结果值。
- phi 和 effect 的输入只引用有执行证据的边；未支持的边保留原因。

这与[部分 SSA 教程](04-ssa.md)和[模型边界](06-boundaries.md)使用同一套语义。

## 任务执行与取消

CPU 分析由通道调度到有限原生线程池，每项分析在一个工作线程上完成。默认工作线程数为 `max(1, available_parallelism / 2)`；`--workers` 可覆盖，`--queue-capacity` 控制额外排队任务，`--retained-jobs` 控制保留多少终态结果。HTTP 接口使用 Axum 路由，由 Hyper 解析请求和生成响应，Tokio 调度网络等待。状态查询和静态资源不等待分析结束，大报告序列化在独立阻塞任务中执行。CPU 分析仍由原生线程池执行，每份 RPC 采集会话保留自己的单线程 reactor。

服务最多同时处理 32 条连接，未完成请求头的客户端也占用连接名额。Hyper 的请求头读取期限为 10 秒；进入应用层后，完整请求体另有 10 秒总期限，慢速分段发送不会延长期限。请求头缓冲上限为 8 KiB，请求体上限为 256 KiB，静态文件上限为 64 MiB；每次等待 socket 写入的上限为 10 秒。

服务支持合法的 HTTP/1.1 chunked 请求与 keep-alive，不要求每次请求关闭连接。损坏的 HTTP 报文由 Hyper 拒绝，可能只有 HTTP 错误状态；进入应用层后的参数、大小、任务和分析错误继续使用共享的类型化 JSON 协议。静态资源保留规范路径和符号链接边界检查。

请求头期限也会回收尚未发送请求的连接，以及响应完成后没有新请求的空闲 keep-alive 连接。Hyper 未处理缓冲区为空时，这类到期属于正常连接回收，不打印错误；缓冲区中存在未完成的请求头时仍记录超时告警，包括与上一请求一并读入的流水线残余数据。连接错误日志包含直接对端地址，便于区分反向代理与其他客户端；请求体和 socket 写入期限继续生效。通过 Caddy 访问时，上游连接由 Caddy 的连接池管理，正常空闲回收不会取消已提交的分析任务。

节点访问使用 Alloy RPC client、Ethereum RPC 类型和区块选择类型；有界传输适配继续约束实际 HTTP 请求次数、响应字节数和超时，取消时释放当前请求及其网络驱动。快照采集仍只在启动时解析一次区块，后续代码、余额、nonce 和存储查询使用固定区块身份；客户端不启用自动重试或批量请求，累计预算记录实际发起的 HTTP 请求。

状态为 `Queued → Running → Completed/Failed`。取消排队任务会直接释放其请求；运行中的任务先进入 `Cancelling`，停止后续工作并丢弃结果，确认工作线程释放任务资源后才进入 `Cancelled`。RPC 请求／响应 future 会被丢弃并关闭 reactor；执行预算检查点、局部关系推理、SSA 构建、报告投影与序列化均检查相同取消作用域。预编译或系统 DNS 等当前调用可能需要先返回，这段时间仍显示 `Cancelling`。每个工作线程由只等待退出的监督线程持有，panic 和停机在 join 完成后确认。通用取消令牌来自无 solver 依赖的 `analysis-control`。

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

本地开发服务同样支持直接传入 URL：`cargo run --locked -p evm-abstract-server -- --assets dist --rpc-config http://127.0.0.1:8545`，也可将 URL 换成 JSON 配置文件路径。

完整验证仍运行[本地门禁](local-ci.md)。原有 native workspace 检查保留，另对 Wasm 目标执行构建、Clippy 和 Dylint。动态派发只在浏览器框架与 Alloy RPC 的精确接入模块内开放；控件、共享协议、HTTP 服务、抽象分析和快照采集策略继续受 `no_dyn` 约束，见[检查规则](no-dynamic-dispatch.md)。

浏览器回归使用 Chromium 的 SwiftShader 软件 WebGPU 适配器，检查实际 WebGPU 上下文及绘制调用，同时覆盖页面分析、DPR 1/2 和图交互。软件适配器验证渲染路径，不代表硬件 GPU 加速性能。

RPC 回归先启动本地 JSON-RPC fixtures，再用真实配置文件启动分析服务，覆盖提供者名单、第二项选择与 `provider_id` 提交、跨合约结果和取消等待中的 RPC。另检查无配置时字节码可用、RPC 禁止提交，以及名单加载失败后的重试。

预算回归在独立页面操作真实输入控件，填写超过旧上限及 `2^53` 的整数，再分析小字节码，核对原始 POST 和报告 `metadata.limits` 精确一致；同时检查 Web 默认值与协议版本。

实现参考：[eframe WebRunner](https://docs.rs/eframe/0.36.2/wasm32-unknown-unknown/eframe/web/struct.WebRunner.html)、[egui Painter](https://docs.rs/egui/0.36.2/egui/struct.Painter.html)。

协议 10 进一步保留未知交互的 `abstract_summaries`、过程依赖与实际覆盖块，以及独立的摘要成功／回退／失败边。Unknown frame 显示为 Abstract environment，不生成捕获指令；对应节点的 inspector 可查看闭合状态与迭代计数。示例与语义见[第 18 课](18-unknown-world.md)。

协议 13 增加摘要入口类别和根过程索引，版本 14 继续保留这些字段，区分已知 bytecode 递归与未知交互。已知递归仍显示真实 Runtime 代码身份，图卡片和 Frame inspector 标出 Recursive bytecode；摘要中实际覆盖的块与主图逐指令执行记录分别展示。闭合关系可以继续产生返回边；未覆盖的终结行为和资源边界仍保留前沿。

保守过程的详细面板分别展示 owner class、完整 catalogue epoch、实际 source 与绑定的 ADDRESS/CALLER/value。构造过程单独标明部署前的 initcode，CREATE 结果为地址类或 zero；空代码、预编译和无效委托沿用各自来源标签。process 不展示内部 native anchor、指令 PC、栈或 SSA，remaining obligations 与报告原有 status/frontiers 分别保留。CLI/Web 的默认策略选择没有随这些显示字段自动改变。
