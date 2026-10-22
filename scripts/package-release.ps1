[CmdletBinding()]
param(
    [string]$BinaryPath = (Join-Path $PSScriptRoot '..\target\release\codex-usage.exe'),
    [string]$Destination = (Join-Path $PSScriptRoot '..\dist')
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

New-Item -ItemType Directory -Force -Path $Destination | Out-Null
Copy-Item -LiteralPath $BinaryPath -Destination (Join-Path $Destination 'codex-usage.exe') -Force
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'install.ps1') -Destination $Destination -Force
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'uninstall.ps1') -Destination $Destination -Force

$revision = (& git rev-parse HEAD).Trim()
if ($LASTEXITCODE -ne 0) { throw 'Unable to determine the source commit.' }
$compiler = (& rustc --version).Trim()
if ($LASTEXITCODE -ne 0) { throw 'Unable to determine the Rust compiler version.' }
$buildInfo = [ordered]@{
    repository = $env:GITHUB_REPOSITORY
    source_commit = $revision
    source_ref = $env:GITHUB_REF
    compiler = $compiler
    build_command = 'cargo build --release --locked'
    workflow_run = if ($env:GITHUB_RUN_ID) { "https://github.com/$env:GITHUB_REPOSITORY/actions/runs/$env:GITHUB_RUN_ID" } else { $null }
    authenticode_signing_step = 'not configured'
}
$utf8 = [Text.UTF8Encoding]::new($false)
[IO.File]::WriteAllText((Join-Path $Destination 'build-info.json'), ($buildInfo | ConvertTo-Json), $utf8)

$payloads = @('codex-usage.exe', 'install.ps1', 'uninstall.ps1', 'build-info.json')
foreach ($name in $payloads) {
    $hash = (Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path $Destination $name)).Hash.ToLowerInvariant()
    [IO.File]::WriteAllText((Join-Path $Destination "$name.sha256"), "$hash  $name`n", $utf8)
}

# The manifest covers every shipped payload and checksum sidecar. It cannot
# contain its own checksum; the release attestation covers the manifest too.
$manifest = foreach ($name in ($payloads + @($payloads | ForEach-Object { "$_.sha256" }))) {
    $hash = (Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path $Destination $name)).Hash.ToLowerInvariant()
    "$hash  $name"
}
[IO.File]::WriteAllText((Join-Path $Destination 'SHA256SUMS'), (($manifest -join "`n") + "`n"), $utf8)
