$ErrorActionPreference = 'Continue'
$root = Split-Path $PSScriptRoot -Parent
Set-Location $root
$log = New-Object System.Collections.Generic.List[string]

function Step($name, [scriptblock]$body) {
    Write-Host "== $name"
    $global:LASTEXITCODE = 0
    & $body 2>&1 | ForEach-Object { Write-Host "   $_" }
    $code = $LASTEXITCODE
    if ($code -ne 0) {
        $log.Add("FAIL  $name (exit $code)")
        $log | ForEach-Object { Write-Host $_ }
        Write-Host "PRECHECK FAILED. fix the step above and run again."
        exit 1
    }
    $log.Add("PASS  $name")
}

Step "unit tests" { cargo test --manifest-path src-tauri\Cargo.toml }
Step "release build" { cargo build --release --manifest-path src-tauri\Cargo.toml }
Step "app.js syntax" { node --check frontend\app.js }
Step "fetch script syntax" { python -m py_compile scripts\fetch_verified_shim.py }
Step "whitespace" { git diff --check; git diff --cached --check }
Step "no schema files staged" {
    $staged = git diff --cached --name-only
    if ($staged -match 'gen/schemas') { Write-Host "staged generated schema files"; $global:LASTEXITCODE = 1 }
}
Step "no co-author lines in last 20 commits" {
    $bad = git log -n 20 --format=%B | Select-String -Pattern 'Co-authored-by' -SimpleMatch
    if ($bad) { $bad | ForEach-Object { Write-Host $_.Line }; $global:LASTEXITCODE = 1 }
}
Step "live signed list (tamper test)" {
    $tt = "D:\dev\HorseyModServer\tamper_test.py"
    if (Test-Path $tt) { python $tt } else { Write-Host "tamper_test.py not found, skipped"; $global:LASTEXITCODE = 0 }
}

$log | ForEach-Object { Write-Host $_ }
Write-Host "PRECHECK PASSED"