mod downloads;
mod kokoros;
mod paths;
mod strip;

use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use std::time::Instant;
use tauri::{AppHandle, State};

use downloads::{DownloadState, ModelStatus};
use kokoros::{KokorosChild, KokorosPort, StartResult};

pub struct PlaybackPid(pub Mutex<Option<u32>>);

#[derive(Serialize)]
pub struct SpeakResult {
    duration_ms: u64,
    char_count: usize,
}

fn kokoros_base_url(port_state: &State<'_, KokorosPort>) -> Option<String> {
    port_state
        .0
        .lock()
        .ok()
        .and_then(|g| *g)
        .map(|p| format!("http://127.0.0.1:{p}"))
}

#[tauri::command]
async fn speak(
    text: String,
    voice: Option<String>,
    state: State<'_, PlaybackPid>,
    port_state: State<'_, KokorosPort>,
) -> Result<SpeakResult, String> {
    let input = strip::strip_markdown(&text);
    let char_count = input.len();
    let chosen_voice = voice.unwrap_or_else(|| "af_heart".into());
    let base = kokoros_base_url(&port_state).ok_or("Kokoros not started")?;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{base}/v1/audio/speech"))
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

#[tauri::command]
async fn list_voices(port_state: State<'_, KokorosPort>) -> Result<Vec<String>, String> {
    let Some(base) = kokoros_base_url(&port_state) else {
        return Ok(fallback_voices());
    };
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_millis(1500))
        .build()
    {
        Ok(c) => c,
        Err(_) => return Ok(fallback_voices()),
    };
    match client.get(format!("{base}/v1/audio/voices")).send().await {
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

#[tauri::command]
async fn model_status(app: AppHandle) -> ModelStatus {
    downloads::model_status(&app)
}

#[tauri::command]
async fn download_model(
    app: AppHandle,
    state: State<'_, DownloadState>,
) -> Result<(), String> {
    downloads::download_model(app, state).await
}

#[tauri::command]
async fn cancel_download(state: State<'_, DownloadState>) -> Result<(), String> {
    downloads::cancel_download(state)
}

#[tauri::command]
async fn start_kokoros(
    app: AppHandle,
    child_state: State<'_, KokorosChild>,
    port_state: State<'_, KokorosPort>,
) -> Result<StartResult, String> {
    kokoros::start_kokoros(app, child_state, port_state).await
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .manage(PlaybackPid(Mutex::new(None)))
        .manage(KokorosChild(Mutex::new(None)))
        .manage(KokorosPort(Mutex::new(None)))
        .manage(DownloadState::new())
        .setup(|app| {
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                downloads::cleanup_partials(&handle).await;
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            speak,
            stop,
            read_file,
            list_voices,
            model_status,
            download_model,
            cancel_download,
            start_kokoros,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|app_handle, event| {
        if let tauri::RunEvent::Exit = event {
            kokoros::shutdown(app_handle);
        }
    });
}
