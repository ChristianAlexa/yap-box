use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use tokio::io::AsyncWriteExt;

use crate::paths;

// Pinned upstream release — both assets must come from the same tag so their
// sizes stay stable. Bump deliberately; never use `latest`.
const MODEL_RELEASE_TAG: &str = "model-files-v1.0";
// The `model-files-v1.0` path segment in ONNX_URL and VOICES_URL must match
// MODEL_RELEASE_TAG above. If you bump the tag, update both URLs.
const ONNX_URL: &str = "https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.0/kokoro-v1.0.onnx";
const VOICES_URL: &str = "https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.0/voices-v1.0.bin";
const ONNX_EXPECTED_BYTES: u64 = 325_532_387;
const VOICES_EXPECTED_BYTES: u64 = 28_214_398;

pub struct DownloadState {
    pub cancel: Arc<AtomicBool>,
    pub in_progress: Arc<AtomicBool>,
}

impl DownloadState {
    pub fn new() -> Self {
        Self {
            cancel: Arc::new(AtomicBool::new(false)),
            in_progress: Arc::new(AtomicBool::new(false)),
        }
    }
}

#[derive(Serialize, Clone)]
pub struct ModelStatus {
    pub present: bool,
    pub onnx_bytes: Option<u64>,
    pub voices_bytes: Option<u64>,
}

#[derive(Serialize, Clone)]
struct ProgressPayload {
    file: &'static str,
    downloaded: u64,
    total: u64,
    overall_pct: f32,
}

#[derive(Serialize, Clone)]
struct CompletePayload {
    onnx_bytes: u64,
    voices_bytes: u64,
}

#[derive(Serialize, Clone)]
struct ErrorPayload {
    file: &'static str,
    message: String,
    cancelled: bool,
}

// Internal error type returned by download_one/download_both. The emit
// serializes into ErrorPayload, which is what the frontend listens on.
enum DownloadError {
    Cancelled,
    Failed { file: &'static str, message: String },
}

pub fn model_status(app: &AppHandle) -> ModelStatus {
    let onnx = std::fs::metadata(paths::onnx_path(app)).ok().map(|m| m.len());
    let voices = std::fs::metadata(paths::voices_path(app)).ok().map(|m| m.len());
    // A truncated or zero-byte file on disk means a prior download crashed
    // mid-write; treat it as absent so the UI re-gates to the download flow
    // rather than handing Kokoros a broken model.
    let present = onnx == Some(ONNX_EXPECTED_BYTES) && voices == Some(VOICES_EXPECTED_BYTES);
    ModelStatus {
        present,
        onnx_bytes: onnx,
        voices_bytes: voices,
    }
}

pub async fn cleanup_partials(app: &AppHandle) {
    let onnx = paths::onnx_path(app);
    let voices = paths::voices_path(app);
    for p in [onnx, voices] {
        let part = PathBuf::from(format!("{}.part", p.display()));
        let _ = tokio::fs::remove_file(&part).await;
    }
}

pub async fn download_model(
    app: AppHandle,
    state: State<'_, DownloadState>,
) -> Result<(), String> {
    if state
        .in_progress
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err("Download already in progress".into());
    }
    state.cancel.store(false, Ordering::SeqCst);

    let cancel = state.cancel.clone();
    let in_progress = state.in_progress.clone();

    let onnx_path = paths::onnx_path(&app);
    let voices_path = paths::voices_path(&app);

    if let Some(parent) = onnx_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("mkdir failed: {e}"))?;
    }
    if let Some(parent) = voices_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("mkdir failed: {e}"))?;
    }

    let app_bg = app.clone();
    tauri::async_runtime::spawn(async move {
        let result = download_both(&app_bg, &cancel, &onnx_path, &voices_path).await;
        in_progress.store(false, Ordering::SeqCst);
        match result {
            Ok(()) => {
                let _ = app_bg.emit(
                    "model-download-complete",
                    CompletePayload {
                        onnx_bytes: ONNX_EXPECTED_BYTES,
                        voices_bytes: VOICES_EXPECTED_BYTES,
                    },
                );
            }
            Err(err) => {
                let _ = tokio::fs::remove_file(format!("{}.part", onnx_path.display())).await;
                let _ = tokio::fs::remove_file(format!("{}.part", voices_path.display())).await;
                let payload = match err {
                    DownloadError::Cancelled => ErrorPayload {
                        file: "",
                        message: "cancelled".into(),
                        cancelled: true,
                    },
                    DownloadError::Failed { file, message } => ErrorPayload {
                        file,
                        message,
                        cancelled: false,
                    },
                };
                let _ = app_bg.emit("model-download-error", payload);
            }
        }
    });

    Ok(())
}

async fn download_both(
    app: &AppHandle,
    cancel: &Arc<AtomicBool>,
    onnx_path: &Path,
    voices_path: &Path,
) -> Result<(), DownloadError> {
    // Each file covers a fraction of the overall 0–100% progress bar, weighted by
    // its size. onnx_span + voices_span == 1.0, and voices_span starts where
    // onnx_span ends.
    let total_bytes = (ONNX_EXPECTED_BYTES + VOICES_EXPECTED_BYTES) as f32;
    let onnx_span = ONNX_EXPECTED_BYTES as f32 / total_bytes;
    let voices_span = VOICES_EXPECTED_BYTES as f32 / total_bytes;

    download_one(
        app,
        cancel,
        "onnx",
        ONNX_URL,
        onnx_path,
        ONNX_EXPECTED_BYTES,
        0.0,
        onnx_span,
    )
    .await?;

    download_one(
        app,
        cancel,
        "voices",
        VOICES_URL,
        voices_path,
        VOICES_EXPECTED_BYTES,
        onnx_span,
        voices_span,
    )
    .await?;

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn download_one(
    app: &AppHandle,
    cancel: &Arc<AtomicBool>,
    file: &'static str,
    url: &str,
    dest: &Path,
    expected_bytes: u64,
    overall_base: f32,
    overall_span: f32,
) -> Result<(), DownloadError> {
    let part = PathBuf::from(format!("{}.part", dest.display()));
    let _ = tokio::fs::remove_file(&part).await;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(60 * 30))
        .build()
        .map_err(|e| DownloadError::Failed {
            file,
            message: format!("client build: {e}"),
        })?;

    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| DownloadError::Failed {
            file,
            message: format!("request: {e}"),
        })?;

    if !resp.status().is_success() {
        return Err(DownloadError::Failed {
            file,
            message: format!("HTTP {}", resp.status()),
        });
    }

    let total = resp.content_length().unwrap_or(expected_bytes);

    let mut file_handle = tokio::fs::File::create(&part)
        .await
        .map_err(|e| DownloadError::Failed {
            file,
            message: format!("create .part: {e}"),
        })?;

    let mut stream = resp.bytes_stream();
    let mut downloaded: u64 = 0;
    let mut last_emit = Instant::now() - Duration::from_secs(1);
    let mut last_emit_bytes: u64 = 0;

    while let Some(chunk) = stream.next().await {
        if cancel.load(Ordering::SeqCst) {
            drop(file_handle);
            let _ = tokio::fs::remove_file(&part).await;
            return Err(DownloadError::Cancelled);
        }
        let bytes = chunk.map_err(|e| DownloadError::Failed {
            file,
            message: format!("stream: {e}"),
        })?;
        file_handle
            .write_all(&bytes)
            .await
            .map_err(|e| DownloadError::Failed {
                file,
                message: format!("write: {e}"),
            })?;
        downloaded += bytes.len() as u64;

        let should_emit = last_emit.elapsed() >= Duration::from_millis(250)
            || (downloaded - last_emit_bytes) >= 1_048_576;
        if should_emit {
            let pct_in_file = if total > 0 {
                (downloaded as f32 / total as f32).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let overall_pct = ((overall_base + pct_in_file * overall_span) * 100.0).min(100.0);
            let _ = app.emit(
                "model-download-progress",
                ProgressPayload {
                    file,
                    downloaded,
                    total,
                    overall_pct,
                },
            );
            last_emit = Instant::now();
            last_emit_bytes = downloaded;
        }
    }

    file_handle
        .flush()
        .await
        .map_err(|e| DownloadError::Failed {
            file,
            message: format!("flush: {e}"),
        })?;
    drop(file_handle);

    let final_meta = tokio::fs::metadata(&part)
        .await
        .map_err(|e| DownloadError::Failed {
            file,
            message: format!("stat .part: {e}"),
        })?;
    if final_meta.len() != expected_bytes {
        let _ = tokio::fs::remove_file(&part).await;
        return Err(DownloadError::Failed {
            file,
            message: format!(
                "size mismatch: got {} bytes, expected {} (tag {})",
                final_meta.len(),
                expected_bytes,
                MODEL_RELEASE_TAG
            ),
        });
    }

    tokio::fs::rename(&part, dest)
        .await
        .map_err(|e| DownloadError::Failed {
            file,
            message: format!("rename: {e}"),
        })?;

    let final_pct = ((overall_base + overall_span) * 100.0).min(100.0);
    let _ = app.emit(
        "model-download-progress",
        ProgressPayload {
            file,
            downloaded: expected_bytes,
            total: expected_bytes,
            overall_pct: final_pct,
        },
    );

    Ok(())
}

pub fn cancel_download(state: State<'_, DownloadState>) -> Result<(), String> {
    state.cancel.store(true, Ordering::SeqCst);
    Ok(())
}
