import type { Editor } from "@tiptap/core";
import { t, type Key } from "./i18n";
export interface SlashCommand {
  id: string;
  label: Key;
  hint: Key;
  glyph: string;
  search: string;
}
const commands: SlashCommand[] = [
  {
    id: "source",
    label: "markdownSource",
    hint: "plainTextHint",
    glyph: "MD",
    search: "markdown source 源码 原始碼",
  },
  {
    id: "link",
    label: "link",
    hint: "plainTextHint",
    glyph: "↗",
    search: "link 链接 連結",
  },
  {
    id: "paragraph",
    label: "paragraph",
    hint: "plainTextHint",
    glyph: "¶",
    search: "text paragraph 正文 文字",
  },
  {
    id: "heading1",
    label: "headingOne",
    hint: "headingHint",
    glyph: "H1",
    search: "h1 heading title 标题 標題",
  },
  {
    id: "heading2",
    label: "headingTwo",
    hint: "headingHint",
    glyph: "H2",
    search: "h2 heading section 标题 標題",
  },
  {
    id: "heading3",
    label: "headingThree",
    hint: "headingHint",
    glyph: "H3",
    search: "h3 heading subsection 标题 標題",
  },
  {
    id: "bulletList",
    label: "bulletList",
    hint: "bulletHint",
    glyph: "•",
    search: "bullet list unordered 列表 清單",
  },
  {
    id: "orderedList",
    label: "orderedList",
    hint: "numberedHint",
    glyph: "1.",
    search: "numbered ordered list 有序 编号 編號",
  },
  {
    id: "taskList",
    label: "taskList",
    hint: "taskHint",
    glyph: "☐",
    search: "todo task checkbox check 待办 待辦 任务 任務",
  },
  {
    id: "blockquote",
    label: "quote",
    hint: "quoteHint",
    glyph: "❝",
    search: "quote blockquote 引用",
  },
  {
    id: "codeBlock",
    label: "code",
    hint: "codeHint",
    glyph: "<>",
    search: "code block 代码 程式碼",
  },
  {
    id: "table",
    label: "table",
    hint: "tableHint",
    glyph: "▦",
    search: "table grid 表格",
  },
  {
    id: "divider",
    label: "divider",
    hint: "dividerHint",
    glyph: "—",
    search: "line divider horizontal 分隔线 分隔線",
  },
  {
    id: "keyword",
    label: "keyword",
    hint: "keywordHint",
    glyph: "K",
    search: "keyword focus bold 关键词 關鍵字 重点 重點",
  },
  {
    id: "timestamp",
    label: "timestamp",
    hint: "timestampHint",
    glyph: "◷",
    search: "time timestamp audio 时间 時間 回听 回聽",
  },
  {
    id: "date",
    label: "today",
    hint: "dateHint",
    glyph: "□",
    search: "today date 日期 今天",
  },
  {
    id: "template",
    label: "meetingTemplate",
    hint: "templateHint",
    glyph: "▤",
    search: "meeting template agenda 会议 會議 模板",
  },
];
export class SlashMenu {
  private popup = document.createElement("div");
  private matches: SlashCommand[] = [];
  private selected = 0;
  private start: number | null = null;
  private dismissed: number | null = null;
  private query = "";
  constructor(
    private editor: () => Editor,
    private run: (id: string, from: number, to: number) => void,
  ) {
    this.popup.className = "slash-menu";
    this.popup.hidden = true;
    this.popup.setAttribute("role", "listbox");
    this.popup.setAttribute("aria-label", t("slashCommands"));
    document.body.append(this.popup);
    this.popup.addEventListener("mousedown", (event) => event.preventDefault());
    this.popup.addEventListener("click", (event) => {
      const button = (event.target as HTMLElement).closest<HTMLElement>(
        "[data-command]",
      );
      if (button) this.execute(button.dataset.command!);
    });
  }
  update(composing = false) {
    const editor = this.editor();
    if (!editor || composing) return;
    const { selection } = editor.state;
    const { $from } = selection;
    if (!selection.empty || editor.isActive("codeBlock")) {
      this.hide();
      return;
    }
    const prefix = $from.parent.textBetween(
      0,
      $from.parentOffset,
      "\n",
      "\ufffc",
    );
    const match = prefix.match(/^(\s*)\/([^\s/]*)$/);
    if (!match) {
      this.dismissed = null;
      this.hide();
      return;
    }
    const start = $from.start() + match[1].length;
    if (this.dismissed === start) {
      this.hide();
      return;
    }
    const query = match[2].toLocaleLowerCase();
    this.matches = commands.filter((c) =>
      `${t(c.label)} ${c.search}`.toLocaleLowerCase().includes(query),
    );
    if (!this.matches.length) {
      this.hide();
      return;
    }
    if (start !== this.start || query !== this.query) this.selected = 0;
    this.start = start;
    this.query = query;
    this.selected = Math.min(this.selected, this.matches.length - 1);
    this.render();
  }
  private render() {
    this.popup.hidden = false;
    this.popup.innerHTML = `<div class="slash-title">${t("slashCommands")}</div>${this.matches.map((c, i) => `<button role="option" aria-selected="${i === this.selected}" data-command="${c.id}"><span class="slash-glyph">${c.glyph}</span><span><strong>${t(c.label)}</strong><small>${t(c.hint)}</small></span></button>`).join("")}<div class="slash-foot">${t("slashKeepText")}</div>`;
    const coords = this.editor().view.coordsAtPos(
      this.editor().state.selection.from,
    );
    this.popup.style.left = `${Math.max(12, Math.min(coords.left, innerWidth - 300))}px`;
    this.popup.style.top = `${Math.max(12, Math.min(coords.bottom + 8, innerHeight - this.popup.offsetHeight - 15))}px`;
    this.popup
      .querySelector("[aria-selected=true]")
      ?.scrollIntoView({ block: "nearest" });
  }
  keydown(event: KeyboardEvent): boolean {
    if (this.popup.hidden || event.isComposing || event.keyCode === 229)
      return false;
    if (event.key === "Escape") {
      this.dismissed = this.start;
      this.hide();
      event.preventDefault();
      event.stopPropagation();
      return true;
    }
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      this.selected =
        (this.selected +
          (event.key === "ArrowDown" ? 1 : -1) +
          this.matches.length) %
        this.matches.length;
      this.render();
      event.preventDefault();
      event.stopPropagation();
      return true;
    }
    if (
      event.key === "Enter" &&
      !event.shiftKey &&
      !event.ctrlKey &&
      !event.metaKey
    ) {
      event.preventDefault();
      event.stopPropagation();
      this.execute(this.matches[this.selected].id);
      return true;
    }
    if (
      event.key === " " ||
      event.key === "/" ||
      (event.key === "Enter" && event.shiftKey)
    ) {
      this.dismissed = this.start;
      this.hide();
    }
    return false;
  }
  private execute(id: string) {
    if (this.start === null) return;
    const from = this.start,
      to = this.editor().state.selection.from;
    this.hide();
    this.dismissed = null;
    this.run(id, from, to);
  }
  hide() {
    this.popup.hidden = true;
    this.start = null;
  }
  destroy() {
    this.popup.remove();
  }
}
