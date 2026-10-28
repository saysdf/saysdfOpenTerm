# saysdfTerminal: shell integration for Windows PowerShell 5.1 and PowerShell 7+ (pwsh).
# Emits OSC 133 (A/B/C/D;<exit>), OSC 7 (cwd) and OSC 1337 SetUserVar
# (WEZTERM_PROG, SAYSDF_LAST_EXIT, SAYSDF_LAST_DURATION_MS).
# Wraps the existing `prompt` and `PSConsoleHostReadLine` functions; never replaces the user's
# prompt and never touches profile files. Safe to dot-source several times.
#
# Disable (any non-empty value): SAYSDFTERM_SHELL_SKIP_ALL | _SEMANTIC_ZONES | _CWD | _USER_VARS
# and the upstream WEZTERM_SHELL_SKIP_* equivalents.
# Multiline input (change multiline-input): Alt+Enter (ESC CR, sent by saysdfTerm for Shift+Enter
# and Ctrl+Enter) -> PSReadLine AddLine, unless Alt+Enter is already bound. Skipped when
# SAYSDFTERM_MULTILINE=0, or inside saysdfTerm with the saysdf.multiline module disabled.
# Command hints (change command-hints): Register-ArgumentCompleter -Native with `saysdf hints complete`
# for git/cargo/docker/kubectl/npm (not over posh-git or an existing native completer unless
# SAYSDF_HINTS_OVERRIDE=1), Tab -> MenuComplete over the default binding (SAYSDFTERM_HINTS_MENU=0 keeps Tab),
# PSReadLine PredictionSource History[AndPlugin] when it is None (SAYSDFTERM_HINTS_INLINE=0 skips).
# Off: SAYSDFTERM_HINTS=0, or inside saysdfTerm with saysdf.hints disabled; per command:
# SAYSDF_HINTS_DISABLED=kubectl,npm ('*' = all). Binary: SAYSDFTERM_HINTS_BIN or saysdf in PATH.
# Prompt suggest (change prompt-suggest): SAYSDFTERM_SUGGEST=1 -> user vars SAYSDF_HISTFILE and
# SAYSDF_PATH at the prompt, only when changed. Not emitted otherwise.

if ($global:__SaysdfTermShellIntegration) { return }
if ($env:SAYSDFTERM_SHELL_SKIP_ALL -or $env:WEZTERM_SHELL_SKIP_ALL) { return }

try {
    $global:__SaysdfTermShellIntegration = $true
    $global:__SaysdfTermOrigPrompt = $function:prompt
    $global:__SaysdfTermOrigReadLine = $null
    $global:__SaysdfTermPromptCount = 0
    $global:__SaysdfTermCmdStart = $null     # set by the ReadLine wrapper for a non-empty line
    $global:__SaysdfTermLastHistoryId = -1

    function global:__SaysdfTermSkip([string]$Part) {
        [bool]((Get-Item -LiteralPath "Env:SAYSDFTERM_SHELL_SKIP_$Part" -ErrorAction Ignore).Value -or
               (Get-Item -LiteralPath "Env:WEZTERM_SHELL_SKIP_$Part" -ErrorAction Ignore).Value)
    }

    function global:__SaysdfTermOsc([string]$Body) {
        "$([char]27)]$Body$([char]7)"
    }

    function global:__SaysdfTermUserVar([string]$Name, [string]$Value) {
        if (__SaysdfTermSkip 'USER_VARS') { return '' }
        $b64 = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($Value))
        __SaysdfTermOsc "1337;SetUserVar=$Name=$b64"
    }

    # SAYSDFTERM_SUGGEST=1 (change prompt-suggest): SAYSDF_HISTFILE / SAYSDF_PATH only when changed.
    function global:__SaysdfTermSuggest {
        if ($env:SAYSDFTERM_SUGGEST -ne '1') { return '' }
        $out = ''
        $hist = ''
        try { $hist = [string](Get-PSReadLineOption -ErrorAction Stop).HistorySavePath } catch { $hist = '' }
        if ($null -eq $global:__SaysdfTermSentHistFile -or $hist -cne $global:__SaysdfTermSentHistFile) {
            $global:__SaysdfTermSentHistFile = $hist
            $out += __SaysdfTermUserVar 'SAYSDF_HISTFILE' $hist
        }
        $path = [string]$env:PATH
        if ($null -eq $global:__SaysdfTermSentPath -or $path -cne $global:__SaysdfTermSentPath) {
            $global:__SaysdfTermSentPath = $path
            $out += __SaysdfTermUserVar 'SAYSDF_PATH' $path
        }
        $out
    }

    function global:__SaysdfTermCwd {
        if (__SaysdfTermSkip 'CWD') { return '' }
        $loc = $ExecutionContext.SessionState.Path.CurrentLocation
        if ($loc.Provider.Name -ne 'FileSystem') { return '' }
        $p = $loc.ProviderPath
        if (-not $p) { return '' }
        $hostName = $env:COMPUTERNAME
        if ($p.StartsWith('\\')) {
            # UNC: \\server\share\dir -> file://server/share/dir
            $parts = $p.Substring(2).Split('\') | Where-Object { $_ -ne '' }
            if (-not $parts) { return '' }
            $hostName = $parts[0]
            $segs = @($parts | Select-Object -Skip 1)
            $path = '/' + (($segs | ForEach-Object { [Uri]::EscapeDataString($_) }) -join '/')
        } else {
            $segs = $p.Replace('\', '/').Split('/') | Where-Object { $_ -ne '' }
            # Keep the drive letter colon unescaped: /C:/Program%20Files
            $enc = for ($i = 0; $i -lt $segs.Count; $i++) {
                if ($i -eq 0 -and $segs[0] -match '^[A-Za-z]:$') { $segs[0] } else { [Uri]::EscapeDataString($segs[$i]) }
            }
            $path = '/' + ($enc -join '/')
            if ($segs.Count -eq 1) { $path += '/' }
        }
        "$([char]27)]7;file://$hostName$path$([char]27)\"
    }

    function global:__SaysdfTermWrapReadLine {
        if ($global:__SaysdfTermOrigReadLine) { return }
        $orig = $function:PSConsoleHostReadLine
        if (-not $orig) { return }
        $global:__SaysdfTermOrigReadLine = $orig
        function global:PSConsoleHostReadLine {
            $__sdfOk = $global:?
            if (-not $__sdfOk) { Write-Error 'saysdfterm' -ErrorAction Ignore }
            $__sdfLine = & $global:__SaysdfTermOrigReadLine
            try {
                if ($__sdfLine -and $__sdfLine.Trim()) {
                    $global:__SaysdfTermCmdStart = [DateTime]::UtcNow
                    $__sdfOut = ''
                    if (-not (__SaysdfTermSkip 'SEMANTIC_ZONES')) { $__sdfOut += __SaysdfTermOsc '133;C' }
                    $__sdfOut += __SaysdfTermUserVar 'WEZTERM_PROG' $__sdfLine
                    if ($__sdfOut) { [Console]::Write($__sdfOut) }
                }
            } catch { }
            $__sdfLine
        }
    }

    function global:__SaysdfTermMultiline {
        if ($global:__SaysdfTermMultilineDone) { return }
        $m = $env:SAYSDFTERM_MULTILINE
        if ($m -eq '0' -or (-not $m -and $env:SAYSDFTERM_SHELL_INTEGRATION)) {
            $global:__SaysdfTermMultilineDone = $true; return
        }
        if (-not (Get-Module PSReadLine)) { return }   # may load later; retried from prompt
        $global:__SaysdfTermMultilineDone = $true
        $bound = Get-PSReadLineKeyHandler -Bound -ErrorAction Ignore | Where-Object { $_.Key -eq 'Alt+Enter' }
        if (-not $bound) { Set-PSReadLineKeyHandler -Chord 'Alt+Enter' -Function AddLine }
    }

    # Command hints (change command-hints): `saysdf hints complete` as the native completer for
    # spec-covered commands, PSReadLine MenuComplete on Tab and the grey history suggestion.
    function global:__SaysdfTermHintsOn {
        $h = $env:SAYSDFTERM_HINTS
        if ($h -eq '0' -or (-not $h -and $env:SAYSDFTERM_SHELL_INTEGRATION)) { return $false }
        if ($env:SAYSDF_HINTS_DISABLED -eq '*' -or $env:SAYSDF_HINTS_DISABLED -eq '1') { return $false }
        return $true
    }

    # Best effort: is a native completer already registered for $Name (by the user or a module)?
    function global:__SaysdfTermHasNativeCompleter([string]$Name) {
        try {
            $ctx = $ExecutionContext.GetType().GetField('_context', 'NonPublic,Instance').GetValue($ExecutionContext)
            $tbl = $ctx.GetType().GetProperty('NativeArgumentCompleters', 'NonPublic,Instance').GetValue($ctx)
            return [bool]($tbl -and ($tbl.ContainsKey($Name) -or $tbl.ContainsKey("$Name.exe")))
        } catch { return $false }
    }

    $global:__SaysdfTermHintsCompleter = {
        param($wordToComplete, $commandAst, $cursorPosition)
        $oldLine = $env:SAYSDF_HINTS_LINE
        $oldEnc = $null
        $lines = @()
        try {
            $start = $commandAst.Extent.StartOffset
            $text = $commandAst.Extent.Text
            $n = [Math]::Max(0, $cursorPosition - $start)
            $line = $text.Substring(0, [Math]::Min($text.Length, $n))
            if ($n -gt $text.Length) { $line += ' ' }
            $env:SAYSDF_HINTS_LINE = $line
            $oldEnc = [Console]::OutputEncoding
            [Console]::OutputEncoding = [Text.Encoding]::UTF8
            $lines = @(& $global:__SaysdfTermHintsBin hints complete --shell pwsh --from-env 2>$null)
        } catch { } finally {
            if ($null -ne $oldEnc) { try { [Console]::OutputEncoding = $oldEnc } catch { } }
            $env:SAYSDF_HINTS_LINE = $oldLine
        }
        foreach ($l in $lines) {
            $p = "$l" -split "`t", 3
            if ($p.Count -lt 3) { continue }
            $type = if ($p[1].StartsWith('-')) { 'ParameterName' } else { 'ParameterValue' }
            [System.Management.Automation.CompletionResult]::new($p[0], $p[1], $type, $p[2])
        }
    }

    function global:__SaysdfTermHints {
        if ($global:__SaysdfTermHintsDone) { return }
        $global:__SaysdfTermHintsDone = $true
        if (-not (__SaysdfTermHintsOn)) { return }
        $bin = $env:SAYSDFTERM_HINTS_BIN
        if (-not $bin) {
            $c = Get-Command saysdf -CommandType Application -ErrorAction Ignore | Select-Object -First 1
            if ($c) { $bin = $c.Source }
        }
        if (-not $bin) { return }
        $global:__SaysdfTermHintsBin = $bin
        $cmds = @('git', 'cargo', 'docker', 'kubectl', 'npm')
        if ($env:SAYSDFTERM_HINTS_COMMANDS) { $cmds = @($env:SAYSDFTERM_HINTS_COMMANDS -split '[,;\s]+' | Where-Object { $_ }) }
        $off = @("$env:SAYSDF_HINTS_DISABLED" -split '[,;\s]+' | Where-Object { $_ })
        $override = $env:SAYSDF_HINTS_OVERRIDE -eq '1'
        $active = @()
        foreach ($c in $cmds) {
            if ($off -contains $c) { continue }
            if (-not $override) {
                # user/module completers win (posh-git for git, carapace, Register-ArgumentCompleter)
                if ($c -eq 'git' -and (Get-Module posh-git)) { continue }
                if (__SaysdfTermHasNativeCompleter $c) { continue }
            }
            Register-ArgumentCompleter -Native -CommandName @($c, "$c.exe") -ScriptBlock $global:__SaysdfTermHintsCompleter
            $active += $c
        }
        $env:SAYSDF_HINTS_ACTIVE = $active -join ','
        $env:SAYSDF_HINTS_SHELL = if ($PSVersionTable.PSEdition -eq 'Core') { 'pwsh' } else { 'powershell' }
    }

    function global:__SaysdfTermHintsInline {
        if ($global:__SaysdfTermHintsInlineDone) { return }
        if (-not (__SaysdfTermHintsOn)) { $global:__SaysdfTermHintsInlineDone = $true; return }
        $m = Get-Module PSReadLine
        if (-not $m) { return }   # may load later; retried from prompt
        $global:__SaysdfTermHintsInlineDone = $true
        if ($env:SAYSDFTERM_HINTS_MENU -ne '0') {
            # Tab -> MenuComplete (list with descriptions), only over the default binding
            $tab = Get-PSReadLineKeyHandler -Bound -ErrorAction Ignore | Where-Object { $_.Key -eq 'Tab' } | Select-Object -First 1
            if (-not $tab -or $tab.Function -in @('TabCompleteNext', 'Complete')) {
                Set-PSReadLineKeyHandler -Chord 'Tab' -Function MenuComplete
            }
        }
        if ($env:SAYSDFTERM_HINTS_INLINE -eq '0') { $env:SAYSDF_HINTS_INLINE = 'off (SAYSDFTERM_HINTS_INLINE=0)'; return }
        $opt = Get-PSReadLineOption
        if (-not ($opt.PSObject.Properties.Name -contains 'PredictionSource')) {
            $env:SAYSDF_HINTS_INLINE = "unavailable: PSReadLine $($m.Version) has no predictions (needs 2.1+)"
            # quiet at shell start: the reason is shown only by 'saysdf hints doctor'
            return
        }
        if ("$($opt.PredictionSource)" -eq 'None') {
            $src = 'History'
            if ($PSVersionTable.PSVersion -ge [version]'7.2' -and $m.Version -ge [version]'2.2') { $src = 'HistoryAndPlugin' }
            try { Set-PSReadLineOption -PredictionSource $src } catch {
                $env:SAYSDF_HINTS_INLINE = "unavailable: $($_.Exception.Message)"; return
            }
        }
        $o = Get-PSReadLineOption
        $env:SAYSDF_HINTS_INLINE = "PSReadLine $($o.PredictionSource) / $($o.PredictionViewStyle) (F2 toggles ListView)"
    }

    function global:prompt {
        # Capture status first: nothing may run before these two lines.
        $__sdfOk = $global:?
        $__sdfHadLec = Test-Path -LiteralPath 'Variable:global:LASTEXITCODE'
        $__sdfLec = $global:LASTEXITCODE

        $__sdfPre = ''
        $__sdfPost = ''
        try {
            __SaysdfTermWrapReadLine   # PSReadLine may load after us (e.g. from the profile)
            __SaysdfTermMultiline
            __SaysdfTermHintsInline

            # Did a command run since the previous prompt?
            $__sdfRan = $false
            $__sdfDur = $null
            $__sdfHist = Get-History -Count 1 -ErrorAction Ignore
            if ($global:__SaysdfTermOrigReadLine) {
                $__sdfRan = [bool]$global:__SaysdfTermCmdStart
            } elseif ($__sdfHist) {
                $__sdfRan = $__sdfHist.Id -ne $global:__SaysdfTermLastHistoryId -and $global:__SaysdfTermPromptCount -gt 0
            } else {
                # No history (non-interactive -Command): every prompt after the first closes a command.
                $__sdfRan = $global:__SaysdfTermPromptCount -gt 0
            }
            if ($__sdfRan) {
                if ($__sdfHist -and $__sdfHist.Id -ne $global:__SaysdfTermLastHistoryId) {
                    $__sdfDur = [long]($__sdfHist.EndExecutionTime - $__sdfHist.StartExecutionTime).TotalMilliseconds
                } elseif ($global:__SaysdfTermCmdStart) {
                    $__sdfDur = [long]([DateTime]::UtcNow - $global:__SaysdfTermCmdStart).TotalMilliseconds
                }
            }
            if ($__sdfHist) { $global:__SaysdfTermLastHistoryId = $__sdfHist.Id }
            $global:__SaysdfTermCmdStart = $null
            $global:__SaysdfTermPromptCount++

            $__sdfCode = 0
            if (-not $__sdfOk) {
                if ($__sdfHadLec -and $__sdfLec -is [int] -and $__sdfLec -ne 0) { $__sdfCode = $__sdfLec } else { $__sdfCode = 1 }
            }

            $__sdfZones = -not (__SaysdfTermSkip 'SEMANTIC_ZONES')
            if ($__sdfRan) {
                if ($__sdfZones) { $__sdfPre += __SaysdfTermOsc "133;D;$__sdfCode" }
                $__sdfPre += __SaysdfTermUserVar 'SAYSDF_LAST_EXIT' "$__sdfCode"
                if ($null -ne $__sdfDur) { $__sdfPre += __SaysdfTermUserVar 'SAYSDF_LAST_DURATION_MS' "$__sdfDur" }
            }
            $__sdfPre += __SaysdfTermUserVar 'WEZTERM_PROG' ''
            if ($__sdfZones) { $__sdfPre += __SaysdfTermOsc '133;A' }
            $__sdfPre += __SaysdfTermCwd
            $__sdfPre += __SaysdfTermSuggest
            if ($__sdfZones) { $__sdfPost = __SaysdfTermOsc '133;B' }
        } catch { }

        # Restore $? and $LASTEXITCODE for the user's prompt.
        if ($__sdfHadLec) { $global:LASTEXITCODE = $__sdfLec }
        if (-not $__sdfOk) { Write-Error 'saysdfterm' -ErrorAction Ignore }
        $__sdfUser = (& $global:__SaysdfTermOrigPrompt) -join ''
        if ($__sdfHadLec) { $global:LASTEXITCODE = $__sdfLec }

        "$__sdfPre$__sdfUser$__sdfPost"
    }

    __SaysdfTermWrapReadLine
    try { __SaysdfTermMultiline } catch { }
    try { __SaysdfTermHints } catch { }
    try { __SaysdfTermHintsInline } catch { }
} catch {
    # Never break the shell: leave the user's prompt as it was.
    if ($global:__SaysdfTermOrigPrompt) { Set-Item -Path Function:global:prompt -Value $global:__SaysdfTermOrigPrompt }
    $global:__SaysdfTermShellIntegration = $false
}
