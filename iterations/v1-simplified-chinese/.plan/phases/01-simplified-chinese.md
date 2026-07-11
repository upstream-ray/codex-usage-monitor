# Phase 01 — 简体中文基础

**Status**: `completed`
**目标**: 新增完整简体中文资源并正确跟随 Windows 系统语言。
**前置**: 无

## 验收判据

- `zh-CN`、`zh-SG` 解析为简体中文，`zh-TW`、`zh-HK` 保持繁体中文。
- 右键菜单、频率、错误与更新提示均有简体中文资源。

## Tasks

- [x] 新增简体中文资源模块 (`src/localization/simplified_chinese.rs`)
- [x] 接入语言枚举、菜单命令和设置持久化 (`src/localization/mod.rs`, `src/window.rs`; `cargo check`)
- [x] 增加系统语言解析测试 (`src/localization/mod.rs`; `cargo test localization::tests`: 2 passed)

## Notes

- 用户授权全部阶段连续执行并在最终统一验收。
