# SessionStart hook: inject the phase-0 directive into new sessions so the
# loop starts by itself instead of waiting for a human prompt.
Write-Output '[aitrace] Phase 0 (open the loop): read the newest memory file under docs/timeline/, pick the top-priority TODO as this round''s task (WIP=1), declare its DoD. Follow the five-principle loops and self-consistency axioms in CLAUDE.md; build/test/deploy discipline per the aitrace skill. If the user already gave a task, that takes precedence and the TODO pool is consulted at closeout.'
