use std::{fs, path::Path};

#[test]
fn fleet_shell_has_no_top_level_keyboard_capture() {
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
        "addEventListener(\"keydown\"",
        "addEventListener('keydown'",
        "addEventListener(\"keyup\"",
        "addEventListener('keyup'",
        "addEventListener(\"keypress\"",
        "addEventListener('keypress'",
        "onkeydown",
        "onkeyup",
        "onkeypress",
        "aria-keyshortcuts",
        ".accelerator(",
        "PredefinedMenuItem",
        "CmdOrCtrl",
        "Cmd/Ctrl",
        "Shift+F10",
        "ContextMenu",
        "shortcut",
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
