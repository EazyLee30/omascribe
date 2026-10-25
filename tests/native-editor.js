// Real WebView editor acceptance. Run only with an isolated /tmp data directory.
// This creates a fictional meeting; it never starts microphone capture.
(async () => {
  const invoke = (command, args = {}) =>
    window.__TAURI_INTERNALS__.invoke(command, args);
  const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  const check = (ok, message) => {
    if (!ok) throw Error(message);
  };
  const until = async (predicate) => {
    for (let i = 0; i < 120; i++) {
      if (await predicate()) return;
      await sleep(100);
    }
    throw Error("Timed out: " + predicate);
  };
  const click = (selector) => {
    const element = document.querySelector(selector);
    check(element, "Missing " + selector);
    element.click();
  };
  const key = (element, name, extra = {}) =>
    element.dispatchEvent(
      new KeyboardEvent("keydown", {
        key: name,
        bubbles: true,
        cancelable: true,
        ...extra,
      }),
    );
  const checks = [];
  const diagnostics = {};
  try {
    await until(() => document.querySelector('[data-action="new"]'));
    click('[data-action="new"]');
    await sleep(600);
    const id = document.querySelector(".meeting-link.selected").dataset.meeting;
    await invoke("update_meeting", {
      id,
      title: "Example · block editing acceptance",
      autoNotes: false,
    });
    const element = document.querySelector(".tiptap"),
      editor = element.editor;
    check(editor, "Tiptap instance missing");
    editor.commands.setContent(
      "First paragraph.\n\nSecond paragraph.\n\nThird paragraph.",
      { contentType: "markdown" },
    );
    editor.commands.focus("end");
    await sleep(100);
    editor.commands.splitBlock();
    document.execCommand("insertText", false, "/h2");
    await until(() => !document.querySelector(".slash-menu").hidden);
    check(
      document.querySelectorAll(".slash-menu [data-command]").length === 1,
      "Slash search not filtered",
    );
    key(element, "Enter");
    document.execCommand("insertText", false, "Action items");
    await until(() => editor.getMarkdown().includes("Action items"));
    check(
      element.querySelector("h2")?.textContent === "Action items",
      "Slash heading failed",
    );
    const menu = () =>
      key(
        element,
        "/",
        /Mac/.test(navigator.platform) ? { metaKey: true } : { ctrlKey: true },
      );
    menu();
    await until(() => !document.querySelector(".block-menu").hidden);
    click('[data-block-action="up"]');
    check(
      editor.getMarkdown().indexOf("Action items") <
        editor.getMarkdown().indexOf("Third paragraph"),
      "Move block failed",
    );
    diagnostics.beforeUndo = editor.getMarkdown();
    editor.commands.undo();
    diagnostics.afterUndo = editor.getMarkdown();
    check(
      editor.getMarkdown().indexOf("Action items") >
        editor.getMarkdown().indexOf("Third paragraph"),
      "Move undo erased earlier text",
    );
    menu();
    click('[data-block-action="duplicate"]');
    check(element.querySelectorAll("h2").length === 2, "Duplicate failed");
    menu();
    click('[data-block-action="delete"]');
    await until(() => document.querySelector(".confirm-dialog"));
    click('.confirm-dialog [value="cancel"]');
    await until(() => !document.querySelector(".confirm-dialog"));
    check(element.querySelectorAll("h2").length === 2, "Cancel deleted block");
    menu();
    click('[data-block-action="delete"]');
    await until(() => document.querySelector(".confirm-dialog"));
    click('.confirm-dialog [value="delete"]');
    await until(() => element.querySelectorAll("h2").length === 1);
    editor.commands.undo();
    check(
      element.querySelectorAll("h2").length === 2,
      "Delete undo lost block",
    );
    checks.push(
      "Search/Enter, block shortcut, reorder, duplicate, confirmed delete, cancel and independent undo",
    );
    editor.commands.focus("end");
    await sleep(100);
    editor.commands.splitBlock();
    document.execCommand("insertText", false, "/zzzzz");
    await until(() => document.querySelector(".slash-empty"));
    key(element, "Escape");
    check(
      document.querySelector(".slash-menu").hidden,
      "Escape left menu open",
    );
    check(
      editor.getMarkdown().includes("/zzzzz"),
      "Escape removed literal slash",
    );
    document.execCommand("insertText", false, " / path");
    check(
      document.querySelector(".slash-menu").hidden,
      "Inline slash reopened menu",
    );
    element.dispatchEvent(
      new CompositionEvent("compositionstart", { bubbles: true }),
    );
    document.execCommand("insertText", false, "中文输入");
    key(element, "Enter", { isComposing: true });
    element.dispatchEvent(
      new CompositionEvent("compositionend", { bubbles: true }),
    );
    await until(async () =>
      (await invoke("snapshot", { id })).notes.some(
        (n) =>
          n.content.includes("中文输入") &&
          n.content.includes("## Action items"),
      ),
    );
    checks.push(
      "Literal slash, Chinese composition and Markdown autosave through native IPC",
    );
    click('[data-action="workspace-view"]');
    click('[data-layout="meeting"]');
    await sleep(400);
    check(
      document.querySelectorAll(".dv-default-tab-close").length === 0,
      "Panel X still displayed",
    );
    const headers = [...document.querySelectorAll(".dock-group-menu")];
    check(headers.length >= 2, "Dock groups missing");
    headers[1].click();
    check(
      document.querySelector(".dock-menu-hint"),
      "Recovery instructions missing",
    );
    click('[data-place="hide"]');
    click('[data-action="workspace-view"]');
    click('[data-layout="meeting"]');
    await sleep(400);
    check(
      document.querySelector(".tiptap").textContent.includes("中文输入"),
      "Layout restore lost notes",
    );
    await invoke("ui_test_resize", { width: 740, height: 560 });
    await sleep(400);
    check(
      document.documentElement.scrollWidth <= innerWidth,
      "Outer horizontal overflow",
    );
    checks.push(
      "Explicit column hide and layout restore preserve notes; 740×560 stays bounded",
    );
    await invoke("ui_test_resize", { width: 1160, height: 780 });
    await invoke("ui_test_report", { report: { passed: true, checks } });
  } catch (error) {
    await invoke("ui_test_report", {
      report: { passed: false, error: String(error), checks, diagnostics },
    });
  }
})();
