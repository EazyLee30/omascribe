// Real mounted meeting shell. IPC and interval time are controlled locally.
import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { build } from "esbuild";

const require = createRequire(import.meta.url);
const { chromium } = require(
  process.env.OMASCRIBE_PLAYWRIGHT_MODULE || "playwright",
);
const bundle = await build({
  entryPoints: ["src/main.ts"],
  bundle: true,
  format: "iife",
  loader: { ".css": "empty" },
  write: false,
  plugins: [
    {
      name: "meeting-ipc",
      setup(build) {
        build.onResolve({ filter: /^@tauri-apps\/api\// }, ({ path }) => ({
          path,
          namespace: "meeting-ipc",
        }));
        build.onLoad({ filter: /.*/, namespace: "meeting-ipc" }, () => ({
          contents: `
          export const invoke = (...args) => window.meetingInvoke(...args);
          export const isTauri = () => false;
          export const listen = async () => () => {};
          export const emit = async () => {};
          export const getCurrentWindow = () => ({});
          export class PhysicalPosition {}
          export class PhysicalSize {}
        `,
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
  const page = await browser.newPage({
    viewport: { width: 1280, height: 900 },
  });
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.route("http://omascribe.test/**", (route) =>
    route.fulfill({
      contentType: "text/html",
      body: '<main id="app"></main>',
    }),
  );
  await page.goto("http://omascribe.test/");
  await page.evaluate(() => {
    const runtime = (id, partial, extra = {}) => ({
      meeting_id: id,
      recording: true,
      paused: false,
      transcribing: false,
      elapsed_ms: 1000,
      processed_ms: 1000,
      level: 0.3,
      partial,
      asr_rtf: 0,
      notes_busy: false,
      notes_status: "",
      notes_elapsed_ms: 0,
      translation_busy: false,
      translation_status: "",
      diarization_busy: false,
      diarization_status: "",
      diarization_meeting_id: null,
      error: null,
      ...extra,
    });
    const meeting = (id) => ({
      id,
      title: `Meeting ${id}`,
      created_at: "2026-11-06T12:00:00Z",
      status: "ready",
      duration_ms: 1000,
      run: 1,
      notes_cursor: 1,
      auto_notes: false,
      diarization: false,
      diarization_cursor: 0,
      speaker_names: {},
    });
    const snapshot = (id) => ({
      meeting: meeting(id),
      runtime: runtime(id, "first partial"),
      notes: [
        {
          id: `note-${id}`,
          meeting_id: id,
          kind: "document",
          content: "My notes",
        },
      ],
      suggestions: [
        {
          id: `minutes-${id}`,
          text: "Minutes",
          status: "accepted",
          sources: [],
        },
      ],
      segments: [
        {
          id: `segment-${id}`,
          text: `Saved passage ${id}`,
          start_ms: 0,
          end_ms: 1000,
          revision: 1,
        },
      ],
      translations: {},
      translation_mode: "off",
    });
    const full = (id, revision, changes = {}) => ({
      kind: "full",
      revision,
      snapshot: { ...snapshot(id), ...changes },
    });
    const delta = (duration_ms, runtime) => ({
      kind: "runtime",
      duration_ms,
      runtime,
    });
    const calls = [];
    const timers = new Map();
    let nextReply;
    window.setInterval = (callback, delay) => {
      timers.set(delay, callback);
      return delay;
    };
    window.clearInterval = (id) => timers.delete(id);
    window.meetingInvoke = async (command, args) => {
      switch (command) {
        case "get_preferences":
          return {
            appearance: {
              mode: "light",
              theme: "neutral",
              accent: "#d83b36",
              background: "",
              foreground: "",
              font: "system",
              font_size: 13,
            },
          };
        case "get_view_settings":
        case "set_view_settings":
          return (
            args?.preferences || {
              transcript_mode: "docked",
              sidebar_collapsed: false,
              notebook_collapsed: false,
              minutes_collapsed: false,
              dock_width: 360,
              transcript_font: 14,
              captions: { enabled: false },
            }
          );
        case "list_meetings":
          return [meeting("a"), meeting("b")];
        case "snapshot":
          return snapshot(args.id);
        case "poll_snapshot": {
          calls.push(args);
          const reply = nextReply;
          nextReply = null;
          return reply ? reply() : full(args.id, `rev-${args.id}`);
        }
        case "diagnostics":
          return {};
        case "set_view_meeting":
        case "take_pending_profile":
        case "update_meeting":
          return null;
        case "extract_focus_terms":
          return [];
        default:
          throw new Error(`Unexpected IPC: ${command}`);
      }
    };
    let scans = 0;
    const stringify = JSON.stringify;
    JSON.stringify = (value, ...args) => {
      if (
        Array.isArray(value) &&
        (value[0]?.id?.startsWith("segment-") ||
          value[1]?.[0]?.id?.startsWith("minutes-"))
      )
        scans++;
      return stringify(value, ...args);
    };
    window.meetingTest = {
      runtime,
      full,
      delta,
      calls,
      timers,
      scans: () => scans,
      tick: (reply) => {
        nextReply = reply;
        return timers.get(600)();
      },
    };
  });
  await page.addScriptTag({ content: bundle.outputFiles[0].text });
  await page.waitForFunction(() => window.meetingTest.timers.has(600));
  const actual = await page.evaluate(async () => {
    const { runtime, full, delta, calls, scans, tick } = window.meetingTest;
    const find = (selector) => document.querySelector(selector);
    const read = (selector) => find(selector)?.textContent;
    const settle = () => new Promise((resolve) => setTimeout(resolve, 0));
    const originalScans = scans();
    const firstRow = find("#transcript .segment");
    const firstNotes = find("#user-notes .tiptap");
    await tick(() =>
      delta(
        8700,
        runtime("a", "first partial", {
          recording: false,
          paused: true,
          notes_busy: true,
          error: "Runtime banner",
        }),
      ),
    );
    const stable = {
      clock: read("#duration"),
      banner: read("#error-banner"),
      pauseHidden: find("#pause-recording").hidden,
      endHidden: find("#end-recording").hidden,
      cancel: find("#generate-notes").title,
      editor: !!firstNotes && firstNotes === find("#user-notes .tiptap"),
      row: !!firstRow && firstRow === find("#transcript .segment"),
      scans: scans() - originalScans,
    };
    const beforeRepeat = scans();
    await tick(() => full("a", "rev-a"));
    const repeatScans = scans() - beforeRepeat;
    await tick(() => delta(9300, runtime("a", "changed partial")));
    const partial = read("#transcript .partial p");
    const translations = (text) => ({
      translation_mode: "bilingual",
      translations: { "segment-a": { text, target: "zh", revision: 1 } },
    });
    await tick(() => full("a", null, translations("第一段译文")));
    const firstTranslation = read("#transcript");
    await tick(() => full("a", null, translations("更新的译文")));
    const nextTranslation = read("#transcript");
    await tick(() => full("a", "rev-a2"));

    let resolveOld;
    const old = tick(
      () =>
        new Promise((resolve) => {
          resolveOld = resolve;
        }),
    );
    find('#meetings [data-meeting="b"]').click();
    await settle();
    const switched = find("#meeting-title").value;
    resolveOld(delta(99000, runtime("a", "STALE")));
    await old;
    return {
      calls,
      stable,
      repeatScans,
      partial,
      firstTranslation,
      nextTranslation,
      switched,
      clock: read("#duration"),
      transcript: read("#transcript"),
      toast: read("#toast"),
    };
  });
  assert.deepEqual(actual.calls, [
    { id: "a", revision: null },
    { id: "a", revision: "rev-a" },
    { id: "a", revision: "rev-a" },
    { id: "a", revision: "rev-a" },
    { id: "a", revision: "rev-a" },
    { id: "a", revision: null },
    { id: "a", revision: null },
    { id: "a", revision: "rev-a2" },
    { id: "b", revision: null },
  ]);
  assert.deepEqual(actual.stable, {
    clock: "00:00:08",
    banner: "Runtime banner",
    pauseHidden: true,
    endHidden: false,
    cancel: "Cancel",
    editor: true,
    row: true,
    scans: 0,
  });
  assert.equal(actual.repeatScans, 0);
  assert.equal(actual.partial, "changed partial");
  assert.match(actual.firstTranslation, /第一段译文/);
  assert.match(actual.nextTranslation, /更新的译文/);
  assert.equal(actual.switched, "Meeting b");
  assert.equal(actual.clock, "00:00:01");
  assert.match(actual.transcript, /Saved passage b/);
  assert.doesNotMatch(actual.transcript, /STALE/);
  assert.equal(actual.toast, "");
  assert.deepEqual(errors, []);
  console.log(
    "PASS main meeting runtime polling, caption fallback and stale response isolation",
  );
} finally {
  await browser.close();
}
