<#
.SYNOPSIS
Benchmark 1 for Lanes on Windows: ready-to-run copies of a repo, made with `git worktree add`
plus the repo's install, against `lane new`.

.DESCRIPTION
Makes 1, 5 and 10 copies of each repo in bench/repos.ini, two ways: `git worktree add` plus the
repo's install (with a warm package cache), and `lane new`. Measures wall time, disk added (the
drop in the volume's free space, from Get-Volume) and the time to the first passing test in a
fresh copy, with hyperfine. Writes bench/results/<date>-<machine>.json.

bench/run.sh is the macOS and Linux version; both write the same results format. How to run
it fairly: bench/README.md.

.EXAMPLE
.\bench\run.ps1 -Repo lanes -Runs 2
A quick smoke run against this repo.

.EXAMPLE
.\bench\run.ps1 -Work D:\lanes-bench
The full matrix on a Dev Drive.
#>
[CmdletBinding()]
param(
    # Only these repos. Default: every repo in bench/repos.ini that runs on Windows, except
    # `lanes` (this repo, the quick target). Lists are split at commas here, because
    # `powershell -File` passes `-Repo astro,ruff` as one string.
    [string[]]$Repo = @(),
    # Timed runs per measurement (fewer than 10 marks a smoke run).
    [int]$Runs = 10,
    # Warmup runs, not counted.
    [int]$Warmup = 1,
    # How many copies to make at once. Strings, split at commas here: `powershell -File` passes
    # `-Copies 1,5` as "1,5", which as a number is 15.
    [string[]]$Copies = @('1', '5', '10'),
    # Where the clones and copies go: on the disk you want to measure. Default: <drive>:\lanes-bench
    # on the first ReFS volume (a Dev Drive), else %USERPROFILE%\lanes-bench.
    [string]$Work = '',
    # The lane binary. Default: `lane` on PATH.
    [string]$Lane = 'lane',
    # The machine's name in the results file. Default: from the OS, CPU and filesystem; never
    # the hostname.
    [string]$Machine = '',
    # Write the results here instead of bench/results/.
    [string]$Out = '',
    # Read free space once it has held still this long (within 1 MiB), in seconds. Windows frees
    # deleted files late, so longer than run.sh's 5.
    [int]$Settle = 15,
    # Skip the time to the first passing test.
    [switch]$NoFirstTest,
    # Skip the copies measurements: to check a repo's test subset, or to measure only that.
    [switch]$NoCopies,
    # Mark the results as a smoke run whatever the settings.
    [switch]$Smoke,
    # Internal: what hyperfine's --prepare and --cleanup run.
    [string]$Step = '',
    [string]$RepoDir = '',
    [string]$Samples = 'none',
    [switch]$Final
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2

$BenchDir = $PSScriptRoot
$Self = Join-Path $BenchDir 'run.ps1'
$RepoRoot = Split-Path -Parent $BenchDir
$ReposFile = Join-Path $BenchDir 'repos.ini'
$PsExe = (Get-Process -Id $PID).Path

# The protocol every published number uses. Anything else is a smoke run.
$StdRuns = 10
$StdWarmup = 1
$StdCopies = @(1, 5, 10)

function Stop-Bench([string]$Message, [string]$Hint = '') {
    [Console]::Error.WriteLine("run.ps1: $Message")
    if ($Hint) { [Console]::Error.WriteLine("  $Hint") }
    exit 1
}

function Write-Step([string]$Message) { [Console]::Error.WriteLine("==> $Message") }

# To stderr, and to the measurement's warnings file, which goes in the results (hyperfine hides
# what its --prepare step prints).
function Write-Warn([string]$Message) {
    [Console]::Error.WriteLine("Warning: $Message")
    if ($env:BENCH_WARNINGS) { [IO.File]::AppendAllText($env:BENCH_WARNINGS, "$Message`n") }
}

# --- bench/repos.ini ---------------------------------------------------------------------------

function Read-Ini {
    $ini = [ordered]@{}
    $sec = $null
    foreach ($line in Get-Content -LiteralPath $ReposFile -Encoding UTF8) {
        $t = $line.Trim()
        if ($t -eq '' -or $t.StartsWith('#') -or $t.StartsWith(';')) { continue }
        if ($t -match '^\[(.+)\]$') {
            $sec = $Matches[1]
            $ini[$sec] = @{}
            continue
        }
        $i = $t.IndexOf('=')
        if ($i -lt 1 -or -not $sec) { continue }
        $k = $t.Substring(0, $i).Trim()
        if (-not $ini[$sec].ContainsKey($k)) { $ini[$sec][$k] = $t.Substring($i + 1).Trim() }
    }
    $ini
}

# The value of KEY in SECTION, preferring KEY.windows (like `test.windows`) over KEY.
function Get-Conf([string]$Section, [string]$Key) {
    $s = $script:Ini[$Section]
    if ($s.ContainsKey("$Key.windows")) { return $s["$Key.windows"] }
    if ($s.ContainsKey($Key)) { return $s[$Key] }
    return ''
}

function Split-Words([string]$Text) { @($Text -split '\s+' | Where-Object { $_ -ne '' }) }

# --- Running programs -------------------------------------------------------------------------

# A command line argument, quoted the way Windows programs read them (PowerShell 5.1 doesn't
# escape quotes inside arguments, so hyperfine's are built by hand).
function ConvertTo-Arg([string]$A) {
    if ($A -ne '' -and $A -notmatch '[\s"]') { return $A }
    $sb = New-Object Text.StringBuilder
    [void]$sb.Append('"')
    $bs = 0
    foreach ($c in $A.ToCharArray()) {
        if ($c -eq '\') { $bs++; continue }
        if ($c -eq '"') { [void]$sb.Append('\' * ($bs * 2 + 1)).Append('"') }
        else { [void]$sb.Append('\' * $bs).Append($c) }
        $bs = 0
    }
    [void]$sb.Append('\' * ($bs * 2)).Append('"')
    $sb.ToString()
}

# Run a program with its output on this console; returns the exit code.
function Invoke-Program([string]$Exe, [string[]]$Arguments, [string]$Dir = (Get-Location).Path) {
    $psi = New-Object Diagnostics.ProcessStartInfo
    $psi.FileName = $Exe
    $psi.Arguments = (@($Arguments | ForEach-Object { ConvertTo-Arg $_ })) -join ' '
    $psi.WorkingDirectory = $Dir
    $psi.UseShellExecute = $false
    $p = [Diagnostics.Process]::Start($psi)
    $p.WaitForExit()
    $p.ExitCode
}

# Run a program quietly; returns its first line of output, or '' when it fails.
function Get-Output([string]$Exe, [string[]]$Arguments) {
    $old = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        $out = & $Exe @Arguments 2>$null
        if ($LASTEXITCODE -ne 0) { return '' }
        return [string](@($out) | Select-Object -First 1)
    } catch { return '' } finally { $ErrorActionPreference = $old }
}

function Invoke-Quiet([string]$Exe, [string[]]$Arguments) {
    $old = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try { & $Exe @Arguments *> $null; return $LASTEXITCODE } catch { return 1 } finally { $ErrorActionPreference = $old }
}

# Remove a folder, deep node_modules paths and all.
function Remove-Tree([string]$Path) {
    if (Test-Path -LiteralPath $Path) {
        Invoke-Quiet 'cmd.exe' @('/D', '/C', "rd /s /q `"\\?\$Path`"") | Out-Null
        if (Test-Path -LiteralPath $Path) { Remove-Item -LiteralPath $Path -Recurse -Force }
    }
}

# --- Free space -------------------------------------------------------------------------------

function Get-DriveLetter([string]$Path) {
    $root = [IO.Path]::GetPathRoot((Resolve-Path -LiteralPath $Path).ProviderPath)
    if ($root -notmatch '^([A-Za-z]):') {
        Stop-Bench "$Path isn't on a drive with a letter" 'Pick a work folder on a lettered drive with -Work.'
    }
    $Matches[1].ToUpper()
}

# Free bytes on the volume holding $Path, from Get-Volume. Never a sum of file sizes: that
# counts a cloned file at full size.
function Get-FreeBytes([string]$Path) {
    [long](Get-Volume -DriveLetter (Get-DriveLetter $Path)).SizeRemaining
}

# The same number through .NET, for polling: Get-Volume takes about a second a call, this a
# millisecond.
function Get-FreeBytesFast([string]$Letter) {
    [long](New-Object IO.DriveInfo $Letter).TotalFreeSpace
}

# Flush, then wait until the free space holds still (within 1 MiB for $Window seconds, default
# $env:BENCH_SETTLE) and, given a floor, is back up to it; return it. Filesystems allocate
# written blocks, and free deleted ones, seconds later: on a Dev Drive, deleting ten worktrees of
# this repo gave nothing back for a minute or more, then all of it at once. So a quiet spell
# alone can't tell that a delete is done; getting back to the free space from before the copies
# can.
function Get-SettledFree([string]$Path, $Floor = $null, [int]$Window = -1) {
    $letter = Get-DriveLetter $Path
    if ($Window -lt 0) {
        $Window = 15
        if ($env:BENCH_SETTLE) { $Window = [int]$env:BENCH_SETTLE }
    }
    $tol = 1MB
    $cap = [math]::Max(120, $Window * 4)
    if ($null -ne $Floor) { $cap = 900 }
    $clock = [Diagnostics.Stopwatch]::StartNew()
    $flushed = -10.0
    $since = 0.0
    $anchor = Get-FreeBytesFast $letter
    $now = $anchor
    while ($true) {
        $t = $clock.Elapsed.TotalSeconds
        if ($t - $flushed -ge 10) {
            try { Write-VolumeCache -DriveLetter $letter -ErrorAction Stop } catch { }
            $flushed = $t
        }
        $now = Get-FreeBytesFast $letter
        if ([math]::Abs($now - $anchor) -gt $tol) { $anchor = $now; $since = $clock.Elapsed.TotalSeconds }
        $t = $clock.Elapsed.TotalSeconds
        if ($t - $since -ge $Window -and ($null -eq $Floor -or $now -ge $Floor)) { break }
        if ($t -ge $cap) {
            if ($null -ne $Floor -and $now -lt $Floor) {
                Write-Warn ("free space on ${letter}: stayed $([math]::Round(($Floor - $now) / 1MB)) MiB short of where it was before the copies for $($cap / 60) minutes; something else wrote to this disk.")
            } else {
                Write-Warn ("free space on ${letter}: didn't hold still for $Window s in $cap s; something else is writing to this disk.")
            }
            break
        }
        Start-Sleep -Milliseconds 500
    }
    Get-FreeBytes $Path
}

# --- Removing copies (hyperfine's --prepare and --cleanup) ------------------------------------

# Remove every copy the bench made in $Dir (hyperfine's --prepare, before every run, and
# --cleanup). With a samples file, free.before holds the free space read before the last run made
# its copies: record how much they took (the drop since then), remove them, and wait until the
# free space is back there, less a little slack, so their deletes never land in the next run;
# then, unless this is the last run, keep that as the next run's free.before. With samples
# `none` (the first-test runs), just remove them and let the disk settle.
function Reset-Copies([string]$Dir, [string]$SamplesFile = 'none', [switch]$IsFinal) {
    $beforeFile = Join-Path $Dir 'free.before'
    $used = [long]0
    $before = $null
    if ($SamplesFile -ne 'none' -and (Test-Path -LiteralPath $beforeFile)) {
        $before = [long](Get-Content -LiteralPath $beforeFile)
        $after = Get-SettledFree $Dir
        $used = $before - $after
        Add-Content -LiteralPath $SamplesFile -Value $used -Encoding ascii
    }
    Remove-Item -LiteralPath $beforeFile -ErrorAction SilentlyContinue
    $main = Join-Path $Dir 'main'
    $lanes = Join-Path $Dir 'lanes'
    if (Test-Path -LiteralPath $lanes) {
        foreach ($d in @(Get-ChildItem -LiteralPath $lanes -Directory -Filter 'bench-*')) {
            Push-Location $main
            try { $code = Invoke-Quiet $env:BENCH_LANE @('rm', $d.Name) } finally { Pop-Location }
            if ($code -ne 0) { Remove-Tree $d.FullName }
        }
    }
    $wt = Join-Path $Dir 'wt'
    if (Test-Path -LiteralPath $wt) {
        foreach ($d in @(Get-ChildItem -LiteralPath $wt -Directory)) {
            Invoke-Quiet 'git' @('-C', $main, 'worktree', 'remove', '--force', '--force', $d.FullName) | Out-Null
            Remove-Tree $d.FullName
        }
    }
    Invoke-Quiet 'git' @('-C', $main, 'worktree', 'prune') | Out-Null
    if (-not $IsFinal) {
        # The timed steps' logs start empty every run, so they don't grow from run to run. (Not
        # after the last run: a failure's log is read then.)
        foreach ($log in @(Get-ChildItem -LiteralPath (Join-Path $Dir 'logs') -File -ErrorAction SilentlyContinue |
                Where-Object { $_.Name -like 'copies-*.log' -or $_.Name -like 'first-test-*.log' })) {
            Clear-Content -LiteralPath $log.FullName
        }
    }
    $now = $null
    if ($null -ne $before) {
        if ($used -lt 0) { $used = 0 }
        $now = Get-SettledFree $Dir ($before - 2MB - [long]($used / 100))
    } elseif (-not $IsFinal) {
        $now = Get-SettledFree $Dir
    }
    if ($SamplesFile -ne 'none' -and -not $IsFinal) { Set-Content -LiteralPath $beforeFile -Value $now -Encoding ascii }
}

if ($Step -eq 'reset') {
    Reset-Copies $RepoDir $Samples -IsFinal:$Final
    exit 0
}

# --- Machine details --------------------------------------------------------------------------

function Get-MachineInfo([string]$WorkDir, [string]$Name) {
    $cpu = @(Get-CimInstance Win32_Processor)[0]
    $cs = Get-CimInstance Win32_ComputerSystem
    $os = Get-CimInstance Win32_OperatingSystem
    $letter = Get-DriveLetter $WorkDir
    $vol = Get-Volume -DriveLetter $letter
    $disk = $null
    $physical = $null
    try {
        $disk = Get-Partition -DriveLetter $letter | Get-Disk
        $physical = @(Get-PhysicalDisk | Where-Object { [string]$_.DeviceId -eq [string]$disk.Number })[0]
    } catch { }
    $devDrive = $null
    $q = Get-Output 'fsutil' @('devdrv', 'query', "${letter}:")
    if ($q) { $devDrive = $q }
    $host_ = $null
    if ($disk -and $disk.BusType -eq 'File Backed Virtual' -and $disk.Location) {
        # A Dev Drive in a VHDX: the disk that holds the file is the one doing the work.
        try {
            $hl = Get-DriveLetter $disk.Location
            $hd = Get-Partition -DriveLetter $hl | Get-Disk
            $host_ = [ordered]@{ volume = "${hl}:"; model = $hd.FriendlyName; bus = [string]$hd.BusType }
        } catch { }
    }
    [ordered]@{
        name          = $Name
        os            = "$($os.Caption) $($os.Version) (build $($os.BuildNumber))"
        kernel        = "Windows NT $($os.Version)"
        arch          = $env:PROCESSOR_ARCHITECTURE
        cpu           = [ordered]@{ model = $cpu.Name.Trim(); logical_cores = [int]$cs.NumberOfLogicalProcessors }
        memory_bytes  = [long]$cs.TotalPhysicalMemory
        disk          = [ordered]@{
            filesystem  = $vol.FileSystem
            mount       = "${letter}:\"
            label       = $vol.FileSystemLabel
            size_bytes  = [long]$vol.Size
            device      = if ($disk) { "disk $($disk.Number)" } else { $null }
            model       = if ($disk) { $disk.FriendlyName } else { $null }
            bus         = if ($disk) { [string]$disk.BusType } else { $null }
            solid_state = if ($physical) { [string]$physical.MediaType } else { $null }
            dev_drive   = $devDrive
            virtual_disk_host = $host_
        }
        lanes_version     = Get-Output $env:BENCH_LANE @('--version')
        git_version       = Get-Output 'git' @('--version')
        hyperfine_version = Get-Output 'hyperfine' @('--version')
        powershell        = $PSVersionTable.PSVersion.ToString()
    }
}

# A name for the results file from the OS, CPU and filesystem: never the hostname.
function Get-MachineSlug([string]$WorkDir) {
    $cpu = @(Get-CimInstance Win32_Processor)[0].Name
    $fs = (Get-Volume -DriveLetter (Get-DriveLetter $WorkDir)).FileSystem
    $s = "windows-$cpu-$fs".ToLower()
    $s = $s -replace '\((r|tm)\)', '' -replace '(cpu|processor|with radeon graphics)', ''
    $s = $s -replace '[^a-z0-9]+', '-' -replace '^-+|-+$', ''
    if ($s.Length -gt 60) { $s = $s.Substring(0, 60) }
    $s.TrimEnd('-')
}

# --- Statistics -------------------------------------------------------------------------------

# Median, and p95 by linear interpolation between the closest ranks (numpy's default). Never the
# best run. run.sh computes the same.
function Get-Stats([double[]]$Values) {
    if ($null -eq $Values -or $Values.Count -eq 0) { return $null }
    $s = [double[]]@($Values | Sort-Object)
    $n = $s.Count
    if ($n % 2 -eq 1) { $median = $s[[int](($n - 1) / 2)] }
    else { $median = ($s[$n / 2 - 1] + $s[$n / 2]) / 2 }
    $r = 0.95 * ($n - 1)
    $lo = [int][math]::Floor($r)
    $hi = [int][math]::Ceiling($r)
    [ordered]@{
        n      = $n
        median = $median
        p95    = $s[$lo] + ($s[$hi] - $s[$lo]) * ($r - $lo)
        runs   = $Values
    }
}

# Wall times from a hyperfine JSON export (1.x has `times`, 2.x `measurements`).
function Get-WallTimes($Export) {
    $r = $Export.results[0]
    if ($r.PSObject.Properties.Name -contains 'times') { return [double[]]@($r.times) }
    [double[]]@($r.measurements | ForEach-Object { $_.time_wall_clock.value })
}

function Read-Json([string]$Path) { Get-Content -LiteralPath $Path -Raw -Encoding UTF8 | ConvertFrom-Json }

# --- Step scripts: exactly what hyperfine times -----------------------------------------------

function ConvertTo-CmdText([string]$Text) { $Text -replace '%', '%%' }

# A step script (.cmd) that runs BODY in the repo's main checkout, its output going to LOG.
# A repo's `env` are defaults (a variable already set wins); a variant's always apply.
function New-StepFile([string]$Path, [string]$Log, [string[]]$Body, [string]$VariantEnv = '') {
    $lines = New-Object Collections.Generic.List[string]
    $lines.Add('@echo off')
    $lines.Add('rem Made by bench/run.ps1. hyperfine times this script.')
    $lines.Add('setlocal')
    $lines.Add("set `"LANE_STATE_DIR=$env:LANE_STATE_DIR`"")
    $lines.Add("set `"LANE_CONFIG_DIR=$env:LANE_CONFIG_DIR`"")
    $lines.Add('set "LANE_TELEMETRY=0"')
    $lines.Add('set "DO_NOT_TRACK=1"')
    foreach ($kv in Split-Words $script:RepoEnv) {
        $k, $v = $kv -split '=', 2
        $lines.Add("if not defined $k set `"$k=$(ConvertTo-CmdText $v)`"")
    }
    foreach ($kv in Split-Words $VariantEnv) {
        $k, $v = $kv -split '=', 2
        $lines.Add("set `"$k=$(ConvertTo-CmdText $v)`"")
    }
    $lines.Add("cd /d `"$(Join-Path $script:Dir 'main')`"")
    foreach ($b in $Body) {
        if ($b.StartsWith(':')) { $lines.Add($b.Substring(1)); continue }
        # `a && b` becomes two calls, each checked, so a .cmd shim like pnpm returns here.
        foreach ($part in ($b -split '\s+&&\s+')) {
            $lines.Add("call $part >>`"$Log`" 2>&1")
            $lines.Add('if errorlevel 1 exit /b 1')
        }
    }
    $lines.Add('exit /b 0')
    [IO.File]::WriteAllLines($Path, $lines.ToArray(), (New-Object Text.UTF8Encoding $false))
}

function Get-CopyPath([string]$Method, [string]$Name) {
    if ($Method -eq 'worktree') { return Join-Path (Join-Path $script:Dir 'wt') $Name }
    Join-Path (Join-Path $script:Dir 'lanes') $Name
}

# Lines that make one copy, ready to run. Lines starting with ':' go in as they are.
function Get-CopyLines([string]$Method, [string]$Name) {
    if ($Method -eq 'worktree') {
        $p = Get-CopyPath $Method $Name
        return @(
            "git worktree add --detach --quiet `"$p`" $($script:Commit)",
            ":pushd `"$p`"",
            (ConvertTo-CmdText $script:Install),
            ':popd'
        )
    }
    @("`"$env:BENCH_LANE`" new $Name --json")
}

# --- Measuring --------------------------------------------------------------------------------

function Get-ResetCommand([string]$SamplesFile, [switch]$IsFinal) {
    $c = "`"$PsExe`" -NoProfile -ExecutionPolicy Bypass -File `"$Self`" -Step reset -RepoDir `"$($script:Dir)`" -Samples `"$SamplesFile`""
    if ($IsFinal) { $c += ' -Final' }
    # hyperfine runs this with cmd /C, which strips the first and last quote of a line that
    # starts with one: so add a pair to strip.
    "`"$c`""
}

# Time STEP with hyperfine, removing the copies before every run. Free-space samples land in
# JSON.disk. Returns whether it worked; a failure stops the bench, unless MayFail.
function Invoke-Measure([string]$Label, [string]$StepFile, [bool]$Disk, [string]$Json, [bool]$MayFail = $false) {
    $samplesFile = 'none'
    Remove-Item -LiteralPath (Join-Path $script:Dir 'free.before') -ErrorAction SilentlyContinue
    $env:BENCH_WARNINGS = "$Json.warnings"
    [IO.File]::WriteAllText($env:BENCH_WARNINGS, '')
    if ($Disk) {
        $samplesFile = "$Json.disk"
        Set-Content -LiteralPath $samplesFile -Value $null -Encoding ascii
    }
    # Start on a quiet disk: a longer quiet spell than between runs.
    Get-SettledFree $script:Dir $null ($Settle * 4) | Out-Null
    Write-Step "$($script:RepoName): $Label"
    $code = Invoke-Program $script:Hyperfine @(
        '-N', '--warmup', "$Warmup", '--runs', "$Runs", '--export-json', $Json,
        '--command-name', $Label,
        '--prepare', (Get-ResetCommand $samplesFile),
        '--cleanup', (Get-ResetCommand $samplesFile -IsFinal),
        "cmd /D /C '$StepFile'")
    if ($code -ne 0) {
        Reset-Copies $script:Dir 'none' -IsFinal
        $log = Join-Path $script:Logs (([IO.Path]::GetFileNameWithoutExtension($StepFile)) + '.log')
        [Console]::Error.WriteLine("The last lines of ${log}:")
        if (Test-Path -LiteralPath $log) { Get-Content -LiteralPath $log -Tail 40 | ForEach-Object { [Console]::Error.WriteLine($_) } }
        if ($MayFail) {
            Write-Step "$($script:RepoName): $Label failed; the results say so, and the bench goes on."
            return $false
        }
        Stop-Bench "$($script:RepoName): $Label failed" "Fix the command in bench/repos.ini, or run $StepFile by hand to see why."
    }
    $true
}

# A first test that didn't pass in a fresh copy, for the results: no time, and the end of its
# log (lines cut to 300 characters: some tools print whole scripts in their errors).
function Get-FailedResult([string]$Method, [string]$Variant, [string]$StepFile, [string]$Json) {
    $log = Join-Path $script:Logs (([IO.Path]::GetFileNameWithoutExtension($StepFile)) + '.log')
    $tail = @(Get-Content -LiteralPath $log -Tail 25 -ErrorAction SilentlyContinue | Where-Object { $_ -match '\S' } |
            ForEach-Object { $l = [string]$_; if ($l.Length -gt 300) { $l.Substring(0, 300) + '...' } else { $l } })
    [ordered]@{
        measure          = 'first_test'
        method           = $Method
        variant          = if ($Variant) { $Variant } else { $null }
        copies           = 1
        failed           = $true
        error            = $tail
        wall_seconds     = $null
        disk_added_bytes = $null
        warnings         = @(Get-Content -LiteralPath "$Json.warnings" -Encoding UTF8 -ErrorAction SilentlyContinue |
                Where-Object { $_ -match '\S' } | ForEach-Object { [string]$_ })
        hyperfine        = $null
    }
}

function Get-Result([string]$Method, [string]$Variant, [int]$N, [string]$Json, [string]$Kind) {
    $export = Read-Json $Json
    # The step's path relative to the work folder, so no user name lands in the results.
    $export.results[0].command = 'steps/' + [IO.Path]::GetFileNameWithoutExtension($Json) + '.cmd'
    $disk = $null
    $diskFile = "$Json.disk"
    if (Test-Path -LiteralPath $diskFile) {
        $d = [double[]]@(Get-Content -LiteralPath $diskFile | Where-Object { $_ -match '\S' } | ForEach-Object { [double]$_ })
        if ($d.Count -gt 0) {
            # The first samples are the warmup runs'.
            $keep = [math]::Min($Runs, $d.Count)
            $disk = Get-Stats ([double[]]@($d[($d.Count - $keep)..($d.Count - 1)]))
        }
    }
    [ordered]@{
        measure          = $Kind
        method           = $Method
        variant          = if ($Variant) { $Variant } else { $null }
        copies           = $N
        wall_seconds     = Get-Stats (Get-WallTimes $export)
        disk_added_bytes = $disk
        # Plain strings: Get-Content's lines carry PSDrive and PSProvider, which ConvertTo-Json would
        # follow for ever.
        warnings         = @(Get-Content -LiteralPath "$Json.warnings" -Encoding UTF8 -ErrorAction SilentlyContinue |
                Where-Object { $_ -match '\S' } | ForEach-Object { [string]$_ })
        hyperfine        = $export.results[0]
    }
}

# --- One repo ---------------------------------------------------------------------------------

function Test-Service([string]$Service) {
    switch ($Service) {
        'postgres' { $port = 5432; if ($env:PGPORT) { $port = [int]$env:PGPORT } }
        'redis' { $port = 6379 }
        default { Stop-Bench "bench/repos.ini names an unknown service ``$Service``" 'Use postgres or redis.' }
    }
    $c = New-Object Net.Sockets.TcpClient
    try {
        $ok = $c.ConnectAsync('127.0.0.1', $port).Wait(2000)
    } catch { $ok = $false } finally { $c.Close() }
    if (-not $ok) {
        Stop-Bench "$($script:RepoName) needs $Service on localhost:$port, and nothing answers there" `
            "Start $Service (bench/README.md says how), or leave $($script:RepoName) out with -Repo."
    }
}

function Invoke-Repo([string]$Name) {
    $env:BENCH_WARNINGS = $null
    $script:RepoName = $Name
    $script:Dir = Join-Path $WorkDir $Name
    $script:Logs = Join-Path $script:Dir 'logs'
    $url = Get-Conf $Name 'url'
    $script:Commit = Get-Conf $Name 'commit'
    $script:Install = Get-Conf $Name 'install'
    $testCmd = Get-Conf $Name 'test'
    $script:RepoEnv = Get-Conf $Name 'env'
    $variants = @(Split-Words (Get-Conf $Name 'variants'))
    if ($variants.Count -eq 0) { $variants = @('stock') }
    if (-not ($url -and $script:Commit -and $script:Install -and $testCmd)) {
        Stop-Bench "bench/repos.ini: [$Name] needs url, commit, install and test"
    }
    $source = $url
    if ($url -eq '.') { $source = $RepoRoot }

    $required = @(Split-Words (Get-Conf $Name 'requires'))
    foreach ($tool in $required) {
        if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) {
            Stop-Bench "$Name needs ``$tool``, which isn't on PATH" `
                "Install it (bench/README.md lists what each repo needs), or leave $Name out with -Repo."
        }
    }
    foreach ($s in Split-Words (Get-Conf $Name 'services')) { Test-Service $s }

    $dir = $script:Dir
    $main = Join-Path $dir 'main'
    $steps = Join-Path $dir 'steps'
    $outDir = Join-Path $dir 'out'
    foreach ($d in @($dir, $script:Logs, $steps, $outDir)) { New-Item -ItemType Directory -Force -Path $d | Out-Null }
    Get-ChildItem -LiteralPath $script:Logs -File | Remove-Item -Force
    Get-ChildItem -LiteralPath $outDir -File | Remove-Item -Force

    # The clone: shallow, at the pinned commit, kept between runs.
    if (-not (Test-Path -LiteralPath (Join-Path $main '.git'))) {
        Write-Step "${Name}: cloning $source at $($script:Commit.Substring(0, 12))"
        Remove-Tree $main
        if ((Invoke-Program 'git' @('init', '--quiet', $main)) -ne 0) { Stop-Bench "couldn't make a Git repo at $main" }
        Invoke-Program 'git' @('-C', $main, 'remote', 'add', 'origin', $source) | Out-Null
    }
    if ((Get-Output 'git' @('-C', $main, 'rev-parse', '-q', '--verify', 'HEAD')) -ne $script:Commit) {
        if ((Invoke-Program 'git' @('-C', $main, 'fetch', '--quiet', '--depth', '1', 'origin', $script:Commit)) -ne 0) {
            Stop-Bench "${Name}: couldn't fetch $($script:Commit) from $source" 'Check the network and the commit in bench/repos.ini.'
        }
        if ((Invoke-Program 'git' @('-C', $main, 'checkout', '--quiet', '--detach', '--force', $script:Commit)) -ne 0) {
            Stop-Bench "${Name}: couldn't check out $($script:Commit)"
        }
    }
    # Lanes follows the pinned commit, with the same install as the worktrees. Kept out of
    # `git status`.
    $toml = @(
        '# Written by bench/run.ps1.',
        'version = 1',
        '[lanes]',
        'dir = "../lanes"',
        "base = `"$($script:Commit)`"",
        '[setup]',
        "install = [`"$($script:Install -replace '\\', '\\' -replace '"', '\"')`"]"
    )
    [IO.File]::WriteAllLines((Join-Path $main 'lane.toml'), $toml, (New-Object Text.UTF8Encoding $false))
    # Versions as the repo sees them (corepack and rustup pick theirs from files in it).
    $tools = [ordered]@{}
    Push-Location $main
    try { foreach ($tool in $required) { $tools[$tool] = Get-Output $tool @('--version') } } finally { Pop-Location }
    $exclude = Join-Path $main '.git\info\exclude'
    if (-not ((Test-Path -LiteralPath $exclude) -and (Select-String -LiteralPath $exclude -Pattern '^lane\.toml$' -Quiet))) {
        Add-Content -LiteralPath $exclude -Value 'lane.toml' -Encoding ascii
    }

    # A clean start: no copies, no `_base`, so the first `lane new` pays for `_base` again. A
    # finished run leaves none; a stopped one may, and their deletes need time to finish.
    Remove-Item -LiteralPath (Join-Path $dir 'free.before') -ErrorAction SilentlyContinue
    $left = @(Get-ChildItem -LiteralPath (Join-Path $dir 'lanes'), (Join-Path $dir 'wt') -Force -ErrorAction SilentlyContinue)
    if ($left.Count -gt 0) {
        Write-Step "${Name}: removing what a stopped run left, then letting the disk settle"
        Reset-Copies $dir 'none' -IsFinal
        Remove-Tree (Join-Path $dir 'lanes')
        Remove-Tree (Join-Path $dir 'wt')
        Invoke-Quiet 'git' @('-C', $main, 'worktree', 'prune') | Out-Null
        # Their size is unknown, so no floor: a long quiet spell instead. A Dev Drive gave back a
        # deleted Astro lane well after 50 quiet seconds.
        Get-SettledFree $dir $null ([math]::Max(120, $Settle * 10)) | Out-Null
    }
    New-Item -ItemType Directory -Force -Path (Join-Path $dir 'wt') | Out-Null

    # Warm package caches: what a careful user has today. Every variant's install runs once: the
    # first in the clone, the others each in a worktree of their own, as the baseline installs, so
    # the clone never switches layouts (pnpm won't remove node_modules without a terminal).
    $firstVariant = $true
    foreach ($v in $variants) {
        Write-Step "${Name}: warming the package cache ($v): $($script:Install)"
        $warm = Join-Path $steps "warm-$v.cmd"
        $log = Join-Path $script:Logs "warm-$v.log"
        $body = @()
        $warmDir = $null
        if (-not $firstVariant) {
            $warmDir = Join-Path (Join-Path $dir 'wt') "warm-$v"
            $body += "git worktree add --detach --quiet `"$warmDir`" $($script:Commit)"
            $body += ":cd /d `"$warmDir`""
        }
        $body += ConvertTo-CmdText $script:Install
        New-StepFile $warm $log $body (Get-Conf $Name "env.$v")
        $code = Invoke-Program 'cmd.exe' @('/D', '/C', $warm)
        if ($warmDir) {
            Invoke-Quiet 'git' @('-C', $main, 'worktree', 'remove', '--force', '--force', $warmDir) | Out-Null
            Remove-Tree $warmDir
            Invoke-Quiet 'git' @('-C', $main, 'worktree', 'prune') | Out-Null
        }
        if ($code -ne 0) {
            if (Test-Path -LiteralPath $log) { Get-Content -LiteralPath $log -Tail 40 | ForEach-Object { [Console]::Error.WriteLine($_) } }
            Stop-Bench "${Name}: ``$($script:Install)`` failed in a plain checkout" 'Fix the install in bench/repos.ini.'
        }
        $firstVariant = $false
    }

    $results = New-Object Collections.Generic.List[object]

    # `_base`: the one-off cost of the first `lane new` in a repo (it installs there once). That
    # first lane stays until the end, so no delete is pending while disk is measured.
    $made = Join-Path $outDir 'base-made.json'
    $baseStep = Join-Path $steps 'base.cmd'
    New-StepFile $baseStep (Join-Path $script:Logs 'base.log') @(":`"$env:BENCH_LANE`" new first --json >`"$made`" 2>>`"$(Join-Path $script:Logs 'base.log')`"", ':if errorlevel 1 exit /b 1')
    $before = Get-SettledFree $dir $null ($Settle * 4)
    Write-Step "${Name}: the first lane new, which sets up _base"
    $baseJson = Join-Path $outDir 'base.json'
    $code = Invoke-Program $script:Hyperfine @('-N', '--runs', '1', '--export-json', $baseJson,
        '--command-name', 'first lane new', "cmd /D /C '$baseStep'")
    if ($code -ne 0) {
        Get-Content -LiteralPath (Join-Path $script:Logs 'base.log') -Tail 40 -ErrorAction SilentlyContinue | ForEach-Object { [Console]::Error.WriteLine($_) }
        Stop-Bench "${Name}: the first ``lane new`` failed" "Run $baseStep by hand to see why."
    }
    $after = Get-SettledFree $dir
    $m = Read-Json $made
    $baseResult = [ordered]@{
        seconds          = (Get-WallTimes (Read-Json $baseJson))[0]
        disk_added_bytes = $before - $after
        clone_method     = $m.clone.method
        filesystem       = $m.clone.filesystem
        installed        = $m.base.installed
        note             = 'One run: the first lane new in the repo makes _base, installs there, then makes the lane. Its disk is both.'
    }

    # Wall time and disk added for N copies at once.
    $counts = $CopyCounts
    if ($NoCopies) { $counts = @() }
    foreach ($n in $counts) {
        foreach ($v in @($variants + 'lane')) {
            $method = 'worktree'; $variant = $v; $venv = ''
            if ($v -eq 'lane') { $method = 'lane'; $variant = '' } else { $venv = Get-Conf $Name "env.$v" }
            $stepFile = Join-Path $steps "copies-$v-$n.cmd"
            $body = @()
            for ($i = 1; $i -le $n; $i++) { $body += Get-CopyLines $method "bench-$i" }
            New-StepFile $stepFile (Join-Path $script:Logs "copies-$v-$n.log") $body $venv
            $json = Join-Path $outDir "copies-$v-$n.json"
            $label = "$n x $method"
            if ($variant) { $label += " ($variant)" }
            $null = Invoke-Measure $label $stepFile $true $json
            $results.Add((Get-Result $method $variant $n $json 'copies'))
        }
    }

    # Time to the first passing test in a fresh copy: make one copy, then run the test there.
    # Last, because removing a copy with build output frees its space so late.
    if (-not $NoFirstTest) {
        foreach ($v in @($variants + 'lane')) {
            $method = 'worktree'; $variant = $v; $venv = ''
            if ($v -eq 'lane') { $method = 'lane'; $variant = '' } else { $venv = Get-Conf $Name "env.$v" }
            $stepFile = Join-Path $steps "first-test-$v.cmd"
            $body = @(Get-CopyLines $method 'bench-t') + @(":cd /d `"$(Get-CopyPath $method 'bench-t')`"", (ConvertTo-CmdText $testCmd))
            New-StepFile $stepFile (Join-Path $script:Logs "first-test-$v.log") $body $venv
            $json = Join-Path $outDir "first-test-$v.json"
            $label = "first test, $method"
            if ($variant) { $label += " ($variant)" }
            # A test that fails in one kind of copy is a finding, not a reason to stop.
            if (Invoke-Measure $label $stepFile $false $json $true) {
                $results.Add((Get-Result $method $variant 1 $json 'first_test'))
            } else {
                $results.Add((Get-FailedResult $method $variant $stepFile $json))
            }
        }
    }

    $repoResult = [ordered]@{
        name            = $Name
        url             = $url
        commit          = $script:Commit
        install         = $script:Install
        test            = $testCmd
        env             = @(Split-Words $script:RepoEnv)
        variants        = @($variants | ForEach-Object { [ordered]@{ name = $_; env = @(Split-Words (Get-Conf $Name "env.$_")) } })
        tools           = $tools
        lane_base_setup = $baseResult
        results         = $results.ToArray()
    }

    # Done: remove the first lane and `_base` too, and give the deletes time, so the next repo
    # starts on a quiet disk. No floor here: after many thousands of files made and deleted, ReFS
    # kept some of the space for good.
    $env:BENCH_WARNINGS = $null
    Write-Step "${Name}: removing the lanes, then letting the disk settle"
    Remove-Tree (Join-Path $dir 'lanes')
    Remove-Tree (Join-Path $dir 'wt')
    Invoke-Quiet 'git' @('-C', $main, 'worktree', 'prune') | Out-Null
    Get-SettledFree $dir $null ($Settle * 10) | Out-Null
    $repoResult
}

# --- Main -------------------------------------------------------------------------------------

if ($Runs -lt 1) { Stop-Bench '-Runs must be 1 or more' }
if ($Warmup -lt 0) { Stop-Bench '-Warmup must be 0 or more' }
if ($Settle -lt 0) { Stop-Bench '-Settle must be 0 or more seconds' }
$Repo = @($Repo | ForEach-Object { $_ -split '[,\s]+' } | Where-Object { $_ -ne '' })
$counts = @($Copies | ForEach-Object { $_ -split '[,\s]+' } | Where-Object { $_ -ne '' })
if ($counts.Count -eq 0 -or @($counts | Where-Object { $_ -notmatch '^[1-9][0-9]*$' }).Count -gt 0) {
    Stop-Bench '-Copies takes whole numbers, 1 or more, like 1,5,10'
}
$CopyCounts = [int[]]@($counts | ForEach-Object { [int]$_ })
$env:BENCH_SETTLE = [string]$Settle

foreach ($tool in @('hyperfine', 'git')) {
    if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) {
        Stop-Bench "``$tool`` isn't on PATH" "Install it: winget install $(if ($tool -eq 'git') { 'Git.Git' } else { 'sharkdp.hyperfine' }), or scoop install $tool."
    }
}
$script:Hyperfine = (Get-Command hyperfine).Source
$laneCmd = Get-Command $Lane -ErrorAction SilentlyContinue
if (-not $laneCmd) {
    Stop-Bench "can't find the lane binary ``$Lane``" 'Install Lanes, or pass -Lane with the path to lane.exe (cargo build --release puts one in target\release).'
}
$env:BENCH_LANE = $laneCmd.Source

if (-not $Work) {
    $refs = @(Get-Volume | Where-Object { $_.FileSystem -eq 'ReFS' -and $_.DriveLetter -and $_.DriveType -eq 'Fixed' })
    if ($refs.Count -gt 0) { $Work = "$($refs[0].DriveLetter):\lanes-bench" }
    else { $Work = Join-Path $env:USERPROFILE 'lanes-bench' }
}
New-Item -ItemType Directory -Force -Path $Work | Out-Null
$WorkDir = (Resolve-Path -LiteralPath $Work).ProviderPath.TrimEnd('\')
if ($WorkDir.Contains("'")) { Stop-Bench "the work folder's path can't contain a single quote" 'Pick another with -Work.' }
# The bench only ever deletes inside a work folder it made.
$marker = Join-Path $WorkDir '.lanes-bench'
if (-not (Test-Path -LiteralPath $marker)) {
    if (@(Get-ChildItem -LiteralPath $WorkDir -Force).Count -gt 0) {
        Stop-Bench "$WorkDir isn't empty and wasn't made by the bench" 'Pick an empty or new folder with -Work.'
    }
    Set-Content -LiteralPath $marker -Value 'Made by bench/run.ps1; everything here can be deleted.' -Encoding ascii
}

# Lanes keeps the bench's lanes apart from yours, and sends nothing.
$env:LANE_STATE_DIR = Join-Path $WorkDir '.lane-state'
$env:LANE_CONFIG_DIR = Join-Path $WorkDir '.lane-config'
$env:LANE_TELEMETRY = '0'
$env:DO_NOT_TRACK = '1'
Remove-Tree $env:LANE_STATE_DIR
New-Item -ItemType Directory -Force -Path $env:LANE_STATE_DIR, $env:LANE_CONFIG_DIR | Out-Null
if ($env:CARGO_TARGET_DIR) {
    [Console]::Error.WriteLine('Note: unsetting CARGO_TARGET_DIR, so each copy builds in its own target folder.')
    Remove-Item Env:CARGO_TARGET_DIR
}

$script:Ini = Read-Ini
$all = @($script:Ini.Keys)
$selected = @()
if ($Repo.Count -eq 0) {
    foreach ($r in $all) {
        if ($r -eq 'lanes') { continue }
        if (@(Split-Words (Get-Conf $r 'platforms')) -contains 'windows') { $selected += $r }
    }
} else {
    foreach ($r in $Repo) {
        if ($all -notcontains $r) { Stop-Bench "bench/repos.ini has no repo ``$r``" "Pick one of: $($all -join ', ')." }
        if (@(Split-Words (Get-Conf $r 'platforms')) -notcontains 'windows') {
            Stop-Bench "$r doesn't run on Windows in this benchmark" 'bench/repos.ini lists where it runs; use bench/run.sh on macOS or Linux (or in WSL2).'
        }
        $selected += $r
    }
}

$reasons = @()
if ($Runs -lt $StdRuns) { $reasons += "runs $Runs < $StdRuns" }
if ($Warmup -lt $StdWarmup) { $reasons += "warmup $Warmup < $StdWarmup" }
if (($CopyCounts -join ' ') -ne ($StdCopies -join ' ')) { $reasons += "copies `"$($CopyCounts -join ' ')`" instead of `"$($StdCopies -join ' ')`"" }
if ($Smoke) { $reasons += '-Smoke' }
$reason = $reasons -join '; '

if (-not $Machine) { $Machine = Get-MachineSlug $WorkDir }
if (-not $Out) {
    $date = (Get-Date).ToUniversalTime().ToString('yyyy-MM-dd')
    $suffix = ''
    if ($reason) { $suffix = '-smoke' }
    $stem = Join-Path (Join-Path $BenchDir 'results') "$date-$Machine$suffix"
    $Out = "$stem.json"
    $i = 2
    while (Test-Path -LiteralPath $Out) { $Out = "$stem-$i.json"; $i++ }
}

Write-Step "Measuring in $WorkDir, $($selected.Count) repo(s): $($selected -join ' ')"
if ($reason) { Write-Step "A smoke run ($reason): never publish these numbers." }
$started = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
$repoResults = @()
foreach ($r in $selected) { $repoResults += , (Invoke-Repo $r) }

$benchCommit = Get-Output 'git' @('-C', $RepoRoot, 'rev-parse', 'HEAD')
$doc = [ordered]@{
    schema       = 1
    benchmark    = 'new-copy'
    script       = 'bench/run.ps1'
    bench_commit = $benchCommit
    smoke        = [bool]$reason
    smoke_reason = if ($reason) { $reason } else { $null }
    started      = $started
    finished     = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
    settings     = [ordered]@{ runs = $Runs; warmup = $Warmup; copies = @(if (-not $NoCopies) { $CopyCounts }); first_test = (-not $NoFirstTest) }
    protocol     = [ordered]@{ runs = $StdRuns; warmup = $StdWarmup; copies = $StdCopies }
    stats        = 'median, and p95 by linear interpolation between the closest ranks; never the best run'
    disk         = "disk added is the drop in the volume's free space (Get-Volume), never a sum of file sizes"
    machine      = Get-MachineInfo $WorkDir $Machine
    repos        = $repoResults
}
New-Item -ItemType Directory -Force -Path (Split-Path -Parent $Out) | Out-Null
$json = ($doc | ConvertTo-Json -Depth 32) -replace "`r`n", "`n"
[IO.File]::WriteAllText($Out, $json + "`n", (New-Object Text.UTF8Encoding $false))

Write-Step "Results: $Out"
$rows = foreach ($rr in $repoResults) {
    foreach ($x in $rr.results) {
        $m = $x.method
        if ($x.variant) { $m += " ($($x.variant))" }
        $diskText = '-'
        if ($x.disk_added_bytes) { $diskText = '{0:N0} MiB' -f ($x.disk_added_bytes.median / 1MB) }
        $median = 'failed'
        $p95 = '-'
        if ($x.wall_seconds) {
            $median = '{0:N2} s' -f $x.wall_seconds.median
            $p95 = '{0:N2} s' -f $x.wall_seconds.p95
        }
        [pscustomobject]@{
            repo            = $rr.name
            measure         = $x.measure
            method          = $m
            copies          = $x.copies
            median          = $median
            p95             = $p95
            'disk (median)' = $diskText
        }
    }
}
[Console]::Error.WriteLine(($rows | Format-Table -AutoSize | Out-String).TrimEnd())
if ($reason) { Write-Step 'A smoke run: never publish these numbers.' }
