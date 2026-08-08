[CmdletBinding()]
param(
    [string]$Output = "artifacts\phase-eight-source-quality.json"
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest
$repo = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
Set-Location $repo

$results = [System.Collections.Generic.List[object]]::new()
function Invoke-Gate {
    param([string]$Name, [scriptblock]$Command)
    $timer = [System.Diagnostics.Stopwatch]::StartNew()
    & $Command
    $exit = $LASTEXITCODE
    $timer.Stop()
    $results.Add([ordered]@{ name = $Name; exitCode = $exit; durationMs = $timer.ElapsedMilliseconds; passed = ($exit -eq 0) })
    if ($exit -ne 0) { throw "Quality gate failed: $Name ($exit)" }
}

Invoke-Gate "prettier" { npx.cmd prettier --check src package.json package-lock.json scripts/phase-eight-ui-check.mjs scripts/phase-eight-report.mjs }
Invoke-Gate "cargo-fmt" { node.exe scripts/run-cargo.mjs fmt --manifest-path src-tauri/Cargo.toml --check }
Invoke-Gate "eslint" { npx.cmd eslint src scripts/phase-eight-ui-check.mjs scripts/phase-eight-report.mjs --max-warnings 0 }
Invoke-Gate "typescript" { npx.cmd tsc -b --pretty false }
Invoke-Gate "frontend-tests" { npx.cmd vitest run --maxWorkers 1 }
Invoke-Gate "e2e-tests" { npx.cmd vitest run --config vitest.e2e.config.ts --maxWorkers 1 }
Invoke-Gate "rust-all-targets" { node.exe scripts/run-cargo.mjs test --manifest-path src-tauri/Cargo.toml --all-targets }
Invoke-Gate "clippy-deny-warnings" { node.exe scripts/run-cargo.mjs clippy --manifest-path src-tauri/Cargo.toml --all-targets --all-features -- -D warnings }
Invoke-Gate "safety-audit" { node.exe scripts/safety-audit.mjs }
Invoke-Gate "git-diff-check" { git.exe -c safe.directory=D:/A/MyProject/ai/Aifirtst diff --check }

$sourceFiles = @(
    Get-ChildItem src, src-tauri\src, src-tauri\migrations -Recurse -File
    Get-Item package.json, package-lock.json, src-tauri\Cargo.toml, src-tauri\Cargo.lock, src-tauri\tauri.conf.json
    Get-ChildItem scripts -File | Where-Object { $_.Name -like "phase-eight-*" -or $_.Name -in @("safety-audit.mjs", "run-cargo.mjs", "run-tauri.mjs") }
) | Sort-Object FullName -Unique
$digestText = foreach ($file in $sourceFiles) {
    if (-not $file.FullName.StartsWith("$repo\", [StringComparison]::OrdinalIgnoreCase)) {
        throw "Source fingerprint path escaped the repository: $($file.FullName)"
    }
    $relative = $file.FullName.Substring($repo.Length + 1).Replace("\", "/")
    $hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $file.FullName).Hash.ToLowerInvariant()
    "$relative`0$hash`n"
}
$sha256 = [System.Security.Cryptography.SHA256]::Create()
try {
    $sourceTreeHashBytes = $sha256.ComputeHash(
        [System.Text.Encoding]::UTF8.GetBytes(($digestText -join ""))
    )
} finally {
    $sha256.Dispose()
}
$sourceTreeSha256 = (($sourceTreeHashBytes | ForEach-Object { $_.ToString("x2") }) -join "")

$report = [ordered]@{
    reportVersion = 1
    generatedAtUtc = [DateTime]::UtcNow.ToString("o")
    sourceTreeSha256 = $sourceTreeSha256
    commands = $results
    passed = ($results.Count -eq 10 -and -not ($results | Where-Object { -not $_.passed }))
}
$outputPath = Join-Path $repo $Output
$outputDirectory = Split-Path -Parent $outputPath
New-Item -ItemType Directory -Path $outputDirectory -Force | Out-Null
if (Test-Path -LiteralPath $outputPath) { throw "Refusing to overwrite $outputPath" }
[System.IO.File]::WriteAllText($outputPath, (($report | ConvertTo-Json -Depth 6) + "`n"), [System.Text.UTF8Encoding]::new($false))
Write-Output ($report | ConvertTo-Json -Depth 6)
