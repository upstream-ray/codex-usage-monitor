[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][int]$ProcessId,
    [int]$DurationMinutes = 10,
    [int]$IntervalSeconds = 10,
    [string]$OutputDirectory = (Join-Path $PSScriptRoot '..\target\resource-monitor')
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

if ($DurationMinutes -lt 1 -or $IntervalSeconds -lt 1) {
    throw 'DurationMinutes and IntervalSeconds must be positive.'
}

$OutputDirectory = [IO.Path]::GetFullPath($OutputDirectory)
New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null

if (-not ('CodexUsage.ResourceMetrics' -as [type])) {
    Add-Type @'
using System;
using System.Runtime.InteropServices;
namespace CodexUsage {
    public static class ResourceMetrics {
        [DllImport("user32.dll")]
        public static extern uint GetGuiResources(IntPtr process, uint flags);
    }
}
'@
}

$Samples = [System.Collections.Generic.List[object]]::new()
$Deadline = [DateTimeOffset]::UtcNow.AddMinutes($DurationMinutes)

while ([DateTimeOffset]::UtcNow -lt $Deadline) {
    $Process = Get-Process -Id $ProcessId -ErrorAction Stop
    $Samples.Add([pscustomobject]@{
        TimestampUtc = [DateTimeOffset]::UtcNow.ToString('O')
        PrivateMB = [Math]::Round($Process.PrivateMemorySize64 / 1MB, 3)
        WorkingSetMB = [Math]::Round($Process.WorkingSet64 / 1MB, 3)
        Handles = $Process.HandleCount
        GdiObjects = [CodexUsage.ResourceMetrics]::GetGuiResources($Process.Handle, 0)
        UserObjects = [CodexUsage.ResourceMetrics]::GetGuiResources($Process.Handle, 1)
    })
    Start-Sleep -Seconds $IntervalSeconds
}

$CsvPath = Join-Path $OutputDirectory 'samples.csv'
$SummaryPath = Join-Path $OutputDirectory 'summary.json'
$Samples | Export-Csv -NoTypeInformation -Encoding UTF8 -Path $CsvPath

$First = $Samples[0]
$Last = $Samples[$Samples.Count - 1]
$Summary = [ordered]@{
    ProcessId = $ProcessId
    DurationMinutes = $DurationMinutes
    IntervalSeconds = $IntervalSeconds
    SampleCount = $Samples.Count
    PrivateMB = [ordered]@{ First = $First.PrivateMB; Last = $Last.PrivateMB; Delta = [Math]::Round($Last.PrivateMB - $First.PrivateMB, 3); Max = ($Samples.PrivateMB | Measure-Object -Maximum).Maximum }
    WorkingSetMB = [ordered]@{ First = $First.WorkingSetMB; Last = $Last.WorkingSetMB; Delta = [Math]::Round($Last.WorkingSetMB - $First.WorkingSetMB, 3); Max = ($Samples.WorkingSetMB | Measure-Object -Maximum).Maximum }
    Handles = [ordered]@{ First = $First.Handles; Last = $Last.Handles; Delta = $Last.Handles - $First.Handles; Max = ($Samples.Handles | Measure-Object -Maximum).Maximum }
    GdiObjects = [ordered]@{ First = $First.GdiObjects; Last = $Last.GdiObjects; Delta = $Last.GdiObjects - $First.GdiObjects; Max = ($Samples.GdiObjects | Measure-Object -Maximum).Maximum }
    UserObjects = [ordered]@{ First = $First.UserObjects; Last = $Last.UserObjects; Delta = $Last.UserObjects - $First.UserObjects; Max = ($Samples.UserObjects | Measure-Object -Maximum).Maximum }
}

$Summary | ConvertTo-Json -Depth 4 | Set-Content -Encoding UTF8 -Path $SummaryPath
$Summary | ConvertTo-Json -Depth 4
