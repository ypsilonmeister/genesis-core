# =============================================================================
# run.ps1 — 30-day run script for Windows (PowerShell) with Self-Healing Loop
#
# Usage:
#   .\run.ps1
#
# Prerequisites:
#   - Build all binaries with `cargo build --workspace` into target/debug/
#   - agy CLI must be installed and authenticated
#   - Edit .env to set API keys
# =============================================================================

$ErrorActionPreference = "Stop"

# Circuit Breaker Configuration (6 hours window, max 3 crashes)
$CircuitBreakerWindowMin = 360
$CircuitBreakerMaxCrashes = 3
$CrashHistory = @()

# Change directory to the script location
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
if ($ScriptDir) {
    Set-Location $ScriptDir
}

# Helper to load/reload .env variables into the process
function Load-Env {
    if (Test-Path ".env") {
        Get-Content ".env" -Encoding UTF8 | ForEach-Object {
            $line = $_.Trim()
            if ($line.Length -gt 0 -and $line[0] -eq 65279) {
                $line = $line.Substring(1)
            }
            if ($line -and -not $line.StartsWith("#")) {
                if ($line -match '^([^=]+)=(.*)$') {
                    $key = $Matches[1].Trim()
                    $val = $Matches[2].Trim()
                    if ($val -match '^"([^"]*)"(.*)$') {
                        $val = $Matches[1]
                    } elseif ($val -match "^'([^']*)'(.*)$") {
                        $val = $Matches[1]
                    } else {
                        if ($val -match '^(.*?)\s*#.*$') {
                            $val = $Matches[1].Trim()
                        }
                    }
                    [System.Environment]::SetEnvironmentVariable($key, $val, "Process")
                }
            }
        }
    }
}

# Set Default Environment Variables (Apply only if not set)
function Set-DefaultEnv {
    if ([string]::IsNullOrEmpty($env:REPAIR_BACKEND)) {
        if ([string]::IsNullOrEmpty($env:CLAUDE_BACKEND)) {
            $env:REPAIR_BACKEND = "claude"
        } else {
            $env:REPAIR_BACKEND = $env:CLAUDE_BACKEND
        }
    }
    if ([string]::IsNullOrEmpty($env:ATTACK_BACKEND)) {
        if ([string]::IsNullOrEmpty($env:GEMINI_BACKEND)) {
            $env:ATTACK_BACKEND = "gemini"
        } else {
            $env:ATTACK_BACKEND = $env:GEMINI_BACKEND
        }
    }
    if ([string]::IsNullOrEmpty($env:ATTACK_PHASE)) { $env:ATTACK_PHASE = "D" }
    if ([string]::IsNullOrEmpty($env:ATTACK_INTERVAL_MIN_SECS)) { $env:ATTACK_INTERVAL_MIN_SECS = "30" }
    if ([string]::IsNullOrEmpty($env:ATTACK_INTERVAL_MAX_SECS)) { $env:ATTACK_INTERVAL_MAX_SECS = "180" }
    if ([string]::IsNullOrEmpty($env:TIER1_TRIGGER_COUNT)) { $env:TIER1_TRIGGER_COUNT = "3" }
    if ([string]::IsNullOrEmpty($env:TIER2_TRIGGER_COUNT)) { $env:TIER2_TRIGGER_COUNT = "5" }
    if ([string]::IsNullOrEmpty($env:RUST_LOG)) { $env:RUST_LOG = "info,orchestrator=debug" }
}

# Create UDS socket directory
$socketDir = "\tmp\genesis-core"
if (-not (Test-Path $socketDir)) {
    New-Item -ItemType Directory -Path $socketDir -Force | Out-Null
}

$LogFile = "orchestrator_run.log"
$MaxAgyAttempts = 3

while ($true) {
    # 1. Load env and defaults
    Load-Env
    Set-DefaultEnv

    # Circuit Breaker Check
    $now = [DateTime]::Now
    $CrashHistory = $CrashHistory | Where-Object { ($now - $_).TotalMinutes -le $CircuitBreakerWindowMin }
    if ($CrashHistory.Count -ge $CircuitBreakerMaxCrashes) {
        Write-Host "[run.ps1] [ERROR] Circuit Breaker triggered! $CircuitBreakerMaxCrashes crashes detected within $CircuitBreakerWindowMin minutes." -ForegroundColor Red
        Write-Host "[run.ps1] Terminating script to prevent infinite repair loops and API key drainage."
        Read-Host "Press Enter to exit..."
        break
    }

    $dateString = Get-Date -Format "yyyy-MM-ddTHH:mm:sszzz"
    Write-Host "[run.ps1] Starting orchestrator at $dateString"
    Write-Host "[run.ps1] ATTACK_PHASE=$env:ATTACK_PHASE, REPAIR_BACKEND=$env:REPAIR_BACKEND, ATTACK_BACKEND=$env:ATTACK_BACKEND"

    # 2. Check build
    if (-not (Test-Path "target\debug\orchestrator.exe")) {
        Write-Host "[run.ps1] orchestrator binary not found, building..."
        cargo build --workspace
    }

    # 3. Kill any leftover module processes from a previous run
    $moduleNames = Get-ChildItem -Path "modules" -Directory | Select-Object -ExpandProperty Name
    $leftover = Get-Process | Where-Object { $_.Name -in $moduleNames } -ErrorAction SilentlyContinue
    if ($leftover) {
        Write-Host "[run.ps1] Killing $($leftover.Count) leftover module process(es): $($leftover.Name -join ', ')"
        $leftover | Stop-Process -Force -ErrorAction SilentlyContinue
    }

    # 4. Execute orchestrator with logging
    Write-Host "[run.ps1] Running orchestrator (logging to $LogFile)..."
    try {
        # Use cmd.exe /c to avoid NativeCommandError (red text exception) in PowerShell when stderr is piped
        cmd.exe /c "cargo run -p orchestrator 2>&1" | Tee-Object -FilePath $LogFile
        $exitCode = $LASTEXITCODE
    } catch {
        $exitCode = 1
    }

    if ($exitCode -eq 0) {
        Write-Host "[run.ps1] Orchestrator exited successfully (0). Exiting loop."
        break
    }

    # 4. Exit if crashed (Self-Healing disabled to keep git clean)
    Write-Host "[run.ps1] Orchestrator crashed with exit code $exitCode."
    Write-Host "[run.ps1] Self-healing is disabled to prevent git pollution. Manual intervention required."
    Read-Host "Press Enter to exit..."
    break
}
