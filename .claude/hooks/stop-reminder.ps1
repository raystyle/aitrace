# Stop hook: one-shot reminder to run the aitrace phase-7 closeout when new
# edits exist since the last phase-7 pass. Guards (self-consistency axiom 3):
#   - never blocks twice in a row (checks stop_hook_active from stdin)
#   - never blocks when the marker .aitrace/.last-stage7 is current
#   - any error allows the stop (fail-open)
$project = $env:CLAUDE_PROJECT_DIR
if (-not $project) { exit 0 }
try {
    $stdin = [Console]::In.ReadToEnd()
    if ($stdin -match '"stop_hook_active"\s*:\s*true') { exit 0 }

    $sessions = Join-Path $project '.aitrace\sessions'
    if (-not (Test-Path $sessions)) { exit 0 }
    $latest = Get-ChildItem $sessions -Directory | Sort-Object Name | Select-Object -Last 1
    if (-not $latest) { exit 0 }
    $edits = Join-Path $latest.FullName 'edits.jsonl'
    if (-not (Test-Path $edits)) { exit 0 }

    $marker = Join-Path $project '.aitrace\.last-stage7'
    if ((Test-Path $marker) -and (Get-Item $marker).LastWriteTime -ge (Get-Item $edits).LastWriteTime) { exit 0 }

    [Console]::Error.WriteLine('[aitrace] New edits recorded since the last phase-7 pass. Run the aitrace skill phase 7 now: patch/diff introspection, update docs/timeline memory, conservation check (produce >=1 AND consume >=1 TODOs, each with DoD), then refresh the marker so this reminder stays quiet: powershell -NoProfile -Command "New-Item -ItemType File -Force .aitrace/.last-stage7 | Out-Null"')
    exit 2
} catch {
    exit 0
}
