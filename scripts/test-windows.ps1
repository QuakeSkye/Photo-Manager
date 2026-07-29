$ErrorActionPreference = 'Stop'

$binary = Join-Path $PSScriptRoot '..\src-tauri\target\release\local-photo-manager.exe'
if (-not (Test-Path -LiteralPath $binary)) {
    throw 'Release binary is missing. Run npm run tauri -- build first.'
}

$testBase = Join-Path $PSScriptRoot '..\src-tauri\target\windows-tests'
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

    [pscustomobject]@{
        test = 'windows_single_instance_lifecycle'
        result = 'passed'
        running_instances = $matching.Count
    } | ConvertTo-Json -Compress
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
        Remove-Item -LiteralPath $testParent -Recurse -Force
    }
}
