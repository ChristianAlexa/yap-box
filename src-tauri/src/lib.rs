mod downloads;
mod kokoros;
mod paths;
mod strip;

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use rodio::Source;
use tauri::{AppHandle, Emitter, State};

use downloads::{DownloadState, ModelStatus};
use kokoros::{KokorosChild, KokorosPort, StartResult};

pub struct Playback {
    pub sink: Mutex<Option<Arc<rodio::Player>>>,
    pub stopped: Arc<AtomicBool>,
}

impl Playback {
    fn new() -> Self {
        Self {
            sink: Mutex::new(None),
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

    if total == 0 {
        return Ok(SpeakResult {
            duration_ms: 0,
            char_count,
            stopped: false,
        });
    }

    // Audio thread owns the device handle (which contains a `!Send` cpal::Stream) for the
    // duration of this call. It hands the Player back to the async side and blocks until
    // signaled to shut down.
    let (sink_tx, sink_rx) =
        tokio::sync::oneshot::channel::<Result<Arc<rodio::Player>, String>>();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

    let audio_handle = tauri::async_runtime::spawn_blocking(move || {
        let mut handle = match rodio::DeviceSinkBuilder::open_default_sink() {
            Ok(h) => h,
            Err(e) => {
                let _ = sink_tx.send(Err(format!("audio stream init: {e}")));
                return;
            }
        };
        handle.log_on_drop(false);
        let sink = Arc::new(rodio::Player::connect_new(handle.mixer()));
        if sink_tx.send(Ok(sink.clone())).is_err() {
            return;
        }
        // Block until the orchestrator signals shutdown. Handle drops on return.
        let _ = shutdown_rx.blocking_recv();
        drop(sink);
        drop(handle);
    });

    let sink = match sink_rx.await {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            let _ = audio_handle.await;
            return Err(e);
        }
        Err(e) => {
            let _ = audio_handle.await;
            return Err(format!("audio thread init: {e}"));
        }
    };

    if let Ok(mut guard) = state.sink.lock() {
        *guard = Some(sink.clone());
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|e| format!("http client build: {e}"))?;

    // Producer: synthesizes chunks, pushes audio bytes downstream.
    // Channel cap 1 keeps producer at most one chunk ahead of the consumer's decode+append.
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Result<Vec<u8>, String>>(1);
    let stopped_flag = state.stopped.clone();
    let producer_client = client.clone();
    let producer_base = base.clone();
    let producer_voice = chosen_voice.clone();
    let producer_chunks = chunks.clone();

    let producer = tokio::spawn(async move {
        for chunk in producer_chunks.iter() {
            if stopped_flag.load(Ordering::SeqCst) {
                return;
            }
            let resp = match producer_client
                .post(format!("{}/v1/audio/speech", producer_base))
                .json(&serde_json::json!({
                    "model": "tts-1",
                    "voice": producer_voice,
                    "input": chunk,
                    "response_format": "wav",
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
            if tx.send(Ok(bytes.to_vec())).await.is_err() {
                return; // consumer dropped
            }
        }
    });

    let start = Instant::now();
    let mut was_stopped = false;
    let mut play_err: Option<String> = None;

    let appended = Arc::new(AtomicUsize::new(0));
    let appending_done = Arc::new(AtomicBool::new(false));
    let chunk_durations: Arc<Mutex<Vec<Option<Duration>>>> =
        Arc::new(Mutex::new(Vec::with_capacity(total)));

    // Progress poller: emits speak-progress with float `done` interpolated within the
    // currently-playing chunk so the bar advances smoothly between chunk boundaries.
    let poller = {
        let sink = sink.clone();
        let appended = appended.clone();
        let appending_done = appending_done.clone();
        let stopped = state.stopped.clone();
        let app = app.clone();
        let chunk_durations = chunk_durations.clone();
        tokio::spawn(async move {
            let mut last_emitted: f64 = -1.0;
            loop {
                tokio::time::sleep(Duration::from_millis(50)).await;
                let queued = sink.len();
                let appended_now = appended.load(Ordering::SeqCst);
                let finished = appended_now.saturating_sub(queued);
                let current = if queued > 0 { Some(finished) } else { None };

                let done_f = match current {
                    Some(idx) => {
                        let dur = chunk_durations
                            .lock()
                            .ok()
                            .and_then(|v| v.get(idx).copied().flatten());
                        let pos = sink.get_pos().as_secs_f64();
                        match dur {
                            Some(d) if d.as_secs_f64() > 0.0 => {
                                let ratio = (pos / d.as_secs_f64()).clamp(0.0, 0.999);
                                idx as f64 + ratio
                            }
                            _ => idx as f64,
                        }
                    }
                    None => finished as f64,
                };

                if (done_f - last_emitted).abs() > 0.005 {
                    let _ = app.emit(
                        "speak-progress",
                        serde_json::json!({ "done": done_f, "total": total }),
                    );
                    last_emitted = done_f;
                }
                if stopped.load(Ordering::SeqCst) {
                    break;
                }
                if appending_done.load(Ordering::SeqCst) && queued == 0 {
                    break;
                }
            }
        })
    };

    // Consumer: decode each received WAV and append to sink. Rodio chains sources gaplessly.
    while let Some(item) = rx.recv().await {
        if state.stopped.load(Ordering::SeqCst) {
            was_stopped = true;
            break;
        }
        let bytes = match item {
            Ok(b) => b,
            Err(e) => {
                play_err = Some(e);
                break;
            }
        };
        let header_dur = wav_duration(&bytes);
        let decoder = match rodio::Decoder::try_from(std::io::Cursor::new(bytes)) {
            Ok(d) => d,
            Err(e) => {
                play_err = Some(format!("wav decode: {e}"));
                break;
            }
        };
        let dur = header_dur.or_else(|| decoder.total_duration());
        if let Ok(mut v) = chunk_durations.lock() {
            v.push(dur);
        }
        sink.append(decoder);
        appended.fetch_add(1, Ordering::SeqCst);
    }

    appending_done.store(true, Ordering::SeqCst);
    drop(rx);
    let _ = producer.await;

    // Drain: wait for queued sources to finish playing (or stop flag to fire).
    while sink.len() > 0 && !state.stopped.load(Ordering::SeqCst) {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let _ = poller.await;

    if state.stopped.load(Ordering::SeqCst) {
        was_stopped = true;
    }

    if !was_stopped && play_err.is_none() {
        let _ = app.emit(
            "speak-progress",
            serde_json::json!({ "done": total, "total": total }),
        );
    }

    if let Ok(mut guard) = state.sink.lock() {
        *guard = None;
    }

    sink.stop();
    sink.clear();
    drop(sink);

    let _ = shutdown_tx.send(());
    let _ = audio_handle.await;

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

fn wav_duration(bytes: &[u8]) -> Option<Duration> {
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }
    let mut pos = 12;
    let mut sample_rate: u32 = 0;
    let mut channels: u16 = 0;
    let mut bits: u16 = 0;
    let mut data_size: Option<u32> = None;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes([
            bytes[pos + 4],
            bytes[pos + 5],
            bytes[pos + 6],
            bytes[pos + 7],
        ]) as usize;
        let body = pos + 8;
        if id == b"fmt " && body + 16 <= bytes.len() {
            channels = u16::from_le_bytes([bytes[body + 2], bytes[body + 3]]);
            sample_rate = u32::from_le_bytes([
                bytes[body + 4],
                bytes[body + 5],
                bytes[body + 6],
                bytes[body + 7],
            ]);
            bits = u16::from_le_bytes([bytes[body + 14], bytes[body + 15]]);
        } else if id == b"data" {
            data_size = Some(size as u32);
            break;
        }
        pos = body + size;
    }
    let ds = data_size? as u64;
    if sample_rate == 0 || channels == 0 || bits == 0 {
        return None;
    }
    let bytes_per_sec = sample_rate as u64 * channels as u64 * (bits as u64 / 8);
    if bytes_per_sec == 0 {
        return None;
    }
    Some(Duration::from_secs_f64(ds as f64 / bytes_per_sec as f64))
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
    // (no sink yet) still aborts the producer + consumer.
    state.stopped.store(true, Ordering::SeqCst);
    let sink_opt = state.sink.lock().ok().and_then(|g| g.clone());
    if let Some(sink) = sink_opt {
        sink.stop();
        sink.clear();
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
