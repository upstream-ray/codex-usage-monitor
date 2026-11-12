# Troubleshooting Codex Usage

## Taskbar error labels

Codex Usage keeps authentication failures separate from transient service failures:

| Simplified Chinese | Other languages | Meaning | Recommended action |
|---|---|---|---|
| `!` | `!` | Enabled service credentials are missing or expired | Sign in with the relevant CLI/app, then refresh Codex Usage. Claude Desktop login does not count as Claude Code CLI login. |
| `网络` | `NET` | Network or TLS connection failed | Check connectivity, VPN, proxy, and firewall settings |
| `限流` | `429` | Provider rate limit | Wait for the provider retry window; Codex Usage retries with backoff |
| `服务` | `5XX` | Provider service failure | Wait and retry; check provider status if it persists |
| `错误` | `ERR` | Invalid or unsupported response | Enable diagnostics and inspect the log |
| `--` | `--` | No reading for this window, or the selected Antigravity quota group is not reported by the current source | For Antigravity, open Antigravity so its local quota summary is used, or choose **Auto** in **Monitored services > Antigravity quota group** |

Authentication failures pause provider polling until the credential source changes, preventing repeated login notifications. For Antigravity, starting Antigravity also ends that pause, because its local quota summary needs no Google token. Transient failures use exponential backoff up to the configured refresh interval.

## Antigravity shows a different group than its quota panel

While Antigravity is running, the monitor reads the quota summary that Antigravity's own panel shows, from Antigravity's language server on `127.0.0.1`. When Antigravity is closed, it uses Google's online quota service instead, which may report fewer groups for some accounts. With `--diagnose`, the log records which source answered (for example `Antigravity quota from its language server (loopback port …)`) and the reported group layout. The log never records the language server's access token.

This local source is an internal Antigravity interface and may change in Antigravity updates. When it stops answering, the monitor falls back to Google's service.

## Diagnostic log

Run:

```powershell
codex-usage.exe --diagnose
```

The log is written to `%TEMP%\codex-usage.log`. It includes:

- application version and executable path
- install channel (direct or portable)
- provider failure category and retry delay
- window creation, taskbar placement, and relaunch events

The log does not include access tokens, refresh tokens, credential file contents, or API response bodies.

## Update failures

Direct installations and portable copies download only the exact `codex-usage.exe` asset and verify it against `codex-usage.exe.sha256` from the same GitHub Release. The updater keeps the previous EXE until the downloaded version has been installed and restarted successfully. If restart fails, the old EXE is restored.

## Reset local position without deleting settings

Right-click the taskbar component and choose **Settings > Reset Position**. Settings are stored at `%APPDATA%\CodexUsage\settings.json`.

## Reinstall while preserving settings

Normal uninstall keeps the settings file. Reinstalling restores the saved language, refresh interval, provider selection, widget visibility, and taskbar position. Use `uninstall.ps1 -RemoveSettings` only when a full reset is intended.
