# 统一托盘身份并消除服务选择误解

> 文件位置：`iterations/v5-tray-provider-reliability/.plan/plan.md`
> 配套 skill：first-flight-phases

## 背景

当前程序为每个已启用的用量服务创建一个百分比托盘图标，与桌面应用图标不一致，也让用户误以为同时运行了多个应用。右键菜单使用“显示模型”命名，但 Codex、Claude Code 和 Antigravity 实际是独立的用量服务。Claude 桌面客户端登录也不等于 Claude Code CLI 登录，不能据此读取 Claude Code 配额。

## 范围

**做：**

- 将多个动态百分比托盘图标统一为一个 Codex Usage 应用图标
- 在单个托盘提示中汇总已启用服务的信息
- 将简体中文“显示模型”改为“监控服务”，同步中英文说明
- 后续识别 Claude Code CLI 的真实可用状态，避免把 Claude Desktop 登录误认为 Claude Code 可监控
- 将已验收实现正式发布为 `v1.9.0`，校验线上资产并完成本机升级

**不做：**

- 不读取或复用 Claude 桌面客户端的私有认证数据
- 不要求用户为了使用 Codex 监控而登录 Claude Code CLI
- 不改变 Codex、Claude Code 或 Antigravity 的配额计算口径

## 阶段总览

| # | 阶段 slug | 一句话目标 | 状态 |
|---|---|---|---|
| 01 | single-tray-identity | 使用单个应用图标并澄清“监控服务”概念 | completed |
| 02 | claude-cli-availability | 仅在 Claude Code CLI 凭据真实可用时提供监控并给出明确状态 | completed |
| 03 | v1.9.0-release | 将已验收功能作为正式版本发布并完成交付清理 | completed |

## 关键决策

- **2026-07-11**：托盘代表 Codex Usage 应用本身，因此无论启用多少服务都只显示一个嵌入式应用图标。
- **2026-07-11**：Codex、Claude Code、Antigravity 是可并列监控的服务，不使用互斥单选。
- **2026-07-11**：Claude Desktop 与 Claude Code CLI 登录相互独立；不尝试读取 Claude Desktop 凭据。
- **2026-07-11**：按用户确认先完成并验收 Phase 01，再实施 Phase 02。
- **2026-07-11**：托盘身份与认证边界属于用户可见行为升级，使用次版本号 `v1.9.0` 发布。

## Open Questions

- [x] Phase 02 中不可用的 Claude Code 保留为禁用菜单项并标注“需登录 CLI”；旧设置自动关闭 Claude Code——用户于 2026-07-11 确认
