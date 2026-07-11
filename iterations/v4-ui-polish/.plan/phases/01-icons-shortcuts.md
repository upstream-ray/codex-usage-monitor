# Phase 01 — 图标与快捷方式

**Status**: `completed`
**目标**: 让应用、EXE、桌面快捷方式和系统托盘在常见小尺寸下具有统一且清晰的视觉。
**前置**: 无

## 验收判据

- ICO 包含多尺寸资源，在 16/20/24/32/48/256px 下轮廓清楚。
- 桌面与开始菜单快捷方式均指向稳定安装 EXE，并显式使用其图标。
- 托盘图标不再是黑色数字方框，仍能表达剩余额度。
- 安装升级保留快捷方式，卸载能清理两处快捷方式。

## Tasks

- [x] 重绘应用图标并生成多尺寸 PNG/ICO（`scripts/generate_icons.py:18`；ICO 验证包含 16/20/24/32/40/48/64/128/256px）
- [x] 重绘动态系统托盘图标（`src/tray_icon.rs:50-151`；圆形进度环、深色底和提供商强调色）
- [x] 增加桌面快捷方式安装与卸载逻辑（`scripts/install.ps1:19,157-173`; `scripts/uninstall.ps1:14,34`）
- [x] 增加快捷方式脚本验证并执行实际安装验收（桌面与开始菜单均指向 `%LOCALAPPDATA%\Programs\CodexUsage\codex-usage.exe,0`；已清理两个指向工作区 build 的旧快捷方式）
- [x] 构建、截图并提交 Phase 01（`cargo test`: 20 passed；`cargo build --release`；`target/v4-desktop-shortcut.png`）

## Notes

- 用户截图：`C:/Users/Ray/AppData/Local/PixPin/Temp/PixPin_2026-07-11_14-11-56.png`、`PixPin_2026-07-11_14-12-06.png`。
- 新桌面快捷方式为 `D:/xuniCpan/Codex Usage.lnk`；Windows 左下角箭头为系统快捷方式覆盖层，底层图标与稳定安装 EXE 完全一致。
