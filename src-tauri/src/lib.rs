mod downloads;
mod kokoros;
mod paths;
mod strip;

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, State};

use downloads::{DownloadState, ModelStatus};
use kokoros::{KokorosChild, KokorosPort, StartResult};

pub struct Playback {
    pub pid: Mutex<Option<u32>>,
    pub stopped: Arc<AtomicBool>,
}

impl Playback {
    fn new() -> Self {
        Self {
            pid: Mutex::new(None),
            stopped: Arc::new(AtomicBool::new(false)),
        }
    }
}

#[derive(Serialize)]
pub struct SpeakResult {
    duration_ms: u64,
    char_count: usize,
    stopped: bool,
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
    app: AppHandle,
    text: String,
    voice: Option<String>,
    state: State<'_, Playback>,
    port_state: State<'_, KokorosPort>,
) -> Result<SpeakResult, String> {
    let input = strip::strip_markdown(&text);
    let char_count = input.len();
    let chosen_voice = voice.unwrap_or_else(|| "af_heart".into());
    let base = kokoros_base_url(&port_state).ok_or("Kokoros not started")?;

    state.stopped.store(false, Ordering::SeqCst);

    let chunks = chunk_text(&input);
    let total = chunks.len();
    let _ = app.emit("speak-progress", serde_json::json!({ "done": 0, "total": total }));

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|e| format!("http client build: {e}"))?;

    // Producer: synthesizes chunks, pushes audio bytes downstream.
    // Channel cap 1 keeps producer at most one chunk ahead of the player.
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Result<(usize, Vec<u8>), String>>(1);
    let stopped_flag = state.stopped.clone();
    let producer_client = client.clone();
    let producer_base = base.clone();
    let producer_voice = chosen_voice.clone();
    let producer_chunks = chunks.clone();

    let producer = tokio::spawn(async move {
        for (i, chunk) in producer_chunks.iter().enumerate() {
            if stopped_flag.load(Ordering::SeqCst) {
                return;
            }
            let resp = match producer_client
                .post(format!("{}/v1/audio/speech", producer_base))
                .json(&serde_json::json!({
                    "model": "tts-1",
                    "voice": producer_voice,
                    "input": chunk,
                }))
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    let _ = tx.send(Err(format!("Kokoros unreachable: {e}"))).await;
                    return;
                }
            };
            if !resp.status().is_success() {
                let _ = tx
                    .send(Err(format!("Kokoros returned HTTP {}", resp.status())))
                    .await;
                return;
            }
            let bytes = match resp.bytes().await {
                Ok(b) => b,
                Err(e) => {
                    let _ = tx
                        .send(Err(format!("Failed to read audio: {e}")))
                        .await;
                    return;
                }
            };
            if stopped_flag.load(Ordering::SeqCst) {
                return;
            }
            if tx.send(Ok((i, bytes.to_vec()))).await.is_err() {
                return; // consumer dropped
            }
        }
    });

    let start = Instant::now();
    let mut was_stopped = false;
    let mut play_err: Option<String> = None;

    while let Some(item) = rx.recv().await {
        if state.stopped.load(Ordering::SeqCst) {
            was_stopped = true;
            break;
        }
        let (i, bytes) = match item {
            Ok(x) => x,
            Err(e) => {
                play_err = Some(e);
                break;
            }
        };

        let temp_path = format!("/tmp/yap_box_{}_{}.wav", std::process::id(), i);
        if let Err(e) = tokio::fs::write(&temp_path, &bytes).await {
            play_err = Some(format!("Failed to write temp file: {e}"));
            break;
        }

        let mut child = match tokio::process::Command::new("afplay").arg(&temp_path).spawn() {
            Ok(c) => c,
            Err(e) => {
                let _ = tokio::fs::remove_file(&temp_path).await;
                play_err = Some(format!("Failed to spawn afplay: {e}"));
                break;
            }
        };

        if let Some(pid) = child.id() {
            if let Ok(mut guard) = state.pid.lock() {
                *guard = Some(pid);
            }
        }

        let wait_result = child.wait().await;

        if let Ok(mut guard) = state.pid.lock() {
            *guard = None;
        }

        let _ = tokio::fs::remove_file(&temp_path).await;

        let status = match wait_result {
            Ok(s) => s,
            Err(e) => {
                play_err = Some(format!("Failed to wait for afplay: {e}"));
                break;
            }
        };

        if state.stopped.load(Ordering::SeqCst) {
            was_stopped = true;
            break;
        }

        if !status.success() {
            play_err = Some(format!("afplay exited with {:?}", status.code()));
            break;
        }

        let _ = app.emit(
            "speak-progress",
            serde_json::json!({ "done": i + 1, "total": total }),
        );
    }

    drop(rx);
    let _ = producer.await;
    state.stopped.store(false, Ordering::SeqCst);

    if let Some(e) = play_err {
        if was_stopped {
            // Stop won the race; treat as clean stop, not error.
        } else {
            return Err(e);
        }
    }

    let duration_ms = start.elapsed().as_millis() as u64;

    Ok(SpeakResult {
        duration_ms,
        char_count,
        stopped: was_stopped,
    })
}

const MIN_CHUNK: usize = 80;
const MAX_CHUNK: usize = 500;

fn chunk_text(input: &str) -> Vec<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }

    let mut sentences: Vec<String> = Vec::new();
    let mut current = String::new();
    let chars: Vec<char> = trimmed.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        current.push(c);
        let is_terminator = matches!(c, '.' | '!' | '?' | '\n');
        let next_is_space_or_end = i + 1 >= chars.len() || chars[i + 1].is_whitespace();
        if is_terminator && next_is_space_or_end {
            let s = current.trim().to_string();
            if !s.is_empty() {
                sentences.push(s);
            }
            current.clear();
        }
        i += 1;
    }
    let tail = current.trim().to_string();
    if !tail.is_empty() {
        sentences.push(tail);
    }

    let mut chunks: Vec<String> = Vec::new();
    let mut buf = String::new();
    for s in sentences {
        if buf.is_empty() {
            buf = s;
        } else if buf.len() + 1 + s.len() <= MAX_CHUNK && buf.len() < MIN_CHUNK {
            buf.push(' ');
            buf.push_str(&s);
        } else if buf.len() < MIN_CHUNK && buf.len() + 1 + s.len() <= MAX_CHUNK * 2 {
            buf.push(' ');
            buf.push_str(&s);
        } else {
            chunks.push(std::mem::take(&mut buf));
            buf = s;
        }
    }
    if !buf.is_empty() {
        chunks.push(buf);
    }

    if chunks.is_empty() {
        chunks.push(trimmed.to_string());
    }
    chunks
}

#[tauri::command]
async fn stop(state: State<'_, Playback>) -> Result<(), String> {
    // Set the flag unconditionally so a Stop pressed mid-synthesis
    // (no afplay alive yet) still aborts the producer + consumer.
    state.stopped.store(true, Ordering::SeqCst);
    let pid_opt = state.pid.lock().ok().and_then(|g| *g);
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
        .manage(Playback::new())
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
