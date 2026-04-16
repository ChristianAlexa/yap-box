use std::net::TcpListener;
use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_shell::process::{CommandChild, CommandEvent};
use tauri_plugin_shell::ShellExt;

use crate::paths;

pub struct KokorosChild(pub Mutex<Option<CommandChild>>);

pub struct KokorosPort(pub Mutex<Option<u16>>);

#[derive(Serialize, Clone)]
pub struct StartResult {
    pub port: u16,
}

// Port allocation is technically racy: bind to :0, read assigned port, drop,
// hand it to koko. For a single-user desktop app this is fine — the window
// between drop and koko's bind is microseconds and nothing else on the machine
// is fighting for ephemeral localhost ports.
fn allocate_port() -> Result<u16, String> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| format!("bind :0: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("local_addr: {e}"))?
        .port();
    drop(listener);
    Ok(port)
}

pub async fn start_kokoros(
    app: AppHandle,
    child_state: State<'_, KokorosChild>,
    port_state: State<'_, KokorosPort>,
) -> Result<StartResult, String> {
    if let Ok(guard) = port_state.0.lock() {
        if let Some(existing) = *guard {
            return Ok(StartResult { port: existing });
        }
    }

    let onnx = paths::onnx_path(&app);
    let voices = paths::voices_path(&app);
    if !onnx.exists() || !voices.exists() {
        return Err("Model files missing".into());
    }

    let port = allocate_port()?;

    let mut sidecar = app
        .shell()
        .sidecar("koko")
        .map_err(|e| format!("sidecar lookup: {e}"))?;

    // espeak-rs-sys bakes its build-time OUT_DIR into the binary, so a
    // CI-built koko looks for phoneme data at a path that only exists on
    // the runner. When we've bundled espeak-ng-data as a Tauri resource,
    // point koko at it via ESPEAK_DATA_PATH. In dev, this directory isn't
    // staged — skip the override and let the locally-built koko use its
    // compiled-in path, which already resolves on the dev machine.
    if let Ok(resource_dir) = app.path().resource_dir() {
        let espeak_data = resource_dir.join("resources").join("espeak-ng-data");
        if espeak_data.join("phontab").exists() {
            sidecar = sidecar.env("ESPEAK_DATA_PATH", espeak_data);
        }
    }

    let sidecar = sidecar.args([
        "-m",
        &onnx.to_string_lossy(),
        "-d",
        &voices.to_string_lossy(),
        "openai",
        "--ip",
        "127.0.0.1",
        "--port",
        &port.to_string(),
    ]);

    let (mut rx, child) = sidecar.spawn().map_err(|e| format!("spawn koko: {e}"))?;

    if let Ok(mut guard) = child_state.0.lock() {
        *guard = Some(child);
    }
    if let Ok(mut guard) = port_state.0.lock() {
        *guard = Some(port);
    }

    let app_log = app.clone();
    tauri::async_runtime::spawn(async move {
        while let Some(event) = rx.recv().await {
            match event {
                CommandEvent::Stdout(line) => {
                    eprintln!("[koko] {}", String::from_utf8_lossy(&line));
                }
                CommandEvent::Stderr(line) => {
                    eprintln!("[koko] {}", String::from_utf8_lossy(&line));
                }
                CommandEvent::Terminated(payload) => {
                    eprintln!("[koko] terminated code={:?}", payload.code);
                    if let Some(state) = app_log.try_state::<KokorosPort>() {
                        if let Ok(mut guard) = state.0.lock() {
                            *guard = None;
                        }
                    }
                    if let Some(state) = app_log.try_state::<KokorosChild>() {
                        if let Ok(mut guard) = state.0.lock() {
                            *guard = None;
                        }
                    }
                    let _ = app_log.emit("kokoros-terminated", payload.code);
                    break;
                }
                _ => {}
            }
        }
    });

    // Await readiness inline: poll /v1/audio/voices until it responds 2xx or
    // we hit the 30s deadline. Keeps the startup flow single-shot — callers
    // get Ok only after the sidecar is actually serving.
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(500))
        .build()
        .map_err(|e| format!("probe client: {e}"))?;
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let url = format!("http://127.0.0.1:{port}/v1/audio/voices");
    loop {
        if std::time::Instant::now() > deadline {
            // Kill the child we spawned; otherwise a retry spawns a second
            // koko alongside this one and the orphan never gets reaped until
            // app exit.
            let child_opt = child_state.0.lock().ok().and_then(|mut g| g.take());
            if let Some(child) = child_opt {
                let _ = child.kill();
            }
            if let Ok(mut guard) = port_state.0.lock() {
                *guard = None;
            }
            return Err("Kokoros failed to become ready within 30s".into());
        }
        if let Ok(resp) = client.get(&url).send().await {
            if resp.status().is_success() {
                return Ok(StartResult { port });
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

pub fn shutdown(app: &AppHandle) {
    if let Some(state) = app.try_state::<KokorosChild>() {
        let child_opt = state.0.lock().ok().and_then(|mut g| g.take());
        if let Some(child) = child_opt {
            let _ = child.kill();
            eprintln!("[yap-box] stopped Kokoros sidecar");
        }
    }
}
