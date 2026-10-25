import type { Editor } from "@tiptap/core";
import { computePosition, flip, offset, shift } from "@floating-ui/dom";
import { t, type Key } from "./i18n";
import { tr, esc } from "./workspace-ui";
import { uiIcon } from "./ui-icons";
interface SlashCommand {
  id: string;
  label: Key;
  hint: Key;
  icon: Parameters<typeof uiIcon>[0];
  search: string;
  shortcut?: string;
  group: "basic" | "meeting" | "advanced";
}
const commands: SlashCommand[] = [
  {
    id: "paragraph",
    label: "paragraph",
    hint: "plainTextHint",
    icon: "text",
    search: "text paragraph 正文 文字",
    group: "basic",
  },
  {
    id: "heading1",
    label: "headingOne",
    hint: "headingHint",
    icon: "heading1",
    search: "h1 heading title 标题 標題",
    shortcut: "#",
    group: "basic",
  },
  {
    id: "heading2",
    label: "headingTwo",
    hint: "headingHint",
    icon: "heading2",
    search: "h2 heading section 标题 標題",
    shortcut: "##",
    group: "basic",
  },
  {
    id: "heading3",
    label: "headingThree",
    hint: "headingHint",
    icon: "heading3",
    search: "h3 heading subsection 标题 標題",
    shortcut: "###",
    group: "basic",
  },
  {
    id: "bulletList",
    label: "bulletList",
    hint: "bulletHint",
    icon: "list",
    search: "bullet list unordered 列表 清單",
    shortcut: "-",
    group: "basic",
  },
  {
    id: "orderedList",
    label: "orderedList",
    hint: "numberedHint",
    icon: "numbered",
    search: "numbered ordered list 有序 编号 編號",
    shortcut: "1.",
    group: "basic",
  },
  {
    id: "taskList",
    label: "taskList",
    hint: "taskHint",
    icon: "task",
    search: "todo task checkbox check 待办 待辦 任务 任務",
    shortcut: "[]",
    group: "basic",
  },
  {
    id: "blockquote",
    label: "quote",
    hint: "quoteHint",
    icon: "quote",
    search: "quote blockquote 引用",
    shortcut: ">",
    group: "basic",
  },
  {
    id: "codeBlock",
    label: "code",
    hint: "codeHint",
    icon: "code",
    search: "code block 代码 程式碼",
    shortcut: "```",
    group: "basic",
  },
  {
    id: "divider",
    label: "divider",
    hint: "dividerHint",
    icon: "divider",
    search: "line divider horizontal 分隔线 分隔線",
    shortcut: "---",
    group: "basic",
  },
  {
    id: "template",
    label: "meetingTemplate",
    hint: "templateHint",
    icon: "template",
    search: "meeting template agenda 会议 會議 模板",
    group: "meeting",
  },
  {
    id: "timestamp",
    label: "timestamp",
    hint: "timestampHint",
    icon: "clock",
    search: "time timestamp audio 时间 時間 回听 回聽",
    group: "meeting",
  },
  {
    id: "keyword",
    label: "keyword",
    hint: "keywordHint",
    icon: "bold",
    search: "keyword focus bold 关键词 關鍵字 重点 重點",
    group: "meeting",
  },
  {
    id: "date",
    label: "today",
    hint: "dateHint",
    icon: "date",
    search: "today date 日期 今天",
    group: "meeting",
  },
  {
    id: "table",
    label: "table",
    hint: "tableHint",
    icon: "table",
    search: "table grid 表格",
    group: "advanced",
  },
  {
    id: "link",
    label: "link",
    hint: "plainTextHint",
    icon: "link",
    search: "link 链接 連結",
    group: "advanced",
  },
  {
    id: "source",
    label: "markdownSource",
    hint: "plainTextHint",
    icon: "source",
    search: "markdown source 源码 原始碼",
    group: "advanced",
  },
];
let menuSequence = 0;
export class SlashMenu {
  private popup = document.createElement("div");
  private list = document.createElement("div");
  private search = document.createElement("input");
  private matches: SlashCommand[] = [];
  private selected = 0;
  private start: number | null = null;
  private dismissed: number | null = null;
  private query = "";
  private manual: { from: number; anchor: HTMLElement } | null = null;
  private dispose = new AbortController();
  private positionVersion = 0;
  constructor(
    private editor: () => Editor,
    private run: (id: string, from: number, to: number) => void,
  ) {
    this.popup.className = "slash-menu";
    this.popup.hidden = true;
    this.list.className = "slash-options";
    this.list.id = `slash-options-${++menuSequence}`;
    this.list.setAttribute("role", "listbox");
    this.list.setAttribute("aria-label", t("slashCommands"));
    this.search.className = "slash-search";
    this.search.placeholder = tr("搜索块类型…", "Search blocks…", "搜尋區塊…");
    this.search.setAttribute("aria-label", this.search.placeholder);
    this.search.setAttribute("role", "combobox");
    this.search.setAttribute("aria-controls", this.list.id);
    this.search.setAttribute("aria-expanded", "true");
    const footer = document.createElement("button");
    footer.className = "slash-close";
    footer.innerHTML = `<span>${tr("关闭菜单", "Close menu", "關閉選單")}</span><kbd>esc</kbd>`;
    footer.addEventListener("click", () => this.dismiss());
    this.popup.append(this.search, this.list, footer);
    document.body.append(this.popup);
    this.popup.addEventListener("mousedown", (e) => {
      if (e.target !== this.search) e.preventDefault();
    });
    this.list.addEventListener("click", (e) => {
      const button = (e.target as HTMLElement).closest<HTMLElement>(
        "[data-command]",
      );
      if (button) this.execute(button.dataset.command!);
    });
    this.list.addEventListener("pointermove", (e) => {
      const button = (e.target as HTMLElement).closest<HTMLElement>(
        "[data-command]",
      );
      if (!button) return;
      this.selected = this.matches.findIndex(
        (c) => c.id === button.dataset.command,
      );
      this.highlight(false);
    });
    this.search.addEventListener("input", (e) => {
      if ((e as InputEvent).isComposing) return;
      this.query = this.search.value.toLocaleLowerCase();
      this.selected = 0;
      this.render();
    });
    this.search.addEventListener("compositionend", () => {
      this.query = this.search.value.toLocaleLowerCase();
      this.selected = 0;
      this.render();
    });
    this.popup.addEventListener("keydown", (e) => {
      this.keydown(e);
    });
    document.addEventListener(
      "pointerdown",
      (e) => {
        if (!this.popup.hidden && !this.popup.contains(e.target as Node))
          this.dismiss(false);
      },
      { signal: this.dispose.signal },
    );
    window.addEventListener("resize", () => this.position(), {
      signal: this.dispose.signal,
    });
    document.addEventListener(
      "scroll",
      (e) => {
        if (!this.popup.hidden && !this.popup.contains(e.target as Node))
          this.hide();
      },
      { capture: true, signal: this.dispose.signal },
    );
  }
  openForBlock(from: number, anchor: HTMLElement) {
    this.hide();
    this.manual = { from, anchor };
    this.query = "";
    this.selected = 0;
    this.search.value = "";
    this.render();
    this.search.focus();
  }
  update(composing = false) {
    const editor = this.editor();
    if (!editor || this.manual) return;
    if (composing) {
      this.hide();
      return;
    }
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
    if (start !== this.start || query !== this.query) this.selected = 0;
    this.start = start;
    this.query = query;
    this.render();
  }
  private render() {
    const query = this.query.trim();
    const filtered = commands.filter((c) =>
      `${t(c.label)} ${c.search}`.toLocaleLowerCase().includes(query),
    );
    const suggested = ["template", "taskList", "heading1"];
    const groups = query
      ? [{ title: tr("搜索结果", "Results", "搜尋結果"), items: filtered }]
      : [
          {
            title: tr("建议", "Suggested", "建議"),
            items: suggested.map((id) => commands.find((c) => c.id === id)!),
          },
          ...(["basic", "meeting", "advanced"] as const).map((group, i) => ({
            title: [
              tr("基础块", "Basic blocks", "基礎區塊"),
              tr("会议工具", "Meeting tools", "會議工具"),
              tr("更多", "More", "更多"),
            ][i],
            items: filtered.filter(
              (c) => c.group === group && !suggested.includes(c.id),
            ),
          })),
        ];
    this.matches = groups.flatMap((g) => g.items);
    this.selected = Math.max(
      0,
      Math.min(this.selected, this.matches.length - 1),
    );
    this.search.hidden = !this.manual;
    this.popup.hidden = false;
    this.list.innerHTML =
      groups
        .filter((g) => g.items.length)
        .map(
          (g) =>
            `<div class="slash-group"><div class="slash-title">${g.title}</div>${g.items.map((c) => `<button type="button" role="option" id="${this.list.id}-${c.id}" data-command="${c.id}" title="${esc(t(c.hint))}" tabindex="-1"><span class="slash-glyph">${uiIcon(c.icon, 20)}</span><span class="slash-label">${esc(t(c.label))}</span><kbd>${esc(c.shortcut || "")}</kbd></button>`).join("")}</div>`,
        )
        .join("") ||
      `<div class="slash-empty" role="status">${tr("没有匹配的块", "No matching blocks", "沒有相符的區塊")}</div>`;
    this.highlight(true);
    this.position();
  }
  private highlight(scroll: boolean) {
    this.list
      .querySelectorAll<HTMLElement>("[data-command]")
      .forEach((button, i) =>
        button.setAttribute("aria-selected", String(i === this.selected)),
      );
    const selected = this.list.querySelector<HTMLElement>(
      '[aria-selected="true"]',
    );
    const input = this.manual ? this.search : this.editor().view.dom;
    if (selected) input.setAttribute("aria-activedescendant", selected.id);
    else input.removeAttribute("aria-activedescendant");
    this.editor().view.dom.setAttribute("aria-controls", this.list.id);
    if (scroll && selected) {
      const top = selected.offsetTop,
        bottom = top + selected.offsetHeight;
      if (top < this.list.scrollTop) this.list.scrollTop = top;
      else if (bottom > this.list.scrollTop + this.list.clientHeight)
        this.list.scrollTop = bottom - this.list.clientHeight;
    }
  }
  private position() {
    if (this.popup.hidden) return;
    const version = ++this.positionVersion;
    const coords = this.editor().view.coordsAtPos(
      this.editor().state.selection.from,
    );
    const reference = this.manual?.anchor || {
      getBoundingClientRect: () =>
        new DOMRect(coords.left, coords.top, 1, coords.bottom - coords.top),
    };
    void computePosition(reference, this.popup, {
      strategy: "fixed",
      placement: "bottom-start",
      middleware: [offset(8), flip(), shift({ padding: 10 })],
    }).then(({ x, y }) => {
      if (version === this.positionVersion && !this.popup.hidden)
        Object.assign(this.popup.style, { left: `${x}px`, top: `${y}px` });
    });
  }
  keydown(event: KeyboardEvent): boolean {
    if (this.popup.hidden || event.isComposing || event.keyCode === 229)
      return false;
    if (event.key === "Escape") {
      this.dismiss();
      event.preventDefault();
      event.stopPropagation();
      return true;
    }
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      if (this.matches.length)
        this.selected =
          (this.selected +
            (event.key === "ArrowDown" ? 1 : -1) +
            this.matches.length) %
          this.matches.length;
      this.highlight(true);
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
      if (!this.matches.length) return false;
      event.preventDefault();
      event.stopPropagation();
      this.execute(this.matches[this.selected].id);
      return true;
    }
    if (
      !this.manual &&
      (event.key === " " ||
        event.key === "/" ||
        (event.key === "Enter" && event.shiftKey))
    )
      this.dismiss(false);
    return false;
  }
  private execute(id: string) {
    const from = this.manual?.from ?? this.start;
    if (from === null) return;
    const to = this.manual ? from : this.editor().state.selection.from;
    this.hide();
    this.dismissed = null;
    this.run(id, from, to);
  }
  private dismiss(focus = true) {
    this.dismissed = this.start;
    const manual = !!this.manual;
    this.hide();
    if (focus && manual) this.editor().commands.focus();
  }
  hide() {
    this.popup.hidden = true;
    this.start = null;
    this.manual = null;
    this.positionVersion++;
    this.editor()?.view.dom.removeAttribute("aria-activedescendant");
    this.editor()?.view.dom.removeAttribute("aria-controls");
  }
  destroy() {
    this.dispose.abort();
    this.hide();
    this.popup.remove();
  }
}
