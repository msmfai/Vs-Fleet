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
                        ws.send(Message::Text(command_frame_text(command)?)).await?;
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

#[tauri::command]
pub fn set_session_muted(
    commands: tauri::State<'_, HubCommandSender>,
    shared: tauri::State<'_, Shared>,
    session_id: String,
    muted: bool,
) -> Result<(), String> {
    ensure_connected(&shared)?;
    let command = if muted {
        Command::mute(session_id)
    } else {
        Command::unmute(session_id)
    };
    commands.send(command)
}

#[tauri::command]
pub fn set_session_soloed(
    commands: tauri::State<'_, HubCommandSender>,
    shared: tauri::State<'_, Shared>,
    session_id: String,
    soloed: bool,
) -> Result<(), String> {
    ensure_connected(&shared)?;
    let command = if soloed {
        Command::solo(session_id)
    } else {
        // The protocol has no separate unsolo command. Per the S25 model,
        // unmute on a soloed session clears solo mode and restores normal pings.
        Command::unmute(session_id)
    };
    commands.send(command)
}

#[tauri::command]
pub fn dismiss_session(
    commands: tauri::State<'_, HubCommandSender>,
    shared: tauri::State<'_, Shared>,
    session_id: String,
) -> Result<(), String> {
    ensure_connected(&shared)?;
    commands.send(Command::dismiss(Target::session(session_id)))
}

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
fn push(
    app: &AppHandle,
    shared: &Shared,
    rendered: RenderedInbox,
    notification_outcomes: &[NotificationOutcome],
) {
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
    let previous_waiting = shared
        .lock()
        .ok()
        .map(|g| g.waiting_count)
        .unwrap_or_default();
    log_notification_outcomes(notification_outcomes);
    deliver_notification_outcomes(app, notification_outcomes);
    update_window_indicators(app, &rendered, previous_waiting, notification_outcomes);
    if let Ok(mut g) = shared.lock() {
        *g = rendered.clone();
    }
    crate::mux::refresh_menu(app);
    let _ = app.emit("inbox", rendered);
}

fn update_window_indicators(
    app: &AppHandle,
    rendered: &RenderedInbox,
    previous_waiting: usize,
    notification_outcomes: &[NotificationOutcome],
) {
    let Some(window) = app.get_window(crate::mux::WINDOW) else {
        return;
    };

    let _ = window.set_title(&fleet_window_title(rendered));
    let _ = window.set_badge_count(waiting_badge_count(rendered.waiting_count));
    #[cfg(target_os = "macos")]
    {
        let _ = window.set_badge_label(waiting_badge_label(rendered.waiting_count));
    }

    let should_alert =
        has_notification_fire(notification_outcomes) || rendered.waiting_count > previous_waiting;

    if rendered.waiting_count == 0 {
        let _ = window.request_user_attention(None);
    } else if should_alert && !window.is_focused().unwrap_or(false) {
        let _ = window.request_user_attention(Some(UserAttentionType::Informational));
    }
}

fn has_notification_fire(outcomes: &[NotificationOutcome]) -> bool {
    outcomes
        .iter()
        .any(|outcome| matches!(outcome, NotificationOutcome::Fire(_)))
}

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
