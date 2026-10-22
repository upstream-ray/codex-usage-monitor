# Runs the packaged installer on a disposable GitHub-hosted Windows runner.
# No provider credentials are loaded and the monitor UI is never launched.
[CmdletBinding()]
param(
    [string]$PackageDirectory = (Join-Path $PSScriptRoot '..\dist'),
    [Parameter(Mandatory = $true)]
    [ValidateSet('Desktop', 'Core')]
    [string]$ExpectedPSEdition
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

if ([Environment]::OSVersion.Platform -ne [PlatformID]::Win32NT -or
    $env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted') {
    throw 'This installation test requires a disposable GitHub-hosted Windows runner.'
}

if ($PSVersionTable.PSEdition -ne $ExpectedPSEdition) {
    throw "Expected PowerShell edition $ExpectedPSEdition, got $($PSVersionTable.PSEdition)."
}
if ($ExpectedPSEdition -eq 'Desktop' -and
    ($PSVersionTable.PSVersion.Major -ne 5 -or $PSVersionTable.PSVersion.Minor -ne 1)) {
    throw 'The Windows PowerShell regression must run under version 5.1.'
}
if ($ExpectedPSEdition -eq 'Core' -and $PSVersionTable.PSVersion.Major -lt 7) {
    throw 'The PowerShell Core regression must run under version 7 or newer.'
}
Write-Output "Testing fresh installation and upgrades under $($PSVersionTable.PSEdition) PowerShell $($PSVersionTable.PSVersion) ($PSHOME)."

function Assert-True([bool]$Condition, [string]$Message) {
    if (-not $Condition) { throw $Message }
}

function Assert-Absent([string]$Path) {
    # Get-Item also detects a dangling link; pre-existing app state is never ours.
    if ($null -ne (Get-Item -LiteralPath $Path -Force -ErrorAction SilentlyContinue)) {
        throw "Refusing to touch pre-existing app state: $Path"
    }
}

function Read-ApprovedPackageHash([string]$FileName) {
    $payload = Join-Path $PackageDirectory $FileName
    $sidecar = Get-Content -Raw -LiteralPath "$payload.sha256"
    $pattern = '^([0-9a-fA-F]{64})\s+\*?' + [regex]::Escape($FileName) + '$'
    if ($sidecar.Trim() -cnotmatch $pattern) {
        throw "The package checksum for $FileName is invalid."
    }
    $expected = $Matches[1]
    $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $payload).Hash
    Assert-True ($actual -eq $expected) "The packaged $FileName does not match its checksum."
    return $expected
}

function Write-Utf8([string]$Path, [string]$Content) {
    [IO.File]::WriteAllText($Path, $Content, [Text.UTF8Encoding]::new($false))
}

function Invoke-ApprovedInstallation([switch]$ManagedProfile) {
    $parameters = @{
        SourcePath = $ExecutablePath
        ExpectedSha256 = $ExecutableHash
        ExpectedUninstallerSha256 = $UninstallerHash
        NoLaunch = $true
    }
    if ($ManagedProfile) { $parameters.Managed = $true }
    # Deliberately invoke in this host. A pwsh-only child would miss Windows
    # PowerShell 5.1's .NET string/null binding and default text encoding.
    & $InstallerPath @parameters

    $processes = @(Get-CimInstance Win32_Process -Filter "Name='codex-usage.exe'")
    Assert-True ($processes.Count -eq 0) '-NoLaunch started the monitor.'
    Assert-True ((Get-FileHash -Algorithm SHA256 -LiteralPath $TargetPath).Hash -eq $ExecutableHash) 'Installation changed the approved executable.'
    $temporarySettings = @(Get-ChildItem -LiteralPath $SettingsDirectory -Filter 'settings-*.tmp')
    Assert-True ($temporarySettings.Count -eq 0) 'Installation left a temporary settings file.'
}

function Assert-PreservedPreferences($Settings, [string]$Scenario) {
    Assert-True ($Settings.tray_offset -eq 42) "$Scenario changed the saved widget position."
    Assert-True ($Settings.widget_visible -is [bool] -and -not $Settings.widget_visible) "$Scenario changed the saved widget visibility."
    Assert-True ($Settings.show_codex -is [bool] -and $Settings.show_codex) "$Scenario changed the enabled provider."
    Assert-True ($Settings.show_claude_code -is [bool] -and -not $Settings.show_claude_code) "$Scenario changed a disabled provider."
    Assert-True ($Settings.show_antigravity -is [bool] -and -not $Settings.show_antigravity) "$Scenario changed a disabled provider."
    Assert-True ($Settings.future_preference.note -ceq $UnicodePreference) "$Scenario did not preserve a UTF-8 preference."
    Assert-True ($Settings.future_preference.retained -is [bool] -and $Settings.future_preference.retained) "$Scenario lost an unrelated nested preference."
}

function Assert-UpdatePolicyExit([int]$Expected, [string]$Scenario) {
    # This CLI mode exits before window startup and credential discovery.
    $process = Start-Process -FilePath $TargetPath -ArgumentList '--check-update-policy' -PassThru -WindowStyle Hidden
    try {
        if (-not $process.WaitForExit(15000)) {
            $process.Kill()
            throw "The update policy helper timed out: $Scenario"
        }
        $process.Refresh()
        Assert-True ($process.ExitCode -eq $Expected) "Update policy exit code was $($process.ExitCode), expected ${Expected}: $Scenario"
    }
    finally {
        $process.Dispose()
    }
}

$PackageDirectory = (Resolve-Path -LiteralPath $PackageDirectory).Path
$InstallerPath = Join-Path $PackageDirectory 'install.ps1'
$ExecutablePath = Join-Path $PackageDirectory 'codex-usage.exe'
# The package is built in the same job from the reviewed checkout. These hashes
# verify those exact test inputs; external deployment also verifies provenance.
$ExecutableHash = Read-ApprovedPackageHash 'codex-usage.exe'
$InstallerHash = Read-ApprovedPackageHash 'install.ps1'
$UninstallerHash = Read-ApprovedPackageHash 'uninstall.ps1'
Assert-True (-not [string]::IsNullOrWhiteSpace($InstallerHash)) 'Missing approved installer hash.'

$LocalAppData = [Environment]::GetFolderPath('LocalApplicationData')
$RoamingAppData = [Environment]::GetFolderPath('ApplicationData')
$MachineAppData = [Environment]::GetFolderPath('CommonApplicationData')
foreach ($folder in @($LocalAppData, $RoamingAppData, $MachineAppData)) {
    Assert-True ([IO.Path]::IsPathRooted($folder)) 'Windows must provide absolute application data folders.'
}
# The installer and application must resolve the same real Windows folders.
Assert-True ([IO.Path]::GetFullPath($env:LOCALAPPDATA) -eq [IO.Path]::GetFullPath($LocalAppData)) 'LOCALAPPDATA is redirected; refusing to install.'
Assert-True ([IO.Path]::GetFullPath($env:APPDATA) -eq [IO.Path]::GetFullPath($RoamingAppData)) 'APPDATA is redirected; refusing to install.'

$InstallDirectory = Join-Path $LocalAppData 'Programs\CodexUsage'
$TargetPath = Join-Path $InstallDirectory 'codex-usage.exe'
$InstalledUninstaller = Join-Path $InstallDirectory 'uninstall.ps1'
$SettingsDirectory = Join-Path $RoamingAppData 'CodexUsage'
$SettingsPath = Join-Path $SettingsDirectory 'settings.json'
$LegacySettingsDirectory = Join-Path $RoamingAppData 'ClaudeCodeUsageMonitor'
$PolicyDirectory = Join-Path $MachineAppData 'CodexUsage'
$PolicyPath = Join-Path $PolicyDirectory 'policy.json'
$ShortcutPath = Join-Path $RoamingAppData 'Microsoft\Windows\Start Menu\Programs\Codex Usage.lnk'
$DesktopShortcutPath = Join-Path ([Environment]::GetFolderPath('Desktop')) 'Codex Usage.lnk'
$UninstallKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\CodexUsage'
$RunKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'

foreach ($path in @($InstallDirectory, $SettingsDirectory, $LegacySettingsDirectory, $PolicyDirectory,
    $ShortcutPath, $DesktopShortcutPath, $UninstallKey)) {
    Assert-Absent $path
}
if (Test-Path -LiteralPath $RunKey) {
    $startupProperties = Get-ItemProperty -LiteralPath $RunKey
    Assert-True ($null -eq $startupProperties.PSObject.Properties['CodexUsage']) 'A pre-existing startup entry must not be changed.'
}
$existingProcesses = @(Get-CimInstance Win32_Process -Filter "Name='codex-usage.exe'")
Assert-True ($existingProcesses.Count -eq 0) 'A running monitor must not be changed by this test.'

# Every app-specific location above was absent. Only these locations may be
# removed below, including when the installer or an assertion fails partway.
$OwnsPolicyDirectory = $false
try {
    Invoke-ApprovedInstallation -ManagedProfile

    Assert-True (Test-Path -LiteralPath $TargetPath -PathType Leaf) 'Managed installation did not install the executable.'
    Assert-True ((Get-FileHash -Algorithm SHA256 -LiteralPath $TargetPath).Hash -eq $ExecutableHash) 'The installed executable differs from the approved package.'
    Assert-True ((Get-FileHash -Algorithm SHA256 -LiteralPath $InstalledUninstaller).Hash -eq $UninstallerHash) 'The installed uninstaller differs from the approved package.'
    Assert-True (Test-Path -LiteralPath $UninstallKey) 'The application was not registered for uninstallation.'
    Assert-True (Test-Path -LiteralPath $ShortcutPath) 'The Start menu shortcut is missing.'
    Assert-True (Test-Path -LiteralPath $DesktopShortcutPath) 'The desktop shortcut is missing.'
    $launchedProcesses = @(Get-CimInstance Win32_Process -Filter "Name='codex-usage.exe'")
    Assert-True ($launchedProcesses.Count -eq 0) '-NoLaunch started the monitor.'

    $settings = [IO.File]::ReadAllText($SettingsPath) | ConvertFrom-Json
    Assert-True ($settings.passive_monitoring -is [bool] -and $settings.passive_monitoring) 'Managed installation must select passive monitoring.'
    Assert-True ($settings.updates_enabled -is [bool] -and -not $settings.updates_enabled) 'Managed installation must disable updates.'
    Assert-UpdatePolicyExit 1 'managed installation disables updates'

    # Seed a real existing settings file, then run the full installer again.
    # Fresh installs use File.Move and cannot reproduce the File.Replace bug.
    # Codepoints keep the script itself ASCII so 5.1's source-file decoding does
    # not influence the UTF-8 settings round-trip assertion.
    $UnicodePreference = -join (@(0x7BA1, 0x7406, 0x8A2D, 0x5B9A, 0x0020, 0x00E9) | ForEach-Object { [char]$_ })
    $OriginalPreferences = [ordered]@{
        passive_monitoring = $false
        updates_enabled = $true
        tray_offset = 42
        widget_visible = $false
        show_codex = $true
        show_claude_code = $false
        show_antigravity = $false
        future_preference = [ordered]@{ note = $UnicodePreference; retained = $true }
    }
    Write-Utf8 $SettingsPath ($OriginalPreferences | ConvertTo-Json -Depth 32)
    Invoke-ApprovedInstallation
    $settings = [IO.File]::ReadAllText($SettingsPath) | ConvertFrom-Json
    Assert-True ($settings.passive_monitoring -is [bool] -and -not $settings.passive_monitoring) 'Ordinary upgrade must preserve an explicit passive-monitoring preference.'
    Assert-True ($settings.updates_enabled -is [bool] -and $settings.updates_enabled) 'Ordinary upgrade must preserve an explicit update preference.'
    Assert-PreservedPreferences $settings 'Ordinary upgrade'
    Assert-UpdatePolicyExit 0 'ordinary upgrade preserves explicitly enabled updates'

    foreach ($attempt in 1..2) {
        Invoke-ApprovedInstallation -ManagedProfile
        $settings = [IO.File]::ReadAllText($SettingsPath) | ConvertFrom-Json
        Assert-True ($settings.passive_monitoring -is [bool] -and $settings.passive_monitoring) 'Managed upgrade must override the previous passive-monitoring preference.'
        Assert-True ($settings.updates_enabled -is [bool] -and -not $settings.updates_enabled) 'Managed upgrade must override the previous update preference.'
        Assert-PreservedPreferences $settings "Managed upgrade $attempt"
        Assert-UpdatePolicyExit 1 "managed upgrade $attempt disables updates"
    }
    $InstalledSettings = [IO.File]::ReadAllText($SettingsPath)

    $settings.updates_enabled = $true
    Write-Utf8 $SettingsPath ($settings | ConvertTo-Json -Depth 32)
    Assert-UpdatePolicyExit 0 'an explicit update preference enables the helper'

    New-Item -ItemType Directory -Path $PolicyDirectory | Out-Null
    $OwnsPolicyDirectory = $true
    Write-Utf8 $PolicyPath '{"disable_updates":true}'
    Assert-UpdatePolicyExit 1 'machine policy overrides an enabled user preference'
    Write-Utf8 $PolicyPath '{"disable_updates":false}'
    Assert-UpdatePolicyExit 0 'a permissive machine policy retains the user preference'
    Write-Utf8 $PolicyPath '{'
    Assert-UpdatePolicyExit 1 'malformed machine policy fails closed'
    Write-Utf8 $PolicyPath '{"disable_update":true}'
    Assert-UpdatePolicyExit 1 'an unknown machine policy field fails closed'

    # Leave the test installation in its original managed profile before removal.
    Write-Utf8 $SettingsPath $InstalledSettings
    Write-Utf8 $PolicyPath '{"disable_updates":true}'
    $PreservedSettingsHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $SettingsPath).Hash
    & $InstalledUninstaller -Quiet

    foreach ($path in @($InstallDirectory, $ShortcutPath, $DesktopShortcutPath, $UninstallKey)) {
        Assert-True (-not (Test-Path -LiteralPath $path)) "Uninstallation left app state at $path"
    }
    Assert-True (Test-Path -LiteralPath $SettingsPath -PathType Leaf) 'Ordinary uninstallation removed settings.'
    Assert-True ((Get-FileHash -Algorithm SHA256 -LiteralPath $SettingsPath).Hash -eq $PreservedSettingsHash) 'Uninstallation modified the preserved settings.'
    Assert-True ((Get-Content -Raw -LiteralPath $PolicyPath) -ceq '{"disable_updates":true}') 'Uninstallation modified machine policy.'
    Write-Output "Fresh installation, ordinary/managed upgrades with UTF-8 settings, policy helper, and settings-preserving uninstallation passed under $ExpectedPSEdition PowerShell $($PSVersionTable.PSVersion)."
}
finally {
    # A failed -NoLaunch assertion can leave this test's executable running.
    Get-CimInstance Win32_Process -Filter "Name='codex-usage.exe'" |
        Where-Object { $_.ExecutablePath -eq $TargetPath } |
        ForEach-Object { Stop-Process -Id $_.ProcessId -Force }
    foreach ($path in @($InstallDirectory, $SettingsDirectory, $ShortcutPath, $DesktopShortcutPath, $UninstallKey)) {
        if (Test-Path -LiteralPath $path) {
            Remove-Item -LiteralPath $path -Recurse -Force
        }
    }
    if (Test-Path -LiteralPath $RunKey) {
        Remove-ItemProperty -LiteralPath $RunKey -Name 'CodexUsage' -ErrorAction SilentlyContinue
    }
    if ($OwnsPolicyDirectory -and (Test-Path -LiteralPath $PolicyDirectory)) {
        Remove-Item -LiteralPath $PolicyDirectory -Recurse -Force
    }
}
