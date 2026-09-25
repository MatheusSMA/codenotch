# Codenotch on Fedora (GNOME)

Your AI coding limits (Claude, Codex, Cursor, Antigravity) as rings in a pill on the right edge of
the screen. Push the mouse against the right edge, level with the pill, to open it; click a ring for
details.

## Install

```bash
sudo dnf install -y git
git clone -b spin-composited https://github.com/MatheusSMA/codenotch.git
cd codenotch
./linux/install.sh
```

The script installs the build tools it needs (asks for your password once), builds the background
service, starts it, and installs the GNOME extension. The first build takes a few minutes. If it
says so, log out and back in once.

## Choose your AIs

Right-click the pill, or run:

```bash
gnome-extensions prefs codenotch@matheussma.github.io
```

Switch on the AIs you use. Each row says whether it is connected. To connect one, sign in with its
own app on this computer:

| AI | How it connects |
|----|-----------------|
| Claude | Install Claude Code and run `claude` once to sign in |
| Codex | Install the Codex CLI and run `codex login` |
| Cursor | Install Cursor and sign in inside the app |
| Antigravity | Install the Antigravity CLI (`agy`) and sign in |

Nothing is uploaded anywhere: the service reads each tool's own login on this machine and asks that
vendor for your usage.

## Update

```bash
cd codenotch && git pull && ./linux/install.sh
```

## If something is off

- Service status: `systemctl --user status codenotch`
- Service log: `~/.config/codenotch/daemon.log`
- Extension errors: `journalctl -b -o cat /usr/bin/gnome-shell | grep -i codenotch`
- Remove: `systemctl --user disable --now codenotch && gnome-extensions uninstall codenotch@matheussma.github.io`

## How it fits together

- `codenotch-daemon` (Rust, `windows/codenotch-daemon`) runs as a systemd user service. It reuses the
  Windows build's fetchers and hook server, and writes JSON to `~/.config/codenotch/`. It also wires
  Claude Code's hooks so the Claude ring can show a turn running or waiting on you.
- The GNOME Shell extension (`linux/codenotch@matheussma.github.io`) only reads those files and
  draws. GNOME on Wayland does not let a normal app pin a window to the screen edge, which is why
  the pill lives in the shell.
