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
    route.fulfill({
      contentType: "text/html",
      body: '<main id="catalog"></main>',
    }),
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
  async function render(
    components,
    verification = null,
    hardware = {
      platform: "windows",
      gpus: {
        status: "detected",
        devices: [{ name: "Fixture GPU", vendor: "nvidia" }],
      },
    },
  ) {
    await page.evaluate(
      ({ components, verification, hardware }) => {
        document.querySelector("#catalog").innerHTML =
          AccelerationUI.accelerationBody(hardware, {
            state: "ready",
            components,
            verification,
          });
      },
      { components, verification, hardware },
    );
  }
  await render([entry, old]);
  const current = page.locator("article").nth(0);
  const retained = page.locator("article").nth(1);
  assert.equal(
    await current.locator('[data-component-action="download"]').count(),
    1,
  );
  assert.equal(
    await retained.locator('[data-component-action="download"]').count(),
    0,
  );
  assert.equal(
    await retained
      .locator('[data-component-action="test"]')
      .getAttribute("data-version"),
    old.version,
  );
  assert.equal(
    await retained
      .locator('[data-component-action="remove"]')
      .getAttribute("data-version"),
    old.version,
  );

  await render([entry, old], {
    state: "active",
    component_id: old.component.id,
    version: old.version,
    reason: null,
    report: null,
  });
  assert.equal(
    await retained.locator('[data-component-action="disable"]').count(),
    1,
  );
  assert.equal(
    await current.locator('[data-component-action="disable"]').count(),
    0,
  );

  await render([{ ...old, compatible: false }]);
  assert.equal(await page.locator('[data-component-action="test"]').count(), 0);
  assert.equal(
    await page.locator('[data-component-action="remove"]').count(),
    1,
  );

  await render([{ ...old, installed: false }]);
  assert.equal(await page.locator('[data-component-action="test"]').count(), 0);
  assert.equal(
    await page.locator('[data-component-action="download"]').count(),
    0,
  );
  assert.equal(
    await page.locator('[data-component-action="remove"]').count(),
    1,
  );
  const vulkan = {
    ...entry,
    component: {
      ...entry.component,
      id: "fixture-vulkan",
      name: "Vulkan",
      backend: "vulkan",
    },
  };
  for (const platform of ["linux", "windows"]) {
    for (const version of [
      null,
      undefined,
      0,
      -1,
      11080,
      12000,
      12080,
      13000,
    ]) {
      const hardware = {
        platform,
        gpus: {
          status: "detected",
          cuda_driver_version: version,
          devices: [{ name: "Fixture NVIDIA", vendor: "nvidia" }],
        },
      };
      const oldDriver = version > 0 && version < 12000;
      const supported = version >= 12000;
      await render([entry, vulkan], null, hardware);
      const cudaCard = page.locator("article").nth(0);
      const vulkanCard = page.locator("article").nth(1);
      assert.equal(
        await cudaCard.locator(".badge.selected").count(),
        supported ? 1 : 0,
        `${platform} CUDA ${String(version)} recommendation`,
      );
      assert.equal(
        await vulkanCard.locator(".badge.selected").count(),
        oldDriver ? 1 : 0,
      );
      assert.equal(
        await cudaCard
          .locator('[data-component-action="download"]')
          .isDisabled(),
        oldDriver,
      );
      if (!supported && !oldDriver) {
        assert.match(
          await page.locator("#catalog").innerText(),
          /CUDA.*(?:未确认|未確認|unconfirmed)/,
        );
      }
      await render(
        [{ ...entry, present: true, installed: true }],
        null,
        hardware,
      );
      assert.equal(
        await page.locator('[data-component-action="test"]').isDisabled(),
        oldDriver,
      );
    }
  }
  for (const vendor of ["amd", "intel"]) {
    await render([entry, vulkan], null, {
      platform: "linux",
      gpus: { status: "detected", devices: [{ name: "Fixture GPU", vendor }] },
    });
    assert.equal(
      await page.locator("article").nth(0).locator(".badge.selected").count(),
      0,
    );
    assert.equal(
      await page
        .locator("article")
        .nth(0)
        .locator('[data-component-action="download"]')
        .isDisabled(),
      true,
    );
    assert.equal(
      await page.locator("article").nth(1).locator(".badge.selected").count(),
      1,
    );
  }
  console.log(
    "PASS retained controls and Linux/Windows CUDA supported/old/unknown recommendations and actions",
  );
} finally {
  await browser.close();
}
