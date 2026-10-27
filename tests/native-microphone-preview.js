// Native ui-test fixture. Only level envelopes are inspected; no audio is saved.
(async () => {
  const invoke = (command, args = {}) =>
    window.__TAURI_INTERNALS__.invoke(command, args);
  const evidence = { passed: false, checks: [] };
  const assert = (ok, message) => {
    if (!ok) throw Error(message);
  };
  const until = async (test) => {
    const end = Date.now() + 10000;
    while (Date.now() < end) {
      if (await test()) return;
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
    throw Error("Microphone preview UI state timed out");
  };
  const click = async (selector) => {
    await until(
      () =>
        document.querySelector(selector) &&
        !document.querySelector(selector).disabled,
    );
    document.querySelector(selector).click();
  };
  try {
    const devices = await invoke("list_devices");
    evidence.devices = devices;
    const realtek = devices.find((d) => /realtek/i.test(d.name));
    assert(realtek, "This hardware fixture requires a Realtek microphone");
    const meetingsBefore = await invoke("list_meetings");
    await click('[data-action="settings"]');
    await click('[data-section="general"]');
    await until(() => document.querySelector("#settings-device"));
    const select = document.querySelector("#settings-device");
    select.value = realtek.id;
    select.dispatchEvent(new Event("change", { bubbles: true }));
    await until(() => localStorage.getItem("omascribe-device") === realtek.id);
    await invoke("show_main_window");
    await until(
      () =>
        document
          .querySelector("[data-microphone-test]")
          ?.getAttribute("aria-pressed") === "false",
    );
    await click("[data-microphone-test]");
    let running;
    await until(async () => {
      running = await invoke("microphone_test_status");
      if (running.error) throw Error(running.error);
      return running.active && !running.starting && running.waveform.length > 3;
    });
    assert(running.device === realtek.id, "Preview opened another input");
    const button = document.querySelector("[data-microphone-test]");
    button.focus();
    evidence.running = {
      device: running.device,
      samples: running.waveform.length,
      peak: running.peak,
      rms: running.rms,
    };
    // Leave the backend alone: the native deadline must stop without UI polling.
    await new Promise((resolve) => setTimeout(resolve, 35000));
    const ended = await invoke("microphone_test_status");
    assert(
      !ended.active && !ended.starting && !ended.error,
      "Native preview did not stop cleanly",
    );
    assert(
      ended.elapsed_ms >= 29000 && ended.elapsed_ms <= 32000,
      "Native deadline was not respected",
    );
    assert(ended.waveform.length <= 80, "Signal history grew without a bound");
    evidence.ended = {
      device: ended.device,
      elapsed_ms: ended.elapsed_ms,
      history: ended.waveform.length,
      peak: ended.peak,
      rms: ended.rms,
    };
    assert(
      document.querySelector("[data-microphone-test]") === button,
      "Polling replaced the focused control",
    );
    evidence.checks.push(
      "Realtek input selected and persisted; actual input envelope updates without replacing controls",
    );
    evidence.checks.push(
      "Native30s deadline stops capture without a UI stop command or UI polling",
    );
    await invoke("show_main_window");
    await until(
      () =>
        document
          .querySelector("[data-microphone-test]")
          ?.getAttribute("aria-pressed") === "false",
    );
    await click("[data-microphone-test]");
    await until(async () => (await invoke("microphone_test_status")).active);
    await click('[data-section="appearance"]');
    await until(async () => !(await invoke("microphone_test_status")).active);
    await click('[data-section="general"]');
    await until(
      () => document.querySelector("#settings-device")?.value === realtek.id,
    );
    const languages = document
      .querySelector("[data-microphone-test]")
      .textContent.includes("Start")
      ? [
          ["zh-CN", "开始检测"],
          ["en", "Start test"],
          ["zh-TW", "開始檢測"],
          ["zh-CN", "开始检测"],
        ]
      : [
          ["en", "Start test"],
          ["zh-CN", "开始检测"],
          ["zh-TW", "開始檢測"],
          ["zh-CN", "开始检测"],
        ];
    for (const [locale, label] of languages) {
      await until(() => document.querySelector("#settings-language"));
      const language = document.querySelector("#settings-language");
      language.value = locale;
      language.dispatchEvent(new Event("change", { bubbles: true }));
      await until(
        () =>
          document.querySelector("#settings-device")?.value === realtek.id &&
          document.querySelector("[data-microphone-test]")?.textContent ===
            label,
      );
    }
    assert(
      JSON.stringify(await invoke("list_meetings")) ===
        JSON.stringify(meetingsBefore),
      "Preview created or changed a meeting",
    );
    evidence.checks.push(
      "Leaving settings releases capture; input choice survives four language changes; no meeting created",
    );
    evidence.passed = true;
  } catch (error) {
    evidence.error = String(error);
  } finally {
    await invoke("stop_microphone_test");
    await invoke("ui_test_report", { report: evidence });
    await invoke("quit");
  }
})();
