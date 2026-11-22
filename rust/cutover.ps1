#!/usr/bin/env pwsh
[CmdletBinding()]
param(
  [Parameter(Position = 0)]
  [string]$Command = "",

  [Parameter(Position = 1)]
  [string]$Target = "",

  [switch]$Apply
)

$ErrorActionPreference = "Stop"

$RustRoot = $PSScriptRoot
$RepoRoot = (Resolve-Path (Join-Path $RustRoot "..")).Path
$IsWindowsHost = ($env:OS -eq "Windows_NT") -or ($IsWindows -eq $true)
$ExeExt = if ($IsWindowsHost) { ".exe" } else { "" }

function Test-TruthyEnv {
  param([string]$Value)
  if ([string]::IsNullOrWhiteSpace($Value)) {
    return $false
  }
  $normalized = $Value.Trim().ToLowerInvariant()
  return $normalized -eq "1" -or $normalized -eq "true"
}

function Resolve-ExistingPath {
  param([string[]]$Candidates)
  foreach ($candidate in $Candidates) {
    if (-not [string]::IsNullOrWhiteSpace($candidate) -and (Test-Path -LiteralPath $candidate -PathType Leaf)) {
      return $candidate
    }
  }
  return $null
}

function Show-Usage {
  Write-Host "Usage: pwsh rust/cutover.ps1 <status|verify|switch <rust|legacy>|help> [-Apply]"
  Write-Host ""
  Write-Host "Commands:"
  Write-Host "  status               Inspect active mode and Rust artifacts across all 5 surfaces"
  Write-Host "  verify               Run non-destructive surface cutover and test checks"
  Write-Host "  switch <rust|legacy> Print/apply kill-switch settings (add -Apply to stage workers)"
  Write-Host "  help                 Show this help"
}

function Show-Status {
  $cliEnv = $env:LEGIONCODE_LEGACY_NODE
  $cliMode = if (Test-TruthyEnv $cliEnv) { "legacy (LEGIONCODE_LEGACY_NODE=$cliEnv)" } else { "rust" }
  $cliBin = Resolve-ExistingPath @(
    $env:LEGIONCODE_RUST_BIN,
    (Join-Path $RustRoot "dist\bin\legioncode$ExeExt"),
    (Join-Path $RustRoot "target\release\legioncode$ExeExt")
  )
  $serverDistExe = Resolve-ExistingPath @((Join-Path $RepoRoot "apps\server\dist-exe\legioncode$ExeExt"))
  $rustWebApp = Resolve-ExistingPath @((Join-Path $RustRoot "dist\web-app\index.html"))

  $desktopEnv = $env:LEGIONCODE_LEGACY_ELECTRON
  $desktopMode = if (Test-TruthyEnv $desktopEnv) { "legacy (LEGIONCODE_LEGACY_ELECTRON=$desktopEnv)" } else { "rust" }
  $desktopBin = Resolve-ExistingPath @(
    (Join-Path $RustRoot "dist\desktop\legioncode-desktop$ExeExt"),
    (Join-Path $RustRoot "target\release\legioncode-desktop$ExeExt")
  )

  $webEnv = $env:LEGIONCODE_LEGACY_WEB
  $webMode = if (Test-TruthyEnv $webEnv) { "legacy (LEGIONCODE_LEGACY_WEB=$webEnv)" } else { "rust" }
  $wasmNode = Resolve-ExistingPath @((Join-Path $RustRoot "dist\wasm-nodejs\legion_wasm_ffi.js"))
  $wasmWeb = Resolve-ExistingPath @((Join-Path $RustRoot "dist\wasm-web\legion_wasm_ffi.js"))

  $mobileScript = Resolve-ExistingPath @((Join-Path $RepoRoot "apps\mobile\scripts\sync-rust-mobile-ffi.mjs"))
  $mobileFiles = @(
    "dist\mobile\include\legion_mobile_ffi.h",
    "dist\mobile\swift\LegionMobileCore.swift",
    "dist\mobile\kotlin\LegionMobileCore.kt",
    "dist\mobile\android\LegionMobileBridge.kt",
    "dist\mobile\apple\Package.swift"
  )
  $mobileReady = ($mobileFiles | Where-Object { -not (Test-Path -LiteralPath (Join-Path $RustRoot $_) -PathType Leaf) }).Count -eq 0

  $workerEnv = $env:LEGIONCODE_LEGACY_WORKER
  $workerMode = if (Test-TruthyEnv $workerEnv) { "legacy (LEGIONCODE_LEGACY_WORKER=$workerEnv)" } else { "rust" }
  $workerFiles = @(
    "dist\cloudflare-workers\worker.mjs",
    "dist\cloudflare-workers\worker_runtime.mjs",
    "dist\cloudflare-workers\wrangler.toml",
    "dist\cloudflare-workers\legion_wasm_ffi.js",
    "dist\cloudflare-workers\legion_wasm_ffi_bg.wasm"
  )
  $workerDistReady = ($workerFiles | Where-Object { -not (Test-Path -LiteralPath (Join-Path $RustRoot $_) -PathType Leaf) }).Count -eq 0
  $relayStaged = Test-Path -LiteralPath (Join-Path $RepoRoot "infra\relay\dist\rust-worker\worker.mjs") -PathType Leaf
  $taskCacheStaged = Test-Path -LiteralPath (Join-Path $RepoRoot "infra\task-cache\dist\rust-worker\worker.mjs") -PathType Leaf

  Write-Host "LegionCode Rust Cutover Status ($RepoRoot)"
  Write-Host "------------------------------------------------------------"
  Write-Host ("CLI / Server       : {0}" -f $cliMode)
  Write-Host ("  Rust binary      : {0}" -f ($cliBin ?? "missing"))
  Write-Host ("  dist-exe staged  : {0}" -f ($serverDistExe ?? "missing"))
  Write-Host ("  dist/web-app     : {0}" -f ($rustWebApp ?? "missing"))
  Write-Host ("Desktop            : {0}" -f $desktopMode)
  Write-Host ("  Rust binary      : {0}" -f ($desktopBin ?? "missing"))
  Write-Host ("Web WASM           : {0}" -f $webMode)
  Write-Host ("  wasm-nodejs      : {0}" -f ($wasmNode ?? "missing"))
  Write-Host ("  wasm-web         : {0}" -f ($wasmWeb ?? "missing"))
  Write-Host ("Mobile FFI         : sync:rust-ffi (script: {0})" -f $(if ($mobileScript) { "present" } else { "pending" }))
  Write-Host ("  dist/mobile      : {0}" -f $(if ($mobileReady) { "complete (5/5)" } else { "incomplete" }))
  Write-Host ("Cloudflare Workers : {0}" -f $workerMode)
  Write-Host ("  dist/workers     : {0}" -f $(if ($workerDistReady) { "complete (5/5)" } else { "incomplete" }))
  Write-Host ("  relay staged     : {0}" -f $(if ($relayStaged) { "yes" } else { "no (run stage:rust)" }))
  Write-Host ("  task-cache staged: {0}" -f $(if ($taskCacheStaged) { "yes" } else { "no (run stage:rust)" }))
}

function Invoke-Checked {
  param(
    [string]$Label,
    [string]$Executable,
    [string[]]$Arguments
  )
  Write-Host "[verify] $Label..."
  & $Executable @Arguments
  if ($LASTEXITCODE -ne 0) {
    throw "Verification failed ($Label) with exit code $LASTEXITCODE."
  }
}

function Invoke-Verify {
  Push-Location $RepoRoot
  try {
    $cliBin = Resolve-ExistingPath @(
      $env:LEGIONCODE_RUST_BIN,
      (Join-Path $RustRoot "dist\bin\legioncode$ExeExt"),
      (Join-Path $RustRoot "target\release\legioncode$ExeExt")
    )
    if ($cliBin) {
      Invoke-Checked "Rust CLI --version" $cliBin @("--version")
    } else {
      Write-Host "[verify] Rust CLI binary missing; skipping --version check."
    }

    $testFiles = @(
      "apps/server/src/rustCutover.test.ts",
      "scripts/build-cli-archive.test.ts"
    )
    $wasmTest = Join-Path $RepoRoot "packages\client-runtime\src\wasm\rustCoreBridge.test.ts"
    if (Test-Path -LiteralPath $wasmTest -PathType Leaf) {
      $testFiles += "packages/client-runtime/src/wasm/rustCoreBridge.test.ts"
    }
    Invoke-Checked "Vitest cutover tests" "vp" (@("test", "run") + $testFiles)

    $desktopBin = Resolve-ExistingPath @(
      (Join-Path $RustRoot "dist\desktop\legioncode-desktop$ExeExt"),
      (Join-Path $RustRoot "target\release\legioncode-desktop$ExeExt")
    )
    $startElectron = Join-Path $RepoRoot "apps\desktop\scripts\start-electron.mjs"
    if ($desktopBin -and (Test-Path -LiteralPath $startElectron -PathType Leaf)) {
      Invoke-Checked "Desktop Rust --self-check" "node" @($startElectron, "--self-check")
    }

    $mobileScript = Join-Path $RepoRoot "apps\mobile\scripts\sync-rust-mobile-ffi.mjs"
    if (Test-Path -LiteralPath $mobileScript -PathType Leaf) {
      Invoke-Checked "Mobile FFI --check" "node" @($mobileScript, "--check")
    }

    foreach ($workerPkg in @("relay", "task-cache")) {
      $workerScript = Join-Path $RepoRoot "infra\$workerPkg\scripts\stage-rust-worker.mjs"
      if (Test-Path -LiteralPath $workerScript -PathType Leaf) {
        Invoke-Checked "$workerPkg worker --check" "node" @($workerScript, "--check")
      }
    }

    Write-Host "[verify] All cutover checks passed."
  } finally {
    Pop-Location
  }
}

function Invoke-Switch {
  param([string]$Mode)
  $normalized = $Mode.Trim().ToLowerInvariant()
  if ($normalized -ne "rust" -and $normalized -ne "legacy") {
    throw "Unknown switch mode '$Mode'. Expected 'rust' or 'legacy'."
  }

  $vars = @(
    "LEGIONCODE_LEGACY_NODE",
    "LEGIONCODE_LEGACY_ELECTRON",
    "LEGIONCODE_LEGACY_WEB",
    "LEGIONCODE_LEGACY_WORKER"
  )

  if ($normalized -eq "rust") {
    foreach ($name in $vars) {
      Remove-Item "Env:$name" -ErrorAction SilentlyContinue
    }
    Write-Host "Switched current process environment to RUST cutover mode."
    Write-Host "To apply in your PowerShell session:"
    foreach ($name in $vars) {
      Write-Host "  Remove-Item Env:$name -ErrorAction SilentlyContinue"
    }
    Write-Host "Mobile FFI sync: pnpm --filter @legioncode/mobile sync:rust-ffi"
    Write-Host "Worker staging : pnpm --filter @legioncode/relay stage:rust && pnpm --filter @legioncode/task-cache stage:rust"

    if ($Apply) {
      Push-Location $RepoRoot
      try {
        foreach ($workerPkg in @("relay", "task-cache")) {
          $workerScript = Join-Path $RepoRoot "infra\$workerPkg\scripts\stage-rust-worker.mjs"
          if (Test-Path -LiteralPath $workerScript -PathType Leaf) {
            Invoke-Checked "Stage $workerPkg Rust worker" "node" @($workerScript)
          }
        }
      } finally {
        Pop-Location
      }
    }
  } else {
    foreach ($name in $vars) {
      Set-Item "Env:$name" "1"
    }
    Write-Host "Switched current process environment to LEGACY rollback mode."
    Write-Host "To apply in your PowerShell session:"
    foreach ($name in $vars) {
      Write-Host "  `$env:$name = '1'"
    }
  }
}

if ([string]::IsNullOrWhiteSpace($Command)) {
  $isInteractive = [Environment]::UserInteractive -and -not [Console]::IsInputRedirected
  if (-not $isInteractive) {
    Show-Status
    exit 0
  }

  Write-Host "LegionCode Cutover Controller"
  Write-Host "  1) status"
  Write-Host "  2) verify"
  Write-Host "  3) switch rust"
  Write-Host "  4) switch legacy"
  Write-Host "  5) exit"
  $choice = (Read-Host "Select action [1-5]").Trim()
  switch ($choice) {
    "1" { $Command = "status" }
    "2" { $Command = "verify" }
    "3" { $Command = "switch"; $Target = "rust" }
    "4" { $Command = "switch"; $Target = "legacy" }
    "5" { exit 0 }
    default { throw "Invalid selection '$choice'." }
  }
}

switch ($Command.Trim().ToLowerInvariant()) {
  "status" { Show-Status }
  "verify" { Invoke-Verify }
  "switch" {
    if ([string]::IsNullOrWhiteSpace($Target)) {
      $Target = Read-Host "Target mode (rust|legacy)"
    }
    Invoke-Switch $Target
  }
  "help" { Show-Usage }
  "-h" { Show-Usage }
  "--help" { Show-Usage }
  default { throw "Unknown command '$Command'. Run with 'help' for usage." }
}
