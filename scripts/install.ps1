[CmdletBinding()]
param(
    [string]$SourcePath,
    [string]$ExpectedSha256,
    [string]$ExpectedUninstallerSha256,
    [ValidatePattern('^v?\d+\.\d+\.\d+([-.][0-9A-Za-z.-]+)?$')]
    [string]$Version,
    [switch]$Managed,
    [switch]$NoLaunch
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$Repository = 'upstream-ray/codex-usage-monitor'
$UserLocalDirectory = [Environment]::GetFolderPath('LocalApplicationData')
$UserRoamingDirectory = [Environment]::GetFolderPath('ApplicationData')
if ([string]::IsNullOrWhiteSpace($UserLocalDirectory) -or [string]::IsNullOrWhiteSpace($UserRoamingDirectory)) {
    throw 'Unable to resolve the Windows user data folders.'
}
$InstallDirectory = Join-Path $UserLocalDirectory 'Programs\CodexUsage'
$TargetPath = Join-Path $InstallDirectory 'codex-usage.exe'
$InstalledUninstaller = Join-Path $InstallDirectory 'uninstall.ps1'
$ShortcutPath = Join-Path $UserRoamingDirectory 'Microsoft\Windows\Start Menu\Programs\Codex Usage.lnk'
$DesktopShortcutPath = Join-Path ([Environment]::GetFolderPath('Desktop')) 'Codex Usage.lnk'
$UninstallKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\CodexUsage'
$RunKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$TempDirectory = Join-Path ([IO.Path]::GetTempPath()) ('codex-usage-install-' + [Guid]::NewGuid().ToString('N'))
$SettingsDirectory = Join-Path $UserRoamingDirectory 'CodexUsage'
$SettingsPath = Join-Path $SettingsDirectory 'settings.json'

if ($Managed -and (-not $SourcePath -or -not $ExpectedSha256)) {
    throw 'Managed installation requires a local -SourcePath and an approved -ExpectedSha256. It never downloads a release.'
}

$StartupWasEnabled = $false
$ExistingStartup = $null
if (Test-Path -LiteralPath $RunKey) {
    $ExistingStartup = try {
        Get-ItemPropertyValue -Path $RunKey -Name 'CodexUsage' -ErrorAction Stop
    }
    catch {
        $null
    }
    $StartupWasEnabled = -not [string]::IsNullOrWhiteSpace($ExistingStartup)
}

function Get-ReleaseAsset {
    param(
        [Parameter(Mandatory = $true)]$Release,
        [Parameter(Mandatory = $true)][string]$Name
    )

    $asset = $Release.assets | Where-Object { $_.name -eq $Name } | Select-Object -First 1
    if (-not $asset) {
        throw "Release asset '$Name' was not found."
    }
    $expectedUrl = "https://github.com/$Repository/releases/download/$($Release.tag_name)/$Name"
    if ($asset.browser_download_url -cne $expectedUrl) {
        throw "Release asset '$Name' did not use its expected GitHub release URL."
    }
    return $asset.browser_download_url
}

function Invoke-ReleaseDownload {
    param(
        [Parameter(Mandatory = $true)][string]$Url,
        [Parameter(Mandatory = $true)][string]$Destination
    )

    if ($Managed) {
        throw 'Network downloads are disabled for managed installation.'
    }
    Invoke-WebRequest -UseBasicParsing -MaximumRedirection 5 -TimeoutSec 30 -Headers @{ 'User-Agent' = 'CodexUsage-Installer' } -Uri $Url -OutFile $Destination
}

function Read-FileChecksum {
    param([string]$Path, [string]$FileName)

    $text = (Get-Content -Raw -LiteralPath $Path).Trim()
    $pattern = '^([0-9a-fA-F]{64})\s+\*?' + [regex]::Escape($FileName) + '$'
    if ($text -notmatch $pattern) {
        throw "Checksum for '$FileName' is missing or invalid."
    }
    return $Matches[1]
}

function Assert-FileChecksum {
    param([string]$Path, [string]$Expected)

    if ($Expected -notmatch '^[0-9a-fA-F]{64}$') {
        throw "A valid SHA256 is required for $(Split-Path -Leaf $Path)."
    }
    $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash
    if ($actual -ne $Expected) {
        throw "SHA256 mismatch for $(Split-Path -Leaf $Path)."
    }
    return $actual
}

function Read-InstallationSettings {
    if (Test-Path -LiteralPath $SettingsPath) {
        $settings = Get-Content -Raw -Encoding UTF8 -LiteralPath $SettingsPath | ConvertFrom-Json
        if ($null -eq $settings -or $settings -isnot [pscustomobject]) {
            throw 'Existing settings must be a JSON object. Correct the settings before installing.'
        }
    }
    else {
        $settings = [pscustomobject]@{}
    }

    # Preserve explicit choices on ordinary upgrades. Managed deployment always
    # selects passive monitoring and disables all application update activity.
    if ($Managed -or -not $settings.PSObject.Properties['passive_monitoring']) {
        $settings | Add-Member -NotePropertyName passive_monitoring -NotePropertyValue $true -Force
    }
    if ($Managed -or -not $settings.PSObject.Properties['updates_enabled']) {
        $settings | Add-Member -NotePropertyName updates_enabled -NotePropertyValue (-not $Managed) -Force
    }
    if ($settings.passive_monitoring -isnot [bool] -or $settings.updates_enabled -isnot [bool]) {
        throw 'Privacy settings must be JSON booleans. Correct them or use -Managed to select the safe profile.'
    }
    return $settings
}

New-Item -ItemType Directory -Force -Path $TempDirectory | Out-Null

try {
    $StagedExecutable = Join-Path $TempDirectory 'codex-usage.exe'
    $StagedUninstaller = Join-Path $TempDirectory 'uninstall.ps1'

    if ($SourcePath) {
        $ResolvedSource = (Resolve-Path -LiteralPath $SourcePath).Path
        Copy-Item -LiteralPath $ResolvedSource -Destination $StagedExecutable -Force

        if (-not $ExpectedSha256) {
            $ExpectedSha256 = Read-FileChecksum -Path "$ResolvedSource.sha256" -FileName 'codex-usage.exe'
        }

        $LocalUninstaller = Join-Path $PSScriptRoot 'uninstall.ps1'
        if (-not (Test-Path -LiteralPath $LocalUninstaller -PathType Leaf)) {
            throw "Local uninstall helper was not found at $LocalUninstaller"
        }
        Copy-Item -LiteralPath $LocalUninstaller -Destination $StagedUninstaller -Force
        if (-not $ExpectedUninstallerSha256) {
            $ExpectedUninstallerSha256 = Read-FileChecksum -Path "$LocalUninstaller.sha256" -FileName 'uninstall.ps1'
        }
    }
    else {
        $ApiUrl = if ($Version) {
            "https://api.github.com/repos/$Repository/releases/tags/v$($Version.TrimStart('v'))"
        }
        else {
            "https://api.github.com/repos/$Repository/releases/latest"
        }

        $Release = Invoke-RestMethod -UseBasicParsing -Headers @{ 'User-Agent' = 'CodexUsage-Installer' } -Uri $ApiUrl
        $ExecutableUrl = Get-ReleaseAsset -Release $Release -Name 'codex-usage.exe'
        $ChecksumUrl = Get-ReleaseAsset -Release $Release -Name 'codex-usage.exe.sha256'
        $UninstallerUrl = Get-ReleaseAsset -Release $Release -Name 'uninstall.ps1'
        $UninstallerChecksumUrl = Get-ReleaseAsset -Release $Release -Name 'uninstall.ps1.sha256'
        $ChecksumPath = Join-Path $TempDirectory 'codex-usage.exe.sha256'
        $UninstallerChecksumPath = Join-Path $TempDirectory 'uninstall.ps1.sha256'

        Invoke-ReleaseDownload -Url $ExecutableUrl -Destination $StagedExecutable
        Invoke-ReleaseDownload -Url $ChecksumUrl -Destination $ChecksumPath
        Invoke-ReleaseDownload -Url $UninstallerUrl -Destination $StagedUninstaller
        Invoke-ReleaseDownload -Url $UninstallerChecksumUrl -Destination $UninstallerChecksumPath

        $ExpectedSha256 = Read-FileChecksum -Path $ChecksumPath -FileName 'codex-usage.exe'
        $ExpectedUninstallerSha256 = Read-FileChecksum -Path $UninstallerChecksumPath -FileName 'uninstall.ps1'
    }

    $ActualSha256 = Assert-FileChecksum -Path $StagedExecutable -Expected $ExpectedSha256
    $null = Assert-FileChecksum -Path $StagedUninstaller -Expected $ExpectedUninstallerSha256
    # Validate existing configuration before changing the installation.
    $null = Read-InstallationSettings

    New-Item -ItemType Directory -Force -Path $InstallDirectory | Out-Null

    Get-CimInstance Win32_Process -Filter "Name='codex-usage.exe'" -ErrorAction SilentlyContinue |
        Where-Object { $_.ExecutablePath -eq $TargetPath } |
        ForEach-Object { Stop-Process -Id $_.ProcessId -Force }

    $NewPath = "$TargetPath.new"
    $BackupPath = "$TargetPath.old"
    Copy-Item -LiteralPath $StagedExecutable -Destination $NewPath -Force
    Remove-Item -LiteralPath $BackupPath -Force -ErrorAction SilentlyContinue

    $HadPreviousVersion = Test-Path -LiteralPath $TargetPath -PathType Leaf
    if ($HadPreviousVersion) {
        Move-Item -LiteralPath $TargetPath -Destination $BackupPath -Force
    }

    try {
        Move-Item -LiteralPath $NewPath -Destination $TargetPath -Force
    }
    catch {
        Remove-Item -LiteralPath $NewPath -Force -ErrorAction SilentlyContinue
        if ($HadPreviousVersion -and (Test-Path -LiteralPath $BackupPath -PathType Leaf)) {
            Move-Item -LiteralPath $BackupPath -Destination $TargetPath -Force
        }
        throw
    }

    try {
        Copy-Item -LiteralPath $StagedUninstaller -Destination $InstalledUninstaller -Force

        $InstalledVersion = (Get-Item -LiteralPath $TargetPath).VersionInfo.ProductVersion
        if (-not $InstalledVersion) {
            throw 'The installed executable does not contain a product version.'
        }

        New-Item -Path $UninstallKey -Force | Out-Null
        $UninstallCommand = "powershell.exe -NoProfile -File `"$InstalledUninstaller`""
        Set-ItemProperty -Path $UninstallKey -Name DisplayName -Value 'Codex Usage'
        Set-ItemProperty -Path $UninstallKey -Name DisplayVersion -Value $InstalledVersion
        Set-ItemProperty -Path $UninstallKey -Name Publisher -Value 'Ray'
        Set-ItemProperty -Path $UninstallKey -Name DisplayIcon -Value $TargetPath
        Set-ItemProperty -Path $UninstallKey -Name InstallLocation -Value $InstallDirectory
        Set-ItemProperty -Path $UninstallKey -Name URLInfoAbout -Value "https://github.com/$Repository"
        Set-ItemProperty -Path $UninstallKey -Name UninstallString -Value $UninstallCommand
        Set-ItemProperty -Path $UninstallKey -Name QuietUninstallString -Value "$UninstallCommand -Quiet"
        Set-ItemProperty -Path $UninstallKey -Name NoModify -Type DWord -Value 1
        Set-ItemProperty -Path $UninstallKey -Name NoRepair -Type DWord -Value 1

        if ($StartupWasEnabled) {
            Set-ItemProperty -Path $RunKey -Name 'CodexUsage' -Value "`"$TargetPath`""
        }

        $Shell = New-Object -ComObject WScript.Shell
        foreach ($LinkPath in @($ShortcutPath, $DesktopShortcutPath)) {
            $ShortcutDirectory = Split-Path -Parent $LinkPath
            New-Item -ItemType Directory -Force -Path $ShortcutDirectory | Out-Null
            $Shortcut = $Shell.CreateShortcut($LinkPath)
            $Shortcut.TargetPath = $TargetPath
            $Shortcut.WorkingDirectory = $InstallDirectory
            $Shortcut.IconLocation = "$TargetPath,0"
            $Shortcut.Description = 'Codex Usage'
            $Shortcut.Save()
        }

        # Ask Explorer to refresh shortcut icons after an in-place EXE upgrade.
        # This avoids a stale cached icon while preserving the standard arrow overlay.
        $IconRefreshTool = Join-Path $env:SystemRoot 'System32\ie4uinit.exe'
        if (Test-Path -LiteralPath $IconRefreshTool -PathType Leaf) {
            Start-Process -FilePath $IconRefreshTool -ArgumentList '-show' -WindowStyle Hidden -Wait
        }

        # Write before the executable can discover credentials or make requests.
        # UTF-8 without a BOM is required by the application's JSON parser.
        $Settings = Read-InstallationSettings
        New-Item -ItemType Directory -Force -Path $SettingsDirectory | Out-Null
        $SettingsTempPath = Join-Path $SettingsDirectory ('settings-' + [Guid]::NewGuid().ToString('N') + '.tmp')
        try {
            [IO.File]::WriteAllText($SettingsTempPath, ($Settings | ConvertTo-Json -Depth 32), [Text.UTF8Encoding]::new($false))
            if (Test-Path -LiteralPath $SettingsPath) {
                # PowerShell converts $null to an empty .NET string, which is
                # an invalid backup path. Pass a true null and keep the atomic replace.
                [IO.File]::Replace($SettingsTempPath, $SettingsPath, [System.Management.Automation.Language.NullString]::Value)
            }
            else {
                [IO.File]::Move($SettingsTempPath, $SettingsPath)
            }
        }
        finally {
            Remove-Item -LiteralPath $SettingsTempPath -Force -ErrorAction SilentlyContinue
        }
    }
    catch {
        if ($HadPreviousVersion -and (Test-Path -LiteralPath $BackupPath -PathType Leaf)) {
            Remove-Item -LiteralPath $TargetPath -Force -ErrorAction SilentlyContinue
            Move-Item -LiteralPath $BackupPath -Destination $TargetPath -Force
        }
        if ($StartupWasEnabled -and $ExistingStartup) {
            Set-ItemProperty -Path $RunKey -Name 'CodexUsage' -Value $ExistingStartup
        }
        throw
    }

    Remove-Item -LiteralPath $BackupPath -Force -ErrorAction SilentlyContinue

    if (-not $NoLaunch) {
        Start-Process -FilePath $TargetPath -WorkingDirectory $InstallDirectory -WindowStyle Hidden
    }

    Write-Output "Codex Usage $InstalledVersion installed to $InstallDirectory"
    Write-Output "SHA256: $ActualSha256"
    if ($Managed) {
        Write-Output 'Managed profile: passive monitoring enabled; update checks, downloads and self-updates disabled.'
    }
}
finally {
    $ExpectedTempParent = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\')
    $ResolvedTemp = [IO.Path]::GetFullPath($TempDirectory).TrimEnd('\')
    if ($ResolvedTemp.StartsWith($ExpectedTempParent + '\', [StringComparison]::OrdinalIgnoreCase)) {
        Remove-Item -LiteralPath $ResolvedTemp -Recurse -Force -ErrorAction SilentlyContinue
    }
}
