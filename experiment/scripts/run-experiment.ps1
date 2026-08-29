# Drives angelo against every repo in manifest.json and collects the reports for
# a future LLM to read. Run from PowerShell, never from Git Bash --
# CLAUDE.md warns Git Bash mangles path-like arguments passed to the angelo
# binary, and this script passes plenty of them.
#
# Resumable: a repo whose reports/<name>/report.html already exists is skipped,
# so re-running after a crash or an interrupt only does the remaining repos.
#
# -Only <name> restricts the run to one manifest entry, for smoke-testing the
# pipeline before committing to the full batch. It appends to the same
# results.jsonl as a full run, so the previous file is copied aside first: an
# -Only re-run once destroyed a finished 50-repo dataset that had no backup.
#
# -PythonVersion picks the interpreter the *target* venvs are built with, which
# is not the same question as which interpreter runs angelo. It is a list, newest
# first: a repo that will not install on the newest falls back to the next.
#
# -AngeloExe runs a locally built binary instead of the pinned wheel, so a fix
# can be measured against the corpus before it is released. -SampleSize shrinks
# the draw for a smoke test; the corpus itself is drawn at 1000.
param(
    [string]$Only = '',
    [string[]]$PythonVersion = @('3.14', '3.13', '3.12', '3.11'),
    [string]$AngeloExe = '',
    [int]$SampleSize = 1000
)

$ErrorActionPreference = 'Continue'

$Experiment = Split-Path -Parent $PSScriptRoot
$ProjectRoot = Split-Path -Parent $Experiment
$Extra = Join-Path $ProjectRoot 'extra'
$Angelo = if ($AngeloExe) { $AngeloExe } else { Join-Path $Experiment '.venv\Scripts\angelo.exe' }
$ReportsDir = Join-Path $Experiment 'reports'
$LogsDir = Join-Path $Experiment 'logs'
$StateFile = Join-Path $Experiment 'state\progress.json'
$ResultsFile = Join-Path $Experiment 'results.jsonl'
$ReadDeps = Join-Path $PSScriptRoot 'read-deps.py'
$DiagnoseCollect = Join-Path $PSScriptRoot 'diagnose-collect.py'
$PerRepoTimeoutMinutes = 45
# How many times a failed collection may be diagnosed and re-tried. Each round
# installs everything the log named, so two rounds is already generous; the cap
# exists so an unfixable project cannot loop.
$CollectRounds = 3

if (Test-Path $ResultsFile) {
    $stamp = (Get-Date).ToString('yyyyMMdd-HHmmss')
    Copy-Item $ResultsFile "$ResultsFile.$stamp.bak"
    Write-Host "previous results.jsonl kept as results.jsonl.$stamp.bak"
}

# The py launcher lists interpreters out of the registry, and that registry can
# still name an install that has been deleted -- asking it to run one then fails
# with "Unable to create process". So every candidate is made to prove it runs
# before the batch commits to it, and the fallback is announced rather than
# silently leaving 50 venvs on an interpreter nobody chose.
function Resolve-Interpreters {
    param([string[]]$Versions)
    $usable = @()
    foreach ($version in $Versions) {
        $found = (& py "-$version" -c "import sys; print(sys.executable)" 2>$null)
        if ($LASTEXITCODE -eq 0 -and $found -and (Test-Path $found)) { $usable += $found }
        else { Write-Host "python $version is listed but not usable here, skipping" }
    }
    if ($usable.Count -eq 0) { $usable = @((Get-Command python -ErrorAction SilentlyContinue).Source) }
    return $usable
}

$Interpreters = @(Resolve-Interpreters -Versions $PythonVersion)
Write-Host "target interpreters, in order: $($Interpreters -join ', ')"

# Asked of the binary rather than hardcoded, so a row always says which build
# produced it -- the whole point of -AngeloExe.
$AngeloVersion = (& $Angelo --version 2>$null) -replace '^angelo\s+', ''
Write-Host "angelo: $Angelo ($AngeloVersion)"

$manifest = Get-Content (Join-Path $Experiment 'manifest.json') -Raw | ConvertFrom-Json
if ($Only) { $manifest = @($manifest | Where-Object { $_.name -eq $Only }) }

function Write-Progress-State {
    param($Index, $Total, $Name, $Status, $StartedAt, $Completed)
    $state = [ordered]@{
        index = $Index
        total = $Total
        current_repo = $Name
        current_status = $Status
        current_started_at = $StartedAt
        updated_at = (Get-Date).ToString('o')
        completed = $Completed
    }
    # Same BOM trap as the config: a marked file is not what a plain JSON reader
    # expects, and progress.json is read by the dashboard and by hand.
    Write-TextFile -Path $StateFile -Lines (($state | ConvertTo-Json -Depth 5) -split "`r?`n")
}

# Windows PowerShell's -Encoding utf8 writes a BOM. A BOM in front of the first
# TOML key makes that key parse as "﻿paths", so angelo silently took the
# default for whichever setting happened to be written first.
function Write-TextFile {
    param([string]$Path, [string[]]$Lines)
    $utf8NoBom = New-Object System.Text.UTF8Encoding $false
    [System.IO.File]::WriteAllText($Path, ($Lines -join "`r`n") + "`r`n", $utf8NoBom)
}

function Set-ConfigValue {
    param([string]$Path, [string]$Key, [string]$Value)
    $lines = Get-Content $Path
    $found = $false
    $out = foreach ($line in $lines) {
        if ($line -match "^\s*$Key\s*=") {
            $found = $true
            "$Key = $Value"
        } else {
            $line
        }
    }
    if (-not $found) { $out += "$Key = $Value" }
    Write-TextFile -Path $Path -Lines $out
}

function Get-VerdictCounts {
    param([string]$OutLog)
    $counts = [ordered]@{ killed = 0; survived = 0; timeout = 0; error = 0; untestable = 0 }
    $scorePercent = $null
    $scoredTotal = $null
    if (-not (Test-Path $OutLog)) { return @{ counts = $counts; score = $scorePercent; scored = $scoredTotal } }
    $text = Get-Content $OutLog
    foreach ($line in $text) {
        if ($line -match '^\s*(killed|survived|timeout|error|untestable):\s*(\d+)\s*$') {
            $counts[$Matches[1]] = [int]$Matches[2]
        } elseif ($line -match 'score:\s*([\d.]+)%\s*\((\d+)/(\d+)\s*detected\)') {
            $scorePercent = [double]$Matches[1]
            $scoredTotal = [int]$Matches[3]
        }
    }
    return @{ counts = $counts; score = $scorePercent; scored = $scoredTotal }
}

# Installs the project itself, and says whether it went in editable. No extras
# are guessed here: pip exits 0 with only a *warning* when an extra does not
# exist, so a chain of guesses always stopped at the first one and reported
# success having installed no test dependencies at all.
function Install-Project {
    param([string]$Py, [string]$Dir, [string]$Log)
    & $Py -m pip install -q -e $Dir *>> $Log
    if ($LASTEXITCODE -eq 0) { return $true }
    & $Py -m pip install -q $Dir *>> $Log
    return $false
}

# The test dependencies the project actually declares, by name, read out of its
# own pyproject. PEP 735 dependency-groups are where most of the ecosystem now
# keeps them, and `pip install .[extra]` cannot reach a group at all.
function Install-TestDeps {
    param([string]$Py, [string]$Dir, [string]$Log)
    $installed = @()
    $json = & $Py $ReadDeps $Dir 2>$null
    if (-not $json) { return $installed }
    $deps = $json | ConvertFrom-Json
    foreach ($group in @($deps.test_groups)) {
        & $Py -m pip install -q --group $group *>> $Log
        if ($LASTEXITCODE -eq 0) { $installed += "group:$group"; break }
    }
    foreach ($extra in @($deps.test_extras)) {
        & $Py -m pip install -q -e "$Dir[$extra]" *>> $Log
        if ($LASTEXITCODE -eq 0) { $installed += "extra:$extra"; break }
    }
    foreach ($req in @($deps.requirements | Select-Object -First 2)) {
        $path = Join-Path $Dir $req
        if (-not (Test-Path $path)) { continue }
        & $Py -m pip install -q -r $path *>> $Log
        if ($LASTEXITCODE -eq 0) { $installed += "requirements:$req" }
    }
    return $installed
}

# Collect, and when it fails, install what the failure named and collect again.
#
# Deliberately no -x: one unimportable optional module used to abort a
# collection that otherwise named thousands of tests, and seeing every error at
# once is what lets a single install round fix them all.
function Test-Collection {
    param([string]$Py, [string]$Dir, [string]$Name, [string]$Log, [string]$VenvLog)
    $installed = @()
    $ok = $false
    for ($round = 0; $round -lt $CollectRounds; $round++) {
        & $Py -m pytest -q --co *> $Log
        if ($LASTEXITCODE -eq 0) { $ok = $true; break }
        $wanted = @(& $Py $DiagnoseCollect $Log --project $Name 2>$null)
        $wanted = @($wanted | Where-Object { $_ -and ($installed -notcontains $_) })
        if ($wanted.Count -eq 0) { break }
        Write-Host "  collect failed, installing: $($wanted -join ', ')"
        & $Py -m pip install -q @wanted *>> $VenvLog
        $installed += $wanted
    }
    $text = (Get-Content $Log -Raw -ErrorAction SilentlyContinue)
    $collected = 0
    $errors = 0
    if ($text -match '(\d+)\s+tests?\s+collected') { $collected = [int]$Matches[1] }
    if ($text -match '(\d+)\s+error') { $errors = [int]$Matches[1] }
    return @{ ok = $ok; collected = $collected; errors = $errors; installed = $installed }
}

$completedCount = 0
$totalRepos = $manifest.Count
$results = New-Object System.Collections.Generic.List[object]

for ($i = 0; $i -lt $totalRepos; $i++) {
    $entry = $manifest[$i]
    $name = $entry.name
    $dir = Join-Path $Extra $name
    $repoReportsDir = Join-Path $ReportsDir $name
    $htmlReport = Join-Path $repoReportsDir 'report.html'
    $logPath = Join-Path $LogsDir "$name.out.log"
    $errLogPath = Join-Path $LogsDir "$name.err.log"

    if (Test-Path $htmlReport) {
        Write-Host "[$($i+1)/$totalRepos] $name -- already done, skipping"
        $completedCount++
        continue
    }

    Write-Progress-State -Index ($i + 1) -Total $totalRepos -Name $name -Status 'cloning' -StartedAt (Get-Date).ToString('o') -Completed $completedCount
    Write-Host "[$($i+1)/$totalRepos] $name -- starting"
    New-Item -ItemType Directory -Force -Path $repoReportsDir | Out-Null
    $runStart = Get-Date
    $row = [ordered]@{
        name = $name
        repo = $entry.repo
        url = $entry.url
        license = $entry.license
        notes = $entry.notes
        started_at = $runStart.ToString('o')
        angelo_version = $AngeloVersion
        sample_requested = $SampleSize
        status = 'unknown'
    }

    if (-not (Test-Path $dir)) {
        # --filter=blob:none rather than --depth 1: a shallow clone carries no
        # tags, so setuptools_scm versions every project 0.1.dev1 -- which is
        # what made pytest's own `minversion = 2.0` check reject its own tree.
        # A partial clone is nearly as fast and keeps the tags.
        git clone --filter=blob:none $entry.url $dir *> (Join-Path $LogsDir "$name.clone.log")
        if (-not (Test-Path $dir)) {
            $row.status = 'clone_failed'
            $results.Add([pscustomobject]$row)
            ($row | ConvertTo-Json -Compress) | Add-Content -Path $ResultsFile
            $completedCount++
            continue
        }
    }
    # Repos cloned shallow by an earlier run still have no tags, and the version
    # setuptools_scm derives from them is what several projects gate their own
    # pytest config on. Fetching them once is cheaper than a wrong version.
    if ((git -C $dir rev-parse --is-shallow-repository 2>$null) -eq 'true') {
        Write-Host "  unshallowing for tags"
        git -C $dir fetch --unshallow --tags --quiet *>> (Join-Path $LogsDir "$name.clone.log")
    }
    $row.commit_sha = (git -C $dir rev-parse --short HEAD 2>$null)
    $row.described_version = (git -C $dir describe --tags --abbrev=0 2>$null)

    Write-Progress-State -Index ($i + 1) -Total $totalRepos -Name $name -Status 'venv-setup' -StartedAt $runStart.ToString('o') -Completed $completedCount
    $venv = Join-Path $dir '.venv'
    $venvLog = Join-Path $LogsDir "$name.venv.log"
    $py = Join-Path $venv 'Scripts\python.exe'
    # Reused only when it has a working pip *and* the interpreter asked for. A
    # venv left behind by an earlier run on a different version is how repos
    # ended up on 3.14 regardless of what the run was configured to use.
    $reusable = $false
    if (Test-Path $py) {
        & $py -m pip --version *> $venvLog
        $reusable = ($LASTEXITCODE -eq 0)
    }

    # --group and a relative requirements path both resolve against the cwd.
    Push-Location $dir
    if ($reusable) {
        $row.editable_install_ok = (Install-Project -Py $py -Dir $dir -Log $venvLog)
    } else {
        # Newest interpreter first, older only when the project will not install
        # on it. Neither end is right for every repo: 3.14 had no wheels yet for
        # pandas, cryptography or pyyaml, while an older one can be red on a
        # project that only keeps its suite green on the version it develops
        # against -- and one already-failing test makes every mutant its tests
        # cover untestable.
        foreach ($candidate in $Interpreters) {
            Remove-Item -Recurse -Force $venv -ErrorAction SilentlyContinue
            & $candidate -m venv $venv *> $venvLog
            if (-not (Test-Path $py)) { continue }
            & $py -m ensurepip --upgrade *>> $venvLog
            & $py -m pip install -q --upgrade pip *>> $venvLog
            & $py -m pip install -q pytest coverage *>> $venvLog
            $row.editable_install_ok = (Install-Project -Py $py -Dir $dir -Log $venvLog)
            if ($row.editable_install_ok) { break }
            Write-Host "  $candidate could not install the project, trying an older one"
        }
    }
    $row.python_version = (& $py -c "import sys; print('%d.%d' % sys.version_info[:2])" 2>$null)
    $row.test_deps_installed = @(Install-TestDeps -Py $py -Dir $dir -Log $venvLog)

    Write-Progress-State -Index ($i + 1) -Total $totalRepos -Name $name -Status 'collect-check' -StartedAt $runStart.ToString('o') -Completed $completedCount
    $collect = Test-Collection -Py $py -Dir $dir -Name $name -Log (Join-Path $LogsDir "$name.collect.log") -VenvLog $venvLog
    $collectOk = $collect.ok
    $row.collects_cleanly = $collectOk
    $row.collected_tests = $collect.collected
    $row.collect_errors = $collect.errors
    $row.installed_to_collect = @($collect.installed)

    if (-not $collectOk) {
        $row.status = 'collect_failed'
        $row.finished_at = (Get-Date).ToString('o')
        $row.wall_seconds = [math]::Round(((Get-Date) - $runStart).TotalSeconds, 1)
        Pop-Location
        ($row | ConvertTo-Json -Compress) | Add-Content -Path $ResultsFile
        $completedCount++
        Write-Host "  -> collect_failed"
        continue
    }

    Remove-Item -Recurse -Force (Join-Path $dir '.angelo') -ErrorAction SilentlyContinue

    Write-Progress-State -Index ($i + 1) -Total $totalRepos -Name $name -Status 'angelo-init' -StartedAt $runStart.ToString('o') -Completed $completedCount
    $oldPath = $env:PATH
    $env:PATH = "$venv\Scripts;$oldPath"

    # --force, or a config left behind by an earlier session is used instead and
    # the run silently measures a different setup than the one it reports.
    & $Angelo init --force *> (Join-Path $LogsDir "$name.init.log")
    $confPath = Join-Path $dir 'angelo.conf'
    if (-not (Test-Path $confPath)) {
        $row.status = 'init_failed'
        $row.finished_at = (Get-Date).ToString('o')
        $row.wall_seconds = [math]::Round(((Get-Date) - $runStart).TotalSeconds, 1)
        $env:PATH = $oldPath
        Pop-Location
        ($row | ConvertTo-Json -Compress) | Add-Content -Path $ResultsFile
        $completedCount++
        Write-Host "  -> init_failed"
        continue
    }

    $reportPath = (Join-Path $repoReportsDir 'stryker-report.json') -replace '\\', '/'
    $htmlReportPath = $htmlReport -replace '\\', '/'
    $sonarPath = (Join-Path $repoReportsDir 'sonar-issues.json') -replace '\\', '/'
    # angelo init leaves test_command as the literal "python -m pytest",
    # resolved by PATH search at exec time. On Windows, CreateProcess checks
    # the *launching* executable's own directory before PATH -- and angelo.exe
    # lives in experiment/.venv/Scripts, which has its own bare python.exe
    # (no pytest, no project deps). That shadows every repo's venv silently:
    # pytest exits ~instantly with "no module named pytest" (exit 1, which
    # angelo's baseline reader treats as a legitimate red suite), so no junit
    # report is ever written and the run fails confusingly downstream instead
    # of loudly here. Pointing test_command at this repo's own venv python by
    # absolute path sidesteps the search order entirely.
    $pyForward = $py -replace '\\', '/'
    Set-ConfigValue -Path $confPath -Key 'test_command' -Value "`"$pyForward -m pytest`""
    Set-ConfigValue -Path $confPath -Key 'sample' -Value $SampleSize
    Set-ConfigValue -Path $confPath -Key 'report' -Value "`"$reportPath`""
    Set-ConfigValue -Path $confPath -Key 'html_report' -Value "`"$htmlReportPath`""
    Set-ConfigValue -Path $confPath -Key 'sonar_report' -Value "`"$sonarPath`""

    Write-Progress-State -Index ($i + 1) -Total $totalRepos -Name $name -Status 'angelo-exec' -StartedAt $runStart.ToString('o') -Completed $completedCount
    $proc = Start-Process -FilePath $Angelo -ArgumentList 'exec' -NoNewWindow -PassThru `
        -RedirectStandardOutput $logPath -RedirectStandardError $errLogPath -WorkingDirectory $dir
    # Touching Handle makes .NET keep the process handle open, without which
    # ExitCode comes back $null after the process ends and every finished run
    # reads as a failure.
    $null = $proc.Handle
    $finished = $proc.WaitForExit($PerRepoTimeoutMinutes * 60 * 1000)
    if (-not $finished) {
        # Kill($true) is .NET Core only. Under Windows PowerShell 5.1 it throws
        # into the catch and the timeout silently does nothing -- which once left
        # an angelo running an hour past its deadline with ~190 orphaned workers.
        # taskkill /T takes the whole tree, which is what a hung suite leaves.
        & taskkill /T /F /PID $proc.Id *>> $errLogPath
        $row.status = 'timed_out_by_harness'
        $proc.WaitForExit(30 * 1000) | Out-Null
    }
    $row.exit_code = if ($proc.HasExited) { $proc.ExitCode } else { $null }

    $env:PATH = $oldPath
    Pop-Location

    $verdicts = Get-VerdictCounts -OutLog $logPath
    # "ok" has to mean angelo scored something. Reading it off the harness
    # timeout alone once filed seven failed runs as successes, and every one of
    # them had exited 1 with no score line at all.
    if ($finished) {
        if ($proc.ExitCode -ne 0) {
            $row.status = 'angelo_failed'
        } elseif ($null -eq $verdicts.score) {
            $row.status = 'no_score'
        } else {
            $row.status = 'ok'
        }
    }
    $row.killed = $verdicts.counts.killed
    $row.survived = $verdicts.counts.survived
    $row.timeout = $verdicts.counts.timeout
    $row.error = $verdicts.counts.error
    $row.untestable = $verdicts.counts.untestable
    $row.score_percent = $verdicts.score
    $row.scored_total = $verdicts.scored
    $row.html_report_written = Test-Path $htmlReport
    $row.finished_at = (Get-Date).ToString('o')
    $row.wall_seconds = [math]::Round(((Get-Date) - $runStart).TotalSeconds, 1)

    ($row | ConvertTo-Json -Compress) | Add-Content -Path $ResultsFile
    $results.Add([pscustomobject]$row)
    $completedCount++
    Write-Host "  -> $($row.status) in $($row.wall_seconds)s"
}

Write-Progress-State -Index $totalRepos -Total $totalRepos -Name 'done' -Status 'finished' -StartedAt (Get-Date).ToString('o') -Completed $completedCount
Write-Host "Experiment finished: $completedCount/$totalRepos repos processed."
