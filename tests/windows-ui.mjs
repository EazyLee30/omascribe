// Run against an isolated development app started with WebView2 CDP enabled.
// No microphone capture or real meeting data. Playwright may be supplied by
// OMASCRIBE_PLAYWRIGHT_MODULE when it is provided by the development host.
import { createRequire } from "node:module";
import { mkdir, writeFile } from "node:fs/promises";
import { resolve } from "node:path";
const require = createRequire(import.meta.url);
const { chromium } = require(process.env.OMASCRIBE_PLAYWRIGHT_MODULE || "playwright");
const output = resolve(process.env.OMASCRIBE_UI_OUTPUT || "artifacts/windows-ui");
await mkdir(output, { recursive: true });
const browser = await chromium.connectOverCDP(process.env.OMASCRIBE_CDP || "http://127.0.0.1:9223");
const checks = [];
const errors = [];
const assert = (value, message) => { if (!value) throw Error(message); };
const invoke = (page, command, args = {}) => page.evaluate(({ command, args }) => window.__TAURI_INTERNALS__.invoke(command, args), { command, args });
const until = async (test) => {
  for (let n = 0; n < 100; n++) {
    if (await test()) return;
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw Error("Timed out waiting for native UI");
};
try {
  const context = browser.contexts()[0];
  const main = context.pages().find((p) => !/companion|transcript|captions/.test(p.url()));
  assert(main, "Main WebView is missing");
  const preferences = await invoke(main, "get_view_settings");
  await invoke(main, "set_view_settings", { preferences: { ...preferences, companion_enabled: true, transcript_mode: "floating" } });
  await until(() => context.pages().some((p) => p.url().includes("companion.html")) && context.pages().some((p) => p.url().includes("transcript.html")));
  const companion = context.pages().find((p) => p.url().includes("companion.html"));
  const transcript = context.pages().find((p) => p.url().includes("transcript.html"));
  for (const page of [main, companion, transcript]) page.on("pageerror", (e) => errors.push(String(e)));
  await companion.locator("#pet").click();
  await until(() => companion.locator("#pet").getAttribute("aria-expanded").then((v) => v === "true"));
  await main.locator('[data-action="settings"]').click();
  await main.locator('[data-section="general"]').click();
  for (const [locale, start, caption] of [
    ["en", "Start recording", "Live captions"],
    ["zh-CN", "开始录音", "实时字幕"],
    ["zh-TW", "開始錄音", "即時字幕"],
    ["en", "Start recording", "Live captions"],
    ["zh-CN", "开始录音", "实时字幕"],
  ]) {
    await main.locator("#settings-language").selectOption(locale);
    await until(() => companion.locator("#pet-start").textContent().then((v) => v.includes(start)));
    assert(await companion.locator("#pet").getAttribute("aria-expanded") === "true", "Language switch collapsed the companion");
    assert(await transcript.locator('[data-action="captions"]').getAttribute("aria-label") === caption, "Floating transcript language is stale");
    assert(await companion.evaluate(() => document.documentElement.lang) === locale, "Companion document language is stale");
  }
  checks.push("Already-open companion and transcript follow EN / simplified / traditional language changes without losing expanded state");
  const hardware = await invoke(main, "hardware_profile");
  await main.locator('[data-section="models"]').click();
  await main.locator("#acceleration-components").waitFor();
  if (hardware.gpus.cuda_driver_version != null && hardware.gpus.cuda_driver_version < 12000)
    assert((await main.locator("#acceleration-components").textContent()).includes("驱动"), "Old CUDA driver has no actionable notice");
  assert((await invoke(main, "acceleration_components")).verification.state !== "active", "GPU became active without user testing");
  checks.push("Native hardware inventory reaches GPU options; detection alone never enables a component");
  await main.screenshot({ path: resolve(output, "hardware-zh.png") });
  await companion.screenshot({ path: resolve(output, "companion-zh.png") });
  await transcript.screenshot({ path: resolve(output, "transcript-zh.png") });
  assert(errors.length === 0, errors.join("\n"));
  await writeFile(resolve(output, "report.json"), JSON.stringify({ passed: true, checks, hardware, errors }, null, 2));
  console.log(JSON.stringify({ passed: true, checks }));
} catch (error) {
  await writeFile(resolve(output, "report.json"), JSON.stringify({ passed: false, checks, errors, error: String(error) }, null, 2));
  throw error;
} finally {
  // Disconnect CDP; the development app remains available for inspection.
  await browser.close();
}
