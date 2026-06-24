//! End-to-end exercise of the `fleet_hub::run` daemon entry point.
//!
//! `run()` is the always-on Hub loop (D2: never auto-exits). These tests drive
//! it the way `main` does — build a `HubConfig`, spawn `run()`, then connect a
//! real WebSocket client and prove subscribe→snapshot + live broadcast work —
//! covering lock acquire, state setup (ephemeral AND persist branches), the GC
//! task spawn, and both the WS and unix listeners. Each test aborts the spawned
//! `run()` task at the end (the daemon never returns on its own).

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use fleet_hub::wire::ClientMessage;
use fleet_hub::HubConfig;
use fleet_protocol::{
    Event, Extra, Location, LocationGlyph, LocationKind, Server, ServerKind, Session, State,
};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

fn unique(tag: &str, ext: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!(
        "fleet-hub-daemon-{}-{}-{}.{}",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        ext
    ));
    p
}

fn sample_session(id: &str) -> Session {
    Session::new(
        id,
        "repo @ main",
        Location {
            kind: LocationKind::Local,
            label: "laptop".into(),
            glyph: LocationGlyph::Laptop,
            attach_hint: None,
            extra: Extra::new(),
        },
        Server {
            kind: ServerKind::Local,
            version: None,
            extra: Extra::new(),
        },
        State::Idle,
        "2026-06-08T00:00:00Z",
    )
}

async fn next_event<S>(ws: &mut tokio_tungstenite::WebSocketStream<S>) -> Event
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    loop {
        let msg = ws.next().await.expect("stream open").expect("frame ok");
        if let Message::Text(txt) = msg {
            return serde_json::from_str(&txt).expect("decodable event");
        }
    }
}

/// Build a config bound to an OS-assigned ephemeral WS port with all state under
/// a private temp dir, so concurrent test runs never collide on the fixed port
/// or the default socket/lock paths.
fn temp_config(tag: &str, persist: bool) -> HubConfig {
    let dir = unique(tag, "d");
    std::fs::create_dir_all(&dir).unwrap();
    // Unix-domain socket paths are bounded by SUN_LEN (~104 bytes), so the socket
    // lives directly under the temp dir with a short, unique name rather than in
    // the (longer) per-test directory.
    let mut sock = std::env::temp_dir();
    sock.push(format!(
        "fh-{}-{}.sock",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    HubConfig {
        ws_addr: "127.0.0.1:0".parse::<SocketAddr>().unwrap(),
        unix_path: sock,
        lock_path: dir.join("hub.lock"),
        db_path: dir.join("hub.db"),
        persist,
        reap_grace: Duration::from_secs(3600),
        session_ttl: Duration::from_secs(24 * 3600),
    }
}

/// The daemon binds an OS-assigned ephemeral port, which the test cannot know in
/// advance. Each test reserves a `:0` port to learn a free address, drops the
/// reservation, then hands that address to `run()` and retry-connects.

#[tokio::test]
async fn run_serves_subscribe_and_broadcast_ephemeral() {
    // Reserve an ephemeral port, learn it, then release and hand it to the daemon.
    let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = probe.local_addr().unwrap();
    drop(probe);

    let mut config = temp_config("ephemeral", false);
    config.ws_addr = addr;

    let handle = tokio::spawn(async move {
        if let Err(e) = fleet_hub::run(config).await {
            eprintln!("daemon run() exited with error: {e:?}");
        }
    });

    // Retry-connect until the daemon's listener is up.
    let url = format!("ws://{addr}");
    let mut sub = connect_with_retry(&url).await;

    // subscribe → empty snapshot.
    let subscribe = serde_json::to_string(&ClientMessage::Subscribe).unwrap();
    sub.send(Message::Text(subscribe.into())).await.unwrap();
    match next_event(&mut sub).await {
        Event::Snapshot { sessions, .. } => assert!(sessions.is_empty()),
        other => panic!("expected empty snapshot, got {other:?}"),
    }

    // A reporter pushes a session → subscriber sees the live broadcast.
    let (mut rep, _r) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let upsert = serde_json::to_string(&ClientMessage::SessionUpsert {
        session: sample_session("s1"),
    })
    .unwrap();
    rep.send(Message::Text(upsert.into())).await.unwrap();
    match next_event(&mut sub).await {
        Event::SessionAdded { session, .. } => assert_eq!(session.session_id, "s1"),
        other => panic!("expected session.added, got {other:?}"),
    }

    handle.abort();
}

#[cfg(unix)]
#[tokio::test]
async fn run_serves_over_unix_socket_persist_mode() {
    let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = probe.local_addr().unwrap();
    drop(probe);

    // persist = true drives the `with_db` branch (durable restore path).
    let mut config = temp_config("persist", true);
    config.ws_addr = addr;
    let unix_path = config.unix_path.clone();

    let handle = tokio::spawn(async move {
        if let Err(e) = fleet_hub::run(config).await {
            eprintln!("daemon run() exited with error: {e:?}");
        }
    });

    // Push a session over the WS listener first.
    let url = format!("ws://{addr}");
    let mut rep = connect_with_retry(&url).await;
    let upsert = serde_json::to_string(&ClientMessage::SessionUpsert {
        session: sample_session("u1"),
    })
    .unwrap();
    rep.send(Message::Text(upsert.into())).await.unwrap();

    // Wait until the WS-pushed session is durably visible (the reporter's push is
    // async w.r.t. our send returning), then connect over the *unix* fast path and
    // confirm it serves the same state. We retry the whole subscribe so the
    // snapshot is taken AFTER the upsert has been projected — avoiding a race
    // between the snapshot and the concurrently-broadcast `session.added`.
    let mut saw_u1 = false;
    for _ in 0..200 {
        let stream = connect_unix_with_retry(&unix_path).await;
        let (mut sub, _resp) = tokio_tungstenite::client_async("ws://localhost/", stream)
            .await
            .unwrap();
        let subscribe = serde_json::to_string(&ClientMessage::Subscribe).unwrap();
        sub.send(Message::Text(subscribe.into())).await.unwrap();
        match next_event(&mut sub).await {
            Event::Snapshot { sessions, .. } => {
                if sessions.iter().any(|s| s.session_id == "u1") {
                    saw_u1 = true;
                    break;
                }
            }
            // A live `session.added` may arrive first if our subscribe raced the
            // projection; that equally proves the unix socket sees the same engine.
            Event::SessionAdded { session, .. } if session.session_id == "u1" => {
                saw_u1 = true;
                break;
            }
            _ => {}
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        saw_u1,
        "the WS-pushed session must become visible over the unix socket"
    );

    handle.abort();
}

#[tokio::test]
async fn run_refuses_second_instance_via_lockfile() {
    // First daemon acquires the lock; a second `run()` with the same lock path
    // must return an error (the D2 single-instance refusal), not start.
    let config1 = temp_config_shared_lock("refuse-1");
    let lock_path = config1.lock_path.clone();
    let db_path = config1.db_path.clone();
    let unix_path = config1.unix_path.clone();

    let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = probe.local_addr().unwrap();
    drop(probe);
    let mut config1 = config1;
    config1.ws_addr = addr;

    let handle = tokio::spawn(async move {
        let _ = fleet_hub::run(config1).await;
    });

    // Wait until the first daemon is up (its WS listener accepts).
    let url = format!("ws://{addr}");
    let _live = connect_with_retry(&url).await;

    // Second instance with the SAME lock path, a different WS port.
    let probe2 = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr2 = probe2.local_addr().unwrap();
    drop(probe2);
    let mut config2 = temp_config_shared_lock("refuse-2");
    config2.lock_path = lock_path; // collide on the lock
    config2.db_path = db_path;
    config2.unix_path = unix_path;
    config2.ws_addr = addr2;

    let result = fleet_hub::run(config2).await;
    assert!(
        result.is_err(),
        "a second run() sharing the lock path must refuse"
    );

    handle.abort();
}

fn temp_config_shared_lock(tag: &str) -> HubConfig {
    temp_config(tag, false)
}

async fn connect_with_retry(
    url: &str,
) -> tokio_tungstenite::WebSocketStream<
    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
> {
    for _ in 0..200 {
        if let Ok((ws, _)) = tokio_tungstenite::connect_async(url).await {
            return ws;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("daemon WS listener never came up at {url}");
}

#[cfg(unix)]
async fn connect_unix_with_retry(path: &std::path::Path) -> tokio::net::UnixStream {
    for _ in 0..200 {
        if let Ok(s) = tokio::net::UnixStream::connect(path).await {
            return s;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("daemon unix listener never came up at {}", path.display());
}
