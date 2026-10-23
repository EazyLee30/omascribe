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
  el.textContent = String(e);
  el.hidden = false;
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
    document.querySelector("#float-state")!.textContent = snapshot.runtime
      .paused
      ? t("paused")
      : snapshot.runtime.recording
        ? t("live")
        : t("transcript");
    const query = (
      document.querySelector<HTMLInputElement>("#float-search")!.value || ""
    ).toLocaleLowerCase();
    const partial =
      snapshot.runtime.meeting_id === snapshot.meeting.id
        ? snapshot.runtime.partial
        : "";
    const next = JSON.stringify(snapshot.segments) + partial + query;
    if (next === signature) return;
    signature = next;
    const bottom = pane.scrollHeight - pane.scrollTop - pane.clientHeight < 80;
    pane.innerHTML =
      snapshot.segments
        .filter((s) => s.text.toLocaleLowerCase().includes(query))
        .map(
          (s) =>
            `<article class="segment"><button class="timestamp" data-start="${s.start_ms}" data-end="${s.end_ms}">${time(s.start_ms)}</button><p>${esc(s.text)}</p></article>`,
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
  document.body.classList.add("floating-transcript-app");
  root.innerHTML = `<header class="float-header"><div class="float-drag"><span class="float-status-dot"></span><div><strong id="float-state">${t("transcript")}</strong><small id="float-title">OmaScribe</small></div></div><div class="float-actions"><button data-action="dock" title="${t("dockRight")}" aria-label="${t("dockRight")}">⇥</button><button data-action="settings" title="${t("displaySettings")}" aria-label="${t("displaySettings")}">⚙</button><button id="float-collapse" data-action="collapse" aria-label="${t("collapse")}">⌃</button><button data-action="close" title="${t("close")}" aria-label="${t("close")}">×</button></div></header><p id="float-preview"></p><div class="float-body"><div class="float-tools"><input id="float-search" placeholder="${t("search")}" aria-label="${t("search")}"><select id="float-snap" aria-label="${t("snapTo")}"><option value="free">${t("freePosition")}</option><option value="left">${t("screenLeft")}</option><option value="right">${t("screenRight")}</option></select></div><div id="float-error" role="alert" hidden></div><div id="float-transcript"></div><audio id="float-player" controls hidden></audio></div><span class="float-resize" role="separator" aria-label="${t("resizeWindow")}">◢</span>`;
  apply(await getViewSettings());
  (document.querySelector("#float-snap") as HTMLSelectElement).value =
    prefs.snap;
  root.querySelector(".float-drag")!.addEventListener("mousedown", (e) => {
    if ((e as MouseEvent).button === 0) invoke("drag_transcript").catch(error);
  });
  root
    .querySelector(".float-resize")!
    .addEventListener("mousedown", () =>
      getCurrentWindow().startResizeDragging("SouthEast").catch(error),
    );
  root.addEventListener("click", async (event) => {
    const button = (event.target as HTMLElement).closest<HTMLElement>("button");
    if (!button) return;
    try {
      switch (button.dataset.action) {
        case "dock":
          await patchViewSettings({ transcript_mode: "docked" });
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
  root
    .querySelector("#float-snap")!
    .addEventListener("change", () =>
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
