# Shared build environment for check.ps1, install.ps1 and bootstrap/bootstrap.ps1 (dot-source it):
#   . "$PSScriptRoot/build-env.ps1"
# - Get-SaysdfTargetDir: the cargo build directory;
# - Set-SaysdfBuildJobs: CARGO_BUILD_JOBS that leaves >= 10% of RAM and CPU free (~1.5 GB per rustc job);
# - Add-SaysdfPerlPath: Strawberry Perl on PATH (Windows; needed for the vendored OpenSSL);
# - Invoke-SaysdfCargo: cargo at low priority (BelowNormal on Windows, nice -n 10 elsewhere).
# Windows PowerShell 5.1 compatible.

$script:SaysdfOnWindows = ($env:OS -eq 'Windows_NT')

# CARGO_TARGET_DIR if set; else, if "<main checkout>-target" exists: that directory for the main
# checkout and "<main checkout>-target-wt/<worktree name>" for a linked git worktree; else "<repo>/target".
# Worktrees must not share the main build directory: cargo decides freshness of workspace crates by
# file mtimes at paths relative to the package, so a build in one worktree makes another checkout
# reuse its stale objects (seen with saysdf-lua-bundle: tests ran the other worktree's Lua sources).
function Get-SaysdfTargetDir([string]$Repo) {
    if ($env:CARGO_TARGET_DIR) { return $env:CARGO_TARGET_DIR }
    $common = $null; $top = $null
    try {
        $common = (& git -C $Repo rev-parse --path-format=absolute --git-common-dir 2>$null)
        if ($LASTEXITCODE -ne 0) { $common = $null }
        $top = (& git -C $Repo rev-parse --show-toplevel 2>$null)
        if ($LASTEXITCODE -ne 0) { $top = $null }
    } catch { $common = $null }
    if ($common) {
        $common = [IO.Path]::GetFullPath(([string]$common).Trim())
        if ((Split-Path -Leaf $common) -eq '.git') {
            $main = Split-Path -Parent $common
            $shared = $main + '-target'
            if (Test-Path -LiteralPath $shared -PathType Container) {
                if ($top) {
                    $top = [IO.Path]::GetFullPath(([string]$top).Trim())
                    if ($top.TrimEnd('\', '/') -ne $main.TrimEnd('\', '/')) {
                        return (Join-Path ($main + '-target-wt') (Split-Path -Leaf $top))
                    }
                }
                return $shared
            }
        }
    }
    return (Join-Path $Repo 'target')
}

# Returns @{ Jobs; Info } without changing the environment.
function Get-SaysdfBuildJobs {
    $cores = [Environment]::ProcessorCount
    $freeGb = $null; $totalGb = $null
    try {
        if ($script:SaysdfOnWindows) {
            $os = Get-CimInstance Win32_OperatingSystem
            $freeGb = $os.FreePhysicalMemory / 1MB      # KB -> GB
            $totalGb = $os.TotalVisibleMemorySize / 1MB
        } elseif (Test-Path /proc/meminfo) {
            $mi = Get-Content /proc/meminfo
            $freeKb = [double](($mi | Where-Object { $_ -match '^MemAvailable:\s+(\d+)' } | Select-Object -First 1) -replace '\D', '')
            $totKb = [double](($mi | Where-Object { $_ -match '^MemTotal:\s+(\d+)' } | Select-Object -First 1) -replace '\D', '')
            $freeGb = $freeKb / 1MB; $totalGb = $totKb / 1MB
        } else {
            $totalGb = [double](& sysctl -n hw.memsize) / 1GB
            $vm = & vm_stat
            $page = [double](($vm | Select-Object -First 1) -replace '\D', '')
            $pages = 0.0
            foreach ($l in $vm) {
                if ($l -match '^Pages (free|inactive|speculative):\s+(\d+)') { $pages += [double]$Matches[2] }
            }
            $freeGb = $pages * $page / 1GB
        }
    } catch { $freeGb = $null }
    $cpuJobs = [Math]::Max(1, [Math]::Min([Math]::Floor($cores * 0.9), $cores - 1))
    if ($freeGb -and $totalGb) {
        $memJobs = [Math]::Max(1, [Math]::Floor(($freeGb - 0.1 * $totalGb) / 1.5))
        $ramInfo = ('free RAM {0:N1} GB of {1:N1} GB' -f $freeGb, $totalGb)
    } else {
        $memJobs = 2
        $ramInfo = 'free RAM unknown'
    }
    $jobs = [int][Math]::Max(1, [Math]::Min($cpuJobs, $memJobs))
    return @{ Jobs = $jobs; Info = "$ramInfo, cores $cores" }
}

# Sets CARGO_BUILD_JOBS unless the user already did.
function Set-SaysdfBuildJobs {
    if ($env:CARGO_BUILD_JOBS) {
        Write-Host "cargo jobs: $($env:CARGO_BUILD_JOBS) (from CARGO_BUILD_JOBS)"
        return
    }
    $j = Get-SaysdfBuildJobs
    $env:CARGO_BUILD_JOBS = [string]$j.Jobs
    Write-Host "cargo jobs: $($j.Jobs) ($($j.Info))"
}

# Strawberry Perl builds the vendored OpenSSL on Windows.
function Add-SaysdfPerlPath {
    if (-not $script:SaysdfOnWindows) { return }
    $perl = 'C:\Strawberry\perl\bin'
    if ((Test-Path $perl) -and ($env:PATH -notlike "*$perl*")) {
        $env:PATH = "$perl;$env:PATH"
    }
}

# cargo at low priority; child processes (rustc) inherit it. Throws on failure.
# Pass the arguments as one array: Invoke-SaysdfCargo @('build', '--release', '-p', 'x').
function Invoke-SaysdfCargo([string[]]$CargoArgs) {
    Write-Host "> cargo $($CargoArgs -join ' ')" -ForegroundColor Cyan
    if ($script:SaysdfOnWindows) {
        $p = Start-Process -FilePath 'cargo' -ArgumentList $CargoArgs -NoNewWindow -PassThru
        $null = $p.Handle   # cache the handle, otherwise ExitCode can be empty
        try { $p.PriorityClass = [System.Diagnostics.ProcessPriorityClass]::BelowNormal } catch {}
        $p.WaitForExit()
        $code = $p.ExitCode
    } else {
        & nice -n 10 cargo @CargoArgs
        $code = $LASTEXITCODE
    }
    if ($code -ne 0) { throw "cargo $($CargoArgs -join ' ') failed with exit code $code" }
}

# Release binaries built by `check.ps1 -Build` (upstream packages wezterm-gui, wezterm,
# wezterm-mux-server and the fork's saysdf-cli produce them).
$script:SaysdfReleasePackages = @('wezterm-gui', 'wezterm', 'wezterm-mux-server', 'saysdf-cli')
$script:SaysdfBinaries = @('saysdfTerm', 'saysdfTerm-cli', 'saysdfTerm-mux-server', 'saysdf')
