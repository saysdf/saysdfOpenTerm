# Installs saysdfTerm from a release build into a per-user folder (no admin rights) and adds a
# Start menu shortcut. The previous install is replaced atomically and restored if the copy fails.
#   powershell -NoProfile -File saysdf/tools/install.ps1 [-Dest <folder>] [-Source <release dir>] [-NoShortcut] [-DryRun]
#   powershell -NoProfile -File saysdf/tools/install.ps1 -Uninstall [-Dest <folder>]
# Default -Dest: $env:SAYSDFTERM_INSTALL_DIR, else %LOCALAPPDATA%\Programs\saysdfTerm.
# Used by bootstrap/bootstrap.ps1. Windows PowerShell 5.1 compatible.
param(
    [string]$Dest,
    [string]$Source,
    [switch]$NoShortcut,
    [switch]$Uninstall,
    [switch]$DryRun
)
$ErrorActionPreference = 'Stop'

$repo = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
. (Join-Path $PSScriptRoot 'build-env.ps1')

if (-not $Dest) {
    if ($env:SAYSDFTERM_INSTALL_DIR) { $Dest = $env:SAYSDFTERM_INSTALL_DIR }
    else { $Dest = Join-Path $env:LOCALAPPDATA 'Programs\saysdfTerm' }
}
$Dest = [IO.Path]::GetFullPath($Dest).TrimEnd('\')
$lnkPath = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\saysdfTerm.lnk'
$destExe = Join-Path $Dest 'saysdfTerm.exe'

function Test-SaysdfRunning {
    $running = @(Get-Process -Name 'saysdfTerm', 'saysdfTerm-mux-server' -ErrorAction SilentlyContinue | Where-Object {
        $p = $null
        try { $p = $_.Path } catch { }
        $p -and ($p -like "$Dest\*")
    })
    return ($running.Count -gt 0)
}

if ($Uninstall) {
    Write-Host "Uninstall saysdfTerm from $Dest"
    if (Test-Path -LiteralPath $Dest) {
        if (-not (Test-Path -LiteralPath $destExe)) {
            Write-Host "$Dest does not contain saysdfTerm.exe - refusing to delete it." -ForegroundColor Red
            exit 1
        }
        if (Test-SaysdfRunning) { Write-Host "saysdfTerm is running from $Dest - close it and try again." -ForegroundColor Yellow; exit 1 }
        if ($DryRun) { Write-Host "[dry run] would remove $Dest" } else { Remove-Item -LiteralPath $Dest -Recurse -Force; Write-Host "Removed $Dest" }
    } else {
        Write-Host "$Dest does not exist."
    }
    if (Test-Path -LiteralPath $lnkPath) {
        $lnkTarget = (New-Object -ComObject WScript.Shell).CreateShortcut($lnkPath).TargetPath
        if ($lnkTarget -notlike "$Dest\*") { Write-Host "Shortcut $lnkPath points to $lnkTarget, not to $Dest - kept." }
        elseif ($DryRun) { Write-Host "[dry run] would remove $lnkPath" } else { Remove-Item -LiteralPath $lnkPath -Force; Write-Host "Removed $lnkPath" }
    }
    Write-Host 'Your configuration (~\.config\saysdfTerm) and data were not touched.'
    exit 0
}

if (-not $Source) { $Source = Join-Path (Get-SaysdfTargetDir $repo) 'release' }

# Binaries come from the release folder; conpty/OpenConsole, shell integration and the license from
# the repository (same layout as saysdf/ci/package.sh).
$binNames = $SaysdfBinaries | ForEach-Object { "$_.exe" }
$conhost = Join-Path $repo 'assets\windows\conhost'
$siSaysdf = Join-Path $repo 'saysdf\shell-integration'
$siWezterm = Join-Path $repo 'assets\shell-integration\wezterm.sh'
$license = Join-Path $repo 'LICENSE.md'

Write-Host 'Install saysdfTerm'
Write-Host "  from: $Source"
Write-Host "  to:   $Dest"
if (-not $NoShortcut) { Write-Host "  Start menu shortcut: $lnkPath" }

$missing = @()
foreach ($n in $binNames) { if (-not (Test-Path -LiteralPath (Join-Path $Source $n))) { $missing += (Join-Path $Source $n) } }
foreach ($n in 'conpty.dll', 'OpenConsole.exe') { if (-not (Test-Path -LiteralPath (Join-Path $conhost $n))) { $missing += (Join-Path $conhost $n) } }
if (-not (Test-Path -LiteralPath $siSaysdf)) { $missing += $siSaysdf }
if (-not (Test-Path -LiteralPath $siWezterm)) { $missing += $siWezterm }
if ($missing.Count -gt 0) {
    if ($DryRun) {
        Write-Host '[dry run] not built yet (the build step creates them):' -ForegroundColor Yellow
        $missing | ForEach-Object { Write-Host "  $_" }
    } else {
        Write-Host 'Missing files:' -ForegroundColor Red
        $missing | ForEach-Object { Write-Host "  $_" }
        Write-Host 'Build first: saysdf/tools/check.ps1 -Build' -ForegroundColor Red
        exit 1
    }
}
if ($DryRun) {
    Write-Host "[dry run] would copy $($binNames -join ', '), conpty.dll, OpenConsole.exe, shell-integration\, LICENSE.md into $Dest (via $Dest.new, atomically)"
    exit 0
}

# Do not kill a running saysdfTerm from Dest.
if (Test-SaysdfRunning) {
    Write-Host "saysdfTerm is running from $Dest - close it and try again." -ForegroundColor Yellow
    exit 1
}

$new = "$Dest.new"
$old = "$Dest.old"
$parent = Split-Path -Parent $Dest
if (-not (Test-Path -LiteralPath $parent)) { New-Item -ItemType Directory -Path $parent | Out-Null }
foreach ($d in $new, $old) { if (Test-Path -LiteralPath $d) { Remove-Item -LiteralPath $d -Recurse -Force } }

$copied = @()
try {
    New-Item -ItemType Directory -Path $new | Out-Null
    foreach ($n in $binNames) { Copy-Item -LiteralPath (Join-Path $Source $n) -Destination $new; $copied += $n }
    foreach ($n in 'conpty.dll', 'OpenConsole.exe') { Copy-Item -LiteralPath (Join-Path $conhost $n) -Destination $new; $copied += $n }
    $si = Join-Path $new 'shell-integration'
    New-Item -ItemType Directory -Path $si | Out-Null
    Copy-Item -Path (Join-Path $siSaysdf '*') -Destination $si -Recurse -Force
    Copy-Item -LiteralPath $siWezterm -Destination $si -Force
    $copied += 'shell-integration\'
    if (Test-Path -LiteralPath $license) { Copy-Item -LiteralPath $license -Destination $new; $copied += 'LICENSE.md' }
} catch {
    if (Test-Path -LiteralPath $new) { Remove-Item -LiteralPath $new -Recurse -Force -ErrorAction SilentlyContinue }
    Write-Host "Copy failed: $($_.Exception.Message). The installed version was not touched." -ForegroundColor Red
    exit 1
}

# Swap: old -> .old, new -> Dest; on failure put the old one back.
$hadOld = Test-Path -LiteralPath $Dest
try {
    if ($hadOld) { Move-Item -LiteralPath $Dest -Destination $old }
    Move-Item -LiteralPath $new -Destination $Dest
} catch {
    Write-Host "Replace failed: $($_.Exception.Message)" -ForegroundColor Red
    if ($hadOld -and (Test-Path -LiteralPath $old) -and -not (Test-Path -LiteralPath $Dest)) {
        Move-Item -LiteralPath $old -Destination $Dest
        Write-Host 'The previous version was restored (the folder may be in use - close saysdfTerm).' -ForegroundColor Yellow
    }
    if (Test-Path -LiteralPath $new) { Remove-Item -LiteralPath $new -Recurse -Force -ErrorAction SilentlyContinue }
    exit 1
}
if (Test-Path -LiteralPath $old) { Remove-Item -LiteralPath $old -Recurse -Force -ErrorAction SilentlyContinue }

if (-not $NoShortcut) {
    $sh = New-Object -ComObject WScript.Shell
    $lnk = $sh.CreateShortcut($lnkPath)
    $lnk.TargetPath = $destExe
    $lnk.WorkingDirectory = $Dest
    $lnk.Save()
} else {
    $lnkPath = $null
}

$ver = (& (Join-Path $Dest 'saysdfTerm-cli.exe') --version) 2>&1 | Out-String
Write-Host "Installed: $($ver.Trim())" -ForegroundColor Green
Write-Host "Folder: $Dest"
Write-Host "Source: $Source"
Write-Host "Copied: $($copied -join ', ')"
if ($lnkPath) { Write-Host "Shortcut: $lnkPath" }
