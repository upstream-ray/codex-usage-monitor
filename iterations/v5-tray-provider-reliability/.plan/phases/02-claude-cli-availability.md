# Phase 02 — Claude Code CLI 可用性

**Status**: `not started`
**目标**: 仅在 Claude Code CLI 的认证确实可用于读取配额时提供监控，并对不可用状态给出不误导的说明。
**前置**: Phase 01 完成并由用户验收。

## 验收判据

- 仅登录 Claude Desktop、未登录 Claude Code CLI 时，不再出现只有 `!` 的假可用状态
- 不读取 Claude Desktop 的私有凭据，也不要求用户登录 Claude Code CLI
- Claude Code CLI 已登录且凭据可用时，现有用量监控继续正常工作
- 不影响 Codex 与 Antigravity 的独立轮询和显示

## Tasks

- [ ] 确认可用性呈现方案并由用户拍板
- [ ] 实现 Claude Code CLI 凭据与配置目录检测
- [ ] 增加明确状态、诊断日志和回归测试
- [ ] 构建安装并由用户验收

## Notes

- 用户明确说明只登录 Claude 桌面客户端，不希望在终端登录 Claude Code。
- 官方文档表明 Claude Code CLI 在 Windows 使用独立的 `.credentials.json`，因此不能将 Claude Desktop 登录等同于 CLI 登录。

