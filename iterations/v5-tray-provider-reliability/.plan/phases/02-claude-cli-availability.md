# Phase 02 — Claude Code CLI 可用性

**Status**: `completed`
**目标**: 仅在 Claude Code CLI 的认证确实可用于读取配额时提供监控，并对不可用状态给出不误导的说明。
**前置**: Phase 01 完成并由用户验收。

## 验收判据

- 仅登录 Claude Desktop、未登录 Claude Code CLI 时，不再出现只有 `!` 的假可用状态
- 不读取 Claude Desktop 的私有凭据，也不要求用户登录 Claude Code CLI
- Claude Code CLI 已登录且凭据可用时，现有用量监控继续正常工作
- 不影响 Codex 与 Antigravity 的独立轮询和显示

## Tasks

- [x] 确认可用性呈现方案并由用户拍板（2026-07-11：无 CLI 凭据时自动关闭并禁用菜单项）
- [x] 实现 Claude Code CLI 凭据与配置目录检测（`src/poller.rs:215-217,646-659`; 支持 `CLAUDE_CONFIG_DIR`）
- [x] 增加明确状态、诊断日志和回归测试（`src/window.rs:395-432,526-539,3355-3364,3999-4012,4127-4140`; `cargo test`: 24 passed）
- [x] 构建安装并由用户验收（`cargo build --release`; installed SHA256 `89BCFA7F5468DFCDE5C353B0632F2AFAC78E531DD681E6DB5DB0C356F72941AA`; 用户于 2026-07-11 确认保持本方案并发布）

## Notes

- 用户明确说明只登录 Claude 桌面客户端，不希望在终端登录 Claude Code。
- 官方文档表明 Claude Code CLI 在 Windows 使用独立的 `.credentials.json`，因此不能将 Claude Desktop 登录等同于 CLI 登录。
- 自测结果：格式检查、24 项测试、发布构建和 `git diff --check` 通过；Clippy 保持 9 条既有风格警告。
- 本机运行验证：旧设置已自动收敛为 `show_claude_code=false`、`show_codex=true`、`show_antigravity=false`，没有触发 Claude Code 登录。
- 用户验收：2026-07-11 确认保持 Phase 02 方案并提交、推送、合并。
- 建议提交信息：`fix: disable unavailable Claude Code monitoring`。
