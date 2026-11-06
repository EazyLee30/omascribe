// Real settings handlers and DOM; IPC uses fictional local component data.
// No GPU, model, download, or native worker is started by this test.
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
  plugins: [
    {
      name: "mock-settings-ipc",
      setup(build) {
        build.onResolve(
          { filter: /^@tauri-apps\/api\/(core|event)$/ },
          (args) => ({
            path: args.path,
            namespace: "settings-ipc",
          }),
        );
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
    },
  ],
});
const browser = await chromium.launch({
  headless: true,
  ...(process.env.OMASCRIBE_BROWSER_EXE
    ? { executablePath: process.env.OMASCRIBE_BROWSER_EXE }
    : {}),
});
try {
  for (const platform of ["windows", "linux"]) {
    const page = await browser.newPage();
    const pageErrors = [];
    page.on("pageerror", (error) => pageErrors.push(error.message));
    await page.route("http://omascribe.test/**", (route) =>
      route.fulfill({
        contentType: "text/html",
        body: '<main id="settings"></main>',
      }),
    );
    await page.goto("http://omascribe.test/");
    await page.addScriptTag({ content: bundle.outputFiles[0].text });
    const observed = await page.evaluate(async (platform) => {
      let prefs = {
        appearance: {
          mode: "system",
          theme: "neutral",
          accent: "#d83b36",
          background: "",
          foreground: "",
          font: "system",
          font_size: 13,
        },
        asr_model: "fixture-asr",
        llm_model: "fixture-writer",
        threads: 2,
        context_size: 2048,
        inference_backend: "cpu",
        resource_mode: "balanced",
        memory_budget_mib: 0,
        llm_keep_alive_seconds: 0,
        auto_notes: true,
        translation_mode: "off",
        minutes_language: "auto",
        diarization: false,
        speaker_count: 0,
        use_knowledge: false,
        use_memory: false,
      };
      const hardware = {
        platform,
        arch: "x86_64",
        os: "Fixture OS",
        cpu: "Fixture CPU",
        cores: 2,
        total_memory_mib: 8192,
        available_memory_mib: 4096,
        free_disk_bytes: 8192,
        tier: "balanced",
        apple_silicon: false,
        backends: ["cpu"],
        resource_presets: [],
        gpus: {
          status: "detected",
          cuda_driver_version: 12080,
          devices: [
            {
              name: "Fixture GPU",
              vendor: "nvidia",
              dedicated_memory_bytes: 4096,
            },
          ],
        },
      };
      const catalog = {
        state: "ready",
        verification: null,
        components: [
          {
            component: {
              id: "fixture-cuda",
              name: "Fixture CUDA",
              backend: "cuda",
              archive: { bytes: 1024 },
              license: "fixture",
            },
            version: "acceleration-retained",
            present: true,
            installed: true,
            downloadable: false,
            compatible: true,
            download: null,
          },
        ],
      };
      const calls = [],
        toasts = [];
      let failSave = false,
        pendingSave = null,
        releaseSave = null;
      window.settingsInvoke = async (command, args) => {
        calls.push({
          command,
          args: structuredClone(args),
          backend: prefs.inference_backend,
        });
        switch (command) {
          case "get_preferences":
            return structuredClone(prefs);
          case "set_preferences":
            if (failSave) {
              failSave = false;
              throw new Error("fixture preference save failed");
            }
            if (pendingSave) await pendingSave;
            prefs = structuredClone(args.preferences);
            return structuredClone(prefs);
          case "get_network_policy":
            return { allow_model_downloads: true };
          case "diagnostics":
            return {
              version: "fixture",
              worker_ready: true,
              server_ready: true,
            };
          case "model_status":
            return [];
          case "hardware_profile":
            return structuredClone(hardware);
          case "acceleration_components":
            return structuredClone(catalog);
          case "verify_acceleration":
            if (prefs.inference_backend !== "auto")
              throw new Error(
                "Choose Automatic inference before testing optional acceleration",
              );
            return;
          default:
            throw new Error(`Unexpected fixture IPC: ${command}`);
        }
      };
      // Polling is tested separately; avoid clock-driven interference here.
      window.setInterval = () => 1;
      window.clearInterval = () => {};
      const settle = () => new Promise((resolve) => setTimeout(resolve, 0));
      const root = document.querySelector("#settings");
      const handle = await Settings.mountSettings(root, {
        section: "models",
        toast: (...args) => toasts.push(args),
        back: async () => {},
        language: async () => {},
        display: async () => {},
        device: () => {},
      });
      const testButton = () =>
        root.querySelector('[data-component-action="test"]');
      const autoButton = () =>
        root.querySelector('[data-component-action="use-auto"]');
      const verified = () =>
        calls.filter((c) => c.command === "verify_acceleration");
      const changeBackend = (value) => {
        const select = root.querySelector('[data-pref="inference_backend"]');
        select.value = value;
        select.dispatchEvent(new Event("change", { bubbles: true }));
      };
      const flush = async () => {
        await handle.flush();
        await settle();
      };
      const initial = {
        testDisabled: Boolean(testButton()?.disabled),
        switchOffered: Boolean(autoButton()),
        text: root.querySelector("#acceleration-components").textContent,
        saves: calls.filter((c) => c.command === "set_preferences").length,
      };
      testButton()?.click();
      await settle();
      const cpuClickTests = verified().length;
      autoButton()?.click();
      await flush();
      const switched = {
        backend: prefs.inference_backend,
        tests: verified().length,
        select: root.querySelector('[data-pref="inference_backend"]').value,
        testEnabled: Boolean(testButton() && !testButton().disabled),
        switchOffered: Boolean(autoButton()),
      };
      testButton()?.click();
      await settle();
      const explicitTests = structuredClone(verified());
      // A queued user switch to CPU must win over the still-rendered Auto button.
      pendingSave = new Promise((resolve) => {
        releaseSave = resolve;
      });
      changeBackend("cpu");
      await settle();
      testButton()?.click();
      releaseSave();
      pendingSave = null;
      await flush();
      const queuedCpu = {
        backend: prefs.inference_backend,
        tests: verified().length,
        testDisabled: Boolean(testButton()?.disabled),
      };
      failSave = true;
      autoButton()?.click();
      await flush();
      const failedSwitch = {
        backend: prefs.inference_backend,
        tests: verified().length,
        testDisabled: Boolean(testButton()?.disabled),
        switchOffered: Boolean(autoButton()),
      };
      // The normal preferences dropdown must update controls without a catalog change.
      changeBackend("auto");
      await flush();
      const dropdownAuto = {
        backend: prefs.inference_backend,
        testEnabled: Boolean(testButton() && !testButton().disabled),
        switchOffered: Boolean(autoButton()),
      };
      // A later dropdown choice must survive the pending button's DOM update.
      changeBackend("cpu");
      await flush();
      pendingSave = new Promise((resolve) => {
        releaseSave = resolve;
      });
      autoButton()?.click();
      await settle();
      changeBackend("cpu");
      releaseSave();
      pendingSave = null;
      await flush();
      const laterCpuChoice = {
        backend: prefs.inference_backend,
        select: root.querySelector('[data-pref="inference_backend"]').value,
        testDisabled: Boolean(testButton()?.disabled),
        tests: verified().length,
      };
      await handle.close();
      return {
        initial,
        cpuClickTests,
        switched,
        explicitTests,
        queuedCpu,
        failedSwitch,
        dropdownAuto,
        laterCpuChoice,
        toasts,
      };
    }, platform);
    console.log(JSON.stringify({ platform, observed }));
    assert.equal(
      observed.initial.testDisabled,
      true,
      `${platform}: explicit CPU must not offer a runnable GPU test`,
    );
    assert.equal(observed.initial.switchOffered, true);
    assert.match(observed.initial.text, /CPU/);
    assert.equal(
      observed.initial.saves,
      0,
      "opening settings must preserve the user's CPU choice",
    );
    assert.equal(observed.cpuClickTests, 0);
    assert.deepEqual(observed.switched, {
      backend: "auto",
      tests: 0,
      select: "auto",
      testEnabled: true,
      switchOffered: false,
    });
    assert.equal(observed.explicitTests.length, 1);
    assert.equal(observed.explicitTests[0].backend, "auto");
    assert.deepEqual(observed.explicitTests[0].args, {
      id: "fixture-cuda",
      version: "acceleration-retained",
    });
    assert.deepEqual(observed.queuedCpu, {
      backend: "cpu",
      tests: 1,
      testDisabled: true,
    });
    assert.deepEqual(observed.failedSwitch, {
      backend: "cpu",
      tests: 1,
      testDisabled: true,
      switchOffered: true,
    });
    assert.deepEqual(observed.dropdownAuto, {
      backend: "auto",
      testEnabled: true,
      switchOffered: false,
    });
    assert.deepEqual(
      observed.laterCpuChoice,
      {
        backend: "cpu",
        select: "cpu",
        testDisabled: true,
        tests: 1,
      },
      `${platform}: a later CPU choice must survive a pending Auto save`,
    );
    assert.ok(
      observed.toasts.some(([text]) =>
        text.includes("fixture preference save failed"),
      ),
    );
    assert.ok(
      !observed.toasts.some(([text]) =>
        /could not be fetched or installed|Choose Automatic inference/.test(
          text,
        ),
      ),
    );
    assert.deepEqual(pageErrors, []);
    console.log(
      `PASS ${platform}: explicit Auto consent, retained component test, queued CPU switch, failed save`,
    );
    await page.close();
  }
} finally {
  await browser.close();
}
