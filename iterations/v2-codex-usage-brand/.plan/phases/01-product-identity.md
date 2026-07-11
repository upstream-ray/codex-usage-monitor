# Phase 01 — 产品身份统一

**Status**: `completed`
**目标**: 所有运行时与构建产物身份统一为 Codex Usage。
**前置**: v1 简体中文迭代完成

## 验收判据

- Cargo 生成 `codex-usage.exe`，PE 产品名为 Codex Usage。
- 窗口、日志、互斥体、类名和发布资源不再使用旧产品名。
- 更新器不会下载上游任意旧 EXE 覆盖派生版本。

## Tasks

- [x] 修改 Cargo 包名和 Windows 元数据 (`Cargo.toml`; `cargo test` 编译包名 `codex-usage`)
- [x] 修改运行时内部标识、日志和更新资源名 (`src/window.rs`, `src/diagnose.rs`, `src/updater.rs`)
- [x] 更新本地化产品标题、README 和发布流程 (`src/localization/*.rs`, `README.md`, `.github/workflows/release.yml`)

## Notes

- 用户要求继续完整实施，完成全部验证后统一汇报。
