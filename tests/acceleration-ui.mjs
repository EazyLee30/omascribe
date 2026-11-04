// Retained component controls in Chromium. Uses fictional catalog data only.
import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { build } from "esbuild";

const require = createRequire(import.meta.url);
const { chromium } = require(
  process.env.OMASCRIBE_PLAYWRIGHT_MODULE || "playwright",
);
const bundle = await build({
  entryPoints: ["src/acceleration-ui.ts"],
  bundle: true,
  format: "iife",
  globalName: "AccelerationUI",
  write: false,
});
const browser = await chromium.launch({
  headless: true,
  ...(process.env.OMASCRIBE_BROWSER_EXE
    ? { executablePath: process.env.OMASCRIBE_BROWSER_EXE }
    : {}),
});
try {
  const page = await browser.newPage();
  await page.route("http://omascribe.test/**", (route) =>
    route.fulfill({ contentType: "text/html", body: '<main id="catalog"></main>' }),
  );
  await page.goto("http://omascribe.test/");
  await page.addScriptTag({ content: bundle.outputFiles[0].text });
  const entry = {
    component: {
      id: "windows-cuda",
      name: "CUDA",
      backend: "cuda",
      archive: { bytes: 1024 },
      license: "fixture",
    },
    version: "acceleration-current",
    present: false,
    installed: false,
    downloadable: true,
    compatible: true,
    download: null,
  };
  const old = {
    ...entry,
    version: "acceleration-retained",
    present: true,
    installed: true,
    downloadable: false,
  };
  async function render(components, verification = null) {
    await page.evaluate(({ components, verification }) => {
      document.querySelector("#catalog").innerHTML = AccelerationUI.accelerationBody(
        {
          platform: "windows",
          gpus: { status: "detected", devices: [{ name: "Fixture GPU", vendor: "nvidia" }] },
        },
        { state: "ready", components, verification },
      );
    }, { components, verification });
  }
  await render([entry, old]);
  const current = page.locator("article").nth(0);
  const retained = page.locator("article").nth(1);
  assert.equal(await current.locator('[data-component-action="download"]').count(), 1);
  assert.equal(await retained.locator('[data-component-action="download"]').count(), 0);
  assert.equal(await retained.locator('[data-component-action="test"]').getAttribute("data-version"), old.version);
  assert.equal(await retained.locator('[data-component-action="remove"]').getAttribute("data-version"), old.version);

  await render([entry, old], {
    state: "active", component_id: old.component.id, version: old.version,
    reason: null, report: null,
  });
  assert.equal(await retained.locator('[data-component-action="disable"]').count(), 1);
  assert.equal(await current.locator('[data-component-action="disable"]').count(), 0);

  await render([{ ...old, compatible: false }]);
  assert.equal(await page.locator('[data-component-action="test"]').count(), 0);
  assert.equal(await page.locator('[data-component-action="remove"]').count(), 1);

  await render([{ ...old, installed: false }]);
  assert.equal(await page.locator('[data-component-action="test"]').count(), 0);
  assert.equal(await page.locator('[data-component-action="download"]').count(), 0);
  assert.equal(await page.locator('[data-component-action="remove"]').count(), 1);
  console.log("PASS current, retained, active, incompatible and incomplete component controls");
} finally {
  await browser.close();
}
