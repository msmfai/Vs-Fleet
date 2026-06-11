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
        "MenuItemBuilder::with_id",
        "MenuItem::with_id",
        "MenuBuilder::new",
        "SubmenuBuilder::new",
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
fn fleet_does_not_install_or_mutate_native_menus() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let main_path = manifest.join("src/main.rs");
    let main = fs::read_to_string(&main_path).unwrap_or_else(|err| {
        panic!("failed to read {}: {err}", main_path.display());
    });
    assert!(
        !main.contains(".menu("),
        "Fleet must not install a custom native menu; leave normal macOS menu ownership to Tauri/AppKit"
    );
    assert!(
        !main.contains(".on_menu_event("),
        "Fleet must not handle native menu events for editor or rail commands"
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
        !contents.contains("pub fn build_menu"),
        "Fleet must not define a custom native menu builder"
    );
    assert!(
        !contents.contains("MenuItemBuilder::with_id("),
        "Fleet must not install command menu items that could grow accelerators later"
    );
    assert!(
        !contents.contains("MenuItem::with_id("),
        "Fleet must not install custom native menu items"
    );
    assert!(
        !contents.contains("MenuBuilder::new"),
        "Fleet must not install custom native menus"
    );
    assert!(
        !contents.contains("SubmenuBuilder::new"),
        "Fleet must not install custom native submenus"
    );
    for pattern in [
        ".cut()",
        ".copy()",
        ".paste()",
        ".undo()",
        ".redo()",
        ".select_all()",
        "\"cmd:",
        "\"spawn:",
        "\"server:",
        "\"rail:",
        "\"external:",
    ] {
        assert!(
            !contents.contains(pattern),
            "Fleet native menu must not contain editor/server command pattern {pattern:?}"
        );
    }
}
