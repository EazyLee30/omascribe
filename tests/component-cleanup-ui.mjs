// Real settings UI and confirmation lifecycle; fictional local inventory and mocked IPC only.
import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { build } from "esbuild";

const require = createRequire(import.meta.url);
const { chromium } = require(
  process.env.OMASCRIBE_PLAYWRIGHT_MODULE || "playwright",
);
const bundle = await build({
  entryPoints: ["src/settings.ts"],
  bundle: true,
  format: "iife",
  globalName: "Settings",
  write: false,
  plugins: [{
    name: "mock-cleanup-ipc",
    setup(build) {
      build.onResolve({ filter: /^@tauri-apps\/api\/(core|event)$/ }, (args) => ({
        path: args.path,
        namespace: "cleanup-ipc",
      }));
      build.onLoad({ filter: /.*/, namespace: "cleanup-ipc" }, () => ({
        contents: `
          export const invoke = (command, args) => window.cleanupInvoke(command, args);
          export const isTauri = () => false;
          export const listen = async () => () => {};
          export const emit = async () => {};
        `,
        loader: "js",
      }));
    },
  }],
});
const browser = await chromium.launch({
  headless: true,
  ...(process.env.OMASCRIBE_BROWSER_EXE
    ? { executablePath: process.env.OMASCRIBE_BROWSER_EXE }
    : {}),
});
const failures = [];
function check(label, assertion) {
  try {
    assertion();
    console.log(`PASS ${label}`);
  } catch (error) {
    failures.push(`${label}: ${error.message}`);
    console.error(`FAIL ${label}: ${error.message}`);
  }
}
try {
  for (const platform of ["windows", "linux"]) {
    const page = await browser.newPage();
    const pageErrors = [];
    page.on("pageerror", (error) => pageErrors.push(error.message));
    await page.route("http://omascribe.test/**", (route) => route.fulfill({
      contentType: "text/html",
      body: '<main id="settings"></main>',
    }));
    await page.goto("http://omascribe.test/");
    await page.addScriptTag({ content: bundle.outputFiles[0].text });
    const observed = await page.evaluate(async (platform) => {
      const calls = [];
      const toasts = [];
      const prefs = {
        appearance: {
          mode: "system", theme: "neutral", accent: "#d83b36",
          background: "", foreground: "", font: "system", font_size: 13,
        },
        asr_model: "fixture-asr", llm_model: "fixture-llm", threads: 2,
        context_size: 2048, inference_backend: "auto", resource_mode: "balanced",
        memory_budget_mib: 0, llm_keep_alive_seconds: 0, auto_notes: true,
        translation_mode: "off", minutes_language: "auto", diarization: false,
        speaker_count: 0, use_knowledge: false, use_memory: false,
      };
      const id = platform === "windows" ? "cuda-windows-x86_64" : "vulkan-linux-x86_64";
      const missing = { id, version: "acceleration-missing", reason: "metadata_missing", busy: false };
      const invalid = { id, version: "acceleration-invalid", reason: "metadata_invalid", busy: false };
      const catalog = {
        state: "catalog_required", verification: null,
        components: [], unavailable: [{ ...missing }, { ...invalid }],
      };
      const hardware = {
        platform, arch: "x86_64", os: "Fixture OS", cpu: "Fixture CPU", cores: 2,
        total_memory_mib: 8192, available_memory_mib: 4096, free_disk_bytes: 8192,
        tier: "balanced", apple_silicon: false, backends: ["cpu"], resource_presets: [],
        gpus: {
          status: "detected", cuda_driver_version: 12080,
          devices: [{ name: "Fixture GPU", vendor: "nvidia" }],
        },
      };
      window.cleanupInvoke = async (command, args) => {
        calls.push({ command, args });
        switch (command) {
          case "get_preferences": return structuredClone(prefs);
          case "get_network_policy": return { allow_model_downloads: false };
          case "diagnostics": return { version: "fixture", worker_ready: true, server_ready: true };
          case "model_status": return [];
          case "hardware_profile": return structuredClone(hardware);
          case "acceleration_components": return structuredClone(catalog);
          case "remove_acceleration_component": return;
          default: throw new Error(`Unexpected cleanup IPC: ${command}`);
        }
      };
      const intervals = new Map();
      let nextTimer = 0;
      window.setInterval = (callback) => {
        const timer = ++nextTimer;
        intervals.set(timer, callback);
        return timer;
      };
      window.clearInterval = (timer) => intervals.delete(timer);
      const settle = () => new Promise((resolve) => setTimeout(resolve, 0));
      const tick = async () => {
        for (const callback of [...intervals.values()]) callback();
        await settle();
      };
      const root = document.querySelector("#settings");
      const handle = await Settings.mountSettings(root, {
        section: "models", toast: (...args) => toasts.push(args),
        back: async () => {}, language: async () => {}, display: async () => {},
        device: () => {},
      });
      const cards = () => [...root.querySelectorAll("#acceleration-components article")];
      const card = (version) => cards().find((entry) => entry.textContent.includes(version));
      const snapshot = () => cards().map((entry) => ({
        text: entry.textContent,
        recommended: entry.querySelectorAll(".badge.selected").length,
        actions: [...entry.querySelectorAll("[data-component-action]")].map((button) => ({
          action: button.dataset.componentAction, id: button.dataset.id,
          version: button.dataset.version, disabled: button.disabled,
        })),
      }));
      const states = { catalog_required: snapshot() };
      for (const state of ["not_published", "ready"]) {
        catalog.state = state;
        await tick();
        states[state] = snapshot();
      }
      catalog.verification = {
        state: "active", component_id: id, version: missing.version,
        reason: null, report: null,
      };
      await tick();
      const staleActive = snapshot();
      catalog.verification = null;
      catalog.unavailable[0].busy = true;
      await tick();
      const busy = snapshot();
      catalog.unavailable[0].busy = false;
      catalog.verification = {
        state: "testing", component_id: id, version: invalid.version,
        reason: null, report: null,
      };
      await tick();
      const testing = snapshot();
      catalog.verification = null;
      await tick();
      calls.length = 0;
      const removals = () => calls.filter((call) => call.command === "remove_acceleration_component");
      const open = async (version) => {
        const button = card(version)?.querySelector('[data-component-action="remove"]');
        if (!button) return null;
        button.click();
        await settle();
        return document.querySelector(".confirm-dialog");
      };
      const canceledDialog = await open(missing.version);
      const canceled = {
        shown: Boolean(canceledDialog), text: canceledDialog?.textContent || "",
        cancelFocused: canceledDialog?.querySelector('[value="cancel"]') === document.activeElement,
      };
      canceledDialog?.close("cancel");
      await settle();
      canceled.calls = removals().length;

      const acceptedDialog = await open(missing.version);
      const replacement = { ...missing, version: "acceleration-replacement" };
      catalog.unavailable = [replacement];
      await tick();
      const accepted = {
        shown: Boolean(acceptedDialog), textAfterPoll: acceptedDialog?.textContent || "",
        replacementShown: Boolean(card(replacement.version)),
      };
      acceptedDialog?.close("delete");
      await settle();
      accepted.calls = structuredClone(removals());

      // The existing healthy retained-version confirmation must keep its semantics.
      catalog.components = [{
        component: {
          id: "fixture-healthy", name: "Healthy retained component", backend: "vulkan",
          archive: { bytes: 1024 }, license: "fixture",
        },
        version: "acceleration-healthy", present: true, installed: true,
        downloadable: false, compatible: true, download: null,
      }];
      await tick();
      const healthyDialog = await open("acceleration-healthy");
      const healthy = { shown: Boolean(healthyDialog), text: healthyDialog?.textContent || "" };
      healthyDialog?.close("cancel");
      await settle();
      healthy.calls = removals().length;

      const closedDialog = await open(replacement.version);
      const closed = { shown: Boolean(closedDialog) };
      await handle.close();
      closedDialog?.close("delete");
      await settle();
      closed.calls = structuredClone(removals());
      return {
        id, states, staleActive, busy, testing, canceled, accepted, healthy, closed,
        timersAfterClose: intervals.size, toasts,
        policyOrNetworkCalls: calls.filter((call) => [
          "get_network_policy", "check_acceleration_components", "download_acceleration_component",
          "verify_acceleration", "disable_acceleration",
        ].includes(call.command)),
      };
    }, platform);
    console.log(JSON.stringify({ platform, observed }));
    check(`${platform} unavailable cards survive remote catalog states`, () => {
      for (const entries of Object.values(observed.states)) {
        assert.equal(entries.length, 2);
        assert.match(entries[0].text, /missing.*cannot be used/i);
        assert.match(entries[1].text, /damaged.*cannot be used/i);
        for (const entry of entries) {
          assert.equal(entry.recommended, 0);
          assert.deepEqual(entry.actions.map((action) => action.action), ["remove"]);
          assert.equal(entry.actions[0].id, observed.id);
          assert.equal(entry.actions[0].disabled, false);
        }
      }
    });
    check(`${platform} unavailable entries cannot inherit active or testing controls`, () => {
      assert.equal(observed.staleActive.length, 2);
      for (const entry of observed.staleActive) {
        assert.doesNotMatch(entry.text, /Verified|automatic|Test & enable/i);
        assert.deepEqual(entry.actions.map((action) => action.action), ["remove"]);
      }
      assert.equal(observed.busy.length, 2);
      assert.equal(observed.busy[0].actions[0].disabled, true);
      assert.equal(observed.busy[1].actions[0].disabled, false);
      assert.equal(observed.testing.length, 2);
      assert.equal(observed.testing.every((entry) => entry.actions[0].disabled), true);
    });
    check(`${platform} cleanup cancellation and exact-version confirmation`, () => {
      assert.equal(observed.canceled.shown, true);
      assert.equal(observed.canceled.cancelFocused, true);
      assert.match(observed.canceled.text, /Clean up unavailable component/i);
      assert.match(observed.canceled.text, /acceleration-missing/);
      assert.match(observed.canceled.text, /Models and meetings will be kept/);
      assert.equal(observed.canceled.calls, 0);
      assert.equal(observed.accepted.shown, true);
      assert.equal(observed.accepted.replacementShown, true);
      assert.match(observed.accepted.textAfterPoll, /acceleration-missing/);
      assert.doesNotMatch(observed.accepted.textAfterPoll, /acceleration-replacement/);
      assert.deepEqual(observed.accepted.calls, [{
        command: "remove_acceleration_component",
        args: { id: observed.id, version: "acceleration-missing" },
      }]);
    });
    check(`${platform} healthy confirmation and close lifecycle remain safe`, () => {
      assert.equal(observed.healthy.shown, true);
      assert.match(observed.healthy.text, /Remove component.*Healthy retained component.*acceleration-healthy/);
      assert.match(observed.healthy.text, /Download and test it again to use it after removal/);
      assert.equal(observed.healthy.calls, 1);
      assert.equal(observed.closed.shown, true);
      assert.deepEqual(observed.closed.calls, observed.accepted.calls);
      assert.equal(observed.timersAfterClose, 0);
      assert.deepEqual(observed.policyOrNetworkCalls, []);
      assert.deepEqual(observed.toasts, []);
      assert.deepEqual(pageErrors, []);
    });
    await page.close();
  }
} finally {
  await browser.close();
}
assert.equal(failures.length, 0, failures.join("\n"));
