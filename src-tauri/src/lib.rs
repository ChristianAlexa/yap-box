mod downloads;
mod kokoros;
mod paths;
mod strip;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use rodio::Source;
use rodio::buffer::SamplesBuffer;
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

const PREVIEW_TEXT: &str = "The quick brown fox jumps over the lazy dog.";

struct PreviewEntry {
    channels: rodio::ChannelCount,
    sample_rate: rodio::SampleRate,
    samples: Vec<f32>,
}

pub struct PreviewCache(Mutex<HashMap<String, PreviewEntry>>);

impl PreviewCache {
    fn new() -> Self {
        Self(Mutex::new(HashMap::new()))
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
    // Channel cap 2 + sink-length gate below give a shallow lookahead buffer (~3-4 chunks)
    // so synthesis latency between chunks doesn't cause gaps in playback, while still
    // streaming on the fly rather than processing the whole text upfront.
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Result<Vec<u8>, String>>(2);
    const MAX_SINK_LOOKAHEAD: usize = 2;
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
        let decoder = match rodio::Decoder::try_from(std::io::Cursor::new(bytes)) {
            Ok(d) => d,
            Err(e) => {
                play_err = Some(format!("wav decode: {e}"));
                break;
            }
        };
        let channels = decoder.channels();
        let sample_rate = decoder.sample_rate();
        let samples: Vec<f32> = decoder.collect();
        let trimmed = trim_silence(samples, channels, sample_rate);
        let frames = trimmed.len() / (channels.get() as usize).max(1);
        let dur = Some(Duration::from_secs_f64(
            frames as f64 / sample_rate.get() as f64,
        ));
        // Backpressure: don't append if the sink already has enough lookahead queued.
        // This caps how far ahead we run during normal playback and prevents racing
        // ahead while paused.
        while sink.len() >= MAX_SINK_LOOKAHEAD && !state.stopped.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        if state.stopped.load(Ordering::SeqCst) {
            was_stopped = true;
            break;
        }
        if let Ok(mut v) = chunk_durations.lock() {
            v.push(dur);
        }
        sink.append(SamplesBuffer::new(channels, sample_rate, trimmed));
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

fn trim_silence(
    samples: Vec<f32>,
    channels: rodio::ChannelCount,
    sample_rate: rodio::SampleRate,
) -> Vec<f32> {
    const SILENCE_THRESHOLD: f32 = 0.005;
    const KEEP_PAD_MS: usize = 15;
    if samples.is_empty() {
        return samples;
    }
    let pad_frames = (sample_rate.get() as usize) * KEEP_PAD_MS / 1000;
    let pad = pad_frames * channels.get() as usize;
    let head = samples
        .iter()
        .position(|s| s.abs() > SILENCE_THRESHOLD)
        .map(|i| i.saturating_sub(pad))
        .unwrap_or(0);
    let tail = samples
        .iter()
        .rposition(|s| s.abs() > SILENCE_THRESHOLD)
        .map(|i| (i + 1 + pad).min(samples.len()))
        .unwrap_or(samples.len());
    if head < tail {
        samples[head..tail].to_vec()
    } else {
        samples
    }
}

const MIN_CHUNK: usize = 160;
const MAX_CHUNK: usize = 700;

const ABBREVS: &[&str] = &[
    "mr", "mrs", "ms", "dr", "st", "jr", "sr", "vs", "etc", "eg", "ie", "no", "fig",
];

fn ends_with_abbrev(buf: &str) -> bool {
    let last = buf
        .rsplit(|c: char| c.is_whitespace())
        .next()
        .unwrap_or("")
        .trim_end_matches('.');
    let lower: String = last
        .chars()
        .filter(|c| c.is_ascii_alphabetic())
        .flat_map(|c| c.to_lowercase())
        .collect();
    if lower.is_empty() {
        return false;
    }
    ABBREVS.iter().any(|a| *a == lower)
}

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
        let blocked_by_abbrev = c == '.' && ends_with_abbrev(&current);
        if is_terminator && next_is_space_or_end && !blocked_by_abbrev {
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
async fn pause(state: State<'_, Playback>) -> Result<(), String> {
    let sink_opt = state.sink.lock().ok().and_then(|g| g.clone());
    if let Some(sink) = sink_opt {
        sink.pause();
    }
    Ok(())
}

#[tauri::command]
async fn resume(state: State<'_, Playback>) -> Result<(), String> {
    let sink_opt = state.sink.lock().ok().and_then(|g| g.clone());
    if let Some(sink) = sink_opt {
        sink.play();
    }
    Ok(())
}

#[tauri::command]
async fn preview_voice(
    voice: String,
    cache: State<'_, PreviewCache>,
    port_state: State<'_, KokorosPort>,
) -> Result<(), String> {
    let cached = cache
        .0
        .lock()
        .ok()
        .and_then(|m| {
            m.get(&voice)
                .map(|e| (e.channels, e.sample_rate, e.samples.clone()))
        });

    let (channels, sample_rate, samples) = match cached {
        Some(t) => t,
        None => {
            let base = kokoros_base_url(&port_state).ok_or("Kokoros not started")?;
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .map_err(|e| format!("http client build: {e}"))?;
            let resp = client
                .post(format!("{}/v1/audio/speech", base))
                .json(&serde_json::json!({
                    "model": "tts-1",
                    "voice": voice,
                    "input": PREVIEW_TEXT,
                    "response_format": "wav",
                }))
                .send()
                .await
                .map_err(|e| format!("Kokoros unreachable: {e}"))?;
            if !resp.status().is_success() {
                return Err(format!("Kokoros returned HTTP {}", resp.status()));
            }
            let bytes = resp
                .bytes()
                .await
                .map_err(|e| format!("Failed to read audio: {e}"))?;
            let decoder = rodio::Decoder::try_from(std::io::Cursor::new(bytes.to_vec()))
                .map_err(|e| format!("wav decode: {e}"))?;
            let channels = decoder.channels();
            let sample_rate = decoder.sample_rate();
            let raw: Vec<f32> = decoder.collect();
            let trimmed = trim_silence(raw, channels, sample_rate);
            if let Ok(mut m) = cache.0.lock() {
                m.insert(
                    voice.clone(),
                    PreviewEntry {
                        channels,
                        sample_rate,
                        samples: trimmed.clone(),
                    },
                );
            }
            (channels, sample_rate, trimmed)
        }
    };

    let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();
    tauri::async_runtime::spawn_blocking(move || {
        let mut handle = match rodio::DeviceSinkBuilder::open_default_sink() {
            Ok(h) => h,
            Err(_) => {
                let _ = done_tx.send(());
                return;
            }
        };
        handle.log_on_drop(false);
        let player = rodio::Player::connect_new(handle.mixer());
        player.append(SamplesBuffer::new(channels, sample_rate, samples));
        while player.len() > 0 {
            std::thread::sleep(Duration::from_millis(50));
        }
        drop(player);
        drop(handle);
        let _ = done_tx.send(());
    });
    let _ = done_rx.await;
    Ok(())
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
            Ok(v) if !v.voices.is_empty() => {
                let filtered: Vec<String> =
                    v.voices.into_iter().filter(|v| is_us_voice(v)).collect();
                if filtered.is_empty() {
                    Ok(fallback_voices())
                } else {
                    Ok(filtered)
                }
            }
            _ => Ok(fallback_voices()),
        },
        _ => Ok(fallback_voices()),
    }
}

// Kokoros voice IDs use a two-char prefix: lang+gender, then '_'.
// 'a' = American English, 'b' = British, 'z' = Mandarin, etc. Keep American
// language-prefixed voices, plus any voice without a language prefix
// (OpenAI-style names like "ballad", "alloy").
fn is_us_voice(name: &str) -> bool {
    let bytes = name.as_bytes();
    if bytes.len() >= 3 && bytes[2] == b'_' {
        return bytes[0] == b'a';
    }
    true
}

fn fallback_voices() -> Vec<String> {
    vec!["af_heart", "af_bella", "af_nicole", "af_sky", "am_adam", "am_michael"]
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
        .manage(PreviewCache::new())
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
            pause,
            resume,
            preview_voice,
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
