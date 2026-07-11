# Phase 04 — 安装、升级与卸载

**Status**: `in progress`
**目标**: 让普通 Windows 用户能够可靠安装、升级和卸载 Codex Usage。
**前置**: Phase 03

## 验收判据

- 全新安装、覆盖升级和卸载路径均可重复验证。
- 设置迁移、自启项和用户数据保留策略明确且符合预期。
- Release 提供校验信息，安装产物来源和版本可追溯。
- WinGet 仅在真实包与发布条件具备后启用。

## Tasks

- [x] 设计安装目录、便携模式、设置保留和卸载策略 (`docs/installation.md`)
- [x] 实现并验证安装与卸载入口 (`scripts/install.ps1`, `scripts/uninstall.ps1`; per-user install、Installed Apps、开始菜单、普通卸载保留设置)
- [x] 将安装产物和 SHA256 接入 Release workflow (`.github/workflows/release.yml`: EXE、SHA256、install.ps1、uninstall.ps1)
- [x] 验证旧版升级、全新安装、卸载与回滚 (1.5.0→1.6.0；设置 SHA256 保持 `5ED392E3FCBEBFBE34CB57F8253034076762E1DF3A32CCEE2F5294E2A20DC001`；故障注入退出 1 并回滚到 1.5.0；最终 PID 34756 运行 1.6.0)
- [~] 配置并验证 WinGet 发布路径
- [ ] 提交并推送安装升级版本

## Notes

- 官方 WinGet 社区仓库不接受脚本作为 installer；PowerShell 脚本用于直接安装体验，WinGet 路径使用 release 单文件 EXE 的 `portable` manifest。
- 正式安装位置为 `%LOCALAPPDATA%\Programs\CodexUsage`；测试结束后已保留 1.6.0 安装并将自启路径修复到稳定安装位置。
