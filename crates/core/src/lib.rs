pub mod audio;
pub mod engine;
pub mod files;
pub mod notes;
pub mod store;
pub mod types;
pub use engine::Engine;
pub use types::*;
pub fn build_identity() -> serde_json::Value {
    serde_json::json!({"schema":1,"source_sha256":env!("OMASCRIBE_SOURCE_SHA256"),"revision_at_compile":env!("OMASCRIBE_COMPILE_REVISION"),"rustc":env!("OMASCRIBE_COMPILE_RUSTC"),"target":env!("OMASCRIBE_COMPILE_TARGET"),"profile":env!("OMASCRIBE_COMPILE_PROFILE")})
}
pub mod platform;

pub mod desktheme;

pub mod inhibit;

mod capture;
pub use capture::wav_pcm_info;
#[cfg(any(target_os = "macos", target_os = "windows"))]
mod native_audio;

pub mod acceleration;
pub mod asr_hotwords;
pub mod components;
pub mod hardware;
pub mod inference;
mod inference_cache;
pub mod inference_trace;
mod inference_worker;
mod minutes_schedule;
mod model_integrity;
mod probe;
mod source_context;
mod task_roles;
pub use inference_worker::WorkerStatus;
pub mod library;
pub mod microphone;
pub mod models;
pub mod network;
pub mod preferences;
pub mod speakers;

pub mod update_signature;

mod recycle;

pub mod translation;

pub mod terminology;
