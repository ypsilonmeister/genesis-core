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

    # 4. Self-Healing Loop if crashed
    Write-Host "[run.ps1] Orchestrator crashed with exit code $exitCode."
    
    # Track crash history for circuit breaker
    $CrashHistory += [DateTime]::Now

    # Ensure git config has name/email so commit doesn't fail
    $gitName = git config user.name
    if ([string]::IsNullOrEmpty($gitName)) {
        git config user.name "Self-Healing Bot"
        git config user.email "self-healing@genesis.local"
    }

    # Create pre-repair git backup commit to serve as a rollback point
    Write-Host "[run.ps1] Creating pre-repair git backup..."
    git add src/ modules/ orchestrator/src/ Cargo.toml Cargo.lock chain.toml
    git commit -m "Self-healing backup: Orchestrator crashed" | Out-Null
    
    $agyAttempt = 1
    $resolved = $false
    
    while ($agyAttempt -le $MaxAgyAttempts) {
        Write-Host "[run.ps1] Invoking gemini for self-healing (Attempt $agyAttempt/$MaxAgyAttempts)..."
        
        # Read last 50 lines of crash log
        $crashLog = ""
        if (Test-Path $LogFile) {
            $crashLog = Get-Content $LogFile -Tail 50 | Out-String
        }
        
        $prompt = @"
The orchestrator has crashed. Please analyze the exception, fix the source code (or configuration files) to resolve the issue, and ensure the workspace builds and all tests pass successfully.
The workspace root directory is "$ScriptDir". You have full permission to read and write files in this directory.

--- CRASH LOGS ---
$crashLog
------------------
"@
        
        # Invoke gemini directly
        gemini -p "$prompt" -y
        
        # Verify if compilation and tests pass now
        Write-Host "[run.ps1] Verifying build and tests after repair..."
        $buildCheck = Start-Process -FilePath "cargo" -ArgumentList "check --workspace" -NoNewWindow -PassThru -Wait
        $testCheck = $null
        if ($buildCheck.ExitCode -eq 0) {
            $testCheck = Start-Process -FilePath "cargo" -ArgumentList "test --workspace" -NoNewWindow -PassThru -Wait
        }
        
        if ($buildCheck.ExitCode -eq 0 -and $testCheck -and $testCheck.ExitCode -eq 0) {
            Write-Host "[run.ps1] Build and all tests succeeded!"
            # Amend the temporary backup commit to a descriptive fix commit
            $commitMsg = "[Self-Healing] Fix crash (Attempt $agyAttempt)`n`nCrash Log Tail:`n$($crashLog.Trim())"
            git add src/ modules/ orchestrator/src/ Cargo.toml Cargo.lock chain.toml
            git commit --amend -m $commitMsg | Out-Null
            $resolved = $true
            break
        } else {
            Write-Host "[run.ps1] Build or tests failed after repair attempt. Rolling back changes..."
            # Reset to the pre-repair backup state
            git reset --hard HEAD | Out-Null
            $agyAttempt++
        }
    }

    if ($resolved) {
        Write-Host "[run.ps1] Re-starting orchestrator..."
    } else {
        Write-Host "[run.ps1] Self-healing failed after $MaxAgyAttempts attempts."
        # Remove the temporary backup commit but keep the files in their original crashed state
        git reset --soft HEAD~1 | Out-Null
        Write-Host "[run.ps1] Restored files to original crash state. Manual intervention required."
        Read-Host "Press Enter to exit..."
        break
    }
}
