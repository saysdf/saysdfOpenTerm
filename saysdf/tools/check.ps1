# One check script for people and agents: cargo check/test/clippy/build with a job limit and low priority.
param(
    [string]$Crate,      # crate for check/test/clippy; without it - the whole workspace
    [switch]$NoTest,     # cargo check only
    [switch]$Clippy,     # cargo clippy instead of check/test
    [switch]$Build,      # release build of saysdfTerm, saysdfTerm-cli, saysdfTerm-mux-server, saysdf (saysdf-cli)
    [switch]$Run,        # build and start the terminal
    [string]$Filter      # only `cargo test <package> <Filter>` (no cargo check), e.g. -Crate wezterm-gui -Filter saysdf_
)
$ErrorActionPreference = 'Stop'

$repo = (Resolve-Path "$PSScriptRoot/../..").Path
Set-Location $repo

. (Join-Path $PSScriptRoot 'build-env.ps1')

# Build cache shared by all worktrees when "<main checkout>-target" exists (a full build is ~25 min)
$env:CARGO_TARGET_DIR = Get-SaysdfTargetDir $repo
# A build leaves >= 10% of CPU and RAM free (it used to be OOM-killed)
Set-SaysdfBuildJobs
# Perl builds the vendored OpenSSL on Windows
Add-SaysdfPerlPath
function Invoke-Cargo { Invoke-SaysdfCargo ([string[]]$args) }

$pkg = if ($Crate) { @('-p', $Crate) } else { @('--workspace') }

if ($Build -or $Run) {
    # Upstream packages wezterm-gui/wezterm/wezterm-mux-server build the saysdfTerm* binaries
    $pkgArgs = @(); foreach ($p in $SaysdfReleasePackages) { $pkgArgs += @('-p', $p) }
    Invoke-Cargo build --release @pkgArgs
    $ext = if ($env:OS -eq 'Windows_NT') { '.exe' } else { '' }
    foreach ($name in $SaysdfBinaries) {
        $bin = Join-Path $env:CARGO_TARGET_DIR "release/$name$ext"
        if (-not (Test-Path $bin)) { throw "not found: $bin" }
    }
    if ($Run) {
        $exe = Join-Path $env:CARGO_TARGET_DIR "release/saysdfTerm$ext"
        Start-Process $exe
    }
} elseif ($Filter) {
    Invoke-Cargo test @pkg $Filter
} elseif ($Clippy) {
    Invoke-Cargo clippy @pkg
} else {
    Invoke-Cargo check @pkg
    if (-not $NoTest) { Invoke-Cargo test @pkg }
}
Write-Host 'OK' -ForegroundColor Green
