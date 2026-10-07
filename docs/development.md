# Nix 开发环境与 VS Code

仓库的构建工具、原生库、语言服务器和检查工具都由 Nix 提供。第一次使用需要主机安装 Nix，并启用 `nix-command`、`flakes`。Nix 根据锁定的源码和校验和下载或构建依赖，不需要另外用 `apt`、`brew`、`cargo install` 安装项目工具。

从仓库根目录进入环境：

```bash
nix develop --no-update-lock-file
cargo check --workspace --all-targets --locked
```

开发环境显式提供 Rust、Cargo、Clippy、rustfmt、rust-analyzer、C/C++ 编译与构建工具、libclang、pkg-config、Z3、Bitwuzla、cvc5、OpenSSL、Graphviz、字体、jq、Taplo、nixfmt、lychee、cargo-nextest 和 Dylint。Python 只用于真实的语言服务器协议回归和可选的性能采样报告。

原生库通过 Nix 的构建环境暴露给 Cargo，包括 `PKG_CONFIG_PATH`、cvc5 头文件/库路径和 libclang。编辑器和命令行使用同一个开发环境；不要把某次构建产生的 `/nix/store/...` 路径手工写入工作区设置。

## 使用现有 VS Code，包括 Remote SSH

用 VS Code 打开仓库根目录，在**该窗口的集成终端**运行：

```bash
nix run --no-update-lock-file .#install-vscode-extensions
```

安装器使用当前窗口的 `code` 命令，将 Nix 锁定的 rust-analyzer 和 Even Better TOML 安装到对应的本地或远端扩展环境。它安装本地 VSIX 文件，不从 Marketplace 选择“最新版”，也不会在每次打开项目时自动安装。VSIX 带有固定版本的服务器配置：Rust 使用仓库工具链，TOML 使用与 CLI 相同的 Nix Taplo。

安装器会保留扩展需要的 Nix 运行库，避免清理 Nix 缓存后服务器路径失效；安装中途失败也会保留此前版本的运行库。完成后执行命令面板里的 `Developer: Reload Window`。

工作区的 `rust-analyzer.server.path` 指向一个很短的[入口](../scripts/rust-analyzer-nix.sh)。它只负责进入当前仓库的 Nix 环境并启动语言服务器，库路径、编译器和工具选择全部在 Nix 中定义。这样 Cargo metadata、构建脚本、保存检查和过程宏都能继承原生依赖环境。

命令面板中的 `Tasks: Run Task` 提供 Nix 下的 Cargo build、test 和 Clippy；默认构建任务也使用 Nix。手动终端命令应先执行 `nix develop`。这些任务的环境是显式的，不需要猜测现有 VS Code 主进程是否从某个 Nix 终端启动。

若仍出现 `Package bitwuzla was not found` 或原生库构建脚本失败，先执行 `rust-analyzer: Restart server`。仅在终端里进入 `nix develop` 不会改变已运行语言服务器的环境。Remote SSH 的 Nix 和依赖位于远端主机。

## 同时固定 VS Code 本身

如果需要连编辑器版本也一起固定，在仓库根目录运行：

```bash
nix run --no-update-lock-file .#vscode -- .
```

这个入口使用 Nix 锁定的 VS Code 和两个扩展，并使用独立的用户数据目录，避免复用已经启动的非 Nix 编辑器进程。现有 VS Code 的个人设置和扩展目录不因此被替换。仓库仅对显式请求的 VS Code 包允许其非自由许可证，没有启用全局 `allowUnfree`。

只构建扩展归档而不安装：

```bash
nix build --no-update-lock-file .#vscode-extensions
```

修改锁文件后，重新进入开发环境；现有 VS Code 重新运行扩展安装器并重载窗口。Nix 生成的扩展会指向新版本的服务器。

## 版本由哪里固定

| 来源 | 覆盖范围 |
| --- | --- |
| [`flake.lock`](../flake.lock) | nixpkgs、rust-overlay、Crane 的确切提交与源码校验和；因此固定系统工具、原生库、VS Code 和扩展来源 |
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
| [`nix/shell.nix`](../nix/shell.nix) | 开发环境；不依赖完整检查集合，也不隐式安装 GUI |
| [`nix/editor.nix`](../nix/editor.nix)、[`nix/editor/extensions.nix`](../nix/editor/extensions.nix) | 锁定 VS Code、扩展、VSIX 安装入口和编辑器验证 |
| [`nix/smt.nix`](../nix/smt.nix)、[`nix/smt/`](../nix/smt/) | 三个求解器所需原生包、编译/链接设置和共存检查 |
| [`nix/dylint.nix`](../nix/dylint.nix)、[`nix/dylint/`](../nix/dylint/) | lint 工具链、可复用插件构建、命令和检查 |
| [`nix/checks.nix`](../nix/checks.nix)、[`nix/checks/`](../nix/checks/) | Rust、格式、文档、安装后示例和后端对照检查 |
| [`nix/commands.nix`](../nix/commands.nix) | 可选实验的 Nix 入口 |

Bitwuzla 和 cvc5 从锁定源码构建，使各自嵌入的 CaDiCaL 符号保持私有。cvc5 使用系统默认链接器，避免该版本 gold 生成的符号版本表与 Rust 的 LLD 不兼容。这里通过编译和链接设置解决问题，不再用 Python 修改已生成的 ELF。首次构建这两个原生包可能较慢，之后可以复用 Nix 产物。

保留的脚本有明确边界：极短的编辑器/命令兼容入口负责调用 Nix；文档链接反例、Taplo LSP 协议检查和性能采样是真实验证逻辑。依赖安装与版本选择由 Nix 模块承担，不在这些测试里另建一套环境。

## 验证

完整门禁及冻结提交要求见[本地检查流程](local-ci.md)。常用入口：

```bash
nix flake check --print-build-logs --no-update-lock-file --option max-jobs 1 --option cores 8
nix run --no-update-lock-file .#no-dyn
nix run --no-update-lock-file .#no-dyn-ui
nix fmt
```

编辑器检查会实际启动锁定的 Code CLI，在临时用户目录安装 VSIX，核对扩展版本、服务器路径和固定状态。TOML 回归另行通过真实 LSP 请求比较保存结果与 CLI；原生求解器共存和安装后分析示例也保留在完整门禁中。
