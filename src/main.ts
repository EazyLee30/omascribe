import { WorkspaceDock, type DockPanel } from "./workspace-dock";
import { translationSelect, passageHTML } from "./translation-ui";
import { confirmDelete, reveal, recordingMotion } from "./interactions";
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
import "./workbench.css";
import { mountSettings } from "./settings";
import { mountLibrary, type LibraryPage } from "./library";
import {
  initAppearance,
  getPreferences,
  savePreferences,
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
};
const icon = (name: string) =>
  `<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="${icons[name] || icons.note}"/></svg>`;
const brand = `<svg viewBox="0 0 256 256" fill="none" aria-hidden="true"><path d="M55 62c0-11 9-20 20-20h106v157H75c-11 0-20 7-20 17V62Z" fill="currentColor" opacity=".16"/><path d="M181 42H77c-12 0-22 10-22 22v137c0-12 10-22 22-22h104V42ZM77 179v35h104" stroke="currentColor" stroke-width="10" stroke-linejoin="round"/><path d="M94 99v32m26-51v70m26-59v49" stroke="currentColor" stroke-width="11" stroke-linecap="round"/><path d="m165 150 24-24 15 15-24 24-20 5 5-20Z" fill="currentColor" stroke="#191b17" stroke-width="5" stroke-linejoin="round"/></svg>`;
let dock: WorkspaceDock | null = null;
let meetingsLoaded = false;
let openTabs: string[] = (() => {
  try {
    const ids = JSON.parse(
      localStorage.getItem("omascribe-meeting-tabs") || "[]",
    );
    return Array.isArray(ids)
      ? ids.filter((id: unknown) => typeof id === "string").slice(0, 12)
      : [];
  } catch {
    return [];
  }
})();
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
    const open = [
      !prefs.notebook_collapsed,
      !prefs.minutes_collapsed,
      prefs.transcript_mode === "docked",
    ];
    const index = ["notebook", "minutes", "transcript"].indexOf(panel);
    if (index >= 0 && open[index] && open.filter(Boolean).length === 1) return;
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
  document.body.classList.add("main-window");
  document.body.classList.toggle(
    "platform-macos",
    /Mac/.test(navigator.platform),
  );
  $("#app").innerHTML =
    `<div class="window-toolbar" data-tauri-drag-region>${panelButton("sidebar", true)}<div id="window-content" data-tauri-drag-region></div><div id="window-panels" hidden><button class="subtle view-trigger" data-action="workspace-view" aria-haspopup="menu" aria-controls="view-menu" aria-expanded="false">${icon("panelRight")}<span>${tr("视图", "View", "檢視")}</span><span aria-hidden="true">⌄</span></button><div id="view-menu" class="view-menu" role="menu" aria-label="${tr("工作区视图", "Workspace view", "工作區檢視")}" hidden></div></div></div><aside class="sidebar">${panelRail("sidebar")}<div id="sidebar-content"><div class="brand"><span class="brand-icon">${brand}</span><div>OmaScribe<small>OFFLINE NOTEBOOK</small></div></div><button class="primary new-meeting quick-record" data-action="quick-record">${icon("mic")}<span>${tr("开始录音", "Start recording", "開始錄音")}</span></button><button class="subtle sidebar-new" data-action="new">${icon("plus")}${t("newMeeting")}</button><button class="subtle import" data-action="import">${icon("upload")}${t("import")}</button><nav class="workspace-navigation">${navItems()
      .map(
        ([id, label]) =>
          `<button class="subtle" data-page="${id}">${glyph(id)}${label}</button>`,
      )
      .join(
        "",
      )}</nav><div class="library-label">${t("library")}<span id="meeting-count">0</span></div><nav id="meetings" aria-label="${t("library")}"></nav><div class="sidebar-bottom"><button class="subtle" data-action="settings">${icon("settings")}${t("settings")}</button></div></div></aside><main id="workspace"><section class="welcome"><div class="welcome-mark">${brand}</div><p class="eyebrow">OMASCRIBE</p><h1>${t("welcome")}</h1><p>${t("welcomeSub")}</p><button class="primary" data-action="quick-record">${icon("mic")}${tr("开始录音", "Start recording", "開始錄音")}</button><div class="welcome-foot">中文 + English <span>·</span> ${tr("录音、转写与纪要在本机处理", "Recording, transcription and minutes run on device", "錄音、轉寫及紀要在本機處理")}</div></section></main><div id="toast" class="toast" role="status"></div><dialog id="modal"></dialog>`;
  renderMeetings();
  if (current && page === "meeting") meetingShell();
  applyViews();
}
function meetingShell() {
  dock?.dispose();
  dock = null;
  page = "meeting";
  $("#workspace").classList.remove("workspace-page");
  notebookEditor?.destroy();
  notebookEditor = null;
  $("#workspace").innerHTML =
    `<header class="meeting-header" data-tauri-drag-region><div class="heading" data-tauri-drag-region><div class="eyebrow" id="meeting-date" data-tauri-drag-region></div><div class="title-line"><span id="meeting-title-size" aria-hidden="true"></span><input id="meeting-title" aria-label="Meeting title" maxlength="160" spellcheck="false" size="1"></div></div><div class="header-actions"><button class="subtle square" data-action="retry" title="${t("retry")}" aria-label="${t("retry")}">${icon("refresh")}</button><button class="subtle" data-action="export">${icon("download")}${t("export")}</button><button id="record" class="record" data-action="record">${icon("mic")}${t("record")}</button><button id="pause-recording" class="outline" data-action="pause-recording" hidden>${icon("pause")}${t("pause")}</button><button id="end-recording" class="record active" data-action="stop-recording" hidden>${icon("stop")}${t("finish")}</button></div></header><div class="session-strip"><div class="session-state"><span id="state-dot" class="dot"></span><span id="state-label"></span><div class="recording-clock"><span class="time" id="duration">00:00:00</span><small>${tr("已录制", "RECORDED", "已錄製")}</small></div><div class="recording-wave" aria-hidden="true">${"<i></i>".repeat(11)}</div></div><div class="session-right"><span id="sync">${t("saved")}</span><div class="session-display" role="group" aria-label="${tr("实时显示", "Live views", "即時顯示")}"><button class="text-button transcript-toggle" data-action="toggle-transcript" aria-pressed="false" aria-controls="transcript-content">${icon("note")}<span>${tr("实时转写", "Live transcript", "即時轉寫")}</span></button><button class="text-button caption-toggle" data-action="captions" aria-pressed="false"><span class="view-cc" aria-hidden="true">CC</span><span>${tr("实时字幕", "Live captions", "即時字幕")}</span></button></div><button class="text-button" data-action="speakers" title="${tr("说话人与自动纪要", "Speakers and automatic minutes", "說話人及自動紀要")}">${glyph("speakers")}<span id="speaker-label">${tr("说话人", "Speakers", "說話人")}</span></button><button class="text-button" data-action="meeting-files">${icon("folder")}${t("files")}</button></div></div><div id="error-banner" role="alert" hidden></div><div class="columns"><section class="notebook"><div id="notebook-content" class="pane-content"><div class="section-heading"><div><h2>${t("yourNotes")}</h2><p>${t("notesHint")}</p></div><div class="section-actions"><button class="subtle square" data-action="delete-note" title="${tr("删除笔记", "Delete note", "刪除筆記")}" aria-label="${tr("删除笔记", "Delete note", "刪除筆記")}">${glyph("trash")}</button></div></div><div id="user-notes"></div></div></section><aside class="minutes-pane"><div id="minutes-content" class="pane-content"><div class="minutes-heading"><div class="section-heading"><div><h2>${t("assistant")}</h2><p>${t("assistantHint")}</p></div><div class="section-actions"><span class="auto-indicator"><i></i>${t("automatic")}</span><button id="generate-notes" class="subtle minutes-generate" data-action="generate" title="${t("generate")}" aria-label="${t("generate")}">${icon("refresh")}</button></div></div><div id="focus-context" class="focus-context"></div><div id="notes-progress" class="notes-progress" role="status" hidden></div><div id="coverage" class="coverage"></div></div><div id="suggestions"></div></div></aside><section class="transcript-pane" aria-label="${t("transcript")}"><div id="transcript-content" class="pane-content"><div class="dock-resizer" role="separator" aria-label="${t("panelWidth")}"></div><div class="transcript-heading"><h2><span id="segment-count">0</span> ${tr("条转写", "passages", "條轉寫")}</h2>${translationSelect("translation-mode")}<span id="translation-status" class="translation-status" role="status"></span><label class="search">${icon("search")}<input id="search" placeholder="${t("search")}" aria-label="${t("search")}"></label></div><div id="transcript" class="transcript-list"></div><div class="player"><span id="player-label">${t("nothingPlaying")}</span><audio id="player" controls hidden></audio></div></div></section></div>`;
  const root = $("#workspace");
  root.classList.add("workbench");
  renderMeetingTabs();
  const header = $(".meeting-header");
  const menu = document.createElement("details");
  menu.className = "meeting-more";
  menu.innerHTML = `<summary aria-label="${tr("会议操作", "Meeting actions", "會議操作")}" title="${tr("会议操作", "Meeting actions", "會議操作")}">•••</summary><div class="meeting-actions-menu"><button data-action="export">${icon("download")}${t("export")}</button><button data-action="meeting-files">${icon("folder")}${t("files")}</button><button data-action="speakers">${glyph("speakers")}<span id="speaker-label">${tr("说话人", "Speakers", "說話人")}</span></button><button data-action="retry">${icon("refresh")}${t("retry")}</button><button data-action="delete-note">${glyph("trash")}${tr("删除笔记", "Delete note", "刪除筆記")}</button></div>`;
  document
    .querySelectorAll(
      ".header-actions [data-action='retry'],.header-actions [data-action='export'],.session-right [data-action='speakers'],.session-right [data-action='meeting-files']",
    )
    .forEach((e) => e.remove());
  header.append($("#sync"), menu);
  const controls = $(".session-strip");
  controls.classList.add("recording-console");
  controls.append($(".header-actions"));
  controls
    .querySelectorAll<HTMLButtonElement>(".session-display button")
    .forEach((button) => {
      button.setAttribute("aria-label", button.textContent!.trim());
      button.title = button.textContent!.trim();
    });
  const columns = $(".columns");
  const panels = {
    notebook: $(".notebook"),
    minutes: $(".minutes-pane"),
    transcript: $(".transcript-pane"),
  };
  columns.className = "panel-parking";
  columns.hidden = true;
  const host = document.createElement("div");
  host.id = "workspace-dock";
  root.insertBefore(host, columns);
  header.insertBefore(controls, $("#sync"));
  dock = new WorkspaceDock(host, panels, columns, (visible) => {
    if (!viewSettings || !dock) return;
    const mode = visible.transcript
      ? "docked"
      : viewSettings.transcript_mode === "floating"
        ? "floating"
        : "hidden";
    const patch = {
      notebook_collapsed: !visible.notebook,
      minutes_collapsed: !visible.minutes,
      transcript_mode: mode as ViewPreferences["transcript_mode"],
    };
    if (
      Object.entries(patch).some(
        ([key, value]) => viewSettings![key as keyof ViewPreferences] !== value,
      )
    ) {
      Object.assign(viewSettings, patch);
      guard(async () => {
        viewSettings = await patchViewSettings(patch);
        applyViews();
      });
    }
    updateLiveButtons();
  });
  if (viewSettings) {
    viewSettings.notebook_collapsed = !dock.has("notebook");
    viewSettings.minutes_collapsed = !dock.has("minutes");
    viewSettings.transcript_mode = dock.has("transcript")
      ? "docked"
      : viewSettings.transcript_mode === "floating"
        ? "floating"
        : "hidden";
  }

  $("#window-panels").hidden = false;
  transcriptSignature = "";
  noteSignature = "";
  suggestionSignature = "";
  applyViews();
  const titleInput = $<HTMLInputElement>("#meeting-title");
  titleInput.addEventListener("input", () => {
    $("#meeting-title-size").textContent = titleInput.value || " ";
  });
  titleInput.addEventListener("keydown", (event) => {
    if (event.key === "Enter") {
      event.preventDefault();
      titleInput.blur();
    }
    if (event.key === "Escape") {
      titleInput.value = snap?.meeting.title || "";
      $("#meeting-title-size").textContent = titleInput.value;
      titleInput.blur();
    }
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
function renderMeetingTabs() {
  const host = document.getElementById("window-content");
  if (!host) return;
  if (meetingsLoaded)
    openTabs = openTabs.filter((id) => meetings.some((m) => m.id === id));
  if (current && !openTabs.includes(current)) openTabs.push(current);
  openTabs = openTabs.slice(-12);
  localStorage.setItem("omascribe-meeting-tabs", JSON.stringify(openTabs));
  host.innerHTML = `<div class="meeting-tabs" role="tablist" aria-label="${tr("打开的会议", "Open meetings", "開啟的會議")}">${openTabs
    .map((id) => {
      const m = meetings.find((m) => m.id === id);
      return m
        ? `<div class="meeting-tab${id === current && page === "meeting" ? " active" : ""}"><button role="tab" aria-selected="${id === current && page === "meeting"}" data-meeting="${esc(id)}" title="${esc(m.title)}">${icon("note")}<span>${esc(m.title)}</span></button><button class="meeting-tab-close" data-close-tab="${esc(id)}" aria-label="${tr("关闭标签", "Close tab", "關閉分頁")} ${esc(m.title)}">×</button></div>`
        : "";
    })
    .join(
      "",
    )}<button class="new-tab" data-action="new-tab" aria-label="${tr("新建标签页", "New tab", "新增分頁")}">+</button></div>`;
}
function renderMeetings() {
  $("#meeting-count").textContent = String(meetings.length);
  $("#meetings").innerHTML = meetings
    .map(
      (m) =>
        `<div class="meeting-item"><button class="meeting-link ${current === m.id ? "selected" : ""}" data-meeting="${m.id}"><span class="meeting-link-title">${esc(m.title)}</span><span class="meeting-link-meta">${new Date(m.created_at).toLocaleDateString(t("dateLocale"), { month: "short", day: "numeric" })}<span>${clock(m.duration_ms).slice(3)}</span></span></button><button class="meeting-delete subtle square" data-action="delete-meeting" data-id="${m.id}" aria-label="${esc(tr("删除会议", "Delete meeting", "刪除會議") + ": " + m.title)}">${glyph("trash")}</button></div>`,
    )
    .join("");
  renderMeetingTabs();
}
async function refreshMeetings() {
  meetings = await invoke<Meeting[]>("list_meetings");
  meetingsLoaded = true;
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
  localStorage.setItem("omascribe-active-meeting", id);
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
    $("#meeting-title-size").textContent =
      $<HTMLInputElement>("#meeting-title").value || " ";
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
    recordingMotion(
      $(".session-state"),
      recording
        ? "recording"
        : paused
          ? "paused"
          : own && r.transcribing
            ? "processing"
            : "ready",
      own ? r.level : 0,
    );
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
    generate.innerHTML =
      icon(r.notes_busy ? "close" : "refresh") +
      `<span>${r.notes_busy ? t("cancel") : tr("整理", "Generate", "整理")}</span>`;
    automatic.classList.toggle("working", r.notes_busy);
    automatic.dataset.enabled = String(data.meeting.auto_notes);
    automatic.innerHTML = `<i></i>${data.meeting.auto_notes ? t("automatic") : tr("自动整理已关闭", "Auto notes off", "自動整理已關閉")}`;
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
  if (status.startsWith("writer:resource-budget-hit:"))
    return tr(
      "整理已暂停以控制内存占用，原文已保留。可选择较小模型或调整资源预算后重试。",
      "Minutes paused to limit memory use; your transcript is preserved. Try a smaller model or adjust the resource budget.",
      "整理已暫停以控制記憶體用量，原文已保留。可選擇較小模型或調整資源預算後重試。",
    );
  if (status === "writer:resource-pressure")
    return tr(
      "系统可用内存过低，已暂停整理。原文已保留，内存恢复后会自动重试。",
      "Available system memory is low. Minutes are paused and will retry when memory recovers; your transcript is preserved.",
      "系統可用記憶體過低，已暫停整理。原文已保留，記憶體恢復後會自動重試。",
    );
  const resourceBudget = status.match(/^writer:resource-budget:(\d+):(\d+)$/);
  if (resourceBudget)
    return tr(
      "所选模型超过当前内存预算。请换用较小模型，或在设置中调整预算。",
      "This model exceeds the current memory budget. Choose a smaller model or adjust the budget in Settings.",
      "所選模型超過目前記憶體預算。請改用較小模型，或於設定中調整預算。",
    );
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
let focusTimer: ReturnType<typeof setTimeout> | undefined;
let focusRequest = 0;
function renderFocus(content: string) {
  clearTimeout(focusTimer);
  const request = ++focusRequest;
  const meeting = current;
  focusTimer = setTimeout(() => {
    void invoke<string[]>("extract_focus_terms", { content })
      .then((words) => {
        const root = document.querySelector("#focus-context");
        if (!root || request !== focusRequest || meeting !== current) return;
        root.innerHTML = words.length
          ? `<span>${t("following")}</span>${words.map((w) => `<span class="focus-chip">${esc(w)}</span>`).join("")}`
          : `<span>${t("focusHint")}</span>`;
      })
      .catch(() => {
        const root = document.querySelector("#focus-context");
        if (root && request === focusRequest) root.textContent = t("focusHint");
      });
  }, 250);
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
  const sig = JSON.stringify([snap.meeting.title, snap.suggestions]);
  if (sig === suggestionSignature) return;
  suggestionSignature = sig;
  const rows = snap.suggestions.filter((s) => s.status !== "dismissed");
  $("#suggestions").innerHTML = rows.length
    ? `<article class="minutes-document"><div class="minutes-document-title"><span>${t("meetingDocument")}</span><h1>${esc(snap.meeting.title)}</h1></div>${rows
        .map(
          (s, index) =>
            `<section class="minutes-section${s.stale ? " needs-review" : ""}" aria-label="${esc(s.focus || t("meetingDocument"))}">${s.text
              .split(/\n\s*\n/)
              .filter((p) => p.trim())
              .map(
                (p) =>
                  `<p>${esc(p.trim().replace(/([^\n])\n(?=[^\n])/g, "$1 "))}<sup class="citation"><button data-citation="${esc(s.id)}" aria-label="${tr("查看引用", "View source", "查看引用")} ${index + 1}${s.stale ? ` · ${t("stale")}` : ""}" aria-haspopup="dialog" title="${tr("查看原话、回听或修改本段", "View source, listen or edit this paragraph", "查看原話、回聽或修改本段")}">${index + 1}${s.stale ? "*" : ""}</button></sup></p>`,
              )
              .join("")}</section>`,
        )
        .join(
          "",
        )}<div class="document-colophon">${tr("点击上标查看原话、回听或修改段落", "Select a superscript to view sources, listen or edit a paragraph", "點擊上標查看原話、回聽或修改段落")}${rows.some((s) => s.stale) ? `<span class="citation-review-note">* ${t("stale")}</span>` : ""}</div></article>`
    : `<div class="empty-suggestions">${icon("note")}<p>${t("emptySuggestions")}</p><small>${t("emptySuggestionsSub")}</small></div>`;
}
function showCitation(id: string) {
  const rows = snap?.suggestions.filter((s) => s.status !== "dismissed") || [];
  const s = rows.find((s) => s.id === id);
  if (!s) return;
  modal(
    `${tr("引用", "Source", "引用")} ${rows.indexOf(s) + 1}`,
    `<div class="citation-context"><p>${esc(s.text)}</p>${s.status === "accepted" ? `<small>${icon("check")}${t("protected")}</small>` : ""}${s.stale ? `<p class="stale">${t("stale")}</p>` : ""}</div><div class="citation-sources">${s.sources.map((q) => `<blockquote><div class="citation-source-meta"><button class="timestamp" data-seek="${q.start_ms}" data-end="${q.end_ms}" aria-label="${t("play")} ${clock(q.start_ms)}">${icon("play")}${clock(q.start_ms)}</button>${q.speaker_name ? `<span>${esc(q.speaker_name)}</span>` : ""}</div><p>${esc(q.quote)}</p></blockquote>`).join("")}</div>`,
    `<button class="subtle" data-edit-suggestion="${esc(s.id)}">${icon("edit")}${t("editLock")}</button><button class="subtle" data-dismiss="${esc(s.id)}">${t("dismiss")}</button>`,
  );
}
function renderTranscript() {
  if (!snap) return;
  const status = document.querySelector<HTMLElement>("#translation-status");
  if (status) {
    status.hidden = snap.translation_mode === "off";
    status.textContent = snap.runtime.translation_status?.startsWith(
      "translation:deferred",
    )
      ? tr(
          "翻译暂缓，原文已保留",
          "Translation paused; original kept",
          "翻譯暫緩，已保留原文",
        )
      : snap.runtime.translation_busy
        ? tr("本地翻译中…", "Translating locally…", "本機翻譯中…")
        : tr("本地翻译", "Local translation", "本機翻譯");
  }
  const select = document.querySelector<HTMLSelectElement>("#translation-mode");
  if (select && document.activeElement !== select)
    select.value = snap.translation_mode || "off";
  const query = $<HTMLInputElement>("#search").value.toLowerCase().trim();
  const partial =
    snap.runtime.meeting_id === current ? snap.runtime.partial : "";
  const sig =
    JSON.stringify(snap.segments) +
    JSON.stringify(snap.meeting.speaker_names) +
    JSON.stringify(snap.translations) +
    snap.translation_mode +
    partial +
    query;
  if (sig === transcriptSignature) return;
  transcriptSignature = sig;
  const pane = $("#transcript");
  const nearBottom =
    pane.scrollHeight - pane.scrollTop - pane.clientHeight < 90;
  const rows = snap.segments.filter((s) =>
    (
      s.text +
      " " +
      (snap!.translations?.[s.id]?.text || "") +
      " " +
      speakerName(s, snap!.meeting)
    )
      .toLowerCase()
      .includes(query),
  );
  $("#segment-count").textContent = String(snap.segments.length);
  pane.innerHTML =
    rows
      .map(
        (s) =>
          `<article class="segment"><div class="segment-top"><button class="timestamp" data-seek="${s.start_ms}" data-end="${s.end_ms}">${clock(s.start_ms)}</button><button class="speaker-chip" data-speaker-segment="${s.id}">${esc(speakerName(s, snap!.meeting))}</button><button class="segment-edit" data-edit-segment="${s.id}" title="${t("edit")}" aria-label="${t("edit")}">${icon("edit")}</button></div>${passageHTML(s, snap!)}</article>`,
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
  closeModal();
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
  if (!dialog.open) {
    dialog.showModal();
    reveal(dialog);
  }
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
  dock?.dispose();
  dock = null;
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
  renderMeetingTabs();
  $("#window-panels").hidden = true;
  document
    .querySelectorAll<HTMLElement>("[data-page]")
    .forEach((b) =>
      b.classList.toggle("selected", b.dataset.page === destination),
    );
  const root = $("#workspace");
  root.classList.remove("dock-transcript", "workbench");
  root.innerHTML = `<div class="page-loading">${tr("正在打开…", "Opening…", "正在開啟…")}</div>`;
  if (destination === "settings") {
    pageHandle = await mountSettings(root, {
      toast,
      section,
      profile,
      back: async () => {
        await refreshMeetings();
        if (!current && meetings[0]) current = meetings[0].id;
        await navigate("meetings");
      },
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

function closeViewMenu(focus = false) {
  const menu = document.getElementById("view-menu");
  if (menu) menu.hidden = true;
  const trigger = document.querySelector<HTMLButtonElement>(
    "[data-action='workspace-view']",
  );
  trigger?.setAttribute("aria-expanded", "false");
  if (focus) trigger?.focus();
}
function renderViewMenu() {
  const menu = document.getElementById("view-menu");
  if (!menu || !viewSettings) return;
  const visible = {
    notebook: !viewSettings.notebook_collapsed,
    minutes: !viewSettings.minutes_collapsed,
    transcript: viewSettings.transcript_mode === "docked",
  };
  menu.innerHTML = `<div class="view-menu-caption">${tr("布局", "LAYOUTS", "佈局")}</div>${[
    [
      "default",
      tr("默认 · 笔记与纪要", "Default · Notes & minutes", "預設 · 筆記與紀要"),
    ],
    ["notes", tr("专注笔记", "Focus on notes", "專注筆記")],
    ["minutes", tr("阅读纪要", "Read minutes", "閱讀紀要")],
    ["meeting", tr("三栏对照", "Three-panel review", "三欄對照")],
  ]
    .map(
      ([id, label]) =>
        `<button role="menuitem" data-layout="${id}">${label}</button>`,
    )
    .join(
      "",
    )}${(dock?.saved() || []).map((l) => `<button role="menuitem" data-saved-layout="${l.id}">${esc(l.name)}${l.default ? " ★" : ""}</button>`).join("")}<div class="view-menu-divider"></div><button role="menuitem" data-action="save-layout">${tr("新建 / 管理布局…", "Save / manage layouts…", "新增 / 管理佈局…")}</button><div class="view-menu-caption">${tr("显示面板", "SHOW PANELS", "顯示面板")}</div>${(["notebook", "minutes"] as const).map((panel) => `<button role="menuitemcheckbox" aria-checked="${visible[panel]}" data-panel="${panel}">${icon("note")}${t(panelLabels[panel])}<span class="view-check">${visible[panel] ? icon("check") : ""}</span></button>`).join("")}<button role="menuitem" data-action="float-transcript">${tr("独立转写窗口", "Separate transcript window", "獨立轉寫視窗")}</button><button role="menuitem" data-action="display-settings">${tr("字幕与显示设置…", "Caption & display settings…", "字幕及顯示設定…")}</button>`;
}
document.addEventListener("focusin", (event) => {
  if (!(event.target as HTMLElement).closest("#window-panels")) closeViewMenu();
});

function applyViews() {
  if (!viewSettings) return;
  const states: Record<Panel, boolean> = {
    sidebar: viewSettings.sidebar_collapsed,
    notebook: viewSettings.notebook_collapsed,
    minutes: viewSettings.minutes_collapsed,
    transcript: viewSettings.transcript_mode !== "docked",
  };
  for (const [panel, collapsed] of Object.entries(states)) {
    if (panel !== "sidebar") continue;
    const control = document.querySelector<HTMLButtonElement>(
      ".window-toolbar [data-panel='sidebar']",
    );
    control?.setAttribute("aria-expanded", String(!collapsed));
    const content = $("#sidebar-content");
    content.hidden = collapsed;
    content.parentElement!.classList.toggle("is-collapsed", collapsed);
    const rail = document.querySelector<HTMLElement>("[data-rail='sidebar']");
    if (rail) rail.hidden = !collapsed;
  }
  if (dock) {
    for (const id of ["notebook", "minutes", "transcript"] as DockPanel[]) {
      const wanted = !states[id];
      if (wanted && !dock.has(id)) dock.show(id, false);
      if (!wanted && dock.has(id)) dock.hide(id);
    }
  }
  renderViewMenu();
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
  updateLiveButtons();
}
function updateLiveButtons() {
  if (!viewSettings) return;
  const transcript =
    document.querySelector<HTMLButtonElement>(".transcript-toggle");
  if (transcript) {
    const visible = viewSettings.transcript_mode !== "hidden";
    transcript.setAttribute("aria-pressed", String(visible));
    transcript.setAttribute("aria-expanded", String(visible));
    transcript.classList.toggle("enabled", visible);
  }
  const captions = document.querySelector<HTMLButtonElement>(".caption-toggle");
  captions?.setAttribute("aria-pressed", String(viewSettings.captions.enabled));
  captions?.classList.toggle("enabled", viewSettings.captions.enabled);
}
async function toggleTranscript(open?: boolean) {
  const p = await getViewSettings();
  const shown = open ?? p.transcript_mode === "hidden";
  if (
    !shown &&
    p.transcript_mode === "docked" &&
    p.notebook_collapsed &&
    p.minutes_collapsed
  )
    return;
  viewSettings = await patchViewSettings({
    transcript_mode: shown ? "floating" : "hidden",
    snap: "free",
  });
  applyViews();
}
async function dockTranscript() {
  viewSettings = await patchViewSettings({ transcript_mode: "docked" });
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
  await dockTranscript();
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
  return navigate("settings", "shortcuts");
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
    snap?.runtime.translation_busy ||
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
  if (!(e.target as HTMLElement).closest("#window-panels")) closeViewMenu();
  const el = (e.target as HTMLElement).closest<HTMLElement>("button");
  if (!el) return;
  if (el.closest(".meeting-more"))
    el.closest("details")?.removeAttribute("open");
  guard(async () => {
    if (el.dataset.closeTab) {
      const id = el.dataset.closeTab;
      if (
        id === snap?.runtime.meeting_id &&
        (snap.runtime.recording || snap.runtime.paused)
      ) {
        toast(
          tr(
            "录音中会保留此标签，可切换其他会议。",
            "This tab stays open while recording. You can switch to another meeting.",
            "錄音期間保留此分頁，可切換其他會議。",
          ),
        );
        return;
      }
      openTabs = openTabs.filter((t) => t !== id);
      if (current === id) {
        await flushNotes();
        const next = openTabs.at(-1);
        if (next) return selectMeeting(next);
        current = null;
        await leavePage();
        shell();
      }
      renderMeetingTabs();
      return;
    }
    if (el.dataset.layout) {
      if (viewSettings?.transcript_mode === "floating")
        viewSettings = await patchViewSettings({ transcript_mode: "hidden" });
      dock?.preset(el.dataset.layout);
      closeViewMenu();
      return;
    }
    if (el.dataset.savedLayout) {
      dock?.open(el.dataset.savedLayout);
      closeViewMenu();
      return;
    }
    if (el.dataset.makeDefault) {
      dock?.makeDefault(el.dataset.makeDefault);
      closeModal();
      renderViewMenu();
      return;
    }
    if (el.dataset.removeLayout) {
      if (
        await confirmDelete(
          tr("删除此布局？", "Delete this layout?", "刪除此佈局？"),
          tr(
            "只删除布局，不影响会议和笔记。",
            "Only the layout is removed. Meetings and notes are kept.",
            "僅刪除佈局，保留會議及筆記。",
          ),
        )
      ) {
        dock?.remove(el.dataset.removeLayout);
        closeModal();
        renderViewMenu();
      }
      return;
    }
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
    if (el.dataset.meeting) {
      closeModal();
      return selectMeeting(el.dataset.meeting);
    }
    if (el.dataset.citation) return showCitation(el.dataset.citation);
    if (el.dataset.seek !== undefined)
      return play(
        Number(el.dataset.seek),
        el.dataset.end ? Number(el.dataset.end) : undefined,
      );
    if (el.dataset.accept || el.dataset.dismiss) {
      if (
        el.dataset.dismiss &&
        !(await confirmDelete(
          tr("删除这段纪要？", "Delete this paragraph?", "刪除此段紀要？"),
          tr(
            "原始录音和转写仍会保留。",
            "The original recording and transcript will be kept.",
            "原始錄音及轉寫仍會保留。",
          ),
        ))
      )
        return;
      await invoke("update_suggestion", {
        meeting: current,
        id: el.dataset.accept || el.dataset.dismiss,
        status: el.dataset.accept ? "accepted" : "dismissed",
        text: null,
      });
      closeModal();
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
      case "delete-meeting": {
        const id = el.dataset.id!;
        if (
          !(await confirmDelete(
            tr("删除会议？", "Delete meeting?", "刪除會議？"),
            tr(
              "会议及笔记将移入已删除列表，可在设置的存储空间中恢复。",
              "Move this meeting and its notes to Deleted. Restore it in Settings → Storage.",
              "會議及筆記移入已刪除列表，可在設定的儲存空間中還原。",
            ),
          ))
        )
          return;
        await flushNotes();
        await invoke("set_meeting_deleted", { id, deleted: true });
        if (current === id) {
          await leavePage();
          current = "";
          snap = null;
        }
        await refreshMeetings();
        if (meetings[0]) await selectMeeting(meetings[0].id);
        else shell();
        return;
      }
      case "delete-note": {
        if (
          !snap ||
          !(await confirmDelete(
            tr("删除这份笔记？", "Delete this note?", "刪除此筆記？"),
            tr(
              "笔记可在左侧「笔记 → 已删除」中恢复，录音与纪要保留。",
              "Restore it from Notes → Deleted. Recording and minutes stay intact.",
              "可在筆記的已刪除列表還原，錄音及紀要保留。",
            ),
          ))
        )
          return;
        await flushNotes();
        const note = snap.notes.find((n) => n.kind === "document");
        const entries =
          await invoke<import("./library").LibraryEntry[]>("list_library");
        const entry = entries.find((e) => e.id === note?.id);
        if (entry)
          await invoke("save_library_entry", {
            entry: { ...entry, deleted: true },
          });
        return current ? selectMeeting(current) : undefined;
      }
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
      case "quick-record": {
        const status = await invoke<{
          runtime: {
            recording: boolean;
            paused: boolean;
            meeting_id: string | null;
          };
        } | null>("transcript_snapshot");
        if (
          (status?.runtime.recording || status?.runtime.paused) &&
          status.runtime.meeting_id
        )
          return selectMeeting(status.runtime.meeting_id);
        await newMeeting();
        await recording();
        return;
      }
      case "new-tab":
        modal(
          tr("打开会议", "Open a meeting", "開啟會議"),
          `<div class="tab-picker">${meetings.map((m) => `<button data-meeting="${esc(m.id)}">${icon("note")}${esc(m.title)}</button>`).join("")}</div>`,
          `<button class="primary" data-action="new">${t("newMeeting")}</button>`,
        );
        return;
      case "save-layout":
        closeViewMenu();
        modal(
          tr("保存与管理布局", "Save and manage layouts", "儲存及管理佈局"),
          `<p class="setting-help">${tr("拖动面板标签即可停靠或合并；按住 Shift 拖动可以浮动。也可使用面板 ⋯ 菜单。", "Drag panel tabs to dock or combine them. Shift-drag to float, or use each panel’s ⋯ menu.", "拖曳面板分頁可停靠或合併，Shift 拖曳可浮動，也可使用面板 ⋯ 選單。")}</p><label>${tr("布局名称", "Layout name", "佈局名稱")}<input id="layout-name" maxlength="40" placeholder="${tr("我的布局", "My layout", "我的佈局")}"></label><div class="saved-layouts">${(dock?.saved() || []).map((l) => `<div><span>${esc(l.name)}</span><button data-make-default="${l.id}">${l.default ? "★" : tr("设为默认", "Set as default", "設為預設")}</button><button data-remove-layout="${l.id}">${glyph("trash")}</button></div>`).join("")}</div>`,
          `<button class="primary" data-action="confirm-save-layout">${tr("保存当前布局", "Save current layout", "儲存目前佈局")}</button>`,
        );
        return;
      case "confirm-save-layout":
        dock?.save($<HTMLInputElement>("#layout-name").value);
        closeModal();
        renderViewMenu();
        toast(
          tr(
            "布局已保存在本机",
            "Layout saved on this device",
            "佈局已儲存於本機",
          ),
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
      case "workspace-view": {
        const menu = $("#view-menu");
        const open = menu.hidden;
        menu.hidden = !open;
        el.setAttribute("aria-expanded", String(open));
        if (open)
          menu
            .querySelector<HTMLButtonElement>("button:not(:disabled)")
            ?.focus();
        return;
      }
      case "toggle-transcript":
        return toggleTranscript();
      case "float-transcript":
        closeViewMenu();
        viewSettings = await patchViewSettings({
          transcript_mode: "floating",
          compact: false,
          ...(viewSettings?.notebook_collapsed &&
          viewSettings?.minutes_collapsed
            ? { notebook_collapsed: false }
            : {}),
        });
        applyViews();
        return;
      case "display-settings":
        closeViewMenu();
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
document.addEventListener("change", (event) => {
  const input = event.target as HTMLSelectElement;
  if (input.id === "translation-mode")
    guard(async () => {
      input.disabled = true;
      try {
        const prefs = await getPreferences();
        prefs.translation_mode = input.value as typeof prefs.translation_mode;
        await savePreferences(prefs);
        await refresh();
      } finally {
        input.disabled = false;
      }
    });
});
document.addEventListener("keydown", (e) => {
  if (e.isComposing || e.keyCode === 229) return;
  const menu = document.querySelector<HTMLElement>("#view-menu:not([hidden])");
  if (
    menu &&
    ["Escape", "ArrowDown", "ArrowUp", "Home", "End"].includes(e.key)
  ) {
    e.preventDefault();
    if (e.key === "Escape") {
      closeViewMenu(true);
      return;
    }
    const buttons = Array.from(
      menu.querySelectorAll<HTMLButtonElement>("button:not(:disabled)"),
    );
    const i = buttons.indexOf(document.activeElement as HTMLButtonElement);
    const next =
      e.key === "Home"
        ? 0
        : e.key === "End"
          ? buttons.length - 1
          : (i + (e.key === "ArrowUp" ? -1 : 1) + buttons.length) %
            buttons.length;
    buttons[next]?.focus();
    return;
  }
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
      await dockTranscript();
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
  await listen<string>("companion-meeting", (e) =>
    guard(async () => {
      await refreshMeetings();
      await selectMeeting(e.payload);
    }),
  );
  await listen<string>("companion-error", (e) => toast(e.payload, true));
  shell();
  await refreshMeetings();
  diag = await invoke<Diagnostic>("diagnostics");

  if (meetings.length) {
    const last = localStorage.getItem("omascribe-active-meeting");
    await selectMeeting(
      meetings.find((m) => m.id === last)?.id || meetings[0].id,
    );
  }
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

document.addEventListener("keydown", (event) => {
  const mod = /Mac/.test(navigator.platform) ? event.metaKey : event.ctrlKey;
  if (!mod || event.altKey) return;
  if (event.key.toLowerCase() === "w" && page === "meeting" && current) {
    event.preventDefault();
    document
      .querySelector<HTMLButtonElement>(
        `[data-close-tab="${CSS.escape(current)}"]`,
      )
      ?.click();
  }
  if (event.key.toLowerCase() === "t") {
    event.preventDefault();
    document
      .querySelector<HTMLButtonElement>("[data-action='new-tab']")
      ?.click();
  }
  if (event.key === "Tab" && openTabs.length) {
    event.preventDefault();
    const i = openTabs.indexOf(current || "");
    guard(() =>
      selectMeeting(
        openTabs[
          (i + (event.shiftKey ? -1 : 1) + openTabs.length) % openTabs.length
        ],
      ),
    );
  }
});
