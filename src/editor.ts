import { tr } from "./workspace-ui";
import { Editor } from "@tiptap/core";
import StarterKit from "@tiptap/starter-kit";
import { Markdown } from "@tiptap/markdown";
import TaskList from "@tiptap/extension-task-list";
import TaskItem from "@tiptap/extension-task-item";
import Placeholder from "@tiptap/extension-placeholder";
import { t, type Key } from "./i18n";
import { TableKit } from "@tiptap/extension-table";
import { SlashMenu } from "./slash";
import { BlockControls } from "./block-controls";

/** One local Markdown document. Polling never replaces the active editor or its selection. */
export class NotebookEditor {
  editor: Editor;
  private source: HTMLTextAreaElement;
  private toolbar: HTMLElement;
  private sourceMode = false;
  private composing = false;
  private slash: SlashMenu;
  private blocks: BlockControls;
  constructor(
    private host: HTMLElement,
    markdown: string,
    private onChange: (markdown: string) => void,
    private context: { timestamp: () => number; play: (ms: number) => void } = {
      timestamp: () => 0,
      play: () => {},
    },
  ) {
    const mod = /Mac/.test(navigator.platform) ? "⌘" : "Ctrl";
    const buttons: [string, string, Key, string][] = [
      ["bold", "<b>B</b>", "bold", "Ctrl B"],
      ["italic", "<i>I</i>", "italic", "Ctrl I"],
      ["strike", "<s>S</s>", "strike", "Ctrl Shift S"],
      ["bulletList", "≡", "bulletList", "Ctrl Shift 8"],
      ["orderedList", "1.", "orderedList", "Ctrl Shift 7"],
      ["taskList", "☐", "taskList", "Ctrl Shift 9"],
      ["blockquote", "❝", "quote", "Ctrl Shift B"],
      ["codeBlock", "&lt;/&gt;", "code", "Ctrl Alt C"],
      ["link", "↗", "link", "Ctrl K"],
      ["table", "▦", "table", "/table"],
      ["undo", "↶", "undo", "Ctrl Z"],
      ["redo", "↷", "redo", "Ctrl Shift Z"],
    ];
    host.innerHTML = `<div class="editor-toolbar" role="toolbar" aria-label="${t("formatting")}"><select class="editor-heading" aria-label="${t("heading")}"><option value="0">${t("paragraph")}</option><option value="1">H1</option><option value="2">H2</option><option value="3">H3</option></select><span class="toolbar-divider"></span>${buttons
      .filter(([action]) => ["bold", "italic", "bulletList"].includes(action))
      .map(
        ([action, label, key, keys]) =>
          `<button type="button" data-format="${action}" title="${t(key)} · ${keys.replace("Ctrl", mod)}" aria-label="${t(key)}" aria-pressed="false">${label}</button>`,
      )
      .join(
        "",
      )}<details class="format-more"><summary aria-label="${tr("更多格式", "More formatting", "更多格式")}" title="${tr("更多格式", "More formatting", "更多格式")}">•••</summary><div class="format-menu">${buttons
      .filter(([action]) => !["bold", "italic", "bulletList"].includes(action))
      .map(
        ([action, label, key]) =>
          `<button type="button" data-format="${action}" aria-label="${t(key)}" aria-pressed="false"><span aria-hidden="true">${label}</span>${t(key)}</button>`,
      )
      .join(
        "",
      )}</div></details><button class="source-toggle" type="button" data-format="source" title="${t("markdownSource")}" aria-label="${t("markdownSource")}">MD</button></div><div class="table-tools" hidden><span>${t("table")}</span><button data-table="addRowAfter">+ ${t("row")}</button><button data-table="addColumnAfter">+ ${t("column")}</button><button data-table="deleteRow">− ${t("row")}</button><button data-table="deleteColumn">− ${t("column")}</button><button data-table="deleteTable">${t("removeTable")}</button></div><button class="source-exit" type="button" hidden>← ${tr("返回编辑", "Back to editor", "返回編輯")}</button><div class="editor-scroll"><div class="editor-canvas"></div><textarea class="markdown-source" aria-label="${t("markdownSource")}" spellcheck="false" hidden></textarea></div><div class="editor-foot"><span>${t("editorHint")}</span><span class="editor-count"></span></div>`;
    this.source = host.querySelector("textarea")!;
    this.toolbar = host.querySelector(".editor-toolbar")!;
    this.toolbar.classList.add("selection-toolbar");
    this.toolbar.hidden = true;
    document.body.append(this.toolbar);
    this.blocks = new BlockControls(
      () => this.editor,
      (from, anchor) => this.slash.openForBlock(from, anchor),
    );
    this.editor = new Editor({
      element: host.querySelector(".editor-canvas")!,
      extensions: [
        StarterKit.configure({ link: { openOnClick: false, autolink: false } }),
        Markdown,
        this.blocks.extension,
        TableKit.configure({ table: { resizable: true } }),
        TaskList,
        TaskItem.configure({ nested: true }),
        Placeholder.configure({
          placeholder: tr(
            "输入 / 搜索块…",
            "Type / to search blocks…",
            "輸入 / 搜尋區塊…",
          ),
          showOnlyCurrent: true,
        }),
      ],
      content: markdown,
      contentType: "markdown",
      editorProps: {
        attributes: {
          "aria-label": t("yourNotes"),
          "aria-multiline": "true",
          role: "textbox",
          spellcheck: "false",
        },
        handleKeyDown: (_view, event) => {
          if (event.isComposing || event.keyCode === 229) return false;
          if (this.slash?.keydown(event)) return true;
          if ((event.ctrlKey || event.metaKey) && event.key === "/") {
            event.preventDefault();
            this.blocks.openAtSelection();
            return true;
          }
          if (
            (event.ctrlKey || event.metaKey) &&
            event.key.toLowerCase() === "k"
          ) {
            event.preventDefault();
            this.openLink();
            return true;
          }
          return false;
        },
        handleClick: (_view, _pos, event) => {
          const link = (event.target as HTMLElement).closest("a");
          if (link) {
            event.preventDefault();
            const match = link.getAttribute("href")?.match(/^#time=(\d+)$/);
            if (match) this.context.play(Number(match[1]));
            return true;
          }
          return false;
        },
      },
      onUpdate: () => {
        if (!this.composing) this.changed();
      },
      onBlur: () => {
        this.toolbar.hidden = true;
      },
      onSelectionUpdate: () => {
        this.updateToolbar();
        this.slash?.update(this.composing);
      },
    });
    this.slash = new SlashMenu(
      () => this.editor,
      (id, from, to) => this.slashCommand(id, from, to),
    );
    const dom = this.editor.view.dom;
    dom.addEventListener("compositionstart", () => (this.composing = true));
    dom.addEventListener("compositionend", () => {
      this.composing = false;
      setTimeout(() => this.changed(), 0);
    });
    this.toolbar.addEventListener("mousedown", (e) => {
      if ((e.target as HTMLElement).closest("button")) e.preventDefault();
    });
    this.toolbar.addEventListener("click", (e) => {
      const action = (e.target as HTMLElement).closest<HTMLButtonElement>(
        "[data-format]",
      )?.dataset.format;
      if (action) {
        this.format(action);
        this.toolbar.querySelector("details")?.removeAttribute("open");
      }
    });
    this.toolbar.querySelector("select")!.addEventListener("change", (e) => {
      const level = Number((e.target as HTMLSelectElement).value);
      if (level)
        this.editor
          .chain()
          .focus()
          .setHeading({ level: level as 1 | 2 | 3 })
          .run();
      else this.editor.chain().focus().setParagraph().run();
      this.updateToolbar();
    });
    host
      .querySelector(".source-exit")!
      .addEventListener("click", () => this.format("source"));
    this.source.addEventListener("input", () => {
      this.onChange(this.source.value);
      this.count(this.source.value);
    });
    this.source.addEventListener("keydown", (e) => {
      if (e.key === "Tab") {
        e.preventDefault();
        this.source.setRangeText(
          "  ",
          this.source.selectionStart,
          this.source.selectionEnd,
          "end",
        );
        this.source.dispatchEvent(new Event("input"));
      }
    });
    host
      .querySelector(".table-tools")!
      .addEventListener("mousedown", (e) => e.preventDefault());
    host.querySelector(".table-tools")!.addEventListener("click", (event) => {
      const action = (event.target as HTMLElement).closest<HTMLElement>(
        "[data-table]",
      )?.dataset.table;
      const chain = this.editor.chain().focus();
      switch (action) {
        case "addRowAfter":
          chain.addRowAfter().run();
          break;
        case "addColumnAfter":
          chain.addColumnAfter().run();
          break;
        case "deleteRow":
          chain.deleteRow().run();
          break;
        case "deleteColumn":
          chain.deleteColumn().run();
          break;
        case "deleteTable":
          chain.deleteTable().run();
          break;
      }
    });
    this.count(markdown);
    this.updateToolbar();
  }
  private changed() {
    const markdown = this.editor.getMarkdown();
    this.onChange(markdown);
    this.count(markdown);
    this.updateToolbar();
    this.slash?.update(this.composing);
  }
  private count(markdown: string) {
    this.host.querySelector(".editor-count")!.textContent =
      `${[...markdown].length.toLocaleString()} ${t("characters")}`;
  }
  private updateToolbar() {
    if (!this.editor) return;
    this.toolbar
      .querySelectorAll<HTMLButtonElement>("[data-format]")
      .forEach((button) => {
        const action = button.dataset.format!;
        button.setAttribute(
          "aria-pressed",
          String(
            action === "source"
              ? this.sourceMode
              : this.editor.isActive(action),
          ),
        );
        button.disabled = this.sourceMode && action !== "source";
        if (action === "undo")
          button.disabled = this.sourceMode || !this.editor.can().undo();
        if (action === "redo")
          button.disabled = this.sourceMode || !this.editor.can().redo();
      });
    (this.host.querySelector(".table-tools") as HTMLElement).hidden =
      this.sourceMode || !this.editor.isActive("table");
    const select = this.toolbar.querySelector("select")!;
    select.disabled = this.sourceMode;
    select.value = String(this.editor.getAttributes("heading").level || 0);
    const selection = this.editor.state.selection;
    this.toolbar.hidden =
      this.sourceMode || selection.empty || !this.editor.isFocused;
    if (!this.toolbar.hidden) {
      const c = this.editor.view.coordsAtPos(selection.from);
      this.toolbar.style.left = `${Math.max(8, Math.min(innerWidth - this.toolbar.offsetWidth - 8, c.left))}px`;
      this.toolbar.style.top = `${Math.max(8, c.top - this.toolbar.offsetHeight - 8)}px`;
    }
  }
  private format(action: string) {
    if (action === "link") {
      this.openLink();
      return;
    }
    if (action === "source") {
      this.slash.hide();
      this.blocks.close();
      this.sourceMode = !this.sourceMode;
      (this.host.querySelector(".source-exit") as HTMLElement).hidden =
        !this.sourceMode;
      if (this.sourceMode) {
        this.source.value = this.editor.getMarkdown();
        this.source.hidden = false;
        this.source.focus();
      } else {
        this.editor.commands.setContent(this.source.value, {
          contentType: "markdown",
        });
        this.source.hidden = true;
        this.editor.commands.focus();
      }
      (this.host.querySelector(".editor-canvas") as HTMLElement).hidden =
        this.sourceMode;
    } else {
      const chain = this.editor.chain().focus();
      switch (action) {
        case "table":
          chain.insertTable({ rows: 3, cols: 2, withHeaderRow: true }).run();
          break;
        case "bold":
          chain.toggleBold().run();
          break;
        case "italic":
          chain.toggleItalic().run();
          break;
        case "strike":
          chain.toggleStrike().run();
          break;
        case "bulletList":
          chain.toggleBulletList().run();
          break;
        case "orderedList":
          chain.toggleOrderedList().run();
          break;
        case "taskList":
          chain.toggleTaskList().run();
          break;
        case "blockquote":
          chain.toggleBlockquote().run();
          break;
        case "codeBlock":
          chain.toggleCodeBlock().run();
          break;
        case "undo":
          chain.undo().run();
          break;
        case "redo":
          chain.redo().run();
          break;
      }
    }
    this.updateToolbar();
  }
  private slashCommand(id: string, from: number, to: number) {
    const chain = this.editor.chain().focus().deleteRange({ from, to });
    if (id.startsWith("heading")) {
      chain.setHeading({ level: Number(id.slice(-1)) as 1 | 2 | 3 }).run();
      return;
    }
    switch (id) {
      case "source":
        chain.run();
        this.format("source");
        break;
      case "link":
        chain.run();
        this.openLink();
        break;
      case "paragraph":
        chain.setParagraph().run();
        break;
      case "bulletList":
        chain.toggleBulletList().run();
        break;
      case "orderedList":
        chain.toggleOrderedList().run();
        break;
      case "taskList":
        chain.toggleTaskList().run();
        break;
      case "blockquote":
        chain.toggleBlockquote().run();
        break;
      case "codeBlock":
        chain.toggleCodeBlock().run();
        break;
      case "table":
        chain.insertTable({ rows: 3, cols: 2, withHeaderRow: true }).run();
        break;
      case "divider":
        chain.setHorizontalRule().run();
        break;
      case "keyword":
        chain.setBold().run();
        break;
      case "timestamp": {
        const ms = this.context.timestamp();
        const seconds = Math.floor(ms / 1000);
        const text = [
          Math.floor(seconds / 3600),
          Math.floor((seconds / 60) % 60),
          seconds % 60,
        ]
          .map((n) => String(n).padStart(2, "0"))
          .join(":");
        chain
          .insertContent({
            type: "text",
            text,
            marks: [{ type: "link", attrs: { href: `#time=${ms}` } }],
          })
          .insertContent(" ")
          .run();
        break;
      }
      case "date":
        chain
          .insertContent(
            new Date().toLocaleDateString(t("dateLocale"), {
              year: "numeric",
              month: "long",
              day: "numeric",
            }),
          )
          .run();
        break;
      case "template":
        chain
          .insertContent(t("templateMarkdown"), { contentType: "markdown" })
          .run();
        break;
    }
    this.updateToolbar();
  }
  private openLink() {
    this.slash.hide();
    const dialog = document.createElement("dialog");
    dialog.className = "editor-link-dialog";
    dialog.innerHTML = `<h2>${t("link")}</h2><label>${t("linkAddress")}<input type="url" placeholder="https://" spellcheck="false"></label><div class="modal-actions"><button class="subtle" data-link="remove">${t("removeLink")}</button><button class="subtle" data-link="cancel">${t("cancel")}</button><button class="primary" data-link="save">${t("save")}</button></div>`;
    const input = dialog.querySelector("input")!;
    input.value = this.editor.getAttributes("link").href || "";
    const close = () => {
      dialog.close();
      dialog.remove();
      this.editor.commands.focus();
    };
    dialog.addEventListener("cancel", () => dialog.remove());
    dialog.addEventListener("click", (event) => {
      const action = (event.target as HTMLElement).closest<HTMLElement>(
        "[data-link]",
      )?.dataset.link;
      if (action === "cancel") {
        close();
        return;
      }
      if (action === "remove") {
        this.editor.chain().focus().extendMarkRange("link").unsetLink().run();
        close();
        return;
      }
      if (action === "save") {
        const href = input.value.trim();
        if (!/^(https?:\/\/|mailto:|#time=)/i.test(href)) {
          input.setCustomValidity(t("validLink"));
          input.reportValidity();
          return;
        }
        if (this.editor.state.selection.empty && !this.editor.isActive("link"))
          this.editor
            .chain()
            .focus()
            .insertContent({
              type: "text",
              text: href,
              marks: [{ type: "link", attrs: { href } }],
            })
            .run();
        else
          this.editor
            .chain()
            .focus()
            .extendMarkRange("link")
            .setLink({ href })
            .run();
        close();
      }
    });
    input.addEventListener("input", () => input.setCustomValidity(""));
    document.body.append(dialog);
    dialog.showModal();
    input.focus();
  }
  toggleSource() {
    this.format("source");
  }
  focus() {
    if (this.sourceMode) this.source.focus();
    else this.editor.commands.focus("end");
  }
  destroy() {
    this.blocks.destroy();
    this.slash.destroy();
    this.toolbar.remove();
    this.editor.destroy();
  }
}
