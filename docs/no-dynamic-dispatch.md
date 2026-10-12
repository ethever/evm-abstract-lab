# 禁止 Rust 动态派发

应用 workspace 默认禁止 trait object（`dyn Trait`）和函数指针（`fn(...)`），包括库、CLI、示例和测试。使用具体类型、泛型、`impl Trait` 或枚举表达分支；闭包和函数项保持具体类型时允许。框架接入有两个精确的模块例外：

- [`evm_abstract_web::framework`](../crates/evm-abstract-web/src/framework.rs) 接入 egui 的 `TextBuffer`、egui_tiles 的 `Behavior`、eframe 应用回调和 wasm-bindgen JavaScript 回调。
- [`evm_abstract::world::rpc::framework`](../crates/evm-abstract/src/world/rpc/framework.rs) 接入 Alloy 的 RPC client 和 transport。Alloy 的 `RpcCall` 默认响应映射使用函数指针，transport 接口返回装箱的异步对象；适配模块承接这些 SDK 类型，并保留项目的有界传输和具体错误。

例外只用于框架接入。自绘控件、前端状态、共享协议、HTTP 服务、抽象分析、SSA，以及 RPC 的快照和按需采集逻辑继续遵循禁用规则。Alloy 接口依据见 [`RpcCall`](https://docs.rs/alloy-rpc-client/2.5.0/alloy_rpc_client/struct.RpcCall.html) 和 [`Transport`](https://docs.rs/alloy-transport/2.5.0/alloy_transport/trait.Transport.html)。

## 运行

Nix 开发环境已提供固定版本的 Dylint、链接器和编译器驱动：

```bash
nix develop
cargo dylint --all
cargo dylint --all --workspace -- --locked --all-targets
cargo dylint --all --workspace -- --locked --all-targets --all-features
no-dyn-ui
no-dyn
```

`--all` 表示加载全部 lint 库；`--workspace`、`--all-targets` 和 `--all-features` 扩大被检查的 Cargo 包、目标和条件编译范围。`no-dyn` 还检查 `wasm32-unknown-unknown` 目标的前端代码和 Rustdoc 提取的 doctest，并用能成功的静态例子和必须被 lint 拒绝的动态例子验证驱动实际生效。默认和 `imbl` 后端分别进入完整 Nix 门禁，避免只检查某个后端；`web-no-dyn` 单独覆盖实际浏览器目标。

也可以在仓库根目录直接运行 Nix 入口：

```bash
nix run --no-update-lock-file .#no-dyn
nix run --no-update-lock-file .#no-dyn-ui
```

兼容命令 [`bash scripts/check-no-dyn.sh`](../scripts/check-no-dyn.sh) 也会先进入 Nix，再检查全部 targets、features 和 doctest。无需单独 `cargo install` 或下载另一份编译器。

命令默认使用 Cargo 的可用 CPU 并发，不保留固定 CPU 余量。`DYLINT_JOBS` 可显式选择任意正整数并发数，也可以直接传入 Cargo 的 `--jobs` 参数；UI 与 doctest 使用[快速测试 profile](development.md#构建-profile-与缓存)。

lint 库有独立的 [`rust-toolchain.toml`](../lints/no_dyn/rust-toolchain.toml) 和 [`Cargo.lock`](../lints/no_dyn/Cargo.lock)。它使用官方脚手架支持的 `nightly-2026-08-20`；应用的构建工具链仍由根目录的 [`rust-toolchain.toml`](../rust-toolchain.toml) 固定，两者都由 Nix 提供。完整检查复用预先构建的同一份 lint 库。修改 `lints/no_dyn` 后，重新运行 `nix run .#no-dyn` 或重新进入开发环境，使命令使用新源码生成的库；`no-dyn-ui` 和直接 `cargo dylint --all` 可用于开发 lint 本身。

## 检查内容

[`no_dyn`](../lints/no_dyn/src/lib.rs) 实现 `LateLintPass`，检查 rustc 完成类型检查后的类型和隐式转换。除了显式 `dyn` 和函数指针，它还检查类型别名、关联类型、嵌套泛型、推断出的值、函数签名，以及转换为 trait object 或函数指针的表达式。宏展开后的代码也检查，不能用本地宏或外部宏隐藏违规代码。

lint 的默认级别为 `forbid`，`#[allow(no_dyn)]` 不能降低级别。框架接入例外同时检查 crate 身份和 rustc 的真实定义父链：只匹配 `evm_abstract_web::framework` 和 `evm_abstract::world::rpc::framework` 的精确模块位置及其词法子项，包括模块内的 trait impl 方法。判断不依赖 Self 类型或格式化符号路径；框架类型在模块外的 impl、其他 crate 的同名模块、控件或采集模块内嵌套的同名模块均不适用。调用接入模块也不会豁免调用方：带有函数指针默认泛型参数的 RPC call、装箱 future 等动态类型必须在适配模块内消化，再向采集逻辑返回具体结果。UI 回归验证模块内的外部 trait 实现、动态返回值与转换、Drop 回调清理、依赖默认泛型和别名，以及上述拒绝范围。

编译器支持代码另外排除 builtin `Debug` 派生中的格式化支持、builtin `#[test]` / `#[bench]` 回调和测试 / proc-macro 入口框架。通过编译器的 builtin 标记识别来源，用户自定义同名宏不能绕过；原始字段类型、测试函数和嵌入的用户表达式继续检查。rustc 对静态闭包和异步闭包使用的内部函数指针表示不会被误认为用户创建的函数指针。

错误通过具体枚举和字段保留。`DecodeError`、`RpcError` 和 CLI 错误不再生成动态 `Error::source()` 转换；标准 `source()` 使用默认 `None`。调用者可匹配具体变体读取底层错误，RPC 的公共 `source` 字段和显示文本保留。

JSON 输入改用[具体 visitor](../crates/evm-abstract-cli/src/world/input.rs)，保留重复 / 未知 / 缺失字段检查、nullable 字段、默认 storage 和 identity tag 顺序；避免 Serde 派生错误路径中的 `&dyn Expected` 转换。

## 范围

这是仓库源码约束，不检查第三方依赖内部实现，也不保证最终机器码没有间接调用。编译器生成的格式化、测试框架以及依赖内部可能仍使用间接调用。[`lints/no_dyn`](../lints/no_dyn) 是独立的开发工具，Dylint 的插件注册接口本身要求动态 lint pass；它不属于应用 workspace。[`ui`](../lints/no_dyn/ui) 中的反例用于证明违规代码会编译失败。

`LateLintPass` 只检查当前条件编译配置；完整门禁覆盖当前 workspace 的默认和全部 features，以及前端的 `wasm32-unknown-unknown` 实现。新增互斥 feature、目标平台专属实现时，应补充相应检查配置。

实现依据：[Dylint 官方说明](https://github.com/trailofbits/dylint)，其中 `cargo dylint new no_dyn` 生成 lint 库脚手架，workspace metadata 让 `cargo dylint --all` 自动加载本地库。
