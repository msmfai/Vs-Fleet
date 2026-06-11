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
fn fleet_installs_one_static_native_shell_menu() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let main_path = manifest.join("src/main.rs");
    let main = fs::read_to_string(&main_path).unwrap_or_else(|err| {
        panic!("failed to read {}: {err}", main_path.display());
    });
    assert!(
        main.contains(".menu(mux::build_menu)"),
        "Fleet must install one static AppKit-aware shell menu instead of Tauri's default Edit menu"
    );
    assert!(
        !main.contains(".on_menu_event("),
        "Fleet must use native predefined shell menu items without an app command handler"
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
        contents.contains("WINDOW_SUBMENU_ID") && contents.contains("HELP_SUBMENU_ID"),
        "Fleet's custom menu must use Tauri's AppKit submenu ids so Window and Help menus stay native"
    );
    assert!(
        !contents.contains("MenuItemBuilder::with_id("),
        "Fleet must not install command menu items that could grow accelerators later"
    );
    assert!(
        !contents.contains("MenuItem::with_id("),
        "Fleet must use native predefined shell menu items, not app-defined menu commands"
    );
    assert!(
        !contents.contains("SubmenuBuilder::new("),
        "Fleet must not build generic AppKit submenus that bypass Tauri's default menu integration"
    );
    for pattern in [
        ".cut()",
        ".copy()",
        ".paste()",
        ".undo()",
        ".redo()",
        ".select_all()",
        "\"Edit\"",
        "PredefinedMenuItem::cut",
        "PredefinedMenuItem::copy",
        "PredefinedMenuItem::paste",
        "PredefinedMenuItem::undo",
        "PredefinedMenuItem::redo",
        "PredefinedMenuItem::select_all",
        "\"cmd:",
        "\"spawn:",
        "\"server:",
        "\"rail:",
        "\"external:",
    ] {
        for (rel, source) in [
            ("src/main.rs", main.as_str()),
            ("src/mux.rs", contents.as_str()),
        ] {
            assert!(
                !source.contains(pattern),
                "{} must not contain editor/server command pattern {pattern:?}",
                rel
            );
        }
    }
}
