export interface Meeting {
  id: string;
  title: string;
  created_at: string;
  status: string;
  duration_ms: number;
  run: number;
  notes_cursor: number;
  auto_notes: boolean;
  diarization: boolean;
  diarization_cursor: number;
  speaker_names: Record<string, string>;
}
export interface Segment {
  id: string;
  meeting_id: string;
  run: number;
  seq: number;
  start_ms: number;
  end_ms: number;
  text: string;
  revision: number;
  speaker: string | null;
  speaker_manual: boolean;
}
export interface Note {
  id: string;
  meeting_id: string;
  content: string;
  kind: string;
  anchor_ms: number;
  updated_at: string;
}
export interface Source {
  segment_id: string;
  revision: number;
  start_ms: number;
  end_ms: number;
  quote: string;
  speaker_name?: string | null;
}
export interface Suggestion {
  focus: string;
  id: string;
  meeting_id: string;
  kind: string;
  text: string;
  sources: Source[];
  status: string;
  stale: boolean;
}
export interface Runtime {
  meeting_id: string | null;
  recording: boolean;
  paused: boolean;
  transcribing: boolean;
  elapsed_ms: number;
  processed_ms: number;
  level: number;
  partial: string;
  asr_rtf: number;
  notes_busy: boolean;
  translation_busy: boolean;
  translation_status: string;
  diarization_busy: boolean;
  diarization_meeting_id: string | null;
  diarization_status: string;
  notes_status: string;
  notes_elapsed_ms: number;
  error: string | null;
}
export interface Snapshot {
  meeting: Meeting;
  segments: Segment[];
  notes: Note[];
  suggestions: Suggestion[];
  runtime: Runtime;
  translation_mode: string;
  translations: Record<
    string,
    { text: string; target: string; revision: number }
  >;
}
export interface Diagnostic {
  data_dir: string;
  asr_ready: boolean;
  llm_ready: boolean;
  recorder_ready: boolean;
  worker_ready: boolean;
  server_ready: boolean;
  available_memory_mib: number | null;
  accent: string;
  version: string;
}

export interface MeetingFile {
  name: string;
  path: string;
  kind: "audio" | "transcript";
  bytes: number;
  take: number | null;
  start_ms: number;
  duration_ms: number;
  status: string;
}
export interface MeetingFiles {
  directory: string;
  files: MeetingFile[];
}
