# =============================================================================
# status.ps1 — Genesis Core System Status (PowerShell)
#
# Usage:
#   .\status.ps1
#
# Prerequisites:
#   - sqlite3 must be in PATH and executable
# =============================================================================

$ErrorActionPreference = "Stop"

# Change directory to the script location
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
if ($ScriptDir) {
    Set-Location $ScriptDir
}

$DbPath = ".\metadata.db"

if (-not (Test-Path $DbPath)) {
    Write-Error "Error: $DbPath not found."
    exit 1
}

Write-Host "=== Genesis Core System Status ==="
Write-Host "Date: $(Get-Date)"
Write-Host "----------------------------------"

# Query SQLite database and format output

# Latest attacks
Write-Host "[Latest Attacks]"
$attacksQuery = "SELECT timestamp, phase, diversity_score FROM attacks ORDER BY id DESC LIMIT 5;"
try {
    $attacksCsv = sqlite3 -csv -header $DbPath $attacksQuery 2>$null
    if ($attacksCsv) {
        $attacksCsv | ConvertFrom-Csv | Format-Table -AutoSize
    } else {
        Write-Host "No attack logs found."
    }
} catch {
    Write-Host "Failed to query database. Ensure sqlite3 is available."
    Write-Host $_
}
Write-Host ""

# Latest modifications
Write-Host "[Latest Modifications]"
$modQuery = "SELECT timestamp, module_name, tier, build_result FROM modifications ORDER BY id DESC LIMIT 5;"
try {
    $modCsv = sqlite3 -csv -header $DbPath $modQuery 2>$null
    if ($modCsv) {
        $modCsv | ConvertFrom-Csv | Format-Table -AutoSize
    } else {
        Write-Host "No modification logs found."
    }
} catch {
    Write-Host "Failed to query database. Ensure sqlite3 is available."
    Write-Host $_
}
Write-Host ""

# Summary statistics
Write-Host "[Summary Statistics]"
try {
    $totalAttacksQuery = "SELECT count(*) AS [Total Attacks] FROM attacks;"
    $totalAttacksCsv = sqlite3 -csv -header $DbPath $totalAttacksQuery 2>$null
    if ($totalAttacksCsv) {
        $totalAttacks = $totalAttacksCsv | ConvertFrom-Csv
        Write-Host "Total Attacks: $($totalAttacks.'Total Attacks')"
    }

    $totalModsQuery = "SELECT count(*) AS [Total Modifications] FROM modifications;"
    $totalModsCsv = sqlite3 -csv -header $DbPath $totalModsQuery 2>$null
    if ($totalModsCsv) {
        $totalMods = $totalModsCsv | ConvertFrom-Csv
        Write-Host "Total Modifications: $($totalMods.'Total Modifications')"
    }
} catch {
    Write-Host "Failed to fetch summary statistics."
}
Write-Host "----------------------------------"
