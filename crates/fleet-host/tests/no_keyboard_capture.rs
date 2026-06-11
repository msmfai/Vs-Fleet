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
        "set_focus(",
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
fn fleet_installs_one_static_native_shell_menu() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let main_path = manifest.join("src/main.rs");
    let main = fs::read_to_string(&main_path).unwrap_or_else(|err| {
        panic!("failed to read {}: {err}", main_path.display());
    });
    assert!(
        main.contains(".menu(mux::build_menu)"),
        "Fleet must install one static AppKit-aware shell menu"
    );
    assert!(
        main.contains(".on_menu_event(") && main.contains("strip_prefix(\"cmd:\")"),
        "Fleet must forward clicked VS Code menu commands through the active bridge"
    );
    assert!(
        !main.contains("enable_macos_default_menu(false)"),
        "disabling the default macOS menu leaves the top-level menu bar unstable"
    );

    let path = manifest.join("src/mux.rs");
    let contents = fs::read_to_string(&path).unwrap_or_else(|err| {
        panic!("failed to read {}: {err}", path.display());
    });
    assert_eq!(
        contents.matches("set_menu(").count(),
        0,
        "Fleet must not install or rebuild native menus; AppKit menu mutation closes open macOS menus"
    );
    assert!(
        contents.contains("pub fn refresh_menu(app: &AppHandle) {\n    let _ = app;\n}"),
        "refresh_menu must stay a no-op so bridge/register/selection churn does not close macOS menus"
    );
    assert!(
        contents.contains("pub fn build_menu<R: tauri::Runtime>"),
        "Fleet must define a static native shell menu"
    );
    assert!(
        contents.contains("\"File\"")
            && contents.contains("\"Edit\"")
            && contents.contains("\"Selection\"")
            && contents.contains("\"View\"")
            && contents.contains("\"Go\"")
            && contents.contains("\"Run\"")
            && contents.contains("\"Terminal\"")
            && contents.contains("workbench.action.terminal.new")
            && contents.contains("workbench.action.files.save")
            && contents.contains("workbench.action.showCommands"),
        "Fleet must keep the mirrored VS Code menu tree for child editor command pass-through"
    );
}
