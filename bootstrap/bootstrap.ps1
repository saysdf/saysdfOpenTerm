# saysdfTerm bootstrap for Windows: build from source and install for the current user.
#
#   powershell -ExecutionPolicy Bypass -File bootstrap\bootstrap.ps1 [-DryRun] [-Yes]
#       [-Dir <folder>] [-Ref <tag>] [-InstallDir <folder>] [-NoShortcut]
#   powershell -ExecutionPolicy Bypass -File bootstrap\bootstrap.ps1 -Uninstall
#
# Steps (each is printed before it runs; nothing is installed without asking, -Yes answers "yes"):
#   1. prerequisites: git, Rust (rustup), Strawberry Perl, MSVC C++ build tools;
#      missing git / Rust / Perl can be installed with winget after you agree;
#   2. source: the checkout this script is in, or a clone of https://github.com/saysdf/saysdfOpenTerm.git
#      into -Dir (default %USERPROFILE%\src\saysdfTerm) at -Ref (default: the latest v* tag);
#   3. release build with saysdf\tools\check.ps1 -Build (job limit leaves >= 10% RAM/CPU free,
#      low priority);
#   4. install with saysdf\tools\install.ps1 into -InstallDir (default
#      %LOCALAPPDATA%\Programs\saysdfTerm) plus a Start menu shortcut; no admin rights needed.
# -DryRun prints the plan and changes nothing. -Uninstall removes the install and the shortcut
# (your config in %USERPROFILE%\.config\saysdfTerm is kept).
# Windows PowerShell 5.1 compatible.
param(
    [string]$Dir,
    [string]$Ref,
    [string]$InstallDir,
    [switch]$Yes,
    [switch]$DryRun,
    [switch]$Uninstall,
    [switch]$NoShortcut
)
$ErrorActionPreference = 'Stop'

$RepoUrl = 'https://github.com/saysdf/saysdfOpenTerm.git'
$ConfigPath = Join-Path $env:USERPROFILE '.config\saysdfTerm\saysdfTerm.lua'
$PsExe = (Get-Process -Id $PID).Path

function Write-Step([string]$t) { Write-Host ''; Write-Host "==> $t" -ForegroundColor Cyan }
function Write-Info([string]$t) { Write-Host "    $t" }
function Write-Warn([string]$t) { Write-Host "    $t" -ForegroundColor Yellow }
function Stop-Bootstrap([string]$t) { Write-Host ''; Write-Host $t -ForegroundColor Red; exit 1 }

# Ask a yes/no question. -Yes answers yes; -DryRun prints the question and assumes yes
# (nothing is executed in a dry run anyway).
function Confirm-Action([string]$question) {
    if ($DryRun) { Write-Info "[dry run] would ask: $question"; return $true }
    if ($Yes) { Write-Info "$question -> yes (-Yes)"; return $true }
    $a = Read-Host "    $question [y/N]"
    return ($a -match '^(y|yes)$')
}

# Run a native command visibly; in a dry run only print it. Throws on a non-zero exit code.
# $safeInDryRun: the command is itself a dry run (install.ps1 -DryRun) and may run.
function Invoke-Native([string]$exe, [string[]]$arguments, [bool]$safeInDryRun = $false) {
    $shown = ($arguments | ForEach-Object { if ($_ -match '\s') { "`"$_`"" } else { $_ } }) -join ' '
    Write-Host "    > $exe $shown" -ForegroundColor DarkGray
    if ($DryRun -and -not $safeInDryRun) { return }
    $saved = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'   # native stderr (git progress) is not an error
    try { & $exe @arguments } finally { $ErrorActionPreference = $saved }
    if ($LASTEXITCODE -ne 0) { Stop-Bootstrap "Failed ($LASTEXITCODE): $exe $shown" }
}

function Update-SessionPath {
    # Pick up PATH entries added by installers (winget, rustup) without dropping this session's own.
    $current = @($env:PATH -split ';' | Where-Object { $_ })
    $stored = @(([Environment]::GetEnvironmentVariable('Path', 'Machine') + ';' + [Environment]::GetEnvironmentVariable('Path', 'User')) -split ';' | Where-Object { $_ })
    foreach ($p in $stored) { if ($current -notcontains $p) { $current += $p } }
    $cargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
    if ((Test-Path $cargoBin) -and ($current -notcontains $cargoBin)) { $current = @($cargoBin) + $current }
    $env:PATH = $current -join ';'
}

function Test-Command([string]$name) { return [bool](Get-Command $name -ErrorAction SilentlyContinue) }

function Install-WithWinget([string]$id, [string]$what) {
    if (-not (Test-Command 'winget')) { return $false }
    if (-not (Confirm-Action "Install $what with 'winget install --id $id -e' (per-user where the package allows)?")) { return $false }
    Invoke-Native 'winget' @('install', '--id', $id, '-e', '--accept-source-agreements', '--accept-package-agreements')
    Update-SessionPath
    return $true
}

# --- where is the source?
$scriptRepo = $null
if ($PSScriptRoot) {
    $candidate = Split-Path -Parent $PSScriptRoot
    if ((Test-Path (Join-Path $candidate 'Cargo.toml')) -and (Test-Path (Join-Path $candidate 'saysdf\tools\check.ps1'))) { $scriptRepo = $candidate }
}
if (-not $Dir) { $Dir = Join-Path $env:USERPROFILE 'src\saysdfTerm' }
$src = if ($scriptRepo) { $scriptRepo } else { [IO.Path]::GetFullPath($Dir) }

$installArgs = @()
if ($InstallDir) { $installArgs += @('-Dest', $InstallDir) }
if ($NoShortcut) { $installArgs += '-NoShortcut' }
if ($DryRun) { $installArgs += '-DryRun' }

Write-Host 'saysdfTerm bootstrap (Windows)' -ForegroundColor Green
if ($DryRun) { Write-Host 'Dry run: nothing will be installed, cloned, built or changed.' -ForegroundColor Yellow }

# --- uninstall
if ($Uninstall) {
    $installer = Join-Path $src 'saysdf\tools\install.ps1'
    if (-not (Test-Path $installer)) { Stop-Bootstrap "Cannot find $installer. Pass -Dir <your saysdfTerm checkout>." }
    Write-Step 'Uninstall'
    Invoke-Native $PsExe (@('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $installer, '-Uninstall') + $installArgs) $true
    Write-Info "The source in $src is kept; delete it yourself if you no longer need it."
    exit 0
}

# --- 1. prerequisites
Write-Step '1/4 Prerequisites'
Update-SessionPath
$hasWinget = Test-Command 'winget'
if (-not $hasWinget) { Write-Warn 'winget not found: missing tools cannot be installed automatically; the script prints what to install.' }
$missing = @()

if (Test-Command 'git') { Write-Info "git: $((& git --version) -join ' ')" }
elseif (-not (Install-WithWinget 'Git.Git' 'Git for Windows')) { $missing += 'git: https://git-scm.com/download/win (or: winget install --id Git.Git -e)' }

if (Test-Command 'cargo') { Write-Info "Rust: $((& cargo --version) -join ' ')" }
elseif (Install-WithWinget 'Rustlang.Rustup' 'Rust (rustup, the official Rust toolchain installer)') {
    if (-not $DryRun -and -not (Test-Command 'cargo')) {
        if (Test-Command 'rustup') { Invoke-Native 'rustup' @('default', 'stable') }
        Update-SessionPath
    }
} else { $missing += 'Rust: https://rustup.rs (or: winget install --id Rustlang.Rustup -e)' }

# The vendored OpenSSL needs a native Windows Perl; Git's MSYS perl does not work.
$perlOk = Test-Path 'C:\Strawberry\perl\bin\perl.exe'
if (-not $perlOk) {
    $perlCmd = Get-Command perl -ErrorAction SilentlyContinue
    if ($perlCmd -and ($perlCmd.Source -notmatch '\\usr\\bin\\')) { $perlOk = $true }
}
if ($perlOk) { Write-Info 'Perl: found' }
elseif (-not (Install-WithWinget 'StrawberryPerl.StrawberryPerl' 'Strawberry Perl (needed to build the bundled OpenSSL)')) {
    $missing += 'Strawberry Perl: https://strawberryperl.com (or: winget install --id StrawberryPerl.StrawberryPerl -e)'
}

# MSVC C++ build tools (the linker and Windows SDK used by Rust's msvc toolchain)
$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
$vcPath = $null
if (Test-Path $vswhere) {
    $vcPath = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
}
if ($vcPath) { Write-Info "MSVC build tools: $vcPath" }
else {
    $missing += ('MSVC C++ build tools (not installed automatically, the installer is large). Install "Desktop development with C++" from ' +
        'https://visualstudio.microsoft.com/visual-cpp-build-tools/ or run: ' +
        'winget install --id Microsoft.VisualStudio.2022.BuildTools -e --override "--passive --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"')
}

if ($missing.Count -gt 0) {
    Write-Warn 'Missing:'
    $missing | ForEach-Object { Write-Warn "  - $_" }
    if (-not $DryRun) { Stop-Bootstrap 'Install the missing tools, open a new PowerShell window and run the bootstrap again.' }
}

# --- 2. source
Write-Step '2/4 Source'
if ($scriptRepo) {
    Write-Info "Using the checkout this script is in: $src"
    if ($Ref) { Write-Warn "-Ref is used only for a fresh clone; check out '$Ref' in $src yourself if you want it." }
    $needInit = $false
    if (Test-Command 'git') {
        $st = & git -C $src submodule status 2>$null
        foreach ($l in $st) { if ($l -match '^-') { $needInit = $true } }
    }
    if ($needInit) { Invoke-Native 'git' @('-C', $src, 'submodule', 'update', '--init', '--recursive') }
    else { Write-Info 'Submodules: initialized' }
} else {
    if (Test-Path (Join-Path $src '.git')) {
        Write-Info "Updating the existing clone in $src"
        Invoke-Native 'git' @('-C', $src, 'fetch', '--tags', 'origin')
    } else {
        Write-Info "Cloning $RepoUrl into $src"
        $parent = Split-Path -Parent $src
        if (-not $DryRun -and -not (Test-Path $parent)) { New-Item -ItemType Directory -Path $parent | Out-Null }
        Invoke-Native 'git' @('clone', '--recurse-submodules', $RepoUrl, $src)
    }
    if (-not $Ref) {
        if ($DryRun -and -not (Test-Path (Join-Path $src '.git'))) { $Ref = '<latest v* tag>' }
        else {
            $Ref = (& git -C $src tag --list 'v*' --sort=-v:refname | Select-Object -First 1)
            if (-not $Ref) { $Ref = 'origin/HEAD'; Write-Warn 'No release tags yet: building the default branch.' }
        }
    }
    Write-Info "Ref: $Ref"
    Invoke-Native 'git' @('-C', $src, 'checkout', '--quiet', $Ref)
    Invoke-Native 'git' @('-C', $src, 'submodule', 'update', '--init', '--recursive')
}

# --- 3. build
Write-Step '3/4 Build (release; the first build takes 20-40 minutes)'
$check = Join-Path $src 'saysdf\tools\check.ps1'
$buildEnv = Join-Path $src 'saysdf\tools\build-env.ps1'
if (Test-Path $buildEnv) {
    . $buildEnv
    $j = Get-SaysdfBuildJobs
    $jobs = if ($env:CARGO_BUILD_JOBS) { "$($env:CARGO_BUILD_JOBS) (from CARGO_BUILD_JOBS)" } else { "$($j.Jobs) ($($j.Info))" }
    Write-Info "Build directory: $(Get-SaysdfTargetDir $src)"
    Write-Info "Parallel jobs: $jobs; cargo runs at BelowNormal priority"
}
Invoke-Native $PsExe @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $check, '-Build')

# --- 4. install
Write-Step '4/4 Install (current user, no admin rights)'
$installer = Join-Path $src 'saysdf\tools\install.ps1'
Invoke-Native $PsExe (@('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $installer) + $installArgs) (Test-Path $installer)

$dest = $InstallDir
if (-not $dest) { if ($env:SAYSDFTERM_INSTALL_DIR) { $dest = $env:SAYSDFTERM_INSTALL_DIR } else { $dest = Join-Path $env:LOCALAPPDATA 'Programs\saysdfTerm' } }
Write-Host ''
if ($DryRun) { Write-Host 'Dry run finished: nothing was changed.' -ForegroundColor Yellow }
else { Write-Host 'Done.' -ForegroundColor Green }
Write-Host "  Installed to:  $dest"
if (-not $NoShortcut) { Write-Host '  Start:         Start menu -> saysdfTerm' }
Write-Host "  Or run:        $(Join-Path $dest 'saysdfTerm.exe')"
Write-Host "  Config file:   $ConfigPath   (create it to customize; see README.md)"
Write-Host "  Source:        $src"
Write-Host "  Update:        run this script again; uninstall: add -Uninstall"
