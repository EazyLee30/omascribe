pub mod audio;
pub mod engine;
pub mod files;
pub mod notes;
pub mod store;
pub mod types;
pub use engine::Engine;
pub use types::*;
pub mod platform;

mod capture;
#[cfg(any(target_os = "macos", target_os = "windows"))]
mod native_audio;
