// Component regression in a real Chromium browser. No app data or microphone.
// Set OMASCRIBE_PLAYWRIGHT_MODULE to the local Playwright package if needed.
import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { build } from "esbuild";

const require = createRequire(import.meta.url);
const { chromium } = require(
  process.env.OMASCRIBE_PLAYWRIGHT_MODULE || "playwright",
);
const bundle = await build({
  entryPoints: ["src/note-candidates.ts"],
  bundle: true,
  format: "iife",
  globalName: "NoteCandidates",
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
  await page.setContent(
    '<textarea id="edit"></textarea><div id="terms"></div>',
  );
  await page.addScriptTag({ content: bundle.outputFiles[0].text });
  const candidates = await page.evaluate(() =>
    NoteCandidates.noteCandidates(
      "OmaScribe\n- Alice\nNVIDIA\nGTX 1650\nALICE\nOne long sentence.\n<img src=x>",
      ["NVIDIA", "RTX 1660"],
    ),
  );
  assert.deepEqual(candidates, [
    "OmaScribe",
    "Alice",
    "NVIDIA",
    "GTX 1650",
    "<img src=x>",
    "RTX 1660",
  ]);
  await page.evaluate((values) => {
    const input = document.querySelector("#edit");
    input.value = "before wrong after";
    input.focus();
    input.setSelectionRange(7, 12);
    window.changes = 0;
    input.addEventListener("input", () => window.changes++);
    NoteCandidates.renderNoteCandidates(
      document.querySelector("#terms"),
      input,
      values,
      "Choose a word",
    );
  }, candidates);
  assert.equal(
    await page.locator("#terms img").count(),
    0,
    "note text must be inert",
  );
  await page.getByRole("button", { name: "NVIDIA", exact: true }).click();
  assert.equal(await page.locator("#edit").inputValue(), "before NVIDIA after");
  assert.equal(await page.evaluate(() => window.changes), 1);
  await page.evaluate(() => {
    const input = document.querySelector("#edit");
    input.setSelectionRange(input.value.length, input.value.length);
    document.querySelector("#terms button").focus();
  });
  await page.keyboard.press("Enter");
  assert.equal(
    await page.locator("#edit").inputValue(),
    "before NVIDIA afterOmaScribe",
  );
  assert.equal(await page.evaluate(() => document.activeElement.id), "edit");
  await page.evaluate(() =>
    NoteCandidates.renderNoteCandidates(
      document.querySelector("#terms"),
      document.querySelector("#edit"),
      [],
      "",
    ),
  );
  assert.equal(await page.locator("#terms").isHidden(), true);
  console.log(
    "PASS note candidates: inert text, deduplication, selection replacement, keyboard insertion, empty state",
  );
} finally {
  await browser.close();
}
