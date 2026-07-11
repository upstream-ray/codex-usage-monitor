# Phase 01 — 当前版本审查与基线固化

**Status**: `completed`
**目标**: 确认当前定制差异安全、完整且可复现，并在独立分支形成首个可追溯提交。
**前置**: v1 简体中文与 v2 品牌迁移完成

## 验收判据

- 全部已跟踪与未跟踪差异均经过审查，没有明显功能、安全或许可回归。
- 未建立的 WinGet 包不会被现有工作流误触发，发布元数据不会误导用户。
- 格式检查、测试、Clippy、release 构建及 `git diff --check` 完成。
- 当前工作位于 `codex/` 前缀分支并形成清晰的基线提交。

## Tasks

- [x] 审查全部源码、文档、工作流和未跟踪文件差异 (`git diff --stat`; `git diff --check`; 逐文件审查 `src/poller.rs`, `src/updater.rs`, `src/window.rs`, `src/localization/*`, `README.md`, `Cargo.toml`, `.github/workflows/release.yml`)
- [x] 修正审查发现的基线发布配置或实现问题 (`.github/workflows/release.yml`: 移除尚不存在的 `Ray.CodexUsage` 自动提交 job，保留 `codex-usage.exe` Release 构建)
- [x] 运行完整自动验证并核对 release 产物 (`cargo fmt -- --check`; `cargo test`: 12 passed; `cargo clippy --all-targets`: 成功、9 warnings; `cargo build --release`; SHA256 `4919D98614882A327288B0287B862E7A590010400BF4EB6DCD993EE36C2277DA`)
- [x] 创建 `codex/` 前缀基线分支并提交审查后的改动 (`codex/v1-codex-usage`; commit `b6a3c9528e9c870d3a57419deda05fad946ee46f`)
- [x] 核对提交内容、工作区状态和可回滚点 (`git show --stat HEAD`; `git diff main...HEAD --check`; merge-base `9b299725c62f51aff82577a7ec634a5fd14a3bd9`)

## Notes

- 初审发现 `.github/workflows/release.yml` 已引用尚未建立的 `Ray.CodexUsage`，而 `Cargo.toml` 的 repository/homepage 仍指向上游；需在提交前明确基线行为。
- 审查确认保留原始 MIT `LICENSE`（Copyright 2025 Craig Constable）和 README 派生归属；更新器只接受精确资产名 `codex-usage.exe`。
- Phase 01 基线提交已完成；`Cargo.toml` 的真实新仓库地址将在 Phase 02 创建公开仓库后更新。
