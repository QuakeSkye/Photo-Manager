$ErrorActionPreference = 'Stop'

$binary = Join-Path $PSScriptRoot '..\src-tauri\target\release\local-photo-manager.exe'
if (-not (Test-Path -LiteralPath $binary)) {
    throw 'Release binary is missing. Run npm run tauri -- build first.'
}

$testBase = Join-Path $PSScriptRoot '..\src-tauri\target\windows-tests'
$resolvedTestBase = [System.IO.Path]::GetFullPath($testBase)
New-Item -ItemType Directory -Path $resolvedTestBase -Force | Out-Null
$previousRecycleParent = $env:FPM_RECYCLE_NTFS_TEST_PARENT
$env:FPM_RECYCLE_NTFS_TEST_PARENT = $resolvedTestBase
$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$results = @()
$cleanupWarning = $null
try {
    & node (Join-Path $PSScriptRoot 'run-cargo.mjs') test --manifest-path (Join-Path $repoRoot 'src-tauri\Cargo.toml') real_sta_worker_recycles_pair_and_restores_identical_content -- --ignored --nocapture
    if ($LASTEXITCODE -ne 0) {
        throw "The real recycle/restore STA test failed with code $LASTEXITCODE."
    }
    $results += [pscustomobject]@{
        test = 'real_sta_worker_recycles_pair_and_restores_identical_content'
        result = 'passed'
        root_scope = 'generated_disposable_ntfs_test_root'
        permanent_delete_fallback = $false
    }
}
finally {
    $env:FPM_RECYCLE_NTFS_TEST_PARENT = $previousRecycleParent
}

$testParent = Join-Path $testBase ('fpm-windows-test-' + [guid]::NewGuid())
$testRoot = Join-Path $testParent ('fpm-generated-test-root-' + [guid]::NewGuid())
New-Item -ItemType Directory -Path $testRoot -Force | Out-Null
[System.IO.File]::WriteAllText(
    (Join-Path $testRoot '.fuji-photo-manager-test-root-v1'),
    "generated-test-root-v1`n",
    [System.Text.UTF8Encoding]::new($false)
)
$previousTestRoot = $env:FPM_TEST_ROOT
$env:FPM_TEST_ROOT = $testRoot

$first = Start-Process -FilePath $binary -WindowStyle Hidden -PassThru
try {
    Start-Sleep -Milliseconds 1500
    if ($first.HasExited) {
        throw 'The first application instance exited unexpectedly.'
    }

    $second = Start-Process -FilePath $binary -WindowStyle Hidden -PassThru
    if (-not $second.WaitForExit(8000)) {
        throw 'The second instance did not hand off to the first instance.'
    }

    if ($second.ExitCode -ne 0) {
        throw "The second instance exited with code $($second.ExitCode)."
    }

    $matching = @(Get-Process -Name 'local-photo-manager' -ErrorAction SilentlyContinue)
    if ($matching.Count -ne 1) {
        throw "Expected exactly one running instance, found $($matching.Count)."
    }

    $singleInstance = [pscustomobject]@{
        test = 'windows_single_instance_lifecycle'
        result = 'passed'
        running_instances = $matching.Count
    }
    $results += $singleInstance
    $singleInstance | ConvertTo-Json -Compress
}
finally {
    if (-not $first.HasExited) {
        $first.Kill()
        $first.WaitForExit()
    }
    $env:FPM_TEST_ROOT = $previousTestRoot
    $resolvedBase = (Resolve-Path -LiteralPath $testBase).Path
    $resolvedParent = (Resolve-Path -LiteralPath $testParent).Path
    if ($resolvedParent.StartsWith($resolvedBase + [System.IO.Path]::DirectorySeparatorChar)) {
        $cleanupParent = Join-Path $resolvedBase ('fpm-cleanup-' + [guid]::NewGuid())
        [System.IO.Directory]::Move($resolvedParent, $cleanupParent)
        foreach ($cleanupTarget in @($cleanupParent, $resolvedParent)) {
            for ($attempt = 0; $attempt -lt 40 -and (Test-Path -LiteralPath $cleanupTarget); $attempt++) {
                try {
                    [System.IO.Directory]::Delete($cleanupTarget, $true)
                }
                catch [System.IO.DirectoryNotFoundException] {
                    # A cache-maintenance worker may remove a child between enumeration and
                    # deletion. Retry the already-validated disposable parent itself.
                }
                catch {
                    if ($attempt -eq 39) {
                        $cleanupWarning = $_.Exception.Message
                    }
                }
                if (Test-Path -LiteralPath $cleanupTarget) {
                    Start-Sleep -Milliseconds 250
                }
            }
            if (Test-Path -LiteralPath $cleanupTarget) {
                $cleanupWarning = "Generated fixture cleanup is pending: $cleanupTarget"
                Write-Warning $cleanupWarning
                break
            }
        }
    }
}

$artifacts = Join-Path $repoRoot 'artifacts'
New-Item -ItemType Directory -Path $artifacts -Force | Out-Null
[pscustomobject]@{
    report_version = 1
    phase = 7
    test_root_only = $true
    filesystem = 'NTFS'
    real_fuji_library_admission_created = $false
    cleanup_warning = $cleanupWarning
    tests = $results
} | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $artifacts 'phase-seven-windows-results.json') -Encoding utf8
