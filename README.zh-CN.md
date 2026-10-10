![Windows](https://img.shields.io/badge/platform-Windows-blue)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

[English](README.md) | **简体中文**

# Codex Usage

<img src=".github/codex-usage-icon.png" alt="Codex Usage 图标" width="96" height="96">

![运行效果](.github/animation.gif)

这张动图展示的是早期分段进度条。当前版本可在**外观**中选择分段或连续进度条。

一款轻量级的 Windows 原生任务栏小组件，用于监控 Codex 用量，并可选择同时显示 Claude Code 和 Google Antigravity 用量。

它常驻任务栏，无需打开 Codex 应用或账户用量页面，就能随时查看 Codex 用量窗口还剩多少。

## 功能亮点

- 显示当前 Codex **5 小时**用量窗口
- 显示当前**每周**用量窗口
- 支持简体中文界面，明确显示剩余用量和重置倒计时
- 可同时显示 Claude Code 用量
- 可显示 Google 5 小时及每周 Gemini 配额窗口中的 Antigravity 模型用量
- 实时显示各项限额的重置倒计时
- 可在剩余配额为 10%、20% 或 30% 时发出提醒；每个重置窗口只提醒一次
- 可分别控制 5 小时和每周用量行的显示
- 小巧的原生组件，直接嵌入 Windows 任务栏
- 使用一个与桌面应用一致的系统托盘图标
- 左键单击托盘图标可显示或隐藏任务栏组件
- 右键菜单支持刷新、监控服务、用量行、配额提醒、更新频率、语言、开机启动、组件显示和软件更新等设置
- 支持多显示器任务栏，可将组件放到指定屏幕的任务栏中
- 可调整配色、进度条样式、进度条粗细和文字大小
- 每个服务列显示 Claude 或 OpenAI 标志，Codex 使用绿色
- 服务没有返回的额度窗口显示 `--` 和空进度条，不再误显示剩余 100%
- 各服务独立刷新和重试；刷新失败时保留上次的数字并标记 `*`
- 鼠标悬停在服务列上，可查看上次成功更新时间、下次刷新时间和错误原因
- 电脑从睡眠中唤醒或网络恢复后自动刷新

## 适用人群

本应用适合已经在 Windows 上**安装并登录 Codex CLI 或 Codex 应用**的用户。

Codex 默认启用，应用会读取 Codex 使用的同一份本地登录凭据。

Antigravity 也是可选功能。若要显示其用量，请安装并登录 Google Antigravity，然后在右键菜单的**监控服务**中启用 **Antigravity**。

如果你希望始终看到自己离用量上限还有多远，而不必反复打开其他页面，这款工具会很合适。

## 系统要求

- Windows 10 或 Windows 11
- 已安装并完成身份验证的 Codex CLI 或 Codex 应用
- 可选：已安装并完成身份验证的 Claude Code
- 可选：若需查看 Antigravity 用量，需安装并登录 Google Antigravity

网络恢复后自动刷新需要 Windows 10 2004 或更高版本。在更早的 Windows 10 上，应用照常运行，但只在睡眠唤醒后和按正常周期刷新。

通过 WSL 使用 Claude Code 也受支持。监控器可以从 Windows 或 WSL 环境读取 Claude Code 凭据。

## 安装

如需按用户安装，请从[最新版本](https://github.com/upstream-ray/codex-usage-monitor/releases/latest)下载 `install.ps1`，然后运行：

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\install.ps1
```

安装程序会校验发布文件的 SHA256，并在无需管理员权限的情况下安装到 `%LOCALAPPDATA%\Programs\CodexUsage`。它还会创建开始菜单快捷方式，并在 Windows“已安装的应用”中添加卸载项。

如需便携使用，可从同一版本页面下载 `codex-usage.exe`，放在任意具有写入权限的目录中运行。你也可以在本地构建：

```powershell
cargo build --release
```

本地构建的可执行文件位于 `target\release\codex-usage.exe`。

## 卸载

可在 Windows“设置”>“应用”>“已安装的应用”中卸载 **Codex Usage**，或运行：

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$env:LOCALAPPDATA\Programs\CodexUsage\uninstall.ps1"
```

卸载时会保留 `%APPDATA%\CodexUsage\settings.json`。如需同时删除设置，请显式添加 `-RemoveSettings`。有关升级、便携版和开机启动的说明，请参阅[安装机制](docs/installation.md)。

## 使用方法

运行：

```powershell
codex-usage
```

启动后，它会出现在任务栏和通知区域的系统托盘中。

- 拖动左侧分隔线可移动任务栏组件
- 选择**设置 → 固定在任务栏最左侧**，可让组件固定在任务栏左端，而不是托盘旁边
- 在多显示器环境中，右键选择**设置 → 显示器**指定任务栏，也可将组件拖到另一个任务栏。选择会保存；断开所选显示器时暂用主任务栏，重新连接后自动返回。
- 右键单击任务栏组件或托盘图标，可设置刷新、监控服务、用量行、配额提醒、更新频率、开机启动、重置位置、语言、软件更新和退出
- 左键单击托盘图标可显示或隐藏任务栏组件
- 如需登录 Windows 后自动运行，请在右键菜单中启用“开机启动”
- 在**外观**菜单中，可选择 Codex 颜色（绿色、中性、蓝色或紫色）并开关**显示服务 Logo**。中性色在深色主题下为白色、浅色主题下为深灰；隐藏 Logo 也会收回其占用的空间。选择会保存，旧设置继续保留绿色和 Logo。

### 监控服务

通过右键菜单中的**监控服务**选择组件要显示的独立服务。这些服务不是互斥模型，因此可以同时监控多个账户：

- **Codex** 默认启用
- **Claude Code** 在已安装并登录 Claude Code CLI 时，可与 Codex 同时显示或单独显示
- **Antigravity** 可与其他提供商同时显示，也可作为独立服务列单独显示

显示多个服务时，每个服务都有自己的用量条和对应的文字颜色。Antigravity 会优先使用 Google 的 Gemini 配额摘要，必要时回退到模型配额数据。

Claude 桌面客户端与 Claude Code CLI 使用相互独立的本地登录状态。只登录 Claude 桌面客户端不会启用 Claude Code 监控。检测不到受支持的 Claude Code CLI 凭据时，菜单会以禁用状态显示 **Claude Code（需登录 CLI）**，并自动关闭该服务。

### 系统托盘图标

无论启用了多少服务，应用始终只显示一个托盘图标，并使用与可执行文件及桌面快捷方式相同的内嵌图标。

将鼠标悬停在托盘图标上，会显示所有已启用服务的紧凑用量摘要。左键单击可显示或隐藏任务栏组件，右键单击可打开设置菜单。

### 用量显示与提醒

在右键菜单的**用量显示**中，可同时显示两个配额周期，也可只显示其中一个。应用始终会保留至少一行。

在**配额提醒**中，可选择剩余配额为 10%、20% 或 30% 时提醒。提醒默认关闭。每个提供商的每个配额窗口只会提醒一次，直到重置时间发生变化；即使应用重启，也不会重复提醒。

简体中文界面的紧凑任务栏行使用 `5h` / `7d`、剩余百分比，以及 `18:30重置` 或 `07/17重置` 这样的具体本地重置时间。

百分比使用固定宽度的数字列，数值从 `9%` 变成 `10%` 时，重置时间的位置不会移动。

### 刷新与状态

每个已启用的服务分别保存自己的数据、上次成功更新时间、错误和重试计划。一个服务变慢或出错，不会拖慢或覆盖其他服务的数字。

| 显示 | 含义 |
| --- | --- |
| `--` 和空进度条 | 服务没有返回这个额度窗口。例如，部分 Codex 账户只有每周额度。 |
| 重置时间后的 `*` | 最近一次刷新失败，或数据已过时。显示的是上次成功刷新的数字。 |
| `!`、`网络`、`限流`、`服务`、`错误` | 应用启动后该服务还没有成功返回过数据，文字说明原因。 |

鼠标悬停在服务列上，可查看上次成功更新时间、下次刷新或重试时间，以及最近的错误。Codex 缺少某个额度窗口时，也会显示简短说明。

刷新规则：

- **更新频率**设置正常刷新间隔。**剩余 ≤20% 时每分钟刷新**默认开启：某个服务的任一额度窗口剩余不超过 20% 时，该服务改为每分钟刷新一次。额度已经用完的服务保持正常间隔，因为重置前它不会再消耗额度。
- 网络和服务器错误会按逐渐变长的间隔重试。服务器返回 `429` 并带有 `Retry-After` 时，应用会等到该时间再请求。
- 登录过期或找不到凭据时，应用停止重试该服务，直到凭据文件发生变化，或你手动选择**刷新**。
- 额度窗口刚重置后，应用会很快再查一次，让新窗口尽快显示。

### 自定义外观

右键任务栏组件或托盘图标，打开**外观**菜单，可以调整：

- **文字与颜色**：跟随系统、高对比深色、高对比浅色。高对比模式使用实色背景和加粗文字，避免半透明任务栏的壁纸降低可读性。
- **进度条样式**：连续圆角或分段（接近项目早期动图的样式）。
- **进度条粗细**：标准或细。
- **文字大小**：标准或较大。

对于深色半透明、约 40 像素高的 Windows 任务栏，可直接选**推荐：半透明深色任务栏**。该预设使用高对比深色、细连续圆角进度条和较大文字。组件高度会限制在当前任务栏高度内，避免两行内容被裁切。所有外观选项保存在 `settings.json` 的 `appearance` 字段中；旧配置会沿用原来的外观。

## 诊断

如需排查启动或显示问题，请运行：

```powershell
codex-usage --diagnose
```

日志将写入：

```text
%TEMP%\codex-usage.log
```

日志会记录应用版本、安装渠道、可执行文件路径、轮询失败类别和重试时间，但不会记录访问令牌或凭据内容。有关任务栏错误标签及恢复步骤，请参阅[故障排除](docs/troubleshooting.md)。

设置保存在：

```text
%APPDATA%\CodexUsage\settings.json
```

## 账户支持

Codex 用量来自本地 Codex 安装中已登录的账户。可选的 Claude Code 监控功能支持 Claude Code 本身所支持的账户类型。

截至 **2026 年 3 月 19 日**，Anthropic 的 Claude Code 设置文档说明：

- **支持：** Pro、Max、Teams、Enterprise 和 Console 账户
- **不支持：** 免费 Claude.ai 方案

如果 Anthropic 以后调整 Claude Code 的可用范围，只要用量数据仍通过相同的身份验证端点提供，本应用会跟随 Claude Code 的支持范围。

## 隐私与安全

本项目是**开源软件**，你可以直接检查它的具体行为。

应用读取的内容：

- `~/.claude/.credentials.json` 中的本地 Claude Code OAuth 凭据
- 如果设置了 `CLAUDE_CONFIG_DIR`，读取该目录中的 Claude Code 凭据文件
- 必要时，已安装 WSL 发行版中的同一凭据文件
- 启用 Codex 时，读取 `$CODEX_HOME/auth.json` 或 `~/.codex/auth.json` 中的本地 Codex 凭据
- 启用 Antigravity 时，读取 Windows 凭据管理器中目标为 `gemini:antigravity` 的本地 OAuth 令牌

应用通过网络发送的请求：

- 请求 Anthropic 的 Claude 端点，以读取用量和速率限制信息
- 启用 Codex 时，请求 ChatGPT 的 Codex 用量端点，以读取 Codex 用量和速率限制信息
- 启用 Antigravity 时，请求 Google Cloud Code / Antigravity 端点，以读取 Antigravity 配额信息
- 仅在使用软件更新检查或自更新功能时请求 GitHub
- 如果设置了 `HTTPS_PROXY`、`HTTP_PROXY` 或 `ALL_PROXY` 等代理环境变量，上述出站请求可能通过代理发送

应用在本地保存的内容：

- 组件位置
- 选中的任务栏或屏幕
- 组件显示状态
- 轮询频率
- 语言偏好
- 上次更新检查时间
- 可见的配额行和低配额提醒阈值
- 用于避免重复提醒的配额窗口通知键
- 显示模型偏好
- 外观、所选显示器、左侧固定和加快刷新设置

应用**不会**执行的操作：

- 不会将凭据发送到其他服务器
- 不使用独立的后端服务
- 不收集分析数据或遥测信息
- 不上传项目文件
- 不直接修改 Codex 凭据文件
- 不读取或复用 Claude 桌面客户端的认证数据

注意：

- 如果 Claude Code 令牌过期，应用可能会在后台调用本地 Claude CLI 进行刷新
- 如果 Codex 令牌过期，应用可能会在后台调用本地 Codex CLI 进行刷新。监控器本身不会写入 `auth.json`，任何凭据更新都由 Codex CLI 完成
- 如果 Antigravity 令牌过期，请打开 Antigravity 并重新登录。监控器不会写入 Windows 凭据管理器
- 便携版可以从本仓库下载最新版本进行自更新
- 代理必须可信，因为代理转发的用量请求会在 TLS 连接内包含 OAuth Bearer 令牌

## 工作原理

监控器会：

1. 查找已启用模型的登录凭据
2. 从 Anthropic、ChatGPT 和/或 Google Antigravity 端点读取当前用量
3. 将结果直接显示在 Windows 任务栏中
4. 保持组件与所选任务栏及托盘区域对齐
5. 在后台定期刷新

如果较新的用量端点不可用，应用可以回退为读取 Claude Messages API 返回的速率限制响应头。

## 开源说明

本项目采用 MIT License。原始 [LICENSE](LICENSE) 及版权声明均予以保留。

Claude 和 OpenAI 标志来自 [Lobe Icons](https://github.com/lobehub/lobe-icons)，采用 MIT License，详见 [src/icons/providers](src/icons/providers)。品牌名称和标志归各自所有者所有。

Codex Usage 是 [CodeZeno/Claude-Code-Usage-Monitor](https://github.com/CodeZeno/Claude-Code-Usage-Monitor) 的持续维护衍生版本。感谢 Craig Constable 和上游贡献者创建原始项目。本仓库中的修改与上游维护者或 OpenAI 不存在隶属或背书关系。

如果你想检查程序行为或审核代码，仓库中提供了全部源码。
