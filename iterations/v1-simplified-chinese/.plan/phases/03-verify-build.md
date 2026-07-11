# Phase 03 — 验证与交付

**Status**: `completed`
**目标**: 通过自动测试、release 编译和实际运行检查生成可交付 EXE。
**前置**: Phase 01、Phase 02

## 验收判据

- `cargo test` 全部通过。
- `cargo build --release` 成功。
- release EXE 能启动并保持低资源占用。
- Git 差异只包含本次功能和实施记录。

## Tasks

- [x] 运行格式检查与完整测试 (`cargo fmt -- --check`; `cargo test`: 9 passed; `cargo clippy --all-targets` 通过并保留 9 条原有风格警告)
- [x] 编译 release EXE (`cargo build --release`; `target/release/claude-code-usage-monitor.exe`, 852992 bytes)
- [x] 启动验证并测量资源占用 (PID 55824; Responding=True; PrivateMB=3.4; `target/widget-printwindow-large.png`)
- [x] 审查最终差异和产物信息 (`git diff --check`; SHA256 `03DAF2D41B74C009871D9604242D95225168EA8468E7F5D9E7F5FA37A3102513`)

## Notes

- 实际任务栏渲染验证文字为“5小时 剩余95% 3小时后重置 / 每周 剩余99% 6天后重置”，未发生截断。
- 新 release 程序已保持运行，设置语言为 `zh-CN`，模型配置仍为 Codex-only。
