use anyhow::Result;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

use crate::event::AgentInfo;

/// The operating mode for a session.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SessionMode {
    Enriched,
    Passive,
}

/// Metadata stored in a session's `meta.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    pub id: String,
    pub project_path: String,
    /// Session start in **epoch milliseconds**. Values written before the
    /// millisecond migration are seconds and are normalized on read.
    #[serde(deserialize_with = "deserialize_started_at_millis")]
    pub started_at: i64,
    pub mode: SessionMode,
    #[serde(default)]
    pub agents: Vec<AgentInfo>,
}

/// Normalize legacy second-resolution timestamps to milliseconds.
///
/// Second timestamps stay below 10^12 until the year 33658; millisecond
/// timestamps are above it, so magnitude separates the two eras.
fn deserialize_started_at_millis<'de, D>(d: D) -> Result<i64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v = i64::deserialize(d)?;
    if v > 0 && v < 1_000_000_000_000 {
        Ok(v * 1000)
    } else {
        Ok(v)
    }
}

/// A handle to an active or created session on disk.
pub struct Session {
    pub id: String,
    pub dir: PathBuf,
}

impl Session {
    /// Generate a session ID in the format `YYYYMMDD-HHMMSS-ffffff`
    /// where `ffffff` is the microsecond-within-second, zero-padded.
    ///
    /// The suffix is **monotonic**: session directory names then sort
    /// lexicographically in true creation order even for sessions created
    /// within the same second. (The previous `micros & 0xFFFF` hex suffix
    /// wrapped arbitrarily, making "latest dir by name" unreliable.)
    pub fn generate_id() -> String {
        Utc::now().format("%Y%m%d-%H%M%S-%6f").to_string()
    }
}

/// Manages sessions stored under a root `sessions_dir` directory.
pub struct SessionManager {
    pub sessions_dir: PathBuf,
}

impl SessionManager {
    pub fn new(sessions_dir: PathBuf) -> Self {
        Self { sessions_dir }
    }

    /// Create a new session: generates an ID, creates the directory layout,
    /// and writes `meta.json`.
    pub fn create(&self) -> Result<Session> {
        let id = Session::generate_id();
        let dir = self.sessions_dir.join(&id);

        fs::create_dir_all(dir.join("snapshots"))?;
        fs::create_dir_all(dir.join("checkpoints"))?;

        let meta = SessionMeta {
            id: id.clone(),
            project_path: String::new(),
            started_at: Utc::now().timestamp_millis(),
            mode: SessionMode::Enriched,
            agents: Vec::new(),
        };

        let meta_path = dir.join("meta.json");
        let meta_json = serde_json::to_string_pretty(&meta)?;
        fs::write(meta_path, meta_json)?;

        Ok(Session { id, dir })
    }

    /// List all sessions sorted by `started_at` ascending.
    pub fn list(&self) -> Result<Vec<SessionMeta>> {
        if !self.sessions_dir.exists() {
            return Ok(Vec::new());
        }

        let mut metas = Vec::new();

        for entry in fs::read_dir(&self.sessions_dir)? {
            let entry = entry?;
            let meta_path = entry.path().join("meta.json");
            if meta_path.exists() {
                let content = fs::read_to_string(&meta_path)?;
                let meta: SessionMeta = serde_json::from_str(&content)?;
                metas.push(meta);
            }
        }

        // Millisecond resolution plus the monotonic id as a tie-break:
        // creation order survives even for sessions started in the same
        // millisecond.
        metas.sort_by(|a, b| a.started_at.cmp(&b.started_at).then(a.id.cmp(&b.id)));
        Ok(metas)
    }

    /// Load the metadata for a specific session by ID.
    pub fn load_meta(&self, id: &str) -> Result<SessionMeta> {
        let meta_path = self.sessions_dir.join(id).join("meta.json");
        let content = fs::read_to_string(&meta_path)?;
        let meta: SessionMeta = serde_json::from_str(&content)?;
        Ok(meta)
    }

    /// Newest session **before** `exclude_id` that actually recorded edits.
    ///
    /// Baseline inheritance walks backwards: the immediately preceding
    /// session may have crashed or recorded nothing (no `edits.jsonl`), and
    /// stopping there would reset every file to `create` on the next
    /// restart. Sessions without an edit log are skipped.
    pub fn latest_prior_session_with_edits(&self, exclude_id: &str) -> Option<PathBuf> {
        let mut dirs: Vec<PathBuf> = fs::read_dir(&self.sessions_dir)
            .ok()?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_dir() && p.file_name().and_then(|n| n.to_str()) != Some(exclude_id))
            .collect();
        dirs.sort();
        dirs.reverse();
        dirs.into_iter().find(|p| p.join("edits.jsonl").is_file())
    }
}

// ─── unit tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_v1_meta_deserializes_with_empty_agents() {
        let v1_json = r#"{"id":"test-123","project_path":"/tmp","started_at":0,"mode":"passive"}"#;
        let meta: SessionMeta = serde_json::from_str(v1_json).unwrap();
        assert!(meta.agents.is_empty());
    }

    #[test]
    fn test_legacy_second_timestamps_are_normalized_to_millis() {
        let legacy = r#"{"id":"s","project_path":"/p","started_at":1787888868,"mode":"passive"}"#;
        let meta: SessionMeta = serde_json::from_str(legacy).unwrap();
        assert_eq!(meta.started_at, 1_787_888_868_000);

        // Already-millisecond values pass through untouched.
        let modern =
            r#"{"id":"s","project_path":"/p","started_at":1787888868123,"mode":"passive"}"#;
        let meta: SessionMeta = serde_json::from_str(modern).unwrap();
        assert_eq!(meta.started_at, 1_787_888_868_123);
    }

    #[test]
    fn test_sessions_list_in_creation_order_within_a_second() {
        let tmp = tempfile::tempdir().unwrap();
        let mgr = SessionManager::new(tmp.path().to_path_buf());

        let s1 = mgr.create().unwrap();
        let s2 = mgr.create().unwrap();

        // Millisecond precision: the second session starts strictly later
        // (session creation does multiple syscalls, so >1ms apart).
        assert!(s2.dir.file_name() > s1.dir.file_name());

        let metas = mgr.list().unwrap();
        assert_eq!(metas.len(), 2);
        assert_eq!(metas[0].id, s1.id, "creation order must hold in list()");
        assert_eq!(metas[1].id, s2.id);
        // Non-decreasing is the contract: on fast platforms (Linux) both
        // creations can land in the same millisecond, and list() then
        // orders by the microsecond id suffix.
        assert!(
            metas[1].started_at >= metas[0].started_at,
            "started_at went backwards: {} vs {}",
            metas[0].started_at,
            metas[1].started_at
        );
    }

    #[test]
    fn test_session_ids_are_lexicographically_monotonic() {
        // Rapid-fire generation must never go backwards by name: dir-name
        // order is creation order, even within the same second.
        let mut prev = String::new();
        for _ in 0..100 {
            let id = Session::generate_id();
            assert!(
                id > prev,
                "session id went backwards or repeated: {prev:?} -> {id:?}"
            );
            assert!(
                id.len() == "YYYYMMDD-HHMMSS-ffffff".len(),
                "unexpected id shape: {id:?}"
            );
            prev = id;
        }
    }

    #[test]
    fn test_latest_prior_session_with_edits_skips_editless_sessions() {
        let tmp = tempfile::tempdir().unwrap();
        let mgr = SessionManager::new(tmp.path().to_path_buf());

        // Three sessions: s1 recorded edits, s2 crashed before recording,
        // s3 is the current one being excluded.
        for (id, with_edits) in [("s1", true), ("s2", false), ("s3", false)] {
            let dir = tmp.path().join(id);
            fs::create_dir_all(&dir).unwrap();
            if with_edits {
                fs::write(dir.join("edits.jsonl"), "").unwrap();
            }
        }

        let found = mgr
            .latest_prior_session_with_edits("s3")
            .expect("should skip s2 and find s1");
        assert_eq!(found.file_name().unwrap(), "s1");

        // An exclusion that matches no directory leaves s1 as the winner.
        let found = mgr
            .latest_prior_session_with_edits("nonexistent")
            .expect("all sessions are candidates");
        assert_eq!(found.file_name().unwrap(), "s1");
    }

    #[test]
    fn test_latest_prior_session_with_edits_none_when_no_logs() {
        let tmp = tempfile::tempdir().unwrap();
        let mgr = SessionManager::new(tmp.path().to_path_buf());
        fs::create_dir_all(tmp.path().join("empty-session")).unwrap();
        assert!(
            mgr.latest_prior_session_with_edits("empty-session")
                .is_none()
        );
    }
}
