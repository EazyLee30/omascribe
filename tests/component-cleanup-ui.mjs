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
      let allowDownloads = false;
      let policyWait;
      const previewEntry = {
        component: {
          id: "cuda-" + platform + "-x86_64", name: "Fixture CUDA", backend: "cuda",
          archive: { bytes: 1024 }, license: "fixture",
        },
        version: "acceleration-ci-40367160891", preview: true, present: false,
        installed: false, downloadable: true, compatible: true, download: null,
      };
      window.cleanupInvoke = async (command, args) => {
        calls.push({ command, args });
        switch (command) {
          case "get_preferences": return structuredClone(prefs);
          case "get_network_policy":
            await policyWait;
            return { allow_model_downloads: allowDownloads };
          case "diagnostics": return { version: "fixture", worker_ready: true, server_ready: true };
          case "model_status": return [];
          case "hardware_profile": return structuredClone(hardware);
          case "acceleration_components": return structuredClone(catalog);
          case "check_acceleration_components":
            catalog.state = args?.preview ? "ready" : "not_published";
            catalog.components = args?.preview ? [previewEntry] : [];
            return structuredClone(catalog);
          case "download_acceleration_component": return;
          case "cancel_acceleration_download": return;
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
      const mount = () => Settings.mountSettings(root, {
        section: "models", toast: (...args) => toasts.push(args),
        back: async () => {}, language: async () => {}, display: async () => {},
        device: () => {},
      });
      const handle = await mount();
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
      const close = async (dialog, result) => {
        if (!dialog) return;
        await new Promise((resolve) => {
          dialog.addEventListener("close", resolve, { once: true });
          dialog.close(result);
        });
        await settle();
      };
      const canceledDialog = await open(missing.version);
      const canceled = {
        shown: Boolean(canceledDialog), text: canceledDialog?.textContent || "",
        cancelFocused: canceledDialog?.querySelector('[value="cancel"]') === document.activeElement,
      };
      await close(canceledDialog, "cancel");
      canceled.calls = removals().length;

      const acceptedDialog = await open(missing.version);
      const replacement = { ...missing, version: "acceleration-replacement" };
      catalog.unavailable = [replacement];
      await tick();
      const accepted = {
        shown: Boolean(acceptedDialog), textAfterPoll: acceptedDialog?.textContent || "",
        replacementShown: Boolean(card(replacement.version)),
      };
      await close(acceptedDialog, "delete");
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
      await close(healthyDialog, "cancel");
      healthy.calls = removals().length;

      const closedDialog = await open(replacement.version);
      const closed = { shown: Boolean(closedDialog) };
      await handle.close();
      await close(closedDialog, "delete");
      closed.calls = structuredClone(removals());
      const cleanup = {
        id, states, staleActive, busy, testing, canceled, accepted, healthy, closed,
        timersAfterClose: intervals.size, toasts: [...toasts],
        policyOrNetworkCalls: calls.filter((call) => [
          "get_network_policy", "check_acceleration_components", "download_acceleration_component",
          "verify_acceleration", "disable_acceleration",
        ].includes(call.command)),
      };

      calls.length = 0;
      catalog.state = "catalog_required";
      catalog.components = [];
      catalog.unavailable = [];
      catalog.preview_available = true;
      const previewHandle = await mount();
      const networkActions = () => calls.filter(({ command }) => [
        "check_acceleration_components", "download_acceleration_component", "verify_acceleration",
      ].includes(command));
      const button = (action) => root.querySelector('[data-component-action="' + action + '"]');
      await tick();
      const beforeCheck = { text: button("check-preview")?.textContent, actions: networkActions() };
      button("check-preview")?.click();
      await settle();
      const denied = networkActions();
      allowDownloads = true;
      root.querySelector('.settings-nav [data-section="models"]')?.click();
      await settle();
      button("check")?.click();
      await settle();
      button("check-preview")?.click();
      await settle();
      const checked = {
        actions: networkActions(),
        recommendation: cards()[0]?.querySelector(".badge.selected")?.textContent,
        text: cards()[0]?.textContent,
      };

      // A cache refresh swaps the rendered card while permission is pending.
      // The user action must still carry the original preview identity.
      let releasePolicy;
      policyWait = new Promise((resolve) => { releasePolicy = resolve; });
      button("download")?.click();
      await settle();
      const stableEntry = { ...previewEntry, version: "acceleration-stable-fixture", preview: false };
      catalog.preview_available = false;
      catalog.components = [stableEntry];
      await tick();
      const stableShown = !button("check-preview") && button("download")?.dataset.version;
      releasePolicy();
      await settle();
      button("download")?.click();
      await settle();
      const preview = { beforeCheck, denied, checked, stableShown, actions: networkActions() };

      const jobId = "12345678-1234-4234-8234-123456789abc";
      const activePreview = {
        ...previewEntry,
        downloadable: false,
        download: { job_id: jobId, state: "downloading", downloaded: 16, total: 1024, error: null },
      };
      catalog.components = [stableEntry, activePreview];
      await tick();
      const stableDisabled = button("download")?.disabled;
      const activeCancel = button("cancel");
      const oldRow = activeCancel?.closest("article");
      const progress = oldRow?.querySelector("progress")?.value;
      activeCancel?.click();
      await settle();
      catalog.components = [activePreview];
      await tick();
      const activeDownload = {
        stableDisabled, progress,
        oldVersion: oldRow?.textContent,
        offlineCancelVisible: Boolean(button("cancel")),
        cancels: calls.filter(({ command }) => command === "cancel_acceleration_download"),
      };
      await previewHandle.close();
      return { ...cleanup, preview, activeDownload };
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
    check(`${platform} active preview remains cancellable with its exact identity after refresh`, () => {
      const { stableDisabled, progress, oldVersion, offlineCancelVisible, cancels } = observed.activeDownload;
      assert.deepEqual(cancels, [{
        command: "cancel_acceleration_download",
        args: {
          id: "cuda-" + platform + "-x86_64",
          version: "acceleration-ci-40367160891",
          jobId: "12345678-1234-4234-8234-123456789abc",
        },
      }]);
      assert.equal(stableDisabled, true);
      assert.equal(progress, 16);
      assert.match(oldVersion, /acceleration-ci-40367160891/);
      assert.equal(offlineCancelVisible, true);
    });
    check(`${platform} preview is explicit and download keeps the rendered channel and version`, () => {
      const { beforeCheck, denied, checked, stableShown, actions } = observed.preview;
      assert.match(beforeCheck.text, /Check preview components/);
      assert.deepEqual(beforeCheck.actions, []);
      assert.deepEqual(denied, []);
      assert.deepEqual(checked.actions.map(({ command }) => command), [
        "check_acceleration_components", "check_acceleration_components",
      ]);
      assert.equal(checked.actions[0].args, undefined);
      assert.deepEqual(checked.actions[1].args, { preview: true });
      assert.equal(checked.recommendation, "Recommended to test");
      assert.match(checked.text, /Preview version.*acceleration-ci-40367160891/);
      assert.equal(stableShown, "acceleration-stable-fixture");
      const id = "cuda-" + platform + "-x86_64";
      assert.deepEqual(actions.slice(2), [
        { command: "download_acceleration_component", args: { id, version: "acceleration-ci-40367160891", preview: true } },
        { command: "download_acceleration_component", args: { id, version: "acceleration-stable-fixture", preview: false } },
      ]);
      assert.deepEqual(pageErrors, []);
    });
    await page.close();
  }
} finally {
  await browser.close();
}
assert.equal(failures.length, 0, failures.join("\n"));
