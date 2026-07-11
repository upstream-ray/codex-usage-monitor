# Phase 02 — 设置与自启迁移

**Status**: `completed`
**目标**: 旧用户升级后保留设置，并且只有一个正确的 Codex Usage 自启项。
**前置**: Phase 01

## 验收判据

- 新设置路径为 `%APPDATA%\CodexUsage\settings.json`。
- 新路径不存在时读取旧设置并写入新路径，旧文件保留。
- 旧自启项存在时迁移到 `CodexUsage` 并删除旧项；旧项不存在时不擅自开启。

## Tasks

- [x] 实现设置文件兼容读取和迁移 (`src/window.rs`; 实际生成 `%APPDATA%\CodexUsage\settings.json`)
- [x] 实现注册表自启项兼容迁移 (`src/window.rs`; 实际迁移为 HKCU Run `CodexUsage` 并删除旧项)
- [x] 增加迁移相关单元测试 (`window::tests`; 设置优先级、旧设置读取、自启迁移决策)

## Notes
