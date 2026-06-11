# Fleet Host Artifacts

This directory is the local home for host-side Fleet visual probes. The probe
outputs are intentionally ignored for public source release because they can
contain local paths, process command lines, local URLs, screenshots, and other
machine-specific state.

Generated keepalive scratch runs go under ignored `keepalive/<timestamp>/`.
Local promoted review evidence can go under ignored `keepalive-reviewed/<date>/`
and includes:

- `host-keepalive.json`: review-server-compatible report;
- `screenshots/*.png`: direct Fleet-window captures tagged with PNG metadata;
- `visual-analysis.json`: classic CV geometry/band analysis for the captures;
- `cv/*.top-mask.png`: top-window edge/dark-strip masks for visual debugging;
- `fleet-host.log`: host stdout/stderr for bridge/editor lifecycle evidence;
- `rss.json` and `rss.txt`: process RSS snapshot.

Browse a local run with:

```sh
node containers/fleet-env/eval/scripts/review-server.mjs \
  --json crates/fleet-host/artifacts/keepalive-reviewed/<date>/host-keepalive.json \
  --dir crates/fleet-host/artifacts/keepalive-reviewed/<date>
```

On macOS the probe captures screenshots by CoreGraphics window id
(`screencapture -l <id>`), so the Fleet window does not need to be frontmost or
uncovered for screenshot evidence. It still uses `osascript`/System Events to
click rail rows during the switch test. If clicking fails with `not allowed
assistive access`, grant Accessibility permission to the terminal app running the
probe, then rerun it.

Mac titlebar regression captures should note `FLEET_MACOS_TITLEBAR_STYLE`.
Fleet defaults to `transparent` so child VS Code webviews do not render under the
native titlebar. Set `FLEET_MACOS_TITLEBAR_STYLE=overlay` only to reproduce or
compare the stale tab/titlebar strip behavior.

If a visual artifact is worth committing, curate it first: remove raw logs/RSS,
strip local absolute paths, and prefer a small documented screenshot over a whole
run directory.
