//! Codenotch with no window of its own.
//!
//! On Windows the pill is drawn by `codenotch-native`. On Linux the desktop draws it — on GNOME a
//! Shell extension (`linux/gnome-extension`) — and all this process does is what the pill cannot:
//! take the hook port Claude Code posts to, and run the usage fetchers. Everything lands as JSON in
//! `~/.config/codenotch/`, which is what the extension reads; the session table goes to
//! `sessions.json` beside it, since the extension cannot ask this process for it.
//!
//! The modules are the Windows build's own files, pulled in by path, so a fix to a fetcher lands in
//! both at once.

#[path = "../../codenotch-native/src/agy_cli.rs"]
mod agy_cli;
#[path = "../../codenotch-native/src/antigravity.rs"]
mod antigravity;
#[path = "../../codenotch-native/src/codex.rs"]
mod codex;
#[path = "../../codenotch-native/src/cursor.rs"]
mod cursor;
#[path = "../../codenotch-native/src/hooks.rs"]
mod hooks;
#[path = "../../codenotch-native/src/hooks_install.rs"]
mod hooks_install;
#[path = "../../codenotch-native/src/state.rs"]
mod state;
#[path = "../../codenotch-native/src/usage.rs"]
mod usage;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

const HOOK_PORT: u16 = 48666;

pub(crate) fn data_dir() -> PathBuf {
    dirs::config_dir().unwrap_or_default().join("codenotch")
}

/// Always written: this runs as a background service, and a log is the only way to see it.
pub(crate) fn trace(line: &str) {
    use std::io::Write;
    let _ = std::fs::create_dir_all(data_dir());
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(data_dir().join("daemon.log")) {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let _ = writeln!(f, "[{secs}] {line}");
    }
}

/// The session table, as the pill needs it: one aggregate state and the live sessions, most urgent
/// first. Written only when it changes.
fn write_sessions(hub: &hooks::Hub) {
    let Ok(store) = hub.store.lock() else { return };
    let snap = store.snapshot("en", "en", true, false);
    let sessions: Vec<_> = snap
        .sessions
        .iter()
        .take(4)
        .map(|s| {
            let what = match s.state.as_str() {
                state::ST_ATTENTION if !s.attn.is_empty() => s.attn.clone(),
                state::ST_ATTENTION => "waiting on you".into(),
                state::ST_RUNNING if !s.last.is_empty() => s.last.clone(),
                state::ST_RUNNING => "working".into(),
                state::ST_DONE => "done".into(),
                _ => "idle".into(),
            };
            let title = if s.title.is_empty() { s.id.clone() } else { s.title.clone() };
            serde_json::json!({ "title": title, "what": what })
        })
        .collect();
    let body = serde_json::json!({ "agg": snap.agg, "sessions": sessions });
    let _ = std::fs::write(data_dir().join("sessions.json"), body.to_string());
}

/// Whether a process still exists: a session whose Claude Code process is gone cannot be waiting.
#[cfg(unix)]
fn pid_alive(pid: u32) -> bool {
    std::path::Path::new(&format!("/proc/{pid}")).exists()
}
#[cfg(not(unix))]
fn pid_alive(_pid: u32) -> bool {
    true
}

fn main() {
    let _ = std::fs::create_dir_all(data_dir());
    let hub = Arc::new(hooks::Hub::default());
    if !hooks::start(hub.clone(), HOOK_PORT) {
        if hooks::port_answers(HOOK_PORT) {
            trace("another codenotch answers on the hook port, leaving");
            return;
        }
        trace("hook port is held but nobody answers: carrying on without hook events");
    } else {
        trace("start: hook port taken");
    }
    if !hooks_install::is_installed() {
        match hooks_install::install() {
            Ok(msg) => trace(&format!("hooks installed: {msg}")),
            Err(e) => trace(&format!("hooks not installed: {e}")),
        }
    }

    hub.usage.lock().map(|mut u| *u = usage::load_persisted()).ok();
    usage::start(hub.clone());
    hub.codex.lock().map(|mut u| *u = codex::load_persisted()).ok();
    codex::start(hub.clone());
    hub.cursor.lock().map(|mut u| *u = cursor::load_persisted()).ok();
    cursor::start(hub.clone());
    hub.antigravity.lock().map(|mut u| *u = antigravity::load_persisted()).ok();
    antigravity::start(hub.clone());

    // The extension asks for fresh numbers by touching this file when the pill opens: the Antigravity
    // CLI path reads only on request, and Cursor polls slowly.
    let refresh = data_dir().join("refresh");
    let mut last_refresh = std::fs::metadata(&refresh).and_then(|m| m.modified()).ok();
    antigravity::request_refresh();

    let mut tick = 0u64;
    loop {
        std::thread::sleep(Duration::from_millis(500));
        tick += 1;
        if hub.take_changed() {
            write_sessions(&hub);
        }
        // Stale sessions age out on the same 30 s beat as the Windows build.
        if tick % 60 == 0 {
            let changed = hub.store.lock().map(|mut st| st.sweep() | st.sweep_stuck(pid_alive)).unwrap_or(false);
            if changed {
                write_sessions(&hub);
            }
        }
        let now = std::fs::metadata(&refresh).and_then(|m| m.modified()).ok();
        if now.is_some() && now != last_refresh {
            last_refresh = now;
            antigravity::request_hover_refresh();
            cursor::request_refresh();
        }
    }
}
