//! Whether a provider is working right now, inferred from its transcript files.
//!
//! The upstream app learns this from hook events posted to its HTTP server, and keeps the result
//! only in memory — nothing on disk says "Claude is busy". This binary does not own that port, so
//! it falls back to the same signal `watcher.rs` uses when hooks do not fire: appends to the
//! session transcripts under `~/.claude/projects/**/*.jsonl`. A file that grew a moment ago means
//! something is being written, which means a turn is in progress.
//!
//! What this file cannot tell you is `attention` — "the chat is waiting on you". That distinction
//! has no signature on disk: a transcript that stopped growing looks identical whether the turn
//! finished or is waiting for an answer. It arrives as a hook event instead, through `hooks.rs`,
//! and the caller prefers that answer whenever it has one. This file is the fallback for when it
//! does not — a provider with no hooks wired up, or one that is not Claude.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

/// How recently a transcript must have grown to count as a turn in progress. Long enough to cover
/// a model thinking between tool calls, short enough that a finished turn stops spinning promptly.
const ACTIVE_WITHIN: Duration = Duration::from_secs(12);

/// Directories are walked no deeper and no wider than this. A transcript tree can hold thousands of
/// files, and this runs on a timer — the newest write is what matters, not a complete census.
const MAX_DEPTH: usize = 4;
const MAX_VISITS: usize = 4000;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Work {
    #[default]
    Idle,
    /// A turn is in progress: the ring spins.
    Running,
    /// The session is waiting on the user — a permission prompt or a question. Only a hook event
    /// can say this; see the module note on why it cannot be inferred from the transcripts.
    Attention,
}

/// Transcript roots per provider, mirroring `watcher.rs::roots`.
fn roots(provider: &str) -> Vec<PathBuf> {
    let Some(home) = dirs::home_dir() else { return Vec::new() };
    match provider {
        "claude" => {
            let mut v = vec![home.join(".claude").join("projects")];
            // The desktop app mirrors each session under its own tree
            if let Some(data) = dirs::config_dir() {
                v.push(data.join("Claude").join("local-agent-mode-sessions"));
            }
            v
        }
        "codex" => vec![home.join(".codex").join("sessions")],
        _ => Vec::new(),
    }
}

/// Only real session transcripts, matching `watcher.rs::is_session_jsonl`: audit logs and sub-agent
/// spool files are written by tooling rather than by a turn in progress.
fn is_transcript(p: &std::path::Path) -> bool {
    if p.extension().map(|e| e != "jsonl").unwrap_or(true) {
        return false;
    }
    !matches!(p.file_name().and_then(|n| n.to_str()), Some("audit.jsonl") | None)
}

/// The most recent write under a provider's roots, or `None` if it has no transcripts at all.
pub fn newest_write(provider: &str) -> Option<SystemTime> {
    let mut newest: Option<SystemTime> = None;
    let mut visits = 0usize;
    let mut stack: Vec<(PathBuf, usize)> = roots(provider).into_iter().map(|r| (r, 0)).collect();

    while let Some((dir, depth)) = stack.pop() {
        if depth > MAX_DEPTH || visits >= MAX_VISITS {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            visits += 1;
            if visits >= MAX_VISITS {
                break;
            }
            let path = entry.path();
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                stack.push((path, depth + 1));
            } else if is_transcript(&path) {
                if let Ok(m) = meta.modified() {
                    if newest.map(|n| m > n).unwrap_or(true) {
                        newest = Some(m);
                    }
                }
            }
        }
    }
    newest
}

/// Turns a last-write time into a state. Split out so the rule can be tested without a filesystem.
pub fn work_from(newest: Option<SystemTime>, now: SystemTime) -> Work {
    match newest {
        Some(t) => match now.duration_since(t) {
            // A file written in the future (clock skew, a network share) counts as right now
            Err(_) => Work::Running,
            Ok(age) if age <= ACTIVE_WITHIN => Work::Running,
            Ok(_) => Work::Idle,
        },
        None => Work::Idle,
    }
}

pub fn probe(provider: &str) -> Work {
    work_from(newest_write(provider), SystemTime::now())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn a_recent_append_means_a_turn_is_running() {
        let now = SystemTime::now();
        assert_eq!(work_from(Some(now), now), Work::Running);
        assert_eq!(work_from(Some(now - Duration::from_secs(3)), now), Work::Running);
    }

    #[test]
    fn an_old_transcript_is_idle() {
        let now = SystemTime::now();
        assert_eq!(work_from(Some(now - ACTIVE_WITHIN - Duration::from_secs(1)), now), Work::Idle);
        assert_eq!(work_from(Some(now - Duration::from_secs(3600)), now), Work::Idle);
    }

    #[test]
    fn no_transcripts_at_all_is_idle_not_a_panic() {
        assert_eq!(work_from(None, SystemTime::now()), Work::Idle);
        assert_eq!(probe("nonsense-provider"), Work::Idle);
    }

    #[test]
    fn a_clock_that_runs_backwards_does_not_read_as_idle() {
        // A file stamped in the future would make `duration_since` fail; treating that as idle
        // would silently stop the ring on any machine with skew.
        let now = SystemTime::now();
        assert_eq!(work_from(Some(now + Duration::from_secs(60)), now), Work::Running);
    }

    #[test]
    fn only_real_transcripts_count() {
        assert!(is_transcript(Path::new(r"C:\x\.claude\projects\p\abc.jsonl")));
        assert!(!is_transcript(Path::new(r"C:\x\.claude\projects\p\audit.jsonl")));
        assert!(!is_transcript(Path::new(r"C:\x\.claude\projects\p\notes.txt")));
        assert!(!is_transcript(Path::new(r"C:\x\.claude\projects\p")));
    }

    #[test]
    fn probing_a_real_provider_returns_without_walking_the_world() {
        // Bounded walk: this must come back quickly even on a machine with a large history.
        let start = std::time::Instant::now();
        let _ = probe("claude");
        assert!(start.elapsed() < Duration::from_secs(3), "the scan took {:?}", start.elapsed());
    }
}
