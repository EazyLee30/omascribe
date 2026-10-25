import type { Editor } from "@tiptap/core";
import DragHandle from "@tiptap/extension-drag-handle";
import { Selection } from "@tiptap/pm/state";
import { closeHistory } from "@tiptap/pm/history";
import {
  autoUpdate,
  computePosition,
  flip,
  offset,
  shift,
} from "@floating-ui/dom";
import { uiIcon } from "./ui-icons";
import { esc, tr } from "./workspace-ui";
import { confirmDelete } from "./interactions";

/** Tiptap handles native drag/drop; commands use one undoable document transaction. */
export class BlockControls {
  private handle = document.createElement("div");
  private menu = document.createElement("div");
  private position = -1;
  private menuPosition = -1;
  private dragged = false;
  private stopTracking: (() => void) | undefined;
  private anchor: HTMLElement | null = null;
  private dispose = new AbortController();
  private originalDoc: Editor["state"]["doc"] | null = null;
  extension;
  constructor(
    private editor: () => Editor,
    private picker: (from: number, anchor: HTMLElement) => void,
  ) {
    this.handle.className = "block-handle";
    const mod = /Mac/.test(navigator.platform) ? "⌘" : "Ctrl";
    const add = tr("在下方插入块", "Insert a block below", "在下方插入區塊");
    const move = tr(
      "拖动移动 · 点击打开块菜单",
      "Drag to move · Click for block menu",
      "拖曳移動 · 點擊開啟區塊選單",
    );
    this.handle.innerHTML = `<button type="button" data-block-add draggable="false" title="${add}" aria-label="${add}">${uiIcon("plus", 18)}</button><button type="button" data-block-grip title="${move} · ${mod}/" aria-label="${move}" aria-haspopup="menu">${uiIcon("grip", 18)}</button>`;
    this.handle
      .querySelector("[data-block-add]")!
      .addEventListener("mousedown", (e) => e.preventDefault());
    this.handle.addEventListener("click", (e) => {
      if (this.dragged) return;
      if ((e.target as HTMLElement).closest("[data-block-add]")) this.insert();
      else this.open(this.position);
    });
    this.menu.className = "block-menu";
    this.menu.hidden = true;
    this.menu.setAttribute("role", "menu");
    this.menu.setAttribute(
      "aria-label",
      tr("块操作", "Block actions", "區塊操作"),
    );
    document.body.append(this.menu);
    this.menu.addEventListener("click", (e) => {
      const action = (e.target as HTMLElement).closest<HTMLElement>(
        "[data-block-action]",
      )?.dataset.blockAction;
      if (action) void this.action(action);
    });
    this.menu.addEventListener("keydown", (e) => {
      if (e.key === "Escape") {
        e.preventDefault();
        this.close();
        this.editor().commands.focus();
      }
      if (["ArrowDown", "ArrowUp", "Home", "End"].includes(e.key)) {
        e.preventDefault();
        const buttons = Array.from(
          this.menu.querySelectorAll<HTMLButtonElement>(
            "button:not(:disabled)",
          ),
        );
        const i = buttons.indexOf(document.activeElement as HTMLButtonElement);
        buttons[
          e.key === "Home"
            ? 0
            : e.key === "End"
              ? buttons.length - 1
              : (i + (e.key === "ArrowDown" ? 1 : -1) + buttons.length) %
                buttons.length
        ]?.focus();
      }
    });
    document.addEventListener(
      "pointerdown",
      (e) => {
        if (
          !this.menu.contains(e.target as Node) &&
          !this.handle.contains(e.target as Node)
        )
          this.close();
      },
      { signal: this.dispose.signal },
    );
    window.addEventListener("resize", () => this.close(), {
      signal: this.dispose.signal,
    });
    document.addEventListener(
      "scroll",
      (e) => {
        if (!this.menu.contains(e.target as Node)) this.close();
      },
      { capture: true, signal: this.dispose.signal },
    );
    this.extension = DragHandle.configure({
      render: () => this.handle,
      computePositionConfig: {
        placement: "left-start",
        strategy: "absolute",
        middleware: [offset(4)],
      },
      onNodeChange: ({ node, editor }) => {
        this.position = -1;
        if (node)
          editor.state.doc.forEach((child, pos) => {
            if (child === node) this.position = pos;
          });
        this.track(
          this.position >= 0
            ? (editor.view.nodeDOM(this.position) as HTMLElement)
            : null,
        );
      },
      onElementDragStart: () => {
        this.dragged = true;
        this.close();
        this.historyBoundary();
      },
      onElementDragEnd: () => {
        this.historyBoundary();
        setTimeout(() => (this.dragged = false), 150);
      },
    });
  }
  private track(anchor: HTMLElement | null) {
    if (anchor === this.anchor) return;
    this.stopTracking?.();
    this.anchor = anchor;
    // The handle lives in the document's scroll container, not the viewport.
    // Floating UI also follows reflow when columns or the window are resized.
    this.stopTracking = anchor
      ? autoUpdate(anchor, this.handle, () => {
          if (this.dragged) return;
          void computePosition(anchor, this.handle, {
            strategy: "absolute",
            placement: "left-start",
            middleware: [offset(4)],
          }).then(({ x, y, strategy }) => {
            if (this.anchor === anchor && anchor.isConnected)
              Object.assign(this.handle.style, {
                position: strategy,
                left: `${x}px`,
                top: `${y}px`,
              });
          });
        })
      : undefined;
  }
  private historyBoundary() {
    const editor = this.editor();
    editor.view.dispatch(closeHistory(editor.state.tr));
  }
  private select(pos: number) {
    const editor = this.editor();
    editor.view.dispatch(
      editor.state.tr.setSelection(
        Selection.near(editor.state.doc.resolve(pos + 1)),
      ),
    );
    editor.commands.focus();
  }
  private insert() {
    const editor = this.editor(),
      node = editor.state.doc.nodeAt(this.position);
    if (!node) return;
    this.close();
    this.historyBoundary();
    if (node.type.name === "paragraph" && !node.content.size) {
      editor
        .chain()
        .focus()
        .setTextSelection(this.position + 1)
        .insertContent("/")
        .run();
    } else {
      const at = this.position + node.nodeSize;
      editor
        .chain()
        .focus()
        .insertContentAt(at, {
          type: "paragraph",
          content: [{ type: "text", text: "/" }],
        })
        .setTextSelection(at + 2)
        .run();
    }
  }
  openAtSelection() {
    const { $from } = this.editor().state.selection;
    this.open($from.depth ? $from.before(1) : $from.pos);
  }
  private open(pos: number) {
    const editor = this.editor(),
      node = editor.state.doc.nodeAt(pos);
    if (!node) return;
    this.menuPosition = pos;
    this.originalDoc = editor.state.doc;
    this.select(pos);
    editor.commands.lockDragHandle();
    const actions: [string, Parameters<typeof uiIcon>[0], string, boolean][] = [
      [
        "turn",
        "text",
        tr("转换为…", "Turn into…", "轉換為…"),
        !node.isTextblock,
      ],
      ["duplicate", "copy", tr("创建副本", "Duplicate", "建立副本"), false],
      ["up", "up", tr("上移", "Move up", "上移"), pos === 0],
      [
        "down",
        "down",
        tr("下移", "Move down", "下移"),
        pos + node.nodeSize === editor.state.doc.content.size,
      ],
      ["delete", "trash", tr("删除块…", "Delete block…", "刪除區塊…"), false],
    ];
    this.menu.innerHTML = `<div class="block-menu-label">${tr("块操作", "Block actions", "區塊操作")}</div>${actions.map(([id, icon, label, disabled]) => `<button role="menuitem" type="button" data-block-action="${id}" ${disabled ? "disabled" : ""}>${uiIcon(icon)}<span>${esc(label)}</span></button>`).join("")}`;
    this.menu.hidden = false;
    const dom = editor.view.nodeDOM(pos) as HTMLElement;
    void computePosition(dom, this.menu, {
      strategy: "fixed",
      placement: "bottom-start",
      middleware: [offset(5), flip(), shift({ padding: 10 })],
    }).then(({ x, y }) => {
      if (!this.menu.hidden)
        Object.assign(this.menu.style, { left: `${x}px`, top: `${y}px` });
    });
    this.menu
      .querySelector<HTMLButtonElement>("button:not(:disabled)")
      ?.focus();
  }
  private async action(action: string) {
    const editor = this.editor(),
      pos = this.menuPosition;
    const doc = this.originalDoc,
      node = editor.state.doc.nodeAt(pos);
    if (!node || doc !== editor.state.doc) {
      this.close();
      return;
    }
    if (action === "turn") {
      const anchor = editor.view.nodeDOM(pos) as HTMLElement;
      this.close();
      this.select(pos);
      this.picker(editor.state.selection.from, anchor);
      return;
    }
    this.close();
    if (action === "delete") {
      const confirmed = await confirmDelete(
        tr("删除这个块？", "Delete this block?", "刪除這個區塊？"),
        tr(
          "删除后可用撤销恢复。",
          "You can undo this change.",
          "刪除後可用復原還原。",
        ),
      );
      if (!confirmed || editor.isDestroyed || editor.state.doc !== doc) return;
      this.historyBoundary();
      editor
        .chain()
        .focus()
        .deleteRange({ from: pos, to: pos + node.nodeSize })
        .run();
      this.historyBoundary();
      return;
    }
    this.historyBoundary();
    if (action === "duplicate") {
      editor
        .chain()
        .focus()
        .insertContentAt(pos + node.nodeSize, node.toJSON())
        .run();
      this.historyBoundary();
      return;
    }
    const siblings: { pos: number; size: number }[] = [];
    editor.state.doc.forEach((child, p) =>
      siblings.push({ pos: p, size: child.nodeSize }),
    );
    const index = siblings.findIndex((s) => s.pos === pos),
      target = siblings[index + (action === "up" ? -1 : 1)];
    if (!target) return;
    const at =
      action === "up" ? target.pos : target.pos + target.size - node.nodeSize;
    const transaction = editor.state.tr
      .delete(pos, pos + node.nodeSize)
      .insert(at, node);
    transaction.setSelection(Selection.near(transaction.doc.resolve(at + 1)));
    editor.view.dispatch(transaction.scrollIntoView());
    this.historyBoundary();
    editor.commands.focus();
  }
  close() {
    if (this.menu.hidden) return;
    this.menu.hidden = true;
    this.menuPosition = -1;
    this.originalDoc = null;
    if (!this.editor().isDestroyed) this.editor().commands.unlockDragHandle();
  }
  destroy() {
    this.dispose.abort();
    this.stopTracking?.();
    this.anchor = null;
    this.close();
    this.menu.remove();
  }
}
