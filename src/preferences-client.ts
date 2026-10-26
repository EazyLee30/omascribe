import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
export interface Appearance {
  mode: string;
  theme: string;
  accent: string;
  background: string;
  foreground: string;
  font: string;
  font_size: number;
}
export interface Preferences {
  appearance: Appearance;
  asr_model: string;
  llm_model: string;
  threads: number;
  context_size: number;
  inference_backend: "auto" | "cpu" | "metal" | "mlx";
  resource_mode: "eco" | "balanced" | "quality";
  memory_budget_mib: number;
  llm_keep_alive_seconds: number;
  auto_notes: boolean;
  translation_mode: "off" | "bilingual" | "zh-en" | "en-zh";
  minutes_language: "auto" | "zh" | "en";
  diarization: boolean;
  speaker_count: number;
  use_knowledge: boolean;
  use_memory: boolean;
}
export interface Profile {
  schema: number;
  name: string;
  hardware_hint: string;
  preferences: Preferences;
}
export interface NetworkPolicy {
  allow_model_downloads: boolean;
  allow_online_recommendations: boolean;
  allow_update_checks: boolean;
}
export interface ModelStatus {
  model: {
    id: string;
    name: string;
    kind: string;
    format: "onnx" | "gguf" | "mlx";
    license: string;
    source: string;
    memory_mib: number;
    bundled: boolean;
    files: { bytes: number }[];
    archive: { bytes: number } | null;
  };
  installed: boolean;
  installed_bytes: number;
  managed: boolean;
  selected: boolean;
  download: {
    state: string;
    downloaded: number;
    total: number;
    error: string | null;
  } | null;
}
export interface Hardware {
  platform: string;
  gpus: {
    cuda_driver_version?: number | null;
    schema: number;
    status: string;
    error: string | null;
    devices: {
      local_id: string;
      name: string;
      vendor: string;
      dedicated_memory_bytes: number | null;
      budget_bytes: number | null;
      budget_source: string | null;
    }[];
  };
  os: string;
  arch: string;
  cpu: string;
  cores: number;
  total_memory_mib: number;
  available_memory_mib: number | null;
  free_disk_bytes: number;
  tier: string;
  recommended: Preferences;
  apple_silicon: boolean;
  backends: string[];
  resource_presets: { id: string; name: string; preferences: Preferences }[];
}
export const defaultAppearance: Appearance = {
  mode: "system",
  theme: "neutral",
  accent: "#d83b36",
  background: "",
  foreground: "",
  font: "system",
  font_size: 13,
};
let current: Preferences | null = null;
const darkQuery = matchMedia("(prefers-color-scheme: dark)");
export function applyAppearance(a: Appearance) {
  const root = document.documentElement;
  const dark = a.mode === "dark" || (a.mode === "system" && darkQuery.matches);
  const palettes: Record<string, string[]> = {
    forest: dark
      ? [
          "#191d18",
          "#151914",
          "#20251e",
          "#2b3227",
          "#e5eadf",
          "#a6b29b",
          "#3d4935",
        ]
      : [
          "#fbfcf8",
          "#f1f3ec",
          "#ffffff",
          "#e9eee2",
          "#222a1e",
          "#606f55",
          "#d8dfd0",
        ],
    neutral: dark
      ? [
          "#1c1c1f",
          "#17171a",
          "#242427",
          "#303034",
          "#eeeef0",
          "#aaaab1",
          "#3d3d43",
        ]
      : [
          "#ffffff",
          "#f5f5f7",
          "#ffffff",
          "#eeeef1",
          "#1a1c1f",
          "#6b6d73",
          "#e4e4e8",
        ],
    slate: dark
      ? [
          "#171e29",
          "#121924",
          "#202a38",
          "#2a3748",
          "#e3eaf4",
          "#9eafc4",
          "#36465b",
        ]
      : [
          "#f8fafc",
          "#eef2f7",
          "#ffffff",
          "#e4ebf4",
          "#1d2939",
          "#5e6f85",
          "#d5deea",
        ],
  };
  const palette = palettes[a.theme] || palettes.forest;
  ["bg", "sidebar", "panel", "surface", "text", "muted", "border"].forEach(
    (key, i) => root.style.setProperty(`--${key}`, palette[i]),
  );
  if (a.background) root.style.setProperty("--bg", a.background);
  if (a.foreground) root.style.setProperty("--text", a.foreground);
  root.style.setProperty("--accent", a.accent);
  const rgb = a.accent
    .slice(1)
    .match(/../g)!
    .map((v) => parseInt(v, 16) / 255)
    .map((v) => (v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4));
  root.style.setProperty(
    "--accent-ink",
    rgb[0] * 0.2126 + rgb[1] * 0.7152 + rgb[2] * 0.0722 > 0.179
      ? "#14161a"
      : "#ffffff",
  );
  root.style.setProperty("--green", dark ? "#b1c69e" : "#40663c");
  root.style.setProperty("--red", dark ? "#edb0a4" : "#a33329");
  root.style.setProperty(
    "--app-font",
    a.font === "serif"
      ? 'Georgia, "Songti SC", serif'
      : a.font === "mono"
        ? 'ui-monospace, "SFMono-Regular", "Noto Sans CJK SC", monospace'
        : '-apple-system, BlinkMacSystemFont, "Segoe UI", "Noto Sans CJK SC", sans-serif',
  );
  root.style.setProperty("--app-font-size", `${a.font_size}px`);
  root.style.colorScheme = dark ? "dark" : "light";
  root.dataset.mode = dark ? "dark" : "light";
}
export async function getPreferences() {
  current = await invoke<Preferences>("get_preferences");
  return structuredClone(current);
}
export async function savePreferences(preferences: Preferences) {
  current = await invoke<Preferences>("set_preferences", { preferences });
  applyAppearance(current.appearance);
  return structuredClone(current);
}
export async function initAppearance() {
  const p = await getPreferences();
  applyAppearance(p.appearance);
  darkQuery.addEventListener("change", () => {
    if (current) applyAppearance(current.appearance);
  });
  await listen<Preferences>("app-preferences", (event) => {
    current = event.payload;
    applyAppearance(current.appearance);
  });
}
