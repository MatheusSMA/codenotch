// Codenotch for GNOME Shell: the pill on the right edge of the primary monitor, and the usage card
// beside it. The numbers come from codenotch-daemon, which writes them to ~/.config/codenotch/; this
// file only reads and draws.
//
// Geometry is the Windows build's (codenotch-native/src/main.rs), which takes it from upstream's
// design frame: frame pixels times 44/117. Keep the two in step.

import Cairo from 'cairo';
import Clutter from 'gi://Clutter';
import GLib from 'gi://GLib';
import Pango from 'gi://Pango';
import PangoCairo from 'gi://PangoCairo';
import Rsvg from 'gi://Rsvg?version=2.0';
import St from 'gi://St';

import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';

import * as Data from './data.js';

// ------------------------------------------------------------------ design sizes (points)

const px = frame => frame * 44 / 117;
const cap = frame => px(frame) / 0.714;
const line = size => Math.ceil(size * 1.193);
const baselineIn = (top, size) => top + size * 0.952 + (line(size) - size * 1.193) / 2;

const PILL_W = px(186);
const RADIUS = px(78.8);
const FILLET = px(103);
const PAD_Y = px(95);
const PAD_BOTTOM = px(75);
const GAP = px(100);
const RING_BOX = px(117);
const TRACK_STROKE = px(9);
const PROGRESS_STROKE = px(6);
const ACTIVITY_D = px(84);
const ACTIVITY_STROKE = px(5.5);
const TEXT_GAP = px(26.9);
const PCT_PX = cap(27);
const MARK = px(60);
const CELL_H = RING_BOX + TEXT_GAP + line(PCT_PX);

const CARD_ZOOM = 1.4;
const CARD_W = px(600);
const CARD_RADIUS = px(49.5);
const CARD_PAD = px(32);
const TAIL_W = px(75);
const TAIL_H = px(87);
const TAIL_GAP = px(28);
const TITLE_PX = cap(26);
const BODY_PX = cap(18);
const HEADER_GAP = px(17);
const HEADER_TO_BLOCK = px(21);
const LABEL_TO_BAR = px(16.8);
const BAR_TO_USED = px(17.8);
const BLOCK_SPACING = px(20);
const BAR_H = px(10.5);
const HAIRLINE = px(2.5);
const SESSION_GAP = px(10);

/// How much of the pill stays on screen when tucked away, and how faint it is there.
const PEEK = 6;
const HIDDEN_OPACITY = 90;
/// The reveal asks for a push towards the edge, not mere presence (see main.rs for the history).
const PUSH_STEP = 12;
const PUSH_GRACE_MS = 350;
const HOVER_PAD = 12;
const HOVER_HYSTERESIS = 80;
const LEAVE_DWELL_MS = 300;
const TICK_MS = 33;
const POLL_MS = 2000;

const INK = [1, 1, 1];
const MUTED = [0.502, 0.502, 0.502];
const WATCH = [0.949, 1.0, 0.0];
const TRACK = [0.188, 0.188, 0.188];
const BAR_TRACK = [0.176, 0.176, 0.176];
const RULE = [0.118, 0.118, 0.118];

// ------------------------------------------------------------------ drawing helpers

function rgba(cr, rgb, a = 1) {
    cr.setSourceRGBA(rgb[0], rgb[1], rgb[2], a);
}

function roundRect(cr, x, y, w, h, r) {
    r = Math.min(r, w / 2, h / 2);
    cr.newSubPath();
    cr.arc(x + w - r, y + r, r, -Math.PI / 2, 0);
    cr.arc(x + w - r, y + h - r, r, 0, Math.PI / 2);
    cr.arc(x + r, y + h - r, r, Math.PI / 2, Math.PI);
    cr.arc(x + r, y + r, r, Math.PI, 1.5 * Math.PI);
    cr.closePath();
}

const fonts = new Map();
function font(size, bold) {
    const key = `${size.toFixed(2)}${bold}`;
    if (!fonts.has(key)) {
        const d = Pango.FontDescription.from_string(bold ? 'Sans Semi-Bold' : 'Sans');
        d.set_absolute_size(size * Pango.SCALE);
        fonts.set(key, d);
    }
    return fonts.get(key);
}

/// Draws `text` with its baseline at `baseline`. `align` is 'left', 'right' or 'center' about `x`.
/// Returns the width drawn. `maxW` ellipsizes.
function text(cr, str, x, baseline, size, rgb, a, {bold = false, align = 'left', maxW = 0} = {}) {
    const layout = PangoCairo.create_layout(cr);
    layout.set_font_description(font(size, bold));
    if (maxW > 0) {
        layout.set_width(Math.max(1, Math.floor(maxW * Pango.SCALE)));
        layout.set_ellipsize(Pango.EllipsizeMode.END);
    }
    layout.set_text(str, -1);
    const [w] = layout.get_pixel_size();
    const left = align === 'right' ? x - w : align === 'center' ? x - w / 2 : x;
    rgba(cr, rgb, a);
    cr.moveTo(left, baseline - layout.get_baseline() / Pango.SCALE);
    PangoCairo.show_layout(cr, layout);
    return w;
}

function textWidth(cr, str, size, bold = false) {
    const layout = PangoCairo.create_layout(cr);
    layout.set_font_description(font(size, bold));
    layout.set_text(str, -1);
    return layout.get_pixel_size()[0];
}

/// Provider marks: each SVG is one monochrome path, used as an alpha mask and tinted here.
class Marks {
    constructor(dir) {
        this._dir = dir;
        this._cache = new Map();
    }

    _surface(provider, size) {
        const key = `${provider}:${size}`;
        if (this._cache.has(key))
            return this._cache.get(key);
        let surface = null;
        try {
            const handle = Rsvg.Handle.new_from_file(`${this._dir}/icons/${provider}.svg`);
            surface = new Cairo.ImageSurface(Cairo.Format.ARGB32, size, size);
            const c = new Cairo.Context(surface);
            handle.render_document(c, new Rsvg.Rectangle({x: 0, y: 0, width: size, height: size}));
            c.$dispose();
        } catch (e) {
            console.warn(`codenotch: cannot draw the ${provider} mark: ${e}`);
            surface = null;
        }
        this._cache.set(key, surface);
        return surface;
    }

    draw(cr, provider, cx, cy, size, rgb, a) {
        const n = Math.max(1, Math.round(size));
        const s = this._surface(provider, n);
        if (!s)
            return;
        rgba(cr, rgb, a);
        cr.maskSurface(s, Math.round(cx - n / 2), Math.round(cy - n / 2));
    }

    clear() {
        this._cache.clear();
    }
}

// ------------------------------------------------------------------ the extension

export default class CodenotchExtension extends Extension {
    enable() {
        this._marks = new Marks(this.path);
        this._readings = [];
        this._work = {agg: 'idle', sessions: []};
        this._shown = false;
        this._card = null;
        this._pushedAt = 0;
        this._prevX = Number.MAX_SAFE_INTEGER;
        this._leftAt = 0;
        this._sinceTick = 0;
        this._t0 = GLib.get_monotonic_time();

        this._clip = new St.Widget({clip_to_allocation: true, reactive: false});
        this._pill = new St.DrawingArea({reactive: true});
        this._pill.connect('repaint', a => this._paintPill(a));
        this._pill.connect('button-press-event', (_a, ev) => this._onPress(ev));
        this._clip.add_child(this._pill);
        Main.layoutManager.addTopChrome(this._clip, {trackFullscreen: true});

        this._cardActor = new St.DrawingArea({reactive: true, opacity: 0, visible: false});
        this._cardActor.connect('repaint', a => this._paintCard(a));
        Main.layoutManager.addTopChrome(this._cardActor, {trackFullscreen: true});

        this._monitorsId = Main.layoutManager.connect('monitors-changed', () => this._layout());
        this._poll();
        this._layout();
        this._timer = GLib.timeout_add(GLib.PRIORITY_DEFAULT, TICK_MS, () => {
            this._tick();
            return GLib.SOURCE_CONTINUE;
        });
    }

    disable() {
        if (this._timer)
            GLib.source_remove(this._timer);
        this._timer = 0;
        if (this._monitorsId)
            Main.layoutManager.disconnect(this._monitorsId);
        this._monitorsId = 0;
        for (const a of [this._clip, this._cardActor]) {
            if (!a)
                continue;
            Main.layoutManager.removeChrome(a);
            a.destroy();
        }
        this._clip = this._pill = this._cardActor = null;
        this._marks?.clear();
        this._marks = null;
    }

    // -------------------------------------------------------------- data

    _poll() {
        const cfg = Data.readConfig();
        this._scale = cfg.scale;
        const before = this._readings.map(r => r.provider).join();
        this._readings = cfg.shown.map(provider => ({provider, ...Data.readingOf(Data.readSnapshot(provider))}));
        this._work = Data.readJson('sessions.json') ?? {agg: 'idle', sessions: []};
        if (before !== this._readings.map(r => r.provider).join() || this._scaleSeen !== this._scale) {
            this._scaleSeen = this._scale;
            if (this._card !== null && this._card >= this._readings.length)
                this._card = null;
            this._layout();
        }
        this._pill?.queue_repaint();
        this._cardActor?.queue_repaint();
    }

    /// Scale in physical pixels: the user's size preference times the monitor's.
    _s() {
        const factor = St.ThemeContext.get_for_stage(global.stage).scale_factor;
        return (this._scale || 1) * factor;
    }

    _geometry() {
        const s = this._s();
        const mon = Main.layoutManager.primaryMonitor;
        const cells = Math.max(1, this._readings.length);
        const pillH = Math.round((PAD_Y + PAD_BOTTOM + cells * CELL_H + (cells - 1) * GAP) * s);
        const w = Math.ceil(PILL_W * s);
        const h = pillH + Math.round(2 * FILLET * s);
        const x = mon.x + mon.width - w;
        const y = Math.round(mon.y + mon.height / 2 - h / 2);
        return {s, mon, w, h, x, y, pillTop: Math.round(FILLET * s), pillH};
    }

    _layout() {
        if (!this._clip)
            return;
        const g = this._geometry();
        this._clip.set_position(g.x, g.y);
        this._clip.set_size(g.w, g.h);
        this._pill.set_size(g.w, g.h);
        this._pill.translation_x = this._shown ? 0 : g.w - PEEK * g.s;
        this._pill.opacity = this._shown ? 255 : HIDDEN_OPACITY;
        this._placeCard();
        this._pill.queue_repaint();
    }

    _ringMid(g, i) {
        return g.pillTop + (PAD_Y + i * (CELL_H + GAP) + RING_BOX / 2) * g.s;
    }

    // -------------------------------------------------------------- pointer

    _tick() {
        if (!this._pill)
            return;
        const g = this._geometry();
        const [x, y] = global.get_pointer();
        const now = GLib.get_monotonic_time() / 1000;
        const right = g.mon.x + g.mon.width;
        const top = g.y + g.pillTop;
        const bottom = top + g.pillH;

        // Arming: a push outwards inside the strip at the very edge. Leaving the strip disarms.
        const edgeStrip = right - PEEK * g.s - 2;
        const atEdge = x >= edgeStrip - PEEK * g.s;
        if (!atEdge)
            this._pushedAt = 0;
        else if (x - this._prevX >= PUSH_STEP)
            this._pushedAt = now;
        this._prevX = x;

        const slack = this._shown ? HOVER_HYSTERESIS : 0;
        const inRows = y >= top - HOVER_PAD - slack && y <= bottom + HOVER_PAD + slack;
        const overPill = inRows && x >= right - g.w - HOVER_PAD - slack && x <= right;
        const overCard = this._card !== null && this._cardRect &&
            x >= this._cardRect.x - HOVER_PAD && x <= right &&
            y >= this._cardRect.y - HOVER_PAD && y <= this._cardRect.y + this._cardRect.h + HOVER_PAD;
        const inside = overPill || overCard;
        const pushedOut = inRows && this._pushedAt > 0 && now - this._pushedAt < PUSH_GRACE_MS;

        let want;
        if (this._shown && inside) {
            this._leftAt = 0;
            want = true;
        } else if (!this._shown) {
            want = pushedOut;
        } else {
            if (!this._leftAt)
                this._leftAt = now;
            want = now - this._leftAt < LEAVE_DWELL_MS;
        }
        if (want !== this._shown)
            this._setShown(want, g);

        this._sinceTick += TICK_MS;
        if (this._sinceTick >= POLL_MS) {
            this._sinceTick = 0;
            this._poll();
        }
        // The working arc turns and the waiting ring breathes: repaint while either is on screen.
        if (this._shown && (this._work.agg === 'running' || this._work.agg === 'attention'))
            this._pill.queue_repaint();
    }

    _setShown(shown, g) {
        this._shown = shown;
        this._pill.remove_all_transitions();
        this._pill.ease({
            translation_x: shown ? 0 : g.w - PEEK * g.s,
            opacity: shown ? 255 : HIDDEN_OPACITY,
            duration: shown ? 340 : 450,
            mode: shown ? Clutter.AnimationMode.EASE_OUT_BACK : Clutter.AnimationMode.EASE_IN_OUT_CUBIC,
        });
        if (shown)
            Data.requestRefresh();
        else
            this._openCard(null);
    }

    _onPress(ev) {
        const button = ev.get_button();
        if (button === 3) {
            this.openPreferences();
            return Clutter.EVENT_STOP;
        }
        if (button !== 1 || !this._shown)
            return Clutter.EVENT_PROPAGATE;
        const g = this._geometry();
        const [, sy] = ev.get_coords();
        const local = sy - g.y - g.pillTop - PAD_Y * g.s;
        const step = (CELL_H + GAP) * g.s;
        const i = Math.floor(local / step);
        const within = local - i * step;
        if (local >= 0 && i < this._readings.length && within <= CELL_H * g.s)
            this._openCard(this._card === i ? null : i);
        return Clutter.EVENT_STOP;
    }

    // -------------------------------------------------------------- card

    _cardHeight(r, cs) {
        const body = line(BODY_PX);
        const rows = Math.max(1, r.rows.length);
        let h = CARD_PAD * 2 + Math.max(MARK, line(TITLE_PX)) + HEADER_TO_BLOCK;
        h += rows * (body + LABEL_TO_BAR + BAR_H + BAR_TO_USED + body) + (rows - 1) * BLOCK_SPACING;
        const sessions = r.provider === 'claude' ? (this._work.sessions ?? []) : [];
        if (sessions.length)
            h += 2 * BLOCK_SPACING + HAIRLINE + sessions.length * body + (sessions.length - 1) * SESSION_GAP;
        return Math.ceil(h * cs);
    }

    _placeCard() {
        if (this._card === null || !this._readings[this._card]) {
            this._cardRect = null;
            return;
        }
        const g = this._geometry();
        const cs = g.s * CARD_ZOOM;
        const r = this._readings[this._card];
        const w = Math.ceil((CARD_W + TAIL_W) * cs);
        const h = this._cardHeight(r, cs);
        const ringMid = g.y + this._ringMid(g, this._card);
        const right = g.mon.x + g.mon.width - g.w - TAIL_GAP * cs;
        const y = Math.round(Math.min(Math.max(ringMid - h / 2, g.mon.y), g.mon.y + g.mon.height - h));
        this._cardRect = {x: Math.round(right - w), y, w, h, tailY: ringMid - y};
        this._cardActor.set_position(this._cardRect.x, y);
        this._cardActor.set_size(w, h);
        this._cardActor.queue_repaint();
    }

    _openCard(i) {
        if (!this._cardActor)
            return;
        this._card = i;
        this._cardActor.remove_all_transitions();
        if (i === null) {
            this._cardActor.ease({
                opacity: 0,
                duration: 200,
                mode: Clutter.AnimationMode.EASE_OUT_QUAD,
                onComplete: () => {
                    if (this._cardActor && this._card === null)
                        this._cardActor.hide();
                },
            });
            return;
        }
        Data.requestRefresh();
        this._placeCard();
        this._cardActor.show();
        this._cardActor.translation_x = 12;
        this._cardActor.ease({
            opacity: 255,
            translation_x: 0,
            duration: 340,
            mode: Clutter.AnimationMode.EASE_OUT_CUBIC,
        });
    }

    // -------------------------------------------------------------- painting

    _paintPill(area) {
        const cr = area.get_context();
        try {
            const g = this._geometry();
            const s = g.s;
            const w = g.w;
            const top = g.pillTop;
            const bot = top + g.pillH;
            const f = FILLET * s;

            // Body, rounded on the left only, and the concave flares joining it to the screen edge.
            cr.setSourceRGBA(0, 0, 0, 1);
            roundRect(cr, 0, top, w + RADIUS * s, g.pillH, RADIUS * s);
            cr.fill();
            cr.moveTo(w, top - f);
            cr.lineTo(w, top);
            cr.lineTo(w - f, top);
            cr.arcNegative(w - f, top - f, f, Math.PI / 2, 0);
            cr.closePath();
            cr.fill();
            cr.moveTo(w, bot + f);
            cr.lineTo(w, bot);
            cr.lineTo(w - f, bot);
            cr.arc(w - f, bot + f, f, -Math.PI / 2, 0);
            cr.closePath();
            cr.fill();

            const cx = w / 2;
            const t = (GLib.get_monotonic_time() - this._t0) / 1e6;
            this._readings.forEach((r, i) => {
                // Text leaves cairo's current point behind, and `arc` would draw a line from it.
                cr.newPath();
                const cy = this._ringMid(g, i);
                const a = r.stale ? 0.55 : 1;
                const trackR = (RING_BOX - TRACK_STROKE) / 2 * s;
                cr.setLineCap(Cairo.LineCap.BUTT);
                cr.setLineWidth(TRACK_STROKE * s);
                rgba(cr, TRACK);
                cr.newPath();
                cr.arc(cx, cy, trackR, 0, 2 * Math.PI);
                cr.stroke();
                if (r.used !== null && r.used > 0) {
                    const frac = Math.min(1, r.used);
                    cr.setLineCap(Cairo.LineCap.ROUND);
                    cr.setLineWidth(PROGRESS_STROKE * s);
                    rgba(cr, Data.band(r.used), a);
                    cr.newPath();
                    cr.arc(cx, cy, trackR, -Math.PI / 2, -Math.PI / 2 + frac * 2 * Math.PI);
                    cr.stroke();
                }
                // Only Claude reports what a session is doing (through the hooks).
                if (r.provider === 'claude') {
                    const ar = (ACTIVITY_D - ACTIVITY_STROKE) / 2 * s;
                    cr.setLineCap(Cairo.LineCap.BUTT);
                    cr.setLineWidth(ACTIVITY_STROKE * s);
                    if (this._work.agg === 'running') {
                        const turn = (t / 1.2) % 1;
                        rgba(cr, INK, 0.95);
                        const a0 = -Math.PI / 2 + turn * 2 * Math.PI;
                        cr.newPath();
                        cr.arc(cx, cy, ar, a0, a0 + 0.28 * 2 * Math.PI);
                        cr.stroke();
                    } else if (this._work.agg === 'attention') {
                        const phase = Math.sin(t / 1.1 * 2 * Math.PI) * 0.5 + 0.5;
                        rgba(cr, WATCH, 0.25 + 0.75 * phase);
                        cr.newPath();
                        cr.arc(cx, cy, ar, 0, 2 * Math.PI);
                        cr.stroke();
                    }
                }
                this._marks.draw(cr, r.provider, cx, cy, MARK * s, INK, a);
                const pctTop = cy + RING_BOX / 2 * s + TEXT_GAP * s;
                text(cr, Data.pctLabel(r.used), cx, baselineIn(pctTop, PCT_PX * s), PCT_PX * s, INK, a,
                    {bold: true, align: 'center'});
            });
        } catch (e) {
            console.error(`codenotch: pill paint failed: ${e}`);
        } finally {
            cr.$dispose();
        }
    }

    _paintCard(area) {
        const cr = area.get_context();
        try {
            if (this._card === null || !this._cardRect)
                return;
            const r = this._readings[this._card];
            if (!r)
                return;
            const cs = this._geometry().s * CARD_ZOOM;
            const w = CARD_W * cs;
            const h = this._cardRect.h;
            const pad = CARD_PAD * cs;

            cr.setSourceRGBA(0, 0, 0, 1);
            roundRect(cr, 0, 0, w, h, CARD_RADIUS * cs);
            cr.fill();
            // The tail: notch.html's curved wedge, its base on the card, its point on the ring.
            const tw = TAIL_W * cs + 1;
            const th = TAIL_H * cs;
            const sx = tw / 32;
            const sy = th / 36;
            const x0 = w - 1;
            const y0 = this._cardRect.tailY - th / 2;
            cr.moveTo(x0, y0);
            cr.curveTo(x0, y0 + 9 * sy, x0 + 18.56 * sx, y0 + 13.68 * sy, x0 + 32 * sx, y0 + 18 * sy);
            cr.curveTo(x0 + 18.56 * sx, y0 + 22.32 * sy, x0, y0 + 27 * sy, x0, y0 + 36 * sy);
            cr.closePath();
            cr.fill();

            const left = pad;
            const right = w - pad;
            const inner = right - left;
            const bodyPx = BODY_PX * cs;
            const bodyLine = line(BODY_PX) * cs;
            let y = pad;

            const headH = Math.max(MARK, line(TITLE_PX)) * cs;
            this._marks.draw(cr, r.provider, left + MARK * cs / 2, y + headH / 2, MARK * cs, INK, 1);
            const titleTop = y + (headH - line(TITLE_PX) * cs) / 2;
            text(cr, `${Data.NAMES[r.provider]} Usage`, left + (MARK + HEADER_GAP) * cs,
                baselineIn(titleTop, TITLE_PX * cs), TITLE_PX * cs, INK, 1, {bold: true});
            y += headH + HEADER_TO_BLOCK * cs;

            if (!r.rows.length) {
                // With no numbers, the fetcher's note is what says why: not signed in, not installed.
                const why = r.note || (r.status === 'absent' ? 'Not installed' : 'No reading yet');
                text(cr, why, left, baselineIn(y, bodyPx), bodyPx, MUTED, 1, {maxW: inner});
            }
            const now = Date.now();
            r.rows.forEach((row, n) => {
                if (n > 0)
                    y += BLOCK_SPACING * cs;
                const reset = row.resetsAt ? Data.resetCopy(row.resetsAt, now) : '';
                const rw = reset ? textWidth(cr, reset, bodyPx) : 0;
                const bl = baselineIn(y, bodyPx);
                text(cr, row.label, left, bl, bodyPx, INK, 1, {maxW: inner - rw - px(20) * cs});
                if (reset)
                    text(cr, reset, right, bl, bodyPx, MUTED, 1, {align: 'right'});
                y += bodyLine + LABEL_TO_BAR * cs;
                const bh = BAR_H * cs;
                rgba(cr, BAR_TRACK);
                roundRect(cr, left, y, inner, bh, bh / 2);
                cr.fill();
                if (row.used !== null && row.used > 0) {
                    rgba(cr, Data.band(row.used));
                    roundRect(cr, left, y, Math.max(bh, inner * Math.min(1, row.used)), bh, bh / 2);
                    cr.fill();
                }
                y += bh + BAR_TO_USED * cs;
                if (row.used !== null)
                    text(cr, `${Data.pctLabel(row.used)} Used`, left, baselineIn(y, bodyPx), bodyPx, INK, 1);
                y += bodyLine;
            });

            const sessions = r.provider === 'claude' ? (this._work.sessions ?? []) : [];
            if (sessions.length) {
                y += BLOCK_SPACING * cs;
                rgba(cr, RULE);
                cr.rectangle(left, y, inner, HAIRLINE * cs);
                cr.fill();
                y += (HAIRLINE + BLOCK_SPACING) * cs;
                sessions.forEach((sess, n) => {
                    if (n > 0)
                        y += SESSION_GAP * cs;
                    const bl = baselineIn(y, bodyPx);
                    const tw2 = text(cr, sess.title, left, bl, bodyPx, INK, 1, {maxW: inner * 0.5});
                    const room = inner - tw2 - 8 * cs;
                    if (room > 12 * cs)
                        text(cr, sess.what, right, bl, bodyPx, MUTED, 1, {align: 'right', maxW: room});
                    y += bodyLine;
                });
            }
        } catch (e) {
            console.error(`codenotch: card paint failed: ${e}`);
        } finally {
            cr.$dispose();
        }
    }
}
