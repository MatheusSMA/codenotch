#!/bin/bash
set -u
UUID=codenotch@matheussma.github.io
export XDG_RUNTIME_DIR=/tmp/xdg; mkdir -p -m700 $XDG_RUNTIME_DIR
E=~/.local/share/gnome-shell/extensions
mkdir -p $E ~/.config/codenotch
cp -r /ext/$UUID $E/; cp -r /test/unsafe@test $E/
now=$(( $(date +%s) * 1000 ))
cat > ~/.config/codenotch/usage.json <<J
{"status":"ok","fetched_at":$now,"note":"","windows":[{"id":"session","label":"Current session","used":0.73,"resets_at":$((now+51*60000))},{"id":"weekly_all","label":"All models","used":0.07,"resets_at":$((now+3*86400000))}]}
J
echo "{\"status\":\"ok\",\"fetched_at\":$now,\"windows\":[{\"id\":\"primary\",\"label\":\"5h limit\",\"used\":0.21,\"resets_at\":$((now+7200000))}]}" > ~/.config/codenotch/codex.json
echo "{\"status\":\"ok\",\"fetched_at\":$now,\"windows\":[{\"id\":\"q\",\"label\":\"Gemini Pro\",\"used\":0.52,\"resets_at\":$((now+9000000))}]}" > ~/.config/codenotch/antigravity.json
echo '{"agg":"running","sessions":[{"title":"proj · s1","what":"Edit main.rs"}]}' > ~/.config/codenotch/sessions.json
echo '{"scale":1.0,"notch_slots":[{"provider":"claude"},{"provider":"codex"},{"provider":"antigravity"}]}' > ~/.config/codenotch/config.json
mkdir -p /run/dbus; dbus-daemon --system --fork 2>/dev/null
(python3 -m dbusmock --template logind --system >/dev/null 2>&1 &); sleep 2
dbus-run-session -- bash -c '
gsettings set org.gnome.shell enabled-extensions "[\"'$UUID'\", \"unsafe@test\"]"
gsettings set org.gnome.shell disable-user-extensions false
gsettings set org.gnome.desktop.background picture-uri ""
gsettings set org.gnome.desktop.background primary-color "#3a6ea5"
gnome-shell --headless --virtual-monitor 1600x1000 --wayland --no-x11 > /tmp/shell.log 2>&1 &
sleep 12
ev() { gdbus call --session --dest org.gnome.Shell --object-path /org/gnome/Shell --method org.gnome.Shell.Eval "$1"; }
shot() { gdbus call --session --dest org.gnome.Shell.Screenshot --object-path /org/gnome/Shell/Screenshot --method org.gnome.Shell.Screenshot.Screenshot false false "$1" >/dev/null; }
ev "Main.overview.hide(); 1"
sleep 2
ev "const e = Main.extensionManager.lookup(\"'$UUID'\"); \`state=\${e.state} error=\${e.error}\`"
shot /out/hidden.png
ev "global._vp = imports.gi.Clutter.get_default_backend().get_default_seat().create_virtual_device(imports.gi.Clutter.InputDeviceType.POINTER_DEVICE); 1"
mv() { ev "global._vp.notify_absolute_motion(global.get_current_time()*1000, $1, $2); 1" >/dev/null; }
# parked on the edge away from the pill, then sliding down: must NOT open
mv 800 500; sleep 0.5; mv 1599 100; sleep 1; mv 1599 300; sleep 0.5; mv 1599 500; sleep 1
ev "\`parked-slide shown=\${Main.extensionManager.lookup(\"'$UUID'\").stateObj._shown}\`"
# away, then a push to the edge level with the pill: must open
mv 1200 500; sleep 0.5; mv 1450 500; sleep 0.05; mv 1599 500; sleep 1
ev "\`push shown=\${Main.extensionManager.lookup(\"'$UUID'\").stateObj._shown}\`"
shot /out/shown.png
# click the first ring
geo=$(ev "const o=Main.extensionManager.lookup(\"'$UUID'\").stateObj; const g=o._geometry(); \`\${Math.round(g.x+g.w/2)} \${Math.round(g.y+o._ringMid(g,0))}\`")
xy=$(echo "$geo" | grep -oE "[0-9]+ [0-9]+")
mv ${xy% *} ${xy#* }; sleep 0.3
ev "global._vp.notify_button(global.get_current_time()*1000, 1, imports.gi.Clutter.ButtonState.PRESSED); global._vp.notify_button(global.get_current_time()*1000, 1, imports.gi.Clutter.ButtonState.RELEASED); 1" >/dev/null
sleep 1
ev "\`card=\${Main.extensionManager.lookup(\"'$UUID'\").stateObj._card}\`"
shot /out/card.png
# leave: must hide after the dwell
mv 600 500; sleep 1.5
ev "\`left shown=\${Main.extensionManager.lookup(\"'$UUID'\").stateObj._shown}\`"
# the configurator
echo "{\"status\":\"needsAuth\",\"windows\":[],\"note\":\"No Codex credential\"}" > ~/.config/codenotch/codex.json
echo "{\"status\":\"absent\",\"windows\":[]}" > ~/.config/codenotch/cursor.json
WAYLAND_DISPLAY=wayland-0 gnome-extensions prefs '$UUID' 2>/tmp/prefs.err; sleep 5
ev "global.display.list_all_windows().map(w=>w.get_title()).join(\"|\")"
ev "Main.overview.hide(); 1" >/dev/null; sleep 1.5
shot /out/prefs.png
mv 1004 477; sleep 0.3
ev "global._vp.notify_button(global.get_current_time()*1000, 1, imports.gi.Clutter.ButtonState.PRESSED); global._vp.notify_button(global.get_current_time()*1000, 1, imports.gi.Clutter.ButtonState.RELEASED); 1" >/dev/null
sleep 3
echo "config after click: $(cat ~/.config/codenotch/config.json | tr -d "[:space:]")"
ev "\`rings=\${Main.extensionManager.lookup(\"'$UUID'\").stateObj._readings.map(r=>r.provider).join()}\`"
cat /tmp/prefs.err | head -5
ev "const w=global.display.list_all_windows().find(w=>w.get_title()&&w.get_title().includes(\"Codenotch\")); w? \`\${w.get_frame_rect().x},\${w.get_frame_rect().y},\${w.get_frame_rect().width},\${w.get_frame_rect().height}\` : \"none\""
'
echo "--- shell log:"; grep -vE "dbus-daemon|goa|evolution|libedbus" /tmp/shell.log | tail -40
