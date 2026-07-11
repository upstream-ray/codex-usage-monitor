# Phase 06 — 用量提醒与显示增强

**Status**: `completed`
**目标**: 在保持轻量的前提下，让用户更主动、更灵活地掌握 Codex 额度。
**前置**: Phase 05

## 验收判据

- 用户可配置低额度提醒阈值，并避免同一窗口重复骚扰。
- 用户可选择显示或隐藏 5 小时、每周用量信息。
- 托盘或悬停信息能显示更精确的重置时间和当前额度。
- 新设置可持久化、向后兼容，并有自动化与实际运行验证。

## Tasks

- [x] 设计提醒阈值、去重规则和显示选项交互（右键菜单：关闭/10%/20%/30%，默认关闭；提供商+窗口+重置时间持久化去重；`README.md:113`）
- [x] 实现低额度 Windows 通知（`src/window.rs:539`, `src/window.rs:634`）
- [x] 实现 5 小时与每周用量独立显示选项（设置兼容 `src/window.rs:321`；命令处理 `src/window.rs:3126`；菜单 `src/window.rs:3429`）
- [x] 增强托盘悬停或详情信息中的精确重置时间（`src/window.rs:502`；UTC 精确到分钟）
- [x] 增加设置兼容、格式化与通知逻辑测试（`src/window.rs:4115`, `src/window.rs:4131`, `src/window.rs:4140`; `cargo test`: 20 passed）
- [x] 完成实际运行验收、提交并推送最终功能版本（功能提交 `b12b59b`；稳定安装路径实际运行通过）

## Notes

- v1.7.0 本地验收 build SHA256: `1BEAEE8A2F98D3785EBBF42BD351467D688606D3104C7D1422273AA8B4583F4A`；GitHub Actions 正式 Release SHA256: `79BA613672244703E3F08FB4AF98E6AA4F40F6AD7A91B26B1FC25AA56830BA7D`，大小均为 924672 字节。
- 稳定安装路径实际运行 PID 18476；通过 Win32 `WM_COMMAND` 切换为 weekly-only 后设置持久化为 `show_session_window=false`，再恢复双行；诊断日志确认 `version=1.7.0`。
- 最终验证：`cargo fmt -- --check`、`cargo test`（20 passed）、`cargo clippy --all-targets`（退出成功，9 条既有风格警告）、`cargo build --release`、`git diff --check` 均通过。
- `v1.7.0` Release 工作流成功，线上包含 EXE、SHA256、安装和卸载脚本；正式安装器升级后设置文件哈希不变，发布版 PID 15096 正常运行：`https://github.com/upstream-ray/codex-usage-monitor/releases/tag/v1.7.0`。
