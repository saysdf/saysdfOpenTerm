# saysdfTerminal: syntax check for shell-integration scripts.
# Parses every .ps1 (powershell.exe and pwsh, if present), `bash -n` for .sh/.bashrc,
# `zsh -n` for zsh files, `fish --no-execute` for .fish. Missing shells are skipped with a message.
# Usage: powershell -NoProfile -File saysdf/tools/check-shell-scripts.ps1 [-Root <dir>] [-RequireAll]
# Works on Windows PowerShell and pwsh (Windows/Linux). -RequireAll: a missing shell that has scripts is an error (exit 1), not a skip.
param(
    [string]$Root = (Join-Path (Split-Path -Parent $PSScriptRoot) 'shell-integration'),
    [switch]$RequireAll
)
$ErrorActionPreference = 'Stop'
$failed = 0

if (-not (Test-Path -LiteralPath $Root)) { Write-Host "no scripts dir: $Root (nothing to check)"; exit 0 }
$Root = (Resolve-Path -LiteralPath $Root).Path
$all = @(Get-ChildItem -LiteralPath $Root -Recurse -File -Force)
$repoRoot = (Resolve-Path (Split-Path -Parent (Split-Path -Parent $PSScriptRoot))).Path
$upstreamSh = Join-Path (Join-Path (Join-Path $repoRoot 'assets') 'shell-integration') 'wezterm.sh'

function Find-Exe([string[]]$Names) {
    foreach ($n in $Names) {
        # skip the WSL launcher System32\bash.exe: it cannot read Windows paths
        $c = Get-Command $n -CommandType Application -ErrorAction SilentlyContinue |
            Where-Object { $_.Source -notmatch '[\\/](System32|Sysnative|SysWOW64)[\\/]bash\.exe$' } | Select-Object -First 1
        if ($c) { return $c.Source }
    }
    return $null
}
$isWin = [IO.Path]::DirectorySeparatorChar -eq '\'

function Skip-Missing([string]$Shell) {
    if ($RequireAll) { Write-Host "FAIL ${Shell}: not found, but -RequireAll is set and there are $Shell scripts"; $script:failed++ }
    else { Write-Host "skip ${Shell}: not found" }
}

function Invoke-Check([string]$Label, [string]$Exe, [string[]]$ArgList) {
    $out = & $Exe @ArgList 2>&1
    if ($LASTEXITCODE -ne 0) {
        Write-Host "FAIL $Label"; $out | ForEach-Object { Write-Host "  $_" }
        $script:failed++
    } else { Write-Host "ok   $Label" }
}

# PowerShell
$ps1 = @($all | Where-Object { $_.Extension -eq '.ps1' })
function Get-ParseCmd([string]$Path) {
    '$e=$null; [void][System.Management.Automation.Language.Parser]::ParseFile(''' + $Path.Replace("'", "''") + ''',[ref]$null,[ref]$e); if($e){$e | ForEach-Object { $_.ToString() }; exit 1}'
}
foreach ($ps in @(@{ n = 'powershell'; exe = (Find-Exe @('powershell.exe')) }, @{ n = 'pwsh'; exe = (Find-Exe @('pwsh.exe', 'pwsh')) })) {
    if (-not $ps.exe) { Write-Host "skip $($ps.n): not found"; continue }
    foreach ($f in $ps1) { Invoke-Check "$($ps.n) parse $($f.FullName)" $ps.exe @('-NoProfile', '-NonInteractive', '-Command', (Get-ParseCmd $f.FullName)) }
}

# bash
$bashFiles = @($all | Where-Object { $_.Extension -in '.sh', '.bash' -or $_.Name -like '*.bashrc' })
if (Test-Path -LiteralPath $upstreamSh) { $bashFiles += Get-Item -LiteralPath $upstreamSh }
function Find-Bash {
    if (-not $isWin) { return (Find-Exe @('bash')) }
    # Windows: Git Bash first; never System32\bash.exe (WSL launcher, fails without a distro)
    $cands = @()
    foreach ($pf in @($env:ProgramFiles, ${env:ProgramFiles(x86)}, $env:ProgramW6432, (Join-Path $env:LOCALAPPDATA 'Programs'))) {
        if ($pf) { $cands += Join-Path (Join-Path (Join-Path $pf 'Git') 'bin') 'bash.exe' }
    }
    $git = Get-Command git.exe -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($git) {
        $gd = Split-Path -Parent $git.Source                       # ...\Git\cmd or ...\Git\bin
        $cands += Join-Path (Split-Path -Parent $gd) 'bin\bash.exe'
        $cands += Join-Path $gd 'bash.exe'
        $ep = & $git.Source --exec-path 2>$null                    # ...\Git\mingw64\libexec\git-core
        if ($ep) {
            $d = $ep.Trim().Replace('/', '\')
            for ($i = 0; $i -lt 4 -and $d; $i++) {
                $cands += Join-Path $d 'bin\bash.exe'
                $d = Split-Path -Parent $d
            }
        }
    }
    $sys = if ($env:SystemRoot) { [IO.Path]::GetFullPath($env:SystemRoot) } else { 'C:\Windows' }
    foreach ($c in $cands) {
        if (-not (Test-Path -LiteralPath $c)) { continue }
        $full = [IO.Path]::GetFullPath($c)
        if ($full.StartsWith($sys, [StringComparison]::OrdinalIgnoreCase)) { continue }
        return $full
    }
    return (Find-Exe @('bash'))   # other bash on PATH (MSYS2, Cygwin); System32 filtered out
}
$bash = Find-Bash
if ($bashFiles.Count -eq 0) { Write-Host 'skip bash: no scripts' }
elseif (-not $bash) { Skip-Missing 'bash' }
else { foreach ($f in $bashFiles) { Invoke-Check "bash -n $($f.FullName)" $bash @('-n', $f.FullName) } }

# zsh
$zshFiles = @($all | Where-Object { $_.Extension -eq '.zsh' -or $_.Name -in '.zshrc', '.zshenv', '.zprofile', '.zlogin' })
$zsh = Find-Exe @('zsh')
if ($zshFiles.Count -eq 0) { Write-Host 'skip zsh: no scripts' }
elseif (-not $zsh) { Skip-Missing 'zsh' }
else { foreach ($f in $zshFiles) { Invoke-Check "zsh -n $($f.FullName)" $zsh @('-n', $f.FullName) } }

# fish
$fishFiles = @($all | Where-Object { $_.Extension -eq '.fish' })
$fish = Find-Exe @('fish')
if ($fishFiles.Count -eq 0) { Write-Host 'skip fish: no scripts' }
elseif (-not $fish) { Skip-Missing 'fish' }
else { foreach ($f in $fishFiles) { Invoke-Check "fish --no-execute $($f.FullName)" $fish @('--no-execute', $f.FullName) } }

if ($failed) { Write-Host "$failed check(s) failed"; exit 1 }
Write-Host 'all shell script checks passed'
exit 0
