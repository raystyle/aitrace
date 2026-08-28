# Conservation-law check for the aitrace five-principle loop (axiom 2):
# every round must PRODUCE >= 1 TODO and CONSUME >= 1 TODO, and each TODO
# carries a DoD. This script counts checkbox states in today's timeline
# memory file so phase 7 can verify the consumption half mechanically.
#
# Usage: powershell -NoProfile -File .claude/scripts/conservation-check.ps1 [-MemoryPath <file>]
# Exit codes: 0 = OK, 2 = nothing consumed today (conservation violation).

param([string]$MemoryPath = "")

if (-not $MemoryPath) {
    $today = Get-Date -Format 'yyyy-MM-dd'
    $MemoryPath = Join-Path $PSScriptRoot "..\..\docs\timeline\$today.md"
}
$MemoryPath = [IO.Path]::GetFullPath($MemoryPath)

if (-not (Test-Path $MemoryPath)) {
    Write-Output "conservation: no memory file at $MemoryPath (fresh day: nothing to check yet)"
    exit 0
}

$content = Get-Content $MemoryPath -Raw -Encoding UTF8
$closed = ([regex]::Matches($content, '(?m)^-\s*\[x\]')).Count
$open = ([regex]::Matches($content, '(?m)^-\s*\[ \]')).Count
$withoutDod = ([regex]::Matches($content, '(?m)^-\s*\[ \][^\r\n]*$')).Count

Write-Output "conservation: closed=$closed open=$open single-line-open(no-DoD-on-line)=$withoutDod"
Write-Output "  file: $MemoryPath"

if ($closed -eq 0) {
    [Console]::Error.WriteLine(
        "conservation violation: closed=0 -- nothing was consumed this day. Close or advance at least one TODO before ending the round."
    )
    exit 2
}
exit 0
