// The configurator: which AIs get a ring on the pill, whether each one is actually connected, and
// the pill's size. Writes the same ~/.config/codenotch/config.json the Windows build reads, so the
// pill (which re-reads it every two seconds) follows along without a restart.

import Adw from 'gi://Adw';
import Gtk from 'gi://Gtk';

import {ExtensionPreferences} from 'resource:///org/gnome/Shell/Extensions/js/extensions/prefs.js';

import * as Data from './data.js';

/// What the daemon last said about a provider, in words someone setting it up can act on.
function statusOf(provider) {
    const snap = Data.readSnapshot(provider);
    if (!snap)
        return 'No data yet. Is the codenotch service running? (systemctl --user status codenotch)';
    const r = Data.readingOf(snap);
    if (snap.status === 'absent')
        return `Not installed. ${Data.HOW_TO_CONNECT[provider]}`;
    if (snap.status === 'needsAuth')
        return `Not signed in. ${Data.HOW_TO_CONNECT[provider]}`;
    if (r.used !== null)
        return `Connected: ${Data.pctLabel(r.used)} used${r.stale ? ' (stale)' : ''}`;
    if (r.rows.length)
        return 'Connected';
    return r.note || snap.status || 'Waiting for the first reading';
}

const SIZES = [
    {label: 'Small', value: 0.8},
    {label: 'Normal', value: 1.0},
    {label: 'Large', value: 1.25},
];

export default class CodenotchPreferences extends ExtensionPreferences {
    fillPreferencesWindow(window) {
        const cfg = Data.readConfig();
        const shown = new Set(cfg.shown);

        const page = new Adw.PreferencesPage({title: 'Codenotch', icon_name: 'preferences-system-symbolic'});

        const ais = new Adw.PreferencesGroup({
            title: 'AIs on the pill',
            description: 'Turn on the ones you use. Each needs its own app or CLI signed in on this computer.',
        });
        const rows = [];
        for (const provider of Data.PROVIDERS) {
            const row = new Adw.SwitchRow({
                title: Data.NAMES[provider],
                subtitle: statusOf(provider),
                active: shown.has(provider),
            });
            row.connect('notify::active', () => {
                if (row.active) {
                    shown.add(provider);
                } else if (shown.size > 1) {
                    shown.delete(provider);
                } else {
                    // The pill needs at least one ring, or there is nothing left to right-click.
                    row.active = true;
                    return;
                }
                Data.writeConfig({shown: [...shown]});
            });
            rows.push([provider, row]);
            ais.add(row);
        }
        page.add(ais);

        const look = new Adw.PreferencesGroup({title: 'Size'});
        const sizes = new Gtk.StringList();
        SIZES.forEach(s => sizes.append(s.label));
        const current = SIZES.reduce((best, s, i) =>
            Math.abs(s.value - cfg.scale) < Math.abs(SIZES[best].value - cfg.scale) ? i : best, 1);
        const size = new Adw.ComboRow({title: 'Pill size', model: sizes, selected: current});
        size.connect('notify::selected', () => Data.writeConfig({scale: SIZES[size.selected].value}));
        look.add(size);
        page.add(look);

        const help = new Adw.PreferencesGroup({
            title: 'Showing the pill',
            description: 'Push the mouse against the right edge of the screen, level with the pill. ' +
                'Click a ring for its details; right-click the pill to open this window.',
        });
        const refresh = new Adw.ActionRow({title: 'Check connections again'});
        const button = new Gtk.Button({label: 'Refresh', valign: Gtk.Align.CENTER});
        button.connect('clicked', () => {
            Data.requestRefresh();
            for (const [provider, row] of rows)
                row.subtitle = statusOf(provider);
        });
        refresh.add_suffix(button);
        help.add(refresh);
        page.add(help);

        window.add(page);
        window.set_default_size(560, 640);
    }
}
