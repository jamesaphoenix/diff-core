#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

fn should_run_launch_smoke() -> bool {
    std::env::var("DIFFCORE_RUN_APP_LAUNCH_TESTS")
        .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

fn bundled_binary_path() -> PathBuf {
    let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    root.pop();
    root.pop();
    root.join("target")
        .join("release")
        .join("bundle")
        .join("macos")
        .join("Diffcore.app")
        .join("Contents")
        .join("MacOS")
        .join("diffcore-tauri")
}

#[test]
fn bundled_app_stays_alive_during_startup() {
    if !should_run_launch_smoke() {
        eprintln!("Skipping app launch smoke test (set DIFFCORE_RUN_APP_LAUNCH_TESTS=1 after `cargo tauri build`)");
        return;
    }

    let binary = bundled_binary_path();
    assert!(
        binary.exists(),
        "Built app binary not found at {}. Run `cargo tauri build` first.",
        binary.display()
    );

    let mut child = Command::new(&binary)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    thread::sleep(Duration::from_secs(4));

    if let Some(status) = child.try_wait().unwrap() {
        let output = child.wait_with_output().unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        panic!(
            "Diffcore exited during startup with {}.\nstdout:\n{}\nstderr:\n{}",
            status, stdout, stderr
        );
    }

    child.kill().unwrap();
    let _ = child.wait_with_output().unwrap();
}
