import { bindWindowDrag, macDesktop } from "./window-drag";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { initAppearance } from "./preferences-client";
import {
  getViewSettings,
  patchViewSettings,
  type ViewPreferences,
} from "./views-client";
import { uiIcon } from "./ui-icons";
import { tr } from "./workspace-ui";
import { translationSelect } from "./translation-ui";
import { getPreferences, savePreferences } from "./preferences-client";
import type { Snapshot } from "./types";
const atlas = new URL(
  "../assets/companion/notebook-mecha-atlas-v1.png",
  import.meta.url,
).href;
import "./companion.css";

const root = document.querySelector<HTMLElement>("#companion")!;
type PetSnapshot = {
  meeting: Pick<Snapshot["meeting"], "id" | "title">;
  runtime: Pick<Snapshot["runtime"], "recording" | "paused" | "elapsed_ms">;
  translation_mode: Snapshot["translation_mode"];
};
let prefs: ViewPreferences,
  snap: PetSnapshot | null = null;
let expanded = false,
  busy = false,
  working = false,
  hovering = false,
  step = 0;
const clock = (ms: number) => {
  const s = Math.floor(ms / 1000);
  return `${Math.floor(s / 60)
    .toString()
    .padStart(2, "0")}:${(s % 60).toString().padStart(2, "0")}`;
};
async function expand(value: boolean) {
  await invoke("set_companion_expanded", { expanded: value });
  expanded = value;
  root.classList.toggle("expanded", value);
  root.querySelector("#pet")!.setAttribute("aria-expanded", String(value));
  root.querySelector<HTMLButtonElement>("#pet-grip")!.hidden = !value;
  if (value) root.querySelector<HTMLButtonElement>("#pet-start")!.focus();
}
function pose() {
  const status = snap?.runtime;
  const recording = !!status?.recording,
    paused = !!status?.paused;
  const idle = [0, 0, 0, 1, 0, 0, 2, 0];
  const index =
    !prefs?.companion_animation ||
    matchMedia("(prefers-reduced-motion: reduce)").matches
      ? paused
        ? 7
        : recording
          ? 4
          : 0
      : hovering
        ? 6
        : paused
          ? 7
          : recording
            ? [4, 5][step % 2]
            : idle[step % idle.length];
  const sprite = root.querySelector<HTMLElement>(".pet-sprite")!;
  sprite.style.backgroundPosition = `${((index % 4) * 100) / 3}% ${index >= 4 ? 100 : 0}%`;
  root.classList.toggle("animated", !!prefs?.companion_animation);
}
function controls() {
  const r = snap?.runtime;
  const recording = !!r?.recording,
    paused = !!r?.paused;
  root.dataset.state = recording ? "recording" : paused ? "paused" : "idle";
  root.querySelector("#pet-time")!.textContent =
    recording || paused ? clock(r!.elapsed_ms) : "";
  root.querySelector("#pet-status")!.textContent = paused
    ? tr("已暂停", "Paused", "已暫停")
    : recording
      ? tr("录音中", "Recording", "錄音中")
      : tr("准备好了", "Ready", "準備好了");
  root.querySelector("#pet-meeting")!.textContent =
    snap?.meeting.title || tr("新会议", "New meeting", "新會議");
  root.querySelector<HTMLButtonElement>("#pet-start")!.hidden =
    recording || paused;
  root.querySelector<HTMLButtonElement>("#pet-new")!.hidden =
    recording || paused;
  const pause = root.querySelector<HTMLButtonElement>("#pet-pause")!;
  pause.hidden = !recording && !paused;
  pause.innerHTML =
    uiIcon(paused ? "play" : "pause") +
    (paused ? tr("继续", "Resume", "繼續") : tr("暂停", "Pause", "暫停"));
  root.querySelector<HTMLButtonElement>("#pet-stop")!.hidden =
    !recording && !paused;
  if (snap)
    root.querySelector<HTMLSelectElement>("#pet-translation")!.value =
      snap.translation_mode;
  root
    .querySelector("#pet-captions")!
    .setAttribute("aria-pressed", String(prefs.captions.enabled));
  root
    .querySelector("#pet")!
    .setAttribute(
      "aria-label",
      `${tr("OmaScribe 录音小助手", "OmaScribe recording companion", "OmaScribe 錄音小助手")} · ${root.querySelector("#pet-status")!.textContent} · ${tr("点击展开，拖动移动", "Click to open, drag to move", "點擊展開，拖曳移動")}`,
    );
  pose();
}
async function refresh() {
  if (busy || !prefs?.companion_enabled) return;
  busy = true;
  try {
    snap = await invoke<PetSnapshot | null>("companion_snapshot");
    controls();
  } finally {
    busy = false;
  }
}
async function init() {
  await initAppearance();
  prefs = await getViewSettings();
  root.innerHTML = `<section class="pet-panel" aria-label="${tr("录音控制", "Recording controls", "錄音控制")}"><header><span><i></i><b id="pet-status"></b></span><button data-action="collapse" aria-label="${tr("收起", "Collapse", "收起")}">${uiIcon("collapse", 16)}</button></header><p id="pet-meeting"></p><p class="pet-local">${tr("录音、转写和整理均在本机", "Recording, transcription and minutes stay local", "錄音、轉寫及整理皆在本機")}</p><div class="pet-record"><button id="pet-start" data-action="start" class="pet-primary">${uiIcon("mic", 20)}${tr("开始录音", "Start recording", "開始錄音")}</button><button id="pet-pause" data-action="pause" hidden></button><button id="pet-stop" data-action="stop" class="pet-stop" hidden>${uiIcon("stop", 16)}${tr("结束", "End", "結束")}</button></div><div class="pet-views"><button data-action="transcript">${uiIcon("dock", 16)}${tr("实时转写", "Transcript", "即時轉寫")}</button><button id="pet-captions" data-action="captions" aria-pressed="false">${uiIcon("captions", 16)}${tr("字幕", "Captions", "字幕")}</button><label>${uiIcon("translate", 16)}${translationSelect("pet-translation")}</label></div><div class="pet-panel-foot"><button data-action="main">${tr("回到会议", "Open meeting", "開啟會議")}${uiIcon("return", 14)}</button><button id="pet-new" data-action="new">${tr("新建录音", "New recording", "新增錄音")}</button><button data-action="hide">${tr("隐藏助手", "Hide pet", "隱藏助手")}</button></div><p id="pet-error" role="alert" hidden></p></section><div class="pet-character"><button class="pet-hover-hide" data-action="hide" title="${tr("隐藏桌面助手", "Hide desktop pet", "隱藏桌面助手")}" aria-label="${tr("隐藏桌面助手", "Hide desktop pet", "隱藏桌面助手")}">${uiIcon("hide", 12)}${tr("隐藏", "Hide", "隱藏")}</button><button id="pet-grip" data-tauri-drag-region title="${tr("拖动小助手", "Drag companion", "拖曳助手")}" aria-label="${tr("拖动小助手", "Drag companion", "拖曳助手")}" hidden>${uiIcon("grip", 14)}</button><button id="pet" type="button" aria-expanded="false"><span class="pet-sprite" aria-hidden="true"></span><span id="pet-time"></span><span class="pet-rec-dot" aria-hidden="true"></span></button></div>`;
  root.querySelector<HTMLElement>(".pet-sprite")!.style.backgroundImage =
    `url("${atlas}")`;
  const pet = root.querySelector<HTMLButtonElement>("#pet")!;
  const wasDragged = bindWindowDrag(pet, showError);
  if (macDesktop) {
    const grip = root.querySelector<HTMLElement>("#pet-grip")!;
    grip.removeAttribute("data-tauri-drag-region");
    bindWindowDrag(grip, showError);
  }
  pet.addEventListener("pointerenter", () => {
    hovering = true;
    pose();
  });
  pet.addEventListener("pointerleave", () => {
    hovering = false;
    pose();
  });
  pet.onclick = () => {
    if (!wasDragged()) expand(!expanded).catch(showError);
  };
  root.addEventListener("click", async (e) => {
    const action = (e.target as HTMLElement).closest<HTMLElement>(
      "[data-action]",
    )?.dataset.action;
    if (!action || working) return;
    working = true;
    root.querySelector<HTMLElement>("#pet-error")!.hidden = true;
    try {
      switch (action) {
        case "start":
        case "new":
          await invoke("companion_start", {
            device: localStorage.getItem("omascribe-device") || "auto",
            newMeeting: action === "new",
          });
          break;
        case "pause":
          await invoke(
            snap?.runtime.paused ? "resume_recording" : "pause_recording",
          );
          break;
        case "stop":
          await invoke("stop_recording");
          break;
        case "main":
          await invoke("show_main_window");
          break;
        case "collapse":
          await expand(false);
          break;
        case "hide":
          await patchViewSettings({ companion_enabled: false });
          break;
        case "transcript":
          await patchViewSettings({
            transcript_mode: "floating",
            snap: "free",
          });
          break;
        case "captions":
          await patchViewSettings({
            captions: { ...prefs.captions, enabled: !prefs.captions.enabled },
          });
          break;
      }
      await refresh();
    } catch (e) {
      showError(e);
    } finally {
      working = false;
    }
  });
  root.querySelector<HTMLSelectElement>("#pet-translation")!.onchange = async (
    e,
  ) => {
    try {
      const p = await getPreferences();
      p.translation_mode = (e.target as HTMLSelectElement)
        .value as typeof p.translation_mode;
      await savePreferences(p);
    } catch (e) {
      showError(e);
    }
  };
  await listen<ViewPreferences>("view-settings", (e) => {
    prefs = e.payload;
    controls();
  });
  await listen("view-meeting", () => refresh().catch(showError));
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") expand(false).catch(showError);
  });
  await refresh();
  setInterval(() => {
    step++;
    if (prefs.companion_enabled) pose();
  }, 650);
  setInterval(() => refresh().catch(showError), 800);
}
function showError(e: unknown) {
  const error = root.querySelector<HTMLElement>("#pet-error");
  if (error) {
    error.textContent = String(e);
    error.hidden = false;
  }
}
init().catch(showError);
