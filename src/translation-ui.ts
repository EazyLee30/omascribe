import type { Segment, Snapshot } from "./types";
import { esc, tr } from "./workspace-ui";
export const translationChoices = () => [
  ["off", tr("原文", "Original", "原文")],
  ["bilingual", tr("双语", "Bilingual", "雙語")],
  ["zh-en", tr("中译英", "ZH → EN", "中譯英")],
  ["en-zh", tr("英译中", "EN → ZH", "英譯中")],
];
export function translationSelect(id: string, value = "off") {
  return `<select id="${id}" class="translation-select" aria-label="${tr("实时翻译模式", "Live translation mode", "即時翻譯模式")}" title="${tr("本地翻译 · 原始转写保留", "Local translation · Original transcript preserved", "本機翻譯 · 保留原始轉寫")}">${translationChoices()
    .map(
      ([key, label]) =>
        `<option value="${key}" ${key === value ? "selected" : ""}>${label}</option>`,
    )
    .join("")}</select>`;
}
export function passageHTML(segment: Segment, snapshot: Snapshot) {
  const mode = snapshot.translation_mode;
  const translated = snapshot.translations?.[segment.id];
  if (!mode || mode === "off") return `<p>${esc(segment.text)}</p>`;
  if (translated && mode !== "bilingual")
    return `<p class="translated-text"><small>${tr("译文", "Translation", "譯文")} · ${translated.target.toUpperCase()}</small>${esc(translated.text)}</p><details class="original-text"><summary>${tr("查看原文", "View original", "查看原文")}</summary><p>${esc(segment.text)}</p></details>`;
  return `<p>${esc(segment.text)}</p>${translated ? `<p class="translated-text"><small>${tr("译文", "Translation", "譯文")} · ${translated.target.toUpperCase()}</small>${esc(translated.text)}</p>` : ""}`;
}
