import { getCurrentWindow } from "@tauri-apps/api/window";
import { PhysicalPosition } from "@tauri-apps/api/dpi";

export const macDesktop = /Mac/.test(navigator.platform);

/** Pointer capture keeps click and drag separate; macOS avoids queued native drag events. */
export function bindWindowDrag(
  element: HTMLElement,
  error: (e: unknown) => void,
  interactive = false,
) {
  const window = getCurrentWindow();
  let gesture: {
    x: number;
    y: number;
    origin?: PhysicalPosition;
    scale?: number;
  } | null = null;
  let moved = false,
    dx = 0,
    dy = 0,
    queued = false;
  const queue = () => {
    if (queued || !gesture?.origin || !gesture.scale) return;
    queued = true;
    requestAnimationFrame(async () => {
      const g = gesture;
      const x = dx,
        y = dy;
      try {
        if (g?.origin && g.scale && element.isConnected)
          await window.setPosition(
            new PhysicalPosition(
              Math.round(g.origin.x + x * g.scale),
              Math.round(g.origin.y + y * g.scale),
            ),
          );
      } catch (e) {
        error(e);
      } finally {
        queued = false;
        if (gesture && (x !== dx || y !== dy)) queue();
      }
    });
  };
  element.addEventListener("pointerdown", (e) => {
    if (
      e.button > 0 ||
      (interactive &&
        (e.target as HTMLElement).closest(
          "button,input,select,a,summary,[role='tab'],[contenteditable='true']",
        ))
    )
      return;
    e.preventDefault();
    try {
      element.setPointerCapture(e.pointerId);
    } catch {
      /* Assistive inputs may not expose an active pointer. */
    }
    const g = { x: e.screenX, y: e.screenY } as NonNullable<typeof gesture>;
    gesture = g;
    moved = false;
    dx = dy = 0;
    if (macDesktop)
      Promise.all([window.outerPosition(), window.scaleFactor()])
        .then(([origin, scale]) => {
          if (gesture !== g) return;
          g.origin = origin;
          g.scale = scale;
          if (moved) queue();
        })
        .catch(error);
  });
  element.addEventListener("pointermove", (e) => {
    if (!gesture) return;
    dx = e.screenX - gesture.x;
    dy = e.screenY - gesture.y;
    if (!moved && Math.hypot(dx, dy) < 5) return;
    moved = true;
    if (macDesktop) queue();
    else {
      gesture = null;
      if (element.hasPointerCapture(e.pointerId))
        element.releasePointerCapture(e.pointerId);
      window.startDragging().catch(error);
    }
  });
  element.addEventListener("pointerup", (e) => {
    const g = gesture;
    if (moved && macDesktop && g?.origin && g.scale)
      window
        .setPosition(
          new PhysicalPosition(
            Math.round(g.origin.x + dx * g.scale),
            Math.round(g.origin.y + dy * g.scale),
          ),
        )
        .catch(error);
    gesture = null;
    if (element.hasPointerCapture(e.pointerId))
      element.releasePointerCapture(e.pointerId);
  });
  element.addEventListener("pointercancel", () => {
    gesture = null;
  });
  element.addEventListener("keydown", () => {
    moved = false;
  });
  return () => {
    const result = moved;
    moved = false;
    return result;
  };
}
