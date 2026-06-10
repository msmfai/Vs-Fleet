# Fleet Host Artifacts

This directory is the source-controlled home for host-side Fleet visual probes.
Generated keepalive runs go under `keepalive/<timestamp>/` and include:

- `host-keepalive.json`: review-server-compatible report;
- `screenshots/*.png`: full-screen captures tagged with PNG metadata;
- `fleet-host.log`: host stdout/stderr for bridge/editor lifecycle evidence;
- `rss.json` and `rss.txt`: process RSS snapshot.

Browse a run with:

```sh
node containers/fleet-env/eval/scripts/review-server.mjs \
  --json crates/fleet-host/artifacts/keepalive/<run>/host-keepalive.json \
  --dir crates/fleet-host/artifacts/keepalive/<run>
```

On macOS the probe uses `osascript`/System Events to locate and click the Fleet
window. If it fails with `not allowed assistive access`, grant Accessibility
permission to the terminal app running the probe, then rerun it.
