use std::{env, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=icons/icon.png");
    println!("cargo:rerun-if-changed=scripts/refresh-icons.sh");

    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR set by cargo");
    match Command::new("bash")
        .arg("scripts/refresh-icons.sh")
        .current_dir(&manifest_dir)
        .status()
    {
        Ok(status) if status.success() => {}
        Ok(status) => {
            println!(
                "cargo:warning=icon refresh exited with {status}; keeping existing generated icons"
            );
        }
        Err(err) => {
            println!(
                "cargo:warning=icon refresh could not run: {err}; keeping existing generated icons"
            );
        }
    }

    tauri_build::build();
}
