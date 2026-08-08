[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$InstalledExe,
    [Parameter(Mandatory = $true)][string]$Installer,
    [Parameter(Mandatory = $true)][string]$QualityReport
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest
$repo = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$exe = (Resolve-Path -LiteralPath $InstalledExe).Path
$installerPath = (Resolve-Path -LiteralPath $Installer).Path
$qualityPath = (Resolve-Path -LiteralPath $QualityReport).Path
if ($exe -like "$repo\src-tauri\target\*") {
    throw "The release-tree executable cannot substitute for an installed candidate."
}

function Invoke-CandidateJson {
    param([string[]]$Arguments)
    $output = & $exe @Arguments
    if ($LASTEXITCODE -ne 0) { throw "Candidate command failed ($LASTEXITCODE): $($Arguments -join ' ')" }
    return ($output -join "`n" | ConvertFrom-Json)
}

$created = Invoke-CandidateJson @("--phase8-self-test", "create-run")
$runRoot = $created.runRoot
if ($runRoot -notmatch '^G:\\test\\local-photo-manager-phase8\\fpm-phase8-run-[0-9a-f-]+$') {
    throw "Candidate returned an invalid run root: $runRoot"
}
Copy-Item -LiteralPath $qualityPath -Destination (Join-Path $runRoot "source-quality-report.json")

Invoke-CandidateJson @("--phase8-self-test", "scale", "--run-root", $runRoot, "--output", "scale-report.json") | Out-Null
Invoke-CandidateJson @("--phase8-self-test", "candidate-manifest", "--run-root", $runRoot, "--installer", $installerPath, "--output", "candidate-manifest.json") | Out-Null
Invoke-CandidateJson @("--phase8-self-test", "io", "--run-root", $runRoot, "--output", "io-report.json") | Out-Null
Invoke-CandidateJson @("--phase8-self-test", "safety", "--run-root", $runRoot, "--output", "safety-report.json") | Out-Null
Invoke-CandidateJson @("--phase8-self-test", "database", "--run-root", $runRoot, "--output", "database-report.json") | Out-Null
Invoke-CandidateJson @("--phase8-self-test", "cache", "--run-root", $runRoot, "--output", "cache-report.json") | Out-Null
Invoke-CandidateJson @("--phase8-self-test", "performance", "--run-root", $runRoot, "--output", "performance-report.json") | Out-Null

$uiOutput = node.exe (Join-Path $PSScriptRoot "phase-eight-ui-check.mjs") --installed-exe $exe --run-root $runRoot
if ($LASTEXITCODE -ne 0) { throw "Candidate UI matrix failed ($LASTEXITCODE)." }
$ui = $uiOutput -join "`n" | ConvertFrom-Json
if (-not $ui.passed) { throw "Candidate UI report did not pass." }
if (-not $ui.evidenceDirectory.StartsWith("$runRoot\", [StringComparison]::OrdinalIgnoreCase)) {
    throw "Candidate UI evidence escaped the verified run root."
}
$uiReportRelative = Join-Path $ui.evidenceDirectory.Substring($runRoot.Length + 1) "ui-candidate-report.json"

node.exe (Join-Path $PSScriptRoot "phase-eight-report.mjs") --run-root $runRoot | Out-Null
if ($LASTEXITCODE -ne 0) { throw "Phase 8 report generation failed ($LASTEXITCODE)." }

$commonEvidence = @(
    "--evidence", "acceptance-matrix.json",
    "--evidence", "defects-report.json",
    "--evidence", "scale-report.json",
    "--evidence", "io-report.json",
    "--evidence", "database-report.json",
    "--evidence", "cache-report.json",
    "--evidence", "performance-report.json",
    "--evidence", "source-quality-report.json",
    "--evidence", $uiReportRelative
)
$importArguments = @(
    "--phase8-self-test", "create-admission", "--run-root", $runRoot,
    "--manifest", "candidate-manifest.json", "--feature", "import",
    "--safety-report", "import-safety-report.json", "--output", "import-admission.json"
) + $commonEvidence
Invoke-CandidateJson $importArguments | Out-Null
$recycleArguments = @(
    "--phase8-self-test", "create-admission", "--run-root", $runRoot,
    "--manifest", "candidate-manifest.json", "--feature", "recycle",
    "--safety-report", "recycle-safety-report.json", "--output", "recycle-admission.json"
) + $commonEvidence
Invoke-CandidateJson $recycleArguments | Out-Null

Invoke-CandidateJson @(
    "--phase8-self-test", "verify-admissions", "--run-root", $runRoot,
    "--manifest", "candidate-manifest.json", "--import", "import-admission.json",
    "--recycle", "recycle-admission.json", "--output", "admission-verification.json"
) | Out-Null
Invoke-CandidateJson @(
    "--phase8-self-test", "verify-candidate", "--run-root", $runRoot,
    "--manifest", "candidate-manifest.json", "--output", "candidate-verification.json"
) | Out-Null

$result = [ordered]@{
    runRoot = $runRoot
    installedExe = $exe
    installer = $installerPath
    uiEvidenceDirectory = $ui.evidenceDirectory
    passed = $true
}
Write-Output ($result | ConvertTo-Json -Depth 4)
