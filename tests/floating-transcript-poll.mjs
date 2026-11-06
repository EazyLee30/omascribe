// Mounted floating window with controlled IPC and clock. No native audio or model runs.
import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { build } from "esbuild";

const require = createRequire(import.meta.url);
const { chromium } = require(
  process.env.OMASCRIBE_PLAYWRIGHT_MODULE || "playwright",
);
const bundle = await build({
  entryPoints: ["src/transcript-window.ts"],
  bundle: true,
  format: "iife",
  loader: { ".css": "empty" },
  write: false,
  plugins: [
    {
      name: "floating-ipc",
      setup(build) {
        build.onResolve({ filter: /^@tauri-apps\/api\// }, ({ path }) => ({
          path,
          namespace: "floating-ipc",
        }));
        build.onLoad({ filter: /.*/, namespace: "floating-ipc" }, () => ({
          contents: `
        export const invoke = (...args) => window.floatingInvoke(...args);
        export const isTauri = () => true;
        export const listen = (...args) => window.floatingListen(...args);
        export const emit = (...args) => window.floatingEmit(...args);
        export const getCurrentWindow = () => window.floatingWindow;
        export class PhysicalPosition {}
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
  const page = await browser.newPage({ viewport: { width: 640, height: 500 } });
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await page.route("http://omascribe.test/**", (route) =>
    route.fulfill({
      contentType: "text/html",
      body: '<main id="app"></main><style>#float-transcript{height:300px;overflow:auto}.segment{min-height:80px}[hidden]{display:none}</style>',
    }),
  );
  await page.goto("http://omascribe.test/");
  await page.evaluate(() => {
    const runtime = (id = "a", partial = "Listening", changes = {}) => ({
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
      ...changes,
    });
    const full = (id = "a", revision = `rev-${id}`, changes = {}) => ({
      kind: "full",
      revision,
      snapshot: {
        meeting: {
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
          speaker_names: { speaker1: "Eazy" },
        },
        runtime: runtime(id),
        segments: Array.from({ length: id === "a" ? 5000 : 2 }, (_, i) => ({
          id: `segment-${id}-${i}`,
          meeting_id: id,
          run: 1,
          seq: i,
          text: `Saved passage ${id} ${i}`,
          start_ms: i * 1000,
          end_ms: (i + 1) * 1000,
          revision: 1,
          speaker: "speaker1",
          speaker_manual: false,
        })),
        translations: {},
        translation_mode: "off",
        ...changes,
      },
    });
    const delta = (duration_ms, runtime) => ({
      kind: "runtime",
      duration_ms,
      runtime,
    });
    const listeners = new Map(),
      timers = new Map(),
      calls = [];
    let nextReply,
      nextClip,
      active = "a";
    let preferences = {
      translation_mode: "off",
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
    let views = {
      transcript_mode: "floating",
      follow_recording: true,
      transcript_font: 14,
      compact: false,
      snap: "free",
      captions: { enabled: false },
    };
    window.setInterval = (callback, delay) => {
      timers.set(delay, callback);
      return delay;
    };
    window.clearInterval = (id) => timers.delete(id);
    window.floatingListen = async (name, callback) => {
      if (!listeners.has(name)) listeners.set(name, []);
      listeners.get(name).push(callback);
      return () => {};
    };
    window.floatingEmit = async (name, payload) => {
      for (const fn of listeners.get(name) || []) await fn({ payload });
    };
    window.floatingWindow = {
      toggleMaximize: async () => calls.push(["maximize"]),
      startResizeDragging: async (edge) => calls.push(["resize", edge]),
    };
    HTMLMediaElement.prototype.play = async function () {
      calls.push(["play", this.src]);
    };
    HTMLMediaElement.prototype.pause = function () {};
    window.floatingInvoke = async (command, args) => {
      calls.push([command, args]);
      switch (command) {
        case "get_preferences":
          return structuredClone(preferences);
        case "set_preferences":
          preferences = args.preferences;
          return structuredClone(preferences);
        case "diagnostics":
          return {};
        case "get_view_settings":
          return structuredClone(views);
        case "set_view_settings":
          views = args.preferences;
          await window.floatingEmit("view-settings", structuredClone(views));
          return structuredClone(views);
        case "poll_transcript": {
          const reply = nextReply;
          nextReply = null;
          return reply
            ? reply()
            : full(active, `rev-${active}`, {
                translation_mode: preferences.translation_mode,
              });
        }
        case "audio_clip": {
          const reply = nextClip;
          nextClip = null;
          return reply ? reply() : "UklGRg==";
        }
        case "start_recording":
        case "pause_recording":
        case "resume_recording":
        case "stop_recording":
        case "show_main_window":
          return null;
        default:
          throw new Error(`Unexpected IPC: ${command}`);
      }
    };
    let scans = 0;
    const stringify = JSON.stringify;
    JSON.stringify = (value, ...args) => {
      if (
        Array.isArray(value) &&
        Array.isArray(value[1]) &&
        value[1][0]?.id?.startsWith("segment-")
      )
        scans++;
      return stringify(value, ...args);
    };
    window.floatingTest = {
      runtime,
      full,
      delta,
      calls,
      timers,
      scans: () => scans,
      tick: (reply) => {
        nextReply = reply;
        return timers.get(700)();
      },
      next: (reply) => {
        nextReply = reply;
      },
      clip: (reply) => {
        nextClip = reply;
      },
      select: async (id) => {
        active = id;
        await window.floatingEmit("view-meeting");
      },
      view: async (patch) => {
        views = { ...views, ...patch };
        await window.floatingEmit("view-settings", structuredClone(views));
      },
    };
  });
  await page.addScriptTag({ content: bundle.outputFiles[0].text });
  await page.waitForFunction(() => window.floatingTest.timers.has(700));
  const actual = await page.evaluate(async () => {
    const {
      runtime,
      full,
      delta,
      calls,
      scans,
      tick,
      select,
      view,
      next,
      clip,
    } = window.floatingTest;
    const find = (s) => document.querySelector(s);
    const read = (s) => find(s)?.textContent;
    const settle = () => new Promise((resolve) => setTimeout(resolve, 0));
    const count = () =>
      calls.filter(([name]) => name === "poll_transcript").length;
    const action = async (name) => {
      find(`[data-action='${name}']`).click();
      await settle();
    };
    const pane = find("#float-transcript");
    const row = find(".segment"),
      partial = find(".partial");
    const initialScans = scans();
    const range = document.createRange();
    range.selectNodeContents(row.querySelector("p"));
    window.getSelection().addRange(range);
    pane.scrollTop = 1000;
    await tick(() =>
      delta(
        8700,
        runtime("a", "Updated live words", { recording: false, paused: true }),
      ),
    );
    const stable = {
      row: row === find(".segment"),
      partial: partial === find(".partial"),
      scans: scans() - initialScans,
      selection: window.getSelection().toString(),
      clock: read("#float-elapsed"),
      live: read(".partial p"),
      scroll: pane.scrollTop,
      status: find("#app").dataset.recording,
      resume: find("[data-action='pause']").title,
      savedCount: pane.querySelectorAll(".segment:not(.partial)").length,
    };
    await action("pause");
    await action("stop");
    pane.scrollTop = pane.scrollHeight;
    await tick(() => delta(9000, runtime("a", "More live words")));
    const follows = pane.scrollHeight - pane.scrollTop - pane.clientHeight < 2;
    await action("pause");
    const search = find("#float-search");
    const beforeSearch = count();
    search.value = "passage a 4999";
    search.dispatchEvent(new Event("input"));
    const searchResult = {
      calls: count() - beforeSearch,
      rows: pane.querySelectorAll(".segment").length,
      text: pane.textContent,
      preview: read("#float-preview"),
    };
    const searchedRow = find(".segment");
    const beforeDeltaScans = scans();
    await tick(() => delta(10000, runtime("a", "Even more live words")));
    const searchStable = {
      row: searchedRow === find(".segment"),
      scans: scans() - beforeDeltaScans,
      partial: !!find(".partial"),
    };
    search.value = "";
    search.dispatchEvent(new Event("input"));
    await tick(() => delta(11000, runtime("other", "Other meeting secret")));
    const foreign = {
      partial: !!find(".partial"),
      preview: read("#float-preview"),
      status: find("#app").dataset.recording,
    };
    await action("record");
    const corrected = full("b", null, {
      translation_mode: "bilingual",
      translations: {
        "segment-b-0": {
          text: "OmaScribe 正确译文",
          target: "zh",
          revision: 1,
        },
      },
    });
    corrected.snapshot.segments[0].text = "Corrected OmaScribe";
    await tick(() => corrected);
    const correctedText = pane.textContent;
    const firstTranslatedRow = find(".segment");
    await tick(() => corrected);
    const unchangedTranslation = firstTranslatedRow === find(".segment");
    const revised = structuredClone(corrected);
    revised.snapshot.translations["segment-b-0"].text = "修订后的译文";
    await tick(() => revised);
    const revisedText = pane.textContent;
    const revoked = structuredClone(revised);
    revoked.snapshot.translations = {};
    await tick(() => revoked);
    const revokedText = pane.textContent;
    await view({ compact: true, captions: { enabled: true } });
    const compact = {
      active: document.body.classList.contains("compact"),
      preview: read("#float-preview"),
      captions: find("[data-action='captions']").getAttribute("aria-pressed"),
    };
    await action("captions");
    await action("collapse");
    const beforeLocale = count();
    await window.floatingEmit("ui-locale", "zh-CN");
    const locale = {
      calls: count() - beforeLocale,
      label: find("[data-action='record']").title,
      speaker: read(".speaker-chip"),
    };
    await window.floatingEmit("ui-locale", "en");
    let finishOld;
    const stalePoll = tick(
      () =>
        new Promise((resolve) => {
          finishOld = resolve;
        }),
    );
    await select("c");
    await settle();
    finishOld(full("stale"));
    await stalePoll;
    await settle();
    const switched = {
      title: read("#float-title"),
      text: pane.textContent,
      error: find("#float-error").hidden,
    };
    let finishSlow;
    const slowPoll = tick(
      () =>
        new Promise((resolve) => {
          finishSlow = resolve;
        }),
    );
    const beforeRapid = count();
    for (let i = 0; i < 25; i++) await select(`rapid-${i}`);
    await settle();
    const rapidPending = count() - beforeRapid;
    const pendingTitle = read("#float-title");
    finishSlow(full("old-slow"));
    await slowPoll;
    await settle();
    const rapid = {
      pending: rapidPending,
      pendingTitle,
      newRequests: count() - beforeRapid,
      title: read("#float-title"),
    };
    let rejectOld;
    const staleError = tick(
      () =>
        new Promise((_, reject) => {
          rejectOld = reject;
        }),
    );
    await select("d");
    await settle();
    rejectOld("STALE ERROR");
    await staleError;
    await settle();
    const ignoredError = {
      title: read("#float-title"),
      hidden: find("#float-error").hidden,
    };
    let finishAudio;
    clip(
      () =>
        new Promise((resolve) => {
          finishAudio = resolve;
        }),
    );
    find(".timestamp").click();
    await select("e");
    await settle();
    finishAudio("UklGRg==");
    await settle();
    const staleAudio = {
      plays: calls.filter(([name]) => name === "play").length,
      hidden: find("#float-player").hidden,
    };
    find(".timestamp").click();
    await settle();
    const audio = {
      plays: calls.filter(([name]) => name === "play").length,
      hidden: find("#float-player").hidden,
    };
    await tick(() => null);
    const deleted = {
      title: read("#float-title"),
      clock: read("#float-elapsed"),
      count: pane.querySelectorAll(".segment").length,
      disabled: find("[data-action='record']").disabled,
      audioHidden: find("#float-player").hidden,
    };
    await tick();
    const recoveredRevision = calls
      .filter(([name]) => name === "poll_transcript")
      .at(-1)[1].revision;
    let finishFollow;
    const oldFollow = tick(
      () =>
        new Promise((resolve) => {
          finishFollow = resolve;
        }),
    );
    next(() => full("followed"));
    await view({ follow_recording: false });
    await settle();
    finishFollow(full("wrong-follow"));
    await oldFollow;
    await settle();
    const followedTitle = read("#float-title");
    let finishHidden;
    const oldVisible = tick(
      () =>
        new Promise((resolve) => {
          finishHidden = resolve;
        }),
    );
    await action("close");
    finishHidden(full("wrong-hidden"));
    await oldVisible;
    const beforeHidden = count();
    await tick();
    const hidden = {
      calls: count() - beforeHidden,
      rows: pane.querySelectorAll(".segment").length,
    };
    await view({ transcript_mode: "floating" });
    await settle();
    const mode = find("#float-translation-mode");
    mode.value = "en-zh";
    await mode.onchange();
    const modeChange = {
      mode: mode.value,
      revision: calls.filter(([name]) => name === "poll_transcript").at(-1)[1]
        .revision,
    };
    await action("return");
    await action("dock");
    const beforeDocked = count();
    await tick();
    const dockedCalls = count() - beforeDocked;
    return {
      stable,
      follows,
      searchResult,
      searchStable,
      foreign,
      correctedText,
      unchangedTranslation,
      revisedText,
      revokedText,
      compact,
      locale,
      switched,
      rapid,
      ignoredError,
      staleAudio,
      audio,
      deleted,
      recoveredRevision,
      followedTitle,
      hidden,
      modeChange,
      dockedCalls,
      calls,
    };
  });
  assert.deepEqual(actual.stable, {
    row: true,
    partial: true,
    scans: 0,
    selection: "Saved passage a 0",
    clock: "00:00:08",
    live: "Updated live words",
    scroll: 1000,
    status: "paused",
    resume: "Resume",
    savedCount: 5000,
  });
  assert.equal(actual.follows, true);
  assert.equal(actual.searchResult.calls, 0);
  assert.equal(actual.searchResult.rows, 1);
  assert.match(actual.searchResult.text, /Saved passage a 4999/);
  assert.equal(actual.searchResult.preview, "More live words");
  assert.deepEqual(actual.searchStable, {
    row: true,
    scans: 0,
    partial: false,
  });
  assert.deepEqual(actual.foreign, {
    partial: false,
    preview: "Saved passage a 4999",
    status: "ready",
  });
  assert.match(actual.correctedText, /Corrected OmaScribe/);
  assert.match(actual.correctedText, /OmaScribe 正确译文/);
  assert.equal(actual.unchangedTranslation, true);
  assert.match(actual.revisedText, /修订后的译文/);
  assert.doesNotMatch(actual.revokedText, /修订后的译文/);
  assert.match(actual.revokedText, /Corrected OmaScribe/);
  assert.deepEqual(actual.compact, {
    active: true,
    preview: "Listening",
    captions: "true",
  });
  assert.equal(actual.locale.calls, 0);
  assert.equal(actual.locale.label, "开始录音");
  assert.equal(actual.locale.speaker, "Eazy");
  assert.equal(actual.switched.title, "Meeting c");
  assert.doesNotMatch(actual.switched.text, /stale/);
  assert.equal(actual.switched.error, true);
  assert.deepEqual(actual.rapid, {
    pending: 0,
    pendingTitle: "OmaScribe",
    newRequests: 1,
    title: "Meeting rapid-24",
  });
  assert.deepEqual(actual.ignoredError, { title: "Meeting d", hidden: true });
  assert.deepEqual(actual.staleAudio, { plays: 0, hidden: true });
  assert.deepEqual(actual.audio, { plays: 1, hidden: false });
  assert.deepEqual(actual.deleted, {
    title: "OmaScribe",
    clock: "00:00:00",
    count: 0,
    disabled: true,
    audioHidden: true,
  });
  assert.equal(actual.recoveredRevision, null);
  assert.equal(actual.followedTitle, "Meeting followed");
  assert.deepEqual(actual.hidden, { calls: 0, rows: 0 });
  assert.deepEqual(actual.modeChange, { mode: "en-zh", revision: null });
  assert.equal(actual.dockedCalls, 0);
  assert(actual.calls.some(([name]) => name === "pause_recording"));
  assert(actual.calls.some(([name]) => name === "resume_recording"));
  assert(actual.calls.some(([name]) => name === "stop_recording"));
  assert(
    actual.calls.some(
      ([name, args]) => name === "start_recording" && args.id === "a",
    ),
  );
  assert.equal(
    actual.calls.filter(([name]) => name === "show_main_window").length,
    2,
  );
  const polls = actual.calls.filter(([name]) => name === "poll_transcript");
  assert.equal(polls[0][1].revision, null);
  assert.equal(polls[1][1].revision, "rev-a");
  assert.equal(polls[2][1].revision, "rev-a");
  assert.deepEqual(errors, []);
  console.log(
    "PASS floating transcript: 5,000 saved passages, zero history scans/replacements on runtime deltas, local search/locale, correction/translation refresh, controls, audio, stale-selection isolation and coalesced slow requests",
  );
} finally {
  await browser.close();
}
