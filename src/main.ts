import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { t, locale, setLocale, type Key, type Locale } from "./i18n";
import type {
  Meeting,
  Snapshot,
  Note,
  Diagnostic,
  MeetingFiles,
} from "./types";
import "./style.css";
import "./workspace.css";
import { mountSettings } from "./settings";
import { mountLibrary, type LibraryPage } from "./library";
import {
  initAppearance,
  getPreferences,
  type Profile,
} from "./preferences-client";
import { tr, glyph, navItems, type PageHandle } from "./workspace-ui";
import { speakerDialog, speakerName } from "./speaker-ui";
import { NotebookEditor } from "./editor";
import {
  getViewSettings,
  setViewSettings,
  patchViewSettings,
  displaySettings,
  type ViewPreferences,
} from "./views-client";

const $ = <T extends HTMLElement = HTMLElement>(q: string) =>
  document.querySelector<T>(q)!;
const esc = (s: unknown) =>
  String(s ?? "").replace(
    /[&<>"']/g,
    (c) =>
      ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[
        c
      ]!,
  );
const clock = (ms: number) => {
  const s = Math.floor(ms / 1000);
  return `${Math.floor(s / 3600)
    .toString()
    .padStart(2, "0")}:${Math.floor((s / 60) % 60)
    .toString()
    .padStart(2, "0")}:${(s % 60).toString().padStart(2, "0")}`;
};
const icons: Record<string, string> = {
  panelLeft:
    "M5 4h14a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2ZM9 4v16",
  panelRight:
    "M5 4h14a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2ZM15 4v16",
  library: "M4 4h4v16H4V4ZM10 4h4v16h-4V4Zm6 1 4-1 3 15-4 1-3-15Z",
  folder: "M3 7V4h6l3 3h9v13H3V7Z",
  plus: "M12 5v14M5 12h14",
  mic: "M9 6a3 3 0 0 1 6 0v6a3 3 0 0 1-6 0V6ZM5 11v1a7 7 0 0 0 14 0v-1M12 19v3M8 22h8",
  pause: "M8 5v14M16 5v14",
  stop: "M6 6h12v12H6Z",
  upload: "M12 16V3M7 8l5-5 5 5M4 16v4h16v-4",
  download: "M12 3v13M7 11l5 5 5-5M4 17v4h16v-4",
  settings:
    "M12 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8ZM12 2v3M12 19v3M2 12h3M19 12h3M5 5l2 2M17 17l2 2M5 19l2-2M17 7l2-2",
  play: "m8 5 11 7-11 7Z",
  check: "m5 12 4 4L19 6",
  arrow: "M5 12h14m-6-6 6 6-6 6",
  close: "m6 6 12 12M6 18 18 6",
  note: "M5 3h14v14l-4 4H5V3ZM9 8h6M9 12h6M14 17v4",
  search: "M10 4a6 6 0 1 0 0 12 6 6 0 0 0 0-12Zm4 10 6 6",
  edit: "m14 4 6 6M4 20l5-1L20 8l-4-4L5 15l-1 5Z",
  refresh:
    "M20 7v5h-5M4 17v-5h5M5 8a8 8 0 0 1 13-3l2 3M4 16l2 3a8 8 0 0 0 13-3",
  shield: "m12 3 8 3v6c0 5-8 9-8 9s-8-4-8-9V6l8-3ZM8 12l3 3 5-6",
};
const icon = (name: string) =>
  `<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="${icons[name] || icons.note}"/></svg>`;
const brand = `<svg viewBox="0 0 256 256" fill="none" aria-hidden="true"><path d="M55 62c0-11 9-20 20-20h106v157H75c-11 0-20 7-20 17V62Z" fill="currentColor" opacity=".16"/><path d="M181 42H77c-12 0-22 10-22 22v137c0-12 10-22 22-22h104V42ZM77 179v35h104" stroke="currentColor" stroke-width="10" stroke-linejoin="round"/><path d="M94 99v32m26-51v70m26-59v49" stroke="currentColor" stroke-width="11" stroke-linecap="round"/><path d="m165 150 24-24 15 15-24 24-20 5 5-20Z" fill="currentColor" stroke="#191b17" stroke-width="5" stroke-linejoin="round"/></svg>`;
let page = "meeting";
let pageHandle: PageHandle | null = null;
let navigation = Promise.resolve();
let notebookEditor: NotebookEditor | null = null;
let viewSettings: ViewPreferences | null = null;
let current: string | null = null,
  snap: Snapshot | null = null,
  meetings: Meeting[] = [],
  diag: Diagnostic | null = null;
let transcriptSignature = "",
  noteSignature = "",
  suggestionSignature = "",
  busy = false,
  chosenDevice = localStorage.getItem("omascribe-device") || "auto";
const timers = new Map<string, ReturnType<typeof setTimeout>>(),
  queues = new Map<string, Promise<unknown>>();
const drafts = new Map<string, { note: Note; content: string; kind: string }>();
let toastTimer: ReturnType<typeof setTimeout>,
  audioUrl: string | null = null;
function toast(message: string, error = false) {
  if (!document.getElementById("toast")) {
    const el = document.createElement("div");
    el.id = "toast";
    el.setAttribute("role", "alert");
    document.body.append(el);
  }
  $("#toast").textContent = message;
  $("#toast").className = `toast visible ${error ? "error" : ""}`;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => ($("#toast").className = "toast"), 7000);
}
async function guard(action: () => Promise<unknown>) {
  try {
    await action();
  } catch (e) {
    toast(String(e), true);
  }
}
type Panel = "sidebar" | "notebook" | "minutes" | "transcript";
const panelLabels: Record<Panel, Key> = {
  sidebar: "library",
  notebook: "yourNotes",
  minutes: "assistant",
  transcript: "transcript",
};
function panelButton(panel: Panel, expanded: boolean) {
  const label = `${t(expanded ? "collapse" : "expand")} · ${t(panelLabels[panel])}`;
  return `<button class="subtle square panel-toggle" data-panel="${panel}" title="${label}" aria-label="${label}" aria-expanded="${expanded}" aria-controls="${panel}-content">${icon(panel === "sidebar" || panel === "notebook" ? "panelLeft" : "panelRight")}</button>`;
}
function panelRail(panel: Panel) {
  if (panel === "sidebar") {
    return `<div class="panel-rail sidebar-rail" data-rail="sidebar" hidden>${panelButton(panel, false)}<nav class="rail-navigation" aria-label="${t("library")}"><button class="rail-action rail-primary" data-action="new" title="${t("newMeeting")}" aria-label="${t("newMeeting")}">${icon("plus")}</button><button class="rail-action" data-action="import" title="${t("import")}" aria-label="${t("import")}">${icon("upload")}</button>${navItems()
      .map(
        ([id, label]) =>
          `<button class="rail-action" data-page="${id}" title="${label}" aria-label="${label}">${glyph(id)}</button>`,
      )
      .join(
        "",
      )}</nav><div class="rail-bottom"><button class="rail-action" data-page="settings" title="${t("settings")}" aria-label="${t("settings")}">${icon("settings")}</button></div></div>`;
  }
  return `<div class="panel-rail" data-rail="${panel}" hidden>${panelButton(panel, false)}<span aria-hidden="true">${t(panelLabels[panel])}</span></div>`;
}
let panelUpdates = Promise.resolve();
function togglePanel(panel: Panel) {
  const update = panelUpdates.then(async () => {
    const prefs = await getViewSettings();
    if (panel === "transcript") {
      prefs.transcript_mode =
        prefs.transcript_mode === "docked" ? "hidden" : "docked";
    } else {
      const key = `${panel}_collapsed` as const;
      prefs[key] = !prefs[key];
    }
    viewSettings = await setViewSettings(prefs);
    applyViews();
    const expanded =
      panel === "transcript"
        ? prefs.transcript_mode === "docked"
        : !prefs[`${panel}_collapsed`];
    document
      .querySelector<HTMLButtonElement>(
        `[data-panel="${panel}"][aria-expanded="${expanded}"]`,
      )
      ?.focus();
  });
  panelUpdates = update.catch(() => {});
  return update;
}
function shell() {
  $("#app").innerHTML =
    `<aside class="sidebar">${panelRail("sidebar")}<div id="sidebar-content"><div class="sidebar-toggle">${panelButton("sidebar", true)}</div><div class="brand"><span class="brand-icon">${brand}</span><div>OmaScribe<small>OFFLINE NOTEBOOK</small></div></div><button class="primary new-meeting" data-action="new">${icon("plus")}${t("newMeeting")}</button><button class="subtle import" data-action="import">${icon("upload")}${t("import")}</button><nav class="workspace-navigation">${navItems()
      .map(
        ([id, label]) =>
          `<button class="subtle" data-page="${id}">${glyph(id)}${label}</button>`,
      )
      .join(
        "",
      )}</nav><div class="library-label">${t("library")}<span id="meeting-count">0</span></div><nav id="meetings" aria-label="${t("library")}"></nav><div class="sidebar-bottom"><div class="local-note">${icon("shield")}<span>${t("local")}<small>${t("private")}</small></span></div><button class="subtle" data-action="settings">${icon("settings")}${t("settings")}</button></div></div></aside><main id="workspace"><section class="welcome"><div class="welcome-mark">${brand}</div><p class="eyebrow">OMASCRIBE</p><h1>${t("welcome")}</h1><p>${t("welcomeSub")}</p><button class="primary" data-action="new">${t("begin")}${icon("arrow")}</button><div class="welcome-foot">中文 + English <span>·</span> ${tr("录音、转写与纪要在本机处理", "Recording, transcription and minutes run on device", "錄音、轉寫及紀要在本機處理")}</div></section></main><div id="toast" class="toast" role="status"></div><dialog id="modal"></dialog>`;
  renderMeetings();
  if (current && page === "meeting") meetingShell();
  applyViews();
}
function meetingShell() {
  page = "meeting";
  $("#workspace").classList.remove("workspace-page");
  notebookEditor?.destroy();
  notebookEditor = null;
  $("#workspace").innerHTML =
    `<header class="meeting-header"><div class="heading"><div class="eyebrow" id="meeting-date"></div><input id="meeting-title" aria-label="Meeting title" maxlength="160" spellcheck="false"></div><div class="header-actions"><button class="subtle square" data-action="retry" title="${t("retry")}" aria-label="${t("retry")}">${icon("refresh")}</button><button class="subtle" data-action="export">${icon("download")}${t("export")}</button><button id="record" class="record" data-action="record">${icon("mic")}${t("record")}</button><button id="pause-recording" class="outline" data-action="pause-recording" hidden>${icon("pause")}${t("pause")}</button><button id="end-recording" class="record active" data-action="stop-recording" hidden>${icon("stop")}${t("finish")}</button></div></header><div class="session-strip"><div class="session-state"><span id="state-dot" class="dot"></span><span id="state-label"></span><span class="time" id="duration">00:00:00</span><div class="meter" aria-hidden="true"><i id="level"></i></div></div><div class="session-right"><span id="sync">${t("saved")}</span><button class="text-button" data-action="speakers" title="${tr("说话人与自动纪要", "Speakers and automatic minutes", "說話人及自動紀要")}">${glyph("speakers")}<span id="speaker-label">${tr("说话人", "Speakers", "說話人")}</span></button><button class="text-button" data-action="meeting-files">${icon("folder")}${t("files")}</button><button class="text-button transcript-toggle" data-action="toggle-transcript" aria-expanded="false">${icon("note")}${t("transcript")}</button><button class="text-button caption-toggle" data-action="captions" title="${t("screenCaptions")}" aria-label="${t("screenCaptions")}">CC</button><button class="text-button" data-action="display-settings" title="${t("displaySettings")}" aria-label="${t("displaySettings")}">${icon("settings")}</button></div></div><div id="error-banner" role="alert" hidden></div><div class="columns"><section class="notebook">${panelRail("notebook")}<div id="notebook-content" class="pane-content"><div class="section-heading"><div><h2>${t("yourNotes")}</h2><p>${t("notesHint")}</p></div><div class="section-actions"><span class="document-label">MARKDOWN</span>${panelButton("notebook", true)}</div></div><div id="user-notes"></div></div></section><aside class="minutes-pane">${panelRail("minutes")}<div id="minutes-content" class="pane-content"><div class="minutes-heading"><div class="section-heading"><div><h2>${t("assistant")}</h2><p>${t("assistantHint")}</p></div><div class="section-actions"><span class="auto-indicator"><i></i>${t("automatic")}</span><button id="generate-notes" class="subtle square" data-action="generate" title="${t("generate")}" aria-label="${t("generate")}">${icon("refresh")}</button>${panelButton("minutes", true)}</div></div><div id="focus-context" class="focus-context"></div><div id="notes-progress" class="notes-progress" role="status" hidden></div><div id="coverage" class="coverage"></div></div><div id="suggestions"></div></div></aside><section class="transcript-pane" aria-label="${t("transcript")}">${panelRail("transcript")}<div id="transcript-content" class="pane-content"><div class="dock-resizer" role="separator" aria-label="${t("panelWidth")}"></div><div class="transcript-heading"><h2>${t("transcript")}<span id="segment-count">0</span></h2><label class="search">${icon("search")}<input id="search" placeholder="${t("search")}" aria-label="${t("search")}"></label><button class="subtle square" data-action="float-transcript" title="${t("floating")}" aria-label="${t("floating")}">↗</button>${panelButton("transcript", true)}</div><div id="transcript" class="transcript-list"></div><div class="player"><span id="player-label">${t("nothingPlaying")}</span><audio id="player" controls hidden></audio></div></div></section></div><footer><span>${icon("shield")}${tr("内容处理：离线", "Processing: offline", "內容處理：離線")} <span class="footer-separator">/</span> ${t("local")}</span><button class="text-button" data-action="shortcuts">? ${t("help")}</button></footer>`;
  transcriptSignature = "";
  noteSignature = "";
  suggestionSignature = "";
  applyViews();
  const resizer = $(".dock-resizer");
  resizer.addEventListener("pointerdown", (event) => {
    resizer.setPointerCapture(event.pointerId);
    const x = event.clientX;
    const width = viewSettings?.dock_width || 320;
    const move = (e: PointerEvent) => {
      const next = Math.max(240, Math.min(560, width + x - e.clientX));
      $("#workspace").style.setProperty("--dock-width", `${next}px`);
      if (viewSettings) viewSettings.dock_width = next;
    };
    const end = () => {
      resizer.removeEventListener("pointermove", move);
      resizer.removeEventListener("pointerup", end);
      if (viewSettings) guard(() => setViewSettings(viewSettings!));
    };
    resizer.addEventListener("pointermove", move);
    resizer.addEventListener("pointerup", end);
  });
  $("#meeting-title").addEventListener("blur", () =>
    guard(async () => {
      if (snap)
        await invoke("update_meeting", {
          id: current,
          title: $<HTMLInputElement>("#meeting-title").value,
          autoNotes: snap.meeting.auto_notes,
        });
      await refreshMeetings();
    }),
  );
  $("#search").addEventListener("input", () => {
    transcriptSignature = "";
    if (snap) renderTranscript();
  });
}
function renderMeetings() {
  $("#meeting-count").textContent = String(meetings.length);
  $("#meetings").innerHTML = meetings
    .map(
      (m) =>
        `<button class="meeting-link ${current === m.id ? "selected" : ""}" data-meeting="${m.id}"><span class="meeting-link-title">${esc(m.title)}</span><span class="meeting-link-meta">${new Date(m.created_at).toLocaleDateString(t("dateLocale"), { month: "short", day: "numeric" })}<span>${clock(m.duration_ms).slice(3)}</span></span></button>`,
    )
    .join("");
}
async function refreshMeetings() {
  meetings = await invoke<Meeting[]>("list_meetings");
  renderMeetings();
}
async function selectMeeting(id: string) {
  await leavePage();
  await flushNotes();
  const data = await invoke<Snapshot>("snapshot", { id });
  if (!data.notes.some((n) => n.kind === "document"))
    await invoke("save_note", {
      id,
      noteId: null,
      content: data.notes
        .map((n) => n.content)
        .filter(Boolean)
        .join("\n\n"),
      kind: "document",
      anchorMs: 0,
    });
  current = id;
  snap = null;
  drafts.clear();
  await invoke("set_view_meeting", { id });
  meetingShell();
  renderMeetings();
  await refresh();
}
function stateText(s: Snapshot) {
  if (s.runtime.meeting_id === current) {
    if (s.runtime.paused) return t("paused");
    if (s.runtime.recording) return t("live");
    if (s.runtime.transcribing) return t("processing");
  }
  return t(
    (
      {
        draft: "draft",
        ready: "ready",
        paused: "paused",
        interrupted: "interrupted",
        transcribing: "processing",
        importing: "processing",
        recording: "live",
      } as Record<string, Key>
    )[s.meeting.status] || "ready",
  );
}
async function refresh() {
  if (!current || busy || page !== "meeting") return;
  busy = true;
  const id = current;
  try {
    const data = await invoke<Snapshot>("snapshot", { id });
    if (current !== id || page !== "meeting") return;
    snap = data;
    $("#speaker-label").textContent = data.runtime.diarization_busy
      ? tr("分析中…", "Analyzing…")
      : tr("说话人", "Speakers", "說話人");
    const r = data.runtime,
      own = r.meeting_id === id,
      recording = own && r.recording,
      paused = own && r.paused;
    if (document.activeElement !== $("#meeting-title"))
      $<HTMLInputElement>("#meeting-title").value = data.meeting.title;
    $("#meeting-date").textContent = new Date(
      data.meeting.created_at,
    ).toLocaleDateString(t("dateLocale"), {
      year: "numeric",
      month: "long",
      day: "numeric",
      weekday: "short",
    });
    $("#state-label").textContent = stateText(data);
    $("#state-dot").className =
      `dot ${recording ? "recording" : own && r.transcribing ? "processing" : ""}`;
    $("#duration").textContent = clock(data.meeting.duration_ms);
    $("#level").style.width = `${Math.min(100, r.level * 500)}%`;
    const record = $<HTMLButtonElement>("#record");
    record.innerHTML =
      icon(paused ? "play" : "mic") +
      t(
        paused
          ? "resume"
          : data.meeting.duration_ms > 0
            ? "recordAnother"
            : "record",
      );
    record.hidden = recording;
    record.disabled = !paused && (r.recording || r.paused || r.transcribing);
    $<HTMLButtonElement>("#pause-recording").hidden = !recording;
    $<HTMLButtonElement>("#end-recording").hidden = !recording && !paused;
    const automatic = $(".auto-indicator");
    const generate = $("#generate-notes");
    const generateLabel = t(r.notes_busy ? "cancel" : "generate");
    generate.title = generateLabel;
    generate.setAttribute("aria-label", generateLabel);
    generate.innerHTML = icon(r.notes_busy ? "close" : "refresh");
    automatic.classList.toggle("working", r.notes_busy);
    automatic.innerHTML = `<i></i>${t(data.meeting.auto_notes ? "automatic" : "paused")}`;
    const banner = $("#error-banner");
    banner.hidden = !(
      (own && r.error) ||
      data.meeting.status === "interrupted"
    );
    banner.textContent =
      own && r.error
        ? r.error
        : data.meeting.status === "interrupted"
          ? t("recovered")
          : "";
    const progress = $("#notes-progress");
    progress.hidden = !r.notes_busy && !r.notes_status;
    progress.textContent = r.notes_busy
      ? `${localizedStatus(r.notes_status) || t("notesWorking")} · ${clock(r.notes_elapsed_ms || 0)}`
      : localizedStatus(r.notes_status);
    progress.classList.toggle("working", r.notes_busy);
    if (recording && r.elapsed_ms - r.processed_ms > 2500)
      $("#state-label").textContent +=
        ` · ${t("lag")} ${((r.elapsed_ms - r.processed_ms) / 1000).toFixed(1)}${t("seconds")}`;
    renderNotes();
    renderSuggestions();
    renderTranscript();
    $("#coverage").textContent =
      `${t("coverage")} ${data.meeting.notes_cursor} / ${data.segments.length}`;
  } finally {
    busy = false;
  }
}
function localizedStatus(status: string) {
  if (
    status.includes("Unsupported speaker attribution:") ||
    status.includes("Ambiguous speaker attribution:")
  )
    return tr(
      "纪要中的说话人归属未通过核对，本轮未更新。可核对转写姓名或换用更大模型后重试。",
      "Speaker attribution could not be verified, so these minutes were not updated. Check transcript names or try a larger model.",
      "紀要中的說話人歸屬未通過核對，本輪未更新。可核對轉寫姓名或換用更大模型後重試。",
    );
  const lowMemory = status.match(/^writer:low-memory:(\d+):(\d+)$/);
  if (lowMemory)
    return t("writerLowMemory")
      .replace("{available}", (Number(lowMemory[1]) / 1024).toFixed(1))
      .replace("{required}", (Number(lowMemory[2]) / 1024).toFixed(1));
  if (status === "writer:memory-unavailable")
    return t("writerMemoryUnavailable");
  if (status === "writer:loading") return t("writerLoading");
  if (status === "writer:reading") return t("writerReading");
  if (status === "writer:checking") return t("writerChecking");
  if (status.startsWith("writer:writing:"))
    return t("writerWriting").replace("{n}", status.split(":")[2]);
  const count = status.match(/^文档已同步 · (\d+) 个章节/);
  if (count) return t("generated").replace("{n}", count[1]);
  if (status === "纪要已与当前关键词和转写同步") return t("minutesSynced");
  return status;
}
function renderNotes() {
  if (!snap) return;
  const note = snap.notes.find((n) => n.kind === "document");
  if (!note || note.id === noteSignature) return;
  noteSignature = note.id;
  notebookEditor?.destroy();
  notebookEditor = new NotebookEditor(
    $("#user-notes"),
    note.content,
    (content) => {
      drafts.set(note.id, { note, content, kind: "document" });
      $("#sync").textContent = t("saving");
      clearTimeout(timers.get(note.id));
      timers.set(
        note.id,
        setTimeout(() => guard(() => persist(note.id)), 650),
      );
      renderFocus(content);
    },
    {
      timestamp: () => snap?.meeting.duration_ms || 0,
      play: (ms) => {
        guard(() => play(ms));
      },
    },
  );
  renderFocus(note.content);
}
function renderFocus(content: string) {
  const bold = [...content.matchAll(/\*\*([^*]+)\*\*/g)].map((m) => m[1]);
  const lines = content
    .split(/\n|[，,；;]/)
    .map((s) => s.replace(/^[#>*\-\d.\s]+/, "").trim())
    .filter((s) => s.length > 0 && s.length <= 24);
  const words = [...new Set(bold.length ? bold : lines)].slice(0, 6);
  $("#focus-context").innerHTML = words.length
    ? `<span>${t("following")}</span>${words.map((w) => `<span class="focus-chip">${esc(w)}</span>`).join("")}`
    : `<span>${t("focusHint")}</span>`;
}
async function persist(id: string) {
  clearTimeout(timers.get(id));
  timers.delete(id);
  const draft = drafts.get(id);
  if (!draft) return;
  drafts.delete(id);
  const previous = queues.get(id) || Promise.resolve();
  const job = previous
    .catch(() => {})
    .then(() =>
      invoke("save_note", {
        id: draft.note.meeting_id,
        noteId: id,
        content: draft.content,
        kind: draft.kind,
        anchorMs: draft.note.anchor_ms,
      }),
    );
  queues.set(id, job);
  try {
    await job;
    if (!drafts.size) $("#sync").textContent = t("saved");
  } catch (e) {
    if (!drafts.has(id)) drafts.set(id, draft);
    $("#sync").textContent = t("error");
    throw e;
  }
}
async function flushNotes() {
  for (const id of [...drafts.keys()]) await persist(id);
  await Promise.all([...queues.values()]);
}
function renderSuggestions() {
  if (!snap) return;
  const sig = JSON.stringify(snap.suggestions);
  if (sig === suggestionSignature) return;
  suggestionSignature = sig;
  const rows = snap.suggestions.filter((s) => s.status !== "dismissed");
  $("#suggestions").innerHTML = rows.length
    ? `<article class="minutes-document"><div class="minutes-document-title"><span>${t("meetingDocument")}</span><h1>${esc(snap.meeting.title)}</h1></div>${rows
        .map(
          (s) =>
            `<section class="minutes-section"><div class="minutes-section-heading"><h3>${esc(s.focus || t("meetingDocument"))}</h3><div class="paragraph-tools"><button class="text-button" data-edit-suggestion="${s.id}" title="${t("editLock")}" aria-label="${t("editLock")}">${icon("edit")}</button><button class="text-button" data-dismiss="${s.id}" title="${t("dismiss")}" aria-label="${t("dismiss")}">${icon("close")}</button></div></div>${s.text
              .split(/\n\s*\n/)
              .map((p) => `<p>${esc(p)}</p>`)
              .join(
                "",
              )}${s.stale ? `<p class="stale">${t("stale")}</p>` : ""}<details class="document-sources"><summary>${s.status === "accepted" ? `${icon("check")}${t("protected")} · ` : ""}${t("source")} <span>${s.sources.map((q) => clock(q.start_ms)).join(" · ")}</span></summary>${s.sources.map((q) => `<blockquote><button class="timestamp" data-seek="${q.start_ms}" data-end="${q.end_ms}">${icon("play")}${clock(q.start_ms)}</button>${esc(q.quote)}</blockquote>`).join("")}</details></section>`,
        )
        .join(
          "",
        )}<div class="document-colophon">${t("documentColophon")}</div></article>`
    : `<div class="empty-suggestions">${icon("note")}<p>${t("emptySuggestions")}</p><small>${t("emptySuggestionsSub")}</small></div>`;
}
function renderTranscript() {
  if (!snap) return;
  const query = $<HTMLInputElement>("#search").value.toLowerCase().trim();
  const partial =
    snap.runtime.meeting_id === current ? snap.runtime.partial : "";
  const sig =
    JSON.stringify(snap.segments) +
    JSON.stringify(snap.meeting.speaker_names) +
    partial +
    query;
  if (sig === transcriptSignature) return;
  transcriptSignature = sig;
  const pane = $("#transcript");
  const nearBottom =
    pane.scrollHeight - pane.scrollTop - pane.clientHeight < 90;
  const rows = snap.segments.filter((s) =>
    (s.text + " " + speakerName(s, snap!.meeting))
      .toLowerCase()
      .includes(query),
  );
  $("#segment-count").textContent = String(snap.segments.length);
  pane.innerHTML =
    rows
      .map(
        (s) =>
          `<article class="segment"><div class="segment-top"><button class="timestamp" data-seek="${s.start_ms}" data-end="${s.end_ms}">${clock(s.start_ms)}</button><button class="speaker-chip" data-speaker-segment="${s.id}">${esc(speakerName(s, snap!.meeting))}</button><button class="segment-edit" data-edit-segment="${s.id}" title="${t("edit")}" aria-label="${t("edit")}">${icon("edit")}</button></div><p>${esc(s.text)}</p></article>`,
      )
      .join("") +
    (partial && !query
      ? `<article class="segment partial"><span class="partial-label"><i></i>${t("live")}</span><p>${esc(partial)}</p></article>`
      : "") +
    (!rows.length && !partial
      ? `<div class="empty-transcript">${icon("mic")}<p>${query ? t("noMatches") : t("emptyTranscript")}</p><small>${query ? "" : t("emptyTranscriptSub")}</small></div>`
      : "");
  if (nearBottom && !query) pane.scrollTop = pane.scrollHeight;
}
async function newMeeting() {
  await flushNotes();
  const m = await invoke<Meeting>("create_meeting", { title: t("untitled") });
  await refreshMeetings();
  await selectMeeting(m.id);
  $<HTMLInputElement>("#meeting-title").focus();
  $<HTMLInputElement>("#meeting-title").select();
}
async function recording() {
  if (!current) return;
  if (snap?.runtime.paused && snap.runtime.meeting_id === current)
    await invoke("resume_recording");
  else if (snap?.runtime.recording && snap.runtime.meeting_id === current)
    await invoke("stop_recording");
  else await invoke("start_recording", { id: current, device: chosenDevice });
  await refresh();
}
function modal(title: string, body: string, actions: string = "") {
  const dialog = $<HTMLDialogElement>("#modal");
  dialog.innerHTML = `<div class="modal-heading"><h2>${esc(title)}</h2><button class="subtle square" data-action="close-modal" aria-label="${t("close")}">${icon("close")}</button></div>${body}${actions ? `<div class="modal-actions">${actions}</div>` : ""}`;
  if (!dialog.open) dialog.showModal();
}
function closeModal() {
  $<HTMLDialogElement>("#modal").close();
}
async function leavePage() {
  await pageHandle?.close();
  pageHandle = null;
  await flushNotes();
  notebookEditor?.destroy();
  notebookEditor = null;
}
async function navigate(
  destination: string,
  section?: string,
  profile?: Profile,
) {
  await leavePage();
  if (destination === "meetings") {
    if (current) return selectMeeting(current);
    page = "meeting";
    shell();
    return;
  }
  page = destination;
  document
    .querySelectorAll<HTMLElement>("[data-page]")
    .forEach((b) =>
      b.classList.toggle("selected", b.dataset.page === destination),
    );
  const root = $("#workspace");
  root.classList.remove("dock-transcript");
  root.innerHTML = `<div class="page-loading">${tr("正在打开…", "Opening…", "正在開啟…")}</div>`;
  if (destination === "settings") {
    pageHandle = await mountSettings(root, {
      toast,
      section,
      profile,
      back: () => navigate("meetings"),
      language: async (l) => {
        setLocale(l);
        await pageHandle?.close();
        pageHandle = null;
        shell();
        await navigate("settings", "general");
      },
      display: showDisplaySettings,
      device: (id) => {
        chosenDevice = id;
        localStorage.setItem("omascribe-device", id);
      },
    });
  } else {
    pageHandle = await mountLibrary(root, destination as LibraryPage, {
      toast,
      meeting: selectMeeting,
      settings: () => navigate("settings", "transcription"),
    });
  }
  applyViews();
}
async function settings() {
  return navigate("settings");
}
async function importAudio() {
  await flushNotes();
  const prefs = await getPreferences();
  modal(
    t("import"),
    `<p class="setting-help">${tr("选择音频后在本机转写。可在转写结束后区分说话人。", "Transcribe audio on this device and optionally separate speakers after transcription.", "選擇音訊後於本機轉寫，可在完成後區分說話人。")}</p><label class="check-setting"><input id="import-speakers" type="checkbox" ${prefs.diarization ? "checked" : ""}>${tr("区分说话人", "Separate speakers", "區分說話人")}</label>`,
    `<button class="primary" id="choose-audio">${tr("选择音频文件", "Choose audio file", "選擇音訊檔案")}</button>`,
  );
  $("#choose-audio").onclick = () =>
    guard(async () => {
      const diarization = $<HTMLInputElement>("#import-speakers").checked;
      closeModal();
      const m = await invoke<Meeting | null>("import_audio", { diarization });
      if (m) {
        await refreshMeetings();
        await selectMeeting(m.id);
      }
    });
}
function applyViews() {
  if (!viewSettings) return;
  const states: Record<Panel, boolean> = {
    sidebar: viewSettings.sidebar_collapsed,
    notebook: viewSettings.notebook_collapsed,
    minutes: viewSettings.minutes_collapsed,
    transcript: viewSettings.transcript_mode !== "docked",
  };
  for (const [panel, collapsed] of Object.entries(states)) {
    const content = document.getElementById(`${panel}-content`);
    if (!content) continue;
    content.hidden = collapsed;
    content.parentElement!.classList.toggle("is-collapsed", collapsed);
    document.querySelector<HTMLElement>(`[data-rail="${panel}"]`)!.hidden =
      !collapsed;
  }
  if (!current || page !== "meeting") return;
  $("#workspace").classList.toggle(
    "dock-transcript",
    viewSettings.transcript_mode === "docked",
  );
  $("#workspace").style.setProperty(
    "--dock-width",
    `${viewSettings.dock_width}px`,
  );
  $("#workspace").style.setProperty(
    "--transcript-font",
    `${viewSettings.transcript_font}px`,
  );
  document
    .querySelector(".transcript-toggle")
    ?.setAttribute(
      "aria-expanded",
      String(viewSettings.transcript_mode !== "hidden"),
    );
  document
    .querySelector(".caption-toggle")
    ?.classList.toggle("enabled", viewSettings.captions.enabled);
}
async function toggleTranscript(open?: boolean) {
  const p = await getViewSettings();
  const shown = open ?? p.transcript_mode === "hidden";
  viewSettings = await patchViewSettings({
    transcript_mode: shown ? "docked" : "hidden",
  });
  applyViews();
}
async function showDisplaySettings() {
  modal(t("displaySettings"), '<div id="display-settings-body"></div>');
  await displaySettings($("#display-settings-body"), (e) =>
    toast(String(e), true),
  );
}
async function play(start: number, end?: number) {
  if (!current || !snap) return;
  const limit = Math.min(
    end || start + 20_000,
    snap.meeting.duration_ms,
    start + 90_000,
  );
  if (limit <= start) return;
  const encoded = await invoke<string>("audio_clip", {
    id: current,
    startMs: start,
    endMs: limit,
  });
  if (audioUrl) URL.revokeObjectURL(audioUrl);
  audioUrl = URL.createObjectURL(
    new Blob([Uint8Array.from(atob(encoded), (c) => c.charCodeAt(0))], {
      type: "audio/wav",
    }),
  );
  await toggleTranscript(true);
  const player = $<HTMLAudioElement>("#player");
  player.src = audioUrl;
  player.hidden = false;
  $("#player-label").textContent = `${t("audioPlaying")} · ${clock(start)}`;
  await player.play();
}
async function editTranscript(id: string) {
  const s = snap?.segments.find((s) => s.id === id);
  if (!s) return;
  modal(
    t("editTranscript"),
    `<textarea id="edit-text" class="modal-text" rows="6">${esc(s.text)}</textarea>`,
    `<button class="primary" id="save-edit">${t("save")}</button>`,
  );
  $("#save-edit").onclick = () =>
    guard(async () => {
      await invoke("edit_segment", {
        id,
        text: $<HTMLTextAreaElement>("#edit-text").value,
        revision: s.revision,
      });
      closeModal();
      await refresh();
    });
}
async function editSuggestion(id: string) {
  const s = snap?.suggestions.find((s) => s.id === id);
  if (!s) return;
  modal(
    t("editSuggestion"),
    `<textarea id="edit-text" class="modal-text" rows="5">${esc(s.text)}</textarea>`,
    `<button class="primary" id="save-edit">${t("save")}</button>`,
  );
  $("#save-edit").onclick = () =>
    guard(async () => {
      await invoke("update_suggestion", {
        meeting: current,
        id,
        status: "accepted",
        text: $<HTMLTextAreaElement>("#edit-text").value,
      });
      closeModal();
      await refresh();
    });
}
let fileMeeting: string | null = null;
let filesBusy = false;
async function refreshFiles() {
  const id = fileMeeting;
  if (!id || !document.querySelector("#meeting-file-list") || filesBusy) return;
  filesBusy = true;
  try {
    const data = await invoke<MeetingFiles>("meeting_files", { id });
    if (fileMeeting !== id || !document.querySelector("#meeting-file-list"))
      return;
    $("#meeting-files-path").textContent = data.directory;
    const size = (n: number) =>
      n >= 1024 * 1024
        ? `${(n / 1024 / 1024).toFixed(1)} MB`
        : `${Math.ceil(n / 1024)} KB`;
    const html = data.files
      .map(
        (f) =>
          `<article class="meeting-file"><span class="file-icon">${icon(f.kind === "audio" ? "mic" : "note")}</span><div class="file-description"><strong>${esc(f.name)}</strong><small>${f.take ? t("take").replace("{n}", String(f.take)) + " · " : t("fullTranscript") + " · "}${f.kind === "audio" ? clock(f.duration_ms) + " · " : ""}${f.status === "recording" ? t("fileRecording") : size(f.bytes)}${f.status === "interrupted" ? " · " + t("interrupted") : ""}</small></div><button class="subtle square" data-file="${esc(f.name)}" ${f.kind === "audio" && f.status === "recording" ? "disabled" : ""} title="${t("showInFolder")}" aria-label="${t("showInFolder")}: ${esc(f.name)}">${icon("folder")}</button></article>`,
      )
      .join("");
    const list = $("#meeting-file-list");
    if (list.innerHTML !== html) list.innerHTML = html;
  } finally {
    filesBusy = false;
  }
}
async function showMeetingFiles() {
  fileMeeting = current;
  modal(
    t("files"),
    `<div class="meeting-files"><p class="file-help">${t("filesHint")}</p><div id="meeting-file-list" aria-live="polite"></div><p class="export-path" id="meeting-files-path"></p></div>`,
    `<button class="primary" data-action="open-meeting-folder">${icon("folder")}${t("openMeetingFolder")}</button>`,
  );
  await refreshFiles();
}
async function exportMeeting() {
  await flushNotes();
  modal(
    t("export"),
    `<label class="check-setting"><input id="export-audio" type="checkbox">${t("exportAudio")}</label>`,
    `<button class="primary" id="export-confirm">${icon("download")}${t("export")}</button>`,
  );
  $("#export-confirm").onclick = () =>
    guard(async () => {
      const button = $<HTMLButtonElement>("#export-confirm");
      button.disabled = true;
      const path = await invoke<string>("export_meeting", {
        id: current,
        audio: $<HTMLInputElement>("#export-audio").checked,
      });
      modal(
        t("exportDone"),
        `<p class="export-path">${esc(path)}</p>`,
        `<button class="primary" data-action="open-exports">${t("openFolder")}</button>`,
      );
    });
}
function shortcuts() {
  modal(
    t("shortcuts"),
    `<div class="shortcuts">${[
      ["Ctrl Shift R", "shortcutRecord"],
      ["Ctrl Shift P", "pauseResume"],
      ["Ctrl N", "shortcutNew"],
      ["Ctrl F", "shortcutSearch"],
      ["Ctrl B / I", "formatInline"],
      ["Ctrl Alt 1 / 2 / 3", "heading"],
      ["Ctrl Shift 8 / 7 / 9", "lists"],
      ["Ctrl Z / Shift Z", "undoRedo"],
    ]
      .map(([keys, label]) => `<p>${t(label as Key)}<kbd>${keys}</kbd></p>`)
      .join("")}</div>`,
  );
}
async function quit() {
  await pageHandle?.flush?.();
  if (current) snap = await invoke<Snapshot>("snapshot", { id: current });
  await flushNotes();
  if (
    snap?.runtime.recording ||
    snap?.runtime.paused ||
    snap?.runtime.transcribing ||
    snap?.runtime.notes_busy ||
    snap?.runtime.diarization_busy
  ) {
    modal(
      "OmaScribe",
      `<p>${t("confirmQuit")}</p>`,
      `<button class="subtle" data-action="close-modal">${t("stay")}</button><button class="danger" data-action="quit">${t("leave")}</button>`,
    );
  } else await invoke("quit");
}
document.addEventListener("click", (e) => {
  const el = (e.target as HTMLElement).closest<HTMLElement>("button");
  if (!el) return;
  guard(async () => {
    if (el.dataset.page) {
      navigation = navigation
        .then(() => navigate(el.dataset.page!))
        .catch((e) => toast(String(e), true));
      return navigation;
    }
    if (el.dataset.speakerSegment && current)
      return speakerDialog(
        $<HTMLDialogElement>("#modal"),
        current,
        el.dataset.speakerSegment,
        refresh,
        toast,
      );
    if (el.dataset.panel) return togglePanel(el.dataset.panel as Panel);
    if (el.dataset.meeting) return selectMeeting(el.dataset.meeting);
    if (el.dataset.seek !== undefined)
      return play(
        Number(el.dataset.seek),
        el.dataset.end ? Number(el.dataset.end) : undefined,
      );
    if (el.dataset.accept || el.dataset.dismiss) {
      await invoke("update_suggestion", {
        meeting: current,
        id: el.dataset.accept || el.dataset.dismiss,
        status: el.dataset.accept ? "accepted" : "dismissed",
        text: null,
      });
      return refresh();
    }
    if (el.dataset.editSegment) return editTranscript(el.dataset.editSegment);
    if (el.dataset.editSuggestion)
      return editSuggestion(el.dataset.editSuggestion);
    if (el.dataset.file)
      return invoke("open_meeting_files", {
        id: fileMeeting,
        name: el.dataset.file,
      });
    switch (el.dataset.action) {
      case "show-library":
        return navigate("meetings");
      case "speakers":
        if (current)
          return speakerDialog(
            $<HTMLDialogElement>("#modal"),
            current,
            undefined,
            refresh,
            toast,
          );
        return;
      case "meeting-files":
        return showMeetingFiles();
      case "open-meeting-folder":
        return invoke("open_meeting_files", { id: fileMeeting, name: null });
      case "new":
        return newMeeting();
      case "record":
        return recording();
      case "settings":
        return settings();
      case "close-modal":
        return closeModal();
      case "shortcuts":
        return shortcuts();
      case "export":
        return exportMeeting();
      case "open-exports":
        return invoke("open_exports");
      case "quit":
        await pageHandle?.flush?.();
        await flushNotes();
        return invoke("quit");
      case "import":
        return importAudio();
      case "generate":
        await flushNotes();
        await invoke(
          snap?.runtime.notes_busy ? "cancel_notes" : "generate_notes",
          { id: current },
        );
        return refresh();
      case "retry":
        await invoke("retranscribe", { id: current });
        return refresh();
      case "toggle-transcript":
        return toggleTranscript();
      case "float-transcript":
        viewSettings = await patchViewSettings({
          transcript_mode: "floating",
          compact: false,
        });
        applyViews();
        return;
      case "display-settings":
        return showDisplaySettings();
      case "captions": {
        const p = await getViewSettings();
        p.captions.enabled = !p.captions.enabled;
        viewSettings = await setViewSettings(p);
        applyViews();
        return;
      }
      case "pause-recording":
        await invoke("pause_recording");
        return refresh();
      case "stop-recording":
        await invoke("stop_recording");
        return refresh();
    }
  });
});
document.addEventListener("keydown", (e) => {
  if (e.isComposing || e.keyCode === 229) return;
  if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "n") {
    e.preventDefault();
    guard(newMeeting);
  }
  if ((e.ctrlKey || e.metaKey) && e.shiftKey && e.key.toLowerCase() === "r") {
    e.preventDefault();
    guard(recording);
  }
  if (
    (e.ctrlKey || e.metaKey) &&
    e.shiftKey &&
    e.key.toLowerCase() === "p" &&
    current
  ) {
    e.preventDefault();
    guard(async () => {
      await invoke(
        snap?.runtime.paused ? "resume_recording" : "pause_recording",
      );
      await refresh();
    });
  }
  if (
    (e.ctrlKey || e.metaKey) &&
    e.key.toLowerCase() === "f" &&
    current &&
    page === "meeting"
  ) {
    e.preventDefault();
    guard(async () => {
      await toggleTranscript(true);
      $<HTMLInputElement>("#search").focus();
    });
  }
});
document.addEventListener("open-settings", (event) =>
  guard(() => navigate("settings", (event as CustomEvent<string>).detail)),
);
async function init() {
  await initAppearance();
  viewSettings = await getViewSettings();
  await listen<ViewPreferences>("view-settings", (event) => {
    viewSettings = event.payload;
    applyViews();
  });
  await listen("request-display-settings", () => guard(showDisplaySettings));
  shell();
  await refreshMeetings();
  diag = await invoke<Diagnostic>("diagnostics");

  if (meetings.length) await selectMeeting(meetings[0].id);
  await listen("close-requested", () => guard(quit));
  const previewProfile = async () => {
    const profile = await invoke<Profile | null>("take_pending_profile");
    if (profile) await navigate("settings", "profiles", profile);
  };
  await listen("profile-import-available", () => guard(previewProfile));
  await guard(previewProfile);
  setInterval(() => guard(refresh), 600);
  setInterval(() => guard(refreshFiles), 3000);
  setInterval(
    () =>
      guard(async () => {
        await refreshMeetings();
        diag = await invoke<Diagnostic>("diagnostics");
      }),
    5000,
  );
}
guard(init);
