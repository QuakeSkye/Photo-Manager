param(
    [Parameter()]
    [string]$Parent = 'G:\test',

    [Parameter()]
    [switch]$DryRun
)

$ErrorActionPreference = 'Stop'
$expectedParent = [System.IO.Path]::GetFullPath('G:\test').TrimEnd('\')
$resolvedParent = (Resolve-Path -LiteralPath $Parent).Path.TrimEnd('\')
if (-not $resolvedParent.Equals($expectedParent, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Phase 8 test parent must be exactly G:\test; received $resolvedParent"
}

$item = Get-Item -LiteralPath $resolvedParent -Force
if (-not $item.PSIsContainer -or ($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint)) {
    throw 'G:\test must be a normal directory, not a reparse point.'
}

$drive = [System.IO.DriveInfo]::new('G:\')
if (-not $drive.IsReady -or $drive.DriveFormat -ne 'NTFS') {
    throw 'G: must be an available NTFS volume.'
}

$maxAllocatedBytes = 300000000000
$minimumSafetyReserve = 100000000000
if ($drive.AvailableFreeSpace -lt ($maxAllocatedBytes + $minimumSafetyReserve)) {
    throw 'G: does not have the 300 GB test allowance plus the 100 GB safety reserve.'
}

$existingChildren = @(
    Get-ChildItem -LiteralPath $resolvedParent -Force |
        Select-Object Name, FullName, Attributes, CreationTimeUtc, LastWriteTimeUtc
)
$result = [ordered]@{
    reportVersion = 1
    dryRun = [bool]$DryRun
    parent = $resolvedParent
    filesystem = $drive.DriveFormat
    totalBytes = $drive.TotalSize
    freeBytes = $drive.AvailableFreeSpace
    maxAllocatedBytes = $maxAllocatedBytes
    minimumSafetyReserveBytes = $minimumSafetyReserve
    protectedExistingChildren = $existingChildren
    dDriveUsedForFixtures = $false
}

$result | ConvertTo-Json -Depth 5
