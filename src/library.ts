import { invoke } from "@tauri-apps/api/core";
import { esc, tr, glyph, navItems, type PageHandle } from "./workspace-ui";
export interface LibraryEntry {
  id: string;
  kind: string;
  title: string;
  content: string;
  aliases: string;
  enabled: boolean;
  knowledge: boolean;
  deleted: boolean;
  meeting_id: string | null;
  updated_at: string;
}
export type LibraryPage = "notes" | "knowledge" | "terms" | "memory";
interface Options {
  toast: (s: string, error?: boolean) => void;
  meeting: (id: string) => Promise<void>;
  settings: () => Promise<void>;
}
export async function mountLibrary(
  root: HTMLElement,
  page: LibraryPage,
  options: Options,
): Promise<PageHandle> {
  let entries = await invoke<LibraryEntry[]>("list_library"),
    selected: LibraryEntry | null = null,
    query = "",
    trash = false,
    alive = true;
  let pending: ReturnType<typeof setTimeout> | undefined,
    queue = Promise.resolve(),
    dirty = false;
  root.classList.add("workspace-page");
  const title = navItems().find(([id]) => id === page)![1];
  const help = {
    notes: tr(
      "你的会议笔记与独立笔记。由你决定哪些内容加入知识库。",
      "Meeting notes and standalone notes. You decide what belongs in the knowledge base.",
      "你的會議及獨立筆記，由你決定哪些加入知識庫。",
    ),
    knowledge: tr(
      "只有明确加入的笔记才会出现在这里；在设置中启用后供本地纪要检索。",
      "Only explicitly added notes appear here. Enable retrieval in settings to provide context to the local writer.",
      "僅顯示明確加入的筆記；於設定中啟用後供本機紀要檢索。",
    ),
    terms: tr(
      "维护人名、产品名和缩写。已启用的词条用于纪要表达，原始转写保留。",
      "Keep names, product terms, and abbreviations consistent in minutes. Original transcripts stay intact.",
      "維護人名、產品名與縮寫，供紀要參考並保留轉寫原文。",
    ),
    memory: tr(
      "保存写作偏好与工作背景。由你手动维护，随时停用或删除。",
      "Save writing preferences and work context. You control what is remembered and used.",
      "儲存寫作偏好及工作背景，由你手動維護，隨時停用或刪除。",
    ),
  }[page];
  root.innerHTML = `<div class="library-page"><header class="page-heading library-heading"><div><span class="eyebrow">OMASCRIBE / LOCAL</span><h1>${glyph(page)}${title}</h1><p>${help}</p></div><button class="primary" data-library="new">${glyph("plus")}${tr("新建", "New", "新增")}</button></header><div class="library-tools"><label class="page-search">${glyph("search")}<input id="library-search" placeholder="${tr("搜索标题与内容", "Search title and content", "搜尋標題及內容")}" aria-label="${tr("搜索", "Search", "搜尋")}"></label><button class="outline" data-library="trash" aria-pressed="false">${glyph("trash")}${tr("已删除", "Deleted", "已刪除")}</button>${page === "knowledge" || page === "memory" ? `<button class="subtle" data-library="settings">${glyph("general")}${tr("使用设置", "Usage settings", "使用設定")}</button>` : ""}</div><div class="library-split"><nav id="library-list" aria-label="${title}"></nav><section id="library-editor"></section></div></div>`;
  const list = root.querySelector<HTMLElement>("#library-list")!,
    editor = root.querySelector<HTMLElement>("#library-editor")!;
  function visible(e: LibraryEntry) {
    return (
      e.deleted === trash &&
      (page === "knowledge"
        ? e.kind === "note" && e.knowledge
        : e.kind ===
          (
            { notes: "note", terms: "term", memory: "memory" } as Record<
              string,
              string
            >
          )[page]) &&
      `${e.title} ${e.content} ${e.aliases}`.toLowerCase().includes(query)
    );
  }
  function renderList() {
    list.innerHTML =
      entries
        .filter(visible)
        .map(
          (e) =>
            `<button class="library-entry ${e.id === selected?.id ? "selected" : ""}" data-entry="${e.id}"><span>${esc(e.title || tr("无标题", "Untitled", "無標題"))}</span><p>${esc(e.content.slice(0, 90))}</p><small>${e.meeting_id ? tr("会议笔记", "Meeting note", "會議筆記") : e.kind === "term" ? tr("术语", "Term", "術語") : e.kind === "memory" ? tr("记忆", "Memory", "記憶") : tr("独立笔记", "Standalone note", "獨立筆記")} ${e.knowledge ? " · " + tr("知识库", "Knowledge base", "知識庫") : ""}${!e.enabled ? " · " + tr("已停用", "Disabled", "已停用") : ""}</small></button>`,
        )
        .join("") ||
      `<div class="library-empty">${glyph(page)}<p>${query ? tr("没有匹配内容", "No matches", "沒有符合的內容") : trash ? tr("没有已删除内容", "No deleted entries", "沒有已刪除內容") : tr("这里还没有内容", "Nothing here yet", "這裡還沒有內容")}</p></div>`;
  }
  function renderEditor() {
    if (!selected) {
      editor.innerHTML = `<div class="library-empty">${glyph(page)}<h2>${tr("留住有用的信息", "Keep useful information", "留下有用的資訊")}</h2><p>${page === "knowledge" ? tr("在笔记中勾选「加入知识库」，或在这里新建。", "Mark a note “Add to knowledge base”, or create one here.", "在筆記勾選「加入知識庫」，或在這裡新增。") : tr("选择一条内容，或新建。", "Select an entry or create one.", "選擇一項內容或新增。")}</p></div>`;
      return;
    }
    const e = selected;
    editor.innerHTML = `<div class="entry-toolbar"><span id="library-save-state" role="status">${tr("已保存在本地", "Saved locally", "已儲存於本機")}</span><div>${e.meeting_id ? `<button class="text-button" data-library="meeting">${tr("打开会议", "Open meeting", "開啟會議")}</button>` : ""}<button class="subtle" data-library="${e.deleted ? "restore" : "delete"}">${glyph(e.deleted ? "back" : "trash")}${e.deleted ? tr("恢复", "Restore", "還原") : tr("删除", "Delete", "刪除")}</button></div></div><input class="entry-title" id="entry-title" maxlength="160" value="${esc(e.title)}" placeholder="${e.kind === "term" ? tr("规范名称", "Canonical term", "標準名稱") : tr("标题", "Title", "標題")}" aria-label="${tr("标题", "Title", "標題")}" ${e.deleted ? "disabled" : ""}>${e.kind === "term" ? `<label class="entry-aliases">${tr("别名 / 缩写 / 常见误写", "Aliases / abbreviations / common misspellings", "別名 / 縮寫 / 常見誤寫")}<input id="entry-aliases" value="${esc(e.aliases)}" maxlength="2000" ${e.deleted ? "disabled" : ""}></label>` : ""}<textarea id="entry-content" aria-label="${tr("内容", "Content", "內容")}" maxlength="200000" placeholder="${e.kind === "term" ? tr("解释、用法或补充说明…", "Definition, usage, or context…", "解釋、用法或補充說明…") : e.kind === "memory" ? tr("例如：会议纪要使用简洁的书面语，保留待确认事项。", "For example: Use concise prose and preserve unresolved questions.", "例如：紀要使用簡潔書面語，保留待確認事項。") : tr("开始写笔记，支持 Markdown…", "Start writing in Markdown…", "開始寫筆記，支援 Markdown…")}" ${e.deleted ? "disabled" : ""}>${esc(e.content)}</textarea><div class="entry-options">${e.kind === "note" ? `<label><input type="checkbox" id="entry-knowledge" ${e.knowledge ? "checked" : ""} ${e.deleted ? "disabled" : ""}>${tr("加入知识库", "Add to knowledge base", "加入知識庫")}</label>` : ""}<label><input type="checkbox" id="entry-enabled" ${e.enabled ? "checked" : ""} ${e.deleted ? "disabled" : ""}>${e.kind === "note" ? tr("允许检索", "Allow retrieval", "允許檢索") : tr("启用", "Enabled", "啟用")}</label><button class="outline" data-library="save" ${e.deleted ? "disabled" : ""}>${tr("保存", "Save", "儲存")}</button></div>`;
  }
  function readEditor() {
    if (!selected || selected.deleted) return;
    selected.title = (
      root.querySelector("#entry-title") as HTMLInputElement
    ).value;
    selected.content = (
      root.querySelector("#entry-content") as HTMLTextAreaElement
    ).value;
    selected.aliases =
      (root.querySelector("#entry-aliases") as HTMLInputElement | null)
        ?.value || "";
    selected.enabled = (
      root.querySelector("#entry-enabled") as HTMLInputElement
    ).checked;
    selected.knowledge =
      (root.querySelector("#entry-knowledge") as HTMLInputElement | null)
        ?.checked || false;
    dirty = true;
    const status = root.querySelector("#library-save-state");
    if (status) status.textContent = tr("正在保存…", "Saving…", "正在儲存…");
  }
  async function flush() {
    clearTimeout(pending);
    if (!dirty || !selected) return queue;
    const entry = structuredClone(selected);
    dirty = false;
    queue = queue
      .then(async () => {
        const saved = await invoke<LibraryEntry>("save_library_entry", {
          entry,
        });
        const i = entries.findIndex((e) => e.id === saved.id);
        if (i >= 0) entries[i] = saved;
        else entries.unshift(saved);
        if (selected?.id === saved.id) {
          selected.updated_at = saved.updated_at;
          const status = root.querySelector("#library-save-state");
          if (status && !dirty)
            status.textContent = tr(
              "已保存在本地",
              "Saved locally",
              "已儲存於本機",
            );
        }
        if (alive) renderList();
      })
      .catch((e) => {
        dirty = true;
        options.toast(String(e), true);
        throw e;
      });
    try {
      await queue;
    } catch (e) {
      queue = Promise.resolve();
      throw e;
    }
  }
  async function action(event: Event) {
    const b = (event.target as HTMLElement).closest<HTMLElement>("button");
    if (!b) return;
    if (b.dataset.entry) {
      await flush();
      selected = structuredClone(
        entries.find((e) => e.id === b.dataset.entry)!,
      );
      renderList();
      renderEditor();
      return;
    }
    switch (b.dataset.library) {
      case "new":
        await flush();
        trash = false;
        selected = {
          id: crypto.randomUUID(),
          kind:
            page === "terms" ? "term" : page === "memory" ? "memory" : "note",
          title: "",
          content: "",
          aliases: "",
          enabled: true,
          knowledge: page === "knowledge",
          deleted: false,
          meeting_id: null,
          updated_at: new Date().toISOString(),
        };
        dirty = true;
        await flush();
        renderList();
        renderEditor();
        (root.querySelector("#entry-title") as HTMLInputElement).focus();
        break;
      case "save":
        readEditor();
        await flush();
        break;
      case "delete":
      case "restore":
        await flush();
        if (selected) {
          selected.deleted = b.dataset.library === "delete";
          dirty = true;
          await flush();
          selected = null;
          renderList();
          renderEditor();
        }
        break;
      case "trash":
        await flush();
        trash = !trash;
        b.setAttribute("aria-pressed", String(trash));
        selected = null;
        renderList();
        renderEditor();
        break;
      case "meeting":
        await flush();
        if (selected?.meeting_id) await options.meeting(selected.meeting_id);
        break;
      case "settings":
        await flush();
        await options.settings();
        break;
    }
  }
  const onClick = (e: Event) =>
    void action(e).catch((e) => options.toast(String(e), true));
  const onInput = (e: Event) => {
    const input = e.target as HTMLInputElement;
    if (input.id === "library-search") {
      query = input.value.trim().toLowerCase();
      renderList();
      return;
    }
    if (input.id.startsWith("entry-")) {
      readEditor();
      clearTimeout(pending);
      pending = setTimeout(() => void flush().catch(() => {}), 400);
    }
  };
  root.addEventListener("click", onClick);
  root.addEventListener("input", onInput);
  renderList();
  renderEditor();
  return {
    flush,
    async close() {
      await flush();
      alive = false;
      root.removeEventListener("click", onClick);
      root.removeEventListener("input", onInput);
      root.classList.remove("workspace-page");
    },
  };
}
