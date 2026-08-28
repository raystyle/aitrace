# Conservation-law check for the aitrace five-principle loop (axiom 2):
# every round must PRODUCE >= 1 TODO and CONSUME >= 1 TODO, each with a DoD.
#
# Per-round accounting uses the git diff of today's timeline memory file:
# run this BEFORE committing the phase-7 memory update -- the uncommitted
# delta IS this round. Checkbox lines added since HEAD are counted as this
# round's produce ([ ]) and consume ([x]); a clean tree means nothing to
# measure (the round was already accounted for at its own commit).
#
# Usage: powershell -NoProfile -File .claude/scripts/conservation-check.ps1 [-MemoryPath <file>]
# Exit codes: 0 = OK / nothing to measure, 2 = conservation violation.

param(
    [string]$MemoryPath = "",
    # Closing round: the pool holds only items that cannot be advanced right
    # now (human-only / cross-session / design rounds), so producing a new
    # TODO would be Goodhart filler. Documented in CLAUDE.md axiom 2.
    [switch]$AllowNoProduce
)

if (-not $MemoryPath) {
    $today = Get-Date -Format 'yyyy-MM-dd'
    $MemoryPath = Join-Path $PSScriptRoot "..\..\docs\timeline\$today.md"
}
$MemoryPath = [IO.Path]::GetFullPath($MemoryPath)

if (-not (Test-Path $MemoryPath)) {
    Write-Output "conservation: no memory file at $MemoryPath (fresh day: nothing to check yet)"
    exit 0
}

# Round delta = uncommitted changes to the memory file (run before commit).
# Git emits UTF-8; Windows PowerShell's default console encoding would
# otherwise smash checkbox lines so [x]/[ ] never match.
$prevEnc = [Console]::OutputEncoding
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
try {
    $diff = git --no-pager -c core.quotepath=false -c i18n.logOutputEncoding=utf-8 diff HEAD -- $MemoryPath 2>$null
} finally {
    [Console]::OutputEncoding = $prevEnc
}
if (-not $diff) {
    # Untracked file (first round of the day): the whole file is the delta.
    if (git ls-files --error-unmatch $MemoryPath 2>$null) {
        Write-Output "conservation: clean tree -- round already accounted at its commit"
        exit 0
    }
    $diff = Get-Content $MemoryPath -Encoding UTF8 | ForEach-Object { "+$_" }
}

$added = $diff | Where-Object { $_ -match '^\+' -and $_ -notmatch '^\+\+\+' }
$thisRoundClosed = ($added | Where-Object { $_ -match '^\+\s*-\s*\[x\]' }).Count
$thisRoundOpen = ($added | Where-Object { $_ -match '^\+\s*-\s*\[ \]' }).Count
# A TODO produced AND closed within the same round lands as a pure-added
# [x] line, so the diff's produced count would miss it. Heuristic: closed
# lines beyond the open lines removed from HEAD are same-round items and
# count as produced as well.
$removedOpen = ($diff | Where-Object { $_ -match '^-\s*-\s*\[ \]' -and $_ -notmatch '^---' }).Count
$sameRound = [Math]::Max(0, $thisRoundClosed - $removedOpen)
$thisRoundProduced = $thisRoundOpen + $sameRound

# Day totals for context.
$content = Get-Content $MemoryPath -Raw -Encoding UTF8
$dayClosed = ([regex]::Matches($content, '(?m)^-\s*\[x\]')).Count
$dayOpen = ([regex]::Matches($content, '(?m)^-\s*\[ \]')).Count

Write-Output "conservation: this round closed=$thisRoundClosed produced=$thisRoundProduced (open-added=$thisRoundOpen same-round-closed=$sameRound; day totals: closed=$dayClosed open=$dayOpen)"

if ($thisRoundClosed -eq 0 -and $thisRoundProduced -eq 0) {
    Write-Output "conservation: no checkbox changes in the round delta -- if work happened, record it in the memory file first"
    exit 0
}
if ($thisRoundClosed -eq 0) {
    [Console]::Error.WriteLine("conservation violation: consumed=0 this round -- close or advance at least one TODO before ending the round")
    exit 2
}
if ($thisRoundProduced -eq 0) {
    if ($AllowNoProduce) {
        Write-Output "conservation: closing round (produced=0 allowed by axiom 2 amendment)"
        exit 0
    }
    [Console]::Error.WriteLine("conservation violation: produced=0 this round -- record a new TODO with DoD (or write 'why nothing was found' as one, or pass -AllowNoProduce for a documented closing round)")
    exit 2
}
exit 0
