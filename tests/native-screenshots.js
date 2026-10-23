// App-owned WebKit snapshots of a pre-generated fictional meeting; no desktop capture.
(async () => {
  const invoke = (cmd, args = {}) =>
    window.__TAURI_INTERNALS__.invoke(cmd, args);
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  const until = async (fn) => {
    for (let i = 0; i < 100; i++) {
      if (await fn()) return;
      await sleep(100);
    }
    throw Error("Timeout");
  };
  try {
    await until(() => document.querySelector(".meeting-link"));
    const meetings = await invoke("list_meetings");
    const m = meetings.find((m) => m.title.startsWith("Example"));
    await invoke("cancel_notes");
    document.querySelector(`[data-meeting="${m.id}"]`).click();
    await invoke("update_meeting", {
      id: m.id,
      title: m.title,
      autoNotes: true,
    });
    await sleep(6000);
    for (const language of ["en", "zh-CN", "zh-TW"]) {
      document.querySelector('[data-action="settings"]').click();
      await until(() => document.querySelector("#language"));
      const select = document.querySelector("#language");
      select.value = language;
      select.dispatchEvent(new Event("change"));
      await sleep(2000);
      await invoke("ui_test_screenshot", { name: "workspace-" + language });
    }
    await invoke("update_meeting", {
      id: m.id,
      title: m.title,
      autoNotes: false,
    });
    await invoke("ui_test_report", {
      report: {
        passed: true,
        screenshots: ["workspace-en", "workspace-zh-CN", "workspace-zh-TW"],
      },
    });
  } catch (e) {
    await invoke("ui_test_report", {
      report: { passed: false, error: String(e) },
    });
  }
})();
