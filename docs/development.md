# Nix 开发环境与 VS Code

VS Code 和插件由用户自行安装。仓库在 `.vscode` 中提供插件推荐、工作区设置和任务；Nix 提供项目的构建工具、原生库、语言服务器和检查工具。第一次使用需要主机安装 Nix，并启用 `nix-command`、`flakes`。Nix 根据锁定的源码和校验和下载或构建项目依赖。

从仓库根目录进入环境：

```bash
nix develop --no-update-lock-file
cargo check --workspace --all-targets --locked
```

开发环境显式提供 Rust、Cargo、Clippy、rustfmt、rust-analyzer、C/C++ 编译与构建工具、libclang、pkg-config、Z3、Bitwuzla、cvc5、OpenSSL、Graphviz、字体、jq、Taplo、nixfmt、lychee、cargo-nextest 和 Dylint。Python 只用于真实的语言服务器协议回归和可选的性能采样报告。

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

Bitwuzla 和 cvc5 从锁定源码构建，使各自嵌入的 CaDiCaL 符号保持私有。cvc5 使用系统默认链接器，避免该版本 gold 生成的符号版本表与 Rust 的 LLD 不兼容。这里通过编译和链接设置解决问题，不再用 Python 修改已生成的 ELF。首次构建这两个原生包可能较慢，之后可以复用 Nix 产物。

保留的脚本有明确边界：极短的编辑器/命令兼容入口负责调用 Nix；文档链接反例、Taplo LSP 协议检查和性能采样是真实验证逻辑。依赖安装与版本选择由 Nix 模块承担，不在这些测试里另建一套环境。

## 验证

完整门禁当前包含 20 项检查，冻结提交要求见[本地检查流程](local-ci.md)。常用入口：

```bash
nix flake check --print-build-logs --no-update-lock-file --option max-jobs 1 --option cores 8
nix run --no-update-lock-file .#no-dyn
nix run --no-update-lock-file .#no-dyn-ui
nix fmt
```

TOML 回归使用 Nix 提供的 Taplo，向真实 LSP 发送格式化请求并与同版本 CLI 比较结果，验证项目的格式配置。原生求解器共存和安装后分析示例也保留在完整门禁中。编辑器和插件的安装由用户负责。
