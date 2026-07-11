# 将 Codex Usage 从本地定制版推进为可公开维护的 Windows 工具

## 背景

当前版本已完成简体中文体验和 Codex Usage 品牌迁移，但全部改动仍未提交，发布元数据仍部分指向上游仓库，也没有属于维护者的公开发布渠道。下一阶段需要先固化可靠基线，再依次完成公开仓库、视觉身份、安装升级、可靠性和产品功能，使项目能够独立、合规地持续维护。

## 范围

**做：**

- 审查并提交当前定制版本，建立可追溯基线
- 创建公开 GitHub 仓库并配置 Release、更新地址和开源归属
- 更新产品图标与展示素材，验证常见 Windows 显示环境
- 建立可安装、升级、卸载和后续接入 WinGet 的发布路径
- 增强错误处理、更新安全性、长期运行可靠性和测试
- 增加额度提醒、显示选项与详细用量信息等实用功能

**不做：**

- 不删除或弱化上游 MIT LICENSE、原作者版权及派生项目说明
- 不向上游 `CodeZeno/Claude-Code-Usage-Monitor` 推送定制提交
- 不引入 Electron、WebView 或其他重量级运行时
- 不擅自创建付费服务、遥测或收集用户凭据

## 阶段总览

| # | 阶段 slug | 一句话目标 | 状态 |
|---|---|---|---|
| 01 | baseline-review | 审查、验证并提交当前定制基线 | completed |
| 02 | public-release | 建立合规的公开仓库与独立发布渠道 | completed |
| 03 | product-visuals | 建立新的产品图标和一致视觉身份 | completed |
| 04 | install-upgrade | 提供可靠的安装、升级与卸载体验 | completed |
| 05 | reliability | 强化错误处理、更新安全和长期运行稳定性 | completed |
| 06 | usage-features | 增加额度提醒和更灵活的用量显示能力 | not started |

## 关键决策

- **2026-07-11**：六个方向按依赖顺序串行实施，每个 phase 独立验收后再进入下一阶段。
- **2026-07-11**：公开仓库作为上游项目的 MIT 派生版本，保留原 LICENSE 和版权，并在 README 明确归属。
- **2026-07-11**：继续坚持纯 Rust + Win32 API 的轻量架构，不为产品化引入 Web 运行时。
- **2026-07-11**：在自有发布渠道建立前，不触发或伪装成已存在的 WinGet 正式包。
- **2026-07-11**：用户明确授权本轮由 Codex 创建分支、提交、创建公开仓库并推送。

## Open Questions

- [x] GitHub 账户或组织归属、仓库最终 URL — `https://github.com/upstream-ray/codex-usage-monitor`
- [x] 正式 WinGet 包首次提交时机 — v1.6.0 manifest 已验证；向 `microsoft/winget-pkgs` 发起公共 PR 需用户单独确认
- [x] 新图标最终方案 — 采用 A“C 形额度环 + 三条剩余量刻度”，青蓝/青柠/深海军蓝配色
