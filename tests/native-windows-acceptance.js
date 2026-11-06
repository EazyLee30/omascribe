// Run by the existing ui-test feature in an isolated native WebView2 workspace.
// No microphone, debugger port, unsigned component activation or private content.
(async () => {
  const invoke = (command, args = {}) => window.__TAURI_INTERNALS__.invoke(command, args);
  const checks = [];
  const languages = [];
  let cancelled = false;
  document.addEventListener("keydown", (event) => { if (event.isTrusted && event.key === "Escape") cancelled = true; });
  const assert = (value, message) => { if (!value) throw Error(message); };
  const until = async (test) => {
    for (let n = 0; n < 100; n++) {
      if (cancelled) throw Error("Native acceptance stopped by user Escape");
      try { if (await test()) return; } catch (_) { /* A native window may still be opening. */ }
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
    throw Error("Native UI did not reach the expected state");
  };
  try {
    await until(() => document.querySelector('[data-action="settings"]'));
    const preferences = await invoke("get_view_settings");
    await invoke("set_view_settings", { preferences: { ...preferences, companion_enabled: true, transcript_mode: "floating" } });
    await until(async () => {
      const windows = await invoke("ui_test_windows");
      return ["main", "companion", "transcript"].every((label) => windows.some((w) => w.label === label && w.visible));
    });
    await invoke("ui_test_window_probe", { label: "companion", expand: true });
    document.querySelector('[data-action="settings"]').click();
    await until(() => document.querySelector('[data-section="general"]'));
    document.querySelector('[data-section="general"]').click();
    for (const [locale, start, caption] of [
      ["en", "Start recording", "Live captions"],
      ["zh-CN", "开始录音", "实时字幕"],
      ["zh-TW", "開始錄音", "即時字幕"],
      ["en", "Start recording", "Live captions"],
      ["zh-CN", "开始录音", "实时字幕"],
    ]) {
      await until(() => document.querySelector("#settings-language"));
      const select = document.querySelector("#settings-language");
      select.value = locale;
      select.dispatchEvent(new Event("change", { bubbles: true }));
      let main, companion, transcript;
      await until(async () => {
        main = await invoke("ui_test_window_probe", { label: "main", expand: false });
        companion = await invoke("ui_test_window_probe", { label: "companion", expand: false });
        transcript = await invoke("ui_test_window_probe", { label: "transcript", expand: false });
        return main.lang === locale && companion.lang === locale && companion.start?.includes(start) && transcript.captionLabel === caption;
      });
      assert(companion.expanded === "true", "Language switch collapsed the companion");
      assert(!main.errors.length && !companion.errors.length && !transcript.errors.length, "Native window raised a script error");
      languages.push({ locale, main, companion, transcript });
    }
    checks.push("Open native companion/transcript follow five language changes and preserve expansion");
    await until(() => document.querySelector('[data-section="models"]'));
    document.querySelector('[data-section="models"]').click();
    await until(() => document.querySelector("#acceleration-components"));
    const hardware = await invoke("hardware_profile");
    const components = await invoke("acceleration_components");
    const gpuText = document.querySelector("#acceleration-components").textContent;
    for (const device of hardware.gpus.devices) assert(gpuText.includes(device.name), "Detected GPU missing from native settings");
    assert(components.verification.state !== "active", "Hardware detection activated GPU inference");
    checks.push("Native hardware reaches settings while activation still requires component verification");
    await invoke("ui_test_report", { report: {
      passed: true, method: "native ui-test feature / fixed cross-window DOM probes",
      checks, languages, hardware, windows: await invoke("ui_test_windows"),
      limits: ["Functional native-window checks; screenshots are separate visual evidence.", "No settings download/activation claim before a signed candidate catalog is available."],
    } });
  } catch (error) {
    await invoke("ui_test_report", { report: { passed: false, checks, languages, error: String(error) } });
  } finally {
    await invoke("quit");
  }
})();
