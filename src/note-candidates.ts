/** Short, explicit note lines are useful as manually selected spelling candidates.
 * These are not facts about what was spoken and are never applied automatically.
 */
export function noteCandidates(
  content: string,
  focusTerms: string[] = [],
): string[] {
  const result: string[] = [];
  const seen = new Set<string>();
  const add = (value: string) => {
    const candidate = value.trim();
    if (!candidate || candidate.length > 80 || /[\r\n]/.test(candidate)) return;
    const key = candidate.toLocaleLowerCase();
    if (seen.has(key) || result.length >= 24) return;
    seen.add(key);
    result.push(candidate);
  };
  for (const line of content.split(/\r?\n/)) {
    const candidate = line
      .trim()
      .replace(/^(?:#{1,6}\s+|[-*+]\s+|\d+[.)]\s+)/, "")
      .replace(/^\*\*(.+)\*\*$/, "$1")
      .trim();
    // Keep explicit short lines, not a whole paragraph or markdown markup.
    if (
      candidate.length >= 2 &&
      candidate.length <= 40 &&
      !/[。！？.!?;；]/.test(candidate)
    )
      add(candidate);
  }
  for (const term of focusTerms) add(term);
  return result;
}

/** Only changes the unsaved textarea; the normal save action owns the revision. */
export function renderNoteCandidates(
  root: HTMLElement,
  input: HTMLTextAreaElement,
  candidates: string[],
  help: string,
): void {
  root.replaceChildren();
  root.hidden = candidates.length === 0;
  if (!candidates.length) return;
  const description = document.createElement("p");
  description.textContent = help;
  const list = document.createElement("div");
  list.className = "edit-note-term-list";
  for (const word of candidates) {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "outline";
    button.textContent = word;
    button.addEventListener("mousedown", (event) => event.preventDefault());
    button.addEventListener("click", () => {
      input.setRangeText(word, input.selectionStart, input.selectionEnd, "end");
      input.dispatchEvent(new Event("input", { bubbles: true }));
      input.focus();
    });
    list.append(button);
  }
  root.append(description, list);
}
