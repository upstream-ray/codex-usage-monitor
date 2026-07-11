# Phase 01 — 单一托盘身份

**Status**: `completed`
**目标**: 托盘只显示一个与桌面一致的 Codex Usage 图标，并用准确名称表达可并列监控的服务。
**前置**: 用户已于 2026-07-11 确认第一阶段范围。

## 验收判据

- 无论启用一个还是多个服务，Windows 托盘中都只显示一个 Codex Usage 图标
- 托盘图标与 EXE/桌面快捷方式使用同一嵌入式图标资源
- 托盘提示可汇总已启用服务的当前信息，且不超过 Windows 提示长度限制
- 简体中文右键菜单显示“监控服务”，README 不再把这些服务描述成模型
- `cargo test`、`cargo clippy --all-targets`、`cargo build --release` 和 `git diff --check` 通过

## Tasks

- [x] 将托盘注册和更新逻辑收敛为单个应用图标（`src/tray_icon.rs:12,40,147-166`）
- [x] 汇总已启用服务的托盘提示并补充单元测试（`src/window.rs:487-503,656-714,3933-3950`; `cargo test`: 21 passed）
- [x] 更新简体中文菜单及中英文 README（`src/localization/simplified_chinese.rs:13`; `src/localization/english.rs:13`; `README.md`; `README.zh-CN.md`）
- [x] 构建发布版并安装到本机供用户截图验收（`cargo build --release`; installed PID 41876; SHA256 `F714F828E84E4A369ED0EB498828732E2EA0B7698282E6A0D41C5994DBC7F342`; 用户于 2026-07-11 确认第一阶段通过）

## Notes

- 用户截图：`C:/Users/Ray/AppData/Local/PixPin/Temp/PixPin_2026-07-11_14-48-58.png`、`PixPin_2026-07-11_14-49-46.png`
- Phase 01 不修改 Claude Code 认证和配额轮询逻辑。
- 自测：`cargo fmt --check`、`cargo test`、`cargo clippy --all-targets`、`cargo build --release`、`git diff --check` 均成功；Clippy 保持 9 条既有风格警告，无新增警告。
- 用户验收：2026-07-11 确认第一阶段通过。
- 建议提交信息：`feat: unify the system tray identity`。
- Commit：`8633086`。
