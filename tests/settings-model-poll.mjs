// Real mountSettings lifecycle in Chromium; only IPC and interval time are mocked.
// Uses the same existing esbuild/Playwright dependencies as acceleration-ui.mjs.
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
    name: "mock-settings-ipc",
    setup(build) {
      build.onResolve({ filter: /^@tauri-apps\/api\/(core|event)$/ }, (args) => ({
        path: args.path,
        namespace: "settings-ipc",
      }));
      build.onLoad({ filter: /.*/, namespace: "settings-ipc" }, () => ({
        contents: `
          export const invoke = (command, args) => window.settingsInvoke(command, args);
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
  for (const platform of ["macos", "windows", "linux"]) {
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
        asr_model: "fixture-asr", llm_model: "qwen3-fixture", threads: 2,
        context_size: 2048, inference_backend: "auto", resource_mode: "balanced",
        memory_budget_mib: 0, llm_keep_alive_seconds: 0, auto_notes: true,
        translation_mode: "off", minutes_language: "auto", diarization: false,
        speaker_count: 0, use_knowledge: false, use_memory: false,
      };
      const model = {
        model: {
          id: "qwen3-fixture", name: "Fixture writer", kind: "llm", format: "gguf",
          license: "fixture", source: "fixture", memory_mib: 1024,
          bundled: false, files: [{ bytes: 1024 }], archive: null,
        },
        installed: false, installed_bytes: 0, managed: false,
        selected: true, download: null,
      };
      const hardware = {
        platform, arch: "fixture", os: "Fixture OS", cpu: "Fixture CPU", cores: 2,
        total_memory_mib: 8192, available_memory_mib: 4096, free_disk_bytes: 8192,
        tier: "balanced", apple_silicon: platform === "macos", backends: ["cpu"],
        resource_presets: [],
        gpus: {
          status: "detected", cuda_driver_version: 12080,
          devices: [{ name: "Fixture GPU", vendor: "nvidia", dedicated_memory_bytes: 4096 }],
        },
      };
      const catalog = {
        state: "ready", verification: null,
        components: [{
          component: {
            id: "fixture-cuda", name: "Fixture CUDA", backend: "cuda",
            archive: { bytes: 1024 }, license: "fixture",
          },
          version: "acceleration-fixture", present: false, installed: false,
          downloadable: true, compatible: true, download: null,
        }],
      };
      window.settingsInvoke = async (command, args) => {
        calls.push({ command, args });
        switch (command) {
          case "get_preferences": return structuredClone(prefs);
          case "get_network_policy": return { allow_model_downloads: true };
          case "diagnostics": return { version: "fixture", worker_ready: true, server_ready: true };
          case "model_status": return structuredClone([model]);
          case "hardware_profile": return structuredClone(hardware);
          case "acceleration_components": return structuredClone(catalog);
          case "download_model":
            model.download = { state: "downloading", downloaded: 256, total: 1024, error: null };
            return;
          default: throw new Error(`Unexpected fixture IPC: ${command}`);
        }
      };
      // Drive the callbacks registered by mountSettings, without real one-second waits.
      const intervals = new Map();
      let nextTimer = 0;
      window.setInterval = (callback) => {
        const id = ++nextTimer;
        intervals.set(id, callback);
        return id;
      };
      window.clearInterval = (id) => intervals.delete(id);
      const settle = () => new Promise((resolve) => setTimeout(resolve, 0));
      const tick = async (count = 1) => {
        for (let i = 0; i < count; i++) {
          for (const callback of [...intervals.values()]) callback();
          await settle();
        }
      };
      const root = document.querySelector("#settings");
      const handle = await Settings.mountSettings(root, {
        section: "models", toast: (...args) => toasts.push(args),
        back: async () => {}, language: async () => {}, display: async () => {},
        device: () => {},
      });
      const counts = () => ({
        model: calls.filter((call) => call.command === "model_status").length,
        components: calls.filter((call) => call.command === "acceleration_components").length,
      });
      const initial = counts();
      await tick(5);
      const idle = counts();
      const card = () => root.querySelector('[data-model="qwen3-fixture"]');
      const clickDownload = async () => {
        card().querySelector('[data-model-action="download_model"]').click();
        await settle();
      };
      await clickDownload();
      const downloading = Boolean(card().querySelector("progress"));
      model.download.state = "verifying";
      await tick();
      const verifying = card().textContent.includes("Verifying files");
      model.download = { ...model.download, state: "failed", error: "fixture interruption" };
      await tick();
      const failed = Boolean(card().querySelector(".inline-error"))
        && Boolean(card().querySelector('[data-model-action="download_model"]'));
      await clickDownload();
      model.download.state = "verifying";
      await tick();
      model.installed = true;
      model.managed = true;
      model.installed_bytes = 1024;
      model.download = { ...model.download, state: "installed", downloaded: 1024 };
      await tick();
      const installed = card().textContent.includes("Installed")
        && !card().querySelector("progress")
        && !card().querySelector(".inline-error")
        && !card().querySelector('[data-model-action="download_model"]');
      const terminal = counts();
      root.querySelector('[data-section="appearance"]').click();
      await settle();
      await tick(3);
      const away = counts();
      root.querySelector('[data-section="models"]').click();
      await settle();
      const reentered = counts();
      const componentCards = root.querySelectorAll("#acceleration-components article").length;
      await handle.close();
      await tick(3);
      return {
        initial, idle, downloading, verifying, failed, installed, terminal,
        away, reentered, closed: counts(), timersAfterClose: intervals.size,
        componentCards, toasts,
        downloads: calls.filter((call) => call.command === "download_model").map((call) => call.args.id),
      };
    }, platform);
    console.log(JSON.stringify({ platform, observed }));
    check(`${platform} model polling and download terminal DOM`, () => {
      assert.equal(observed.initial.model, 1);
      assert.equal(observed.idle.model, 6);
      assert.equal(observed.terminal.model, 12);
      assert.equal(observed.downloading, true);
      assert.equal(observed.verifying, true);
      assert.equal(observed.failed, true);
      assert.equal(observed.installed, true);
      assert.deepEqual(observed.downloads, ["qwen3-fixture", "qwen3-fixture"]);
      assert.deepEqual(observed.toasts, []);
      assert.deepEqual(pageErrors, []);
    });
    check(`${platform} section and close lifecycle`, () => {
      assert.deepEqual(observed.away, observed.terminal);
      assert.equal(observed.reentered.model, observed.terminal.model + 1);
      assert.deepEqual(observed.closed, observed.reentered);
      assert.equal(observed.timersAfterClose, 0);
    });
    check(`${platform} optional component queries`, () => {
      const supported = platform !== "macos";
      assert.equal(observed.initial.components, supported ? 1 : 0, "initial Models entry");
      assert.equal(observed.idle.components, supported ? 6 : 0, "five idle timer callbacks");
      assert.equal(observed.terminal.components, supported ? observed.terminal.model : 0, "download actions and terminal states");
      assert.equal(observed.reentered.components, supported ? observed.reentered.model : 0, "Models reentry");
      assert.equal(observed.componentCards, supported ? 1 : 0);
    });
    await page.close();
  }
} finally {
  await browser.close();
}
assert.equal(failures.length, 0, failures.join("\n"));
