mod strip;

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Instant;
use tauri::{Manager, State};

pub struct PlaybackPid(pub Mutex<Option<u32>>);

pub struct KokorosProcess(pub Mutex<Option<tokio::process::Child>>);

#[derive(Serialize)]
pub struct SpeakResult {
    duration_ms: u64,
    char_count: usize,
}

#[tauri::command]
async fn speak(
    text: String,
    voice: Option<String>,
    state: State<'_, PlaybackPid>,
) -> Result<SpeakResult, String> {
    let input = strip::strip_markdown(&text);
    let char_count = input.len();
    let chosen_voice = voice.unwrap_or_else(|| "af_heart".into());
    let kokoro_url =
        std::env::var("KOKORO_URL").unwrap_or_else(|_| "http://localhost:3000".into());

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{kokoro_url}/v1/audio/speech"))
        .json(&serde_json::json!({
            "model": "tts-1",
            "voice": chosen_voice,
            "input": input,
        }))
        .send()
        .await
        .map_err(|e| format!("Kokoros unreachable: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("Kokoros returned HTTP {}", resp.status()));
    }

    let bytes = resp.bytes().await.map_err(|e| format!("Failed to read audio: {e}"))?;

    let temp_path = format!("/tmp/yap_box_{}.wav", std::process::id());
    tokio::fs::write(&temp_path, &bytes)
        .await
        .map_err(|e| format!("Failed to write temp file: {e}"))?;

    let start = Instant::now();
    let mut child = tokio::process::Command::new("afplay")
        .arg(&temp_path)
        .spawn()
        .map_err(|e| format!("Failed to spawn afplay: {e}"))?;

    let pid = child.id();
    if let Some(pid) = pid {
        if let Ok(mut guard) = state.0.lock() {
            *guard = Some(pid);
        }
    }

    let wait_result = child.wait().await;

    if let Ok(mut guard) = state.0.lock() {
        *guard = None;
    }

    let _ = tokio::fs::remove_file(&temp_path).await;

    let status = wait_result.map_err(|e| format!("Failed to wait for afplay: {e}"))?;
    let duration_ms = start.elapsed().as_millis() as u64;

    if !status.success() {
        return Err(format!("afplay exited with {:?}", status.code()));
    }

    Ok(SpeakResult {
        duration_ms,
        char_count,
    })
}

#[tauri::command]
async fn stop(state: State<'_, PlaybackPid>) -> Result<(), String> {
    let pid_opt = state.0.lock().ok().and_then(|g| *g);
    if let Some(pid) = pid_opt {
        use nix::sys::signal::{kill, Signal};
        use nix::unistd::Pid;
        let _ = kill(Pid::from_raw(pid as i32), Signal::SIGTERM);
    }
    Ok(())
}

#[derive(Deserialize)]
struct VoicesResponse {
    voices: Vec<String>,
}

// Prefers Kokoros' /v1/audio/voices endpoint so the list tracks the installed
// voicepacks. Falls back to a curated set if Kokoros is unreachable.
#[tauri::command]
async fn list_voices() -> Result<Vec<String>, String> {
    let kokoro_url =
        std::env::var("KOKORO_URL").unwrap_or_else(|_| "http://localhost:3000".into());
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_millis(1500))
        .build()
    {
        Ok(c) => c,
        Err(_) => return Ok(fallback_voices()),
    };
    match client
        .get(format!("{kokoro_url}/v1/audio/voices"))
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => match resp.json::<VoicesResponse>().await {
            Ok(v) if !v.voices.is_empty() => Ok(v.voices),
            _ => Ok(fallback_voices()),
        },
        _ => Ok(fallback_voices()),
    }
}

fn fallback_voices() -> Vec<String> {
    vec![
        "af_heart", "af_bella", "af_nicole", "af_sky", "am_adam", "am_michael", "bf_emma",
        "bf_isabella", "bm_george", "bm_lewis",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

#[tauri::command]
async fn read_file(path: String) -> Result<String, String> {
    const MAX_BYTES: u64 = 1_048_576;
    let ext_ok = std::path::Path::new(&path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("txt") || e.eq_ignore_ascii_case("md"))
        .unwrap_or(false);
    if !ext_ok {
        return Err("Only .txt or .md files are allowed".into());
    }
    let meta = tokio::fs::symlink_metadata(&path)
        .await
        .map_err(|e| format!("Failed to read {path}: {e}"))?;
    if !meta.file_type().is_file() {
        return Err("Path is not a regular file".into());
    }
    if meta.len() > MAX_BYTES {
        return Err("File too large (max 1 MiB)".into());
    }
    tokio::fs::read_to_string(&path)
        .await
        .map_err(|e| format!("Failed to read {path}: {e}"))
}

async fn is_kokoros_up() -> bool {
    let kokoro_url =
        std::env::var("KOKORO_URL").unwrap_or_else(|_| "http://localhost:3000".into());
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_millis(1500))
        .build()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    client.get(&kokoro_url).send().await.is_ok()
}

#[tauri::command]
async fn kokoros_reachable() -> bool {
    is_kokoros_up().await
}

fn resolve_koko_binary() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("KOKOROS_BINARY") {
        let path = PathBuf::from(p);
        if path.exists() {
            return Some(path);
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let default = PathBuf::from(home).join("dev/Kokoros/target/release/koko");
        if default.exists() {
            return Some(default);
        }
    }
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let candidate = dir.join("koko");
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .manage(PlaybackPid(Mutex::new(None)))
        .manage(KokorosProcess(Mutex::new(None)))
        .setup(|app| {
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                if is_kokoros_up().await {
                    eprintln!("[yap-box] Kokoros already running — attaching");
                    return;
                }
                let Some(binary) = resolve_koko_binary() else {
                    eprintln!(
                        "[yap-box] no koko binary found (set KOKOROS_BINARY, install to PATH, or put at ~/dev/Kokoros/target/release/koko)"
                    );
                    return;
                };
                match tokio::process::Command::new(&binary)
                    .arg("openai")
                    .stdout(std::process::Stdio::inherit())
                    .stderr(std::process::Stdio::inherit())
                    .spawn()
                {
                    Ok(child) => {
                        eprintln!(
                            "[yap-box] spawned Kokoros from {} (pid {:?})",
                            binary.display(),
                            child.id()
                        );
                        if let Some(state) = handle.try_state::<KokorosProcess>() {
                            if let Ok(mut guard) = state.0.lock() {
                                *guard = Some(child);
                            }
                        }
                    }
                    Err(e) => eprintln!(
                        "[yap-box] failed to spawn {}: {e}",
                        binary.display()
                    ),
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            speak,
            stop,
            read_file,
            kokoros_reachable,
            list_voices
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|app_handle, event| {
        if let tauri::RunEvent::Exit = event {
            if let Some(state) = app_handle.try_state::<KokorosProcess>() {
                let child_opt = state.0.lock().ok().and_then(|mut g| g.take());
                if let Some(child) = child_opt {
                    if let Some(pid) = child.id() {
                        use nix::sys::signal::{kill, Signal};
                        use nix::unistd::Pid;
                        let _ = kill(Pid::from_raw(pid as i32), Signal::SIGTERM);
                        eprintln!("[yap-box] stopped Kokoros (pid {pid})");
                    }
                }
            }
        }
    });
}
