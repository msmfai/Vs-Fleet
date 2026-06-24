//! The Hub link: subscribe to the Hub over WebSocket, fold its
//! `fleet.snapshot` + delta stream into the **real** [`fleet_host_core::InboxModel`]
//! reducer, and push every resulting view to the webview (managed state for the
//! initial `get_inbox` pull, plus a live `inbox` event on every change).
//!
//! Reconnects with a fixed backoff; a dropped link shows the last known tabs with
//! `connected:false` rather than blanking the window. This is the exact wire +
//! reducer the CLI and e2e faces use — nothing is faked.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use tauri::{AppHandle, Emitter, Manager, UserAttentionType};
use tauri_plugin_notification::NotificationExt;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use fleet_host_core::{
    focus::focus_command,
    notify::{view_transition, NotificationIntent, NotificationOutcome},
    InboxModel,
};
use fleet_hub::ClientMessage;
use fleet_protocol::{Command, Event, Target};

use crate::render::{render_at, RenderedInbox};

/// Shared latest-rendered-inbox, read by the `get_inbox` command on webview load.
pub type Shared = Arc<Mutex<RenderedInbox>>;

/// Thread-safe handle used by Tauri commands to send face→Hub commands on the
/// live Hub websocket owned by the background link.
#[derive(Clone)]
pub struct HubCommandSender {
    tx: mpsc::UnboundedSender<Command>,
}

impl HubCommandSender {
    pub fn send(&self, command: Command) -> Result<(), String> {
        self.tx
            .send(command)
            .map_err(|_| "hub command channel closed".to_string())
    }
}

pub fn command_channel() -> (HubCommandSender, mpsc::UnboundedReceiver<Command>) {
    let (tx, rx) = mpsc::unbounded_channel();
    (HubCommandSender { tx }, rx)
}

/// Run the Hub link forever: connect, subscribe, fold, push; reconnect on drop.
// Glue: the forever connect/reconnect loop, driving the Tauri `AppHandle` (window
// emits/notifications via `connect_once`/`mark_disconnected`). No headless exit;
// the pure folding decisions it delegates to are unit-tested.
#[cfg_attr(coverage_nightly, coverage(off))]
pub async fn run(
    app: AppHandle,
    shared: Shared,
    ws_url: String,
    mut command_rx: mpsc::UnboundedReceiver<Command>,
) {
    let mut model = InboxModel::new();
    let mut seen_snapshot = false;
    loop {
        if let Err(e) = connect_once(
            &app,
            &shared,
            &ws_url,
            &mut command_rx,
            &mut model,
            &mut seen_snapshot,
        )
        .await
        {
            tracing::warn!(error = %e, url = %ws_url, "hub link error; retrying");
        }
        // Link is down — show the last tabs as disconnected, then back off.
        mark_disconnected(&app, &shared);
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

// Glue: opens the real Hub WebSocket, subscribes, and folds the snapshot/delta
// stream into the reducer, pushing each rendered view through the `AppHandle`.
// Needs a live webview + a Hub socket; the pure pieces it composes
// (`event_derives_notifications`, `command_frame_text`, `render_at`, the reducer)
// are unit-tested in their own crates / here.
#[cfg_attr(coverage_nightly, coverage(off))]
async fn connect_once(
    app: &AppHandle,
    shared: &Shared,
    ws_url: &str,
    command_rx: &mut mpsc::UnboundedReceiver<Command>,
    model: &mut InboxModel,
    seen_snapshot: &mut bool,
) -> Result<()> {
    let (mut ws, _resp) = tokio_tungstenite::connect_async(ws_url).await?;
    ws.send(Message::Text(r#"{"type":"subscribe"}"#.into()))
        .await?;
    tracing::info!(url = %ws_url, "host face connected to Hub; subscribed");

    let mut commands_closed = false;

    loop {
        tokio::select! {
            maybe_command = command_rx.recv(), if !commands_closed => {
                match maybe_command {
                    Some(command) => {
                        ws.send(Message::Text(command_frame_text(command)?.into())).await?;
                    }
                    None => commands_closed = true,
                }
            }
            msg = ws.next() => {
                let Some(msg) = msg else { break; };
                let ev = match msg? {
                    Message::Text(txt) => serde_json::from_str::<Event>(&txt).ok(),
                    Message::Binary(bin) => serde_json::from_slice::<Event>(&bin).ok(),
                    Message::Close(_) => break,
                    Message::Ping(_) | Message::Pong(_) => None,
                    _ => None,
                };
                if let Some(ev) = ev {
                    let previous_view = model.view();
                    let derives_notifications = event_derives_notifications(&ev, *seen_snapshot);
                    let is_snapshot = matches!(&ev, Event::Snapshot { .. });
                    model.apply(ev);
                    if is_snapshot {
                        *seen_snapshot = true;
                    }
                    let now = fleet_hub::persist::now_iso();
                    let view = model.view();
                    let outcomes = if derives_notifications {
                        view_transition(&previous_view, &view)
                    } else {
                        Vec::new()
                    };
                    push(app, shared, render_at(&view, true, Some(&now)), &outcomes);
                }
            }
        }
    }
    Ok(())
}

fn event_derives_notifications(ev: &Event, seen_snapshot: bool) -> bool {
    !matches!(ev, Event::Snapshot { .. }) || seen_snapshot
}

fn command_frame_text(command: Command) -> Result<String> {
    Ok(serde_json::to_string(&ClientMessage::Command { command })?)
}

/// The mute/unmute command for a (un)mute toggle. Pure.
fn mute_toggle_command(session_id: String, muted: bool) -> Command {
    if muted {
        Command::mute(session_id)
    } else {
        Command::unmute(session_id)
    }
}

/// The command for a solo toggle. Pure. The protocol has no separate unsolo
/// command — per the S25 model, unmute on a soloed session clears solo mode and
/// restores normal pings.
fn solo_toggle_command(session_id: String, soloed: bool) -> Command {
    if soloed {
        Command::solo(session_id)
    } else {
        Command::unmute(session_id)
    }
}

// Thin Tauri command wrapper: gate on connection, then send the pure command on
// the live Hub socket. The State plumbing needs a running app; the command
// selection (`mute_toggle_command`) and the gate (`ensure_connected`) are tested.
#[cfg_attr(coverage_nightly, coverage(off))]
#[tauri::command]
pub fn set_session_muted(
    commands: tauri::State<'_, HubCommandSender>,
    shared: tauri::State<'_, Shared>,
    session_id: String,
    muted: bool,
) -> Result<(), String> {
    ensure_connected(&shared)?;
    commands.send(mute_toggle_command(session_id, muted))
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[tauri::command]
pub fn set_session_soloed(
    commands: tauri::State<'_, HubCommandSender>,
    shared: tauri::State<'_, Shared>,
    session_id: String,
    soloed: bool,
) -> Result<(), String> {
    ensure_connected(&shared)?;
    commands.send(solo_toggle_command(session_id, soloed))
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[tauri::command]
pub fn dismiss_session(
    commands: tauri::State<'_, HubCommandSender>,
    shared: tauri::State<'_, Shared>,
    session_id: String,
) -> Result<(), String> {
    ensure_connected(&shared)?;
    commands.send(Command::dismiss(Target::session(session_id)))
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[tauri::command]
pub fn focus_session(
    commands: tauri::State<'_, HubCommandSender>,
    shared: tauri::State<'_, Shared>,
    session_id: String,
) -> Result<(), String> {
    ensure_connected(&shared)?;
    commands.send(focus_command(&session_id))
}

fn ensure_connected(shared: &Shared) -> Result<(), String> {
    let connected = shared
        .lock()
        .ok()
        .map(|inbox| inbox.connected)
        .unwrap_or(false);
    if connected {
        Ok(())
    } else {
        Err("hub disconnected".to_string())
    }
}

/// Store the rendered inbox in shared state and emit it to the webview.
// Glue: writes shared state and `emit`s the `inbox` event + native window
// indicators/notifications through the `AppHandle`. The decision of whether to
// emit (`should_emit_inbox_update`) and what indicators to set
// (`window_indicator_update`) are unit-tested.
#[cfg_attr(coverage_nightly, coverage(off))]
fn push(
    app: &AppHandle,
    shared: &Shared,
    rendered: RenderedInbox,
    notification_outcomes: &[NotificationOutcome],
) {
    let previous = shared.lock().ok().map(|g| g.clone());
    if !should_emit_inbox_update(previous.as_ref(), &rendered, notification_outcomes) {
        tracing::debug!(
            connected = rendered.connected,
            tabs = rendered.tabs.len(),
            waiting = rendered.waiting_count,
            "unchanged inbox payload skipped"
        );
        return;
    }

    // Observability: the window renders from this exact payload, so logging it
    // proves (without needing to inspect the webview) what the window shows.
    let summary: Vec<String> = rendered
        .tabs
        .iter()
        .map(|t| format!("{}={}", t.title, t.state))
        .collect();
    tracing::info!(
        connected = rendered.connected,
        tabs = rendered.tabs.len(),
        waiting = rendered.waiting_count,
        "inbox → window: [{}]",
        summary.join(", ")
    );
    log_notification_outcomes(notification_outcomes);
    deliver_notification_outcomes(app, notification_outcomes);
    update_window_indicators(app, previous.as_ref(), &rendered, notification_outcomes);
    if let Ok(mut g) = shared.lock() {
        *g = rendered.clone();
    }
    let _ = app.emit("inbox", rendered);
}

fn should_emit_inbox_update(
    previous: Option<&RenderedInbox>,
    rendered: &RenderedInbox,
    notification_outcomes: &[NotificationOutcome],
) -> bool {
    if previous != Some(rendered) {
        return true;
    }
    notification_outcomes
        .iter()
        .any(|outcome| !matches!(outcome, NotificationOutcome::Noop))
}

// Glue: applies the computed indicator update to the native window (title /
// badge / dock attention) via the `AppHandle`. The computation
// (`window_indicator_update`) is unit-tested; this only pushes it to AppKit/Win32.
#[cfg_attr(coverage_nightly, coverage(off))]
fn update_window_indicators(
    app: &AppHandle,
    previous: Option<&RenderedInbox>,
    rendered: &RenderedInbox,
    notification_outcomes: &[NotificationOutcome],
) {
    let Some(window) = app.get_window(crate::mux::WINDOW) else {
        return;
    };
    let update = window_indicator_update(previous, rendered, notification_outcomes);

    if let Some(title) = update.title {
        let _ = window.set_title(&title);
    }
    if let Some(badge_count) = update.badge_count {
        let _ = window.set_badge_count(badge_count);
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(badge_label) = update.badge_label {
            let _ = window.set_badge_label(badge_label);
        }
    }

    match update.attention {
        AttentionUpdate::None => {}
        AttentionUpdate::Clear => {
            let _ = window.request_user_attention(None);
        }
        AttentionUpdate::Request if !window.is_focused().unwrap_or(false) => {
            let _ = window.request_user_attention(Some(UserAttentionType::Informational));
        }
        AttentionUpdate::Request => {}
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WindowIndicatorUpdate {
    title: Option<String>,
    badge_count: Option<Option<i64>>,
    #[cfg(target_os = "macos")]
    badge_label: Option<Option<String>>,
    attention: AttentionUpdate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AttentionUpdate {
    None,
    Clear,
    Request,
}

fn window_indicator_update(
    previous: Option<&RenderedInbox>,
    rendered: &RenderedInbox,
    notification_outcomes: &[NotificationOutcome],
) -> WindowIndicatorUpdate {
    let title = fleet_window_title(rendered);
    let previous_title = previous.map(fleet_window_title);
    let badge_count = waiting_badge_count(rendered.waiting_count);
    let previous_badge_count = previous.map(|inbox| waiting_badge_count(inbox.waiting_count));
    let previous_waiting = previous
        .map(|inbox| inbox.waiting_count)
        .unwrap_or_default();
    let should_alert =
        has_notification_fire(notification_outcomes) || rendered.waiting_count > previous_waiting;

    WindowIndicatorUpdate {
        title: (previous_title.as_deref() != Some(title.as_str())).then_some(title),
        badge_count: (previous_badge_count != Some(badge_count)).then_some(badge_count),
        #[cfg(target_os = "macos")]
        badge_label: {
            let badge_label = waiting_badge_label(rendered.waiting_count);
            let previous_badge_label =
                previous.map(|inbox| waiting_badge_label(inbox.waiting_count));
            (previous_badge_label != Some(badge_label.clone())).then_some(badge_label)
        },
        attention: if rendered.waiting_count == 0 && previous_waiting > 0 {
            AttentionUpdate::Clear
        } else if rendered.waiting_count > 0 && should_alert {
            AttentionUpdate::Request
        } else {
            AttentionUpdate::None
        },
    }
}

fn has_notification_fire(outcomes: &[NotificationOutcome]) -> bool {
    outcomes
        .iter()
        .any(|outcome| matches!(outcome, NotificationOutcome::Fire(_)))
}

// Observability glue: emits tracing lines whose fields are level-gated (and so
// not exercised at the default level). No return value / behavior to assert.
#[cfg_attr(coverage_nightly, coverage(off))]
fn log_notification_outcomes(outcomes: &[NotificationOutcome]) {
    for outcome in outcomes {
        match outcome {
            NotificationOutcome::Fire(intent) => tracing::info!(
                session_id = %intent.session_id,
                title = %intent.title,
                sound = intent.sound.map(|sound| sound.as_str()).unwrap_or(""),
                "notification intent fired"
            ),
            NotificationOutcome::AutoResolve { session_id } => tracing::info!(
                session_id = %session_id,
                "notification auto-resolved"
            ),
            NotificationOutcome::Noop => {}
        }
    }
}

// Glue: fires each Fire(intent) as a real desktop notification via the Tauri
// notification plugin (`AppHandle`). The decision of which outcomes fire
// (`has_notification_fire`) is unit-tested.
#[cfg_attr(coverage_nightly, coverage(off))]
fn deliver_notification_outcomes(app: &AppHandle, outcomes: &[NotificationOutcome]) {
    for outcome in outcomes {
        let NotificationOutcome::Fire(intent) = outcome else {
            continue;
        };
        if let Err(e) = send_notification(app, intent) {
            tracing::warn!(
                error = %e,
                session_id = %intent.session_id,
                "desktop notification failed"
            );
        }
    }
}

// Glue: builds and shows a Tauri desktop notification (`AppHandle` + the
// notification plugin). The stable id derivation (`notification_id`) is tested.
#[cfg_attr(coverage_nightly, coverage(off))]
fn send_notification(
    app: &AppHandle,
    intent: &NotificationIntent,
) -> tauri_plugin_notification::Result<()> {
    let mut builder = app
        .notification()
        .builder()
        .id(notification_id(&intent.session_id))
        .title(intent.title.clone())
        .body(intent.body.clone())
        .group("fleet")
        .extra("session_id", &intent.session_id)
        .auto_cancel();
    if let Some(sound) = intent.sound {
        builder = builder.sound(sound.as_str());
    }
    builder.show()
}

fn notification_id(session_id: &str) -> i32 {
    let mut hash: u32 = 0x811c9dc5;
    for byte in session_id.as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x01000193);
    }
    (hash & 0x7fff_ffff) as i32
}

fn fleet_window_title(rendered: &RenderedInbox) -> String {
    match (rendered.waiting_count, rendered.connected) {
        (0, true) => "Fleet".into(),
        (0, false) => "Fleet (Disconnected)".into(),
        (1, true) => "Fleet (1 waiting)".into(),
        (1, false) => "Fleet (1 waiting, disconnected)".into(),
        (n, true) => format!("Fleet ({n} waiting)"),
        (n, false) => format!("Fleet ({n} waiting, disconnected)"),
    }
}

fn waiting_badge_count(waiting_count: usize) -> Option<i64> {
    if waiting_count == 0 {
        None
    } else {
        Some(waiting_count as i64)
    }
}

#[cfg(target_os = "macos")]
fn waiting_badge_label(waiting_count: usize) -> Option<String> {
    (waiting_count > 0).then(|| waiting_count.to_string())
}

/// Flip the last-known inbox to `connected:false` (keep the tabs visible).
// Glue: mutates shared state and re-`push`es through the `AppHandle` so the
// window shows the disconnected banner. Reached only from the reconnect loop.
#[cfg_attr(coverage_nightly, coverage(off))]
fn mark_disconnected(app: &AppHandle, shared: &Shared) {
    let mut snapshot = shared.lock().ok().map(|g| g.clone()).unwrap_or_default();
    if snapshot.connected {
        snapshot.connected = false;
        push(app, shared, snapshot, &[]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inbox(waiting_count: usize, connected: bool) -> RenderedInbox {
        RenderedInbox {
            waiting_count,
            connected,
            ..Default::default()
        }
    }

    #[test]
    fn command_channel_delivers_then_reports_closed() {
        let (sender, mut rx) = command_channel();
        sender.send(Command::mute("s1")).unwrap();
        let received = rx.try_recv().unwrap();
        assert_eq!(
            command_frame_text(received).unwrap(),
            command_frame_text(Command::mute("s1")).unwrap()
        );

        // Dropping the receiver closes the channel; send then reports the error.
        drop(rx);
        assert_eq!(
            sender.send(Command::mute("s1")),
            Err("hub command channel closed".to_string())
        );
    }

    #[test]
    fn mute_toggle_selects_mute_or_unmute() {
        assert_eq!(
            command_frame_text(mute_toggle_command("s1".into(), true)).unwrap(),
            command_frame_text(Command::mute("s1")).unwrap()
        );
        assert_eq!(
            command_frame_text(mute_toggle_command("s1".into(), false)).unwrap(),
            command_frame_text(Command::unmute("s1")).unwrap()
        );
    }

    #[test]
    fn solo_toggle_selects_solo_or_unmute() {
        assert_eq!(
            command_frame_text(solo_toggle_command("s1".into(), true)).unwrap(),
            command_frame_text(Command::solo("s1")).unwrap()
        );
        // No unsolo command — clearing solo sends unmute.
        assert_eq!(
            command_frame_text(solo_toggle_command("s1".into(), false)).unwrap(),
            command_frame_text(Command::unmute("s1")).unwrap()
        );
    }

    #[test]
    fn hub_command_frame_is_flattened_for_wire_protocol() {
        let txt = command_frame_text(Command::mute("s1")).unwrap();
        let v: serde_json::Value = serde_json::from_str(&txt).unwrap();
        assert_eq!(v["type"], "command");
        assert_eq!(v["command"], "mute");
        assert_eq!(v["session_id"], "s1");
    }

    #[test]
    fn hub_command_frame_can_clear_solo_via_unmute() {
        let txt = command_frame_text(Command::unmute("s1")).unwrap();
        let v: serde_json::Value = serde_json::from_str(&txt).unwrap();
        assert_eq!(v["type"], "command");
        assert_eq!(v["command"], "unmute");
        assert_eq!(v["session_id"], "s1");
    }

    #[test]
    fn hub_command_frame_can_dismiss_session() {
        let txt = command_frame_text(Command::dismiss(Target::session("s1"))).unwrap();
        let v: serde_json::Value = serde_json::from_str(&txt).unwrap();
        assert_eq!(v["type"], "command");
        assert_eq!(v["command"], "dismiss");
        assert_eq!(v["target"]["type"], "session");
        assert_eq!(v["target"]["session_id"], "s1");
    }

    #[test]
    fn hub_command_frame_can_focus_session() {
        let txt = command_frame_text(Command::focus(Target::session("s1"))).unwrap();
        let v: serde_json::Value = serde_json::from_str(&txt).unwrap();
        assert_eq!(v["type"], "command");
        assert_eq!(v["command"], "focus");
        assert_eq!(v["target"]["type"], "session");
        assert_eq!(v["target"]["session_id"], "s1");
    }

    #[test]
    fn initial_snapshot_does_not_fire_notifications_but_reconnect_snapshot_can() {
        let snapshot = Event::snapshot(vec![]);
        let delta = Event::session_removed("s1");

        assert!(!event_derives_notifications(&snapshot, false));
        assert!(event_derives_notifications(&snapshot, true));
        assert!(event_derives_notifications(&delta, false));
    }

    #[test]
    fn ensure_connected_rejects_disconnected_inbox() {
        let connected = Arc::new(Mutex::new(inbox(0, true)));
        assert!(ensure_connected(&connected).is_ok());

        let disconnected = Arc::new(Mutex::new(inbox(0, false)));
        assert_eq!(
            ensure_connected(&disconnected),
            Err("hub disconnected".to_string())
        );
    }

    #[test]
    fn fleet_window_title_reflects_attention_and_connection() {
        assert_eq!(fleet_window_title(&inbox(0, true)), "Fleet");
        assert_eq!(fleet_window_title(&inbox(0, false)), "Fleet (Disconnected)");
        assert_eq!(fleet_window_title(&inbox(1, true)), "Fleet (1 waiting)");
        assert_eq!(
            fleet_window_title(&inbox(1, false)),
            "Fleet (1 waiting, disconnected)"
        );
        assert_eq!(fleet_window_title(&inbox(3, true)), "Fleet (3 waiting)");
        assert_eq!(
            fleet_window_title(&inbox(2, false)),
            "Fleet (2 waiting, disconnected)"
        );
    }

    #[test]
    fn waiting_badge_count_is_only_present_for_waiting_sessions() {
        assert_eq!(waiting_badge_count(0), None);
        assert_eq!(waiting_badge_count(3), Some(3));
    }

    #[test]
    fn repeated_inbox_update_does_not_touch_native_window_state() {
        let previous = inbox(0, true);
        let rendered = inbox(0, true);
        let update = window_indicator_update(Some(&previous), &rendered, &[]);

        assert_eq!(update.title, None);
        assert_eq!(update.badge_count, None);
        #[cfg(target_os = "macos")]
        assert_eq!(update.badge_label, None);
        assert_eq!(update.attention, AttentionUpdate::None);
    }

    #[test]
    fn repeated_inbox_update_is_not_emitted_to_webview() {
        let previous = inbox(0, true);
        let rendered = inbox(0, true);

        assert!(!should_emit_inbox_update(Some(&previous), &rendered, &[]));
        assert!(!should_emit_inbox_update(
            Some(&previous),
            &rendered,
            &[NotificationOutcome::Noop]
        ));
    }

    #[test]
    fn changed_inbox_update_is_emitted_to_webview() {
        let previous = inbox(0, true);
        let rendered = inbox(1, true);

        assert!(should_emit_inbox_update(Some(&previous), &rendered, &[]));
    }

    #[test]
    fn waiting_delta_updates_badge_title_and_attention_once() {
        let previous = inbox(0, true);
        let rendered = inbox(1, true);
        let update = window_indicator_update(Some(&previous), &rendered, &[]);

        assert_eq!(update.title, Some("Fleet (1 waiting)".into()));
        assert_eq!(update.badge_count, Some(Some(1)));
        #[cfg(target_os = "macos")]
        assert_eq!(update.badge_label, Some(Some("1".into())));
        assert_eq!(update.attention, AttentionUpdate::Request);
    }

    #[test]
    fn resolved_waiting_delta_clears_attention_once() {
        let previous = inbox(1, true);
        let rendered = inbox(0, true);
        let update = window_indicator_update(Some(&previous), &rendered, &[]);

        assert_eq!(update.title, Some("Fleet".into()));
        assert_eq!(update.badge_count, Some(None));
        #[cfg(target_os = "macos")]
        assert_eq!(update.badge_label, Some(None));
        assert_eq!(update.attention, AttentionUpdate::Clear);
    }

    #[test]
    fn notification_fire_outcome_requests_attention() {
        let fire = NotificationOutcome::Fire(fleet_host_core::notify::NotificationIntent {
            session_id: "s1".into(),
            title: "Approval needed".into(),
            body: "Agent is waiting".into(),
            sound: None,
        });
        assert!(has_notification_fire(&[fire]));
        assert!(!has_notification_fire(&[
            NotificationOutcome::AutoResolve {
                session_id: "s1".into(),
            }
        ]));
        assert!(!has_notification_fire(&[]));
    }

    #[test]
    fn notification_id_is_stable_positive_and_session_scoped() {
        let first = notification_id("session-alpha");
        assert_eq!(first, notification_id("session-alpha"));
        assert_ne!(first, notification_id("session-beta"));
        assert!(first >= 0);
    }
}
