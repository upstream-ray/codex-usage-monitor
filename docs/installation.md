# Codex Usage installation model

Codex Usage remains a single native Windows executable. Installation only places the executable and an uninstall helper in a stable per-user directory; it does not add a runtime, service, driver, telemetry component, or machine-wide dependency.

## Direct installation

- Install directory: `%LOCALAPPDATA%\Programs\CodexUsage`
- Executable: `%LOCALAPPDATA%\Programs\CodexUsage\codex-usage.exe`
- Uninstall helper: `%LOCALAPPDATA%\Programs\CodexUsage\uninstall.ps1`
- Start menu shortcut: `%APPDATA%\Microsoft\Windows\Start Menu\Programs\Codex Usage.lnk`
- Desktop shortcut: `%USERPROFILE%\Desktop\Codex Usage.lnk`
- Add/Remove Programs key: `HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\CodexUsage`

The installer is per-user and does not request elevation. It verifies the release SHA256 before replacing an existing executable, launches the app after installation (unless `-NoLaunch` is passed), and does not modify `PATH`. Replacement uses a temporary file and keeps the previous executable until the new file has been placed successfully.

## Portable mode

`codex-usage.exe` can be run from any user-writable directory without installation. Portable mode uses the same `%APPDATA%\CodexUsage\settings.json` settings as a direct installation and supports in-app self-updates.

## Settings and startup behavior

- Upgrades preserve `%APPDATA%\CodexUsage\settings.json`.
- Normal uninstall preserves settings so a later reinstall restores preferences.
- `uninstall.ps1 -RemoveSettings` explicitly deletes the settings directory.
- The installer does not enable startup automatically. If startup was already enabled, installation preserves that choice and updates the registry value to the stable installed executable. Users otherwise control startup under **Settings → General → Start with Windows**.
- If Windows has disabled the startup entry in **Settings → Apps → Startup**, Codex Usage detects the blocked state and shows an **Open Startup apps** button on **Settings → General** so you can re-enable it directly in Windows Settings.
- Uninstall removes the `CodexUsage` startup entry because its executable no longer exists.

## WinGet

Codex Usage is not published to WinGet. Install it with `install.ps1` or run the portable EXE.

## Managed installations

The **Privacy and updates** settings control passive monitoring and update checks. Passive monitoring is on by default. Provider credential discovery starts when its service is enabled. Authentication failures pause retries until credentials change or the user retries. Codex and Claude inference-based recovery is available only when passive monitoring is switched off.

For an offline, managed installation, supply a trusted local executable and its approved SHA256.

```powershell
.\install.ps1 -Managed -SourcePath .\codex-usage.exe -ExpectedSha256 <approved-sha256> -NoLaunch
```

The installer saves `passive_monitoring: true` and `updates_enabled: false` before launch. Other settings are copied from the existing JSON object. For an organization-wide policy, administrators can place a JSON object at `%ProgramData%\CodexUsage\policy.json` and restrict writes to authorized administrators.

```json
{
  "force_passive_monitoring": true,
  "disable_updates": true
}
```

The application rechecks the policy at update boundaries, including the update helper. An unreadable or invalid policy enforces passive monitoring and disables updates.
