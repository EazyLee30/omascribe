// Runs inside the real Tauri/WebKit application, never in a mock browser.
// Use an isolated /tmp workspace. Brief microphone recordings stay in that workspace.
(async () => {
  const invoke = (cmd, args = {}) =>
    window.__TAURI_INTERNALS__.invoke(cmd, args);
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  const until = async (fn, ms = 15000) => {
    const end = Date.now() + ms;
    while (Date.now() < end) {
      if (await fn()) return;
      await sleep(150);
    }
    throw Error("Timed out: " + String(fn));
  };
  const click = (q) => {
    const e = document.querySelector(q);
    if (!e) throw Error("Missing " + q);
    e.click();
  };
  const assert = (v, m) => {
    if (!v) throw Error(m);
  };
  const checks = [];
  const writerPhases = new Set();
  let capture;
  const source = async (text) => {
    click('[data-format="source"]');
    const e = document.querySelector(".markdown-source");
    e.value = text;
    e.dispatchEvent(new Event("input"));
    click('[data-format="source"]');
    await sleep(900);
  };
  try {
    await until(() => document.querySelector('[data-action="new"]'));
    const old = document.querySelector(".meeting-link.selected")?.dataset
      .meeting;
    click('[data-action="new"]');
    await until(
      () =>
        document.querySelector(".tiptap") &&
        document.querySelector(".meeting-link.selected")?.dataset.meeting !==
          old,
    );
    const mid = document.querySelector(".meeting-link.selected").dataset
      .meeting;
    await invoke("update_meeting", {
      id: mid,
      title: "Native validation · recording controls",
      autoNotes: false,
    });
    await source("");
    let editor = document.querySelector(".tiptap");
    editor.focus();
    document.execCommand("insertText", false, "/table");
    await until(() => !document.querySelector(".slash-menu").hidden);
    editor.dispatchEvent(
      new KeyboardEvent("keydown", {
        key: "Enter",
        bubbles: true,
        cancelable: true,
      }),
    );
    await until(() => document.querySelector(".tiptap table"));
    await sleep(900);
    let snap = await invoke("snapshot", { id: mid });
    assert(
      snap.notes.some((n) => n.content.includes("|")),
      "Table did not round-trip to Markdown",
    );
    checks.push("Slash command inserts a Markdown table");
    await source("");
    editor.focus();
    document.execCommand("insertText", false, "/");
    await until(() => !document.querySelector(".slash-menu").hidden);
    editor.dispatchEvent(
      new KeyboardEvent("keydown", {
        key: "Escape",
        bubbles: true,
        cancelable: true,
      }),
    );
    document.execCommand("insertText", false, "normal / path");
    await sleep(900);
    assert(
      document.querySelector(".slash-menu").hidden,
      "Literal slash opened menu",
    );
    assert(
      editor.textContent.includes("/normal / path"),
      "Literal slash was lost",
    );
    checks.push("Escape preserves literal slash and inline slash stays text");
    await source(
      "# Meeting notes\n\n- **预算** 3000？\n- [ ] Friday test\n\n> Personal thought.\n",
    );
    assert(document.querySelector(".tiptap h1"), "Rendered heading missing");
    assert(
      document.querySelector(".tiptap input[type=checkbox]"),
      "Task missing",
    );
    editor.focus();
    editor.dispatchEvent(
      new CompositionEvent("compositionstart", { bubbles: true }),
    );
    document.execCommand("insertText", false, "中文输入");
    editor.dispatchEvent(
      new KeyboardEvent("keydown", {
        key: "Enter",
        ctrlKey: true,
        isComposing: true,
        bubbles: true,
      }),
    );
    editor.dispatchEvent(
      new CompositionEvent("compositionend", { bubbles: true }),
    );
    await sleep(1100);
    snap = await invoke("snapshot", { id: mid });
    assert(
      snap.notes.some((n) => n.content.includes("中文输入")),
      "IME save failed",
    );
    checks.push(
      "Markdown rendering, task list, Chinese composition and autosave",
    );
    click('[data-action="record"]');
    await until(
      async () => (await invoke("snapshot", { id: mid })).runtime.recording,
    );
    await sleep(3500);
    click('[data-action="pause-recording"]');
    await until(
      async () => (await invoke("snapshot", { id: mid })).runtime.paused,
    );
    await sleep(800);
    const paused = (await invoke("snapshot", { id: mid })).meeting.duration_ms;
    await sleep(1800);
    assert(
      (await invoke("snapshot", { id: mid })).meeting.duration_ms === paused,
      "Audio continued during pause",
    );
    click('[data-action="record"]');
    await until(
      async () => (await invoke("snapshot", { id: mid })).runtime.recording,
    );
    await sleep(3000);
    click('[data-action="stop-recording"]');
    await until(async () => {
      const s = await invoke("snapshot", { id: mid });
      return !s.runtime.recording && !s.runtime.transcribing;
    }, 45000);
    capture = await invoke("snapshot", { id: mid });
    assert(
      capture.meeting.status === "ready",
      "Recording failed " + capture.runtime.error,
    );
    assert(
      capture.meeting.duration_ms > paused + 2000,
      "Resume did not append audio",
    );
    checks.push("Start → pause (duration freezes) → resume (append) → finish");
    await until(() => !document.querySelector('#record').disabled);
    click('[data-action="record"]');
    await until(async () => (await invoke('snapshot', {id: mid})).runtime.recording);
    await sleep(2200);
    click('[data-action="stop-recording"]');
    await until(async () => {
      const s = await invoke('snapshot', {id: mid});
      return !s.runtime.recording && !s.runtime.transcribing;
    }, 45000);
    const appended = await invoke('snapshot', {id: mid});
    assert(appended.meeting.duration_ms > capture.meeting.duration_ms + 1200, 'Second take did not append');
    const files = await invoke('meeting_files', {id: mid});
    assert(files.files.filter(f => f.kind === 'audio').length === 2, 'Expected two WAV files');
    assert(files.files.every(f => f.status === 'ready'), 'Unfinished file');
    click('[data-action="meeting-files"]');
    await until(() => document.querySelectorAll('.meeting-file').length === 5);
    assert(document.querySelector('#meeting-files-path').textContent === files.directory, 'Wrong folder');
    if (window.__OMASCRIBE_SCREENSHOTS__) await invoke('ui_test_screenshot', {name: 'meeting-files'});
    await invoke('open_meeting_files', {id: mid, name: 'recording-001.wav'});
    click('[data-action="close-modal"]');
    checks.push('Finish → record another take in the same meeting; five readable files and folder controls');

    const clip = await invoke("audio_clip", {
      id: mid,
      startMs: 0,
      endMs: 3000,
    });
    assert(clip.startsWith("UklGR"), "WAV invalid");
    const exported = await invoke("export_meeting", { id: mid, audio: true });
    checks.push("Local WAV playback and Markdown + audio export");
    const m2 = await invoke("create_meeting", {
      title: "Native validation · finish while paused",
    });
    await invoke("update_meeting", {
      id: m2.id,
      title: m2.title,
      autoNotes: false,
    });
    await invoke("start_recording", { id: m2.id, device: "auto" });
    await sleep(2100);
    await invoke("pause_recording");
    await until(
      async () => (await invoke("snapshot", { id: m2.id })).runtime.paused,
    );
    await sleep(600);
    await invoke("stop_recording");
    await until(async () => {
      const s = await invoke("snapshot", { id: m2.id });
      return s.meeting.status === "ready" && !s.runtime.transcribing;
    }, 45000);
    checks.push("Finish while paused drains ASR and saves meeting");
    let views = await invoke("get_view_settings");
    views.transcript_mode = "floating";
    views.compact = false;
    views.snap = "right";
    await invoke("set_view_settings", { preferences: views });
    await sleep(1800);
    views.compact = true;
    await invoke("set_view_settings", { preferences: views });
    await sleep(400);
    views.compact = false;
    views.snap = "left";
    await invoke("set_view_settings", { preferences: views });
    await sleep(400);
    views.transcript_mode = "docked";
    await invoke("set_view_settings", { preferences: views });
    await until(() => document.querySelector("#workspace.dock-transcript"));
    checks.push("Floating, compact, edge snap and right dock modes");
    const meetings = await invoke("list_meetings");
    const fixture = meetings.find((m) => m.title.startsWith("Example"));
    assert(fixture, "Missing fictional fixture");
    click(`[data-meeting="${fixture.id}"]`);
    await until(
      () =>
        document.querySelector(".tiptap h1")?.textContent ===
        "Friday prototype review",
    );
    views = await invoke("get_view_settings");
    views.transcript_mode = "hidden";
    views.captions.enabled = true;
    views.captions.position = "top";
    views.captions.idle_seconds = 0;
    await invoke("set_view_settings", { preferences: views });
    await sleep(1200);
    views.captions.position = "bottom";
    await invoke("set_view_settings", { preferences: views });
    await sleep(1000);
    views.captions.enabled = false;
    await invoke("set_view_settings", { preferences: views });
    checks.push("Native screen captions at top and bottom");
    const started = Date.now();
    await invoke("update_meeting", {
      id: fixture.id,
      title: fixture.title,
      autoNotes: true,
    });
    await until(
      async () =>
        (await invoke("snapshot", { id: fixture.id })).runtime.notes_busy,
      15000,
    );
    await until(async () => {
      const s = await invoke("snapshot", { id: fixture.id });
      if (s.runtime.notes_status.startsWith('writer:')) writerPhases.add(s.runtime.notes_status.split(':')[1]);
      if (
        !s.runtime.notes_busy &&
        s.runtime.notes_status &&
        !s.suggestions.length
      )
        throw Error(s.runtime.notes_status);
      return s.suggestions.length > 0 && !s.runtime.notes_busy;
    }, 620000);
    const result = await invoke("snapshot", { id: fixture.id });
    assert(
      result.suggestions.every((s) => s.sources.length),
      "Missing citations",
    );
    assert(
      result.suggestions.every((s) => !s.text.includes("3000")),
      "Personal estimate became a fact",
    );
    assert(
      result.suggestions.some((s) => /未|不定/.test(s.text)),
      "Lost uncertainty",
    );
    await until(() => document.querySelector(".minutes-document"));
    assert(
      !document.querySelector("#generate"),
      "Manual update button still required",
    );
    checks.push(
      "Automatic keyword + transcript pipeline produces cited document without update button",
    );
    const section = result.suggestions[0];
    await invoke("update_suggestion", {
      meeting: fixture.id,
      id: section.id,
      status: "accepted",
      text: section.text + "（人工校订）",
    });
    await invoke("update_meeting", {
      id: fixture.id,
      title: fixture.title,
      autoNotes: false,
    });
    const path = await invoke("export_meeting", {
      id: fixture.id,
      audio: false,
    });
    checks.push("Edited paragraph protected and full minutes exported");
    await invoke("ui_test_report", {
      report: {
        passed: true,
        checks,
        recording_duration_ms: capture.meeting.duration_ms,
        rtf: capture.runtime.asr_rtf,
        export_path: exported,
        document_export: path,
        document_latency_ms: Date.now() - started,
        writer_phases: [...writerPhases],
        model_document: result.suggestions,
        fixture_id: fixture.id,
      },
    });
  } catch (error) {
    await invoke("ui_test_report", {
      report: {
        passed: false,
        checks,
        error: String(error),
        stack: error.stack,
      },
    });
  }
})();
