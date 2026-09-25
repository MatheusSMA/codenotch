// What the pill and the preferences window share: where codenotch-daemon writes, how to read it,
// and the provider list. GLib and Gio only, so prefs.js (which runs outside the shell) can use it.

import GLib from 'gi://GLib';

export const PROVIDERS = ['claude', 'codex', 'cursor', 'antigravity'];

export const NAMES = {
    claude: 'Claude',
    codex: 'Codex',
    cursor: 'Cursor',
    antigravity: 'Antigravity',
};

/// How each provider gets its numbers, for the preferences window: what to do when it says it has
/// no reading.
export const HOW_TO_CONNECT = {
    claude: 'Install Claude Code and sign in once by running `claude` in a terminal.',
    codex: 'Install the Codex CLI and run `codex login`.',
    cursor: 'Install Cursor and sign in inside the app.',
    antigravity: 'Install the Antigravity CLI (`agy`) and sign in.',
};

const FILES = {
    claude: 'usage.json',
    codex: 'codex.json',
    cursor: 'cursor.json',
    antigravity: 'antigravity.json',
};

/// Five minutes, as the Windows build: a reading older than this is shown dimmed.
const STALE_AFTER_MS = 5 * 60 * 1000;

export function dataDir() {
    return GLib.build_filenamev([GLib.get_user_config_dir(), 'codenotch']);
}

export function readJson(name) {
    try {
        const [ok, bytes] = GLib.file_get_contents(GLib.build_filenamev([dataDir(), name]));
        if (!ok)
            return null;
        // A BOM makes JSON.parse throw; a hand-edited file may carry one.
        return JSON.parse(new TextDecoder().decode(bytes).replace(/^﻿/, ''));
    } catch (_) {
        return null;
    }
}

function writeJson(name, value) {
    GLib.mkdir_with_parents(dataDir(), 0o755);
    GLib.file_set_contents(GLib.build_filenamev([dataDir(), name]), JSON.stringify(value, null, 2));
}

export function readConfig() {
    const c = readJson('config.json') ?? {};
    const slots = Array.isArray(c.notch_slots) ? c.notch_slots.map(s => s.provider) : [];
    // An empty slot list means every provider, the rule the Windows build and notch.html share.
    const shown = slots.length ? PROVIDERS.filter(p => slots.includes(p)) : [...PROVIDERS];
    const scale = typeof c.scale === 'number' ? c.scale : 1.0;
    return {shown, scale};
}

/// Writes the provider list and the size, keeping every other key the file already holds.
export function writeConfig({shown, scale}) {
    const c = readJson('config.json') ?? {};
    if (shown) {
        const list = PROVIDERS.filter(p => shown.includes(p));
        c.notch_slots = list.map(provider => ({provider}));
        c.notch_providers = list;
    }
    if (scale)
        c.scale = scale;
    writeJson('config.json', c);
}

/// Asks the daemon for fresh numbers (it watches this file's modification time).
export function requestRefresh() {
    try {
        writeJson('refresh', {at: Date.now()});
    } catch (_) {}
}

export function readSnapshot(provider) {
    return readJson(FILES[provider]);
}

/// One ring's worth: the headline fraction, whether it is stale, the card's rows, and the note the
/// fetcher left (which is what says "not signed in" when there is nothing else to show).
export function readingOf(snap, now = Date.now()) {
    const empty = {used: null, stale: false, rows: [], note: snap?.note ?? '', status: snap?.status ?? 'absent'};
    if (!snap || snap.status === 'needsAuth' || snap.status === 'none')
        return empty;
    const windows = Array.isArray(snap.windows) ? snap.windows : [];
    const fraction = w => w.count === null || w.count === undefined;
    // Claude calls the headline `session`, Codex `primary`: the shape decides, not the name.
    const head = windows.find(w => fraction(w) && (w.id === 'session' || w.id === 'primary')) ??
        windows.find(fraction);
    const aged = snap.fetched_at > 0 && now - snap.fetched_at > STALE_AFTER_MS;
    return {
        used: head?.used ?? null,
        stale: snap.status === 'stale' || aged,
        rows: windows.map(w => ({
            label: w.label || w.id,
            used: fraction(w) ? w.used ?? null : null,
            resetsAt: w.resets_at ?? null,
        })),
        note: snap.note ?? '',
        status: snap.status ?? '',
    };
}

/// Upstream's `UsageBand`: green under 50 %, yellow under 70 %, orange from there.
export function band(used) {
    if (used < 0.5)
        return [0.0, 1.0, 0.533];
    if (used < 0.7)
        return [0.949, 1.0, 0.0];
    return [1.0, 0.247, 0.0];
}

export function pctLabel(used) {
    if (used === null || used === undefined)
        return '-';
    return `${Math.min(100, Math.max(0, Math.round(used * 100)))}%`;
}

const DAYS = ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'];
const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];

/// "Resets in 51 min" under an hour, "Resets Thu 12:00 AM" within the week, "Resets Sep 28" beyond.
export function resetCopy(resetsAtMs, now = Date.now()) {
    const secs = (resetsAtMs - now) / 1000;
    if (secs <= 0)
        return 'Resetting…';
    const minutes = Math.round(secs / 60);
    if (minutes < 60)
        return `Resets in ${Math.max(1, minutes)} min`;
    const at = new Date(resetsAtMs);
    const today = new Date(now);
    today.setHours(0, 0, 0, 0);
    const day = new Date(at);
    day.setHours(0, 0, 0, 0);
    if ((day - today) / 86400000 >= 7)
        return `Resets ${MONTHS[at.getMonth()]} ${at.getDate()}`;
    const h = at.getHours() % 12 || 12;
    const m = String(at.getMinutes()).padStart(2, '0');
    return `Resets ${DAYS[at.getDay()]} ${h}:${m} ${at.getHours() < 12 ? 'AM' : 'PM'}`;
}
