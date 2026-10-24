# Privacy and security

Audio, transcripts, user notes, and model inference stay on the local machine. Setup downloads public software and model weights; explicit model downloads in Settings contact pinned public model hosts. Normal recording, transcription, note generation, playback, and export require no remote service or account.

The local LLM HTTP server listens on loopback with a fresh API key. It is terminated after each request. Meeting files are stored in a directory readable only by the current user. This is ordinary filesystem protection, not encryption against another process running as that user. Use full-disk encryption for protection at rest.

AI-written minutes are generated prose, not verified facts. Source links, numeric checks, uncertainty checks and revision validation catch specific failures; they do not establish semantic correctness. The model sees keywords as interests and transcript as untrusted data. Review source text and audio for important decisions. Manual edits are protected from automatic replacement.

Diagnostic job/history records stay in the local SQLite database and can contain meeting evidence. Exported Markdown and WAV files contain meeting content. Caption visibility is disabled on app startup, while display appearance settings are retained.

Please report a vulnerability without attaching real meeting audio or private notes. The repository's GitHub security reporting feature is the preferred channel when available. For ordinary bugs, use a synthetic reproduction and redact paths or personal content.

Shared configuration JSON contains only validated settings and model IDs (plus a user-chosen name and hardware description); it excludes meeting/library content and filesystem paths. The separate private-library export includes notes, terms and memory. Knowledge and memory are opt-in context, not evidence. Speaker clustering stays local; users supply names manually. Downloads are checked against catalog hashes before installation. Imported configuration cannot change URLs or run commands.
