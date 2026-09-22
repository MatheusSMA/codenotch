//! codenotch-hook: the minimal client Claude Code's hooks call.
//! Duties: 1) report the event plus stdin JSON to the main app; 2) launch the main app if it is not running.
//! Iron rule: never block Claude Code — ~2 s total budget, and every failure exits 0 silently.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const DEFAULT_PORT: u16 = 48666;
const MAX_STDIN: u64 = 256 * 1024;
/// The whole run, stdin included. Claude Code kills a hook that outlives its own timeout and
/// prints the failure to the user, so overrunning this is worse than losing one event.
const TOTAL_BUDGET: Duration = Duration::from_millis(2000);
/// How long to wait for the JSON on stdin. Claude Code writes it at once but does not always
/// close the pipe afterwards, so waiting for EOF can wait for ever.
const STDIN_BUDGET: Duration = Duration::from_millis(400);

fn main() {
    let started = Instant::now();
    let event = std::env::args().nth(1).unwrap_or_else(|| "ping".into());

    // The hook's stdin is the JSON Claude Code provides (session_id / cwd / prompt / message…)
    let body = read_with_deadline(std::io::stdin(), STDIN_BUDGET);

    let port = read_port();
    let ppid = parent_pid();

    if send(port, &event, ppid, &body).is_ok() {
        return;
    }
    // Main app not running: launch it detached, then retry until the budget runs out
    spawn_main();
    while started.elapsed() < TOTAL_BUDGET {
        std::thread::sleep(Duration::from_millis(100));
        if send(port, &event, ppid, &body).is_ok() {
            return;
        }
    }
    // Give up quietly — never affect Claude Code
}

/// Reads until EOF or the deadline, whichever comes first, and keeps whatever arrived.
/// The reader thread is left behind on a timeout: it dies with the process moments later.
fn read_with_deadline<R: Read + Send + 'static>(src: R, budget: Duration) -> String {
    let buf = Arc::new(Mutex::new(Vec::<u8>::new()));
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let sink = buf.clone();
    std::thread::spawn(move || {
        let mut src = src.take(MAX_STDIN);
        let mut chunk = [0u8; 8 * 1024];
        while let Ok(n) = src.read(&mut chunk) {
            if n == 0 {
                break;
            }
            match sink.lock() {
                Ok(mut b) => b.extend_from_slice(&chunk[..n]),
                Err(_) => return,
            }
        }
        let _ = tx.send(());
    });
    let _ = rx.recv_timeout(budget);
    // ponytail: a timeout can cut the JSON mid-object and the server then parses an empty event.
    // Stopping at a balanced closing brace would save those; worth it only if it shows up in practice.
    let bytes = buf.lock().map(|b| b.clone()).unwrap_or_default();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Pulls "port": N out of %APPDATA%\codenotch\config.json (hand-rolled scan, no dependency)
fn read_port() -> u16 {
    let path = match std::env::var("APPDATA") {
        Ok(a) => format!("{a}\\codenotch\\config.json"),
        Err(_) => return DEFAULT_PORT,
    };
    let Ok(txt) = std::fs::read_to_string(path) else {
        return DEFAULT_PORT;
    };
    if let Some(i) = txt.find("\"port\"") {
        let digits: String = txt[i + 6..]
            .chars()
            .skip_while(|c| !c.is_ascii_digit())
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if let Ok(p) = digits.parse() {
            return p;
        }
    }
    DEFAULT_PORT
}

fn send(port: u16, event: &str, ppid: u32, body: &str) -> std::io::Result<()> {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_millis(300))?;
    s.set_write_timeout(Some(Duration::from_millis(700)))?;
    s.set_read_timeout(Some(Duration::from_millis(700)))?;
    let req = format!(
        "POST /event?e={}&ppid={} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        event,
        ppid,
        body.len(),
        body
    );
    s.write_all(req.as_bytes())?;
    let mut buf = [0u8; 64];
    let _ = s.read(&mut buf); // wait for a response fragment to confirm delivery; failure does not matter
    Ok(())
}

/// Launches the main app detached: no inherited handles, no window, never waits
fn spawn_main() {
    let Ok(me) = std::env::current_exe() else { return };
    let Some(dir) = me.parent() else { return };
    // The native build ships beside this helper under its own name; the Tauri one is still
    // accepted so the same hook keeps working for either.
    let Some(exe) = ["codenotch-native.exe", "codenotch.exe"]
        .iter()
        .map(|n| dir.join(n))
        .find(|p| p.exists())
    else {
        return;
    };
    let mut cmd = std::process::Command::new(exe);
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW);
    }
    let _ = cmd.spawn();
}

/// Parent process PID (≈ the Claude Code CLI process) via NtQueryInformationProcess, no dependency
#[cfg(windows)]
fn parent_pid() -> u32 {
    #[repr(C)]
    struct Pbi {
        exit_status: isize,
        peb: usize,
        affinity_mask: usize,
        base_priority: isize,
        unique_process_id: usize,
        inherited_from_unique_process_id: usize,
    }
    extern "system" {
        fn NtQueryInformationProcess(
            handle: isize,
            class: u32,
            info: *mut Pbi,
            len: u32,
            ret_len: *mut u32,
        ) -> i32;
    }
    unsafe {
        let mut pbi = std::mem::zeroed::<Pbi>();
        let mut ret = 0u32;
        // -1 = GetCurrentProcess()
        if NtQueryInformationProcess(
            -1,
            0,
            &mut pbi,
            std::mem::size_of::<Pbi>() as u32,
            &mut ret,
        ) == 0
        {
            return pbi.inherited_from_unique_process_id as u32;
        }
    }
    0
}

#[cfg(not(windows))]
fn parent_pid() -> u32 {
    std::os::unix::process::parent_id()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hands over one chunk and then never returns, the way Claude Code's pipe behaves when it
    /// writes the event JSON and holds the write end open.
    struct StallAfterFirst(Option<&'static [u8]>);

    impl Read for StallAfterFirst {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            match self.0.take() {
                Some(data) => {
                    out[..data.len()].copy_from_slice(data);
                    Ok(data.len())
                }
                None => loop {
                    std::thread::sleep(Duration::from_millis(50));
                },
            }
        }
    }

    #[test]
    fn returns_what_arrived_when_the_pipe_never_closes() {
        let started = Instant::now();
        let body = read_with_deadline(
            StallAfterFirst(Some(b"{\"prompt\":\"oi\"}")),
            Duration::from_millis(200),
        );
        assert_eq!(body, "{\"prompt\":\"oi\"}");
        assert!(
            started.elapsed() < Duration::from_millis(1500),
            "blocked for {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn reads_everything_when_the_pipe_closes() {
        let body = read_with_deadline(
            std::io::Cursor::new(b"{\"session_id\":\"abc\"}".to_vec()),
            Duration::from_secs(5),
        );
        assert_eq!(body, "{\"session_id\":\"abc\"}");
    }
}
