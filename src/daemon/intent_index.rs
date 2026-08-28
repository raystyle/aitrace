//! Incremental Claude Code transcript index for edit-intent resolution.
//!
//! Claude Code's PostToolUse payload carries no "why" for an edit, but the
//! session transcript (`transcript_path`) holds it: every `tool_use` block is
//! preceded up the `parentUuid` chain by the assistant's declared text (or
//! thinking), and user prompts arrive both as user text entries and
//! `last-prompt` markers. Those lines are written *before* the tool runs --
//! but not always in tree order: assistant parent text can land in the file
//! several seconds *after* its tool_use line (lazy writes). Lookups therefore
//! walk the parent chain live at query time instead of resolving once at
//! absorb time, so late-arriving ancestors are found by the next refresh.
//!
//! The index tails the transcript file incrementally: each refresh reads only
//! the lines appended since the last one (a trailing partial line is left for
//! the next pass). Parsed entries keep at most a truncated snippet each, so
//! memory stays bounded by the number of transcript entries.

use std::collections::HashMap;
use std::io::{BufRead, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// Snippet cap for stored intents. Inline annotations only have one line.
const MAX_INTENT_CHARS: usize = 200;

/// Extract the tool-use id from a `session_id:tool_use_id` operation id.
///
/// Returns `None` when the operation id carries no tool-use part (the
/// session-only fallback), which means no transcript lookup is possible.
pub fn tool_use_id_from_operation(operation_id: &str) -> Option<&str> {
    let (_session, tool_use) = operation_id.rsplit_once(':')?;
    if tool_use.is_empty() {
        None
    } else {
        Some(tool_use)
    }
}

#[derive(Debug, Clone)]
enum EntryKind {
    /// Assistant entry with a text block: the stated plan for what follows.
    AssistantText(String),
    /// Assistant entry with only a thinking block: fallback intent source.
    AssistantThinking(String),
    /// Assistant entry that is itself a tool call (skipped during the walk,
    /// so a batch of parallel tool calls shares the one preceding text).
    ToolUse,
    /// A real user text entry: a prompt source that never lags.
    UserText(String),
    /// tool_result wrappers / attachments / system / anything else: the walk
    /// boundary.
    Other,
}

#[derive(Debug, Clone)]
struct IndexedEntry {
    parent_uuid: Option<String>,
    kind: EntryKind,
}

/// Tracks one transcript file and answers intent lookups.
pub struct IntentIndex {
    entries: HashMap<String, IndexedEntry>,
    /// tool_use_id → uuid of the entry that carries the tool_use block.
    tool_use_entries: HashMap<String, String>,
    user_prompt: Option<String>,
    /// Transcript being tailed and the byte offset already consumed.
    source: Option<(PathBuf, u64)>,
}

impl Default for IntentIndex {
    fn default() -> Self {
        Self::new()
    }
}

impl IntentIndex {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            tool_use_entries: HashMap::new(),
            user_prompt: None,
            source: None,
        }
    }

    /// Operation intent (assistant text, else thinking) for a tool-use id.
    ///
    /// Walks the parent chain at query time, so an ancestor appended after
    /// the tool_use line is picked up by the next refresh + query.
    pub fn operation_intent(&self, tool_use_id: &str) -> Option<String> {
        let uuid = self.tool_use_entries.get(tool_use_id)?;
        self.intent_for_entry(uuid)
    }

    /// The most recent user prompt seen in the transcript.
    ///
    /// Fed by both `last-prompt` markers and real user text entries --
    /// whichever appears later in the file wins, because the marker alone
    /// can lag a turn behind.
    pub fn user_prompt(&self) -> Option<String> {
        self.user_prompt.clone()
    }

    /// Consume transcript lines appended since the last refresh.
    ///
    /// Switching to a different transcript resets the index (one transcript
    /// per recording session in practice).
    pub fn refresh(&mut self, path: &Path) {
        let need_reset = self.source.as_ref().is_none_or(|(p, _)| p != path);
        if need_reset {
            self.entries.clear();
            self.tool_use_entries.clear();
            self.user_prompt = None;
            self.source = Some((path.to_path_buf(), 0));
        }
        let mut offset = match &self.source {
            Some((_, o)) => *o,
            None => return,
        };

        let Ok(mut file) = std::fs::File::open(path) else {
            return;
        };
        if file.seek(SeekFrom::Start(offset)).is_err() {
            return;
        }
        let mut reader = std::io::BufReader::new(file);
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                // EOF
                Ok(0) => break,
                Ok(n) => {
                    // A trailing partial line (the writer is mid-append) is
                    // not consumed; it will be complete by the next refresh.
                    if !line.ends_with('\n') {
                        break;
                    }
                    offset += n as u64;
                    self.absorb_line(line.trim_end_matches(['\n', '\r']));
                }
                Err(_) => break,
            }
        }
        if let Some((_, o)) = &mut self.source {
            *o = offset;
        }
        tracing::debug!(
            "transcript refresh: {} entries, {} tool ids, prompt={:?}",
            self.entries.len(),
            self.tool_use_entries.len(),
            self.user_prompt.as_deref().map(|s| &s[..s.len().min(40)])
        );
    }

    fn absorb_line(&mut self, line: &str) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            return;
        };

        // last-prompt entries carry the current user request and no uuid.
        if v.get("type").and_then(|t| t.as_str()) == Some("last-prompt") {
            if let Some(p) = v.get("lastPrompt").and_then(|p| p.as_str()) {
                self.user_prompt = Some(snippet(p));
            }
            return;
        }

        let Some(uuid) = v.get("uuid").and_then(|u| u.as_str()) else {
            return;
        };
        let parent_uuid = v
            .get("parentUuid")
            .and_then(|u| u.as_str())
            .map(|s| s.to_string());
        let entry_type = v.get("type").and_then(|t| t.as_str());

        let kind = if entry_type == Some("assistant") {
            Self::assistant_kind(&v, uuid, &mut self.tool_use_entries)
        } else if entry_type == Some("user") {
            Self::user_kind(&v)
        } else {
            EntryKind::Other
        };

        if let EntryKind::UserText(p) = &kind {
            self.user_prompt = Some(p.clone());
        }

        self.entries
            .insert(uuid.to_string(), IndexedEntry { parent_uuid, kind });
    }

    fn assistant_kind(
        v: &serde_json::Value,
        uuid: &str,
        tool_use_entries: &mut HashMap<String, String>,
    ) -> EntryKind {
        let mut text = None;
        let mut thinking = None;
        let mut has_tool_use = false;
        let Some(blocks) = content_blocks(v) else {
            return EntryKind::Other;
        };
        for block in blocks {
            match block.get("type").and_then(|t| t.as_str()) {
                Some("text") => {
                    if text.is_none() {
                        text = block
                            .get("text")
                            .and_then(|t| t.as_str())
                            .filter(|t| !t.trim().is_empty());
                    }
                }
                Some("thinking") => {
                    if thinking.is_none() {
                        thinking = block
                            .get("thinking")
                            .and_then(|t| t.as_str())
                            .filter(|t| !t.trim().is_empty());
                    }
                }
                Some("tool_use") => {
                    has_tool_use = true;
                    if let Some(id) = block.get("id").and_then(|i| i.as_str()) {
                        tool_use_entries.insert(id.to_string(), uuid.to_string());
                    }
                }
                _ => {}
            }
        }
        match (text, thinking) {
            (Some(t), _) => EntryKind::AssistantText(snippet(t)),
            (None, Some(th)) => EntryKind::AssistantThinking(snippet(th)),
            (None, None) if has_tool_use => EntryKind::ToolUse,
            (None, None) => EntryKind::Other,
        }
    }

    fn user_kind(v: &serde_json::Value) -> EntryKind {
        let content = v.get("message").and_then(|m| m.get("content"));
        let text = match content.and_then(|c| c.as_array()) {
            Some(blocks) => blocks.iter().find_map(|block| {
                if block.get("type").and_then(|t| t.as_str()) == Some("text") {
                    block
                        .get("text")
                        .and_then(|t| t.as_str())
                        .filter(|t| !t.trim().is_empty())
                } else {
                    None
                }
            }),
            // Plain-string content (older transcript shape).
            None => content
                .and_then(|c| c.as_str())
                .filter(|t| !t.trim().is_empty()),
        };
        match text {
            Some(t) => EntryKind::UserText(snippet(t)),
            None => EntryKind::Other,
        }
    }

    /// Walk the parent chain from `uuid`: the nearest assistant text wins;
    /// with no text up to the boundary, fall back to the nearest thinking.
    fn intent_for_entry(&self, uuid: &str) -> Option<String> {
        let mut first_thinking: Option<String> = None;
        let mut cur = self.entries.get(uuid)?;
        'walk: loop {
            match &cur.kind {
                EntryKind::AssistantText(t) => return Some(t.clone()),
                EntryKind::AssistantThinking(t) => {
                    if first_thinking.is_none() {
                        first_thinking = Some(t.clone());
                    }
                }
                EntryKind::ToolUse | EntryKind::UserText(_) | EntryKind::Other => {}
            }
            let Some(parent) = cur.parent_uuid.as_deref() else {
                break 'walk;
            };
            let Some(next) = self.entries.get(parent) else {
                break 'walk;
            };
            cur = next;
        }
        first_thinking
    }
}

fn content_blocks(v: &serde_json::Value) -> Option<&Vec<serde_json::Value>> {
    v.get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_array())
}

/// Collapse whitespace and cap the snippet length.
fn snippet(text: &str) -> String {
    let collapsed: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= MAX_INTENT_CHARS {
        collapsed
    } else {
        let cut: String = collapsed.chars().take(MAX_INTENT_CHARS).collect();
        format!("{cut}…")
    }
}

// ---- unit tests ---------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn assistant_text(uuid: &str, parent: Option<&str>, text: &str) -> String {
        let parent_json = parent
            .map(|p| format!("\"parentUuid\":\"{p}\","))
            .unwrap_or_default();
        format!(
            r#"{{"uuid":"{uuid}",{parent_json}"type":"assistant","message":{{"role":"assistant","content":[{{"type":"text","text":"{text}"}}]}}}}"#
        )
    }

    fn assistant_tool_use(uuid: &str, parent: Option<&str>, tool_id: &str) -> String {
        let parent_json = parent
            .map(|p| format!("\"parentUuid\":\"{p}\","))
            .unwrap_or_default();
        format!(
            r#"{{"uuid":"{uuid}",{parent_json}"type":"assistant","message":{{"role":"assistant","content":[{{"type":"tool_use","id":"{tool_id}","name":"Edit","input":{{}}}}]}}}}"#
        )
    }

    fn assistant_thinking(uuid: &str, parent: Option<&str>, text: &str) -> String {
        let parent_json = parent
            .map(|p| format!("\"parentUuid\":\"{p}\","))
            .unwrap_or_default();
        format!(
            r#"{{"uuid":"{uuid}",{parent_json}"type":"assistant","message":{{"role":"assistant","content":[{{"type":"thinking","thinking":"{text}"}}]}}}}"#
        )
    }

    fn user_tool_result(uuid: &str, parent: Option<&str>) -> String {
        let parent_json = parent
            .map(|p| format!("\"parentUuid\":\"{p}\","))
            .unwrap_or_default();
        format!(
            r#"{{"uuid":"{uuid}",{parent_json}"type":"user","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"x"}}]}}}}"#
        )
    }

    fn user_text(uuid: &str, parent: Option<&str>, text: &str) -> String {
        let parent_json = parent
            .map(|p| format!("\"parentUuid\":\"{p}\","))
            .unwrap_or_default();
        format!(
            r#"{{"uuid":"{uuid}",{parent_json}"type":"user","message":{{"role":"user","content":[{{"type":"text","text":"{text}"}}]}}}}"#
        )
    }

    fn write_transcript(path: &Path, lines: &[String]) {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        for l in lines {
            writeln!(f, "{l}").unwrap();
        }
    }

    #[test]
    fn text_before_tool_use_is_the_intent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        write_transcript(
            &path,
            &[
                assistant_text("u1", None, "fix the sort key"),
                assistant_tool_use("u2", Some("u1"), "call_1"),
            ],
        );
        let mut idx = IntentIndex::new();
        idx.refresh(&path);
        assert_eq!(
            idx.operation_intent("call_1").as_deref(),
            Some("fix the sort key")
        );
    }

    #[test]
    fn batch_tool_uses_share_the_preceding_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        write_transcript(
            &path,
            &[
                assistant_text("u1", None, "apply the same fix everywhere"),
                assistant_tool_use("u2", Some("u1"), "call_1"),
                assistant_tool_use("u3", Some("u2"), "call_2"),
                assistant_tool_use("u4", Some("u3"), "call_3"),
            ],
        );
        let mut idx = IntentIndex::new();
        idx.refresh(&path);
        for call in ["call_1", "call_2", "call_3"] {
            assert_eq!(
                idx.operation_intent(call).as_deref(),
                Some("apply the same fix everywhere"),
                "{call} should share the batch intent"
            );
        }
    }

    #[test]
    fn thinking_is_used_only_without_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        // Chain 1: tool_use -> thinking -> user (no text anywhere).
        // Chain 2: tool_use -> thinking -> text.
        write_transcript(
            &path,
            &[
                user_tool_result("r0", None),
                assistant_thinking("w1", Some("r0"), "internal reasoning A"),
                assistant_tool_use("w2", Some("w1"), "call_a"),
                assistant_thinking("w3", Some("w2"), "internal reasoning B"),
                assistant_text("w4", Some("w3"), "stated plan B"),
                assistant_tool_use("w5", Some("w4"), "call_b"),
            ],
        );
        let mut idx = IntentIndex::new();
        idx.refresh(&path);
        // No text on the chain: nearest thinking is the fallback.
        assert_eq!(
            idx.operation_intent("call_a").as_deref(),
            Some("internal reasoning A")
        );
        // Text exists further up: text wins over the nearer thinking.
        assert_eq!(
            idx.operation_intent("call_b").as_deref(),
            Some("stated plan B")
        );
    }

    #[test]
    fn walk_stops_at_user_boundary() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        // A tool_use whose ancestry has no assistant text at all.
        write_transcript(
            &path,
            &[
                user_tool_result("r0", None),
                assistant_tool_use("u2", Some("r0"), "call_orphan"),
            ],
        );
        let mut idx = IntentIndex::new();
        idx.refresh(&path);
        assert_eq!(idx.operation_intent("call_orphan"), None);
    }

    #[test]
    fn late_parent_text_is_found_by_live_lookup() {
        // The real-world regression: Claude Code appends the parent text
        // AFTER the tool_use line, so resolve-at-absorb-time missed it.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        write_transcript(&path, &[assistant_tool_use("u2", Some("u1"), "call_late")]);
        let mut idx = IntentIndex::new();
        idx.refresh(&path);
        assert_eq!(
            idx.operation_intent("call_late"),
            None,
            "parent not yet in file: no intent yet"
        );
        // The parent text lands seconds later; a later refresh + query finds it.
        write_transcript(&path, &[assistant_text("u1", None, "late arriving plan")]);
        idx.refresh(&path);
        assert_eq!(
            idx.operation_intent("call_late").as_deref(),
            Some("late arriving plan")
        );
    }

    #[test]
    fn incremental_refresh_ignores_partial_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        write_transcript(
            &path,
            &[
                assistant_text("u1", None, "round one"),
                assistant_tool_use("u2", Some("u1"), "call_1"),
            ],
        );
        // Append the first half of another tool_use line (no trailing
        // newline), as if the writer were mid-append.
        use std::io::Write;
        let full_line = assistant_tool_use("u3", Some("u2"), "call_2");
        let (head, tail) = full_line.split_at(full_line.len() / 2);
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        write!(f, "{head}").unwrap();
        drop(f);

        let mut idx = IntentIndex::new();
        idx.refresh(&path);
        assert_eq!(idx.operation_intent("call_1").as_deref(), Some("round one"));

        // Complete the line and refresh again: it is picked up now.
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(f, "{tail}").unwrap();
        drop(f);
        idx.refresh(&path);
        assert_eq!(idx.operation_intent("call_2").as_deref(), Some("round one"));
    }

    #[test]
    fn last_prompt_is_tracked() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        write_transcript(
            &path,
            &[r#"{"type":"last-prompt","lastPrompt":"fix the bug","leafUuid":"x"}"#.to_string()],
        );
        let mut idx = IntentIndex::new();
        idx.refresh(&path);
        assert_eq!(idx.user_prompt().as_deref(), Some("fix the bug"));
    }

    #[test]
    fn user_text_entries_feed_the_prompt_without_lag() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        write_transcript(
            &path,
            &[
                r#"{"type":"last-prompt","lastPrompt":"older request","leafUuid":"x"}"#.to_string(),
                user_text("p1", None, "newer request"),
            ],
        );
        let mut idx = IntentIndex::new();
        idx.refresh(&path);
        // The later-in-file user entry wins over the stale marker.
        assert_eq!(idx.user_prompt().as_deref(), Some("newer request"));
    }

    #[test]
    fn tool_result_users_do_not_feed_the_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        write_transcript(
            &path,
            &[
                r#"{"type":"last-prompt","lastPrompt":"the ask","leafUuid":"x"}"#.to_string(),
                user_tool_result("r1", None),
            ],
        );
        let mut idx = IntentIndex::new();
        idx.refresh(&path);
        assert_eq!(idx.user_prompt().as_deref(), Some("the ask"));
    }

    #[test]
    fn switching_transcripts_resets() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.jsonl");
        let b = dir.path().join("b.jsonl");
        write_transcript(
            &a,
            &[
                assistant_text("u1", None, "from a"),
                assistant_tool_use("u2", Some("u1"), "call_1"),
            ],
        );
        write_transcript(
            &b,
            &[
                assistant_text("v1", None, "from b"),
                assistant_tool_use("v2", Some("v1"), "call_2"),
            ],
        );
        let mut idx = IntentIndex::new();
        idx.refresh(&a);
        idx.refresh(&b);
        assert_eq!(idx.operation_intent("call_2").as_deref(), Some("from b"));
        assert_eq!(idx.operation_intent("call_1"), None);
    }

    #[test]
    fn long_intents_are_truncated() {
        let long = "word ".repeat(100);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        write_transcript(
            &path,
            &[
                assistant_text("u1", None, &long),
                assistant_tool_use("u2", Some("u1"), "call_1"),
            ],
        );
        let mut idx = IntentIndex::new();
        idx.refresh(&path);
        let intent = idx.operation_intent("call_1").unwrap();
        assert!(
            intent.chars().count() <= MAX_INTENT_CHARS + 1,
            "intent not truncated"
        );
        assert!(intent.ends_with('…'));
    }

    #[test]
    fn operation_id_tool_use_extraction() {
        assert_eq!(
            tool_use_id_from_operation("sess-1:call_abc"),
            Some("call_abc")
        );
        assert_eq!(tool_use_id_from_operation("sess-1"), None);
        assert_eq!(tool_use_id_from_operation("sess-1:"), None);
    }
}
