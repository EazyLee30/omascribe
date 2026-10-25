import { invoke } from "@tauri-apps/api/core";
import { t, type Key } from "./i18n";
export interface CaptionPreferences {
  enabled: boolean;
  position: "top" | "bottom";
  font_size: number;
  lines: number;
  width_percent: number;
  margin: number;
  opacity: number;
  text_color: string;
  background_color: string;
  click_through: boolean;
  idle_seconds: number;
  radius: number;
  align: "left" | "center";
  monitor: number;
}
export interface ViewPreferences {
  sidebar_collapsed: boolean;
  notebook_collapsed: boolean;
  minutes_collapsed: boolean;
  transcript_mode: "hidden" | "docked" | "floating";
  dock_width: number;
  floating_width: number;
  floating_height: number;
  compact: boolean;
  snap: "free" | "left" | "right";
  follow_recording: boolean;
  transcript_font: number;
  captions: CaptionPreferences;
  companion_enabled: boolean;
  companion_animation: boolean;
  companion_anchor: [number, number] | null;
}
export const getViewSettings = () =>
  invoke<ViewPreferences>("get_view_settings");
export const setViewSettings = (preferences: ViewPreferences) =>
  invoke<ViewPreferences>("set_view_settings", { preferences });
export async function patchViewSettings(patch: Partial<ViewPreferences>) {
  return setViewSettings({ ...(await getViewSettings()), ...patch });
}
export async function displaySettings(
  host: HTMLElement,
  onError: (e: unknown) => void,
) {
  let prefs = await getViewSettings();
  const monitors =
    await invoke<{ id: number; name: string; width: number; height: number }[]>(
      "display_monitors",
    );
  const options = (values: [string, Key][], current: string) =>
    values
      .map(
        ([value, label]) =>
          `<option value="${value}" ${current === value ? "selected" : ""}>${t(label)}</option>`,
      )
      .join("");
  const range = (
    key: string,
    label: Key,
    value: number,
    min: number,
    max: number,
    step = 1,
    suffix = "",
  ) =>
    `<label class="display-range"><span>${t(label)}<output>${value}${suffix}</output></span><input data-pref="${key}" type="range" value="${value}" min="${min}" max="${max}" step="${step}"></label>`;
  host.innerHTML = `<div class="display-section"><h3>${t("transcriptWindow")}</h3><label class="setting">${t("displayMode")}<select data-pref="transcript_mode">${options(
    [
      ["hidden", "hidden"],
      ["docked", "dockRight"],
      ["floating", "floating"],
    ],
    prefs.transcript_mode,
  )}</select></label><label class="setting">${t("snapTo")}<select data-pref="snap">${options(
    [
      ["free", "freePosition"],
      ["left", "screenLeft"],
      ["right", "screenRight"],
    ],
    prefs.snap,
  )}</select></label><div class="display-grid">${range("transcript_font", "fontSize", prefs.transcript_font, 11, 28, 1, " px")}${range("dock_width", "panelWidth", prefs.dock_width, 240, 560, 10, " px")}</div><label class="check-setting"><input type="checkbox" data-pref="follow_recording" ${prefs.follow_recording ? "checked" : ""}>${t("followLive")}</label></div><div class="display-section"><div class="display-section-title"><h3>${t("screenCaptions")}</h3><label class="switch"><input type="checkbox" data-pref="captions.enabled" ${prefs.captions.enabled ? "checked" : ""}><span>${t("enabled")}</span></label></div><p class="setting-help">${t("captionHelp")}</p><label class="setting">${t("position")}<select data-pref="captions.position">${options(
    [
      ["top", "screenTop"],
      ["bottom", "screenBottom"],
    ],
    prefs.captions.position,
  )}</select></label><label class="setting">${t("display")}<select data-pref="captions.monitor"><option value="-1">${t("primaryDisplay")}</option>${monitors.map((m) => `<option value="${m.id}" ${prefs.captions.monitor === m.id ? "selected" : ""}>${m.name.replace(/[<>&"']/g, "")} · ${m.width}×${m.height}</option>`).join("")}</select></label><div class="display-grid">${range("captions.font_size", "fontSize", prefs.captions.font_size, 14, 64, 1, " px")}${range("captions.lines", "captionLines", prefs.captions.lines, 1, 5)}${range("captions.width_percent", "captionWidth", prefs.captions.width_percent, 30, 100, 1, "%")}${range("captions.margin", "edgeMargin", prefs.captions.margin, 0, 240, 4, " px")}${range("captions.opacity", "backgroundOpacity", Math.round(prefs.captions.opacity * 100), 0, 100, 1, "%")}${range("captions.radius", "cornerRadius", prefs.captions.radius, 0, 36, 2, " px")}</div><div class="display-grid"><label class="color-setting">${t("textColor")}<input type="color" data-pref="captions.text_color" value="${prefs.captions.text_color}"></label><label class="color-setting">${t("backgroundColor")}<input type="color" data-pref="captions.background_color" value="${prefs.captions.background_color}"></label></div><label class="setting">${t("alignment")}<select data-pref="captions.align">${options(
    [
      ["center", "alignCenter"],
      ["left", "alignLeft"],
    ],
    prefs.captions.align,
  )}</select></label><label class="setting">${t("hideAfter")}<select data-pref="captions.idle_seconds">${[0, 5, 12, 30, 60].map((n) => `<option value="${n}" ${prefs.captions.idle_seconds === n ? "selected" : ""}>${n ? `${n} ${t("seconds")}` : t("never")}</option>`).join("")}</select></label><label class="check-setting"><input type="checkbox" data-pref="captions.click_through" ${prefs.captions.click_through ? "checked" : ""}>${t("clickThrough")}</label><p class="setting-help">${t("captionPrivacy")}</p></div><button class="text-button display-reset">${t("resetDisplay")}</button>`;
  let pending: ReturnType<typeof setTimeout>;
  let queue = Promise.resolve();
  const update = (element: HTMLInputElement | HTMLSelectElement) => {
    const path = element.dataset.pref!;
    let value: unknown =
      element instanceof HTMLInputElement && element.type === "checkbox"
        ? element.checked
        : element.value;
    if (
      element.type === "range" ||
      ["captions.monitor", "captions.idle_seconds"].includes(path)
    )
      value = Number(value);
    if (element.type === "range") {
      const output = element.closest("label")?.querySelector("output");
      if (output)
        output.textContent =
          String(value) +
          (path.includes("opacity") || path.includes("percent")
            ? "%"
            : ["lines"].some((x) => path.endsWith(x))
              ? ""
              : " px");
    }
    if (path === "captions.opacity") value = Number(value) / 100;
    if (path.startsWith("captions."))
      Object.assign(prefs.captions, { [path.slice(9)]: value });
    else Object.assign(prefs, { [path]: value });
    clearTimeout(pending);
    pending = setTimeout(
      () => {
        const snapshot = structuredClone(prefs);
        queue = queue
          .then(async () => {
            prefs = await setViewSettings(snapshot);
          })
          .catch(onError);
      },
      element.type === "range" ? 180 : 0,
    );
  };
  host
    .querySelectorAll<HTMLInputElement | HTMLSelectElement>("[data-pref]")
    .forEach((element) =>
      element.addEventListener(
        element.type === "range" ? "input" : "change",
        () => update(element),
      ),
    );
  host.querySelector(".display-reset")!.addEventListener("click", () => {
    clearTimeout(pending);
    queue = queue
      .then(async () => {
        prefs = {
          sidebar_collapsed: false,
          notebook_collapsed: false,
          minutes_collapsed: false,
          transcript_mode: "hidden",
          dock_width: 320,
          floating_width: 540,
          floating_height: 420,
          compact: false,
          snap: "right",
          follow_recording: true,
          transcript_font: 15,
          companion_enabled: true,
          companion_animation: true,
          companion_anchor: null,
          captions: {
            enabled: false,
            position: "bottom",
            font_size: 28,
            lines: 2,
            width_percent: 80,
            margin: 32,
            opacity: 0.8,
            text_color: "#f5f5f5",
            background_color: "#101010",
            click_through: true,
            idle_seconds: 12,
            radius: 10,
            align: "center",
            monitor: -1,
          },
        };
        await setViewSettings(prefs);
        await displaySettings(host, onError);
      })
      .catch(onError);
  });
}
