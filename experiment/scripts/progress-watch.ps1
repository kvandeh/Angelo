# Live dashboard for run-experiment.ps1. Reads state/progress.json and
# results.jsonl and redraws every 3 seconds. Safe to close and reopen; it only
# reads, never writes.

$Experiment = Split-Path -Parent $PSScriptRoot
$StateFile = Join-Path $Experiment 'state\progress.json'
$ResultsFile = Join-Path $Experiment 'results.jsonl'

$host.UI.RawUI.WindowTitle = 'Angelo experiment progress'

while ($true) {
    Clear-Host
    Write-Host '=== Angelo 0.3.0 mutation-testing experiment ===' -ForegroundColor Cyan
    Write-Host ''

    if (-not (Test-Path $StateFile)) {
        Write-Host 'Waiting for run-experiment.ps1 to start...'
        Start-Sleep -Seconds 3
        continue
    }

    try {
        $state = Get-Content $StateFile -Raw | ConvertFrom-Json
    } catch {
        Start-Sleep -Seconds 3
        continue
    }

    $total = $state.total
    $index = $state.index
    $completed = $state.completed
    $pct = if ($total -gt 0) { [math]::Round(($index / $total) * 100) } else { 0 }
    $barWidth = 40
    $filled = [math]::Round($barWidth * $pct / 100)
    $bar = ('#' * $filled) + ('-' * ($barWidth - $filled))

    Write-Host "[$bar] $pct%  ($index / $total repos)"
    Write-Host ''
    Write-Host "Current repo : $($state.current_repo)"
    Write-Host "Stage        : $($state.current_status)"
    if ($state.current_started_at) {
        $started = [datetime]$state.current_started_at
        $elapsed = (Get-Date) - $started
        Write-Host ("Elapsed here : {0:mm\:ss}" -f $elapsed)
    }
    Write-Host ''

    $rows = @()
    if (Test-Path $ResultsFile) {
        $rows = Get-Content $ResultsFile | ForEach-Object {
            try { $_ | ConvertFrom-Json } catch { $null }
        } | Where-Object { $_ -ne $null }
    }

    $ok = ($rows | Where-Object { $_.status -eq 'ok' }).Count
    $timedOut = ($rows | Where-Object { $_.status -eq 'timed_out_by_harness' }).Count
    $failed = ($rows | Where-Object { $_.status -notin @('ok', 'timed_out_by_harness') }).Count
    $avgSeconds = 0
    $withTime = $rows | Where-Object { $_.wall_seconds } | ForEach-Object { $_.wall_seconds }
    if ($withTime.Count -gt 0) { $avgSeconds = ($withTime | Measure-Object -Average).Average }

    Write-Host "Completed    : $completed / $total"
    Write-Host "  ok           : $ok"
    Write-Host "  timed out    : $timedOut"
    Write-Host "  failed/other : $failed"
    Write-Host ''

    if ($avgSeconds -gt 0 -and $total -gt $completed) {
        $remaining = $total - $completed
        $etaSeconds = $remaining * $avgSeconds
        $eta = [timespan]::FromSeconds($etaSeconds)
        Write-Host ("Avg time/repo so far : {0:mm\:ss}" -f ([timespan]::FromSeconds($avgSeconds)))
        Write-Host ("Estimated remaining  : {0:hh\:mm\:ss} (rough -- varies a lot by repo size)" -f $eta)
    }

    Write-Host ''
    Write-Host "Last updated: $($state.updated_at)" -ForegroundColor DarkGray
    Write-Host 'Ctrl+C to close this window (the experiment keeps running in the background).' -ForegroundColor DarkGray

    Start-Sleep -Seconds 3
}
