# PR 合并前的本地检查

本仓库在**每次 PR 合并前**，由提交者或合并者在本地运行完整 `nix flake check`。检查通过后才能合并；GitHub Actions 不运行这项门禁。

## 1. 准备最终提交

先把最新 main 的内容同步到 PR 分支，解决冲突并提交所有改动。使用独立工作树时，切到该工作树的仓库根目录。

```bash
git status --porcelain
git rev-parse HEAD origin/main
```

第一条命令应没有输出，表示工作树和暂存区干净。第二条依次输出 HEAD 和已同步的 origin/main 的完整 SHA，分别记录待检查的提交和 main 基线。SHA 是 Git 提交的唯一标识，不是分支名。

## 2. 在本地执行完整门禁

```bash
nix flake check --print-build-logs --no-update-lock-file
```

需要启用 `nix-command` 和 `flakes` 的 Nix。仓库不设置并发上限或 CPU 预留，构建并发由宿主 Nix 配置决定；调用者可以通过 `max-jobs` 和 `cores` 自行调整。`--no-update-lock-file` 防止检查时修改依赖锁。

等待命令结束，确认 **Nix 自身的退出码为 0**。检查范围以 [`nix/system.nix` 汇总的 `checks`](../nix/system.nix) 为准，可以列出实际检查项：

```bash
nix eval --json .#checks.x86_64-linux --apply builtins.attrNames
```

[Rust 检查](../nix/checks/rust.nix)使用快速 test profile，分别覆盖默认、imbl、全部 features 的所有 targets 和 doctest，包含 ignored 测试，并分别运行 Clippy 和 Rustdoc。release 与 imbl release 产物也会构建，并用于[安装后示例](../nix/checks/examples.nix)、[后端对照与求解器检查](../nix/checks/runtime.nix)。测试和发布构建分开缓存，见[构建说明](development.md#构建-profile-与缓存)。

Web 前端另外覆盖 Wasm 构建、Clippy、Dylint 与真实 Chromium 页面交互，定义见 [`nix/web.nix`](../nix/web.nix) 和 [Dylint 检查](../nix/dylint/checks.nix)。[原生求解器检查](../nix/smt.nix)包含 Z3、Bitwuzla、cvc5、CaDiCaL 的上游自测和实际安装库的共存检查。[检查模块](../nix/checks.nix)还包含 Rust / Nix 格式、文档、默认 / imbl / 全 features 的 Dylint 动态派发检查和 lint 的 UI 回归测试（见[检查说明](no-dynamic-dispatch.md)）。`toml-lint` 用 Taplo 检查仓库 TOML 的语法与重复键；`toml-format` 检查格式，并通过真实 Taplo LSP 请求验证配置下的格式化结果与同版本 CLI 一致，均使用 Nix Taplo 并覆盖隐藏目录中的 TOML。规则见 [`taplo.toml`](../taplo.toml)。VS Code 和插件由用户自行安装；门禁检查项目工具和配置，不验证用户安装的任意插件版本。局部 Cargo 测试或单独链接检查用于开发阶段，不能替代完整门禁。

失败或中断时，修正问题，再检查最终提交。若把输出通过管道交给 `tee` 保存，必须启用 `pipefail` 或另外保存 Nix 的退出码，避免把日志工具的成功当成检查成功。

## 3. 核对并记录结果

检查结束后再次运行：

```bash
git fetch origin
git rev-parse HEAD origin/main
git status --porcelain
```

两个 SHA 都应与检查前相同，状态输出仍应为空。在 PR 描述中记录：

- 被检查的完整提交 SHA。
- 本次检查已同步的 main 基线 SHA。
- 本地平台和实际执行命令，包括资源选项。
- 命令已完成、退出码为 0，以及关键结果；保存完整日志供核对。

合并者核对 PR 的远端 head 与记录的 SHA 一致，并确认结果来自该提交的本地完整检查，再执行合并。

## 4. 合并内容变化后重新检查

PR 新增提交、同步新的 main、解决冲突或改变依赖锁后，都需要对新的最终提交重跑完整门禁，并更新 PR 记录。合并前还应确认 main 未相对记录的基线前进；若已变化，先同步最新 main，再重跑并更新记录。旧提交或旧合并基线的通过结果不能作为新的合并依据。

此流程只改变检查的执行位置和时机，完整检查的定义仍由 [`flake.nix`](../flake.nix) 维护。返回[课程与开发导航](../README.md#验证与源码导航)。
