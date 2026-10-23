// Automatic model and visual QA on a fictional fixture. No microphone capture.
(async () => {
  const invoke = (cmd, args = {}) =>
    window.__TAURI_INTERNALS__.invoke(cmd, args);
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  const until = async (fn) => {
    for (let i = 0; i < 900; i++) {
      if (await fn()) return;
      await sleep(250);
    }
    throw Error("Timeout");
  };
  const check = (x, s) => {
    if (!x) throw Error(s);
  };
  try {
    await until(() => document.querySelector(".meeting-link"));
    const meetings = await invoke("list_meetings");
    const m = meetings.find((m) => m.title.startsWith("Example"));
    check(m, "Missing fixture");
    document.querySelector(`[data-meeting="${m.id}"]`).click();
    await sleep(1200);
    await invoke("update_meeting", {
      id: m.id,
      title: m.title,
      autoNotes: true,
    });
    const start = Date.now();
    await until(async () => {
      const s = await invoke("snapshot", { id: m.id });
      return (
        s.meeting.notes_cursor === s.segments.length &&
        s.suggestions.length > 0 &&
        !s.runtime.notes_busy
      );
    });
    const s = await invoke("snapshot", { id: m.id });
    check(
      s.suggestions.every((s) => s.sources.length && !s.text.includes("3000")),
      "Invalid evidence",
    );
    check(
      s.suggestions.some((s) => s.text.includes("张老师")),
      "Missing owner",
    );
    check(
      s.suggestions.some((s) => s.text.includes("周五")),
      "Missing date",
    );
    check(
      s.suggestions.some((s) => s.text.includes("预算")),
      "Missing keyword",
    );
    await invoke("ui_test_report", {
      report: {
        passed: true,
        fixture_id: m.id,
        latency_ms: Date.now() - start,
        document: s.suggestions,
        phase: "document",
      },
    });
    for (const language of ["en", "zh-CN", "zh-TW"]) {
      document.querySelector('[data-action="settings"]').click();
      await until(() => document.querySelector("#language"));
      const select = document.querySelector("#language");
      select.value = language;
      select.dispatchEvent(new Event("change"));
      await sleep(2000);
      await invoke("ui_test_screenshot", { name: "workspace-" + language });
    }
    // Capture the matching slash-command menu and right-side transcript dock.
    document.querySelector('[data-action="settings"]').click();
    await until(() => document.querySelector("#language"));
    const en = document.querySelector("#language");
    en.value = "en";
    en.dispatchEvent(new Event("change"));
    await sleep(1600);
    const prefs = await invoke("get_view_settings");
    prefs.transcript_mode = "docked";
    await invoke("set_view_settings", { preferences: prefs });
    await sleep(1000);
    await invoke("ui_test_screenshot", { name: "transcript-dock" });
    prefs.transcript_mode = "hidden";
    await invoke("set_view_settings", { preferences: prefs });
    const editor = document.querySelector(".tiptap");
    editor.focus();
    const range = document.createRange();
    range.selectNodeContents(editor);
    range.collapse(false);
    getSelection().removeAllRanges();
    getSelection().addRange(range);
    document.execCommand("insertParagraph");
    document.execCommand("insertText", false, "/");
    await sleep(600);
    await invoke("ui_test_screenshot", { name: "slash-menu" });
    editor.dispatchEvent(
      new KeyboardEvent("keydown", {
        key: "Escape",
        bubbles: true,
        cancelable: true,
      }),
    );
    document.execCommand("delete");
    await invoke("ui_test_report", {
      report: {
        passed: true,
        fixture_id: m.id,
        latency_ms: Date.now() - start,
        document: s.suggestions,
        phase: "screenshots",
      },
    });
  } catch (e) {
    await invoke("ui_test_report", {
      report: { passed: false, error: String(e), stack: e.stack },
    });
  }
})();
