use std::{fs, path::Path};

#[test]
fn fleet_shell_has_no_app_wide_keyboard_capture() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let files = [
        "src/main.rs",
        "src/mux.rs",
        "ui/main.js",
        "ui/index.html",
        "ui/styles.css",
        "../../packages/fleet-bridge/src/extension.ts",
    ];
    let forbidden = [
        "document.addEventListener(\"keydown\"",
        "document.addEventListener('keydown'",
        "window.addEventListener(\"keydown\"",
        "window.addEventListener('keydown'",
        "document.addEventListener(\"keyup\"",
        "document.addEventListener('keyup'",
        "window.addEventListener(\"keyup\"",
        "window.addEventListener('keyup'",
        "document.addEventListener(\"keypress\"",
        "document.addEventListener('keypress'",
        "window.addEventListener(\"keypress\"",
        "window.addEventListener('keypress'",
        ".accelerator(",
        "CmdOrCtrl",
        "Cmd/Ctrl",
    ];

    for rel in files {
        let path = manifest.join(rel);
        let contents = fs::read_to_string(&path).unwrap_or_else(|err| {
            panic!("failed to read {}: {err}", path.display());
        });
        for pattern in forbidden {
            assert!(
                !contents.contains(pattern),
                "{} must not contain top-level keyboard capture pattern {:?}",
                rel,
                pattern
            );
        }
    }
}

#[test]
fn native_menu_is_not_rebuilt_after_startup() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let path = manifest.join("src/mux.rs");
    let contents = fs::read_to_string(&path).unwrap_or_else(|err| {
        panic!("failed to read {}: {err}", path.display());
    });
    assert_eq!(
        contents.matches("set_menu(").count(),
        1,
        "Fleet must not rebuild the native menu after startup; set_menu closes open macOS menus"
    );
    assert!(
        contents.contains("pub fn refresh_menu(app: &AppHandle) {\n    let _ = app;\n}"),
        "refresh_menu must stay a no-op so bridge/register/selection churn does not close macOS menus"
    );
}
