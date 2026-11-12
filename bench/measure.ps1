<#
.SYNOPSIS
Measures an app's RAM split into "agents" (the CLI process trees) and
"overhead" (everything else the app runs). PLAN.md section 6.

.DESCRIPTION
Every -Interval seconds for -Seconds, walks the process tree under the app's
root process (CIM Win32_Process parent links) and sums Private Bytes
(PrivatePageCount) and working set. A process whose name matches -Agents
starts an agent subtree: it and everything under it count as agents. The rest
is the app's overhead, which is the number to compare between hubs, since the
CLIs cost the same in any of them.

Writes every sample to bench/results/<label>-<scenario>-<time>.csv, prints the
median and p95, and appends that summary to bench/results/summary.csv.

.EXAMPLE
# SpectaCLI with 4 agents idle, 5 minutes
.\bench\measure.ps1 -Root spectacli -Label spectacli -Scenario 4-idle

.EXAMPLE
# The floor: the same CLIs in Windows Terminal
.\bench\measure.ps1 -Root WindowsTerminal -Label windows-terminal -Scenario 4-idle

.EXAMPLE
# A quick check by PID
.\bench\measure.ps1 -Root 12345 -Label test -Seconds 10
#>
param(
    # Process name (without .exe) or PID of the app's root process.
    [Parameter(Mandatory = $true)] [string] $Root,
    # Candidate name in the results, e.g. spectacli, yzpzcode, windows-terminal.
    [Parameter(Mandatory = $true)] [string] $Label,
    # Scenario name, e.g. empty, 2-idle, 4-streaming, 4-hidden.
    [string] $Scenario = "adhoc",
    [int] $Seconds = 300,
    [double] $Interval = 1,
    # Process names that start an agent subtree.
    [string] $Agents = '^(claude|codex|gemini|opencode|aider|node|bun)\.exe$',
    # Default: bench/results next to this script.
    [string] $OutDir = ""
)

$ErrorActionPreference = "Stop"
# Not a param default: Windows PowerShell 5.1 leaves $PSScriptRoot empty
# there when the script is started with `powershell -File`.
if (-not $OutDir) { $OutDir = Join-Path (Split-Path -Parent $MyInvocation.MyCommand.Path) "results" }

function Get-Roots([hashtable] $byPid, [string] $root) {
    if ($root -match '^\d+$') {
        if (-not $byPid.ContainsKey([uint32]$root)) { return @() }
        return @($byPid[[uint32]$root])
    }
    $name = "$root.exe"
    $matched = @($byPid.Values | Where-Object { $_.Name -ieq $name })
    # Only the topmost ones: a multi-process app's helpers share its name.
    return @($matched | Where-Object {
        $parent = $byPid[$_.ParentProcessId]
        -not ($parent -and $parent.Name -ieq $name)
    })
}

function Measure-Once {
    $procs = Get-CimInstance Win32_Process -Property ProcessId, ParentProcessId, Name, PrivatePageCount, WorkingSetSize, CreationDate
    $byPid = @{}
    $children = @{}
    foreach ($p in $procs) {
        $byPid[$p.ProcessId] = $p
        if ($p.ProcessId -ne $p.ParentProcessId) {
            if (-not $children.ContainsKey($p.ParentProcessId)) { $children[$p.ParentProcessId] = New-Object System.Collections.ArrayList }
            [void]$children[$p.ParentProcessId].Add($p)
        }
    }
    $roots = Get-Roots $byPid $Root
    if ($roots.Count -eq 0) { return $null }

    $s = [ordered]@{
        overheadPrivate = [uint64]0; overheadWs = [uint64]0; overheadProcs = 0
        agentPrivate = [uint64]0; agentWs = [uint64]0; agentProcs = 0; agentTrees = 0
    }
    # Breadth-first; each entry carries whether it is inside an agent subtree.
    $queue = New-Object System.Collections.Queue
    foreach ($r in $roots) { $queue.Enqueue(@($r, $false)) }
    $seen = @{}
    while ($queue.Count -gt 0) {
        $p, $inAgent = $queue.Dequeue()
        if ($seen.ContainsKey($p.ProcessId)) { continue }
        $seen[$p.ProcessId] = $true
        if (-not $inAgent -and $p.Name -match $Agents) {
            $inAgent = $true
            $s.agentTrees++
        }
        if ($inAgent) {
            $s.agentPrivate += [uint64]$p.PrivatePageCount
            $s.agentWs += [uint64]$p.WorkingSetSize
            $s.agentProcs++
        } else {
            $s.overheadPrivate += [uint64]$p.PrivatePageCount
            $s.overheadWs += [uint64]$p.WorkingSetSize
            $s.overheadProcs++
        }
        if ($children.ContainsKey($p.ProcessId)) {
            foreach ($c in $children[$p.ProcessId]) {
                # A child starts after its parent; an older one only reuses a dead parent's PID.
                if ($c.CreationDate -ge $p.CreationDate) { $queue.Enqueue(@($c, $inAgent)) }
            }
        }
    }
    return $s
}

function Get-Percentile([double[]] $values, [double] $pct) {
    $sorted = $values | Sort-Object
    $i = [math]::Ceiling($pct / 100 * $sorted.Count) - 1
    return $sorted[[math]::Max(0, [math]::Min($i, $sorted.Count - 1))]
}

New-Item -ItemType Directory -Force $OutDir | Out-Null
$stamp = Get-Date -Format "yyyyMMdd-HHmmss"
$csv = Join-Path $OutDir "$Label-$Scenario-$stamp.csv"
$mb = 1MB
$rows = New-Object System.Collections.ArrayList
$start = Get-Date
$deadline = $start.AddSeconds($Seconds)

Write-Host "Measuring '$Root' as $Label/$Scenario for $Seconds s -> $csv"
while ((Get-Date) -lt $deadline) {
    $tickStart = Get-Date
    $s = Measure-Once
    if ($null -eq $s) {
        # Not started, or closed mid-run: stop instead of warning every second.
        if ($rows.Count -eq 0) {
            Write-Error "No process named '$Root' is running. Start the app first, or check its name in Task Manager > Details (without .exe)."
        }
        Write-Warning "'$Root' is no longer running; stopping early."
        break
    } else {
        $row = [pscustomobject]@{
            seconds          = [math]::Round(((Get-Date) - $start).TotalSeconds, 1)
            overhead_private_mb = [math]::Round($s.overheadPrivate / $mb, 1)
            overhead_ws_mb      = [math]::Round($s.overheadWs / $mb, 1)
            overhead_procs      = $s.overheadProcs
            agent_private_mb    = [math]::Round($s.agentPrivate / $mb, 1)
            agent_ws_mb         = [math]::Round($s.agentWs / $mb, 1)
            agent_procs         = $s.agentProcs
            agent_trees         = $s.agentTrees
            total_private_mb    = [math]::Round(($s.overheadPrivate + $s.agentPrivate) / $mb, 1)
        }
        [void]$rows.Add($row)
        $row | Export-Csv -Path $csv -Append -NoTypeInformation -Encoding UTF8
        Write-Host ("{0,6}s  overhead {1,7} MB ({2} procs)  agents {3,7} MB ({4} trees)" -f $row.seconds, $row.overhead_private_mb, $row.overhead_procs, $row.agent_private_mb, $row.agent_trees)
    }
    $left = $Interval - ((Get-Date) - $tickStart).TotalSeconds
    if ($left -gt 0) { Start-Sleep -Milliseconds ([int]($left * 1000)) }
}

if ($rows.Count -eq 0) {
    Write-Error "No samples: '$Root' was never running."
}

$summary = [pscustomobject]@{
    when                    = $start.ToString("s")
    label                   = $Label
    scenario                = $Scenario
    samples                 = $rows.Count
    agent_trees             = Get-Percentile ($rows | ForEach-Object agent_trees) 50
    overhead_private_median = Get-Percentile ($rows | ForEach-Object overhead_private_mb) 50
    overhead_private_p95    = Get-Percentile ($rows | ForEach-Object overhead_private_mb) 95
    overhead_ws_median      = Get-Percentile ($rows | ForEach-Object overhead_ws_mb) 50
    agent_private_median    = Get-Percentile ($rows | ForEach-Object agent_private_mb) 50
    total_private_median    = Get-Percentile ($rows | ForEach-Object total_private_mb) 50
    total_private_p95       = Get-Percentile ($rows | ForEach-Object total_private_mb) 95
    csv                     = Split-Path $csv -Leaf
}
$summary | Export-Csv -Path (Join-Path $OutDir "summary.csv") -Append -NoTypeInformation -Encoding UTF8
Write-Host ""
Write-Host ("{0}/{1}: overhead {2} MB median, {3} MB p95 (working set {4} MB); agents {5} MB in {6} trees; {7} samples" -f `
    $Label, $Scenario, $summary.overhead_private_median, $summary.overhead_private_p95, $summary.overhead_ws_median, `
    $summary.agent_private_median, $summary.agent_trees, $summary.samples)
