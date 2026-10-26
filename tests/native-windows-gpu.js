// Existing ui-test startup hook; use only an isolated workspace and the normal
// signed candidate selector. The runner supplies a fixed phase/fictional meeting.
(async () => {
  const config = window.__omascribeGpuQa || { phase: "download" };
  const invoke = (command, args = {}) => window.__TAURI_INTERNALS__.invoke(command, args);
  const evidence = { phase: config.phase, checks: [], passed: false };
  let cancelled = false;
  document.addEventListener("keydown", (event) => {
    if (event.isTrusted && event.key === "Escape") cancelled = true;
  });
  const assert = (condition, message) => { if (!condition) throw Error(message); };
  const until = async (test, timeout = 30000) => {
    const end = Date.now() + timeout;
    while (Date.now() < end) {
      if (cancelled) throw Error("GPU acceptance stopped by user Escape");
      if (await test()) return;
      await new Promise((resolve) => setTimeout(resolve, 250));
    }
    throw Error("GPU acceptance state timed out");
  };
  const button = async (selector) => {
    await until(() => document.querySelector(selector));
    const element = document.querySelector(selector);
    assert(!element.disabled, "Required settings action is disabled: " + selector);
    element.click();
  };
  const models = async () => {
    await button('[data-action="settings"]');
    await button('[data-section="models"]');
    await until(() => document.querySelector("#acceleration-components"));
  };
  const catalog = () => invoke("acceleration_components");
  const componentId = "cuda-windows-x86_64";
  const test = async () => {
    await button(`[data-component-action="test"][data-id="${componentId}"]`);
    await until(async () => {
      const state = (await catalog()).verification;
      if (["test_failed", "cpu_preferred"].includes(state.state)) throw Error(JSON.stringify(state));
      return state.state === "active";
    }, 180000);
    evidence.enabled = (await catalog()).verification;
    evidence.checks.push("Normal local CPU/GPU fact, offload and speed gate enabled the signed component");
  };
  const generate = async () => {
    assert(config.meeting, "Runner must seed a fictional transcript in this private workspace");
    const before = await invoke("snapshot", { id: config.meeting });
    const target = Math.max(...before.segments.map((s) => s.seq));
    assert(target > 0, "Missing fictional transcript");
    if (config.phase === "fault") await invoke("ui_test_report", { report: { ...evidence, stage: "fault-ready", passed: null } });
    await invoke("generate_notes", { id: config.meeting });
    let snapshot;
    await until(async () => {
      snapshot = await invoke("snapshot", { id: config.meeting });
      return !snapshot.runtime.notes_busy && snapshot.meeting.notes_cursor >= target;
    }, 180000);
    assert(snapshot.suggestions.length, "No generated document");
    assert(JSON.stringify(snapshot.segments) === JSON.stringify(before.segments), "Generation changed the fictional source");
    for (const section of snapshot.suggestions) for (const citation of section.sources) {
      assert(snapshot.segments.some((s) => s.id === citation.segment_id && s.revision === citation.revision && s.text === citation.quote), "Invalid generated citation");
    }
    evidence.generation = { meeting: config.meeting, cursor: snapshot.meeting.notes_cursor, sections: snapshot.suggestions.length, runtime: snapshot.runtime };
    evidence.checks.push("Generation covered immutable fictional transcript with exact citations; runner separately audits effective backend");
  };
  try {
    if (config.phase === "download") {
      evidence.before = await catalog();
      assert(!evidence.before.components.some((c) => c.installed), "Download case requires a fresh workspace");
      assert(evidence.before.verification.state !== "active", "Detection enabled GPU without testing");
      const policy = await invoke("get_network_policy");
      await invoke("set_network_policy", { policy: { ...policy, allow_model_downloads: true } });
      const prefs = await invoke("get_preferences");
      await invoke("set_preferences", { preferences: { ...prefs, inference_backend: "auto", threads: 2, context_size: 2048, memory_budget_mib: 4096, auto_notes: true, translation_mode: "bilingual", minutes_language: "zh", llm_keep_alive_seconds: 0 } });
      await models();
      await button('[data-component-action="check"]');
      await until(async () => (await catalog()).state === "ready", 60000);
      evidence.catalog = await catalog();
      assert(evidence.catalog.components.some((c) => c.component.id === componentId), "Signed CUDA candidate is absent");
      await button(`[data-component-action="download"][data-id="${componentId}"]`);
      await until(async () => {
        const c = (await catalog()).components.find((c) => c.component.id === componentId);
        if (c?.download?.state === "failed") throw Error(JSON.stringify(c.download));
        return c?.installed;
      }, 900000);
      evidence.installed = await catalog();
      assert(evidence.installed.verification.state !== "active", "Download enabled GPU without testing");
      evidence.checks.push("Settings fetched normal-key signed catalog, downloaded and installed immutable CUDA archive without automatic activation");
      await test();
    } else {
      evidence.before = await catalog();
      await models();
      if (["restart", "cpu", "fault"].includes(config.phase)) assert(evidence.before.verification.state === "active", "Restart lost the hardware/model/runtime-bound activation");
      if (config.phase === "enable") await test();
      if (config.phase === "cpu") {
        await button('[data-component-action="disable"]');
        await until(async () => (await catalog()).verification.state !== "active");
        evidence.checks.push("Settings Use CPU cleared activation; runner audits worker cleanup and next backend");
      }
      if (config.phase === "memory") {
        const prefs = await invoke("get_preferences");
        await invoke("set_preferences", { preferences: { ...prefs, context_size: 16384 } });
        await until(() => document.querySelector(`[data-component-action="test"][data-id="${componentId}"]`));
        await button(`[data-component-action="test"][data-id="${componentId}"]`);
        await until(async () => (await catalog()).verification.state === "test_failed", 60000);
        evidence.memory = (await catalog()).verification;
        assert(/memory|显存|GPU/i.test(evidence.memory.reason || ""), "Failure did not identify GPU memory admission");
        await invoke("set_preferences", { preferences: prefs });
        assert((await catalog()).verification.state !== "active", "Memory refusal retained activation");
        evidence.checks.push("Real device memory admission refused the larger context; this is not a driver allocation-OOM injection");
      }
      if (config.meeting) await generate();
    }
    evidence.after = await catalog();
    evidence.passed = true;
  } catch (error) {
    evidence.error = String(error);
    try { evidence.after = await catalog(); } catch (_) { /* Keep the original failure. */ }
  } finally {
    await invoke("ui_test_report", { report: evidence });
    await invoke("quit");
  }
})();
