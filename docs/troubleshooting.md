# Troubleshooting Codex Usage

## Taskbar error labels

Codex Usage keeps authentication failures separate from transient service failures:

| Simplified Chinese | Other languages | Meaning | Recommended action |
|---|---|---|---|
| `!` | `!` | Enabled service credentials are missing, expired, or rejected | Check the supported credential source, sign in with the relevant CLI/app, then refresh Codex Usage. Claude Desktop login does not count as Claude Code CLI login. |
| `网络` | `NET` | Network or TLS connection failed | Check connectivity, VPN, proxy, and firewall settings |
| `限流` | `429` | Provider rate limit | Wait for the provider retry window; Codex Usage retries with backoff |
| `服务` | `5XX` | Provider service failure | Wait and retry; check provider status if it persists |
| `错误` | `ERR` | Invalid or unsupported response | Enable diagnostics and inspect the log |

Authentication failures pause provider polling until the credential source changes, preventing repeated login notifications. Transient failures use exponential backoff up to the configured refresh interval.

## Codex is signed in, but the monitor shows `!`

A successful Codex sign-in does not guarantee a compatible credential file. Check [Account Support](../README.md#account-support), especially file storage versus keyring or ephemeral storage, the Windows user and inherited `CODEX_HOME`, WSL-only installations, and company gateway requirements. The monitor does not use API keys or browser cookies. Preserve your organization's authentication controls.

In passive mode, let the official Codex app or CLI renew the session, or sign in there again, then choose **Refresh**. The monitor rereads the supported credential file; it does not renew the session itself. A `401` or `403` can also indicate workspace or endpoint restrictions that another sign-in will not resolve.

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

The log may also include local paths, launch arguments, and WSL distribution names. Review it for account or machine identifiers before sharing, and do not attach credential files or tokens to a report.

## Update failures

Direct installations and portable copies download only the exact `codex-usage.exe` asset and verify it against `codex-usage.exe.sha256` from the same GitHub Release. The updater keeps the previous EXE until the downloaded version has been installed and restarted successfully. If restart fails, the old EXE is restored.

## Installer reports "The path is not of a legal form"

Older preview installers passed an empty backup path to .NET when replacing an existing settings file. This affected upgrades even when the package path and checksums were correct. The installer now passes a true null backup path and explicitly reads UTF-8 settings so Windows PowerShell 5.1 preserves non-ASCII preferences.

Use the corrected package and rerun its installer with the approved hashes; no uninstall or settings reset is needed. The failed upgrade may leave the monitor stopped after restoring its previous executable. A successful retry updates the installation and starts the monitor normally.

## Widget disappears after a display change, or covers the clock

Display changes now check that Windows accepted the new taskbar attachment before saving the selected display. A rejected move keeps the previous attachment and reports the failure. Placement uses the selected taskbar's actual client coordinates, including displays positioned to the left of or above the primary screen.

The widget stays to the left of detected notification-area and clock controls, including the separate clock used by secondary taskbars. When those controls cannot be identified, automatic placement uses the left edge instead of assuming that the clock area is empty. This is a taskbar overlay; it does not reserve space in the Windows task-button layout.

After installing the fix, choose **Settings > Display**, then **Settings > Reset Position** if the old offset is no longer useful. Keep **Show Widget** and the intended **Monitored services** checked. If Windows rejects the move, the message and a `--diagnose` log can help identify the taskbar or display configuration involved.

## Reset local position without deleting settings

Right-click the taskbar component and choose **Settings > Reset Position**. Settings are stored at `%APPDATA%\CodexUsage\settings.json`.

## Reinstall while preserving settings

Normal uninstall keeps the settings file. Reinstalling restores the saved language, refresh interval, provider selection, widget visibility, and taskbar position. Use `uninstall.ps1 -RemoveSettings` only when a full reset is intended.
