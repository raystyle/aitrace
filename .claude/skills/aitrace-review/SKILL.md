---
name: aitrace-review
description: >
  Self-correction workflow using aitrace edit history. Use when tests fail,
  behavior regresses after AI-assisted edits, or the user asks to bisect,
  rewind, or inspect what changed in this session.
allowed-tools: Read Grep Glob Bash(cargo test *) Bash(cargo build *) mcp__aitrace__list_sessions mcp__aitrace__get_timeline mcp__aitrace__get_frame mcp__aitrace__diff_frames mcp__aitrace__search_edits mcp__aitrace__get_regression_window mcp__aitrace__subscribe_edits
---

# aitrace Self-Correction Review

Use this skill when tests fail or behavior regresses after a series of AI-assisted edits. Scrub the aitrace timeline via MCP and fix the regression at its source.

Project MCP is already in `.mcp.json` as server `aitrace` (`aitrace mcp`). Do not add a user-scoped MCP server. The daemon must be recording (`aitrace daemon status`).

## Workflow

### Phase 1: Load Context

1. Call `list_sessions` to find the active or most recent session
2. Call `get_timeline` with the session ID to get the full edit history
3. Note the total number of edits, which files were touched, and the edit range

### Phase 2: Identify Scope

1. Group edits by `operation_id` to understand logical units of work
2. Group edits by file to see which files changed most
3. Identify the "before" state (frame 1 or the start of the current work)

### Phase 3: Run Verification

1. Run `cargo test` at the repo root
2. If everything passes, report success and stop
3. If there are failures, note the specific errors and failing tests

### Phase 4: Bisect the Regression

1. Call `get_regression_window` with the relevant file filter to narrow candidates
2. Start a binary search through the candidate frames:
   a. Pick the midpoint frame
   b. Call `get_frame` at that point to see the file state
   c. Use `diff_frames` to compare the midpoint against the known-good state
   d. Assess whether the regression-causing change is before or after this point
   e. Narrow the window and repeat
3. Once you identify the exact frame that introduced the issue, call `diff_frames` between it and the previous frame to see exactly what changed

### Phase 5: Fix Surgically

1. Call `get_frame` at the frame just before the regression to see the intended state
2. Understand what the edit was trying to do (check the `intent` field)
3. Write a targeted fix that preserves the intent but corrects the error
4. Do NOT revert the entire edit — fix the specific issue

### Phase 6: Verify Fix

1. Re-run `cargo test` to confirm the regression is fixed
2. Run `get_timeline` again to confirm the fix was recorded
3. Report what was found, what frame introduced it, and what was fixed

## Tips

- `search_edits` with a regex finds frames that touched a function or variable
- Fix multiple regressions one at a time; re-run tests after each
- `subscribe_edits` for live notifications as new edits are recorded
- `file_filter` on `get_timeline` narrows to one file
