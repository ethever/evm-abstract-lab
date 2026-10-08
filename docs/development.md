# Nix 开发环境与 VS Code

VS Code 和插件由用户自行安装。仓库在 `.vscode` 中提供插件推荐、工作区设置和任务；Nix 提供项目的构建工具、原生库、语言服务器和检查工具。第一次使用需要主机安装 Nix，并启用 `nix-command`、`flakes`。Nix 根据锁定的源码和校验和下载或构建项目依赖。

从仓库根目录进入环境：

```bash
nix develop --no-update-lock-file
cargo check --workspace --all-targets --locked
```

开发环境显式提供 Rust、Cargo、Clippy、rustfmt、rust-analyzer、Wasm target、wasm-bindgen、C/C++ 编译与构建工具、libclang、pkg-config、Z3、Bitwuzla、cvc5、OpenSSL、Graphviz、字体、jq、Taplo、nixfmt、lychee、cargo-nextest 和 Dylint。Python 用于真实的语言服务器协议回归和可选的性能采样报告，浏览器检查另使用 Nix 提供的 Playwright 和 Chromium。

原生库通过 Nix 的构建环境暴露给 Cargo，包括 `PKG_CONFIG_PATH`、cvc5 头文件/库路径和 libclang。Rust 语言服务器与命令行构建使用同一个开发环境；不要把某次构建产生的 `/nix/store/...` 路径手工写入工作区设置。

## 接入现有 VS Code，包括 Remote SSH

用已有的 VS Code 打开仓库根目录，按[工作区推荐](../.vscode/extensions.json)自行安装 `rust-lang.rust-analyzer` 和 `tamasfe.even-better-toml`。使用 Remote SSH 时，在远端工作区安装对应插件，并确保远端主机可使用 Nix。

[工作区设置](../.vscode/settings.json)为 Rust 语言服务器选择项目环境，并配置 TOML 格式规则。`rust-analyzer.server.path` 使用一个很短的[入口](../scripts/rust-analyzer-nix.sh)，进入当前仓库的 Nix 环境后再启动服务器。库路径、编译器和工具选择在 Nix 中定义，因此 Cargo metadata、构建脚本、保存检查和过程宏都能继承原生依赖环境。

Even Better TOML 默认使用插件自带的 Taplo，其版本随用户安装的插件变化。Nix 提供固定版本的 Taplo CLI 和用于协议回归的 Taplo LSP。两端共同读取 [`taplo.toml`](../taplo.toml)，但不保证使用同一 Taplo 版本；门禁也不验证用户安装的任意插件版本。

命令面板中的 `Tasks: Run Task` 提供[工作区任务](../.vscode/tasks.json)：`Nix: build`、`Nix: test` 和 `Nix: clippy`。默认构建任务也使用 Nix。手动终端命令应先执行 `nix develop`；插件自身的 Run/Test 按钮行为由插件配置决定，需要固定项目环境时使用这些 Nix 任务。

安装插件或更新工作区设置后，执行 `Developer: Reload Window`。若出现 `Package bitwuzla was not found` 或原生库构建脚本失败，核对工作区设置已生效，再执行 `rust-analyzer: Restart server`。仅在终端里进入 `nix develop` 不会改变已运行语言服务器的环境。

修改锁文件后，重新进入开发环境并重启 rust-analyzer。编辑器和插件版本由用户管理；项目锁文件管理下表中的工具和依赖。

## 版本由哪里固定

| 来源 | 覆盖范围 |
| --- | --- |
| [`flake.lock`](../flake.lock) | nixpkgs、rust-overlay、Crane 的确切提交与源码校验和；固定构建工具、原生库、格式器和检查工具 |
| [`rust-toolchain.toml`](../rust-toolchain.toml) | 应用的 Rust 版本及 rust-analyzer、Clippy、rustfmt、rust-src 组件 |
| [`Cargo.lock`](../Cargo.lock) | 应用 Rust 依赖；Nix 根据锁文件下载并提供 vendored 源码 |
| [`lints/no_dyn/rust-toolchain.toml`](../lints/no_dyn/rust-toolchain.toml)、[lint 的 Cargo.lock](../lints/no_dyn/Cargo.lock) | 编译器插件的独立 nightly 和 Rust 依赖 |
| [`nix/dylint/packages.nix`](../nix/dylint/packages.nix) | Dylint 工具源码的固定提交与校验和 |

升级需要显式更改对应来源并重新验证；开发命令使用 `--locked` 或 `--no-update-lock-file` 保留所选版本。

当前锁定的 nixpkgs 支持 `x86_64-linux`、`aarch64-linux`、`aarch64-darwin`，flake 只声明这些平台。具体 PR 的实际构建平台以验证记录为准；派生求值成功不等于已在该平台完成运行测试。

## Nix 模块职责

| 模块 | 负责什么 |
| --- | --- |
| [`flake.nix`](../flake.nix) | 锁定输入、支持平台和公开输出 |
| [`nix/system.nix`](../nix/system.nix) | 连接模块，选择公共工具链 |
| [`nix/dependencies.nix`](../nix/dependencies.nix) | 构建和开发共用的依赖清单、原生环境变量 |
| [`nix/build.nix`](../nix/build.nix) | Rust 源码过滤、Cargo 缓存、默认/imbl 构建 |
| [`nix/shell.nix`](../nix/shell.nix) | 开发环境及其显式依赖 |
| [`nix/smt.nix`](../nix/smt.nix)、[`nix/smt/`](../nix/smt/) | 三个求解器所需原生包、编译/链接设置和共存检查 |
| [`nix/dylint.nix`](../nix/dylint.nix)、[`nix/dylint/`](../nix/dylint/) | lint 工具链、可复用插件构建、命令和检查 |
| [`nix/checks.nix`](../nix/checks.nix)、[`nix/checks/`](../nix/checks/) | Rust、格式、文档、安装后示例和后端对照检查 |
| [`nix/commands.nix`](../nix/commands.nix) | 可选实验的 Nix 入口 |
| [`nix/web.nix`](../nix/web.nix) | egui 的 Wasm 资源、匹配的 wasm-bindgen、服务入口和浏览器检查 |

Bitwuzla 和 cvc5 从锁定源码构建，使各自嵌入的 CaDiCaL 符号保持私有。cvc5 使用系统默认链接器，避免该版本 gold 生成的符号版本表与 Rust 的 LLD 不兼容。这里通过编译和链接设置解决问题，不再用 Python 修改已生成的 ELF。首次构建这两个原生包可能较慢，之后可以复用 Nix 产物。

保留的脚本有明确边界：极短的编辑器/命令兼容入口负责调用 Nix；文档链接反例、Taplo LSP 协议检查和性能采样是真实验证逻辑。依赖安装与版本选择由 Nix 模块承担，不在这些测试里另建一套环境。

## 构建 profile 与缓存

应用的 profile 在根目录 [`Cargo.toml`](../Cargo.toml) 定义。日常开发和测试优先缩短编译时间，安装的 CLI、分析服务和 Wasm 优先运行性能：

| 命令 | Profile | 编译方式 |
| --- | --- | --- |
| `cargo build`、`cargo check`、`cargo run` | `dev` | `opt-level=0`、无调试信息、关闭 LTO、256 个 codegen units、增量编译 |
| `cargo test`、Nix 工作区测试与 doctest | `test`，继承 `dev` | 同样使用快速编译，并开启 debug assertions 与整数溢出检查 |
| `cargo build --release`、`nix build`、`nix run`、`nix profile add` | `release` | `opt-level=3`、完整 fat LTO、1 个 codegen unit、关闭增量编译、移除调试信息 |
| `cargo bench` | `bench`，继承 `release` | 使用与交付产物相同的优化设置 |

完整 LTO 跨 crate 优化，单个 codegen unit 减少 crate 内的优化边界，代价是编译与链接更慢。这是面向运行性能的静态优化配置；具体吞吐仍需用真实工作负载测量，PGO 需要另外采集有代表性的运行数据。设置含义见 [Cargo profiles](https://doc.rust-lang.org/cargo/reference/profiles.html#lto)。编译器插件 `lints/no_dyn` 有独立的 workspace 和 nightly，其 UI 测试也使用快速的 test profile。

```bash
nix develop --no-update-lock-file --command cargo test --workspace --locked
nix build --no-update-lock-file
nix run --no-update-lock-file -- --help
nix profile add .
nix run --no-update-lock-file .#web -- --bind 127.0.0.1:8080
```

安装、开发环境和应用运行的构建依赖图中没有工作区测试、Dylint 检查或浏览器测试。项目定制的 Z3、Bitwuzla、cvc5 和 CaDiCaL 也使用不运行自测的安装构建，其上游自测在完整门禁中单独执行。安装成功只表示产物已生成；发布前的验证入口是 `nix flake check`。

仓库没有 `jobs=8`、固定测试线程数或 CPU 预留。普通 Cargo 使用可用 CPU 的默认并发；Nix 构建遵循调用者的 Nix 配置和 `NIX_BUILD_CORES`。需要手动选择并发时，可以设置 Cargo 的 `--jobs` / `CARGO_BUILD_JOBS`，或者 Nix 的 `max-jobs` / `cores`；项目不添加额外上限。`DYLINT_JOBS` 和 `STATE_BACKEND_JOBS` 仅在显式设置时覆盖 Cargo 的默认选择。

Nix 使用以下缓存层（[Crane 的依赖缓存](https://crane.dev/API.html#cranelibbuilddepsonly)）：

| 层 | 缓存内容 | 何时重新生成 |
| --- | --- | --- |
| 工具链与原生库 | Rust、Dylint 工具、SMT 库 | 对应锁定输入、版本或原生构建设置变化 |
| Vendored 源码 | 同一份 Cargo.lock 的第三方 crate 源码 | 锁文件或 registry 配置变化 |
| Rust 依赖产物 | 默认 / imbl / 全 features 的快速测试依赖，默认 / imbl 的 release 依赖，Wasm 与 nightly lint 依赖 | 工具链、profile、features、target、依赖或编译环境变化 |
| 应用产物 | CLI、分析服务、原生前端和优化后的 Wasm | 相关 Rust、C++、manifest 或编译期 fixture 变化 |
| Web 打包 | wasm-bindgen 输出与 HTML 页面 | Wasm 产物、匹配的 wasm-bindgen 或 HTML 变化 |
| 验证结果 | 独立的测试、lint、文档、原生库和浏览器检查 | 对应检查的输入变化 |

修改应用 `.rs` 不会丢弃第三方依赖产物；修改 Markdown 不会重编应用；修改 Web HTML 只重做资源打包。不同 profile、features、target 和编译器的产物有独立缓存，避免把快速测试依赖当作 release 依赖复用。`nix flake check` 也复用输入未变化的验证结果，输入相同的缓存结果就是同一项检查的先前成功产物。

## 验证

完整门禁覆盖 native workspace 与 Wasm 前端，在 Linux 上还运行实际浏览器检查；冻结提交要求见[本地检查流程](local-ci.md)。常用入口：

```bash
nix flake check --print-build-logs --no-update-lock-file
nix run --no-update-lock-file .#no-dyn
nix run --no-update-lock-file .#no-dyn-ui
nix fmt
```

TOML 回归使用 Nix 提供的 Taplo，向真实 LSP 发送格式化请求并与同版本 CLI 比较结果，验证项目的格式配置。原生求解器共存和安装后分析示例也保留在完整门禁中。编辑器和插件的安装由用户负责。
