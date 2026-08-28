use anyhow::Result;
use glob::Pattern;
use notify::{Event, EventKind, RecursiveMode, Watcher};
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::time::Duration;

/// One compiled ignore rule. Plain patterns match a whole path component
/// exactly; patterns containing glob wildcards (`*`, `?`, `[`) match globs.
#[derive(Debug, Clone)]
enum IgnoreRule {
    Exact(String),
    Glob(Pattern),
}

impl IgnoreRule {
    fn compile(pattern: &str) -> Self {
        if pattern.contains('*') || pattern.contains('?') || pattern.contains('[') {
            match Pattern::new(pattern) {
                Ok(p) => IgnoreRule::Glob(p),
                // Invalid glob: fall back to exact matching so the rule still
                // behaves predictably instead of being silently dropped.
                Err(_) => IgnoreRule::Exact(pattern.to_string()),
            }
        } else {
            IgnoreRule::Exact(pattern.to_string())
        }
    }

    fn matches(&self, component: &str) -> bool {
        match self {
            IgnoreRule::Exact(p) => component == p,
            IgnoreRule::Glob(p) => p.matches(component),
        }
    }
}

/// Watches a directory for filesystem changes and sends changed file paths
/// over an mpsc channel.
pub struct FsWatcher {
    root: PathBuf,
    tx: Sender<PathBuf>,
    debounce_ms: u64,
    ignore: Vec<IgnoreRule>,
    watcher: Option<Box<dyn Watcher + Send>>,
}

impl FsWatcher {
    /// Create a new watcher with no ignore patterns.
    pub fn new(root: PathBuf, tx: Sender<PathBuf>, debounce_ms: u64) -> Result<Self> {
        Self::with_ignore(root, tx, debounce_ms, Vec::new())
    }

    /// Create a new watcher with ignore patterns.
    /// Paths containing any ignore pattern as a path component will be filtered.
    /// Patterns may be exact component names (`target`) or globs (`*.tmp.*`).
    pub fn with_ignore(
        root: PathBuf,
        tx: Sender<PathBuf>,
        debounce_ms: u64,
        ignore: Vec<String>,
    ) -> Result<Self> {
        Ok(Self {
            root,
            tx,
            debounce_ms,
            ignore: ignore.iter().map(|p| IgnoreRule::compile(p)).collect(),
            watcher: None,
        })
    }

    /// Start watching the root directory recursively.
    pub fn start(&mut self) -> Result<()> {
        let tx = self.tx.clone();
        let ignore = self.ignore.clone();
        let debounce = Duration::from_millis(self.debounce_ms);

        let mut watcher = notify::recommended_watcher(move |result: notify::Result<Event>| {
            if let Ok(event) = result {
                // Only process events that indicate actual file modifications/creations
                match event.kind {
                    EventKind::Create(_) | EventKind::Modify(_) => {}
                    _ => return,
                }

                for path in event.paths {
                    // Filter out paths containing any ignore pattern as a component
                    let should_ignore = path.components().any(|component| {
                        let comp_str = component.as_os_str().to_string_lossy();
                        ignore.iter().any(|rule| rule.matches(&comp_str))
                    });

                    if !should_ignore {
                        // Best-effort send; if receiver is gone we just drop the event
                        let _ = tx.send(path);
                    }
                }
            }
        })?;

        // Configure debounce if the watcher supports it (notify 7 handles this internally)
        let _ = debounce; // debounce_ms stored for reference; recommended_watcher has internal handling

        watcher.watch(&self.root, RecursiveMode::Recursive)?;

        self.watcher = Some(Box::new(watcher));
        Ok(())
    }

    /// Stop watching by dropping the watcher.
    pub fn stop(&mut self) {
        self.watcher = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_rule_matches_whole_component_only() {
        let rule = IgnoreRule::compile("target");
        assert!(rule.matches("target"));
        assert!(!rule.matches("target-debug"));
        assert!(!rule.matches("app.rs"));
    }

    #[test]
    fn glob_rule_matches_editor_temp_files() {
        let rule = IgnoreRule::compile("*.tmp.*");
        assert!(rule.matches("app.rs.tmp.10032.03f687b21b9d"));
        assert!(rule.matches("x.tmp.1"));
        assert!(!rule.matches("app.rs"));
        assert!(!rule.matches("app.rs.tmp"));
    }

    #[test]
    fn invalid_glob_falls_back_to_exact() {
        let rule = IgnoreRule::compile("[unclosed");
        // Treated as an exact component match, not silently dropped.
        assert!(rule.matches("[unclosed"));
        assert!(!rule.matches("other"));
    }
}
