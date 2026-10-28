# Export a release of the private repository into a local clone of the public repository
# https://github.com/saysdf/saysdfOpenTerm (openspec change public-release).
#
#   saysdf/tools/release-public.ps1 -Version 0.1.0 -DryRun
#   saysdf/tools/release-public.ps1 -Version 0.1.0 -PublicRepo C:\src\saysdfTerm
#
# What it does:
#   1. requires a clean private tree; exports HEAD with `git archive` into a fresh temp directory;
#   2. drops the paths excluded in saysdf/public/manifest.toml and puts the overlay at the root;
#   3. leak scan (fails on any hit): .github/workflows, .env files, private keys, and in files that
#      differ from the upstream WezTerm base: files over the size limit, Cyrillic outside the
#      allow-list ([scan] cyrillic_allow files and lines with cyrillic_allow_marker - the same
#      keys the i18n source test reads), matches of the private deny-list (kept OUTSIDE the repository), references to
#      excluded private paths ([scan] private_refs, except private_refs_allow);
#   4. builds the public tree from the private tree entries (modes and submodules preserved) and
#      checks that every exported file hashes to the same blob id as in the private repository;
#   5. creates one commit "saysdfTerm vX.Y.Z" + changelog section on top of the previous release
#      (or on top of the upstream WezTerm commit in an empty clone; a merge with the newer upstream
#      commit when the private repository merged one) and an annotated tag vX.Y.Z.
# It never pushes: it prints the push commands. -DryRun stops after step 4 and writes nothing.
# Windows PowerShell 5.1 compatible.
param(
    [Parameter(Mandatory = $true)][string]$Version,
    [string]$PublicRepo,
    [switch]$DryRun,
    [string]$Manifest,                 # default: saysdf/public/manifest.toml
    [string]$DenyList,                 # default: [scan] denylist from the manifest
    [string]$UpstreamRef = 'upstream/main',
    [switch]$KeepTemp                  # keep the exported tree for inspection
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2.0

$utf8 = New-Object System.Text.UTF8Encoding $false

function Write-Step([string]$text) { Write-Host "==> $text" -ForegroundColor Cyan }
function Fail([string]$text) { throw "release-public: $text" }

# --- git through System.Diagnostics.Process: exact bytes in and out (no PowerShell pipeline
# re-encoding), stdin for --index-info / --stdin-paths, per-call environment.
function Format-Arg([string]$a) {
    if ($a -eq '') { return '""' }
    if ($a -notmatch '[\s"]') { return $a }
    $r = '"'; $bs = 0
    foreach ($ch in $a.ToCharArray()) {
        if ($ch -eq '\') { $bs++; continue }
        if ($ch -eq '"') { $r += ('\' * (2 * $bs + 1)) + '"'; $bs = 0; continue }
        $r += ('\' * $bs) + $ch; $bs = 0
    }
    return $r + ('\' * (2 * $bs)) + '"'
}

function Invoke-Git {
    param(
        [string]$Dir,
        [string[]]$GitArgs,
        [byte[]]$InputBytes,
        [hashtable]$Environment,
        [switch]$AllowFail
    )
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = 'git'
    $psi.Arguments = ((@('-C', $Dir) + $GitArgs) | ForEach-Object { Format-Arg $_ }) -join ' '
    $psi.UseShellExecute = $false
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.RedirectStandardInput = ($null -ne $InputBytes)
    $psi.StandardOutputEncoding = $utf8
    $psi.StandardErrorEncoding = $utf8
    if ($Environment) { foreach ($k in $Environment.Keys) { $psi.EnvironmentVariables[$k] = [string]$Environment[$k] } }
    # The stdin writer must not emit a UTF-8 BOM: .NET Framework takes its encoding from
    # Console.InputEncoding (no StandardInputEncoding property before .NET Core 2.1).
    $savedIn = $null
    if ($null -ne $InputBytes) {
        if ($psi.PSObject.Properties['StandardInputEncoding']) { $psi.StandardInputEncoding = $utf8 }
        else {
            try { $savedIn = [Console]::InputEncoding; [Console]::InputEncoding = $utf8 } catch { $savedIn = $null }
        }
    }
    try { $p = [System.Diagnostics.Process]::Start($psi) }
    finally { if ($savedIn) { try { [Console]::InputEncoding = $savedIn } catch { } } }
    $outTask = $p.StandardOutput.ReadToEndAsync()
    $errTask = $p.StandardError.ReadToEndAsync()
    if ($null -ne $InputBytes) {
        # git may exit early on an error; its stderr explains why, not the broken pipe
        try { $p.StandardInput.BaseStream.Write($InputBytes, 0, $InputBytes.Length) } catch { }
        try { $p.StandardInput.Close() } catch { }
    }
    $p.WaitForExit()
    $res = New-Object PSObject -Property @{ Code = $p.ExitCode; Out = $outTask.Result; Err = $errTask.Result }
    if ($res.Code -ne 0 -and -not $AllowFail) {
        Fail ("git {0} failed ({1}):`n{2}" -f ($GitArgs -join ' '), $res.Code, $res.Err.Trim())
    }
    return $res
}

function Get-GitLine([string]$Dir, [string[]]$GitArgs) { return (Invoke-Git -Dir $Dir -GitArgs $GitArgs).Out.Trim() }

# --- manifest: a small TOML subset (design D1)
function Remove-TomlComment([string]$line) {
    $inStr = $false; $esc = $false
    for ($i = 0; $i -lt $line.Length; $i++) {
        $c = $line[$i]
        if ($inStr) {
            if ($esc) { $esc = $false } elseif ($c -eq '\') { $esc = $true } elseif ($c -eq '"') { $inStr = $false }
        } elseif ($c -eq '"') { $inStr = $true }
        elseif ($c -eq '#') { return $line.Substring(0, $i) }
    }
    return $line
}

function ConvertFrom-TomlValue([string]$v, [string]$key) {
    $v = $v.Trim()
    $strRe = '"((?:[^"\\]|\\.)*)"'
    if ($v.StartsWith('[')) {
        if (-not $v.EndsWith(']')) { Fail "manifest: unterminated array for $key" }
        $items = @()
        foreach ($m in [regex]::Matches($v, $strRe)) { $items += [regex]::Unescape($m.Groups[1].Value) }
        return , $items
    }
    if ($v -match "^$strRe$") { return [regex]::Unescape($Matches[1]) }
    if ($v -eq 'true') { return $true }
    if ($v -eq 'false') { return $false }
    if ($v -match '^-?\d+(\.\d+)?$') { return [double]$v }
    Fail "manifest: unsupported value for ${key}: $v"
}

function Read-Manifest([string]$path) {
    $map = @{}
    $section = ''
    $pendingKey = $null; $buf = ''
    foreach ($raw in [IO.File]::ReadAllLines($path, $utf8)) {
        $line = (Remove-TomlComment $raw).Trim()
        if ($pendingKey) {
            $buf += ' ' + $line
            if ($line.EndsWith(']')) { $map[$pendingKey] = ConvertFrom-TomlValue $buf $pendingKey; $pendingKey = $null }
            continue
        }
        if (-not $line) { continue }
        if ($line -match '^\[([A-Za-z0-9_.-]+)\]$') { $section = $Matches[1]; continue }
        if ($line -match '^([A-Za-z0-9_-]+)\s*=\s*(.*)$') {
            $key = if ($section) { "$section.$($Matches[1])" } else { $Matches[1] }
            $val = $Matches[2].Trim()
            if ($val.StartsWith('[') -and -not $val.EndsWith(']')) { $pendingKey = $key; $buf = $val; continue }
            $map[$key] = ConvertFrom-TomlValue $val $key
            continue
        }
        Fail "manifest: cannot parse line: $raw"
    }
    if ($pendingKey) { Fail "manifest: unterminated array for $pendingKey" }
    return $map
}

# Glob -> anchored regex (design D1): * and ? within a segment, ** across segments,
# no "/" = name at any depth, a leading "/" = root only, a match on a directory covers everything under it.
function ConvertTo-GlobRegex([string]$glob) {
    $g = $glob.Trim()
    $rooted = $g.StartsWith('/')
    $g = $g.Trim('/')
    $sb = New-Object System.Text.StringBuilder
    $i = 0
    while ($i -lt $g.Length) {
        $c = $g[$i]
        if ($c -eq '*') {
            if ($i + 1 -lt $g.Length -and $g[$i + 1] -eq '*') {
                if ($i + 2 -lt $g.Length -and $g[$i + 2] -eq '/') { [void]$sb.Append('(?:.*/)?'); $i += 3; continue }
                [void]$sb.Append('.*'); $i += 2; continue
            }
            [void]$sb.Append('[^/]*')
        } elseif ($c -eq '?') { [void]$sb.Append('[^/]') }
        else { [void]$sb.Append([regex]::Escape([string]$c)) }
        $i++
    }
    $prefix = if ($rooted -or $g.Contains('/')) { '^' } else { '(?:^|/)' }
    return New-Object System.Text.RegularExpressions.Regex ($prefix + $sb.ToString() + '(?:/.*)?$')
}

function Test-AnyMatch($regexes, [string]$path) {
    foreach ($r in $regexes) { if ($r.IsMatch($path)) { return $true } }
    return $false
}

function Expand-HomePath([string]$p) {
    if ($p.StartsWith('~')) { $p = $HOME + $p.Substring(1) }
    return [IO.Path]::GetFullPath($p)
}

# ls-tree -r -z -> list of entries
function Get-TreeEntries([string]$Dir, [string]$Rev) {
    $out = (Invoke-Git -Dir $Dir -GitArgs @('ls-tree', '-r', '-z', '--full-tree', $Rev)).Out
    $list = New-Object System.Collections.ArrayList
    foreach ($rec in $out.Split([char]0)) {
        if (-not $rec) { continue }
        $tab = $rec.IndexOf("`t")
        $meta = $rec.Substring(0, $tab).Split(' ')
        [void]$list.Add([pscustomobject]@{ Mode = $meta[0]; Type = $meta[1]; Sha = $meta[2]; Path = $rec.Substring($tab + 1); Src = $null })
    }
    return $list
}

function Get-ChangelogSection([string]$text, [string]$ver) {
    $lines = $text -split "`r?`n"
    $head = '^##\s+v?' + [regex]::Escape($ver) + '(\s|$)'
    $start = -1
    for ($i = 0; $i -lt $lines.Count; $i++) { if ($lines[$i] -match $head) { $start = $i; break } }
    if ($start -lt 0) { return $null }
    $body = @()
    for ($i = $start + 1; $i -lt $lines.Count; $i++) {
        if ($lines[$i] -match '^##\s') { break }
        $body += $lines[$i]
    }
    return (($body -join "`n").Trim())
}

# ===================================================================================== main
if ($Version -notmatch '^\d+\.\d+\.\d+$') { Fail "-Version must be X.Y.Z, got '$Version'" }
$tag = "v$Version"
if (-not $DryRun -and -not $PublicRepo) { Fail '-PublicRepo <local clone of the public repository> is required (or use -DryRun)' }

$repo = Get-GitLine $PSScriptRoot @('rev-parse', '--show-toplevel')
Write-Step "private repository: $repo"

$dirty = (Invoke-Git -Dir $repo -GitArgs @('status', '--porcelain', '--untracked-files=no')).Out.Trim()
if ($dirty) { Fail "private tree has uncommitted changes, commit or stash them first:`n$dirty" }
$head = Get-GitLine $repo @('rev-parse', 'HEAD')

if (-not $Manifest) { $Manifest = Join-Path $repo 'saysdf/public/manifest.toml' }
$cfg = Read-Manifest $Manifest
$overlay = ''
if ($cfg.ContainsKey('export.overlay')) { $overlay = ([string]$cfg['export.overlay']).Trim('/') }
$excludeRe = @(); if ($cfg.ContainsKey('export.exclude')) { $excludeRe = @($cfg['export.exclude'] | ForEach-Object { ConvertTo-GlobRegex $_ }) }
$allowRe = @(); if ($cfg.ContainsKey('scan.cyrillic_allow')) { $allowRe = @($cfg['scan.cyrillic_allow'] | ForEach-Object { ConvertTo-GlobRegex $_ }) }
# Lines with this marker may contain Cyrillic (test data); shared with saysdf/i18n/tests/sources.rs
$allowMarker = ''; if ($cfg.ContainsKey('scan.cyrillic_allow_marker')) { $allowMarker = [string]$cfg['scan.cyrillic_allow_marker'] }
$maxMb = 5; if ($cfg.ContainsKey('scan.max_file_size_mb')) { $maxMb = [double]$cfg['scan.max_file_size_mb'] }
# References to private (excluded) paths in published text: one combined regex + allow-list globs
$privRefRe = $null
if ($cfg.ContainsKey('scan.private_refs') -and @($cfg['scan.private_refs']).Count -gt 0) {
    try { $privRefRe = New-Object System.Text.RegularExpressions.Regex ('(?:' + (@($cfg['scan.private_refs']) -join ')|(?:') + ')') }
    catch { Fail "manifest: bad regex in [scan] private_refs: $($_.Exception.Message)" }
}
$privAllowRe = @(); if ($cfg.ContainsKey('scan.private_refs_allow')) { $privAllowRe = @($cfg['scan.private_refs_allow'] | ForEach-Object { ConvertTo-GlobRegex $_ }) }
Write-Step "manifest: $Manifest ($($excludeRe.Count) excludes, overlay '$overlay')"

if (-not $DenyList) {
    if (-not $cfg.ContainsKey('scan.denylist')) { Fail 'no -DenyList and no [scan] denylist in the manifest' }
    $DenyList = [string]$cfg['scan.denylist']
}
$DenyList = Expand-HomePath $DenyList
if (-not (Test-Path -LiteralPath $DenyList -PathType Leaf)) {
    Fail "private deny-list not found: $DenyList`nCreate it (one regex per line, '#' comments) with markers that must never be published: private host names, local paths, names. It must stay outside the repository."
}
$denyRe = @()
$n = 0
foreach ($l in [IO.File]::ReadAllLines($DenyList, $utf8)) {
    $n++
    $t = $l.Trim()
    if (-not $t -or $t.StartsWith('#')) { continue }
    try { $denyRe += , @($n, (New-Object System.Text.RegularExpressions.Regex $t)) } catch { Fail "deny-list line ${n}: bad regex: $($_.Exception.Message)" }
}
Write-Step "deny-list: $DenyList ($($denyRe.Count) rules)"

$changelogText = (Invoke-Git -Dir $repo -GitArgs @('show', 'HEAD:saysdf/public/CHANGELOG.md')).Out
$notes = Get-ChangelogSection $changelogText $Version
if (-not $notes) { Fail "saysdf/public/CHANGELOG.md has no (or an empty) section '## $Version'" }
$message = "saysdfTerm $tag`n`n$notes`n"

$upRes = Invoke-Git -Dir $repo -GitArgs @('merge-base', 'HEAD', $UpstreamRef) -AllowFail
if ($upRes.Code -ne 0) { Fail "cannot find the upstream base: git merge-base HEAD $UpstreamRef failed. Is the 'upstream' remote fetched?" }
$upstream = $upRes.Out.Trim()
Write-Step "HEAD $head, upstream base $upstream"

# --- public clone checks and parents (design D4)
$branch = $null; $parents = @(); $needFetch = $false; $prevHead = $null
if ($PublicRepo) {
    $PublicRepo = [IO.Path]::GetFullPath($PublicRepo)
    if ((Invoke-Git -Dir $PublicRepo -GitArgs @('rev-parse', '--git-dir') -AllowFail).Code -ne 0) { Fail "not a git repository: $PublicRepo" }
    $pubTop = Get-GitLine $PublicRepo @('rev-parse', '--show-toplevel')
    if ([IO.Path]::GetFullPath($pubTop) -eq [IO.Path]::GetFullPath($repo)) { Fail '-PublicRepo points to the private repository' }
    $br = Invoke-Git -Dir $PublicRepo -GitArgs @('symbolic-ref', '--short', 'HEAD') -AllowFail
    if ($br.Code -ne 0) { Fail 'the public clone is in detached HEAD state; check out its main branch' }
    $branch = $br.Out.Trim()
    if ((Invoke-Git -Dir $PublicRepo -GitArgs @('rev-parse', '--verify', '--quiet', "refs/tags/$tag") -AllowFail).Code -eq 0) { Fail "tag $tag already exists in the public clone" }
    $ph = Invoke-Git -Dir $PublicRepo -GitArgs @('rev-parse', '--verify', '--quiet', 'HEAD') -AllowFail
    if ($ph.Code -eq 0) {
        $prevHead = $ph.Out.Trim()
        $pdirty = (Invoke-Git -Dir $PublicRepo -GitArgs @('status', '--porcelain', '--untracked-files=no')).Out.Trim()
        if ($pdirty) { Fail "the public clone has uncommitted changes:`n$pdirty" }
        $desc = Invoke-Git -Dir $PublicRepo -GitArgs @('describe', '--exact-match', '--tags', '--match', 'v*', 'HEAD') -AllowFail
        if ($desc.Code -ne 0) { Write-Host "warning: public HEAD $prevHead is not a release tag" -ForegroundColor Yellow }
        $parents = @($prevHead)
        $has = (Invoke-Git -Dir $PublicRepo -GitArgs @('cat-file', '-e', "$upstream^{commit}") -AllowFail).Code -eq 0
        $isAnc = $false
        if ($has) { $isAnc = (Invoke-Git -Dir $PublicRepo -GitArgs @('merge-base', '--is-ancestor', $upstream, $prevHead) -AllowFail).Code -eq 0 }
        if (-not $isAnc) { $parents += $upstream; $needFetch = -not $has }
    } else {
        $parents = @($upstream); $needFetch = $true
        $cnt = (Invoke-Git -Dir $PublicRepo -GitArgs @('rev-list', '--all', '--count') -AllowFail).Out.Trim()
        if ($cnt -and $cnt -ne '0') { Fail 'the public clone has no HEAD but has other refs; use a fresh clone' }
    }
    Write-Step "public clone: $PublicRepo, branch $branch, parents: $($parents -join ', ')"
}

$tmp = Join-Path ([IO.Path]::GetTempPath()) ('saysdfTerm-release-' + [guid]::NewGuid().ToString('N').Substring(0, 10))
$treeDir = Join-Path $tmp 'tree'
$ok = $false
try {
    # --- 1. export (git archive to a file + tar; never through the PowerShell pipeline)
    Write-Step "export HEAD to $treeDir"
    New-Item -ItemType Directory -Path $treeDir | Out-Null
    $tarFile = Join-Path $tmp 'export.tar'
    Invoke-Git -Dir $repo -GitArgs @('-c', 'core.autocrlf=false', '-c', 'core.eol=lf', 'archive', '--format=tar', '-o', $tarFile, 'HEAD') | Out-Null
    $tarExe = 'tar'
    if ($env:SystemRoot -and (Test-Path (Join-Path $env:SystemRoot 'System32\tar.exe'))) { $tarExe = Join-Path $env:SystemRoot 'System32\tar.exe' }
    & $tarExe -xf $tarFile -C $treeDir
    if ($LASTEXITCODE -ne 0) { Fail "tar -xf failed ($LASTEXITCODE)" }
    Remove-Item -LiteralPath $tarFile

    # --- 2. excludes and overlay
    $all = Get-TreeEntries $repo 'HEAD'
    $final = New-Object System.Collections.Hashtable ([StringComparer]::Ordinal)            # public path -> entry
    $excluded = New-Object System.Collections.ArrayList
    $overlayEntries = New-Object System.Collections.ArrayList
    $ovPrefix = if ($overlay) { "$overlay/" } else { $null }
    foreach ($e in $all) {
        if ($ovPrefix -and $e.Path.StartsWith($ovPrefix)) { [void]$overlayEntries.Add($e); continue }
        if (Test-AnyMatch $excludeRe $e.Path) { [void]$excluded.Add($e.Path); continue }
        $final[$e.Path] = $e
    }
    foreach ($p in $excluded) {
        $f = Join-Path $treeDir $p
        if (Test-Path -LiteralPath $f -PathType Leaf) { Remove-Item -LiteralPath $f -Force }
    }
    $overlayPaths = New-Object System.Collections.Hashtable ([StringComparer]::Ordinal)
    foreach ($e in $overlayEntries) {
        $dest = $e.Path.Substring($ovPrefix.Length)
        $destFile = Join-Path $treeDir $dest
        $destParent = Split-Path -Parent $destFile
        if (-not (Test-Path -LiteralPath $destParent)) { New-Item -ItemType Directory -Path $destParent | Out-Null }
        Copy-Item -LiteralPath (Join-Path $treeDir $e.Path) -Destination $destFile -Force
        $final[$dest] = [pscustomobject]@{ Mode = $e.Mode; Type = $e.Type; Sha = $e.Sha; Path = $dest; Src = $null }
        $overlayPaths[$dest] = $true
    }
    if ($overlay -and (Test-Path -LiteralPath (Join-Path $treeDir $overlay))) { Remove-Item -LiteralPath (Join-Path $treeDir $overlay) -Recurse -Force }
    if ($overlay -and $overlayEntries.Count -eq 0) { Write-Host "warning: overlay '$overlay' is empty" -ForegroundColor Yellow }
    # empty directories left by excludes
    Get-ChildItem -LiteralPath $treeDir -Recurse -Directory | Sort-Object { $_.FullName.Length } -Descending | ForEach-Object {
        if (-not (Get-ChildItem -LiteralPath $_.FullName -Force)) { Remove-Item -LiteralPath $_.FullName -Force }
    }
    $paths = @($final.Keys | Sort-Object -CaseSensitive)
    Write-Step ("public tree: {0} entries, {1} excluded, {2} from the overlay" -f $paths.Count, $excluded.Count, $overlayEntries.Count)

    # --- 3. leak scan (design D3)
    Write-Step 'leak scan'
    $upstreamSha = New-Object System.Collections.Hashtable ([StringComparer]::Ordinal)
    foreach ($e in (Get-TreeEntries $repo $upstream)) { $upstreamSha[$e.Path] = $e.Sha }
    $hits = New-Object System.Collections.ArrayList
    $cyr = New-Object System.Text.RegularExpressions.Regex '[\u0400-\u04FF]{2,}'
    $keyRe = New-Object System.Text.RegularExpressions.Regex 'BEGIN [A-Z0-9 ]*PRIVATE KEY'
    $envRe = New-Object System.Text.RegularExpressions.Regex '(^|/)\.env(\.[^/]*)?$'
    $maxBytes = [long]($maxMb * 1MB)
    $scanned = 0
    foreach ($p in $paths) {
        $e = $final[$p]
        if ($p.StartsWith('.github/workflows/') -or $p.Contains('/.github/workflows/')) { [void]$hits.Add("${p}: GitHub workflow (no CI on GitHub)") }
        if ($envRe.IsMatch($p)) { [void]$hits.Add("${p}: .env file") }
        if ($e.Type -ne 'blob') { continue }
        $file = Join-Path $treeDir $p
        $changed = $overlayPaths.ContainsKey($p) -or (-not $upstreamSha.ContainsKey($p)) -or ($upstreamSha[$p] -ne $e.Sha)
        $size = (Get-Item -LiteralPath $file).Length
        if ($changed -and $size -gt $maxBytes) { [void]$hits.Add(("{0}: {1:N1} MB, over the {2} MB limit" -f $p, ($size / 1MB), $maxMb)); continue }
        if ($size -gt $maxBytes) { continue }   # unchanged large upstream file: name checks only
        $bytes = [IO.File]::ReadAllBytes($file)
        $probe = [Math]::Min($bytes.Length, 8000)
        $binary = $false
        for ($i = 0; $i -lt $probe; $i++) { if ($bytes[$i] -eq 0) { $binary = $true; break } }
        if ($binary) { continue }
        $text = $utf8.GetString($bytes)
        $lines = $null
        if ($keyRe.IsMatch($text)) {
            $lines = $text.Split("`n")
            for ($i = 0; $i -lt $lines.Count; $i++) { if ($keyRe.IsMatch($lines[$i])) { [void]$hits.Add("${p}:$($i + 1): private key block") } }
        }
        if (-not $changed) { continue }
        $scanned++
        if (-not (Test-AnyMatch $allowRe $p) -and $cyr.IsMatch($text)) {
            if (-not $lines) { $lines = $text.Split("`n") }
            $found = 0
            for ($i = 0; $i -lt $lines.Count; $i++) {
                if ($cyr.IsMatch($lines[$i]) -and -not ($allowMarker -and $lines[$i].Contains($allowMarker))) {
                    $found++
                    if ($found -le 3) { [void]$hits.Add("${p}:$($i + 1): Cyrillic text") }
                }
            }
            if ($found -gt 3) { [void]$hits.Add("${p}: ... $($found - 3) more lines with Cyrillic text") }
        }
        if ($privRefRe -and -not (Test-AnyMatch $privAllowRe $p) -and $privRefRe.IsMatch($text)) {
            if (-not $lines) { $lines = $text.Split("`n") }
            for ($i = 0; $i -lt $lines.Count; $i++) {
                $m = $privRefRe.Match($lines[$i])
                if ($m.Success) { [void]$hits.Add("${p}:$($i + 1): reference to the unpublished path '$($m.Value)'") }
            }
        }
        foreach ($d in $denyRe) {
            if ($d[1].IsMatch($text)) {
                if (-not $lines) { $lines = $text.Split("`n") }
                for ($i = 0; $i -lt $lines.Count; $i++) { if ($d[1].IsMatch($lines[$i])) { [void]$hits.Add("${p}:$($i + 1): private marker (deny-list line $($d[0]))") } }
            }
        }
    }
    Write-Step "scanned $scanned changed text files"
    if ($hits.Count -gt 0) {
        Write-Host "Leak scan found $($hits.Count) problem(s):" -ForegroundColor Red
        $hits | ForEach-Object { Write-Host "  $_" }
        $byFile = @{}
        foreach ($h in $hits) { $byFile[($h -split ':')[0]] = $true }
        Write-Host "Files: $($byFile.Count). Fix them in the private repository (or extend the manifest allow-list / excludes) and run again." -ForegroundColor Red
        Fail 'leak scan failed, nothing was committed'
    }
    Write-Host 'leak scan: clean' -ForegroundColor Green

    # --- 4. blobs: hash the exported files; every id must equal the private blob id
    $blobPaths = @($paths | Where-Object { $final[$_].Type -eq 'blob' })
    $writeDir = $repo; $hashArgs = @('hash-object', '--no-filters', '--stdin-paths')
    if ($PublicRepo -and -not $DryRun) { $writeDir = $PublicRepo; $hashArgs = @('hash-object', '-w', '--no-filters', '--stdin-paths') }
    Write-Step ("verify {0} blobs{1}" -f $blobPaths.Count, $(if ($writeDir -eq $PublicRepo) { ' and write them into the public clone' } else { '' }))
    $sb = New-Object System.Text.StringBuilder
    foreach ($p in $blobPaths) { [void]$sb.Append(((Join-Path $treeDir $p) -replace '\\', '/')).Append("`n") }
    $ids = (Invoke-Git -Dir $writeDir -GitArgs $hashArgs -InputBytes ($utf8.GetBytes($sb.ToString()))).Out.Split([char[]]@([char]10), [StringSplitOptions]::RemoveEmptyEntries)
    if ($ids.Count -ne $blobPaths.Count) { Fail "hash-object returned $($ids.Count) ids for $($blobPaths.Count) files" }
    $bad = @()
    for ($i = 0; $i -lt $blobPaths.Count; $i++) { if ($ids[$i].Trim() -ne $final[$blobPaths[$i]].Sha) { $bad += $blobPaths[$i] } }
    if ($bad.Count -gt 0) { Fail ("exported content differs from the private blobs (line endings or corruption):`n  " + (($bad | Select-Object -First 20) -join "`n  ")) }
    Write-Host 'blobs: identical to the private tree' -ForegroundColor Green

    $msgFile = Join-Path $tmp 'message.txt'
    [IO.File]::WriteAllText($msgFile, $message, $utf8)

    if ($DryRun -or -not $PublicRepo) {
        Write-Host ''
        Write-Host '--- dry run: nothing written ---' -ForegroundColor Yellow
        Write-Host "release:   $tag from private $head"
        Write-Host "excluded:  $($excluded.Count) files, e.g. $((($excluded | ForEach-Object { ($_ -split '/')[0] } | Sort-Object -Unique) | Select-Object -First 12) -join ', ')"
        Write-Host "overlay:   $((@($overlayPaths.Keys) | Sort-Object) -join ', ')"
        if ($PublicRepo) {
            Write-Host "public:    $PublicRepo, branch $branch"
            Write-Host "parents:   $($parents -join ' ')"
            if ($needFetch) { Write-Host "would fetch upstream commit $upstream (and its WezTerm history) from $repo" }
        }
        Write-Host 'message:'
        $message.TrimEnd().Split("`n") | ForEach-Object { Write-Host "  | $_" }
        $ok = $true
        return
    }

    # --- 5. commit and tag in the public clone
    if ($needFetch) {
        Write-Step "fetch upstream commit $upstream (WezTerm history) from the private repository"
        Invoke-Git -Dir $PublicRepo -GitArgs @('fetch', '--no-tags', '--quiet', $repo, $upstream) | Out-Null
    }
    $idx = Join-Path $tmp 'index'
    $env1 = @{ GIT_INDEX_FILE = $idx }
    $info = New-Object System.Text.StringBuilder
    foreach ($p in $paths) { $e = $final[$p]; [void]$info.Append("$($e.Mode) $($e.Type) $($e.Sha)`t$p").Append([char]0) }
    Invoke-Git -Dir $PublicRepo -GitArgs @('update-index', '-z', '--index-info') -InputBytes ($utf8.GetBytes($info.ToString())) -Environment $env1 | Out-Null
    $tree = (Invoke-Git -Dir $PublicRepo -GitArgs @('write-tree') -Environment $env1).Out.Trim()
    $check = (Invoke-Git -Dir $PublicRepo -GitArgs @('ls-tree', '-r', '-z', '--name-only', $tree)).Out.Split([char[]]@([char]0), [StringSplitOptions]::RemoveEmptyEntries)
    if ($check.Count -ne $paths.Count) { Fail "tree has $($check.Count) entries, expected $($paths.Count)" }
    foreach ($c in $check) { if (-not $final.ContainsKey($c)) { Fail "unexpected path in the built tree: '$c'" } }
    $ctArgs = @('commit-tree', $tree)
    foreach ($par in $parents) { $ctArgs += @('-p', $par) }
    $ctArgs += @('-F', $msgFile)
    $commit = (Invoke-Git -Dir $PublicRepo -GitArgs $ctArgs).Out.Trim()
    $old = if ($prevHead) { $prevHead } else { '0000000000000000000000000000000000000000' }
    Invoke-Git -Dir $PublicRepo -GitArgs @('update-ref', '-m', "release $tag", "refs/heads/$branch", $commit, $old) | Out-Null
    Write-Step 'update the public working tree'
    Invoke-Git -Dir $PublicRepo -GitArgs @('reset', '--hard', '--quiet', $commit) | Out-Null
    Invoke-Git -Dir $PublicRepo -GitArgs @('tag', '-a', $tag, '-F', $msgFile, $commit) | Out-Null

    Write-Host ''
    Write-Host "Released $tag -> $commit (tree $tree, parents $($parents -join ' '))" -ForegroundColor Green
    Write-Host 'Nothing was pushed. Review, then push:'
    Write-Host "  git -C `"$PublicRepo`" log --oneline -3 $branch"
    Write-Host "  git -C `"$PublicRepo`" push origin $branch"
    Write-Host "  git -C `"$PublicRepo`" push origin $tag"
    Write-Host 'Mark the released private commit (optional):'
    Write-Host "  git -C `"$repo`" tag public/$tag $head"
    $ok = $true
} finally {
    if ($KeepTemp) {
        if (Test-Path -LiteralPath $tmp) { Write-Host "exported tree kept in $treeDir" -ForegroundColor Yellow }
    } elseif (Test-Path -LiteralPath $tmp) {
        Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
    }
}
