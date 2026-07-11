# Phase 03 — 改名版本验证

**Status**: `completed`
**目标**: 生成可长期运行的 Codex Usage EXE，并验证身份、配置、数据和资源占用。
**前置**: Phase 01、Phase 02

## 验收判据

- 格式检查和全部测试通过。
- `target/release/codex-usage.exe` 构建成功，PE 元数据正确。
- 旧设置成功迁移，Codex-only 与简体中文配置保持不变。
- 新程序真实运行、数据可见、私有内存保持轻量。

## Tasks

- [x] 运行格式检查、测试和 release 构建 (`cargo fmt -- --check`; `cargo test`: 12 passed; `cargo build --release`)
- [x] 验证设置与自启注册表迁移 (`%APPDATA%\CodexUsage\settings.json`; HKCU Run `CodexUsage`; 旧项已删除)
- [x] 启动新 EXE 并验证进程、窗口与资源占用 (`codex-usage.exe`; 窗口标题 `Codex Usage`; PrivateMB=3.2)
- [x] 审查最终差异和产物哈希 (`git diff --check`; SHA256 `D7AA4FAC0AD4B47E2893C7DEB507FCCA92032CC7691F7AAA2ACAAF5AD864521D`)

## Notes

- release 目录只保留 `codex-usage.exe`，旧名称构建产物已清理。
- PE 元数据：CompanyName=Ray，ProductName=Codex Usage，OriginalFilename=codex-usage.exe。
- 最终进程 PID 48296，Responding=True，WorkingSetMB=20.5，PrivateMB=3.2。
