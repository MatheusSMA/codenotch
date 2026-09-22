//! The Codenotch pill, drawn straight into a layered Win32 window.
//!
//! Why this exists: the Tauri build carries WebView2, which measured 415 MB across seven processes
//! for a strip that shows two percentages, and its working arc had to be capped to about 10 fps
//! because an SVG transform re-rasterises on the main thread. A layered window fed by
//! UpdateLayeredWindow hands the compositor a finished bitmap instead.
//!
//! This binary reads the snapshots the existing app persists (`usage.json`, `codex.json`) and owns
//! none of the fetching, so it stays additive: nothing upstream is edited and the two can run side
//! by side.

#![cfg_attr(not(test), windows_subsystem = "windows")]

mod activity;
mod codex;
mod glyphs;
mod hooks;
mod hooks_install;
mod paint;
mod state;
mod text;
mod usage;

use activity::Work;
use glyphs::Marks;
use hooks::Hub;
use paint::{band, Canvas};
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use text::Text;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_RBUTTON};
use windows::Win32::UI::WindowsAndMessaging::*;

// ---------------------------------------------------------------- design sizes
// Design pixels, matching notch.html so the two builds line up side by side. Everything is
// multiplied by the config's `scale` and the monitor's before it reaches the screen.
const PILL_W: f32 = 70.0;
const PAD_Y: f32 = 18.0;
const GAP: f32 = 14.0;
const RING_BOX: f32 = 56.0;
const TEXT_GAP: f32 = 6.0;
const RADIUS: f32 = 20.0;
/// Size of the percentage under each ring, from `.pct{font-size:15px}` in notch.html.
const PCT_PX: f32 = 15.0;
/// Side of the provider mark inside the ring, from `.glyph .mark` in notch.html.
const MARK: f32 = 26.0;
/// Radius of the concave arcs above and below the pill, from `--fillet` in notch.html.
const FILLET: f32 = 26.0;

// The detail panels, to the left of the pill. One per provider rather than a single card holding
// all of them: each provider is its own thing, and stacked blocks inside one box made the divisions
// hard to see. Wider and larger than notch.html's `#card`, which was sized for hover rather than
// for reading.
const CARD_W: f32 = 320.0;
const CARD_GAP: f32 = 12.0;
/// Breathing room to the left of the panel. Without it the window was exactly wide enough for
/// panel + gap + pill, so the panel started at x=0 and its own rounded corner and hairline were
/// clipped off by the edge of the canvas.
const CARD_MARGIN: f32 = 10.0;
const CARD_PAD: f32 = 18.0;
const CARD_RADIUS: f32 = 16.0;
const TITLE_PX: f32 = 16.0;
const ROW_PX: f32 = 13.0;
const TITLE_GAP: f32 = 12.0;
const ROW_GAP: f32 = 12.0;
/// A usage bar reads at a glance; a number has to be compared against a limit you have to remember.
const BAR_H: f32 = 7.0;
const BAR_GAP: f32 = 6.0;

const PEEK: f32 = 6.0; // how much stays on screen when tucked away
/// How far in from the edge the push that opens the pill is still read as a push. It is deliberately
/// much narrower than the area that keeps the pill out: reaching for the edge is a deliberate move,
/// while a pointer that merely passes near it is not asking for anything. Sized to `PEEK`, so the
/// push has to land on the sliver that is actually on screen. Widen it if the edge starts feeling
/// like it has to be hit exactly.
const REVEAL_REACH: f32 = PEEK;
const HOVER_PAD: f32 = 12.0;
/// Extra room the pointer gets before the pill decides it has left. Without it a single threshold
/// plus an unsteady hand is a switch being flicked: a pointer resting near the edge crosses it
/// constantly and the pill flaps. Sized from a real trace, where a resting hand wandered about
/// 70 px — a margin narrower than the wobble it is meant to absorb would not be a fix.
const HOVER_HYSTERESIS: f32 = 80.0;
/// And it has to stay outside for this long. Carried over from `LEAVE_MS` in the Tauri build, for
/// the same reason: a pointer crossing the boundary on its way somewhere else is not a decision.
const LEAVE_DWELL: Duration = Duration::from_millis(300);
const TICK: Duration = Duration::from_millis(16);
const POLL: Duration = Duration::from_secs(2);
/// One turn of the working arc, matching `spin 1.2s` in notch.html.
const SPIN_PERIOD: f32 = 1.2;
/// One breath of the attention ring, matching `pulse 1.1s`.
const PULSE_PERIOD: f32 = 1.1;
/// The hook port the Claude Code hooks already post to. Taking it means the original app cannot
/// run at the same time — which is the point: whoever holds it owns the session state.
const HOOK_PORT: u16 = 48666;

/// With CODENOTCH_TRACE set, every change of state is appended to `%TEMP%\codenotch-native.log`.
/// Poking at this window from outside is unreliable — it is per-monitor DPI aware while most tools
/// are not, so a cursor read from another process does not agree with what this one sees. Asking
/// the app what it thinks is the only account that means anything.
pub(crate) fn trace(line: &str) {
    if std::env::var_os("CODENOTCH_TRACE").is_none() {
        return;
    }
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(std::env::temp_dir().join("codenotch-native.log"))
    {
        let _ = writeln!(f, "{line}");
    }
}

// Spring timings, in the terms SwiftUI uses so the numbers can be compared with Apple's own.
// `duration` is the perceptual duration -- their docs call it "approximately equal to the settling
// duration" -- and `bounce` runs from 0 (critically damped, no overshoot) to 1 (undamped). Apple's
// default is duration 0.5, bounce 0; measured at 60 fps that is about 380 ms of visible movement.
//
// Arriving and leaving are not mirror images. Coming in is quick and allowed one small overshoot,
// because a panel that answers the pointer slowly feels unresponsive. Going away takes its time:
// nothing is waiting on it, and a dismissal that hurries reads as the thing being snatched away
// rather than leaving.
const REVEAL_DURATION: f32 = 0.34;
const REVEAL_BOUNCE: f32 = 0.28; // passes the target once and settles
const DISMISS_DURATION: f32 = 0.95; // about 665 ms visible, comfortably past Apple's default
const DISMISS_BOUNCE: f32 = 0.04; // all but critically damped: no wobble on the way out
/// The panel's own box closes slower than a swap and without any bounce: it is the thing being
/// read, so it is not snatched away.
///
/// It was 1.1 — longer than the pill's own exit, on the reasoning that the panel should be the last
/// to go. Reported as stuck, and it was: the pill cannot start retracting until the panel is nearly
/// gone, so leaving ran 1.1 then 0.95 end to end, close to two seconds, with the middle of it spent
/// on a fade already too faint to see. It now hands over to the pill while it is still faintly on
/// screen, and the two read as one departure.
const PANEL_CLOSE_DURATION: f32 = 0.55;
/// Closing on the way to another panel is a different move from closing for good. Nobody is waiting
/// on a dismissal, but during a swap the next panel is what you asked for, so the shut half has to
/// get out of the way.
const PANEL_SWAP_DURATION: f32 = 0.3;
/// The panel is considered gone at this much rather than at nothing. A spring approaches zero
/// exponentially, so the last sliver costs more time than all the rest of the close — and that
/// sliver is what read as the swap hanging just before the other panel arrived.
const PANEL_GONE: f32 = 0.06;
/// How much of its speed the panel keeps through a swap. Zeroing it made the next panel start from
/// a standstill, which broke one movement into two.
const SWAP_CARRY: f32 = 0.55;
/// While a panel is still on screen the pill stays out, however the pointer left. The panel is
/// drawn inside the window, so retracting first does not animate it away — it drags it off the
/// edge, and what the eye sees is the panel being cut rather than closing.
///
/// Where this sits decides whether leaving is one move or two. At 0.04 the pill waited out all but
/// the last invisible sliver of the fade, which left a dead beat — panel already gone, pill not yet
/// moving — and that beat is what read as leaving being stuck. Handing over at 0.12, faint but not
/// yet nothing, overlaps them.
const PANEL_LINGER: f32 = 0.12;

/// The sliver left at the edge is faint rather than invisible: it has to be findable, or the hover
/// target is a secret. Raise it if it needs to be more obvious, drop it to 0.0 to hide it outright.
const HIDDEN_ALPHA: f32 = 0.35;
/// How small the rings start before settling to full size. Subtle on purpose — a panel that visibly
/// zooms reads as a phone app, not as part of the desktop.
const CONTENT_MIN_SCALE: f32 = 0.93;

/// Pure white. notch.html uses #e8e8ea, which next to a black pill reads as grey.
const INK: [f32; 3] = [1.0, 1.0, 1.0];
const MUTED: [f32; 3] = [0.62, 0.62, 0.65];
/// Amber, for a session waiting on an answer — the same cue `.arc-pulse` carries in notch.html.
const WATCH: [f32; 3] = [0.98, 0.80, 0.08];
const PILL_BG: [f32; 3] = [0.0, 0.0, 0.0];
const CARD_BG: [f32; 3] = [0.04, 0.04, 0.04];
const PILL_EDGE: [f32; 3] = [0.18, 0.18, 0.18];
const TRACK: [f32; 3] = [0.16, 0.16, 0.16];
const DISC: [f32; 3] = [0.165, 0.165, 0.165];

// ---------------------------------------------------------------- persisted data

#[derive(Deserialize)]
struct Window_ {
    id: String,
    #[serde(default)]
    label: String,
    used: Option<f32>,
    #[serde(default)]
    count: Option<i64>,
}

#[derive(Deserialize)]
struct Snapshot {
    #[serde(default)]
    status: String,
    #[serde(default)]
    windows: Vec<Window_>,
    #[serde(default)]
    fetched_at: i64,
}

/// What one cell shows: the headline percentage, plus every window for the detail card.
struct Reading {
    used: Option<f32>,
    /// A reading that is still worth showing but is no longer fresh. notch.html draws these at
    /// `.dim{opacity:.55}` rather than hiding them, so a number the user saw a minute ago does not
    /// turn into a dash the moment a refresh is late.
    stale: bool,
    rows: Vec<(String, Option<f32>)>,
    work: Work,
    /// Live sessions, when hook events are reaching us. Empty for providers with no hooks.
    sessions: Vec<(String, String)>,
}

/// Five minutes, the window `staleOf` in notch.html uses before it stops trusting a reading.
const STALE_AFTER_MS: i64 = 5 * 60 * 1000;

pub(crate) fn data_dir() -> PathBuf {
    dirs::config_dir().unwrap_or_default().join("codenotch")
}

/// What the hook events say, when we are the ones receiving them. `None` means the session table
/// is empty — nothing has reported in — and the caller should fall back to watching the files.
fn work_from_hooks(hub: &Hub) -> Option<(Work, Vec<(String, String)>)> {
    let store = hub.store.lock().ok()?;
    let snap = store.snapshot("en", "en", true, false);
    if snap.sessions.is_empty() {
        return None;
    }
    let work = match snap.agg.as_str() {
        state::ST_ATTENTION => Work::Attention,
        state::ST_RUNNING => Work::Running,
        _ => Work::Idle,
    };
    // The card lists what is actually going on, most urgent first — `snapshot` already sorted them.
    let sessions = snap
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
            (title, what)
        })
        .collect();
    Some((work, sessions))
}

fn read_provider(provider: &str, hub: &Hub) -> Reading {
    let file = match provider {
        "codex" => "codex.json",
        "cursor" => "cursor.json",
        "antigravity" => "antigravity.json",
        _ => "usage.json",
    };
    let mut r = match std::fs::read_to_string(data_dir().join(file))
        .ok()
        .and_then(|t| serde_json::from_str::<Snapshot>(&t).ok())
    {
        Some(snap) => reading_of(&snap, now_ms()),
        None => Reading { used: None, stale: false, rows: Vec::new(), work: Work::Idle, sessions: Vec::new() },
    };
    // Hook events are the authority — they are the only thing that can say "waiting on you". The
    // file scan is the fallback for a provider that never reports, or before the first event lands.
    match (provider == "claude").then(|| work_from_hooks(hub)).flatten() {
        Some((work, sessions)) => {
            r.work = work;
            r.sessions = sessions;
        }
        None => r.work = activity::probe(provider),
    }
    r
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Split out so the status rules can be tested without touching the disk.
fn reading_of(snap: &Snapshot, now: i64) -> Reading {
    // `needsAuth` and `none` mean there is no number to show; `stale` means show it dimmed.
    if snap.status == "needsAuth" || snap.status == "none" {
        return Reading { used: None, stale: false, rows: Vec::new(), work: Work::Idle, sessions: Vec::new() };
    }
    // The headline is the first reading expressed as a fraction rather than a count. Claude calls
    // it `session`, Codex calls it `primary`, so the shape decides rather than the name.
    let head = snap
        .windows
        .iter()
        .find(|w| w.count.is_none() && (w.id == "session" || w.id == "primary"))
        .or_else(|| snap.windows.iter().find(|w| w.count.is_none()));
    let aged = snap.fetched_at > 0 && now - snap.fetched_at > STALE_AFTER_MS;
    Reading {
        used: head.and_then(|w| w.used),
        stale: snap.status == "stale" || aged,
        rows: snap
            .windows
            .iter()
            .map(|w| {
                let label = if w.label.is_empty() { w.id.clone() } else { w.label.clone() };
                (label, if w.count.is_none() { w.used } else { None })
            })
            .collect(),
        work: Work::Idle,
        sessions: Vec::new(),
    }
}

#[derive(Deserialize, Default)]
struct Slot {
    provider: String,
}

#[derive(Deserialize)]
struct Cfg {
    #[serde(default = "one")]
    scale: f32,
    #[serde(default = "half")]
    notch_y: f32,
    #[serde(default)]
    notch_slots: Vec<Slot>,
}

fn one() -> f32 {
    1.0
}
fn half() -> f32 {
    0.5
}

impl Default for Cfg {
    fn default() -> Self {
        Cfg { scale: 1.0, notch_y: 0.5, notch_slots: Vec::new() }
    }
}

impl Cfg {
    fn load() -> Cfg {
        std::fs::read_to_string(data_dir().join("config.json"))
            .ok()
            .and_then(|t| serde_json::from_str::<Cfg>(&t).ok())
            .map(|mut c| {
                // The app snaps scale to one of three sizes; anything else is a stale hand edit.
                c.scale = if c.scale < 0.9 { 0.8 } else if c.scale < 1.125 { 1.0 } else { 1.25 };
                c
            })
            .unwrap_or_default()
    }

    /// An empty slot list means every provider, which is the rule notch.html documents.
    fn providers(&self) -> Vec<&'static str> {
        let known = ["claude", "codex", "cursor", "antigravity"];
        if self.notch_slots.is_empty() {
            return known.to_vec();
        }
        self.notch_slots
            .iter()
            .filter_map(|s| known.iter().find(|k| **k == s.provider).copied())
            .collect()
    }
}

fn pretty(provider: &str) -> &'static str {
    match provider {
        "claude" => "Claude",
        "codex" => "Codex",
        "cursor" => "Cursor",
        "antigravity" => "Antigravity",
        _ => "Unknown",
    }
}

// ---------------------------------------------------------------- animation

/// A mass-spring-damper, integrated per frame, driving one number: how open the pill is, where 0 is
/// tucked at the edge and 1 is fully out.
///
/// Why this instead of an easing curve: a curve is a function of elapsed time, so interrupting one
/// means restarting from zero velocity. Pull the pointer away half-way through and the pill stops
/// dead before turning round. A spring carries its velocity through a target change, so a reversal
/// keeps the momentum it already had.
///
/// Parameterised exactly as SwiftUI's `spring(duration:bounce:)`, so the constants above mean what
/// Apple's mean and can be read against their presets. `duration` is the perceptual duration, which
/// their documentation describes as approximately the settling duration; `bounce` is 0 for a
/// critically damped spring up to 1 for an undamped one, and the damping fraction is `1 - bounce`.
struct Spring {
    value: f32,
    vel: f32,
    duration: f32,
    bounce: f32,
}

impl Spring {
    fn new(value: f32, duration: f32, bounce: f32) -> Spring {
        Spring { value, vel: 0.0, duration, bounce }
    }

    fn step(&mut self, target: f32, dt: f32) {
        use std::f32::consts::PI;
        let zeta = 1.0 - self.bounce;
        let k = (2.0 * PI / self.duration).powi(2); // stiffness, unit mass
        let c = 4.0 * PI * zeta / self.duration;
        // Substepped: one 16 ms leap with a stiff spring can integrate its way out of the screen.
        const SUB: usize = 8;
        let h = (dt / SUB as f32).min(0.004);
        for _ in 0..SUB {
            let accel = -k * (self.value - target) - c * self.vel;
            self.vel += accel * h;
            self.value += self.vel * h;
        }
    }

    fn settled(&self, target: f32) -> bool {
        (self.value - target).abs() < 0.0005 && self.vel.abs() < 0.005
    }
}

fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

/// Opacity and content scale, both read off how open the pill is.
///
/// The fade runs ahead of the movement on the way in: at a third of the way out the pill is already
/// almost solid, because matching opacity to position one-for-one makes an entrance feel like it is
/// dragging something heavy. On the way out that same curve is wrong — it holds full opacity for
/// most of the trip and then drops all at once, which is the part that reads as being snatched
/// away. Leaving fades evenly instead, so the pill is still visible while it travels.
fn skin(openness: f32, leaving: bool) -> (f32, f32) {
    let o = openness.clamp(0.0, 1.5);
    let fade = if leaving { o.min(1.0) } else { (o * 1.7).clamp(0.0, 1.0) };
    let alpha = HIDDEN_ALPHA + (1.0 - HIDDEN_ALPHA) * ease_out(fade);
    let scale = CONTENT_MIN_SCALE + (1.0 - CONTENT_MIN_SCALE) * o.min(1.06);
    (alpha, scale)
}

// ---------------------------------------------------------------- layout

struct Layout {
    scale: f32,
    /// Full window: the panels' column plus the pill's.
    w: i32,
    h: i32,
    /// Left edge of the pill within the window.
    pill_left: f32,
    /// Top of the pill body inside the window, and its height. The pill is centred vertically,
    /// because the panel stack beside it can be taller than the pill is.
    pill_top: f32,
    pill_h: f32,
    cell_h: f32,
    pct_px: f32,
}

impl Layout {
    /// `panels_h` is how tall the open panel stack needs to be, in scaled pixels. The window has to
    /// cover whichever is taller, the pill or the panels, or the panels would be clipped.
    fn new(scale: f32, cells: usize, panels_h: f32) -> Layout {
        let cells = cells.max(1) as f32;
        let cell_h = RING_BOX + TEXT_GAP + PCT_PX;
        let pill_h = (PAD_Y * 2.0 + cells * cell_h + (cells - 1.0) * GAP) * scale;
        // The fillets live outside the pill at both ends, so the window is taller than the pill by
        // one fillet radius above and below.
        let pill_block = pill_h + FILLET * 2.0 * scale;
        let h = pill_block.max(panels_h);
        let w = CARD_MARGIN + CARD_W + CARD_GAP + PILL_W + 4.0;
        Layout {
            scale,
            w: (w * scale).round() as i32,
            h: h.round() as i32,
            pill_left: (CARD_MARGIN + CARD_W + CARD_GAP) * scale,
            // Both rounded. With a fractional height the top edge lands on one subpixel phase and
            // the bottom on another, and the two fillets come out visibly different shapes.
            pill_top: ((h - pill_h) / 2.0).round(),
            pill_h: pill_h.round(),
            cell_h,
            pct_px: (PCT_PX * scale).max(6.0),
        }
    }
}

impl Layout {
    /// Which ring the pointer is over, given a y in window coordinates. The rows are the pill's
    /// own cells, so a click lands on the provider it looks like it landed on.
    fn cell_at(&self, y: f32, cells: usize) -> Option<usize> {
        let step = (self.cell_h + GAP) * self.scale;
        let first = self.pill_top + PAD_Y * self.scale;
        if y < first {
            return None;
        }
        let i = ((y - first) / step).floor() as usize;
        // The gap between two cells belongs to neither, or a click between rings would open one
        // of them at random.
        let within = (y - first) - i as f32 * step;
        (i < cells && within <= self.cell_h * self.scale).then_some(i)
    }
}

/// What a click on ring `hit` does to the open panel. Clicking the open ring shuts it, a different
/// ring swaps to that one, and a click that landed between rings leaves things as they were.
fn next_panel(current: Option<usize>, hit: Option<usize>) -> Option<usize> {
    match (current, hit) {
        (Some(open), Some(i)) if open == i => None,
        (_, Some(i)) => Some(i),
        (open, None) => open,
    }
}

/// Where the panel spring is headed. Open only when what is drawn is what was asked for: during a
/// swap the two differ, and the panel shuts on its way to the other one.
fn card_target(shown: Option<usize>, want: Option<usize>) -> f32 {
    if shown.is_some() && shown == want { 1.0 } else { 0.0 }
}

/// The speed the card spring keeps once its contents have been swapped out under it.
///
/// A swap reflects it, so the panel that was asked for starts already moving and the pair reads as
/// one gesture bouncing off the bottom. A dismissal must not, and used to: the same branch runs for
/// `Some -> None`, so closing for good turned round and climbed back up with nothing left to draw.
/// `pill_target` holds the pill out for as long as the card is above `PANEL_LINGER`, so that climb
/// was paid for as a stall on an empty panel column before the pill could start retracting.
fn carry_through(want: Option<usize>, vel: f32) -> f32 {
    if want.is_some() { -vel * SWAP_CARRY } else { 0.0 }
}

/// How far along a staggered item is, given the panel's own progress. Rows arrive one after the
/// other rather than all at once — the "per object" sequencing in Apple's own motion work — and
/// each one eases out on its own. `i` is the row's place in the order, `n` how many there are.
fn stagger(progress: f32, i: usize, n: usize) -> f32 {
    if n <= 1 {
        return progress.clamp(0.0, 1.0);
    }
    // The last row still finishes with the panel: the lead shrinks as the list grows, so a long
    // list does not trail off past the animation it belongs to.
    const LEAD: f32 = 0.45;
    let step = LEAD / (n - 1) as f32;
    let start = i as f32 * step;
    ease_out(((progress - start) / (1.0 - LEAD)).clamp(0.0, 1.0))
}

/// Where the pill spring is headed. Out while the pointer is on it, and for as long as a panel is
/// still on screen — the panel lives inside this window, so retracting first carries it off the
/// edge instead of letting it close.
fn pill_target(pointer_inside: bool, card: f32) -> f32 {
    if pointer_inside || card > PANEL_LINGER { 1.0 } else { 0.0 }
}

fn pct_label(used: Option<f32>) -> String {
    match used {
        None => "-".to_string(),
        Some(u) => format!("{}%", (u * 100.0).round().clamp(0.0, 100.0) as u32),
    }
}

// ---------------------------------------------------------------- drawing

struct Frame<'a> {
    lay: &'a Layout,
    readings: &'a [(&'a str, Reading)],
    /// Content scale, 0.93 to 1.0, riding the reveal spring.
    cs: f32,
    /// How open the detail panel is, 0 to 1.
    card: f32,
    /// Which provider's panel it is. One at a time: the panel belongs to the ring you clicked.
    panel: Option<usize>,
    /// Seconds since start, for the working arc.
    t: f32,
}

fn render(c: &mut Canvas, f: &Frame, marks: &mut Marks, font: &mut Text) {
    c.clear();
    let (lay, s) = (f.lay, f.lay.scale);
    let cx = lay.w as f32 - (PILL_W / 2.0) * s;
    let mid = lay.h as f32 / 2.0;
    let toward_mid = |y: f32| mid + (y - mid) * f.cs;

    if f.card > 0.01 {
        draw_panels(c, f, font);
    }

    // The pill body, pushed right by its radius so only the left corners round, as on screen.
    let pill_top = lay.pill_top;
    let pill_bot = lay.pill_top + lay.pill_h;
    c.round_rect(
        cx + RADIUS * s,
        (pill_top + pill_bot) / 2.0,
        (PILL_W / 2.0 + RADIUS) * s,
        (pill_bot - pill_top) / 2.0,
        RADIUS * s,
        PILL_BG,
        Some(PILL_EDGE),
        1.0,
    );

    // The fillets: concave quarter-arcs joining the pill to the screen edge above and below, so the
    // shape reads as carved out of the edge rather than floating in front of it.
    c.fillet(lay.w as f32, pill_top, FILLET * s, true, PILL_BG, PILL_EDGE);
    c.fillet(lay.w as f32, pill_bot, FILLET * s, false, PILL_BG, PILL_EDGE);

    let mut top = pill_top + PAD_Y * s;
    for (provider, r) in f.readings {
        let ring_cy = toward_mid(top + (RING_BOX / 2.0) * s);
        // A reading that is merely old still shows, at the same .55 the web build dims it to
        let a = if r.stale { 0.55 } else { 1.0 };
        c.arc(cx, ring_cy, 11.0 * s * f.cs, 22.0 * s * f.cs, DISC, 1.0, 0.0, 1.0);
        c.arc(cx, ring_cy, 25.0 * s * f.cs, 5.0 * s * f.cs, TRACK, 1.0, 0.0, 1.0);
        if let Some(u) = r.used {
            let frac = u.clamp(0.0, 1.0);
            if frac > 0.0 {
                c.arc(cx, ring_cy, 25.0 * s * f.cs, 5.0 * s * f.cs, band(u), a, 0.0, frac);
            }
        }

        // The inner ring says what the session is doing: a short arc turning while a turn is in
        // progress, a whole ring breathing amber while it waits on an answer. Same 28 % sweep,
        // 1.2 s turn and 1.1 s breath as `.arc-spin` and `.arc-pulse` in notch.html.
        match r.work {
            Work::Running => {
                let turn = (f.t / SPIN_PERIOD).fract();
                c.arc(cx, ring_cy, 19.0 * s * f.cs, 2.5 * s * f.cs, INK, 0.95, turn, turn + 0.28);
            }
            Work::Attention => {
                let phase = (f.t / PULSE_PERIOD * std::f32::consts::TAU).sin() * 0.5 + 0.5;
                c.arc(cx, ring_cy, 19.0 * s * f.cs, 2.5 * s * f.cs, WATCH, 0.25 + 0.75 * phase, 0.0, 1.0);
            }
            Work::Idle => {}
        }

        let msize = (MARK * s * f.cs).round().max(1.0) as u32;
        if let Some(m) = marks.get(provider, msize) {
            let half = m.size as f32 / 2.0;
            c.mask(&m.alpha, m.size as i32, (cx - half).round() as i32, (ring_cy - half).round() as i32, INK, a);
        }

        // Baseline rather than top edge: that is the line real type sits on.
        let baseline = toward_mid(top + (RING_BOX + TEXT_GAP) * s + lay.pct_px);
        font.centred(c, &pct_label(r.used), cx, baseline, lay.pct_px, INK, a);
        top += (lay.cell_h + GAP) * s;
    }
}

/// Height one provider's panel needs, in scaled pixels.
fn panel_height(lay: &Layout, r: &Reading) -> f32 {
    let s = lay.scale;
    let rows = r.rows.len().max(1) as f32;
    let mut h = CARD_PAD * 2.0 * s;
    h += TITLE_PX * s + TITLE_GAP * s;
    // Each limit is a label line and the bar under it.
    h += rows * (ROW_PX + BAR_GAP + BAR_H) * s + (rows - 1.0) * ROW_GAP * s;
    if !r.sessions.is_empty() {
        h += ROW_GAP * s;
        h += r.sessions.len() as f32 * ROW_PX * s + (r.sessions.len() as f32 - 1.0) * ROW_GAP * 0.5 * s;
    }
    h
}

/// The tallest a single panel can get, so the window is big enough whichever ring is clicked.
/// Only one is ever shown, so this is a maximum rather than a sum.
fn panels_height(lay: &Layout, readings: &[(&str, Reading)]) -> f32 {
    readings.iter().map(|(_, r)| panel_height(lay, r)).fold(0.0, f32::max)
}

/// A usage bar: a dark track with the used fraction filled in its band colour. Reads at a glance,
/// which a percentage does not — 47 % means nothing until you recall what the limit was.
fn draw_bar(c: &mut Canvas, left: f32, top: f32, w: f32, h: f32, used: Option<f32>, a: f32) {
    let r = h / 2.0;
    // `a` is the panel's fade. It used to be ignored here, so while the panel faded in the bars
    // were already at full strength — they arrived before the box around them.
    c.round_rect(left + w / 2.0, top + r, w / 2.0, r, r, TRACK, None, a);
    let Some(u) = used else { return };
    let frac = u.clamp(0.0, 1.0);
    if frac <= 0.0 {
        return;
    }
    // Never thinner than its own cap, or a reading of 1 % draws a wedge instead of a dot.
    let fill = (w * frac).max(h);
    c.round_rect(left + fill / 2.0, top + r, fill / 2.0, r, r, band(u), None, a);
}

/// The panel of whichever ring was clicked. One at a time: two panels side by side turned the
/// glance into a search, and a provider's detail belongs to that provider's ring.
fn draw_panels(c: &mut Canvas, f: &Frame, font: &mut Text) {
    let (lay, s) = (f.lay, f.lay.scale);
    let Some(idx) = f.panel else { return };
    let Some((provider, r)) = f.readings.get(idx) else { return };
    let w = CARD_W * s;
    // It slides the last few pixels in as it fades, so it arrives rather than blinking on.
    let left = lay.pill_left - CARD_GAP * s - w + (1.0 - f.card) * 12.0 * s;
    let a = f.card;

    {
        let h = panel_height(lay, r);
        // Centred on its own ring rather than on the window: the panel points at what opened it.
        let ring_mid = lay.pill_top + PAD_Y * s + idx as f32 * (lay.cell_h + GAP) * s + RING_BOX / 2.0 * s;
        let top = (ring_mid - h / 2.0).clamp(0.0, (lay.h as f32 - h).max(0.0));
        c.round_rect(left + w / 2.0, top + h / 2.0, w / 2.0, h / 2.0, CARD_RADIUS * s, CARD_BG, Some(PILL_EDGE), a);

        let text_left = left + CARD_PAD * s;
        let text_right = left + w - CARD_PAD * s;
        let inner_w = text_right - text_left;
        let mut y = top + CARD_PAD * s + TITLE_PX * s;

        font.left_aligned(c, pretty(provider), text_left, y, TITLE_PX * s, INK, a);
        let tag = match r.work {
            Work::Attention => Some(("waiting on you", WATCH)),
            Work::Running => Some(("working", INK)),
            Work::Idle if r.stale => Some(("stale", MUTED)),
            Work::Idle => None,
        };
        if let Some((tag, tone)) = tag {
            let tw = font.width(tag, ROW_PX * s);
            font.left_aligned(c, tag, text_right - tw, y, ROW_PX * s, tone, a);
        }
        y += TITLE_GAP * s;

        if r.rows.is_empty() {
            y += ROW_PX * s;
            font.left_aligned(c, "no reading", text_left, y, ROW_PX * s, MUTED, a);
        }
        // Rows arrive in order rather than together, each easing out on its own, and each slides
        // the last few pixels up into place as it fades.
        let total_rows = r.rows.len() + r.sessions.len();
        for (n, (label, used)) in r.rows.iter().enumerate() {
            if n > 0 {
                y += ROW_GAP * s;
            }
            y += ROW_PX * s;
            // `ry` is where this row draws, `y` stays the layout cursor the loop advances.
            let t = stagger(f.card, n, total_rows);
            let a = a * t;
            let ry = y + (1.0 - t) * 6.0 * s;
            // The percentage still sits at the end of the label line: the bar carries the shape,
            // the number is there when the exact value matters.
            let value = pct_label(*used);
            let vw = font.width(&value, ROW_PX * s);
            let room = (inner_w - vw - 10.0 * s).max(10.0);
            let label = font.elide(label, ROW_PX * s, room);
            font.left_aligned(c, &label, text_left, ry, ROW_PX * s, MUTED, a);
            font.left_aligned(c, &value, text_right - vw, ry, ROW_PX * s, MUTED, a);
            y += BAR_GAP * s;
            draw_bar(c, text_left, ry + BAR_GAP * s, inner_w, BAR_H * s, *used, a);
            y += BAR_H * s;
        }

        // Live sessions below the limits: what is running, and what is waiting on an answer.
        if !r.sessions.is_empty() {
            y += ROW_GAP * s;
            for (n, (title, what)) in r.sessions.iter().enumerate() {
                if n > 0 {
                    y += ROW_GAP * 0.5 * s;
                }
                y += ROW_PX * s;
                let t = stagger(f.card, r.rows.len() + n, total_rows);
                let a = a * t;
                let ry = y + (1.0 - t) * 6.0 * s;
                let head = font.elide(title, ROW_PX * s, inner_w * 0.45);
                let pen = font.left_aligned(c, &head, text_left, ry, ROW_PX * s, INK, a * 0.9);
                let room = text_right - pen - 8.0 * s;
                if room > 12.0 * s {
                    let detail = font.elide(what, ROW_PX * s, room);
                    let dw = font.width(&detail, ROW_PX * s);
                    font.left_aligned(c, &detail, text_right - dw, ry, ROW_PX * s, MUTED, a);
                }
            }
        }

    }
}

// ---------------------------------------------------------------- window

unsafe extern "system" fn wndproc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if m == WM_DESTROY {
        PostQuitMessage(0);
        return LRESULT(0);
    }
    DefWindowProcW(h, m, w, l)
}

/// Bounds of the monitor the notch lives on (the primary one, where the app places it) and its
/// scale factor. The process is per-monitor DPI aware, so these are physical pixels: the design
/// sizes have to be multiplied by the scale factor or the pill comes out 1/1.5 too small on a
/// 150 % display. The Tauri build multiplies by its monitor scale factor for the same reason.
unsafe fn primary_monitor() -> (RECT, f32) {
    let mon = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
    let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    let rect = if GetMonitorInfoW(mon, &mut info).as_bool() {
        info.rcMonitor
    } else {
        RECT { left: 0, top: 0, right: 1920, bottom: 1080 }
    };
    let (mut dx, mut dy) = (96u32, 96u32);
    let dpi = if GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy).is_ok() {
        dx as f32 / 96.0
    } else {
        1.0
    };
    (rect, dpi)
}

/// Whether a process id is still running. Used to drop sessions whose owner is gone: a chat that
/// asked a question and was then killed cannot still be waiting for the answer.
fn pid_alive(pid: u32) -> bool {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    unsafe {
        match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
            Ok(h) => {
                let _ = CloseHandle(h);
                true
            }
            // Access denied means it exists but belongs to someone else; only a missing process
            // counts as gone, or an elevated session would be dropped the moment it appeared.
            Err(e) => e.code().0 as u32 == 0x8007_0005,
        }
    }
}

/// Most of this window is transparent. Without this, every click in the card's empty column would
/// land here instead of on whatever is behind it, so the window is click-through except when the
/// pointer is actually over something solid.
unsafe fn set_click_through(hwnd: HWND, on: bool) {
    let cur = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
    let bit = WS_EX_TRANSPARENT.0 as isize;
    let next = if on { cur | bit } else { cur & !bit };
    if next != cur {
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, next);
    }
}

/// There is no tray icon and no settings window, so this menu is the only handle on the process
/// itself: the way to pick up a new build, and the way to stop it.
const MENU_RESTART: usize = 1;
const MENU_QUIT: usize = 2;

/// Opens it at the pointer and returns what was picked, or 0 if it was dismissed.
///
/// The calls around `TrackPopupMenu` are the documented pair rather than superstition. A popup menu
/// only closes on a click elsewhere when its owner is the foreground window, and this one is
/// `WS_EX_NOACTIVATE` precisely so that hovering the pill never steals focus — so the style comes
/// off for as long as the menu is up and goes straight back on. Right-aligned because the pill sits
/// against the right edge of the screen, where a left-aligned menu would open off it.
unsafe fn pill_menu(hwnd: HWND, at: POINT) -> usize {
    let Ok(menu) = CreatePopupMenu() else { return 0 };
    let _ = AppendMenuW(menu, MF_STRING, MENU_RESTART, w!("Restart"));
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    let _ = AppendMenuW(menu, MF_STRING, MENU_QUIT, w!("Quit"));

    let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
    SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex & !(WS_EX_NOACTIVATE.0 as isize));
    let _ = SetForegroundWindow(hwnd);
    let picked = TrackPopupMenu(
        menu,
        TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY | TPM_RIGHTALIGN,
        at.x,
        at.y,
        0,
        hwnd,
        None,
    );
    let _ = PostMessageW(hwnd, WM_NULL, WPARAM(0), LPARAM(0));
    SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex);
    let _ = DestroyMenu(menu);
    picked.0 as usize
}

fn main() {
    unsafe {
        // Without this the monitor rectangle comes back in scaled coordinates and the pill lands
        // short of the edge on any display that is not at 100 %.
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);

        let hinst = windows::Win32::System::LibraryLoader::GetModuleHandleW(None).unwrap();
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinst.into(),
            lpszClassName: w!("CodenotchNative"),
            ..Default::default()
        });

        let mut cfg = Cfg::load();
        let mut providers = cfg.providers();
        let (mon, dpi) = primary_monitor();
        // The config scale is the user's size preference; the monitor scale converts design pixels
        // to this display's physical ones. Both multiply.
        // A first pass with no panels, only so there is a Layout to measure them against; the
        // real height lands on the next line, once the readings are known.
        let mut lay = Layout::new(cfg.scale * dpi, providers.len(), 0.0);
        let mut hidden_x = mon.right - (PEEK * cfg.scale * dpi).round() as i32;
        let y0 = mon.top + ((mon.bottom - mon.top) as f32 * cfg.notch_y - lay.h as f32 / 2.0).round() as i32;

        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT,
            w!("CodenotchNative"),
            w!("Codenotch"),
            WS_POPUP,
            hidden_x,
            y0,
            lay.w,
            lay.h,
            None,
            None,
            hinst,
            None,
        )
        .unwrap();
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);

        let screen = GetDC(None);
        let memdc = CreateCompatibleDC(screen);
        let mut canvas = Canvas::new(lay.w, lay.h);
        let mut marks = Marks::default();
        // Without a usable UI font there is nothing to print a percentage with; the pill would be
        // rings and no numbers, so this is a hard stop rather than a silent half-drawn panel.
        let Some(mut font) = Text::system() else { return };

        // Taking the hook port is what makes "waiting on you" possible: those events are the only
        // thing that can tell a finished turn from one waiting for an answer. If the bind fails the
        // original app still holds it, and the pill falls back to inferring from the transcripts —
        // it keeps working, it just cannot show amber.
        let hub = Arc::new(Hub::default());
        let owns_hooks = hooks::start(hub.clone(), HOOK_PORT);
        // Losing the bind means a second instance: `start` already retried for two seconds, which
        // covers a restart from the pill's own menu inheriting the port from the outgoing process.
        // The hook spawns this binary whenever its POST cannot connect — including while the first
        // one is still booting — so without this the machine collects pills that never hear an
        // event and shadow the real one at the edge.
        if !owns_hooks {
            trace("second instance: the hook port is taken, leaving");
            return;
        }
        // Wire Claude Code's hooks at startup if they are not already pointing here. Without them
        // nothing posts to the port above, and `attention` can never fire: it is the one state with
        // no signature on disk. Installing is idempotent and backs the settings file up first.
        if !hooks_install::is_installed() {
            match hooks_install::install() {
                Ok(msg) => trace(&format!("hooks installed: {msg}")),
                Err(e) => trace(&format!("hooks not installed: {e}")),
            }
        }
        // The usage fetcher. It persists usage.json, which is what the pill reads, so taking the
        // port stops meaning frozen numbers: this binary now refreshes them itself.
        hub.usage.lock().map(|mut u| *u = usage::load_persisted()).ok();
        usage::start(hub.clone());
        hub.codex.lock().map(|mut u| *u = codex::load_persisted()).ok();
        codex::start(hub.clone());
        // Stale session cleanup, on the same 30 s beat the Tauri build uses. Without it the table
        // only ever grows: a running session never falls back to idle, a finished one is never
        // removed, and a session stuck on attention paints the ring amber for ever.
        {
            let hub = hub.clone();
            std::thread::spawn(move || loop {
                std::thread::sleep(Duration::from_secs(30));
                let changed = {
                    let Ok(mut store) = hub.store.lock() else { continue };
                    let a = store.sweep();
                    let b = store.sweep_stuck(pid_alive);
                    a || b
                };
                if changed {
                    hub.mark_changed();
                }
            });
        }


        let mut readings: Vec<(&str, Reading)> = providers.iter().map(|p| (*p, read_provider(p, &hub))).collect();
        lay = Layout::new(cfg.scale * dpi, providers.len(), panels_height(&lay, &readings));
        let mut shown_x = mon.right - lay.w;
        let mut y = mon.top + ((mon.bottom - mon.top) as f32 * cfg.notch_y - lay.h as f32 / 2.0).round() as i32;
        trace(&format!(
            "start owns_hooks={owns_hooks} window={}x{} pill_left={:.0} hidden={hidden_x} shown={shown_x} y={y}",
            lay.w, lay.h, lay.pill_left
        ));
        let mut last_poll = Instant::now();

        // One spring drives the reveal, a second the card, so the two can be at different points
        // without fighting each other.
        let mut open = Spring::new(0.0, REVEAL_DURATION, REVEAL_BOUNCE);
        let mut card = Spring::new(0.0, REVEAL_DURATION, 0.0);
        let mut shown = false;
        // Which ring's panel is out, if any.
        // What the pointer asked for, and what is actually drawn. They differ only while one panel
        // is closing to let another open — a swap is a close and an open, not a jump cut.
        let mut panel_want: Option<usize> = None;
        let mut panel_shown: Option<usize> = None;
        let mut was_down = false;
        let mut was_right = false;
        // When the pointer first went outside, or None while it is inside.
        let mut left_at: Option<Instant> = None;
        // Where the pointer was last frame, and whether it was already in the edge strip: the
        // reveal triggers on the crossing, so it needs both.
        let mut was_at_edge = false;
        let mut prev_x = i32::MAX;
        let mut click_through = true;
        let mut dirty = true;
        let mut pos = hidden_x as f32;
        let mut alpha = HIDDEN_ALPHA;
        let mut drawn_scale = CONTENT_MIN_SCALE;
        let mut drawn_card = 0.0f32;
        let start = Instant::now();
        let mut last_frame = Instant::now();
        let mut surface: Option<Surface> = None;

        loop {
            let frame_start = Instant::now();
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                if msg.message == WM_QUIT {
                    if let Some(sfc) = surface.take() {
                        sfc.release(memdc);
                    }
                    let _ = DeleteDC(memdc);
                    ReleaseDC(None, screen);
                    return;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }

            // A hook event is the whole point of holding the port: an answer waiting on the user
            // should light up now, not on the next two-second tick.
            if hub.take_changed() {
                readings = providers.iter().map(|p| (*p, read_provider(p, &hub))).collect();
                trace(&format!(
                    "hook event -> {}",
                    readings.iter().map(|(p, r)| format!("{p}={:?}", r.work)).collect::<Vec<_>>().join(" ")
                ));
                // A hook event can add a session line, which makes the panel taller. Without this
                // the window keeps its old height and the panel is clipped — the poll path already
                // resized, this one did not.
                let wanted = Layout::new(cfg.scale * dpi, providers.len(), panels_height(&lay, &readings));
                if wanted.h != lay.h {
                    lay = wanted;
                    canvas = Canvas::new(lay.w, lay.h);
                    shown_x = mon.right - lay.w;
                    y = mon.top + ((mon.bottom - mon.top) as f32 * cfg.notch_y - lay.h as f32 / 2.0).round() as i32;
                }
                dirty = true;
            }

            if last_poll.elapsed() >= POLL {
                last_poll = Instant::now();
                let fresh = Cfg::load();
                if fresh.scale != cfg.scale || fresh.providers() != providers {
                    cfg = fresh;
                    providers = cfg.providers();
                    lay = Layout::new(cfg.scale * dpi, providers.len(), panels_height(&lay, &readings));
                    canvas = Canvas::new(lay.w, lay.h);
                    shown_x = mon.right - lay.w;
                    hidden_x = mon.right - (PEEK * cfg.scale * dpi).round() as i32;
                    y = mon.top + ((mon.bottom - mon.top) as f32 * cfg.notch_y - lay.h as f32 / 2.0).round() as i32;
                    dirty = true;
                }
                let next: Vec<(&str, Reading)> = providers.iter().map(|p| (*p, read_provider(p, &hub))).collect();
                let changed = next.len() != readings.len()
                    || next.iter().zip(&readings).any(|(a, b)| {
                        a.1.used != b.1.used || a.1.work != b.1.work || a.1.stale != b.1.stale
                    });
                if changed {
                    readings = next;
                    // The panels can change height — a session appearing adds a line — and the
                    // window has to be tall enough for them or they get clipped.
                    let wanted = Layout::new(cfg.scale * dpi, providers.len(), panels_height(&lay, &readings));
                    if wanted.h != lay.h {
                        lay = wanted;
                        canvas = Canvas::new(lay.w, lay.h);
                        shown_x = mon.right - lay.w;
                        y = mon.top + ((mon.bottom - mon.top) as f32 * cfg.notch_y - lay.h as f32 / 2.0).round() as i32;
                    }
                    dirty = true;
                }
            }

            let mut cur = POINT::default();
            let _ = GetCursorPos(&mut cur);
            let pad = HOVER_PAD * cfg.scale * dpi;
            // Hot area in screen coordinates: the pill alone, or the whole window once the card is
            // out. The card's empty column must not hold the pill open while it is shut.
            //
            // The edge strip is folded in rather than used only while tucked away. Without it the
            // pill flaps: the strip pulls it out, and a pointer resting on the strip is not yet
            // inside the pill's own rectangle while that rectangle is still mostly off-screen, so
            // it immediately decides to leave, and the spring hunts somewhere in the middle.
            // Measured against where the pill is going, not where it currently is. Using the live
            // position shrinks the hot area to the edge strip for as long as the pill is still on
            // its way out, so a pointer that drifts twenty pixels mid-slide sends it back.
            let edge_strip = hidden_x as f32 - 2.0;
            let settled_left = shown_x as f32 + if panel_shown.is_some() { 0.0 } else { lay.pill_left };
            let hot_left = settled_left.min(edge_strip);
            // Asymmetric on purpose: harder to leave than to arrive. The boundary to cross going
            // out sits further in than the one coming back, so a hand that is not perfectly still
            // cannot straddle both.
            let edge = if shown { hot_left - pad - HOVER_HYSTERESIS } else { hot_left - pad };
            let in_rows = (cur.y as f32) >= y as f32 - pad - if shown { HOVER_HYSTERESIS } else { 0.0 }
                && (cur.y as f32) <= (y + lay.h) as f32 + pad + if shown { HOVER_HYSTERESIS } else { 0.0 };
            let inside = in_rows && (cur.x as f32) >= edge;
            // Opening asks for an outward push into a strip at the very edge, not mere presence
            // anywhere in the area that keeps the pill out. The pointer has to cross into that
            // strip while travelling towards the edge, so a pointer parked out there — on a
            // settings pane pinned to the side, or coming down the edge from above — never crosses
            // anything, and the pill stays put instead of covering what is under it.
            let reveal_edge = edge_strip - REVEAL_REACH * cfg.scale * dpi;
            let at_edge = in_rows && (cur.x as f32) >= reveal_edge;
            let pushed_out = at_edge && !was_at_edge && cur.x > prev_x;
            was_at_edge = at_edge;
            prev_x = cur.x;

            // Leaving waits; arriving does not. A pointer on its way past should not open it, but
            // it definitely should not close it either.
            let want = if inside && (shown || pushed_out) {
                left_at = None;
                true
            } else if !shown {
                false
            } else {
                match left_at {
                    Some(t) if t.elapsed() >= LEAVE_DWELL => false,
                    Some(_) => true,
                    None => {
                        left_at = Some(Instant::now());
                        true
                    }
                }
            };

            // Clicking the pill toggles the card. Read from the key state rather than a window
            // message: the window is click-through most of the time, so the message may never
            // arrive, while this is true either way.
            let down = GetAsyncKeyState(VK_LBUTTON.0 as i32) < 0;
            let over_pill = in_rows && (cur.x as f32) >= pos + lay.pill_left;
            if down && !was_down && over_pill && shown {
                // Which ring was hit decides which panel opens; clicking the open one shuts it,
                // and clicking a different ring swaps rather than stacking a second panel.
                let hit = lay.cell_at(cur.y as f32 - y as f32, readings.len());
                panel_want = next_panel(panel_want, hit);
                trace(&format!("click cell={hit:?} panel={panel_want:?}"));
            }
            was_down = down;

            // Right-click is the whole settings surface. `TrackPopupMenu` blocks here until the
            // menu closes, which freezes the pill mid-frame; `dt` is clamped further down, so the
            // frame that comes back afterwards cannot launch the springs across the screen.
            let right = GetAsyncKeyState(VK_RBUTTON.0 as i32) < 0;
            if right && !was_right && over_pill && shown {
                match pill_menu(hwnd, cur) {
                    MENU_RESTART => {
                        // The outgoing process still holds the hook port for a moment; the new one
                        // retries the bind rather than starting up deaf to events.
                        if let Ok(exe) = std::env::current_exe() {
                            let _ = std::process::Command::new(exe).spawn();
                        }
                        std::process::exit(0);
                    }
                    MENU_QUIT => std::process::exit(0),
                    _ => {}
                }
                // Both buttons may still be down on the way back, and a stale edge here would open
                // a panel nobody clicked on.
                was_down = GetAsyncKeyState(VK_LBUTTON.0 as i32) < 0;
                last_frame = Instant::now();
            }
            was_right = right;

            if want != shown {
                trace(&format!(
                    "hover {} cursor=({},{}) hot_left={hot_left:.0} pos={pos:.0} hidden={hidden_x} shown={shown_x} rows={in_rows}",
                    if want { "enter" } else { "leave" },
                    cur.x, cur.y
                ));
                // The spring's value and velocity carry straight over; its constants are set from
                // the target further down, once per frame. A reversal therefore keeps the momentum
                // it already had.
                shown = want;
            }
            // Leaving closes the panel too: it must not be left hanging open off the edge.
            if !shown && panel_want.is_some() {
                panel_want = None;
            }

            // Solid only where something is drawn, so clicks elsewhere reach the desktop.
            let should_catch = shown && inside;
            if should_catch == click_through {
                click_through = !should_catch;
                set_click_through(hwnd, click_through);
            }

            let now = Instant::now();
            let dt = (now - last_frame).as_secs_f32().min(0.05); // a stall must not launch the spring
            last_frame = now;

            // The panel closes before it swaps: the box shrinks away, the contents change at the
            // bottom of that dip, and it opens again. Changing them mid-flight would be a cut.
            let card_to = card_target(panel_shown, panel_want);
            // Three cases, not two: opening, closing for good, and closing on the way to another
            // panel. The last one used to borrow the dismissal's timing and spent a second closing
            // something the user had already asked to replace.
            let swapping = panel_shown != panel_want && panel_want.is_some();
            card.duration = match (card_to > 0.5, swapping) {
                (true, _) => REVEAL_DURATION,
                (false, true) => PANEL_SWAP_DURATION,
                (false, false) => PANEL_CLOSE_DURATION,
            };
            if !card.settled(card_to) {
                card.step(card_to, dt);
            }
            if card.value < PANEL_GONE && panel_shown != panel_want {
                panel_shown = panel_want;
                card.vel = carry_through(panel_want, card.vel);
            }

            // The pill waits for the panel to finish leaving before it retracts, or the panel goes
            // out of the frame instead of out of the way.
            let open_target = pill_target(shown, card.value);
            open.duration = if open_target > 0.5 { REVEAL_DURATION } else { DISMISS_DURATION };
            open.bounce = if open_target > 0.5 { REVEAL_BOUNCE } else { DISMISS_BOUNCE };
            if !open.settled(open_target) {
                open.step(open_target, dt);
            }

            let (next_alpha, next_scale) = skin(open.value, open_target < 0.5);
            // Clamped at the settled position. The spring overshoots by design, but `shown_x` is
            // already where the window's right edge meets the screen's: going past it pulls the
            // pill away from the edge and shows the side of it that is meant to be off-screen.
            // The bounce is not lost, it just lives in the content scale, which overshoots too —
            // a thing that hits a wall stops, and what is inside it keeps going for a moment.
            let next_pos = (hidden_x as f32 + (shown_x - hidden_x) as f32 * open.value)
                .max(shown_x as f32)
                .min(hidden_x as f32);
            let next_card = card.value.clamp(0.0, 1.0);
            let animating_ring = open.value > 0.02
                && readings.iter().any(|(_, r)| matches!(r.work, Work::Running | Work::Attention));

            let moved = (next_pos - pos).abs() > 0.01 || (next_alpha - alpha).abs() > 0.002;
            let rescaled = (next_scale - drawn_scale).abs() > 0.002 || (next_card - drawn_card).abs() > 0.002;
            pos = next_pos;
            alpha = next_alpha;

            // The working arc has to be redrawn every frame; everything else only when it changed.
            if dirty || rescaled || animating_ring {
                render(
                    &mut canvas,
                    &Frame {
                        lay: &lay,
                        readings: &readings,
                        cs: next_scale,
                        card: next_card,
                        panel: panel_shown,
                        t: start.elapsed().as_secs_f32(),
                    },
                    &mut marks,
                    &mut font,
                );
                drawn_scale = next_scale;
                drawn_card = next_card;
                dirty = false;
            } else if !moved {
                // Nothing changed: skip the blit entirely, which is what keeps this at roughly no
                // CPU while it sits at the edge.
                let spent = frame_start.elapsed();
                if spent < TICK {
                    std::thread::sleep(TICK - spent);
                }
                continue;
            }

            // The DIB is made once per layout, not once per frame. Allocating and freeing 600 KB
            // sixty times a second was pure overhead, and it landed on exactly the frames that were
            // already doing the most work.
            if surface.as_ref().map(|s: &Surface| s.w != lay.w || s.h != lay.h).unwrap_or(true) {
                if let Some(old) = surface.take() {
                    old.release(memdc);
                }
                surface = Surface::new(screen, memdc, lay.w, lay.h);
            }
            if let Some(sfc) = surface.as_ref() {
                std::ptr::copy_nonoverlapping(canvas.buf.as_ptr(), sfc.bits, canvas.buf.len());
                let blend = BLENDFUNCTION {
                    BlendOp: AC_SRC_OVER as u8,
                    // The fade rides on the layered window's global alpha, so it costs nothing:
                    // the bitmap is untouched and only this one byte changes per frame.
                    SourceConstantAlpha: (alpha.clamp(0.0, 1.0) * 255.0).round() as u8,
                    AlphaFormat: AC_SRC_ALPHA as u8,
                    ..Default::default()
                };
                let top_left = POINT { x: pos.round() as i32, y };
                let size = SIZE { cx: lay.w, cy: lay.h };
                let src = POINT { x: 0, y: 0 };
                let _ = UpdateLayeredWindow(
                    hwnd,
                    screen,
                    Some(&top_left),
                    Some(&size),
                    memdc,
                    Some(&src),
                    COLORREF(0),
                    Some(&blend),
                    ULW_ALPHA,
                );
            }

            // Pace on the frame boundary rather than sleeping a fixed tick after the work. A flat
            // sleep makes the period `work + tick`, so the frames that draw the most — a panel
            // opening, a swap — are also the slowest to come round, which is what read as a
            // stutter exactly when something interesting was happening.
            let spent = frame_start.elapsed();
            if spent < TICK {
                std::thread::sleep(TICK - spent);
            }
        }
    }
}

/// The bitmap the layered window is fed. Held across frames: its size only changes when the layout
/// does.
struct Surface {
    dib: HGDIOBJ,
    prev: HGDIOBJ,
    bits: *mut u8,
    w: i32,
    h: i32,
}

impl Surface {
    unsafe fn new(screen: HDC, memdc: HDC, w: i32, h: i32) -> Option<Surface> {
        let bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h, // top-down, matching the canvas
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let dib = CreateDIBSection(screen, &bi, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
        let prev = SelectObject(memdc, dib);
        Some(Surface { dib: dib.into(), prev, bits: bits as *mut u8, w, h })
    }

    unsafe fn release(self, memdc: HDC) {
        let _ = SelectObject(memdc, self.prev);
        let _ = DeleteObject(self.dib);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(status: &str, used: f32, fetched_at: i64) -> Snapshot {
        Snapshot {
            status: status.into(),
            windows: vec![Window_ {
                id: "session".into(),
                label: "Current session".into(),
                used: Some(used),
                count: None,
            }],
            fetched_at,
        }
    }

    fn reading(used: Option<f32>, work: Work) -> Reading {
        Reading { used, stale: false, rows: vec![("Current session".into(), used)], work, sessions: Vec::new() }
    }

    // ------------------------------------------------------------ readings

    #[test]
    fn a_stale_reading_still_shows_its_number() {
        // The bug this replaces: any status other than "ok" became a dash, so a snapshot that had
        // simply not been refreshed for a minute lost its number entirely.
        let r = reading_of(&snap("stale", 0.21, 1_000), 2_000);
        assert_eq!(r.used, Some(0.21));
        assert!(r.stale);
    }

    #[test]
    fn missing_authorisation_has_no_number_to_show() {
        for status in ["needsAuth", "none"] {
            let r = reading_of(&snap(status, 0.21, 1_000), 2_000);
            assert_eq!(r.used, None, "{status} should render as a dash");
        }
    }

    #[test]
    fn a_fresh_reading_is_not_dimmed_but_an_old_one_is() {
        let now = 10 * 60 * 1000;
        assert!(!reading_of(&snap("ok", 0.5, now - 1_000), now).stale);
        assert!(reading_of(&snap("ok", 0.5, now - STALE_AFTER_MS - 1), now).stale);
    }

    #[test]
    fn every_window_reaches_the_card_even_when_only_one_is_the_headline() {
        let s = Snapshot {
            status: "ok".into(),
            windows: vec![
                Window_ { id: "session".into(), label: "Current session".into(), used: Some(0.2), count: None },
                Window_ { id: "weekly_all".into(), label: "Weekly".into(), used: Some(0.28), count: None },
            ],
            fetched_at: 1,
        };
        let r = reading_of(&s, 2);
        assert_eq!(r.used, Some(0.2), "the headline is still the session window");
        assert_eq!(r.rows.len(), 2, "but the card gets both");
        assert_eq!(r.rows[1].0, "Weekly");
    }

    #[test]
    fn a_window_with_no_label_falls_back_to_its_id() {
        let s = Snapshot {
            status: "ok".into(),
            windows: vec![Window_ { id: "primary".into(), label: String::new(), used: Some(0.1), count: None }],
            fetched_at: 1,
        };
        assert_eq!(reading_of(&s, 2).rows[0].0, "primary");
    }

    #[test]
    fn percentages_round_and_clamp() {
        assert_eq!(pct_label(None), "-");
        assert_eq!(pct_label(Some(0.0)), "0%");
        assert_eq!(pct_label(Some(0.214)), "21%");
        assert_eq!(pct_label(Some(1.0)), "100%");
        assert_eq!(pct_label(Some(2.5)), "100%", "an over-full reading must not read 250%");
    }

    // ------------------------------------------------------------ config

    #[test]
    fn an_empty_slot_list_means_every_provider() {
        let c = Cfg { scale: 1.0, notch_y: 0.5, notch_slots: vec![] };
        assert_eq!(c.providers().len(), 4);
    }

    #[test]
    fn listed_slots_are_kept_in_order_and_unknown_names_dropped() {
        let c = Cfg {
            scale: 1.0,
            notch_y: 0.5,
            notch_slots: vec![
                Slot { provider: "codex".into() },
                Slot { provider: "nonsense".into() },
                Slot { provider: "claude".into() },
            ],
        };
        assert_eq!(c.providers(), vec!["codex", "claude"]);
    }

    // ------------------------------------------------------------ layout

    #[test]
    fn the_pill_grows_by_one_cell_per_provider() {
        let one = Layout::new(1.0, 1, 0.0);
        let two = Layout::new(1.0, 2, 0.0);
        assert!(two.h > one.h, "two providers must be taller than one");
        assert_eq!(two.w, one.w, "width is fixed by the card and the pill, not the cell count");
    }

    #[test]
    fn scale_shrinks_both_axes() {
        let big = Layout::new(1.0, 2, 0.0);
        let small = Layout::new(0.8, 2, 0.0);
        assert!(small.w < big.w && small.h < big.h);
    }

    #[test]
    fn the_window_is_wide_enough_for_the_card_beside_the_pill() {
        let lay = Layout::new(1.0, 2, 0.0);
        assert!(lay.pill_left > CARD_W, "the card must fit to the left of the pill");
        assert!((lay.w as f32) - lay.pill_left >= PILL_W, "and the pill must still fit");
    }

    #[test]
    fn a_panel_grows_with_the_rows_it_has_to_show() {
        let lay = Layout::new(1.0, 1, 0.0);
        let one = reading(Some(0.2), Work::Idle);
        let mut wide = reading(Some(0.2), Work::Idle);
        wide.rows = (0..4).map(|i| (format!("row {i}"), Some(0.1))).collect();
        assert!(panel_height(&lay, &wide) > panel_height(&lay, &one));
    }

    #[test]
    fn the_panel_has_room_for_its_own_edge() {
        // It used to start at exactly x=0, so its rounded corner and hairline were cut off by the
        // canvas. The window has to be wider than panel plus gap plus pill.
        let lay = Layout::new(1.0, 2, 0.0);
        let panel_left = lay.pill_left - CARD_GAP - CARD_W;
        assert!(panel_left >= 4.0, "the panel starts at {panel_left}, flush with the edge");
        assert!((lay.w as f32) - lay.pill_left >= PILL_W, "and the pill still fits");
    }

    #[test]
    fn the_window_fits_the_tallest_panel_not_all_of_them() {
        // Only one panel is ever out, so the window has to cover the largest rather than the sum.
        let lay = Layout::new(1.0, 2, 0.0);
        let small = reading(Some(0.2), Work::Idle);
        let mut big = reading(Some(0.2), Work::Running);
        big.sessions = vec![("a".into(), "x".into()), ("b".into(), "y".into())];
        let both = [("claude", big), ("codex", small)];
        let need = panels_height(&lay, &both);
        assert_eq!(need, panel_height(&lay, &both[0].1), "the tallest one decides");
        assert!(need < panel_height(&lay, &both[0].1) + panel_height(&lay, &both[1].1));
    }

    #[test]
    fn the_pill_waits_for_the_panel_to_finish_leaving() {
        // The panel is drawn inside this window. Retracting while it is still visible does not
        // animate it away, it drags it off the edge — which is what "it does not leave the way it
        // arrived" looked like.
        assert_eq!(pill_target(false, 1.0), 1.0, "pointer gone but panel still open");
        assert_eq!(pill_target(false, 0.3), 1.0, "still closing");
        assert_eq!(pill_target(false, 0.0), 0.0, "panel gone, now it may retract");
        assert_eq!(pill_target(true, 0.0), 1.0, "pointer on it");
    }

    #[test]
    fn rows_arrive_one_after_another() {
        // Per-object sequencing: at the same moment the first row is further along than the last.
        let early = stagger(0.5, 0, 4);
        let late = stagger(0.5, 3, 4);
        assert!(early > late, "the first row should lead: {early:.2} against {late:.2}");
    }

    #[test]
    fn every_row_starts_hidden_and_ends_shown() {
        for n in [1usize, 2, 5] {
            for i in 0..n {
                assert_eq!(stagger(0.0, i, n), 0.0, "row {i} of {n} should start invisible");
                assert!((stagger(1.0, i, n) - 1.0).abs() < 1e-3, "row {i} of {n} should finish");
            }
        }
    }

    #[test]
    fn a_long_list_still_finishes_with_the_panel() {
        // The lead shrinks as the list grows, or the last row of a long one would still be sliding
        // in after the box around it had settled.
        assert!((stagger(1.0, 19, 20) - 1.0).abs() < 1e-3);
    }

    #[test]
    fn a_swap_is_quicker_than_a_dismissal() {
        // Closing for good has nobody waiting on it. Closing to make room for the panel that was
        // just asked for does, and it used to borrow the dismissal's second-long timing.
        assert!(PANEL_SWAP_DURATION < PANEL_CLOSE_DURATION);
        assert!(PANEL_SWAP_DURATION <= REVEAL_DURATION, "the shut half should not outlast the open");
    }

    #[test]
    fn the_swap_point_is_not_the_bottom_of_the_tail() {
        // A spring approaches zero exponentially: waiting for it to reach nothing spends more time
        // on the last invisible sliver than on the whole visible close.
        let closing = run(1.0, 0.0, PANEL_SWAP_DURATION, 0.0);
        let to_gone = closing.iter().take_while(|v| **v >= PANEL_GONE).count();
        let to_nothing = closing.len();
        assert!(PANEL_GONE > 0.0 && PANEL_GONE < 0.15, "and it still has to look gone");
        assert!(
            (to_nothing - to_gone) * 3 > to_nothing,
            "the tail should be worth skipping: {to_gone} frames to fade, {to_nothing} to settle"
        );
    }

    #[test]
    fn the_swap_keeps_some_of_its_speed() {
        // Reflected, not discarded: the panel is still moving when its contents change.
        let incoming = -2.0f32;
        let after = -incoming * SWAP_CARRY;
        assert!(after > 0.0, "it should be heading back out");
        assert!(after < -incoming, "but not gain energy from the turn");
    }

    #[test]
    fn a_swap_closes_before_it_opens() {
        // Drawn panel and wanted panel differ only mid-swap, and the box is shut for that stretch.
        assert_eq!(card_target(Some(0), Some(0)), 1.0, "settled open");
        assert_eq!(card_target(Some(0), Some(1)), 0.0, "on its way to the other one");
        assert_eq!(card_target(Some(0), None), 0.0, "closing for good");
        assert_eq!(card_target(None, Some(1)), 0.0, "nothing drawn yet");
    }

    #[test]
    fn leaving_is_unhurried() {
        // Twice it came back as still too fast. Leaving is now the longer of the two, and the
        // panel's own box is longer again — it is the thing being read, so it goes last.
        // Apple's stock spring is duration 0.5, bounce 0. This is deliberately past it.
        const APPLE_DEFAULT_DURATION: f32 = 0.5;
        assert!(DISMISS_DURATION > REVEAL_DURATION, "leaving should outlast arriving");
        assert!(DISMISS_DURATION > APPLE_DEFAULT_DURATION, "and outlast a stock spring too");
        assert!(PANEL_CLOSE_DURATION > PANEL_SWAP_DURATION, "and closing for good outlasts a swap");
        assert!(DISMISS_BOUNCE < 0.1, "a dismissal should not wobble");
    }

    /// Frames until the spring has covered all but the last 5 %, at 60 fps. The rest is sub-pixel
    /// creep that nobody sees, so this — not the time to settle — is how long the move looks.
    fn visible_frames(duration: f32, bounce: f32) -> usize {
        run(1.0, 0.0, duration, bounce).iter().take_while(|v| **v > 0.05).count()
    }

    #[test]
    fn the_exit_takes_long_enough_to_read_as_a_departure() {
        // Twice reported as still too quick. Measuring the wrong thing cost a round: a spring
        // released from rest always covers most of its distance early — the force is largest at
        // the largest displacement — so "distance in the first quarter of the frames" says nothing
        // about how rushed it looks, and no damping value changes it. What matters is how long the
        // part you can actually see lasts.
        let frames = visible_frames(DISMISS_DURATION, DISMISS_BOUNCE);
        let seconds = frames as f32 / 60.0;
        assert!(seconds > 0.55, "the exit is over in {seconds:.2}s, which reads as a snap");
        assert!(seconds < 1.1, "and this long starts to feel like waiting: {seconds:.2}s");
    }

    #[test]
    fn a_dismissal_settles_instead_of_bouncing() {
        // The bug this exists for. The swap's velocity reflection fired on dismissals too, because
        // drawn and wanted differ there as well — so closing for good turned round at the bottom
        // and climbed back up with nothing left to draw, and `pill_target` held the pill out for
        // the whole climb. A stall on an empty panel column is what read as leaving being stuck.
        assert_eq!(carry_through(None, -2.0), 0.0, "closing for good stops at the bottom");
        assert!(carry_through(Some(1), -2.0) > 0.0, "a swap turns round and heads back out");

        let mut card = Spring::new(PANEL_GONE, PANEL_CLOSE_DURATION, 0.0);
        card.vel = carry_through(None, -0.6);
        let mut prev = card.value;
        for _ in 0..120 {
            card.step(0.0, 1.0 / 60.0);
            assert!(card.value <= prev + 1e-6, "it climbed from {prev:.4} to {:.4}", card.value);
            prev = card.value;
        }
    }

    #[test]
    fn leaving_is_one_move_rather_than_two() {
        // The pill used to wait out all but the last invisible sliver of the panel's fade, so a
        // dismissal was the panel's close and then the pill's, end to end. It now takes over while
        // the panel is still faintly on screen — but not before, or the panel is dragged off the
        // edge while it is still readable instead of being allowed to close.
        let closing = run(1.0, 0.0, PANEL_CLOSE_DURATION, 0.0);
        let held = closing.iter().take_while(|v| pill_target(false, **v) > 0.5).count();
        let fade = closing.iter().take_while(|v| **v > 0.05).count();
        assert!(held > 0, "the pill must not bolt while the panel is still solid");
        assert!(held < fade, "the pill still waits out the whole fade: {held} of {fade} frames");

        let end_to_end = (held + visible_frames(DISMISS_DURATION, DISMISS_BOUNCE)) as f32 / 60.0;
        assert!(end_to_end < 1.25, "leaving takes {end_to_end:.2}s, which reads as waiting");
    }

    #[test]
    fn the_fade_holds_on_the_way_out() {
        // The entrance is deliberately opaque early. Reusing that curve on the exit kept the pill
        // solid for most of the trip and then dropped it all at once, which is the part that read
        // as a snap rather than a departure.
        let half_in = skin(0.5, false).0;
        let half_out = skin(0.5, true).0;
        assert!(half_in > half_out, "entering should be more solid at the half-way point");

        // And on the way out it should still be clearly visible most of the way.
        assert!(skin(0.5, true).0 > HIDDEN_ALPHA + (1.0 - HIDDEN_ALPHA) * 0.5);
    }

    #[test]
    fn a_click_opens_the_ring_it_landed_on() {
        assert_eq!(next_panel(None, Some(1)), Some(1));
        assert_eq!(next_panel(Some(0), Some(1)), Some(1), "a different ring swaps");
    }

    #[test]
    fn clicking_the_open_ring_shuts_it() {
        assert_eq!(next_panel(Some(1), Some(1)), None);
    }

    #[test]
    fn a_click_between_rings_changes_nothing() {
        // The gap belongs to neither ring; treating it as a hit would open one at random.
        assert_eq!(next_panel(Some(1), None), Some(1));
        assert_eq!(next_panel(None, None), None);
    }

    #[test]
    fn the_rings_map_to_the_cells_under_them() {
        let lay = Layout::new(1.0, 2, 0.0);
        let first = lay.pill_top + PAD_Y;
        assert_eq!(lay.cell_at(first + 1.0, 2), Some(0));
        assert_eq!(lay.cell_at(first + lay.cell_h + GAP + 1.0, 2), Some(1));
        assert_eq!(lay.cell_at(first - 5.0, 2), None, "above the first ring");
        assert_eq!(lay.cell_at(first + lay.cell_h + 2.0, 2), None, "in the gap between them");
        assert_eq!(lay.cell_at(first + (lay.cell_h + GAP) * 5.0, 2), None, "past the last ring");
    }

    #[test]
    fn the_window_grows_when_the_panels_outgrow_the_pill() {
        // The panels sit beside the pill, so anything taller than it would be clipped by a window
        // sized for the pill alone.
        let tight = Layout::new(1.0, 1, 0.0);
        let roomy = Layout::new(1.0, 1, tight.h as f32 + 200.0);
        assert!(roomy.h > tight.h, "the window must cover the taller of the two");
        assert!(roomy.pill_top > tight.pill_top, "and the pill stays centred in it");
    }

    #[test]
    fn a_sessions_list_makes_its_panel_taller() {
        let lay = Layout::new(1.0, 1, 0.0);
        let quiet = reading(Some(0.2), Work::Idle);
        let mut busy = reading(Some(0.2), Work::Running);
        busy.sessions = vec![("proj".into(), "working".into()), ("outro".into(), "waiting".into())];
        assert!(panel_height(&lay, &busy) > panel_height(&lay, &quiet));
    }

    // ------------------------------------------------------------ animation

    /// Runs the spring at a fixed 60 fps until it settles, returning every value it passed through.
    fn run(from: f32, target: f32, duration: f32, bounce: f32) -> Vec<f32> {
        let mut sp = Spring::new(from, duration, bounce);
        let mut seen = vec![sp.value];
        for _ in 0..600 {
            if sp.settled(target) {
                break;
            }
            sp.step(target, 1.0 / 60.0);
            seen.push(sp.value);
        }
        seen
    }

    #[test]
    fn the_reveal_spring_overshoots_once_and_settles() {
        let seen = run(0.0, 1.0, REVEAL_DURATION, REVEAL_BOUNCE);
        let peak = seen.iter().copied().fold(f32::MIN, f32::max);
        assert!(peak > 1.0, "the reveal should pass its target, peaked at {peak}");
        assert!(peak < 1.15, "the overshoot should be a nudge, not a bounce: {peak}");
        assert!((seen.last().unwrap() - 1.0).abs() < 0.01, "it must come to rest on the target");
    }

    #[test]
    fn the_dismiss_spring_never_overshoots() {
        let seen = run(1.0, 0.0, DISMISS_DURATION, DISMISS_BOUNCE);
        let under = seen.iter().copied().fold(f32::MAX, f32::min);
        assert!(under >= -0.005, "dismiss dipped past its target to {under}");
    }

    #[test]
    fn the_entrance_is_prompt_and_the_exit_is_not() {
        // The entrance answers the pointer, so it has to be quick. The exit has nobody waiting on
        // it and was reported as rushed three times, so it is deliberately the long one — this
        // assertion used to demand both settle inside a second and had to be rewritten rather than
        // satisfied.
        let arrive = run(0.0, 1.0, REVEAL_DURATION, REVEAL_BOUNCE).len();
        let leave = run(1.0, 0.0, DISMISS_DURATION, DISMISS_BOUNCE).len();
        assert!(arrive < 60, "the entrance should settle briskly, took {arrive} frames");
        assert!(leave > arrive, "the exit should take longer than the entrance");
        assert!(leave < 180, "but not turn into a wait: {leave} frames");
    }

    #[test]
    fn a_reversal_keeps_the_momentum_it_already_had() {
        // The whole point of a spring over an easing curve: interrupting it does not discard the
        // velocity. Measured against an identical spring released from rest at the same place —
        // not against the sign of `vel`, because the dismiss spring is stiff enough to flip that
        // inside a single 16 ms frame, which is correct and says nothing about continuity.
        let mut moving = Spring::new(0.0, REVEAL_DURATION, REVEAL_BOUNCE);
        for _ in 0..6 {
            moving.step(1.0, 1.0 / 60.0);
        }
        assert!(moving.vel > 0.0, "should be travelling outward by now");

        let mut from_rest = Spring::new(moving.value, DISMISS_DURATION, DISMISS_BOUNCE);
        moving.duration = DISMISS_DURATION;
        moving.bounce = DISMISS_BOUNCE;

        moving.step(0.0, 1.0 / 60.0);
        from_rest.step(0.0, 1.0 / 60.0);
        assert!(moving.value > from_rest.value, "momentum should carry it further out");
    }

    #[test]
    fn a_stalled_frame_cannot_launch_the_spring() {
        let mut sp = Spring::new(0.0, REVEAL_DURATION, REVEAL_BOUNCE);
        sp.step(1.0, 0.05);
        assert!(sp.value.is_finite() && sp.value.abs() < 3.0, "diverged to {}", sp.value);
    }

    #[test]
    fn opacity_leads_the_movement_and_stays_in_range() {
        let (a_third, _) = skin(0.33, false);
        assert!(a_third > 0.85, "a third of the way out it should be nearly solid, got {a_third}");
        for i in 0..=150 {
            let (a, sc) = skin(i as f32 / 100.0, false);
            assert!((HIDDEN_ALPHA..=1.0).contains(&a), "opacity left its range: {a}");
            assert!((CONTENT_MIN_SCALE..=1.01).contains(&sc), "scale left its range: {sc}");
        }
    }

    #[test]
    fn content_starts_small_and_ends_full_size() {
        assert!((skin(0.0, false).1 - CONTENT_MIN_SCALE).abs() < 1e-3);
        assert!((skin(1.0, false).1 - 1.0).abs() < 0.01);
    }

    // ------------------------------------------------------------ hover

    /// The rule the loop uses, lifted out so it can be checked without a window.
    fn hot_left(shown_x: f32, pill_left: f32, hidden_x: f32, card_open: bool) -> f32 {
        (shown_x + if card_open { 0.0 } else { pill_left }).min(hidden_x - 2.0)
    }

    #[test]
    fn the_hot_area_does_not_shrink_while_the_pill_is_moving() {
        // Two bugs met here. The pill used to flap, because the strip pulled it out and then the
        // pointer on that strip was not yet inside the pill's own rectangle. Anchoring on the
        // settled position fixes that and a second, subtler one: measured against the live
        // position the hot area is only the strip until the pill lands, so a pointer drifting a
        // few pixels mid-slide sent it straight back.
        let (pill_left, hidden_x, shown_x) = (310.0, 3433.0, 3042.0);
        let left = hot_left(shown_x, pill_left, hidden_x, false);
        assert!(left <= hidden_x - 2.0, "the edge strip must stay hot");
        assert!(
            hidden_x - left > 60.0,
            "the hot area should be the whole pill, not a sliver: {} px wide",
            hidden_x - left
        );
        // Anything on the strip stays hot regardless of where the animation currently is.
        for cursor in [3437.0, 3400.0, 3360.0] {
            assert!(cursor >= left, "{cursor} should be inside a hot area starting at {left}");
        }
    }

    /// The two thresholds the loop uses: it is harder to leave than to arrive.
    fn edge_for(shown: bool, hot_left: f32, pad: f32) -> f32 {
        if shown { hot_left - pad - HOVER_HYSTERESIS } else { hot_left - pad }
    }

    #[test]
    fn a_jittery_pointer_on_the_boundary_cannot_make_it_flap() {
        // Measured from a real trace: a hand resting near the edge wandered over roughly 40 px and
        // crossed a single threshold seventeen times in six seconds, and the pill flapped with it.
        let (hot_left, pad) = (3351.0, 14.4);
        let enter = edge_for(false, hot_left, pad);
        let leave = edge_for(true, hot_left, pad);
        assert!(leave < enter, "leaving must be the harder of the two");

        // Everything the observed hand touched, once open, stays open.
        for cursor in [3299.0, 3307.0, 3320.0, 3330.0, 3348.0, 3368.0] {
            assert!(cursor >= leave, "{cursor} would have closed it again");
        }
    }

    #[test]
    fn the_hysteresis_is_wider_than_the_wobble_it_absorbs() {
        // The trace's spread; the margin has to cover it or the fix does not hold.
        assert!(HOVER_HYSTERESIS > 3368.0 - 3299.0 - 14.4, "margin too tight for a real hand");
    }

    #[test]
    fn the_overshoot_never_pulls_the_pill_off_the_screen_edge() {
        // The spring peaks above 1.0 on purpose, but the window is flush with the screen edge at
        // its settled position: any further and the right side of the pill, which is supposed to be
        // off-screen, comes into view with desktop behind it.
        let (hidden_x, shown_x) = (3433.0, 2953.0);
        let place = |v: f32| (hidden_x + (shown_x - hidden_x) * v).max(shown_x).min(hidden_x);
        let peak = run(0.0, 1.0, REVEAL_DURATION, REVEAL_BOUNCE)
            .iter()
            .copied()
            .fold(f32::MIN, f32::max);
        assert!(peak > 1.0, "the spring should still overshoot: {peak}");
        assert_eq!(place(peak), shown_x, "but the window must stop at the edge");
        for v in [-0.2, 0.0, 0.5, 1.0, 1.2] {
            let p = place(v);
            assert!((shown_x..=hidden_x).contains(&p), "position left its track at {v}: {p}");
        }
    }

    /// Mirrors the loop: only a crossing made while travelling outwards opens it.
    fn pushed_out(prev_x: i32, was_at_edge: bool, cur_x: i32, at_edge: bool) -> bool {
        at_edge && !was_at_edge && cur_x > prev_x
    }

    #[test]
    fn only_an_outward_push_opens_the_pill() {
        // Walking out to the edge: cold, then crossing rightwards into the strip.
        assert!(pushed_out(3000, false, 3425, true), "a push to the edge should open it");
        // Parked out there already - a pane pinned to the side, a pointer left on the strip.
        assert!(!pushed_out(3400, true, 3400, true), "presence alone must not open it");
        // Coming down the edge from above: the row band turns true without any sideways travel.
        assert!(!pushed_out(3400, false, 3400, true), "a vertical arrival must not open it");
        // Crossing back inwards.
        assert!(!pushed_out(3400, false, 3380, true), "moving inwards must not open it");
        // First frame, before there is a previous position to compare against.
        assert!(!pushed_out(i32::MAX, false, 3400, true), "startup must not open it");
    }

    #[test]
    fn a_pointer_well_clear_of_the_edge_is_not_hot() {
        let (pill_left, hidden_x, shown_x) = (310.0, 3433.0, 3042.0);
        assert!(2000.0 < hot_left(shown_x, pill_left, hidden_x, false), "the desktop should be cold");
        let _ = pill_left;
    }

    #[test]
    fn an_open_card_widens_the_hot_area_to_the_whole_window() {
        let (pill_left, hidden_x, shown_x) = (310.0, 3433.0, 3042.0);
        let shut = hot_left(shown_x, pill_left, hidden_x, false);
        let open = hot_left(shown_x, pill_left, hidden_x, true);
        assert!(open < shut, "an open card must catch the pointer over its own column");
        assert_eq!(open, shown_x, "which starts at the window's left edge");
    }

    // ------------------------------------------------------------ drawing

    fn paint(lay: &Layout, readings: &[(&str, Reading)], card: f32, t: f32) -> Canvas {
        let mut c = Canvas::new(lay.w, lay.h);
        let f = Frame { lay, readings, cs: 1.0, card, t, panel: Some(0) };
        render(&mut c, &f, &mut Marks::default(), &mut Text::system().unwrap());
        c
    }

    /// Ink strictly left of the pill's column, which is where the card lives.
    fn left_ink(c: &Canvas, lay: &Layout) -> usize {
        c.buf
            .chunks(4)
            .enumerate()
            .filter(|(i, px)| ((*i as i32) % lay.w) < (lay.pill_left as i32) - 20 && px[3] > 8)
            .count()
    }

    /// Writes what the pill actually looks like to a PNG, so a shape bug can be looked at instead
    /// of guessed at from a screenshot. Ignored by default: it is a darkroom, not an assertion.
    /// `cargo test -p codenotch-native --release -- --ignored dump_the_pill --nocapture`
    #[test]
    #[ignore]
    fn dump_the_pill() {
        use resvg::tiny_skia;
        let lay = Layout::new(1.2, 2, 0.0);
        let readings = vec![
            ("claude", reading(Some(0.27), Work::Idle)),
            ("codex", reading(Some(0.02), Work::Idle)),
        ];
        // Which panel to dump comes from the environment, so both can be looked at without edits.
        let which: usize = std::env::var("DUMP_PANEL").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
        let open = std::env::var("DUMP_OPEN").is_ok();
        let needed = panels_height(&lay, &readings);
        let lay = Layout::new(1.2, 2, needed);
        let mut c = Canvas::new(lay.w, lay.h);
        render(
            &mut c,
            &Frame { lay: &lay, readings: &readings, cs: 1.0, card: if open { 1.0 } else { 0.0 }, t: 0.0, panel: Some(which) },
            &mut Marks::default(),
            &mut Text::system().unwrap(),
        );

        let mut pm = tiny_skia::Pixmap::new(lay.w as u32, lay.h as u32).unwrap();
        // The canvas is premultiplied BGRA; a Pixmap is premultiplied RGBA, so only B and R swap.
        // Composited over mid-grey first, or a dark shape on a transparent page is unreadable.
        for (dst, src) in pm.pixels_mut().iter_mut().zip(c.buf.chunks(4)) {
            let (b, g, r, a) = (src[0] as u32, src[1] as u32, src[2] as u32, src[3] as u32);
            let over = |c: u32| ((c + 96 * (255 - a) / 255).min(255)) as u8;
            *dst = tiny_skia::PremultipliedColorU8::from_rgba(over(r), over(g), over(b), 255).unwrap();
        }
        let out = std::env::temp_dir().join("pill.png");
        pm.save_png(&out).unwrap();
        println!("escrito: {} ({}x{})", out.display(), lay.w, lay.h);
    }

    #[test]
    #[ignore]
    fn probe_the_pill() {
        let lay = Layout::new(1.2, 2, 0.0);
        let readings = vec![
            ("claude", reading(Some(0.27), Work::Idle)),
            ("codex", reading(Some(0.02), Work::Idle)),
        ];
        let c = paint(&lay, &readings, 0.0, 0.0);
        let at = |x: i32, y: i32| {
            let i = ((y * lay.w + x) * 4) as usize;
            (c.buf[i + 2], c.buf[i + 1], c.buf[i], c.buf[i + 3])
        };
        println!("janela {}x{} pill_top={} pill_h={}", lay.w, lay.h, lay.pill_top, lay.pill_h);
        let mid_x = lay.w - 35;
        println!("centro da pill      {:?}", at(mid_x, lay.h / 2));
        println!("entre os aneis      {:?}", at(mid_x, (lay.pill_top + lay.pill_h / 2.0) as i32));
        println!("canto sup esq pill  {:?}", at((lay.pill_left + 8.0) as i32, lay.pill_top as i32 + 40));

        // Perfil vertical na coluna encostada na borda: as duas pontas tem que ser espelho
        let col = lay.w - 2;
        let top_edge = lay.pill_top as i32;
        // Last row the pill occupies, not the first one past it: off by one here makes a
        // symmetric shape look lopsided, which cost me a diagnosis.
        let bot_edge = (lay.pill_top + lay.pill_h) as i32 - 1;
        println!("
coluna x={col}, alpha subindo do topo da pill:");
        for d in 0..6 {
            println!("  topo -{d:>2}px alpha={:>3}   base +{d:>2}px alpha={:>3}",
                at(col, top_edge - d).3, at(col, (bot_edge + d).min(lay.h - 1)).3);
        }
        println!("
largura solida (alpha>200) por linha, do topo e da base:");
        for d in [0, 4, 8, 16, 24, 30] {
            let w_top = (0..lay.w).filter(|x| at(*x, (top_edge - d).max(0)).3 > 200).count();
            let w_bot = (0..lay.w).filter(|x| at(*x, (bot_edge + d).min(lay.h - 1)).3 > 200).count();
            println!("  {d:>2}px  topo={w_top:>3}  base={w_bot:>3}");
        }
    }

    #[test]
    #[ignore]
    fn how_long_a_frame_takes() {
        // The swap and the close redraw every frame. At 60 fps a frame has 16.7 ms; anything close
        // to that here is the stutter.
        let lay = Layout::new(1.2, 2, 0.0);
        let mut big = reading(Some(0.27), Work::Running);
        big.rows = (0..3).map(|i| (format!("Limit {i}"), Some(0.3))).collect();
        big.sessions = vec![("proj".into(), "working".into()), ("outro".into(), "waiting".into())];
        let readings = vec![("claude", big), ("codex", reading(Some(0.02), Work::Idle))];
        let lay = Layout::new(1.2, 2, panels_height(&lay, &readings));
        let mut c = Canvas::new(lay.w, lay.h);
        let mut marks = Marks::default();
        let mut font = Text::system().unwrap();

        for (name, card) in [("pill so", 0.0), ("painel aberto", 1.0), ("meio da troca", 0.5)] {
            let f = Frame { lay: &lay, readings: &readings, cs: 1.0, card, t: 0.3, panel: Some(0) };
            render(&mut c, &f, &mut marks, &mut font); // aquece os caches
            let t0 = std::time::Instant::now();
            const N: u32 = 30;
            for _ in 0..N {
                render(&mut c, &f, &mut marks, &mut font);
            }
            let per = t0.elapsed().as_secs_f64() * 1000.0 / N as f64;
            println!("{name:>16}: {per:.2} ms por frame  ({}x{})", lay.w, lay.h);
        }
    }

    #[test]
    fn rendering_actually_puts_pixels_in_the_buffer() {
        // Splits render() from the blit: if this passes and nothing shows on screen, the fault is
        // in UpdateLayeredWindow, not in the painting.
        let lay = Layout::new(1.2, 2, 0.0);
        let readings = vec![("claude", reading(Some(0.2), Work::Idle)), ("codex", reading(Some(0.02), Work::Idle))];
        let c = paint(&lay, &readings, 0.0, 0.0);

        let opaque = c.buf.chunks(4).filter(|px| px[3] > 200).count();
        assert!(opaque > 500, "the pill should leave a large opaque area, got {opaque} pixels");
        let bright = c.buf.chunks(4).filter(|px| px[3] > 200 && px[0] > 120).count();
        assert!(bright > 20, "ring, mark and digits should be light, got {bright} bright pixels");
    }

    #[test]
    fn a_shut_card_paints_nothing_on_the_left() {
        let lay = Layout::new(1.0, 1, 0.0);
        let readings = vec![("claude", reading(Some(0.2), Work::Idle))];
        assert_eq!(left_ink(&paint(&lay, &readings, 0.0, 0.0), &lay), 0, "the card's column must be empty while shut");
    }

    #[test]
    fn an_open_card_paints_to_the_left_of_the_pill() {
        let lay = Layout::new(1.0, 1, 0.0);
        let readings = vec![("claude", reading(Some(0.2), Work::Idle))];
        let ink = left_ink(&paint(&lay, &readings, 1.0, 0.0), &lay);
        assert!(ink > 1000, "an open card should fill its column, got {ink} pixels");
    }

    #[test]
    fn the_working_arc_turns_over_time() {
        // Two frames a third of a turn apart must not be identical, or the ring is frozen.
        let lay = Layout::new(1.0, 1, 0.0);
        let readings = vec![("claude", reading(Some(0.2), Work::Running))];
        let a = paint(&lay, &readings, 0.0, 0.0);
        let b = paint(&lay, &readings, 0.0, SPIN_PERIOD / 3.0);
        assert_ne!(a.buf, b.buf, "the working arc should have moved");
    }

    #[test]
    fn an_idle_provider_draws_the_same_frame_every_time() {
        // The inverse: nothing animates when no turn is in progress, or the loop would repaint
        // forever and the idle CPU saving would disappear.
        let lay = Layout::new(1.0, 1, 0.0);
        let readings = vec![("claude", reading(Some(0.2), Work::Idle))];
        let a = paint(&lay, &readings, 0.0, 0.0);
        let b = paint(&lay, &readings, 0.0, 9.0);
        assert_eq!(a.buf, b.buf, "an idle pill must be static");
    }
}
