import { locale } from "./i18n";
export const tr = (zh: string, en: string, tw?: string) =>
  locale === "en" ? en : locale === "zh-TW" ? tw || zh : zh;
export const esc = (s: unknown) =>
  String(s ?? "").replace(
    /[&<>"']/g,
    (c) =>
      ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[
        c
      ]!,
  );
export const bytes = (n: number) =>
  n >= 1024 ** 3
    ? `${(n / 1024 ** 3).toFixed(1)} GiB`
    : `${(n / 1024 ** 2).toFixed(1)} MiB`;
const paths: Record<string, string> = {
  appearance:
    "M12 3a9 9 0 1 0 0 18h1a2 2 0 0 0 0-4h-1a2 2 0 0 1 0-4h6a3 3 0 0 0 3-3c0-4-4-7-9-7ZM7 9h.01M10 6h.01M15 6h.01",
  updates:
    "M20 7v5h-5M4 17v-5h5M6.1 7a7 7 0 0 1 11.6-2L20 8M4 16l2.3 3A7 7 0 0 0 18 17",
  models: "M7 7h10v10H7zM9 2v5m6-5v5M9 17v5m6-5v5M2 9h5m-5 6h5m10-6h5m-5 6h5",
  general:
    "M12 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8ZM12 2v3m0 14v3M2 12h3m14 0h3M5 5l2 2m10 10 2 2M5 19l2-2M17 7l2-2",
  transcription:
    "M9 6a3 3 0 0 1 6 0v6a3 3 0 0 1-6 0V6ZM5 11v1a7 7 0 0 0 14 0v-1M12 19v3",
  storage: "M3 7V4h6l3 3h9v13H3V7Z",
  privacy: "M12 3 3 7v5c0 5 4 8 9 10 5-2 9-5 9-10V7l-9-4Zm-4 9 3 3 5-6",
  profiles:
    "M15 8 9 11m6 5-6-3M18 3a3 3 0 1 0 0 6 3 3 0 0 0 0-6ZM6 9a3 3 0 1 0 0 6 3 3 0 0 0 0-6Zm12 6a3 3 0 1 0 0 6 3 3 0 0 0 0-6Z",
  meetings: "M5 4h14v17H5V4ZM8 2v4m8-4v4M5 9h14M9 13h6m-6 4h4",
  notes: "M5 3h14v14l-4 4H5V3ZM9 8h6m-6 4h6m-1 5v4",
  knowledge: "M3 4h7l2 2 2-2h7v15h-7l-2 2-2-2H3V4Zm9 2v15",
  terms: "M3 18 8 4l5 14M5 13h6m4-6h6m-3-3v14m-3-5 6 5m0-5-6 5",
  memory:
    "M8 4a3 3 0 0 0-4 4 4 4 0 0 0 0 7 4 4 0 0 0 8 4V6a3 3 0 0 0-4-2Zm8 0a3 3 0 0 1 4 4 4 4 0 0 1 0 7 4 4 0 0 1-8 4M6 9l3 2m9-2-3 2M6 16l3-2m9 2-3-2",
  speakers:
    "M9 3a3 3 0 1 0 0 6 3 3 0 0 0 0-6ZM3 21v-5a6 6 0 0 1 12 0v5m2-17a3 3 0 0 1 0 6m1 3a5 5 0 0 1 3 5v3",
  back: "m14 5-7 7 7 7",
  plus: "M12 5v14M5 12h14",
  check: "m5 12 4 4L19 6",
  download: "M12 3v13m-5-5 5 5 5-5M4 17v4h16v-4",
  upload: "M12 16V3M7 8l5-5 5 5M4 16v4h16v-4",
  trash: "M3 6h18M9 6V3h6v3M5 6l1 15h12l1-15M10 10v7m4-7v7",
  search: "M10 4a6 6 0 1 0 0 12 6 6 0 0 0 0-12Zm4 10 6 6",
};
export const glyph = (name: string) =>
  `<svg width="19" height="19" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="${paths[name] || paths.general}"/></svg>`;
export interface PageHandle {
  flush?(): Promise<void>;
  close(): Promise<void>;
}
export const navItems = () => [
  ["meetings", tr("会议", "Meetings", "會議")],
  ["notes", tr("笔记", "Notes", "筆記")],
  ["knowledge", tr("知识库", "Knowledge base", "知識庫")],
  ["terms", tr("专有名词表", "Glossary", "專有名詞表")],
  ["memory", tr("记忆", "Memory", "記憶")],
];
