use std::path::PathBuf;
use tauri::{AppHandle, Manager};

pub fn model_dir(app: &AppHandle) -> PathBuf {
    app.path()
        .app_local_data_dir()
        .expect("app_local_data_dir resolvable")
        .join("models")
}

pub fn onnx_path(app: &AppHandle) -> PathBuf {
    model_dir(app).join("checkpoints").join("kokoro-v1.0.onnx")
}

pub fn voices_path(app: &AppHandle) -> PathBuf {
    model_dir(app).join("data").join("voices-v1.0.bin")
}
