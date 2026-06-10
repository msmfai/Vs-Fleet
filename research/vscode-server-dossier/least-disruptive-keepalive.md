# Least-Disruptive Keepalive Plan

Date: 2026-06-10

## Decision

The least disruptive route to "all Fleet tabs stay connected" is a persistent
child-webview pool implemented almost entirely inside `crates/fleet-host/src/mux.rs`.

Keep the existing rail UI, bridge protocol, reporter, spawned-server supervisor,
and native menu command forwarding. Replace only the singleton editor surface
with one editor child webview per Fleet server, selected by visibility and
layout instead of by navigating the only editor webview to a new URL.

The first implementation should be feature-gated, for example:

```text
FLEET_EDITOR_KEEPALIVE=1
```

That gate is a rollout guard, not the target product behavior. After VM
screenshots, bridge-generation checks, and memory measurements pass, flip the
default to persistent editors.

## Why This Is The Smallest Useful Change

Current code shape:

- `build_window` creates one rail child webview and one singleton editor child
  webview.
- `select_impl` stores the selected server id, finds the server URL, and
  navigates the singleton editor when the URL changes.
- `retile` sizes only the rail and singleton editor.
- `ui/main.js` only invokes `select_server(id)` and renders selection state.
- `bridge.rs` only cares which server id is active when forwarding commands.

That means the public behavior boundary can stay the same:

- rail click still calls `select_server(id)`;
- autospawn still calls `select_spawned(app, id)`;
- native menu forwarding still asks for the selected server id;
- bridge phone-home messages do not need a schema change;
- reporter/hub code does not need to know how editor surfaces are hosted.

The singleton editor is the part causing the disconnect: switching servers
navigates away from the old VS Code workbench page, destroying the page that owns
that workbench client's sockets. Persistent editors keep one loaded workbench
document per Fleet server and switch only which document is visible.

## Implementation Shape

Use a mode split while developing:

```rust
enum EditorMode {
    Singleton,
    Persistent,
}
```

Keep the current singleton path available until the VM suite proves persistent
editors are stable. In persistent mode:

1. Replace `MuxState.loaded: Mutex<Option<String>>` with per-server editor
   metadata:

   ```rust
   struct EditorEntry {
       label: String,
       url: String,
       loaded: bool,
   }
   ```

   Store this as `Mutex<HashMap<String, EditorEntry>>`. Prefer storing labels
   rather than long-lived `Webview` handles, and recover handles with
   `app.get_webview(&entry.label)` when needed.

2. Generate stable, unique webview labels from server ids:

   ```text
   editor:<escaped-server-id>
   ```

   Use a small helper that restricts labels to safe characters, because server
   ids can later come from external push registrations.

3. On `select_impl(app, id, force_navigate)` in persistent mode:

   - set `selected = Some(id.clone())`;
   - resolve the URL via `server_url(app, &id)`;
   - ensure the server has an editor child webview, creating it on demand;
   - navigate that child only if it has never loaded this URL, the URL changed,
     or `force_navigate` is true during startup retry;
   - hide or park the previously selected editor;
   - show, focus, and retile the newly selected editor.

4. On `retile(app)`:

   - keep the rail at `RAIL_W`;
   - size only the selected editor to the pane;
   - keep inactive editors hidden or parked according to the strategy below.

5. On `close_server(app, sup, id)`:

   - close and remove the server's editor webview;
   - call `sup.close(&id)`;
   - emit `SERVERS_CHANGED`;
   - if the closed server was selected, choose another displayed server or leave
     the editor pane blank.

6. Keep `select_spawned` retry logic, but make it per-server. A startup retry
   should only force-navigation for that server's child webview; it should not
   disturb any other visible or hidden editor.

7. Add log fields for `mode`, `server_id`, `editor_label`, `url`, and `force`.
   The VM tests should be able to show whether a switch caused visibility work
   only, or an unintended navigation/reload.

## Visibility Strategy

Start with Tauri's real webview visibility APIs:

- selected editor: `show()`, `set_position(...)`, `set_size(...)`, `set_focus()`;
- inactive editors: `hide()`.

Build editor webviews with background throttling disabled where Tauri supports
it:

```rust
WebviewBuilder::new(label, tauri::WebviewUrl::External(url))
    .background_throttling(BackgroundThrottlingPolicy::Disabled)
```

Tauri's docs/source say hidden or minimized browser views may throttle timers
and even unload after roughly five minutes by default. The same docs say the
background throttling policy is supported on macOS 14+ and unsupported on Linux,
Windows, and Android. Since Fleet is currently chasing macOS behavior first,
this is the right first mitigation, but it must be verified in a soak test.

If hidden webviews still disconnect, use this fallback order:

1. Park inactive editors offscreen at the normal editor-pane size while keeping
   them shown. This may avoid the document-hidden path, but it can reintroduce
   renderer/artifact risk, so it needs screenshots.
2. Keep selected plus a warm LRU pool. This lowers memory and blast radius but
   does not satisfy "all tabs at all times"; treat it as a safety fallback only.
3. Use separate hidden native windows only if child webviews cannot be made
   stable. This is more disruptive because it risks macOS Alt-Tab/window
   artifacts and complicates focus.

Do not return to 1x1 or zero-size hidden editor views as the first fallback. The
current source comments already record churn/garble from occluded/minimal views,
and the recent black artifact report makes that path suspect.

## What To Leave Alone Initially

- `crates/fleet-host/ui/main.js`: selection and rendering can stay as-is.
- `crates/fleet-host/src/bridge.rs`: no bridge protocol change is needed for
  persistent editor lifetime.
- `packages/fleet-bridge`: no extension activation change is needed for this
  specific keepalive fix.
- `crates/fleet-host/src/reporter.rs` and hub/inbox code: not involved in
  editor lifetime.
- Native menu command list: still forwards to the selected server id.
- Icon/build packaging work: unrelated to keepalive.

One related but separate fix should follow: Fleet-spawned servers should remain
listed while their supervisor process exists, even if the bridge reconnects.
That makes "disconnected" a status, not an accidental tab deletion. It is
valuable, but it does not replace persistent editor webviews.

## Verification Before Flipping The Default

Add or extend one VM behavior around a single Fleet app run:

1. Start Fleet with `FLEET_EDITOR_KEEPALIVE=1`.
2. Spawn two `code serve-web` servers.
3. Wait for both bridge hellos and record bridge generations.
4. Open an integrated terminal in each server and print a unique marker.
5. Switch between the two servers at least 20 times.
6. Leave one editor inactive for more than six minutes, then switch back.
7. Capture full-window screenshots that include both the rail and editor pane.
8. Record process RSS for Fleet, VS Code server processes, WebKit/Code Helper
   renderers, and reporters.

Acceptance:

- switching does not increment either bridge generation;
- switching does not log `server deregistered (bridge dropped)`;
- each terminal marker is still visible after switching back;
- screenshots show the rail, selected tab, VS Code tabs, and editor contents;
- no black pane, cropped editor, missing rail, or offscreen artifact appears;
- memory growth per hidden editor is measured and does not approach local memory
  pressure at the chosen default tab count.

## First Patch Boundary

The implementation patch should mostly touch `crates/fleet-host/src/mux.rs`.
If it needs to touch more than the following files, stop and re-check the design:

- `crates/fleet-host/src/mux.rs`;
- one VM/eval behavior or screenshot probe;
- docs for the new environment flag while it is gated.

That boundary is the main reason this plan is lower risk than a client/server
rewrite or bridge-first architecture change.
