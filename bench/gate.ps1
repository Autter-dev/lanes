#Requires -Version 5.1
<#
.SYNOPSIS
Lanes' P1 gate: five Claude Code agents work in five lanes on one repo, made through
`claude -w`, with no manual steps.

.DESCRIPTION
Checks that `lane setup` has put Claude Code in lanes, that the repo is clean and that nothing
called gate-* exists. Then starts five `claude -w gate-N -p "..."` sessions at once, each
adding a comment to a different file, and checks the lanes, their ports, that each change
stayed in its own lane, that Git has no worktree outside the lanes folder, that
`git worktree add` is turned down, and that `lane savings` counts the lanes. Then it removes
the lanes the way Claude Code does (its WorktreeRemove hook) and checks they're gone and still
counted. bench/GATE.md explains each check.

Exits 0 when every check passes, 1 when one fails and 2 when the arguments are wrong. It never
prompts. Session output goes to a folder in TEMP, which the last line names.

.PARAMETER Repo
The repo to run the gate on: a clean Git checkout. The real gate uses withastro/astro.

.PARAMETER Lane
The `lane` program to use. Default: the one on PATH. Claude Code's hooks get this one too.

.PARAMETER Claude
The `claude` program to use. Default: the one on PATH.

.PARAMETER Timeout
Seconds to wait for the five sessions (or, with -NoAgents, the five `lane new`s). Default 1200.

.PARAMETER NoAgents
Don't run Claude Code: make the lanes with `lane new gate-N`, edit a file in each with this
script, then run the same checks and cleanup. For testing the gate itself.

.PARAMETER Json
Print one JSON object with every check instead of a line per check.

.EXAMPLE
powershell -NoProfile -ExecutionPolicy Bypass -File bench\gate.ps1 -Repo D:\src\astro
#>
param(
    [string]$Repo,
    [string]$Lane,
    [string]$Claude,
    [int]$Timeout = 1200,
    [switch]$NoAgents,
    [switch]$Json
)

$ErrorActionPreference = 'Stop'
$Names = @(1..5 | ForEach-Object { "gate-$_" })
$Started = Get-Date
$Checks = New-Object System.Collections.ArrayList
$Mode = if ($NoAgents) { 'no-agents' } else { 'agents' }
$RepoRoot = $null
$LaneExe = $null
$LaneVersion = $null
$ClaudeExe = $null
$GitExe = $null
$LogDir = $null
$Sessions = @()
$MadeLanes = $false
$Cleaned = $false

# ---------- output ----------

function Say([string]$Text) { [Console]::Error.WriteLine("gate: $Text") }

function Add-Check([string]$Id, [bool]$Passed, [string]$Detail) {
    [void]$script:Checks.Add([ordered]@{ id = $Id; passed = $Passed; detail = $Detail })
    if (-not $Json) {
        $mark = if ($Passed) { 'PASS' } else { 'FAIL' }
        [Console]::Out.WriteLine(('{0}  {1,-13} {2}' -f $mark, $Id, $Detail))
    }
}

function Write-Result([string]$Note) {
    $failed = @($script:Checks | Where-Object { -not $_.passed })
    $passed = ($script:Checks.Count -gt 0) -and ($failed.Count -eq 0) -and -not $Note
    $seconds = [int]((Get-Date) - $Started).TotalSeconds
    if ($Json) {
        $result = [ordered]@{
            passed       = $passed
            mode         = $Mode
            repo         = $RepoRoot
            lane         = $LaneExe
            lane_version = $LaneVersion
            claude       = $ClaudeExe
            logs         = $LogDir
            seconds      = $seconds
            checks       = @($script:Checks)
        }
        if ($Note) { $result.note = $Note }
        [Console]::Out.WriteLine(($result | ConvertTo-Json -Depth 6))
    } else {
        if ($Note) { [Console]::Out.WriteLine($Note) }
        $logs = if ($LogDir) { " Logs: $LogDir" } else { '' }
        if ($passed) {
            [Console]::Out.WriteLine("Gate passed: $($script:Checks.Count) of $($script:Checks.Count) checks, in $seconds s.$logs")
        } elseif (-not $Note) {
            $ids = ($failed | ForEach-Object { $_.id }) -join ', '
            [Console]::Out.WriteLine("Gate failed: $($failed.Count) of $($script:Checks.Count) checks ($ids), in $seconds s.$logs")
        }
    }
    if ($passed) { return 0 }
    return 1
}

# ---------- running programs ----------
# Programs run through System.Diagnostics.Process, so Windows PowerShell 5.1 neither mangles
# quotes in arguments nor turns a line on stderr into an error.

function Quote-Arg([string]$Arg) {
    if ($Arg.Length -gt 0 -and $Arg -notmatch '[\s"]') { return $Arg }
    $sb = New-Object System.Text.StringBuilder
    [void]$sb.Append('"')
    $slashes = 0
    foreach ($c in $Arg.ToCharArray()) {
        if ($c -eq [char]'\') { $slashes++; continue }
        if ($c -eq [char]'"') { [void]$sb.Append('\' * (2 * $slashes + 1)).Append('"') }
        else { [void]$sb.Append('\' * $slashes).Append($c) }
        $slashes = 0
    }
    [void]$sb.Append('\' * (2 * $slashes)).Append('"')
    $sb.ToString()
}

function Start-Tool([string]$File, [string[]]$Arguments, [string]$Dir, [string]$InputText, [hashtable]$Environment) {
    $line = ''
    if ($Arguments) { $line = ($Arguments | ForEach-Object { Quote-Arg $_ }) -join ' ' }
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    if ($File -match '\.(cmd|bat)$') {
        # npm installs `claude` as a .cmd file, which only cmd.exe runs.
        $psi.FileName = $env:ComSpec
        $psi.Arguments = '/d /s /c "' + (Quote-Arg $File) + ' ' + $line + '"'
    } else {
        $psi.FileName = $File
        $psi.Arguments = $line
    }
    $psi.WorkingDirectory = $Dir
    $psi.UseShellExecute = $false
    $psi.CreateNoWindow = $true
    $psi.RedirectStandardInput = $true
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.StandardOutputEncoding = New-Object System.Text.UTF8Encoding $false
    $psi.StandardErrorEncoding = New-Object System.Text.UTF8Encoding $false
    if ($Environment) { foreach ($k in $Environment.Keys) { $psi.EnvironmentVariables[$k] = $Environment[$k] } }
    $p = [System.Diagnostics.Process]::Start($psi)
    $out = $p.StandardOutput.ReadToEndAsync()
    $err = $p.StandardError.ReadToEndAsync()
    if ($InputText) {
        $bytes = (New-Object System.Text.UTF8Encoding $false).GetBytes($InputText)
        $p.StandardInput.BaseStream.Write($bytes, 0, $bytes.Length)
        $p.StandardInput.BaseStream.Flush()
    }
    # Nothing waits on stdin: a session reads end-of-file, never a prompt.
    $p.StandardInput.Close()
    [pscustomobject]@{ Process = $p; OutTask = $out; ErrTask = $err; Code = $null; Out = ''; Err = ''; TimedOut = $false }
}

function Stop-Tree([int]$ProcessId) {
    try { $null = Invoke-Tool "$env:SystemRoot\System32\taskkill.exe" @('/T', '/F', '/PID', "$ProcessId") $env:SystemRoot } catch { }
}

function Wait-Tool($Tool, [datetime]$Deadline) {
    $ms = [Math]::Max(0, [Math]::Min([double][int]::MaxValue, ($Deadline - (Get-Date)).TotalMilliseconds))
    if (-not $Tool.Process.WaitForExit([int]$ms)) {
        $Tool.TimedOut = $true
        Stop-Tree $Tool.Process.Id
        [void]$Tool.Process.WaitForExit(10000)
    }
    [void]$Tool.OutTask.Wait(10000)
    [void]$Tool.ErrTask.Wait(10000)
    if ($Tool.OutTask.IsCompleted) { $Tool.Out = $Tool.OutTask.Result }
    if ($Tool.ErrTask.IsCompleted) { $Tool.Err = $Tool.ErrTask.Result }
    if (-not $Tool.TimedOut) { $Tool.Code = $Tool.Process.ExitCode }
    $Tool
}

function Invoke-Tool([string]$File, [string[]]$Arguments, [string]$Dir, [string]$InputText) {
    if (-not $Dir) { $Dir = $script:RepoRoot }
    Wait-Tool (Start-Tool $File $Arguments $Dir $InputText) ((Get-Date).AddMinutes(30))
}

function ConvertFrom-JsonText([string]$Text) {
    if (-not $Text -or -not $Text.Trim()) { return $null }
    try { return ($Text | ConvertFrom-Json) } catch { return $null }
}

# `lane <args> --json`, with its output parsed into .Json.
function Invoke-Lane([string[]]$Arguments, [string]$Dir) {
    $r = Invoke-Tool $script:LaneExe ($Arguments + '--json') $Dir
    $r | Add-Member -NotePropertyName Json -NotePropertyValue (ConvertFrom-JsonText $r.Out) -PassThru
}

# What `lane` said went wrong: its JSON error, else stderr.
function Get-LaneError($Result) {
    if ($Result.TimedOut) { return 'timed out' }
    if ($Result.Json -and $Result.Json.error) {
        $e = $Result.Json.error
        if ($e.hint) { return "$($e.message) ($($e.hint))" }
        return [string]$e.message
    }
    $text = ($Result.Err + ' ' + $Result.Out).Trim()
    if ($text.Length -gt 300) { $text = $text.Substring(0, 300) + '...' }
    if ($text) { return $text }
    "exit $($Result.Code)"
}

function Invoke-Git([string]$Dir, [string[]]$Arguments) {
    Invoke-Tool $script:GitExe (@('-C', $Dir) + $Arguments) $Dir
}

function Find-Program([string]$Given, [string]$Name) {
    if ($Given -and (Test-Path -LiteralPath $Given -PathType Leaf)) {
        return (Resolve-Path -LiteralPath $Given).ProviderPath
    }
    $want = if ($Given) { $Given } else { $Name }
    $found = @(Get-Command $want -CommandType Application -ErrorAction SilentlyContinue |
            Where-Object { $_.Path -match '\.(exe|cmd|bat)$' })
    if ($found.Count -gt 0) { return $found[0].Path }
    $null
}

# ---------- paths and lanes ----------

function Norm([string]$Path) {
    if (-not $Path) { return '' }
    (($Path -replace '/', '\').TrimEnd('\')).ToLowerInvariant()
}

function Test-Under([string]$Path, [string]$Dir) {
    (Norm $Path).StartsWith((Norm $Dir) + '\')
}

function Get-Worktrees {
    $r = Invoke-Git $script:RepoRoot @('worktree', 'list', '--porcelain')
    @($r.Out -split "`n" | Where-Object { $_ -like 'worktree *' } |
            ForEach-Object { ($_.Substring(9).TrimEnd("`r")) -replace '/', '\' })
}

# This repo's lanes, by name.
function Get-Lanes {
    $r = Invoke-Lane @('list')
    $byName = @{}
    if ($r.Code -eq 0 -and $r.Json) {
        foreach ($l in @($r.Json.lanes)) { if ($l) { $byName[[string]$l.name] = $l } }
    }
    [pscustomobject]@{ Ok = ($r.Code -eq 0 -and $r.Json); Lanes = $byName; Result = $r }
}

function Get-Savings {
    $r = Invoke-Lane @('savings')
    if ($r.Code -ne 0 -or -not $r.Json -or $null -eq $r.Json.total) { return $null }
    $r.Json
}

function Format-Ports($l) { '{0}-{1}' -f $l.ports.start, ([int]$l.ports.start + [int]$l.ports.count - 1) }

function Test-Overlap($a, $b) {
    $aEnd = [int]$a.ports.start + [int]$a.ports.count - 1
    $bEnd = [int]$b.ports.start + [int]$b.ports.count - 1
    ([int]$a.ports.start -le $bEnd) -and ([int]$b.ports.start -le $aEnd)
}

# Claude Code's user settings: where `lane setup` puts its hooks.
function Get-ClaudeSettingsPath {
    if ($env:LANE_AGENTS_HOME) { return (Join-Path $env:LANE_AGENTS_HOME '.claude\settings.json') }
    if ($env:CLAUDE_CONFIG_DIR) { return (Join-Path $env:CLAUDE_CONFIG_DIR 'settings.json') }
    Join-Path $HOME '.claude\settings.json'
}

# Five tracked files to edit, one per lane: README.md files first, then other small text files,
# shallowest first. Paths with only plain characters, so they pass through any shell untouched.
function Get-GateFiles {
    $r = Invoke-Git $script:RepoRoot @('ls-files', '-s', '-z')
    $exts = @('md', 'txt', 'ts', 'js', 'mjs', 'py', 'rs', 'go', 'toml', 'yml', 'yaml', 'sh')
    $candidates = foreach ($entry in ($r.Out -split [char]0)) {
        if ($entry -notmatch '^100644 [0-9a-f]+ 0\t(.+)$') { continue }
        $path = $Matches[1]
        if ($path -notmatch '^[A-Za-z0-9_./-]+$' -or $path -match '(^|/)\.') { continue }
        $ext = [IO.Path]::GetExtension($path).TrimStart('.').ToLowerInvariant()
        if ($exts -notcontains $ext) { continue }
        $full = Join-Path $script:RepoRoot ($path -replace '/', '\')
        if (-not (Test-Path -LiteralPath $full -PathType Leaf)) { continue }
        if ((Get-Item -LiteralPath $full).Length -gt 200KB) { continue }
        $leaf = ($path -split '/')[-1]
        $rank = if ($leaf -ieq 'README.md') { 0 } elseif ($ext -eq 'md') { 1 } else { 2 }
        [pscustomobject]@{ Path = $path; Rank = $rank; Depth = ($path -split '/').Count }
    }
    @($candidates | Sort-Object Rank, Depth, Path | Select-Object -First 5 | ForEach-Object { $_.Path })
}

# The line the scripted edit adds (-NoAgents): a comment in the file's own syntax.
function Get-CommentLine([string]$File, [string]$Name) {
    $text = "edited in lane $Name"
    switch ([IO.Path]::GetExtension($File).ToLowerInvariant()) {
        '.md' { return "<!-- $text -->" }
        { @('.ts', '.js', '.mjs', '.rs', '.go') -contains $_ } { return "// $text" }
        '.txt' { return $text }
        default { return "# $text" }
    }
}

function Add-Line([string]$Path, [string]$Line) {
    $text = [IO.File]::ReadAllText($Path)
    $nl = if ($text.Contains("`r`n")) { "`r`n" } else { "`n" }
    $add = if ($text.Length -eq 0 -or $text.EndsWith("`n")) { "$Line$nl" } else { "$nl$Line$nl" }
    [IO.File]::AppendAllText($Path, $add)
}

# Remove the gate's lanes the way Claude Code does when it's done with a worktree: its
# WorktreeRemove hook. `lane rm` takes any the hook leaves.
function Remove-GateLanes {
    $done = [pscustomobject]@{ Hook = @(); Rm = @(); Failed = @() }
    $list = Get-Lanes
    foreach ($name in $Names) {
        $l = $list.Lanes[$name]
        if (-not $l) { continue }
        $hookInput = [ordered]@{
            session_id      = 'lane-gate'
            cwd             = $script:RepoRoot
            hook_event_name = 'WorktreeRemove'
            worktree_path   = [string]$l.path
        } | ConvertTo-Json -Compress
        $r = Invoke-Tool $script:LaneExe @('hook', 'claude', 'worktree-remove', '--json') $script:RepoRoot $hookInput
        if ($r.Code -eq 0 -and -not (Test-Path -LiteralPath $l.path)) { $done.Hook += $name; continue }
        $rm = Invoke-Lane @('rm', $name)
        if ($rm.Code -eq 0) { $done.Rm += $name } else { $done.Failed += "${name}: $(Get-LaneError $rm)" }
    }
    $script:Cleaned = $true
    $done
}

# ---------- the gate ----------

function Invoke-Gate {
    # Arguments, and the programs the gate runs.
    $usage = $null
    if (-not $Repo) { $usage = 'which repo? Run it as: gate.ps1 -Repo <path to a clean checkout> [-NoAgents] [-Json]' }
    elseif ($Timeout -lt 1) { $usage = '-Timeout is in seconds and must be 1 or more, like -Timeout 1200.' }
    if ($usage) {
        Say $usage
        if ($Json) { [Console]::Out.WriteLine(([ordered]@{ passed = $false; error = $usage } | ConvertTo-Json)) }
        return 2
    }
    $script:GitExe = Find-Program '' 'git'
    $script:LaneExe = Find-Program $Lane 'lane'
    if (-not $NoAgents) { $script:ClaudeExe = Find-Program $Claude 'claude' }
    $problems = @()
    $versions = @()
    if (-not $GitExe) { $problems += 'git isn''t on PATH: install Git for Windows' }
    if (-not $LaneExe) {
        $problems += 'no lane program found: install Lanes (lanes.autter.dev/download) or pass -Lane <path to lane.exe>'
    }
    if (-not (Test-Path -LiteralPath $Repo -PathType Container)) {
        $problems += "$Repo isn't a folder: pass -Repo <path to a clean checkout>"
    }
    if ($problems.Count -eq 0) {
        $script:RepoRoot = (Resolve-Path -LiteralPath $Repo).ProviderPath
        $v = Invoke-Tool $LaneExe @('--version') $RepoRoot
        $script:LaneVersion = $v.Out.Trim()
        if ($v.Code -ne 0) { $problems += "$LaneExe --version failed: $(Get-LaneError $v)" }
        else { $versions += "$LaneVersion ($LaneExe)" }
        $s = Invoke-Lane @('savings', '--help') $RepoRoot
        if ($s.Code -ne 0) {
            $problems += "this lane has no ``lane savings``: install a Lanes that has it, or pass -Lane <path>"
        }
        $g = Invoke-Tool $GitExe @('--version') $RepoRoot
        $versions += $g.Out.Trim()
        if (-not $NoAgents) {
            if (-not $ClaudeExe) {
                $problems += 'no claude program found: install Claude Code or pass -Claude <path>'
            } else {
                $c = Invoke-Tool $ClaudeExe @('--version') $RepoRoot
                if ($c.Code -ne 0) { $problems += "$ClaudeExe --version failed: $($c.Err.Trim())" }
                else { $versions += "claude $($c.Out.Trim()) ($ClaudeExe)" }
            }
        }
    }
    if ($problems.Count -gt 0) {
        Add-Check 'tools' $false ($problems -join '; ')
        return (Write-Result 'Gate not run: fix what failed above, then run it again.')
    }
    Add-Check 'tools' $true ($versions -join ', ')

    # The repo: its main checkout, clean.
    $top = Invoke-Git $RepoRoot @('rev-parse', '--show-toplevel')
    if ($top.Code -ne 0) {
        Add-Check 'clean' $false "$RepoRoot isn't a Git repo: pass -Repo <path to a clean checkout>"
        return (Write-Result 'Gate not run: fix what failed above, then run it again.')
    }
    $script:RepoRoot = $top.Out.Trim() -replace '/', '\'
    $script:RepoRoot = @(Get-Worktrees)[0]

    # 1. Claude Code is in lanes on this machine: `lane doctor` says so, and its settings have
    # the four hooks `lane setup` adds.
    $doctor = Invoke-Lane @('doctor')
    $byId = @{}
    if ($doctor.Json) { foreach ($c in @($doctor.Json.checks)) { if ($c) { $byId[[string]$c.id] = $c } } }
    $hooks = $byId['agent-hooks']
    $onPath = $byId['lane-path']
    $problems = @()
    if (-not $hooks) {
        $problems += "lane doctor --json has no agent-hooks check ($(Get-LaneError $doctor))"
    } elseif (@('ok', 'warn') -notcontains [string]$hooks.status) {
        $problems += "lane doctor: agents in lanes: $($hooks.summary)"
    } else {
        $notSetUp = ''
        if ([string]$hooks.summary -match '^(.*) not set up') { $notSetUp = ($Matches[1] -split ';')[-1] }
        if ($notSetUp -like '*Claude Code*') { $problems += "lane doctor: $($hooks.summary)" }
    }
    if ($onPath -and [string]$onPath.status -ne 'ok') { $problems += "lane doctor: $($onPath.summary)" }
    $settingsPath = Get-ClaudeSettingsPath
    $settings = $null
    if (Test-Path -LiteralPath $settingsPath -PathType Leaf) {
        $settings = ConvertFrom-JsonText ([IO.File]::ReadAllText($settingsPath))
    }
    $events = [ordered]@{
        WorktreeCreate = 'worktree-create'; WorktreeRemove = 'worktree-remove'
        SessionStart = 'session-start'; PreToolUse = 'pre-tool-use'
    }
    $missing = @()
    foreach ($event in $events.Keys) {
        $commands = @()
        if ($settings -and $settings.hooks) {
            foreach ($group in @($settings.hooks.$event)) {
                if ($group) { foreach ($h in @($group.hooks)) { if ($h) { $commands += [string]$h.command } } }
            }
        }
        if (-not ($commands | Where-Object { $_ -like "*lane hook claude $($events[$event])*" })) { $missing += $event }
    }
    if ($missing.Count -gt 0) {
        $problems += "Claude Code's settings ($settingsPath) have no Lanes hook for $($missing -join ', ')"
    }
    if ($problems.Count -gt 0) {
        Add-Check 'setup' $false (($problems -join '; ') + '. Run `lane setup --yes`, then run the gate again.')
    } else {
        Add-Check 'setup' $true "lane doctor: $($hooks.summary); Claude Code's hooks are in $settingsPath"
    }

    # 2. The repo is clean.
    $status = Invoke-Git $RepoRoot @('status', '--porcelain')
    $head = (Invoke-Git $RepoRoot @('rev-parse', 'HEAD')).Out.Trim()
    $branch = (Invoke-Git $RepoRoot @('rev-parse', '--abbrev-ref', 'HEAD')).Out.Trim()
    $dirty = @($status.Out -split "`n" | Where-Object { $_.Trim() })
    $short = if ($head.Length -ge 7) { $head.Substring(0, 7) } else { $head }
    if ($status.Code -ne 0) {
        Add-Check 'clean' $false "git status failed in ${RepoRoot}: $($status.Err.Trim())"
    } elseif ($dirty.Count -gt 0) {
        $what = if ($dirty.Count -eq 1) { '1 changed or untracked file' } else { "$($dirty.Count) changed or untracked files" }
        Add-Check 'clean' $false "$what in ${RepoRoot}: commit or stash them (or use a fresh clone), then run the gate again"
    } else {
        Add-Check 'clean' $true "$RepoRoot on $branch at $short, nothing to commit"
    }

    # 3. Nothing else called gate-*: no lane, no worktree, no folder where the lanes go.
    $before = Get-Lanes
    $worktreesBefore = @(Get-Worktrees)
    $found = @()
    if (-not $before.Ok) { $found += "lane list failed: $(Get-LaneError $before.Result)" }
    foreach ($name in $before.Lanes.Keys) { if ($name -like 'gate-*') { $found += "the lane $name" } }
    foreach ($w in $worktreesBefore) { if ((Split-Path $w -Leaf) -like 'gate-*') { $found += "the worktree $w" } }
    $dirs = @((Split-Path $RepoRoot -Parent) + '\' + (Split-Path $RepoRoot -Leaf) + '.lanes')
    $dirs += @($worktreesBefore | Select-Object -Skip 1 | ForEach-Object { Split-Path $_ -Parent })
    foreach ($d in ($dirs | Sort-Object -Unique)) {
        if (-not (Test-Path -LiteralPath $d -PathType Container)) { continue }
        foreach ($f in @(Get-ChildItem -LiteralPath $d -Directory -Filter 'gate-*')) {
            if (-not ($found -like "*$($f.FullName)")) { $found += "the folder $($f.FullName)" }
        }
    }
    if ($found.Count -gt 0) {
        Add-Check 'no-gate' $false ("found $($found -join ', '): remove them (``lane rm <name>``, or delete the folder), then run the gate again")
    } else {
        Add-Check 'no-gate' $true 'no lane, worktree or folder called gate-*'
    }

    $files = @(Get-GateFiles)
    if ($files.Count -lt 5) {
        Add-Check 'files' $false "the repo needs five small tracked text files to edit (README.md files are best); found $($files.Count)"
    }
    if (@($Checks | Where-Object { -not $_.passed }).Count -gt 0) {
        return (Write-Result 'Gate not run: fix what failed above, then run it again.')
    }

    $savingsBefore = Get-Savings
    $script:LogDir = Join-Path ([IO.Path]::GetTempPath()) ('lane-gate-' + (Get-Date -Format 'yyyyMMdd-HHmmss'))
    New-Item -ItemType Directory -Force -Path $LogDir | Out-Null

    # Make the five lanes: five Claude Code sessions at once, or five `lane new`s and an edit.
    $script:MadeLanes = $true
    $deadline = (Get-Date).AddSeconds($Timeout)
    $t0 = Get-Date
    $problems = @()
    if (-not $NoAgents) {
        Say "starting five Claude Code sessions in $RepoRoot (claude -w gate-1 ... gate-5); this can take a few minutes"
        # Claude Code's hooks run `lane` by name: they get the same one as the gate.
        $envForClaude = @{ PATH = (Split-Path $LaneExe -Parent) + ';' + $env:PATH }
        $script:Sessions = @(for ($i = 0; $i -lt 5; $i++) {
                $prompt = ('Add one line at the end of {0}: a comment, in that file''s comment syntax, that says edited in lane {1}. Change nothing else, then reply with the line you added.' -f $files[$i], $Names[$i])
                $claudeArgs = @('-w', $Names[$i], '--permission-mode', 'acceptEdits', '--tools', 'Read,Edit',
                    '--output-format', 'json', '-p', $prompt)
                Start-Tool $ClaudeExe $claudeArgs $RepoRoot '' $envForClaude
            })
        for ($i = 0; $i -lt 5; $i++) {
            $s = Wait-Tool $Sessions[$i] $deadline
            [IO.File]::WriteAllText((Join-Path $LogDir "$($Names[$i]).json"), $s.Out)
            [IO.File]::WriteAllText((Join-Path $LogDir "$($Names[$i]).err.txt"), $s.Err)
            $answer = ConvertFrom-JsonText $s.Out
            if ($s.TimedOut) { $problems += "$($Names[$i]) was still running after $Timeout s" }
            elseif ($s.Code -ne 0) { $problems += "$($Names[$i]) exited with $($s.Code) (see $LogDir\$($Names[$i]).err.txt)" }
            elseif (-not $answer -or $answer.is_error) { $problems += "$($Names[$i]) didn't answer (see $LogDir\$($Names[$i]).json)" }
        }
        $script:Sessions = @()
        $took = [int]((Get-Date) - $t0).TotalSeconds
        if ($problems.Count -gt 0) { Add-Check 'made' $false ($problems -join '; ') }
        else { Add-Check 'made' $true "five claude -w sessions each edited a file and answered, in $took s" }
    } else {
        Say "making five lanes with lane new in $RepoRoot (no agents)"
        $made = @(foreach ($name in $Names) { Start-Tool $LaneExe @('new', $name, '--json') $RepoRoot })
        for ($i = 0; $i -lt 5; $i++) {
            $m = Wait-Tool $made[$i] $deadline
            [IO.File]::WriteAllText((Join-Path $LogDir "$($Names[$i]).json"), $m.Out)
            [IO.File]::WriteAllText((Join-Path $LogDir "$($Names[$i]).err.txt"), $m.Err)
            $m | Add-Member -NotePropertyName Json -NotePropertyValue (ConvertFrom-JsonText $m.Out)
            if ($m.Code -ne 0 -or -not $m.Json -or -not $m.Json.lane) {
                $problems += "lane new $($Names[$i]) failed: $(Get-LaneError $m)"
                continue
            }
            $path = Join-Path ([string]$m.Json.lane.path) ($files[$i] -replace '/', '\')
            Add-Line $path (Get-CommentLine $files[$i] $Names[$i])
        }
        $took = [int]((Get-Date) - $t0).TotalSeconds
        if ($problems.Count -gt 0) { Add-Check 'made' $false ($problems -join '; ') }
        else { Add-Check 'made' $true "five lane new ran at once and the script edited a file in each, in $took s (no agents)" }
    }

    # 4a. Five ready lanes, gate-1 to gate-5, in one folder, with port blocks no other lane has.
    $after = Get-Lanes
    $all = Invoke-Lane @('list', '--all')
    $problems = @()
    $ours = @()
    foreach ($name in $Names) {
        $l = $after.Lanes[$name]
        if (-not $l) { $problems += "$name isn't in lane list"; continue }
        $ours += $l
        if ([string]$l.status -ne 'ready') { $problems += "$name is $($l.status), not ready" }
        if (-not $NoAgents -and [string]$l.agent -ne 'claude') {
            $problems += "$name wasn't made by Claude Code's WorktreeCreate hook (agent: $($l.agent))"
        }
    }
    for ($i = 0; $i -lt $ours.Count; $i++) {
        for ($j = $i + 1; $j -lt $ours.Count; $j++) {
            if (Test-Overlap $ours[$i] $ours[$j]) {
                $problems += "$($ours[$i].name) ($(Format-Ports $ours[$i])) and $($ours[$j].name) ($(Format-Ports $ours[$j])) share ports"
            }
        }
    }
    $ourIds = @($ours | ForEach-Object { [string]$_.id })
    foreach ($other in @($all.Json.lanes)) {
        if (-not $other -or $ourIds -contains [string]$other.id) { continue }
        foreach ($l in $ours) {
            if (Test-Overlap $l $other) {
                $problems += "$($l.name) ($(Format-Ports $l)) shares ports with $($other.name) in $($other.repo)"
            }
        }
    }
    $parents = @($ours | ForEach-Object { Norm (Split-Path ([string]$_.path) -Parent) } | Sort-Object -Unique)
    $LanesDir = $null
    if ($ours.Count -gt 0) { $LanesDir = Split-Path ([string]$ours[0].path) -Parent }
    if ($parents.Count -gt 1) { $problems += "the lanes are in $($parents.Count) folders, not one" }
    if ($problems.Count -gt 0) {
        Add-Check 'lanes' $false ($problems -join '; ')
    } else {
        $by = if ($NoAgents) { '' } else { ', made by Claude Code' }
        Add-Check 'lanes' $true ("gate-1 to gate-5 ready in $LanesDir$by, ports " + (($ours | ForEach-Object { Format-Ports $_ }) -join ', '))
    }

    # 4b. Each lane's change is in its own folder only, and the main checkout is untouched.
    $problems = @()
    for ($i = 0; $i -lt 5; $i++) {
        $l = $after.Lanes[$Names[$i]]
        if (-not $l) { $problems += "$($Names[$i]) is missing"; continue }
        $st = Invoke-Git ([string]$l.path) @('status', '--porcelain', '--untracked-files=no')
        $changed = @($st.Out -split "`n" | Where-Object { $_.Trim() } | ForEach-Object { $_.TrimEnd("`r").Substring(3) })
        if ($changed.Count -ne 1 -or $changed[0] -ne $files[$i]) {
            $what = if ($changed.Count -eq 0) { 'nothing' } else { $changed -join ', ' }
            $problems += "$($Names[$i]) changed $what (expected only $($files[$i]))"
            continue
        }
        $text = [IO.File]::ReadAllText((Join-Path ([string]$l.path) ($files[$i] -replace '/', '\')))
        if ($text -notmatch "$($Names[$i])(?!\d)") { $problems += "$($Names[$i])'s $($files[$i]) doesn't name the lane" }
    }
    $status = Invoke-Git $RepoRoot @('status', '--porcelain')
    $dirty = @($status.Out -split "`n" | Where-Object { $_.Trim() } | ForEach-Object { $_.TrimEnd("`r") })
    $headNow = (Invoke-Git $RepoRoot @('rev-parse', 'HEAD')).Out.Trim()
    if ($dirty.Count -gt 0) { $problems += "the main checkout changed: $($dirty -join ', ')" }
    if ($headNow -ne $head) { $problems += "the main checkout moved from $short to $headNow" }
    if ($problems.Count -gt 0) {
        Add-Check 'isolation' $false ($problems -join '; ')
    } else {
        Add-Check 'isolation' $true ("each lane changed only its own file (" + ($files -join ', ') + "); the main checkout is clean at $short")
    }

    # 4c. Git has no worktree outside the lanes folder (besides the main checkout, and any that
    # were there before the gate).
    $worktrees = @(Get-Worktrees)
    $outside = @($worktrees | Where-Object { (Norm $_) -ne (Norm $RepoRoot) -and -not ($LanesDir -and (Test-Under $_ $LanesDir)) })
    $old = @($worktreesBefore | ForEach-Object { Norm $_ })
    $new = @($outside | Where-Object { $old -notcontains (Norm $_) })
    $earlier = @($outside | Where-Object { $old -contains (Norm $_) })
    if (-not $LanesDir) {
        Add-Check 'worktrees' $false 'no lanes, so no lanes folder to check against'
    } elseif ($new.Count -gt 0) {
        Add-Check 'worktrees' $false "worktrees outside ${LanesDir}: $($new -join ', ')"
    } else {
        $inside = @($worktrees | Where-Object { Test-Under $_ $LanesDir }).Count
        $note = ''
        if ($earlier.Count -gt 0) { $note = "; $($earlier.Count) from before the gate are elsewhere: $($earlier -join ', ')" }
        Add-Check 'worktrees' $true "the main checkout and $inside worktrees in $LanesDir, none elsewhere$note"
    }

    # 4d. `git worktree add` from inside a session is turned down: Claude Code's PreToolUse
    # hook, run the way Claude Code runs it, from inside gate-1, for its Bash and PowerShell tools.
    $cwd = $RepoRoot
    if ($after.Lanes['gate-1']) { $cwd = [string]$after.Lanes['gate-1'].path }
    $problems = @()
    $reason = ''
    foreach ($try in @(@('Bash', 'git worktree add ../gate-extra'), @('PowerShell', 'git worktree add ..\gate-extra'))) {
        $hookInput = [ordered]@{
            session_id      = 'lane-gate'
            cwd             = $cwd
            hook_event_name = 'PreToolUse'
            tool_name       = $try[0]
            tool_input      = [ordered]@{ command = $try[1] }
        } | ConvertTo-Json -Compress
        $r = Invoke-Tool $LaneExe @('hook', 'claude', 'pre-tool-use') $cwd $hookInput
        $v = ConvertFrom-JsonText $r.Out
        if ($r.Code -ne 0 -or -not $v -or [string]$v.hookSpecificOutput.permissionDecision -ne 'deny') {
            $problems += "$($try[0]): ``$($try[1])`` wasn't turned down (exit $($r.Code), $($r.Out.Trim()) $($r.Err.Trim()))"
        } else {
            $reason = ([string]$v.hookSpecificOutput.permissionDecisionReason -split '(?<=\.) ')[0]
        }
    }
    if ($problems.Count -gt 0) { Add-Check 'deny' $false ($problems -join '; ') }
    else { Add-Check 'deny' $true "git worktree add in gate-1 is turned down for Bash and PowerShell: $reason" }

    # 4e. `lane savings` counts the five lanes.
    $savings = Get-Savings
    if (-not $savingsBefore -or -not $savings) {
        Add-Check 'savings' $false 'lane savings --json failed: run it in the repo to see why'
    } else {
        $wantNow = [int]$savingsBefore.now.lanes + 5
        $wantTotal = [int]$savingsBefore.total.lanes + 5
        if ([int]$savings.now.lanes -eq $wantNow -and [int]$savings.total.lanes -eq $wantTotal) {
            Add-Check 'savings' $true "lane savings counts the five: $($savings.now.lanes) lanes now (was $($savingsBefore.now.lanes)), $($savings.total.lanes) in total (was $($savingsBefore.total.lanes))"
        } else {
            Add-Check 'savings' $false "lane savings counts $($savings.now.lanes) lanes now (expected $wantNow) and $($savings.total.lanes) in total (expected $wantTotal)"
        }
    }

    # 5. Clean up through Claude Code's WorktreeRemove hook, then check the lanes are gone.
    Say 'removing the lanes through Claude Code''s WorktreeRemove hook'
    $done = Remove-GateLanes
    $problems = @($done.Failed)
    $left = Get-Lanes
    foreach ($name in $Names) { if ($left.Lanes[$name]) { $problems += "$name is still in lane list" } }
    foreach ($l in $ours) {
        if (Test-Path -LiteralPath ([string]$l.path)) { $problems += "$($l.path) is still there" }
    }
    $worktrees = @(Get-Worktrees | ForEach-Object { Norm $_ })
    foreach ($l in $ours) {
        if ($worktrees -contains (Norm ([string]$l.path))) { $problems += "Git still has the worktree $($l.path)" }
    }
    $status = Invoke-Git $RepoRoot @('status', '--porcelain')
    if ($status.Out.Trim()) { $problems += 'the main checkout has changes' }
    if ($problems.Count -gt 0) {
        Add-Check 'cleanup' $false (($problems -join '; ') + '. Remove what is left with `lane rm <name>`.')
    } else {
        $how = "$($done.Hook.Count) through Claude Code's WorktreeRemove hook"
        if ($done.Rm.Count -gt 0) { $how += ", $($done.Rm.Count) with lane rm" }
        Add-Check 'cleanup' $true "the five lanes are gone ($how): no lane, folder or worktree left, and the main checkout is clean"
    }

    # 6. Removed lanes still count in `lane savings`' total.
    $savings = Get-Savings
    if (-not $savingsBefore -or -not $savings) {
        Add-Check 'savings-total' $false 'lane savings --json failed: run it in the repo to see why'
    } else {
        $wantNow = [int]$savingsBefore.now.lanes
        $wantTotal = [int]$savingsBefore.total.lanes + 5
        $wantRemoved = [int]$savingsBefore.total.removed + 5
        if ([int]$savings.now.lanes -eq $wantNow -and [int]$savings.total.lanes -eq $wantTotal -and [int]$savings.total.removed -eq $wantRemoved) {
            Add-Check 'savings-total' $true "the removed lanes still count: $($savings.total.lanes) lanes in total, $($savings.total.removed) of them removed; $($savings.now.lanes) now"
        } else {
            Add-Check 'savings-total' $false "lane savings counts $($savings.now.lanes) lanes now (expected $wantNow), $($savings.total.lanes) in total (expected $wantTotal), $($savings.total.removed) removed (expected $wantRemoved)"
        }
    }

    Write-Result ''
}

$code = 1
try {
    $code = Invoke-Gate
} catch {
    Add-Check 'error' $false "the gate stopped: $($_.Exception.Message) (line $($_.InvocationInfo.ScriptLineNumber))"
    $code = Write-Result ''
} finally {
    # Interrupted or failed part-way: stop the sessions and take the lanes away, so the next
    # run starts clean.
    foreach ($s in @($Sessions)) { if ($s -and -not $s.Process.HasExited) { Stop-Tree $s.Process.Id } }
    if ($MadeLanes -and -not $Cleaned -and $LaneExe -and $RepoRoot) {
        try { $null = Remove-GateLanes } catch { }
    }
}
exit $code
