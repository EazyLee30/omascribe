import { uiIcon } from "./ui-icons";
import { tr } from "./workspace-ui";
import { passageHTML, translationSelect } from "./translation-ui";
import { getPreferences, savePreferences } from "./preferences-client";
import { invoke } from "@tauri-apps/api/core";
import { listen, emit } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  getViewSettings,
  setViewSettings,
  patchViewSettings,
  type ViewPreferences,
} from "./views-client";
import type { Snapshot } from "./types";
import { t } from "./i18n";
import "./style.css";
import "./workspace.css";
import "./floating.css";
import { initAppearance } from "./preferences-client";
import { speakerName } from "./speaker-ui";
const esc = (value: string) =>
  value.replace(
    /[&<>"']/g,
    (c) =>
      ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[
        c
      ]!,
  );
const time = (ms: number) => {
  const n = Math.floor(ms / 1000);
  return [Math.floor(n / 3600), Math.floor((n / 60) % 60), n % 60]
    .map((n) => String(n).padStart(2, "0"))
    .join(":");
};
let prefs: ViewPreferences,
  snapshot: Snapshot | null = null,
  signature = "",
  busy = false,
  audioUrl: string | null = null;
const root = document.querySelector<HTMLElement>("#app")!;
function error(e: unknown) {
  const el = document.querySelector<HTMLElement>("#float-error")!;
  if (el) {
    el.textContent = String(e);
    el.hidden = false;
  }
}
function apply(p: ViewPreferences) {
  prefs = p;
  document.documentElement.style.setProperty(
    "--transcript-font",
    `${p.transcript_font}px`,
  );
  document.body.classList.toggle("compact", p.compact);
  document.querySelector<HTMLElement>("#float-collapse")!.title = t(
    p.compact ? "expand" : "collapse",
  );
}
async function render() {
  if (busy || prefs.transcript_mode !== "floating") return;
  busy = true;
  try {
    snapshot = await invoke<Snapshot | null>("transcript_snapshot");
    const pane = document.querySelector<HTMLElement>("#float-transcript")!;
    if (!snapshot) {
      pane.innerHTML = `<div class="empty-transcript"><p>${t("emptyTranscript")}</p></div>`;
      return;
    }
    document.querySelector("#float-title")!.textContent =
      snapshot.meeting.title;
    const own = snapshot.runtime.meeting_id === snapshot.meeting.id;
    const recording = own && snapshot.runtime.recording,
      paused = own && snapshot.runtime.paused;
    root.dataset.recording = recording
      ? "recording"
      : paused
        ? "paused"
        : "ready";
    document.querySelector("#float-elapsed")!.textContent = time(
      snapshot.meeting.duration_ms,
    );
    const pause = root.querySelector<HTMLButtonElement>(
      "[data-action='pause']",
    )!;
    pause.hidden = !recording && !paused;
    pause.innerHTML = uiIcon(paused ? "play" : "pause");
    pause.title = paused ? t("resume") : t("pause");
    pause.setAttribute("aria-label", pause.title);
    (root.querySelector("[data-action='stop']") as HTMLElement).hidden =
      !recording && !paused;
    (root.querySelector("[data-action='record']") as HTMLElement).hidden =
      recording || paused;
    root.querySelector<HTMLSelectElement>("#float-translation-mode")!.value =
      snapshot.translation_mode;
    root
      .querySelector("[data-action='captions']")!
      .setAttribute("aria-pressed", String(prefs.captions.enabled));
    document.querySelector("#float-state")!.textContent = paused
      ? t("paused")
      : recording
        ? t("live")
        : t("transcript");
    const query = (
      document.querySelector<HTMLInputElement>("#float-search")!.value || ""
    ).toLocaleLowerCase();
    const partial =
      snapshot.runtime.meeting_id === snapshot.meeting.id
        ? snapshot.runtime.partial
        : "";
    const next =
      JSON.stringify(snapshot.segments) +
      JSON.stringify(snapshot.meeting.speaker_names) +
      JSON.stringify(snapshot.translations) +
      snapshot.translation_mode +
      partial +
      query;
    if (next === signature) return;
    signature = next;
    const bottom = pane.scrollHeight - pane.scrollTop - pane.clientHeight < 80;
    pane.innerHTML =
      snapshot.segments
        .filter((s) =>
          (s.text + " " + (snapshot?.translations?.[s.id]?.text || ""))
            .toLocaleLowerCase()
            .includes(query),
        )
        .map(
          (s) =>
            `<article class="segment"><button class="timestamp" data-start="${s.start_ms}" data-end="${s.end_ms}">${time(s.start_ms)}</button><span class="speaker-chip">${esc(speakerName(s, snapshot!.meeting))}</span>${passageHTML(s, snapshot!)}</article>`,
        )
        .join("") +
      (!query && partial
        ? `<article class="segment partial"><span class="partial-label">${t("live")}</span><p>${esc(partial)}</p></article>`
        : "");
    document.querySelector("#float-preview")!.textContent =
      partial || snapshot.segments.at(-1)?.text || t("emptyTranscript");
    if (bottom && !query) pane.scrollTop = pane.scrollHeight;
  } catch (e) {
    error(e);
  } finally {
    busy = false;
  }
}
async function init() {
  await initAppearance();
  document.body.classList.add("floating-transcript-app");
  root.innerHTML = `<header class="float-header"><div class="float-drag" data-tauri-drag-region title="${tr("拖动独立浮窗", "Drag this window", "拖曳獨立浮窗")}">${uiIcon("grip", 14)}<span class="float-status-dot"></span><span id="float-elapsed">00:00:00</span></div><div class="float-record-controls"><button data-action="record" title="${t("record")}" aria-label="${t("record")}">${uiIcon("mic")}</button><button data-action="pause" title="${t("pause")}" aria-label="${t("pause")}" hidden>${uiIcon("pause")}</button><button data-action="stop" class="float-stop" hidden>${uiIcon("stop", 14)}${tr("结束", "End", "結束")}</button></div><span class="float-separator"></span><label class="float-language" title="${tr("本地翻译 · 保留原文", "On-device translation · Original kept", "本機翻譯 · 保留原文")}">${uiIcon("translate", 16)}${translationSelect("float-translation-mode")}</label><button data-action="captions" title="${tr("实时字幕", "Live captions", "即時字幕")}" aria-label="${tr("实时字幕", "Live captions", "即時字幕")}" aria-pressed="false">${uiIcon("captions")}</button><button data-action="maximize" title="${tr("最大化 / 还原", "Maximize / restore", "最大化 / 還原")}" aria-label="${tr("最大化 / 还原", "Maximize / restore", "最大化 / 還原")}">${uiIcon("maximize")}</button><details class="float-more"><summary aria-label="${tr("更多操作", "More actions", "更多操作")}">${uiIcon("more")}</summary><div class="float-more-menu"><button data-action="dock">${uiIcon("dock")}${tr("停靠到主窗口", "Dock in main window", "停靠至主視窗")}</button><button data-action="search">${uiIcon("search")}${tr("搜索转写", "Search transcript", "搜尋轉寫")}</button><button id="float-collapse" data-action="collapse">${uiIcon("collapse")}${tr("收起 / 展开", "Collapse / expand", "收起 / 展開")}</button><label>${tr("屏幕位置", "Screen position", "螢幕位置")}<select id="float-snap" aria-label="${t("snapTo")}"><option value="free">${t("freePosition")}</option><option value="left">${t("screenLeft")}</option><option value="right">${t("screenRight")}</option></select></label><button data-action="settings">${tr("字幕与显示设置…", "Caption & display settings…", "字幕及顯示設定…")}</button></div></details><button data-action="close" title="${t("close")}" aria-label="${t("close")}">${uiIcon("close", 16)}</button></header><div class="float-context"><span id="float-state"></span><small id="float-title">OmaScribe</small></div><p id="float-preview"></p><div class="float-body"><div class="float-tools" hidden><input id="float-search" placeholder="${t("search")}" aria-label="${t("search")}"></div><div id="float-error" role="alert" hidden></div><div id="float-transcript"></div><audio id="float-player" controls hidden></audio><button class="float-return" data-action="return">${uiIcon("return", 16)}${tr("回到会议", "Back to meeting", "回到會議")}</button></div><span class="float-resize" role="separator" aria-label="${t("resizeWindow")}"></span>`;
  const mode = root.querySelector<HTMLSelectElement>(
    "#float-translation-mode",
  )!;
  mode.value = (await getPreferences()).translation_mode;
  mode.onchange = async () => {
    try {
      const p = await getPreferences();
      p.translation_mode = mode.value as typeof p.translation_mode;
      await savePreferences(p);
      await render();
    } catch (e) {
      error(e);
    }
  };
  apply(await getViewSettings());
  (document.querySelector("#float-snap") as HTMLSelectElement).value =
    prefs.snap;
  root
    .querySelector(".float-resize")!
    .addEventListener("mousedown", () =>
      getCurrentWindow().startResizeDragging("SouthEast").catch(error),
    );
  root.addEventListener("click", async (event) => {
    const button = (event.target as HTMLElement).closest<HTMLElement>("button");
    if (!button) return;
    try {
      button.closest("details")?.removeAttribute("open");
      switch (button.dataset.action) {
        case "record":
          if (snapshot)
            await invoke("start_recording", {
              id: snapshot.meeting.id,
              device: localStorage.getItem("omascribe-device") || "auto",
            });
          break;
        case "pause":
          await invoke(
            snapshot?.runtime.paused ? "resume_recording" : "pause_recording",
          );
          break;
        case "stop":
          await invoke("stop_recording");
          break;
        case "captions":
          await patchViewSettings({
            captions: { ...prefs.captions, enabled: !prefs.captions.enabled },
          });
          break;
        case "maximize":
          await getCurrentWindow().toggleMaximize();
          break;
        case "return":
          await invoke("show_main_window");
          break;
        case "search": {
          const tools = root.querySelector<HTMLElement>(".float-tools")!;
          tools.hidden = !tools.hidden;
          if (!tools.hidden)
            root.querySelector<HTMLInputElement>("#float-search")!.focus();
          break;
        }

        case "dock":
          await patchViewSettings({ transcript_mode: "docked" });
          await invoke("show_main_window");
          return;
        case "close":
          await patchViewSettings({ transcript_mode: "hidden" });
          return;
        case "collapse":
          await patchViewSettings({ compact: !prefs.compact });
          return;
        case "settings":
          await emit("request-display-settings");
          return;
      }
      if (button.dataset.start && snapshot) {
        const encoded = await invoke<string>("audio_clip", {
          id: snapshot.meeting.id,
          startMs: Number(button.dataset.start),
          endMs: Math.min(
            Number(button.dataset.end),
            Number(button.dataset.start) + 90000,
          ),
        });
        if (audioUrl) URL.revokeObjectURL(audioUrl);
        audioUrl = URL.createObjectURL(
          new Blob([Uint8Array.from(atob(encoded), (c) => c.charCodeAt(0))], {
            type: "audio/wav",
          }),
        );
        const player =
          document.querySelector<HTMLAudioElement>("#float-player")!;
        player.src = audioUrl;
        player.hidden = false;
        await player.play();
      }
    } catch (e) {
      error(e);
    }
  });
  root.querySelector("#float-search")!.addEventListener("input", () => {
    signature = "";
    render();
  });
  root.querySelector("#float-snap")!.addEventListener("change", () =>
    patchViewSettings({
      snap: (root.querySelector("#float-snap") as HTMLSelectElement)
        .value as ViewPreferences["snap"],
    }).catch(error),
  );
  await listen<ViewPreferences>("view-settings", (event) =>
    apply(event.payload),
  );
  await listen("view-meeting", () => {
    signature = "";
    render();
  });
  await render();
  setInterval(render, 700);
}
init().catch(error);
