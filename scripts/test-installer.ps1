# Focused tests for the installer configuration and verification boundaries.
# Only pure helper functions are loaded; the installer is never executed.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

foreach ($script in Get-ChildItem -LiteralPath $PSScriptRoot -Filter '*.ps1') {
    $syntaxTokens = $null
    $syntaxErrors = $null
    [void][System.Management.Automation.Language.Parser]::ParseFile($script.FullName, [ref]$syntaxTokens, [ref]$syntaxErrors)
    if ($syntaxErrors.Count -ne 0) { throw ($syntaxErrors | Out-String) }
}

$installerPath = Join-Path $PSScriptRoot 'install.ps1'
$tokens = $null
$errors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile($installerPath, [ref]$tokens, [ref]$errors)
if ($errors.Count -ne 0) { throw ($errors | Out-String) }
$helpers = @('Read-FileChecksum', 'Assert-FileChecksum', 'Read-InstallationSettings', 'Invoke-ReleaseDownload')
foreach ($name in $helpers) {
    $definition = $ast.Find({ param($node) $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq $name }, $true)
    if (-not $definition) { throw "Missing installer helper: $name" }
    . ([scriptblock]::Create($definition.Extent.Text))
}

function Assert-Equal($Actual, $Expected, [string]$Message) {
    if ($Actual -cne $Expected) { throw $Message }
}
function Assert-Fails([scriptblock]$Action, [string]$ExpectedMessage) {
    try { & $Action | Out-Null }
    catch {
        if ($_.Exception.Message -like $ExpectedMessage) { return }
        throw
    }
    throw 'Expected the operation to fail.'
}

$testRoot = Join-Path ([IO.Path]::GetTempPath()) ('codex-usage-installer-tests-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $testRoot | Out-Null
try {
    $SettingsPath = Join-Path $testRoot 'settings.json'
    $Managed = $false
    $settings = Read-InstallationSettings
    Assert-Equal $settings.passive_monitoring $true 'New installation must default to passive monitoring.'
    Assert-Equal $settings.updates_enabled $true 'Ordinary installation should retain the normal update default.'

    [IO.File]::WriteAllText($SettingsPath, '{"passive_monitoring":false,"updates_enabled":true,"taskbar_offset":42}')
    $settings = Read-InstallationSettings
    Assert-Equal $settings.passive_monitoring $false 'Ordinary upgrades must preserve an explicit privacy choice.'
    Assert-Equal $settings.updates_enabled $true 'Ordinary upgrades must preserve an explicit update choice.'
    $Managed = $true
    $settings = Read-InstallationSettings
    Assert-Equal $settings.passive_monitoring $true 'Managed installation must enable passive monitoring.'
    Assert-Equal $settings.updates_enabled $false 'Managed installation must disable updates.'
    Assert-Equal $settings.taskbar_offset 42 'Managed installation must preserve unrelated preferences.'

    $script:requestCount = 0
    function Invoke-WebRequest { $script:requestCount++; throw 'Unexpected network request.' }
    Assert-Fails { Invoke-ReleaseDownload -Url 'https://example.invalid/' -Destination (Join-Path $testRoot 'download') } 'Network downloads are disabled*'
    Assert-Equal $script:requestCount 0 'Managed installation must not call the HTTP client.'

    $payload = Join-Path $testRoot 'uninstall.ps1'
    [IO.File]::WriteAllText($payload, '# test payload, never executed')
    $hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $payload).Hash
    Assert-Equal (Assert-FileChecksum -Path $payload -Expected $hash) $hash 'Matching checksum should pass.'
    Assert-Fails { Assert-FileChecksum -Path $payload -Expected ('0' * 64) } 'SHA256 mismatch*'
    Assert-Fails { Assert-FileChecksum -Path $payload -Expected '' } 'A valid SHA256 is required*'

    $sidecar = "$payload.sha256"
    [IO.File]::WriteAllText($sidecar, "$hash  wrong-name.ps1")
    Assert-Fails { Read-FileChecksum -Path $sidecar -FileName 'uninstall.ps1' } 'Checksum*is missing or invalid*'
    [IO.File]::WriteAllText($sidecar, "$hash  uninstall.ps1`n")
    Assert-Equal (Read-FileChecksum -Path $sidecar -FileName 'uninstall.ps1') $hash 'Checksum must name the intended file.'
    Write-Output 'Installer boundary tests passed.'
}
finally {
    Remove-Item -LiteralPath $testRoot -Recurse -Force
}
