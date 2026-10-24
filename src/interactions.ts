import { animate } from "motion/mini";
import { esc, tr } from "./workspace-ui";

export const reduceMotion = () =>
  matchMedia("(prefers-reduced-motion: reduce)").matches;
export function reveal(element: HTMLElement) {
  if (!reduceMotion())
    animate(
      Array.from(element.children),
      {
        opacity: [0.65, 1],
        transform: ["translateY(5px)", "translateY(0)"],
      },
      { duration: 0.22, ease: "easeOut" },
    );
}
/** One shared, keyboard-accessible confirmation for all destructive actions. */
export function confirmDelete(
  title: string,
  detail: string,
  label = tr("删除", "Delete", "刪除"),
): Promise<boolean> {
  if (document.querySelector(".confirm-dialog")) return Promise.resolve(false);
  return new Promise((resolve) => {
    const dialog = document.createElement("dialog");
    dialog.className = "confirm-dialog";
    dialog.setAttribute("aria-labelledby", "confirm-delete-title");
    dialog.innerHTML = `<h2 id="confirm-delete-title">${esc(title)}</h2><p>${esc(detail)}</p><form method="dialog"><div class="modal-actions"><button class="outline" value="cancel" autofocus>${tr("取消", "Cancel")}</button><button class="danger" value="delete">${esc(label)}</button></div></form>`;
    dialog.addEventListener(
      "close",
      () => {
        const accepted = dialog.returnValue === "delete";
        dialog.remove();
        resolve(accepted);
      },
      { once: true },
    );
    document.body.append(dialog);
    dialog.showModal();
    reveal(dialog);
  });
}

export function recordingMotion(
  root: HTMLElement,
  state: string,
  level: number,
) {
  root.dataset.state = state;
  root.querySelectorAll<HTMLElement>(".recording-wave i").forEach((bar, i) => {
    const height =
      state === "recording"
        ? 0.16 +
          Math.min(1, level * 7) * (0.4 + 0.6 * Math.abs(Math.sin(i * 1.9)))
        : 0.16;
    if (reduceMotion()) bar.style.transform = `scaleY(${height})`;
    else
      animate(
        bar,
        { transform: `scaleY(${height})` },
        { duration: 0.32, ease: "easeOut" },
      );
  });
}
