![Windows](https://img.shields.io/badge/platform-Windows-blue)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

**English** | [简体中文](README.zh-CN.md)

# Codex Usage

<img src=".github/codex-usage-icon.png" alt="Codex Usage icon" width="96" height="96">

![Screenshot](.github/animation.gif)

This animation shows the original segmented-bar design. Current builds let you choose segmented or continuous bars in **Appearance**.

A lightweight native Windows taskbar widget for monitoring Codex usage, with optional Claude Code and Google Antigravity usage display.

It sits in your taskbar and shows how much of your Codex usage window remains without opening the Codex app or account usage page.

## What You Get

- A **5h** bar for your current Codex usage window
- A **7d** bar for your current weekly window
- Simplified Chinese display with explicit remaining usage and reset countdowns
- Optional Claude Code usage alongside Codex
- Optional Antigravity model usage bars for Google's 5-hour and weekly Gemini quota windows
- A live countdown until each limit resets
- Optional low-quota alerts at 10%, 20%, or 30% remaining, deduplicated per reset window
- Independent display controls for the 5-hour and weekly rows
- A small native widget that lives directly in the Windows taskbar
- One system tray icon that matches the desktop app icon
- Left-click the tray icon to toggle the taskbar widget on or off
- Right-click options for refresh, monitored services, usage rows, quota alerts, update frequency, language, startup, widget visibility, and updates
- Multi-monitor taskbar placement, so the widget can live on the taskbar for the screen you prefer
- Appearance options for palette, bar style, bar thickness, and text size
- Claude and OpenAI marks for each service column, with Codex shown in green
- Quota windows that a service does not report show `--` and an empty bar instead of a false 100%
- Each service refreshes and retries on its own; a failed refresh keeps the last numbers and marks them `*`
- Hover over a service column to see its last successful update, next refresh, and any error
- Automatic refresh after the computer wakes from sleep or the network comes back
- Passive monitoring by default, with authentication recovery handled by the provider's sign-in flow
- Optional managed controls that enforce passive monitoring and disable software updates

## Who This Is For

This app is for Windows users with a **Codex ChatGPT sign-in stored in a supported local credential file**. See [Account Support](#account-support) before installing.

Codex is enabled by default. The monitor reuses the supported credential file; it does not provide its own sign-in flow.

Antigravity support is optional too. To show Antigravity usage, install and sign in to Google Antigravity, then enable the **Antigravity** service from the right-click **Monitored services** menu.

It works best if you want a simple "how close am I to the limit?" display that is always visible.

## Requirements

- Windows 10 or Windows 11
- A Codex CLI or app ChatGPT session using the [supported file-based authentication](#account-support)
- Optional: Claude Code installed and authenticated
- Optional: Google Antigravity installed and authenticated, if you want Antigravity usage

Refresh after network recovery needs Windows 10 version 2004 or later. On earlier Windows 10 builds the app still runs, but it only refreshes after wake from sleep and on its normal schedule.

If you use Claude Code through WSL, that is supported too. The monitor can read your Claude Code credentials from Windows or from your WSL environment.

## Install

For a per-user installation, download `install.ps1` from the [latest release](https://github.com/upstream-ray/codex-usage-monitor/releases/latest), verify it using the [release verification guide](docs/release-verification.md), then run:

```powershell
powershell.exe -NoProfile -File .\install.ps1
```

The installer verifies SHA256 checksums for the executable and uninstall helper and installs to `%LOCALAPPDATA%\Programs\CodexUsage` without administrator access. It adds a Start menu shortcut and an entry in Windows Installed Apps. It respects the machine's PowerShell execution policy.

For a company-managed account, use the [managed installation profile](docs/installation.md#managed-installation) with a locally approved executable and hash. It enables passive monitoring and disables updates before first launch. Administrators can enforce these choices using [machine policy](docs/managed-policy.md).

For portable use, download `codex-usage.exe` from the same release and run it from any user-writable directory. You can also build it locally:

```powershell
cargo build --release --locked
```

Local builds create the executable at `target\release\codex-usage.exe`.

## Uninstall

Uninstall **Codex Usage** from Windows Settings > Apps > Installed apps, or run:

```powershell
powershell.exe -NoProfile -File "$env:LOCALAPPDATA\Programs\CodexUsage\uninstall.ps1"
```

Uninstalling preserves `%APPDATA%\CodexUsage\settings.json`. Add `-RemoveSettings` to delete settings explicitly. See [Installation model](docs/installation.md) for upgrade, portable, and startup behavior.

## Use

Run:

```powershell
codex-usage
```

Once running, it will appear in your taskbar and as one tray icon in the notification area.

- Drag the left divider to move the taskbar widget
- Choose **Settings → Pin to the left edge of the taskbar** to keep the widget at the left edge instead of next to the tray
- On multi-monitor setups, choose **Settings → Display** from the right-click menu or drag the widget onto another taskbar. The selection is saved; the primary taskbar is used while the selected display is disconnected, and the widget returns when it reconnects.
- A rejected display change keeps the previous taskbar and shows an error. Automatic placement accounts for notification-area and secondary-taskbar clock controls; see [placement troubleshooting](docs/troubleshooting.md#widget-disappears-after-a-display-change-or-covers-the-clock) if a taskbar layout still needs adjustment.
- Right-click the taskbar widget or tray icon for refresh, monitored services, usage rows, quota alerts, update frequency, Start with Windows, reset position, language, updates, and exit
- Left-click the tray icon to toggle the taskbar widget on or off
- Enable `Start with Windows` from the right-click menu if you want it to launch automatically when you sign in

### Monitored Services

Use the right-click **Monitored services** menu to choose which independent services the widget displays. The services are not mutually exclusive, so you can monitor more than one account at the same time:

- **Codex** is enabled by default
- **Claude Code** can be enabled alongside Codex or shown by itself
- **Antigravity** can be enabled alongside the other providers or shown by itself as its own service column

When multiple services are shown, each service has its own usage bar and matching usage text color. Antigravity prefers Google's Gemini quota summary when available and falls back to model quota data when needed.

Provider choices are loaded before credential discovery. Disabled services do not have their credential files, credential-store entries, or WSL credentials probed. You can leave every service off; that choice persists and no usage polling runs until you enable one.

Claude Desktop and Claude Code CLI use separate local sessions. Signing in to Claude Desktop does not enable Claude Code monitoring. Claude Code remains selectable without a CLI login. If an enabled service has no valid credentials, its column shows `!` with sign-in guidance; the monitor does not silently disable it. Sign in through the provider's own app or CLI, then choose **Refresh**.

### Passive Monitoring And Updates

**Settings → Passive monitoring** is on by default, including when existing settings have no explicit value. The monitor reads usage using enabled providers' existing credentials. It does not run model prompts to recover authentication or obtain fallback rate-limit headers. Missing, expired, or rejected credentials produce a sign-in message and suspend that service's automatic retries until credentials change or you explicitly refresh.

**Settings → Allow updates** is on by default for normal installations. When enabled, the app automatically checks GitHub for a new release approximately once a day and allows manual checks and installation. Turn it off to block update checks, downloads, WinGet launches, and staged self-updates. The installed version remains visible. The [managed installer](docs/installation.md#managed-installation) turns this setting off before launch.

Both preferences persist in `%APPDATA%\CodexUsage\settings.json`. Changes made in the menu take effect at the next relevant operation. An administrator can [enforce passive monitoring and disable updates](docs/managed-policy.md); enforced menu items show **Managed by your organization** and cannot be loosened in the app.

Turning passive monitoring off explicitly enables legacy behavior: recovery can run `codex exec .` or `claude -p .`, and Claude's fallback can submit a Messages API request. These are actual model requests, may consume quota or incur charges, and can involve CLI hooks and the current project's configuration. Keep passive monitoring on when this behavior is not approved.

### System Tray Icon

The app always shows one tray icon using the same embedded icon as the executable and desktop shortcut, regardless of how many services are enabled.

Hovering over the tray icon shows a compact summary for all enabled services. Left-clicking it toggles the taskbar widget; right-clicking it opens the settings menu.

### Usage Display And Alerts

Use the right-click **Usage display** menu to show both quota rows or only one. The app always keeps at least one row visible.

Use **Quota alerts** to choose a remaining-quota threshold of 10%, 20%, or 30%. Alerts are off by default. Each provider and quota window is notified only once until its reset time changes, including across app restarts.

In Simplified Chinese, the compact taskbar rows use `5h` / `7d`, remaining percentage, and a concrete local reset value such as `18:30重置` or `07/17重置`.

Percentages use fixed digit columns, so the reset text does not shift when a value changes from `9%` to `10%`.

### Refresh And Status

Each enabled service keeps its own data, last successful update, error, and retry schedule. A slow or failing service does not delay or replace the numbers of another service.

| Display | Meaning |
| --- | --- |
| `--` and an empty bar | The service did not report this quota window. For example, some Codex accounts only have a weekly limit. |
| `*` after the reset text | The latest refresh failed or the data is old. The numbers are from the last successful refresh. |
| `!`, `NET`, `429`, `5XX`, or `ERR` (Chinese: `!`, `网络`, `限流`, `服务`, `错误`) | The service has not returned data yet since the app started, and the label shows why. |

Hover over a service column to see when it last updated successfully, when it refreshes or retries next, and the latest error. Missing Codex windows also get a short explanation.

Refresh behavior:

- **Update frequency** sets the normal interval. **Refresh every minute when remaining ≤20%** is on by default. It refreshes a service every minute while one of its quota windows has 20% or less left. A service with a used-up window keeps the normal interval, because it cannot use more quota until the reset.
- Network and server errors retry with increasing delays. When a server answers `429` with a `Retry-After` time, the app waits until that time.
- In passive mode, when a login expires, credentials are missing, or a provider rejects them, the app stops retrying that service until credentials change or you choose **Refresh**. Repeated wake or network-recovery events do not resume a blocked login.
- Shortly after a quota window resets, the app checks again so the new window shows quickly.

### Appearance

Right-click the widget or tray icon and open **Appearance** to choose a system, high-contrast dark, or high-contrast light palette; continuous or segmented bars; standard or slim bars; and standard or larger text. High-contrast palettes add a solid backdrop and bold text for readability over translucent taskbars. The **Recommended: translucent dark taskbar** action applies high-contrast dark colors, slim continuous rounded bars, and larger text. The widget also fits its height to the selected taskbar, avoiding clipped rows on 40-pixel taskbars. Appearance choices are saved in `settings.json`; existing settings keep their previous appearance until changed.

## Diagnostics

If you need to troubleshoot startup or visibility issues, run:

```powershell
codex-usage --diagnose
```

This writes a log file to:

```text
%TEMP%\codex-usage.log
```

The log records the application version, install channel, executable path, polling failure category, and retry timing. It can also include local paths, launch arguments, and WSL distribution names; review it before sharing. See [Troubleshooting](docs/troubleshooting.md) for the taskbar error labels and recovery steps.

Settings are saved to:

```text
%APPDATA%\CodexUsage\settings.json
```

## Account Support

Codex monitoring currently supports **file-based ChatGPT authentication**. It reads the Windows user's `.codex\auth.json`, or `auth.json` under the `CODEX_HOME` inherited by the monitor process. The file must contain `tokens.access_token`; the monitor sends `tokens.account_id` when present. A `CODEX_HOME` set only in a terminal may not reach a monitor launched from the Start menu or at sign-in.

Codex keyring-only and ephemeral sessions, API-key authentication, and browser-only sign-ins are unsupported. Codex credentials in WSL are not discovered automatically. The monitor queries the fixed `https://chatgpt.com/backend-api/wham/usage` endpoint and does not read Codex's custom gateway/backend configuration. Company or SSO sessions can work when they provide the supported file and the workspace permits this endpoint; being signed in to Codex alone does not establish compatibility.

In passive mode, renewal is handled by the **official Codex app or CLI**. After it refreshes the stored session, or you sign in there again, choose **Refresh** in the monitor. Monitoring alone does not keep a login alive. Keep your organization's credential-storage and gateway requirements; do not export tokens or weaken those requirements to accommodate this version.

Optional Claude Code monitoring works with the account types supported by Claude Code.

As of **March 19, 2026**, Anthropic's Claude Code setup documentation says:

- **Supported:** Pro, Max, Teams, Enterprise, and Console accounts
- **Not supported:** the free Claude.ai plan

If Anthropic changes Claude Code availability in the future, this app should follow whatever Claude Code supports, as long as the usage data remains exposed through the same authenticated endpoints.

## Privacy And Security

This project is **open source**, so you can inspect exactly what it does.

What the app reads, only for services you have enabled:

- If Claude Code is enabled, its local OAuth credentials from `~/.claude/.credentials.json`, the directory set in `CLAUDE_CONFIG_DIR`, or a supported WSL environment
- If Codex is enabled, the supported Windows Codex credential file described in [Account Support](#account-support); the whole file is read into memory, while only its access token and optional account ID are used
- If Antigravity is enabled, your local Antigravity OAuth token from Windows Credential Manager target `gemini:antigravity`

What the app sends over the network:

- Requests to Anthropic's Claude usage endpoint, if Claude Code is enabled
- Requests to ChatGPT's Codex usage endpoint to read your Codex usage and rate-limit information, if Codex is enabled
- Requests to Google's Cloud Code / Antigravity endpoints to read your Antigravity quota information, if Antigravity is enabled
- Requests to GitHub for the automatic daily release check and manual update operations, only while **Allow updates** is enabled and machine policy permits it
- If proxy environment variables such as `HTTPS_PROXY`, `HTTP_PROXY`, or `ALL_PROXY` are set, those outbound requests may use that proxy

What the app stores locally:

- Widget position
- Selected taskbar / screen
- Widget visibility
- Polling frequency
- Language preference
- Last update check time
- Visible quota rows and low-quota alert threshold
- Quota-window notification keys used to prevent duplicate alerts
- Displayed model preferences
- Appearance, selected display, left-edge pinning, and the faster-refresh setting
- Passive-monitoring and software-update preferences

What it does **not** do:

- It does not send your credentials to any other server
- It does not use a separate backend service
- It does not collect analytics or telemetry
- In passive mode, it does not submit model prompts or invoke CLI inference to recover authentication or read usage
- It does not directly edit your Codex credentials file
- It does not read or reuse Claude Desktop authentication data

Notes:

- In passive mode, expired or rejected Claude Code and Codex credentials require sign-in through the provider's app or CLI. The monitor does not refresh them by running model prompts. Legacy CLI recovery and the Messages API fallback are available only after explicitly turning passive monitoring off, subject to machine policy.
- If your Antigravity token is expired, open Antigravity and sign in again. The monitor does not write Windows Credential Manager entries itself.
- The OAuth bearer tokens remain the provider's existing tokens with their original permissions. Passive mode restricts this monitor's behavior; it does not turn those tokens into read-only credentials or sandbox the executable.
- Portable installs can update themselves from this repository only when updates are allowed. Release checksums provide integrity; see [release verification](docs/release-verification.md) for commit-bound provenance and its limits. This project does not claim an Authenticode publisher signature.
- Proxies should be trusted because proxied usage requests include your OAuth bearer token inside the TLS connection

See [managed policy](docs/managed-policy.md) for the system policy path, failure behavior, deployment requirements, and limitations. Requests or external processes already started before a restriction changes cannot be undone.

## How It Works

The monitor:

1. Loads provider choices and privacy preferences, applies machine policy, then discovers only enabled providers' credentials
2. Reads your current usage from Anthropic, ChatGPT, and/or Google's Antigravity endpoints
3. Shows the result directly in the Windows taskbar
4. Keeps the widget aligned with the selected taskbar and tray area
5. Refreshes periodically in the background

In passive mode, an unavailable usage endpoint is reported without submitting a model request. The Claude Messages API fallback is available only when passive monitoring is explicitly disabled and machine policy allows it.

## Open Source

This project is licensed under the MIT License. The original [LICENSE](LICENSE) and copyright notice are preserved.

The Claude and OpenAI marks come from [Lobe Icons](https://github.com/lobehub/lobe-icons) under the MIT License. See [src/icons/providers](src/icons/providers). Brand names and marks belong to their owners.

Codex Usage is a maintained derivative of [CodeZeno/Claude-Code-Usage-Monitor](https://github.com/CodeZeno/Claude-Code-Usage-Monitor). Thanks to Craig Constable and the upstream contributors for the original project. Changes in this repository are not affiliated with or endorsed by the upstream maintainers or OpenAI.

If you want to inspect the behavior or audit the code, everything is in this repository.
