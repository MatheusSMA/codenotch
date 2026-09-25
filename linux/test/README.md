# Headless GNOME Shell test

Runs the extension inside a real GNOME Shell (Fedora 44, GNOME 50) with no screen, drives a virtual
mouse, and saves screenshots. Proves that the extension loads, that the reveal only answers a push
towards the edge, that clicking a ring opens the card, and that the preferences switches change the
pill.

```bash
docker build -t codenotch-gnome linux/test
docker run --rm -v "$PWD/linux:/ext:ro" -v "$PWD/linux/test:/test:ro" -v "$PWD/out:/out" codenotch-gnome bash /test/run.sh
```

Expected lines: `state=1`, `parked-slide shown=false`, `push shown=true`, `card=0`, `left shown=false`,
and a config with `cursor` added after the click. Screenshots land in `out/`.

`unsafe@test` is a test-only extension that turns on the shell's unsafe mode, which the D-Bus `Eval`
and `Screenshot` calls need. It is never installed by `install.sh`.
