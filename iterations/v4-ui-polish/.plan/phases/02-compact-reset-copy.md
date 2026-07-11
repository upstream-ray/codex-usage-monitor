# Phase 02 — 紧凑重置文案

**Status**: `completed`
**目标**: 将常驻信息压缩为短标签、剩余百分比和具体重置时刻或日期。
**前置**: Phase 01

## 验收判据

- 简体中文标签显示为 `5h` 与 `7d`。
- 保留“剩余xx%”语义。
- 5 小时窗口显示 `HH:mm重置`，每周窗口显示 `MM/DD重置`。
- 每行保留一条连续圆角进度条，不再绘制分段方块。
- 文案、布局、托盘提示与测试保持一致且无截断。

## Tasks

- [x] 设计并实现具体重置时间格式化规则（Windows 本地时区 `src/native_interop.rs:32`; `HH:mm` / `MM/DD`：`src/poller.rs:1559-1601`）
- [x] 更新简体中文短标签、连续进度条与布局宽度（`src/localization/simplified_chinese.rs:35-36`; `src/window.rs:3926`）
- [x] 更新托盘提示并增加边界测试（本地精确时间 `src/window.rs:475-486`; 格式测试 `src/poller.rs:1713-1748`, `src/window.rs:4129-4143`）
- [x] 实际安装截图验收（v1.8.0 PID 64268；DPI 192 下窗口 566×92px，无截断；`target/v4-compact-reset-verification.png`）
- [x] 构建、提交、推送并发布修订版本（实现 commit `fa6165f`；版本 `1.8.0`）

## Notes

- 用户截图：`C:/Users/Ray/AppData/Local/PixPin/Temp/PixPin_2026-07-11_14-12-12.png`。
- 用户确认目标格式：`5h  剩余82%  18:30重置` / `7d  剩余97%  07/17重置`，每行一条连续进度条。
- 最终验证：`cargo test` 20 passed；`cargo clippy --all-targets` 成功且保持 9 条既有风格警告；`cargo build --release`、`git diff --check` 通过。
- `v1.8.0` Release 工作流成功；正式 EXE 922624 字节，SHA256 `B4B27CB391DA6AE82920D73D216179FC28019BD6CD9929B008512862105B359F`。线上安装器升级后设置哈希不变、桌面快捷方式保留，正式版 PID 56632：`https://github.com/upstream-ray/codex-usage-monitor/releases/tag/v1.8.0`。
