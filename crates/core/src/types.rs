use serde::{Deserialize, Serialize};

pub const SAMPLE_RATE: u64 = 16_000;
pub const CHUNK_SAMPLES: u64 = SAMPLE_RATE * 30;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Meeting {
    pub id: String,
    pub title: String,
    pub created_at: String,
    pub status: String,
    pub duration_ms: u64,
    pub run: u32,
    pub notes_cursor: u32,
    pub auto_notes: bool,
    #[serde(default)]
    pub notes_fingerprint: String,
    #[serde(default)]
    pub diarization: bool,
    #[serde(default)]
    pub diarization_cursor: u32,
    #[serde(default)]
    pub speaker_names: std::collections::BTreeMap<String, String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordingTake {
    pub id: String,
    pub meeting_id: String,
    pub number: u32,
    pub started_at: String,
    pub start_sample: u64,
    pub end_sample: Option<u64>,
    pub status: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeetingFile {
    pub name: String,
    pub path: String,
    pub kind: String,
    pub bytes: u64,
    pub take: Option<u32>,
    pub start_ms: u64,
    pub duration_ms: u64,
    pub status: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeetingFiles {
    pub directory: String,
    pub files: Vec<MeetingFile>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Segment {
    pub id: String,
    pub meeting_id: String,
    pub run: u32,
    pub seq: u32,
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
    pub revision: u32,
    #[serde(default)]
    pub speaker: Option<String>,
    #[serde(default)]
    pub speaker_manual: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserNote {
    pub id: String,
    pub meeting_id: String,
    pub content: String,
    pub kind: String,
    pub anchor_ms: u64,
    pub updated_at: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Source {
    pub segment_id: String,
    pub revision: u32,
    pub start_ms: u64,
    pub end_ms: u64,
    pub quote: String,
    #[serde(default)]
    pub speaker_name: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Suggestion {
    #[serde(default)]
    pub focus: String,
    pub id: String,
    pub meeting_id: String,
    pub kind: String,
    pub text: String,
    pub sources: Vec<Source>,
    pub status: String,
    pub stale: bool,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RuntimeStatus {
    pub meeting_id: Option<String>,
    pub recording: bool,
    pub paused: bool,
    pub transcribing: bool,
    pub elapsed_ms: u64,
    pub processed_ms: u64,
    pub level: f32,
    pub partial: String,
    pub asr_rtf: f64,
    pub notes_busy: bool,
    #[serde(default)]
    pub translation_busy: bool,
    #[serde(default)]
    pub translation_status: String,
    pub notes_status: String,
    pub notes_elapsed_ms: u64,
    pub error: Option<String>,
    #[serde(default)]
    pub diarization_busy: bool,
    #[serde(default)]
    pub diarization_meeting_id: Option<String>,
    #[serde(default)]
    pub diarization_status: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub meeting: Meeting,
    pub segments: Vec<Segment>,
    pub notes: Vec<UserNote>,
    pub suggestions: Vec<Suggestion>,
    pub runtime: RuntimeStatus,
    #[serde(default)]
    pub translation_mode: String,
    #[serde(default)]
    pub translations: std::collections::HashMap<String, crate::translation::Translation>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Device {
    pub id: String,
    pub name: String,
    /// `microphone`, `mixed` (mic + system audio) or `system` (a playback
    /// monitor / loopback source). Older records deserialize as empty.
    #[serde(default)]
    pub kind: String,
}

impl Device {
    /// An input that captures the room through a microphone.
    pub fn microphone(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            kind: "microphone".into(),
        }
    }
    /// A single input that already mixes microphone and system audio.
    pub fn mixed(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            kind: "mixed".into(),
        }
    }
    /// A playback monitor, i.e. what applications are sending to a speaker.
    pub fn system(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            kind: "system".into(),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostics {
    pub data_dir: String,
    pub asr_ready: bool,
    pub llm_ready: bool,
    pub recorder_ready: bool,
    pub worker_ready: bool,
    pub server_ready: bool,
    pub available_memory_mib: Option<u64>,
    pub accent: String,
    /// Active desktop theme when the session is an Omarchy one.
    #[serde(default)]
    pub desktop_theme: Option<crate::desktheme::DesktopTheme>,
    pub version: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum AsrEvent {
    Ready,
    Progress {
        processed_ms: u64,
        rtf: f64,
    },
    Partial {
        text: String,
    },
    /// An utterance was flushed/reset; contains no provisional transcript content.
    Boundary {
        processed_ms: u64,
        cause: String,
    },
    Segment {
        seq: u32,
        start_ms: u64,
        end_ms: u64,
        text: String,
    },
    Done {
        processed_ms: u64,
        rtf: f64,
    },
    Error {
        message: String,
    },
}
