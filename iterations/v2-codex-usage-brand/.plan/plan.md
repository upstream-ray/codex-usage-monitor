# 将派生工具完整迁移为 Codex Usage

## 背景

当前程序已经完成简体中文体验，但包名、EXE、Windows 元数据、设置目录和开机自启仍使用 Claude Code Usage Monitor。若只修改标题，会造成配置丢失、重复自启和更新回原版的风险。本迭代完成完整品牌迁移并保留现有用户设置。

## 范围

**做：**

- 统一产品名、包名、EXE、日志、窗口类和发布产物为 Codex Usage
- 将设置从 `ClaudeCodeUsageMonitor` 无损迁移到 `CodexUsage`
- 将注册表自启项从 `ClaudeCodeUsageMonitor` 迁移到 `CodexUsage`
- 更新 README、发布流程和更新资源名称
- 构建并运行验证 `codex-usage.exe`

**不做：**

- 不删除 MIT LICENSE 或原作者版权声明
- 不新增安装器、WinGet 正式发布或 GitHub Fork 仓库
- 不更换应用图标

## 阶段总览

| # | 阶段 slug | 一句话目标 | 状态 |
|---|---|---|---|
| 01 | product-identity | 统一源码、元数据与发布产物名称 | completed |
| 02 | settings-startup-migration | 无损迁移设置目录与开机自启项 | completed |
| 03 | verify-rebranded-build | 构建并运行验证 Codex Usage | completed |

## 关键决策

- **2026-07-11**：新包名和 EXE 使用 `codex-usage`，展示名使用 `Codex Usage`。
- **2026-07-11**：保留旧设置目录作为回滚副本，新程序首次启动时复制并继续使用新目录。
- **2026-07-11**：只有检测到旧自启项时才迁移，避免为未启用自启的用户擅自开启。
- **2026-07-11**：更新器只匹配 `codex-usage.exe`，不再回退下载上游任意 EXE，避免覆盖为原版。

## Open Questions

- 无。正式发布仓库和 WinGet 包留待后续配置。
