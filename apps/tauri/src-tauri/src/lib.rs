use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;

use tauri::Manager;

/// Holds the sidecar child process so we can kill it when the window closes.
struct SidecarState(Mutex<Option<Child>>);

/// The repo root, computed at compile time from CARGO_MANIFEST_DIR.
/// CARGO_MANIFEST_DIR is `apps/tauri/src-tauri`, so the root is three levels up.
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

/// Extract the `http://127.0.0.1:<port>` URL from a `dsh web:` stdout line.
fn parse_dsh_url(line: &str) -> Option<String> {
    let rest = line.strip_prefix("dsh web:")?;
    let url = rest.trim_start().split_whitespace().next()?;
    url.starts_with("http://").then(|| url.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(SidecarState(Mutex::new(None)))
        .setup(|app| {
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }

            let window = app
                .get_webview_window("main")
                .expect("main window must exist");

            let repo = repo_root();
            log::info!("spawning dsh web sidecar in {}", repo.display());

            // On Windows, `pnpm` is a `.cmd` shim that Rust's `Command` does
            // not resolve automatically — shell out through `cmd /C` so the
            // PATHEXT resolution finds `pnpm.cmd`. On Unix, `pnpm` is a real
            // shebang executable and spawns directly.
            let mut cmd = if cfg!(windows) {
                let mut c = Command::new("cmd");
                c.args(["/C", "pnpm", "run", "dsh", "web", "--port", "0"]);
                c
            } else {
                let mut c = Command::new("pnpm");
                c.args(["run", "dsh", "web", "--port", "0"]);
                c
            };
            cmd.current_dir(&repo)
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit());

            let mut child = cmd.spawn()?;

            let stdout = child.stdout.take().expect("piped stdout");
            let state = app.state::<SidecarState>();
            *state.0.lock().unwrap() = Some(child);

            std::thread::spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    let line = match line {
                        Ok(l) => l,
                        Err(e) => {
                            log::error!("dsh web stdout read failed: {e}");
                            return;
                        }
                    };
                    log::info!("dsh web: {line}");
                    if let Some(url) = parse_dsh_url(&line) {
                        log::info!("navigating to {url}");
                        let js = format!("window.location.href = '{url}'");
                        if let Err(e) = window.eval(&js) {
                            log::error!("navigation failed: {e}");
                        }
                        return;
                    }
                }
            });

            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::Destroyed = event {
                let state = window.app_handle().state::<SidecarState>();
                let mut guard = state.0.lock().unwrap();
                if let Some(mut child) = guard.take() {
                    drop(guard);
                    log::info!("killing dsh web sidecar");
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
