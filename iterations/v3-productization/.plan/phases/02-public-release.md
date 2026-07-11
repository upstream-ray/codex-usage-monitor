# Phase 02 — 公开仓库与独立发布渠道

**Status**: `in progress`
**目标**: 在用户 GitHub 账户下建立公开的 `codex-usage-monitor` 仓库，并让源码、Release 和更新检查指向该仓库。
**前置**: Phase 01

## 验收判据

- 公开仓库 `codex-usage-monitor` 创建成功且默认分支可访问。
- LICENSE、原作者版权、上游链接和派生说明清晰保留。
- Cargo、README、更新器和 Release workflow 均指向真实的新仓库与 `codex-usage.exe`。
- 基线提交成功推送，Release 流程至少通过静态检查或受控测试。

## Tasks

- [x] 确认 GitHub 登录身份并创建公开仓库 (`gh auth status`; public repo `https://github.com/upstream-ray/codex-usage-monitor`)
- [x] 配置独立远程且保留上游远程用于同步 (`origin=git@github.com:upstream-ray/codex-usage-monitor.git`; `upstream=https://github.com/CodeZeno/Claude-Code-Usage-Monitor.git`)
- [x] 更新仓库元数据、README、更新器与发布工作流 (`Cargo.toml`: repository/homepage 指向新仓库、version `1.5.0`; `README.md`: Release 下载与上游归属；`src/updater.rs`: 从 Cargo repository 派生 API 地址；`.github/workflows/release.yml`: `codex-usage.exe`)
- [x] 验证 MIT 许可、原作者归属和派生说明 (`LICENSE`: 原文保留 Copyright 2025 Craig Constable；`README.md`: 上游链接、感谢、非关联声明；`cargo test`: 12 passed)
- [ ] 推送基线分支并验证线上仓库内容
- [ ] 创建并验证首个受控 GitHub Release

## Notes

- 首个派生版本使用 `1.5.0`，延续上游 `1.4.8` 之后的版本顺序，避免已安装 `1.4.8` 用户被语义降级。
