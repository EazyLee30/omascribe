import { listen } from "@tauri-apps/api/event";
import "./caption.css";

type Preferences = {
  font_size: number;
  lines: number;
  text_color: string;
  background_color: string;
  opacity: number;
  radius: number;
  align: string;
};
const caption = document.querySelector<HTMLDivElement>("#caption")!;
void listen<{ text: string; preferences: Preferences }>(
  "caption-text",
  ({ payload }) => {
    const p = payload.preferences;
    caption.replaceChildren();
    const lines = payload.text.split("\n");
    caption.classList.toggle("caption-bilingual", lines.length > 1);
    for (const text of lines) {
      const line = document.createElement("div");
      line.className = "caption-line";
      line.textContent = text;
      caption.append(line);
    }
    caption.style.fontSize = `${p.font_size}px`;
    caption.style.color = p.text_color;
    caption.style.backgroundColor = `${p.background_color}${Math.round(
      p.opacity * 255,
    )
      .toString(16)
      .padStart(2, "0")}`;
    caption.style.borderRadius = `${p.radius}px`;
    caption.style.textAlign = p.align;
    caption.style.setProperty("--lines", String(p.lines));
  },
);
