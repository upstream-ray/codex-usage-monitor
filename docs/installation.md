# Codex Usage installation model

Codex Usage remains a single native Windows executable. Installation only places the executable and an uninstall helper in a stable per-user directory; it does not add a runtime, service, driver, telemetry component, or machine-wide dependency.

## Direct installation

- Install directory: `%LOCALAPPDATA%\Programs\CodexUsage`
- Executable: `%LOCALAPPDATA%\Programs\CodexUsage\codex-usage.exe`
- Uninstall helper: `%LOCALAPPDATA%\Programs\CodexUsage\uninstall.ps1`
- Start menu shortcut: `%APPDATA%\Microsoft\Windows\Start Menu\Programs\Codex Usage.lnk`
- Add/Remove Programs key: `HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\CodexUsage`

The installer is per-user and does not request elevation. It verifies SHA256 checksums for the executable and uninstall helper before replacing an existing installation. Replacement uses a temporary file and keeps the previous executable until the new file has been placed successfully. The installer and registered uninstall command do not override PowerShell execution policy.

Before running a downloaded installer, verify the release provenance and checksums as described in [Release verification](release-verification.md). A checksum retrieved from the same release establishes integrity; it does not independently authenticate the publisher.

## Managed installation

Extract the complete Windows build artifact to a local folder. Keep `codex-usage.exe`, `install.ps1`, `uninstall.ps1`, their `.sha256` sidecars, `SHA256SUMS`, and `build-info.json` together. After your organization has approved the source commit and executable hash, run:

```powershell
.\install.ps1 -Managed -SourcePath .\codex-usage.exe -ExpectedSha256 '<approved SHA256>'
```

`-Managed` requires a local executable and an explicit approved SHA256. It performs no release lookup or download. The companion uninstall helper must match `uninstall.ps1.sha256`, or a separately supplied `-ExpectedUninstallerSha256`. Add `-NoLaunch` to install without starting the monitor.

Before the first launch, the installer atomically writes these boolean preferences in `%APPDATA%\CodexUsage\settings.json`:

```json
{
  "passive_monitoring": true,
  "updates_enabled": false
}
```

Other settings are preserved. Ordinary installations preserve existing explicit privacy choices; when the new fields are absent, they enable passive monitoring and retain the normal update-enabled behavior. Use `-Managed` to ensure updates are off. Malformed settings are reported rather than silently discarded.

These settings are user preferences and can be changed in the application. For mandatory controls, deploy the [managed machine policy](managed-policy.md) through your device-management process. The application resolves `%PROGRAMDATA%\CodexUsage\policy.json` through Windows' system folder API, checks it at each relevant side-effect boundary, and prevents user preferences from loosening its restrictions. The per-user installer does not create or change this administrator-managed policy.

With updates disabled, the monitor does not check GitHub, download updates, start WinGet upgrades, or apply staged updates. The installed version remains visible. Backend checks also apply to the command-line update helper; disabling updates while a helper waits for the parent process prevents replacement. Disabling updates cannot retract a network request already sent before the setting changed.

To deploy a subsequent approved version, close the monitor and rerun the managed installer from its verified local artifact. A fork's preview artifact identifies its exact commit in its artifact name and `build-info.json`; the Cargo package version may still match the upstream base version.

## Portable mode

`codex-usage.exe` can be run from any user-writable directory without installation. Portable mode uses the same `%APPDATA%\CodexUsage\settings.json` settings as a direct installation.

For a managed portable deployment, merge the two settings above into the existing JSON before starting the executable. Do not replace the entire settings file if it contains other preferences. Portable mode still reads credentials for enabled providers; it is not a credential sandbox.

## Settings and startup behavior

- Upgrades preserve `%APPDATA%\CodexUsage\settings.json`.
- Normal uninstall preserves settings so a later reinstall restores preferences.
- `uninstall.ps1 -RemoveSettings` explicitly deletes the settings directory.
- The installer does not enable startup automatically. If startup was already enabled, installation preserves that choice and updates the registry value to the stable installed executable. Users otherwise control startup from the application's settings menu.
- Uninstall removes the `CodexUsage` startup entry because its executable no longer exists.

## WinGet

Codex Usage is not published to WinGet. Install it with `install.ps1` or run the portable EXE.
