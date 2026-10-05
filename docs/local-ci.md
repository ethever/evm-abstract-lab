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
nix flake check --print-build-logs --no-update-lock-file --option max-jobs 1 --option cores 8
```

需要启用 `nix-command` 和 `flakes` 的 Nix。按机器余量调整 `max-jobs` 和 `cores`；这两个选项限制构建并发，不减少检查内容。`--no-update-lock-file` 防止检查时修改依赖锁。

等待命令结束，确认 **Nix 自身的退出码为 0**。检查范围以 [`flake.nix` 的 `checks`](../flake.nix) 为准，包含测试、格式、文档和打包后示例；局部 Cargo 测试或单独链接检查用于开发阶段，不能替代完整门禁。

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

此流程只改变检查的执行位置和时机，完整检查的定义仍由 `flake.nix` 维护。返回[课程与开发导航](../README.md#验证与源码导航)。
