//! Single-instance refusal at the daemon level (PLAN D2; G0: "lockfile/
//! single-instance").
//!
//! The `lockfile` unit tests prove the `InstanceLock` primitive. This test
//! proves the *daemon* honors it: launching the real `fleet-hub` binary twice
//! against the same runtime dir makes the second process refuse and exit with
//! the distinct lock exit code (2), while the first stays up.

use std::process::Command;
use std::time::Duration;

/// Path to the compiled `fleet-hub` binary under test.
fn hub_bin() -> std::path::PathBuf {
    // Cargo exposes the integration-test binary's own dir; the crate binaries
    // live alongside it (target/<profile>/fleet-hub).
    let mut p = std::env::current_exe().expect("test exe path");
    p.pop(); // the test binary file
    if p.ends_with("deps") {
        p.pop(); // out of deps/ into <profile>/
    }
    p.push("fleet-hub");
    p
}

fn unique_runtime_dir(tag: &str) -> std::path::PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!(
        "fleet-hub-it-{}-{}-{}",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    p
}

#[test]
fn second_daemon_refuses_with_lock_exit_code() {
    let bin = hub_bin();
    assert!(bin.exists(), "hub binary not built at {}", bin.display());
    let runtime_dir = unique_runtime_dir("single");

    // First daemon: long-lived. Bind WS on an ephemeral port so we don't
    // collide with a real Hub or a parallel test.
    let mut first = Command::new(&bin)
        .env("FLEET_RUNTIME_DIR", &runtime_dir)
        // Ephemeral WS port so we never collide with a real Hub on the default.
        .env("FLEET_WS_PORT", "0")
        // Keep logs quiet; not asserted here.
        .env("RUST_LOG", "off")
        .spawn()
        .expect("spawn first hub");

    // Give the first instance a moment to acquire the lock and bind.
    std::thread::sleep(Duration::from_millis(800));
    // It must still be running (never auto-exits, D2).
    assert!(
        first.try_wait().expect("try_wait first").is_none(),
        "first hub should still be running"
    );

    // Second daemon: same runtime dir → must refuse.
    let second = Command::new(&bin)
        .env("FLEET_RUNTIME_DIR", &runtime_dir)
        .env("FLEET_WS_PORT", "0")
        .env("RUST_LOG", "off")
        .output()
        .expect("run second hub");

    // Exit code 2 is the lock-refusal code from main.rs.
    assert_eq!(
        second.status.code(),
        Some(2),
        "second hub should refuse with exit code 2; stderr: {}",
        String::from_utf8_lossy(&second.stderr)
    );

    // Cleanup: stop the first daemon.
    let _ = first.kill();
    let _ = first.wait();
    let _ = std::fs::remove_dir_all(&runtime_dir);
}
