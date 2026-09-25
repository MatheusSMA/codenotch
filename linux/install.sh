#!/usr/bin/env bash
# Installs Codenotch on Fedora with GNOME:
#   1. builds codenotch-daemon (hook server + usage fetchers) and codenotch-hook,
#   2. runs the daemon as a systemd user service,
#   3. installs the GNOME Shell extension that draws the pill.
# Run it from a clone of the repository, as your normal user (it asks for sudo only for dnf).
set -euo pipefail

UUID="codenotch@matheussma.github.io"
REPO="$(cd "$(dirname "$0")/.." && pwd)"
BIN="$HOME/.local/share/codenotch"
EXT="$HOME/.local/share/gnome-shell/extensions/$UUID"
UNIT="$HOME/.config/systemd/user/codenotch.service"

say() { printf '\n\033[1m%s\033[0m\n' "$*"; }

say "1/4  Build tools"
need=()
command -v cargo >/dev/null || need+=(cargo rust)
command -v cc >/dev/null || need+=(gcc)
rpm -q openssl-devel >/dev/null 2>&1 || need+=(openssl-devel)
rpm -q pkgconf-pkg-config >/dev/null 2>&1 || need+=(pkgconf-pkg-config)
if ((${#need[@]})); then
    sudo dnf install -y "${need[@]}"
else
    echo "already installed"
fi

say "2/4  Building (the first build takes a few minutes)"
(cd "$REPO/windows" && cargo build --release -p codenotch-daemon -p codenotch-hook)

say "3/4  Background service"
# Stop first: a running binary cannot be replaced in place.
systemctl --user stop codenotch.service 2>/dev/null || true
install -Dm755 "$REPO/windows/target/release/codenotch-daemon" "$BIN/codenotch-daemon"
install -Dm755 "$REPO/windows/target/release/codenotch-hook" "$BIN/codenotch-hook"
mkdir -p "$(dirname "$UNIT")"
cat > "$UNIT" <<EOF
[Unit]
Description=Codenotch: AI usage limits for the GNOME pill
After=network-online.target

[Service]
ExecStart=$BIN/codenotch-daemon
Restart=on-failure
RestartSec=5

[Install]
WantedBy=default.target
EOF
systemctl --user daemon-reload
systemctl --user enable --now codenotch.service
systemctl --user --no-pager status codenotch.service | head -n 3 || true

say "4/4  GNOME Shell extension"
rm -rf "$EXT"
mkdir -p "$EXT"
cp -r "$REPO/linux/$UUID/." "$EXT/"
if gnome-extensions enable "$UUID" 2>/dev/null; then
    enabled_now=1
else
    # GNOME on Wayland only discovers a new extension at login. Add it to the enabled list now so
    # it comes up on its own after logging back in.
    enabled_now=0
    current="$(gsettings get org.gnome.shell enabled-extensions)"
    if [[ "$current" != *"$UUID"* ]]; then
        if [[ "$current" == "@as []" || "$current" == "[]" ]]; then
            gsettings set org.gnome.shell enabled-extensions "['$UUID']"
        else
            gsettings set org.gnome.shell enabled-extensions "${current%]}, '$UUID']"
        fi
    fi
fi
gsettings set org.gnome.shell disable-user-extensions false

say "Done."
if ((enabled_now)); then
    echo "The pill is on the right edge of your main screen: push the mouse against the edge to open it."
else
    echo "Log out and back in once so GNOME loads the extension. Then push the mouse against the"
    echo "right edge of your main screen to open the pill."
fi
echo
echo "Choose your AIs:   gnome-extensions prefs $UUID   (or right-click the pill)"
echo "Service log:       ~/.config/codenotch/daemon.log"
